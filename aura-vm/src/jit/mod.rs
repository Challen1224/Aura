//! JIT compilation for the Aura VM.
//!
//! Tiered execution: methods run interpreted until they pass an invocation
//! threshold, at which point the compiler lowers their bytecode to an SSA-like
//! IR, runs optimization passes, performs linear-scan register allocation, and
//! emits x86-64 machine code. Complex operations (heap allocation, calls, field
//! access, exceptions) are delegated to VM helper stubs so the JIT shares the
//! interpreter's semantics.
//!
//! The JIT is currently x86-64 only; on other architectures [`Jit`] is a stub
//! that reports [`CompileError::Unsupported`].

use aura_bytecode::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// The method uses a construct the JIT cannot lower.
    Unsupported(String),
    /// Failed to allocate executable memory.
    OutOfMemory,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Unsupported(msg) => write!(f, "jit: unsupported: {msg}"),
            CompileError::OutOfMemory => write!(f, "jit: out of executable memory"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Result of running a compiled method on the native stack.
#[derive(Debug, Clone)]
pub enum FrameResult {
    /// Method returned normally with this value.
    Normal(Value),
    /// Method threw; the value is the exception.
    Exception(Value),
}

#[cfg(target_arch = "x86_64")]
mod x64 {
    pub mod asm;
    pub mod mem;
    pub mod regalloc;
    pub mod codegen;
    pub mod helpers;
}
#[cfg(target_arch = "x86_64")]
pub use x64::{asm, codegen, helpers, mem, regalloc};

/// Architecture-independent IR: lowering, CFG construction and optimization.
pub mod ir;

#[cfg(target_arch = "x86_64")]
pub mod value;

#[cfg(target_arch = "x86_64")]
mod compiler;
#[cfg(target_arch = "x86_64")]
pub use compiler::{JIT_THRESHOLD, CompiledMethod, Jit};

#[cfg(not(target_arch = "x86_64"))]
mod stub;
#[cfg(not(target_arch = "x86_64"))]
pub use stub::{JIT_THRESHOLD, CompiledMethod, Jit};
