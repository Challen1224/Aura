//! Executable memory management for the JIT.
//!
//! Uses raw Linux x86-64 syscalls (`mmap`/`mprotect`/`munmap`) so the VM has
//! no dependency on `libc`. Memory is allocated writable, populated with
//! machine code, and then marked read+execute (W^X) before being called.

use std::fmt;
use std::ptr;

/// `mmap` syscall number on Linux x86-64.
const SYS_MMAP: u64 = 9;
/// `mprotect` syscall number on Linux x86-64.
const SYS_MPROTECT: u64 = 10;
/// `munmap` syscall number on Linux x86-64.
const SYS_MUNMAP: u64 = 11;

const PROT_READ: u64 = 0x1;
const PROT_WRITE: u64 = 0x2;
const PROT_EXEC: u64 = 0x4;
const MAP_PRIVATE: u64 = 0x02;
const MAP_ANONYMOUS: u64 = 0x20;

/// Page size on x86-64 Linux.
pub const PAGE_SIZE: usize = 4096;

/// Round `n` up to the nearest multiple of [`PAGE_SIZE`].
pub fn page_align_up(n: usize) -> usize {
    (n + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Raw `mmap` syscall. Returns the mapped address, or an `Err` containing the
/// kernel errno when the mapping fails.
unsafe fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> Result<usize, i64> {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_MMAP => ret,
        in("rdi") addr,
        in("rsi") len,
        in("rdx") prot,
        in("r10") flags,
        in("r8") fd,
        in("r9") off,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    let signed = ret as i64;
    if (-4095..0).contains(&signed) {
        Err(signed)
    } else {
        Ok(ret as usize)
    }
}

/// Raw `mprotect` syscall.
unsafe fn sys_mprotect(addr: usize, len: usize, prot: u64) -> Result<(), i64> {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_MPROTECT => ret,
        in("rdi") addr as u64,
        in("rsi") len as u64,
        in("rdx") prot,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    let r = ret as i64;
    if r < 0 && r > -4095 {
        Err(r)
    } else {
        Ok(())
    }
}

/// Raw `munmap` syscall.
unsafe fn sys_munmap(addr: usize, len: usize) -> Result<(), i64> {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_MUNMAP => ret,
        in("rdi") addr as u64,
        in("rsi") len as u64,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    let r = ret as i64;
    if r < 0 && r > -4095 {
        Err(r)
    } else {
        Ok(())
    }
}

/// Error produced when managing executable memory fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryError(pub String);

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "jit memory error: {}", self.0)
    }
}

impl std::error::Error for MemoryError {}

/// A page-aligned region of executable memory.
///
/// The region is writable until [`ExecutableMemory::freeze`] is called, which
/// transitions it to read+execute and makes further writes impossible. The
/// mapping is released on drop.
pub struct ExecutableMemory {
    ptr: *mut u8,
    len: usize,
    frozen: bool,
}

impl ExecutableMemory {
    /// Allocate a fresh page-aligned region of at least `size` bytes, writable.
    pub fn allocate(size: usize) -> Result<Self, MemoryError> {
        let len = page_align_up(size);
        let ptr = unsafe {
            sys_mmap(0, len as u64, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, u64::MAX, 0)
                .map_err(|e| MemoryError(format!("mmap failed with errno {e}")))?
        };
        Ok(Self {
            ptr: ptr as *mut u8,
            len,
            frozen: false,
        })
    }

    /// Write the given code bytes into the region, then mark it read+execute.
    /// After this call the region must not be written again.
    pub fn finalize(&mut self, code: &[u8]) -> Result<(), MemoryError> {
        if self.frozen {
            return Err(MemoryError("region already frozen".into()));
        }
        if code.len() > self.len {
            return Err(MemoryError(format!(
                "code of {} bytes exceeds region of {} bytes",
                code.len(),
                self.len
            )));
        }
        unsafe {
            ptr::copy_nonoverlapping(code.as_ptr(), self.ptr, code.len());
        }
        unsafe {
            sys_mprotect(self.ptr as usize, self.len, PROT_READ | PROT_EXEC)
                .map_err(|e| MemoryError(format!("mprotect failed with errno {e}")))?;
        }
        self.frozen = true;
        Ok(())
    }

    /// Address of the start of the executable region.
    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Address of the start of the executable region as a raw code pointer.
    pub fn code_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Byte length of the region (page-aligned).
    pub fn len(&self) -> usize {
        self.len
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        unsafe {
            let _ = sys_munmap(self.ptr as usize, self.len);
        }
    }
}

/// An executable function produced by the JIT.
pub struct JitFunction {
    memory: ExecutableMemory,
}

impl JitFunction {
    /// Create a JIT function from already-assembled code bytes.
    pub fn new(code: Vec<u8>) -> Result<Self, MemoryError> {
        let mut memory = ExecutableMemory::allocate(code.len())?;
        memory.finalize(&code)?;
        if let Ok(p) = std::env::var("AURA_DUMP_JIT_PTR") {
            eprintln!("JIT code ptr = {:#x}", memory.ptr() as usize);
        }
        Ok(Self { memory })
    }

    /// Call the function with the given pointer-sized arguments (SysV x86-64:
    /// up to six 64-bit arguments in `rdi, rsi, rdx, rcx, r8, r9`).
    ///
    /// # Safety
    /// The caller must pass arguments matching the function's expected ABI and
    /// ensure the function was compiled correctly.
    pub unsafe fn call_abi(&self, args: &[u64]) -> u64 {
        let entry: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 =
            std::mem::transmute(self.memory.ptr());
        let mut a = [0u64; 6];
        for (dst, src) in a.iter_mut().zip(args.iter()) {
            *dst = *src;
        }
        entry(a[0], a[1], a[2], a[3], a[4], a[5])
    }
}

impl fmt::Debug for JitFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JitFunction")
            .field("ptr", &self.memory.ptr())
            .field("len", &self.memory.len())
            .finish()
    }
}
