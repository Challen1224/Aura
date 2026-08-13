//! A small x86-64 assembler for the JIT.
//!
//! Emits bytes for the subset of the ISA the baseline/optimizing JIT needs:
//! integer moves/ALU, `imul`/`idiv`, shifts, compares, condition codes, jumps
//! and calls with label fixups, plus SSE scalar floating point. Memory
//! operands support `[base + disp]`. The System V AMD64 calling convention is
//! used for helper calls.

use std::collections::HashMap;

/// General purpose register (64-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpr(pub u8);

/// XMM floating point register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xmm(pub u8);

// General purpose registers.
#[allow(non_upper_case_globals)]
pub mod regs {
    use super::Gpr;
    pub const RAX: Gpr = Gpr(0);
    pub const RCX: Gpr = Gpr(1);
    pub const RDX: Gpr = Gpr(2);
    pub const RBX: Gpr = Gpr(3);
    pub const RSP: Gpr = Gpr(4);
    pub const RBP: Gpr = Gpr(5);
    pub const RSI: Gpr = Gpr(6);
    pub const RDI: Gpr = Gpr(7);
    pub const R8: Gpr = Gpr(8);
    pub const R9: Gpr = Gpr(9);
    pub const R10: Gpr = Gpr(10);
    pub const R11: Gpr = Gpr(11);
    pub const R12: Gpr = Gpr(12);
    pub const R13: Gpr = Gpr(13);
    pub const R14: Gpr = Gpr(14);
    pub const R15: Gpr = Gpr(15);
}

// XMM registers.
#[allow(non_upper_case_globals)]
pub mod xmms {
    use super::Xmm;
    pub const XMM0: Xmm = Xmm(0);
    pub const XMM1: Xmm = Xmm(1);
    pub const XMM2: Xmm = Xmm(2);
    pub const XMM3: Xmm = Xmm(3);
    pub const XMM4: Xmm = Xmm(4);
    pub const XMM5: Xmm = Xmm(5);
    pub const XMM6: Xmm = Xmm(6);
    pub const XMM7: Xmm = Xmm(7);
    pub const XMM8: Xmm = Xmm(8);
    pub const XMM9: Xmm = Xmm(9);
    pub const XMM10: Xmm = Xmm(10);
    pub const XMM11: Xmm = Xmm(11);
    pub const XMM12: Xmm = Xmm(12);
    pub const XMM13: Xmm = Xmm(13);
    pub const XMM14: Xmm = Xmm(14);
    pub const XMM15: Xmm = Xmm(15);
}

/// Memory operand `[base + disp]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mem {
    /// Base register.
    pub base: Gpr,
    /// Signed displacement.
    pub disp: i32,
}

impl Mem {
    /// `[base + disp]`.
    pub fn new(base: Gpr, disp: i32) -> Self {
        Self { base, disp }
    }
}

/// A location that can appear as a source/destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// General purpose register.
    Gpr(Gpr),
    /// Memory operand.
    Mem(Mem),
}

/// A jump/call target that gets resolved by [`Asm::bind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(usize);

/// Size of the relocation for a label reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixupKind {
    /// 8-bit relative offset.
    Rel8,
    /// 32-bit relative offset.
    Rel32,
}

#[derive(Debug, Clone, Copy)]
struct Fixup {
    offset: usize,
    label: usize,
    kind: FixupKind,
}

/// Minimal x86-64 assembler.
#[derive(Debug, Default)]
pub struct Asm {
    code: Vec<u8>,
    next_label: usize,
    label_offsets: HashMap<usize, usize>,
    fixups: Vec<Fixup>,
}

/// Condition codes for `jcc`/`setcc`.
#[derive(Debug, Clone, Copy)]
pub enum Cond {
    /// Overflow flag set.
    O,
    /// Zero / equal.
    Z,
    /// Not zero / not equal.
    NZ,
    /// Signed less than.
    L,
    /// Signed less than or equal.
    LE,
    /// Signed greater than.
    G,
    /// Signed greater than or equal.
    GE,
    /// Unsigned below.
    B,
    /// Unsigned below or equal.
    BE,
    /// Unsigned above.
    A,
    /// Unsigned above or equal.
    AE,
    /// Sign flag set (negative).
    S,
    /// Sign flag clear (non-negative).
    NS,
    /// Parity flag set (unordered float compare).
    P,
    /// Parity flag clear (ordered float compare).
    NP,
}

fn cond_code(c: Cond) -> u8 {
    match c {
        Cond::O => 0,
        Cond::Z => 4,
        Cond::NZ => 5,
        Cond::B => 2,
        Cond::BE => 6,
        Cond::A => 7,
        Cond::AE => 3,
        Cond::L => 12,
        Cond::LE => 14,
        Cond::G => 15,
        Cond::GE => 13,
        Cond::S => 8,
        Cond::NS => 9,
        Cond::P => 10,
        Cond::NP => 11,
    }
}

impl Asm {
    /// Create an empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh label.
    pub fn label(&mut self) -> Label {
        let id = self.next_label;
        self.next_label += 1;
        Label(id)
    }

    /// Bind the current position to `label`, resolving pending fixups.
    pub fn bind(&mut self, label: Label) {
        let pos = self.code.len();
        self.label_offsets.insert(label.0, pos);
        let matches: Vec<usize> = self
            .fixups
            .iter()
            .enumerate()
            .filter(|(_, f)| f.label == label.0)
            .map(|(i, _)| i)
            .collect();
        for i in matches.into_iter().rev() {
            self.patch_fixup(i, pos);
        }
        self.fixups.retain(|f| f.label != label.0);
    }

    fn patch_fixup(&mut self, idx: usize, target: usize) {
        let f = self.fixups[idx];
        match f.kind {
            FixupKind::Rel8 => {
                let from = f.offset + 1;
                let delta = target as i64 - from as i64;
                assert!((-128..=127).contains(&delta), "rel8 out of range");
                self.code[f.offset] = delta as i8 as u8;
            }
            FixupKind::Rel32 => {
                let from = f.offset + 4;
                let delta = target as i64 - from as i64;
                assert!(
                    (i32::MIN as i64..=i32::MAX as i64).contains(&delta),
                    "rel32 out of range"
                );
                let bytes = (delta as i32).to_le_bytes();
                self.code[f.offset..f.offset + 4].copy_from_slice(&bytes);
            }
        }
    }

    /// Finish assembly and return the machine code.
    pub fn assemble(self) -> Vec<u8> {
        assert!(
            self.fixups.is_empty(),
            "{} unbound fixups",
            self.fixups.len()
        );
        self.code
    }

    /// Current code length.
    pub fn len(&self) -> usize {
        self.code.len()
    }

    // ------------------------------------------------------------------
    // Low-level emission
    // ------------------------------------------------------------------

    fn emit(&mut self, byte: u8) {
        self.code.push(byte);
    }

    fn emit_bytes(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    fn emit_u32(&mut self, v: u32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn emit_i32(&mut self, v: i32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn emit_i64(&mut self, v: i64) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn rex(&mut self, w: bool, r: bool, x: bool, b: bool) {
        let mut rex = 0x40;
        if w {
            rex |= 0x08;
        }
        if r {
            rex |= 0x04;
        }
        if x {
            rex |= 0x02;
        }
        if b {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.emit(rex);
        }
    }

    fn modrm(&mut self, mode: u8, reg: u8, rm: u8) {
        self.emit((mode << 6) | ((reg & 7) << 3) | (rm & 7));
    }

    /// Emit a ModRM/SIB sequence for `[base + disp]` with `reg` as the reg field.
    fn modrm_mem(&mut self, reg: u8, base: Gpr, disp: i32) {
        let code = base.0;
        let low = code & 7;
        self.rex(false, (reg >> 3) != 0, false, (code >> 3) != 0);
        let (mode, rm, need_sib) = if disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (disp as i8) as i32 == disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, reg, rm);
        if need_sib {
            // scale=1, index=none, base=rsp (already reflected by REX.B for r12)
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(disp as i8 as u8),
            2 => self.emit_i32(disp),
            _ => {}
        }
    }

    fn modrm_reg(&mut self, reg: u8, rm: Gpr) {
        self.rex(false, (reg >> 3) != 0, false, (rm.0 >> 3) != 0);
        self.modrm(3, reg, rm.0);
    }

    fn modrm_reg_w(&mut self, reg: u8, rm: Gpr) {
        self.rex(true, (reg >> 3) != 0, false, (rm.0 >> 3) != 0);
        self.modrm(3, reg, rm.0);
    }

    fn opcode2(&mut self, op: u8) {
        self.emit(0x0F);
        self.emit(op);
    }

    // ------------------------------------------------------------------
    // Moves
    // ------------------------------------------------------------------

    /// `mov r64, r64`
    pub fn mov_rr(&mut self, dst: Gpr, src: Gpr) {
        self.rex(true, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.emit(0x8B);
        self.modrm(3, dst.0, src.0);
    }

    /// `mov r64, imm` (chooses imm32 sign-extended or movabs).
    pub fn mov_ri(&mut self, dst: Gpr, imm: i64) {
        if (i32::MIN as i64..=i32::MAX as i64).contains(&imm) {
            self.rex(true, false, false, (dst.0 >> 3) != 0);
            self.emit(0xC7);
            self.modrm(3, 0, dst.0);
            self.emit_i32(imm as i32);
        } else {
            self.rex(true, false, false, (dst.0 >> 3) != 0);
            self.emit(0xB8 + (dst.0 & 7));
            self.emit_i64(imm);
        }
    }

    /// `mov r64, [mem]`
    pub fn mov_rm(&mut self, dst: Gpr, mem: Mem) {
        self.rex(true, (dst.0 >> 3) != 0, false, (mem.base.0 >> 3) != 0);
        self.emit(0x8B);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, dst.0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
    }

    /// `mov [mem], r64`
    pub fn mov_mr(&mut self, mem: Mem, src: Gpr) {
        self.rex(true, (src.0 >> 3) != 0, false, (mem.base.0 >> 3) != 0);
        self.emit(0x89);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, src.0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
    }

    /// `mov dword [mem], imm32` (zero/sign extends into a wider slot only via
    /// later loads; used to write small tag constants).
    pub fn mov_mi(&mut self, mem: Mem, imm: i64) {
        self.rex(false, false, false, (mem.base.0 >> 3) != 0);
        self.emit(0xC7);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, 0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
        self.emit_i32(imm as i32);
    }

    /// `mov r32, r32` — zero-extends into the 64-bit destination.
    pub fn mov_r32_rr(&mut self, dst: Gpr, src: Gpr) {
        self.rex(false, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.emit(0x89);
        self.modrm(3, dst.0, src.0);
    }

    /// `lea r64, [mem]`
    pub fn lea(&mut self, dst: Gpr, mem: Mem) {
        self.rex(true, (dst.0 >> 3) != 0, false, (mem.base.0 >> 3) != 0);
        self.emit(0x8D);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, dst.0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
    }

    /// `movzx r64, al` (zero-extend a byte reg).
    pub fn movzx_r8(&mut self, dst: Gpr, src: Gpr) {
        self.rex(true, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.opcode2(0xB6);
        self.modrm(3, dst.0, src.0);
    }

    // ------------------------------------------------------------------
    // ALU
    // ------------------------------------------------------------------

    /// Emit `op r/m64, reg64` (0x01-0x31 family, e.g. add/sub/xor/and/or).
    fn alu_rm_reg(&mut self, op: u8, reg: Gpr, rm: Operand) {
        match rm {
            Operand::Gpr(g) => {
                self.rex(true, (reg.0 >> 3) != 0, false, (g.0 >> 3) != 0);
                self.emit(op);
                self.modrm(3, reg.0, g.0);
            }
            Operand::Mem(m) => {
                self.rex(true, (reg.0 >> 3) != 0, false, (m.base.0 >> 3) != 0);
                self.emit(op);
                let code = m.base.0;
                let low = code & 7;
                let (mode, rm, need_sib) = if m.disp == 0 && low != 5 {
                    (0u8, low, low == 4)
                } else if (m.disp as i8) as i32 == m.disp {
                    (1u8, low, low == 4)
                } else {
                    (2u8, low, low == 4)
                };
                self.modrm(mode, reg.0, rm);
                if need_sib {
                    self.emit(0x24);
                }
                match mode {
                    1 => self.emit(m.disp as i8 as u8),
                    2 => self.emit_i32(m.disp),
                    _ => {}
                }
            }
        }
    }

    /// Emit `add/sub/... r64, r/m64` (0x03 family: rm source).
    fn alu_reg_rm(&mut self, op: u8, reg: Gpr, rm: Operand) {
        match rm {
            Operand::Gpr(g) => {
                self.rex(true, (reg.0 >> 3) != 0, false, (g.0 >> 3) != 0);
                self.emit(op);
                self.modrm(3, reg.0, g.0);
            }
            Operand::Mem(m) => {
                self.rex(true, (reg.0 >> 3) != 0, false, (m.base.0 >> 3) != 0);
                self.emit(op);
                let code = m.base.0;
                let low = code & 7;
                let (mode, rm, need_sib) = if m.disp == 0 && low != 5 {
                    (0u8, low, low == 4)
                } else if (m.disp as i8) as i32 == m.disp {
                    (1u8, low, low == 4)
                } else {
                    (2u8, low, low == 4)
                };
                self.modrm(mode, reg.0, rm);
                if need_sib {
                    self.emit(0x24);
                }
                match mode {
                    1 => self.emit(m.disp as i8 as u8),
                    2 => self.emit_i32(m.disp),
                    _ => {}
                }
            }
        }
    }

    /// `add r64, imm` (chooses imm8/imm32).
    pub fn add_ri(&mut self, dst: Gpr, imm: i64) {
        self.imm_alu_ri(0, dst, imm);
    }

    /// `sub r64, imm`
    pub fn sub_ri(&mut self, dst: Gpr, imm: i64) {
        self.imm_alu_ri(5, dst, imm);
    }

    /// `and r64, imm`
    pub fn and_ri(&mut self, dst: Gpr, imm: i64) {
        self.imm_alu_ri(4, dst, imm);
    }

    /// `or r64, imm`
    pub fn or_ri(&mut self, dst: Gpr, imm: i64) {
        self.imm_alu_ri(1, dst, imm);
    }

    /// `xor r64, imm`
    pub fn xor_ri(&mut self, dst: Gpr, imm: i64) {
        self.imm_alu_ri(6, dst, imm);
    }

    fn imm_alu_ri(&mut self, ext: u8, dst: Gpr, imm: i64) {
        if imm >= i8::MIN as i64 && imm <= i8::MAX as i64 {
            self.rex(true, false, false, (dst.0 >> 3) != 0);
            self.emit(0x83);
            self.modrm(3, ext, dst.0);
            self.emit(imm as i8 as u8);
        } else {
            assert!(
                (i32::MIN as i64..=i32::MAX as i64).contains(&imm),
                "imm out of range for r64,imm32"
            );
            self.rex(true, false, false, (dst.0 >> 3) != 0);
            self.emit(0x81);
            self.modrm(3, ext, dst.0);
            self.emit_i32(imm as i32);
        }
    }

    /// `add r64, r64`
    pub fn add_rr(&mut self, dst: Gpr, src: Gpr) {
        self.alu_rm_reg(0x01, src, Operand::Gpr(dst));
    }

    /// `add r64, [mem]`
    pub fn add_rm(&mut self, dst: Gpr, mem: Mem) {
        self.alu_reg_rm(0x03, dst, Operand::Mem(mem));
    }

    /// `sub r64, r64`
    pub fn sub_rr(&mut self, dst: Gpr, src: Gpr) {
        self.alu_rm_reg(0x29, src, Operand::Gpr(dst));
    }

    /// `sub r64, [mem]`
    pub fn sub_rm(&mut self, dst: Gpr, mem: Mem) {
        self.alu_reg_rm(0x2B, dst, Operand::Mem(mem));
    }

    /// `and r64, r64`
    pub fn and_rr(&mut self, dst: Gpr, src: Gpr) {
        self.alu_rm_reg(0x21, src, Operand::Gpr(dst));
    }

    /// `and r64, [mem]`
    pub fn and_rm(&mut self, dst: Gpr, mem: Mem) {
        self.alu_reg_rm(0x23, dst, Operand::Mem(mem));
    }

    /// `or r64, r64`
    pub fn or_rr(&mut self, dst: Gpr, src: Gpr) {
        self.alu_rm_reg(0x09, src, Operand::Gpr(dst));
    }

    /// `or r64, [mem]`
    pub fn or_rm(&mut self, dst: Gpr, mem: Mem) {
        self.alu_reg_rm(0x0B, dst, Operand::Mem(mem));
    }

    /// `xor r64, r64`
    pub fn xor_rr(&mut self, dst: Gpr, src: Gpr) {
        self.alu_rm_reg(0x31, src, Operand::Gpr(dst));
    }

    /// `xor r64, [mem]`
    pub fn xor_rm(&mut self, dst: Gpr, mem: Mem) {
        self.alu_reg_rm(0x33, dst, Operand::Mem(mem));
    }

    /// `imul r64, r64` (dst *= src)
    pub fn imul_rr(&mut self, dst: Gpr, src: Gpr) {
        self.rex(true, (src.0 >> 3) != 0, false, (dst.0 >> 3) != 0);
        self.opcode2(0xAF);
        self.modrm(3, src.0, dst.0);
    }

    /// `neg r64`
    pub fn neg(&mut self, dst: Gpr) {
        self.rex(true, false, false, (dst.0 >> 3) != 0);
        self.emit(0xF7);
        self.modrm(3, 3, dst.0);
    }

    /// `not r64`
    pub fn not(&mut self, dst: Gpr) {
        self.rex(true, false, false, (dst.0 >> 3) != 0);
        self.emit(0xF7);
        self.modrm(3, 2, dst.0);
    }

    /// `cqo` (sign-extend rax into rdx)
    pub fn cqo(&mut self) {
        self.emit(0x48);
        self.emit(0x99);
    }

    /// `idiv r64` (rdx:rax / src; quotient rax, remainder rdx)
    pub fn idiv(&mut self, src: Gpr) {
        self.rex(true, false, false, (src.0 >> 3) != 0);
        self.emit(0xF7);
        self.modrm(3, 7, src.0);
    }

    /// `div r64` (unsigned)
    pub fn div(&mut self, src: Gpr) {
        self.rex(true, false, false, (src.0 >> 3) != 0);
        self.emit(0xF7);
        self.modrm(3, 6, src.0);
    }

    // ------------------------------------------------------------------
    // Shifts
    // ------------------------------------------------------------------

    fn shift_ri(&mut self, ext: u8, dst: Gpr, imm: u8) {
        self.rex(true, false, false, (dst.0 >> 3) != 0);
        self.emit(0xC1);
        self.modrm(3, ext, dst.0);
        self.emit(imm);
    }

    /// `shl r64, imm8`
    pub fn shl_ri(&mut self, dst: Gpr, imm: u8) {
        self.shift_ri(4, dst, imm);
    }

    /// `shr r64, imm8`
    pub fn shr_ri(&mut self, dst: Gpr, imm: u8) {
        self.shift_ri(5, dst, imm);
    }

    /// `sar r64, imm8`
    pub fn sar_ri(&mut self, dst: Gpr, imm: u8) {
        self.shift_ri(7, dst, imm);
    }

    // ------------------------------------------------------------------
    // Test / compare
    // ------------------------------------------------------------------

    /// `test r64, r64`
    pub fn test_rr(&mut self, a: Gpr, b: Gpr) {
        self.rex(true, (a.0 >> 3) != 0, false, (b.0 >> 3) != 0);
        self.emit(0x85);
        self.modrm(3, a.0, b.0);
    }

    /// `test r64, imm32`
    pub fn test_ri(&mut self, a: Gpr, imm: i32) {
        self.rex(true, false, false, (a.0 >> 3) != 0);
        self.emit(0xF7);
        self.modrm(3, 0, a.0);
        self.emit_i32(imm);
    }

    /// `cmp r64, r64`
    pub fn cmp_rr(&mut self, a: Gpr, b: Gpr) {
        self.alu_rm_reg(0x39, b, Operand::Gpr(a));
    }

    /// `cmp r64, imm`
    pub fn cmp_ri(&mut self, a: Gpr, imm: i64) {
        if imm >= i8::MIN as i64 && imm <= i8::MAX as i64 {
            self.rex(true, false, false, (a.0 >> 3) != 0);
            self.emit(0x83);
            self.modrm(3, 7, a.0);
            self.emit(imm as i8 as u8);
        } else {
            assert!((i32::MIN as i64..=i32::MAX as i64).contains(&imm));
            self.rex(true, false, false, (a.0 >> 3) != 0);
            self.emit(0x81);
            self.modrm(3, 7, a.0);
            self.emit_i32(imm as i32);
        }
    }

    /// `cmp r64, [mem]`
    pub fn cmp_rm(&mut self, a: Gpr, mem: Mem) {
        self.rex(true, (a.0 >> 3) != 0, false, (mem.base.0 >> 3) != 0);
        self.emit(0x3B);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, a.0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Condition codes
    // ------------------------------------------------------------------

    /// `setcc r8` — writes the condition byte into the low byte of `dst` and
    /// zero-extends into the full register. A REX prefix is always emitted so
    /// `spl/bpl/sil/dil` and `r8b..r15b` select correctly.
    pub fn setcc(&mut self, cond: Cond, dst: Gpr) {
        self.emit(0x40 | ((dst.0 >> 3) & 1));
        self.opcode2(0x90 + cond_code(cond));
        self.modrm(3, 0, dst.0);
        self.movzx_r8(dst, dst);
    }

    /// `jcc label` (rel32 or rel8).
    pub fn jcc(&mut self, cond: Cond, target: Label) {
        // Emit rel32 form always; simplest and correct.
        self.emit(0x0F);
        self.emit(0x80 + cond_code(cond));
        let offset = self.code.len();
        self.code.extend_from_slice(&[0; 4]);
        self.fixups.push(Fixup {
            offset,
            label: target.0,
            kind: FixupKind::Rel32,
        });
    }

    /// `jmp label`
    pub fn jmp(&mut self, target: Label) {
        self.emit(0xE9);
        let offset = self.code.len();
        self.code.extend_from_slice(&[0; 4]);
        self.fixups.push(Fixup {
            offset,
            label: target.0,
            kind: FixupKind::Rel32,
        });
    }

    /// `call r64` (indirect, for helper addresses).
    pub fn call_r(&mut self, target: Gpr) {
        self.rex(false, false, false, (target.0 >> 3) != 0);
        self.emit(0xFF);
        self.modrm(3, 2, target.0);
    }

    /// `call rel32` (direct, to a label).
    pub fn call_label(&mut self, target: Label) {
        self.emit(0xE8);
        let offset = self.code.len();
        self.code.extend_from_slice(&[0; 4]);
        self.fixups.push(Fixup {
            offset,
            label: target.0,
            kind: FixupKind::Rel32,
        });
    }

    /// `jmp r64`
    pub fn jmp_r(&mut self, target: Gpr) {
        self.rex(false, false, false, (target.0 >> 3) != 0);
        self.emit(0xFF);
        self.modrm(3, 4, target.0);
    }

    /// `ret`
    pub fn ret(&mut self) {
        self.emit(0xC3);
    }

    /// `push r64`
    pub fn push(&mut self, reg: Gpr) {
        self.rex(false, false, false, (reg.0 >> 3) != 0);
        self.emit(0x50 + (reg.0 & 7));
    }

    /// `pop r64`
    pub fn pop(&mut self, reg: Gpr) {
        self.rex(false, false, false, (reg.0 >> 3) != 0);
        self.emit(0x58 + (reg.0 & 7));
    }

    /// `nop`
    pub fn nop(&mut self) {
        self.emit(0x90);
    }

    /// `int3`
    pub fn int3(&mut self) {
        self.emit(0xCC);
    }

    /// `ud2`
    pub fn ud2(&mut self) {
        self.emit(0x0F);
        self.emit(0x0B);
    }

    /// Load a 64-bit absolute address into a register (`movabs`).
    pub fn movabs(&mut self, dst: Gpr, addr: u64) {
        self.rex(true, false, false, (dst.0 >> 3) != 0);
        self.emit(0xB8 + (dst.0 & 7));
        self.emit_i64(addr as i64);
    }

    /// `sub r64, r64` for stack frame allocation on rsp.
    pub fn sub_rsp_imm(&mut self, imm: u32) {
        self.emit(0x48);
        self.emit(0x81);
        self.modrm(3, 5, regs::RSP.0);
        self.emit_i32(imm as i32);
    }

    /// `add r64, imm` on rsp.
    pub fn add_rsp_imm(&mut self, imm: u32) {
        self.emit(0x48);
        self.emit(0x81);
        self.modrm(3, 0, regs::RSP.0);
        self.emit_i32(imm as i32);
    }

    // ------------------------------------------------------------------
    // SSE scalar float
    // ------------------------------------------------------------------

    fn sse_rr(&mut self, prefix: u8, op: u8, dst: Xmm, src: Xmm) {
        self.emit(prefix);
        self.rex(false, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.opcode2(op);
        self.modrm(3, dst.0, src.0);
    }

    fn sse_rm(&mut self, prefix: u8, op: u8, dst: Xmm, mem: Mem) {
        self.emit(prefix);
        self.rex(false, (dst.0 >> 3) != 0, false, (mem.base.0 >> 3) != 0);
        self.opcode2(op);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, dst.0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
    }

    fn sse_mr(&mut self, prefix: u8, op: u8, src: Xmm, mem: Mem) {
        self.emit(prefix);
        self.rex(false, (src.0 >> 3) != 0, false, (mem.base.0 >> 3) != 0);
        self.opcode2(op);
        let code = mem.base.0;
        let low = code & 7;
        let (mode, rm, need_sib) = if mem.disp == 0 && low != 5 {
            (0u8, low, low == 4)
        } else if (mem.disp as i8) as i32 == mem.disp {
            (1u8, low, low == 4)
        } else {
            (2u8, low, low == 4)
        };
        self.modrm(mode, src.0, rm);
        if need_sib {
            self.emit(0x24);
        }
        match mode {
            1 => self.emit(mem.disp as i8 as u8),
            2 => self.emit_i32(mem.disp),
            _ => {}
        }
    }

    /// `movsd xmm, xmm`
    pub fn movsd_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF2, 0x10, dst, src);
    }

    /// `movsd xmm, [mem]`
    pub fn movsd_rm(&mut self, dst: Xmm, mem: Mem) {
        self.sse_rm(0xF2, 0x10, dst, mem);
    }

    /// `movsd [mem], xmm`
    pub fn movsd_mr(&mut self, mem: Mem, src: Xmm) {
        self.sse_mr(0xF2, 0x11, src, mem);
    }

    /// `addsd xmm, xmm`
    pub fn addsd_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF2, 0x58, dst, src);
    }

    /// `addsd xmm, [mem]`
    pub fn addsd_rm(&mut self, dst: Xmm, mem: Mem) {
        self.sse_rm(0xF2, 0x58, dst, mem);
    }

    /// `subsd xmm, xmm`
    pub fn subsd_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF2, 0x5C, dst, src);
    }

    /// `subsd xmm, [mem]`
    pub fn subsd_rm(&mut self, dst: Xmm, mem: Mem) {
        self.sse_rm(0xF2, 0x5C, dst, mem);
    }

    /// `mulsd xmm, xmm`
    pub fn mulsd_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF2, 0x59, dst, src);
    }

    /// `mulsd xmm, [mem]`
    pub fn mulsd_rm(&mut self, dst: Xmm, mem: Mem) {
        self.sse_rm(0xF2, 0x59, dst, mem);
    }

    /// `divsd xmm, xmm`
    pub fn divsd_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF2, 0x5E, dst, src);
    }

    /// `divsd xmm, [mem]`
    pub fn divsd_rm(&mut self, dst: Xmm, mem: Mem) {
        self.sse_rm(0xF2, 0x5E, dst, mem);
    }

    /// `comisd xmm, xmm` (sets ZF/PF/CF; operands not ordered)
    pub fn comisd_rr(&mut self, a: Xmm, b: Xmm) {
        self.sse_rr(0x66, 0x2F, a, b);
    }

    /// `ucomisd xmm, xmm`
    pub fn ucomisd_rr(&mut self, a: Xmm, b: Xmm) {
        self.sse_rr(0x66, 0x2E, a, b);
    }

    /// `ucomisd xmm, m64` (unordered compare of `a` with `[mem]`)
    pub fn ucomisd_rm(&mut self, a: Xmm, mem: Mem) {
        self.sse_rm(0x66, 0x2E, a, mem);
    }

    /// `cvtsi2sd xmm, r64`
    pub fn cvtsi2sd_rr(&mut self, dst: Xmm, src: Gpr) {
        self.emit(0xF2);
        self.rex(true, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.opcode2(0x2A);
        self.modrm(3, dst.0, src.0);
    }

    /// `cvttsd2si r64, xmm`
    pub fn cvttsd2si_rr(&mut self, dst: Gpr, src: Xmm) {
        self.emit(0xF2);
        self.rex(true, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.opcode2(0x2C);
        self.modrm(3, dst.0, src.0);
    }

    /// `cvtsd2ss xmm, xmm`
    pub fn cvtsd2ss_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF2, 0x5A, dst, src);
    }

    /// `cvtss2sd xmm, xmm`
    pub fn cvtss2sd_rr(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0xF3, 0x5A, dst, src);
    }

    /// `xorpd xmm, xmm` (zero a register).
    pub fn xorpd(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(0x66, 0x57, dst, src);
    }

    /// Move a GPR into the low 64 bits of an XMM (`movq xmm, r64`).
    pub fn movq_xmm_rr(&mut self, dst: Xmm, src: Gpr) {
        self.emit(0x66);
        self.rex(true, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.emit(0x0F);
        self.emit(0x6E);
        self.modrm(3, dst.0, src.0);
    }

    /// Move the low 64 bits of an XMM into a GPR (`movq r64, xmm`).
    pub fn movq_rr_xmm(&mut self, dst: Gpr, src: Xmm) {
        self.emit(0x66);
        self.rex(true, (dst.0 >> 3) != 0, false, (src.0 >> 3) != 0);
        self.emit(0x0F);
        self.emit(0x7E);
        self.modrm(3, src.0, dst.0);
    }
}
