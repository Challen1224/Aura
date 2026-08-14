//! x86-64 machine-code generation from the SSA IR.
//!
//! Register allocation assigns every virtual register a fixed home. This
//! module turns each IR block into straight-line machine code:
//!
//! * scalars (`Int`/`Bool`/`Char`) live in 64-bit GPRs or 16-byte frame slots,
//! * floats live in XMM registers or slots,
//! * unknown-typed values (boxes) always live in slots.
//!
//! For known types the value is kept *unboxed* (an Int's payload, a Bool's or
//! Char's tag qword, a Float's bits). Anything the JIT cannot inline is
//! marshalled into the native frame's argument area and dispatched to the
//! helper stubs, which execute the interpreter's exact semantics.

use super::asm::regs::{RAX, RBP, RBX, RCX, RDX, RDI, R10, R11, R13, R14, R15, RSI, RSP};
use super::asm::xmms::XMM0;
use super::asm::{Asm, Cond, Gpr, Label, Mem, Xmm};
use super::helpers::{NF_DISPATCHER, NF_FRAME_BASE, NF_OP, NF_RESULT, NF_VM, raise_overflow_addr};
use crate::jit::ir::{BinOp, Block, ConvTarget, Function, Insn, KType, Term, UnOp, VReg};
use super::regalloc::{Allocation, Home, SLOT_BYTES};

/// Status returned from compiled code (matches [`super::helpers`]).
pub const STATUS_OK: u64 = 0;
/// The method threw; the exception is in the native frame's result slot.
pub const STATUS_EXCEPTION: u64 = 1;
/// A VM error occurred; it is stored in `vm.jit_error`.
pub const STATUS_ERROR: u64 = 2;

/// Generate machine code for `func`, using the homes from `alloc`.
/// Emit machine code for `func`. Returns the code bytes and the size of the
/// generated code's stack frame in bytes (the region the GC scans for roots).
pub fn emit_function(func: &Function, alloc: &Allocation) -> Result<(Vec<u8>, usize), String> {
    let mut cg = Codegen::new(func, alloc)?;
    let frame_bytes = cg.frame;
    let code = cg.emit()?;
    Ok((code, frame_bytes))
}

struct Codegen<'a> {
    func: &'a Function,
    alloc: &'a Allocation,
    asm: Asm,

    // Frame layout (all offsets relative to r13 = frame base).
    marshal_base: i32,
    zero_slot: i32,
    nf_slot: i32,
    spill_base: i32,
    frame: usize,

    epilogue: Label,
    overflow: Label,
    block_labels: Vec<Label>,
}

impl<'a> Codegen<'a> {
    fn new(func: &'a Function, alloc: &'a Allocation) -> Result<Self, String> {
        let locals_bytes = func.locals_bytes();
        let args_area = (func.max_stack as usize + 2) * SLOT_BYTES;
        let marshal_base = locals_bytes as i32;
        let zero_slot = (locals_bytes + func.max_stack as usize * SLOT_BYTES) as i32;
        let nf_slot = (locals_bytes + (func.max_stack as usize + 1) * SLOT_BYTES) as i32;
        let spill_base = (locals_bytes + args_area) as i32;
        let frame = spill_base as usize + func.spill_bytes;

        let mut asm = Asm::new();
        let epilogue = asm.label();
        let overflow = asm.label();
        let mut block_labels = Vec::with_capacity(func.blocks.len());
        for _ in 0..func.blocks.len() {
            block_labels.push(asm.label());
        }

        Ok(Self {
            func,
            alloc,
            asm,
            marshal_base,
            zero_slot,
            nf_slot,
            spill_base,
            frame,
            epilogue,
            overflow,
            block_labels,
        })
    }

    // ------------------------------------------------------------------
    // Layout helpers
    // ------------------------------------------------------------------

    fn home(&self, v: VReg) -> Home {
        self.alloc.homes[v as usize]
    }

    fn local_mem(&self, idx: u16) -> Mem {
        Mem::new(R13, (idx as i32) * SLOT_BYTES as i32)
    }

    fn local_payload(&self, idx: u16) -> Mem {
        Mem::new(R13, (idx as i32) * SLOT_BYTES as i32 + 8)
    }

    fn spill_tag(&self, slot: usize) -> Mem {
        Mem::new(R13, self.spill_base + (slot as i32) * SLOT_BYTES as i32)
    }

    fn spill_payload(&self, slot: usize) -> Mem {
        Mem::new(R13, self.spill_base + (slot as i32) * SLOT_BYTES as i32 + 8)
    }

    fn marshal_mem(&self, k: usize) -> Mem {
        Mem::new(R13, self.marshal_base + (k as i32) * SLOT_BYTES as i32)
    }

    // ------------------------------------------------------------------
    // Main driver
    // ------------------------------------------------------------------

    fn emit(&mut self) -> Result<Vec<u8>, String> {
        self.emit_prologue();
        for b in 0..self.func.blocks.len() {
            let block = &self.func.blocks[b];
            if !block.reachable {
                continue;
            }
            self.asm.bind(self.block_labels[b]);
            self.emit_block(block)?;
        }
        self.asm.bind(self.overflow);
        self.emit_overflow_handler();
        self.asm.bind(self.epilogue);
        self.emit_epilogue();
        Ok(std::mem::take(&mut self.asm).assemble())
    }

    fn emit_prologue(&mut self) {
        self.asm.push(RBP);
        self.asm.mov_rr(RBP, RSP);
        self.asm.push(R13);
        self.asm.push(R14);
        self.asm.push(R15);
        self.asm.push(RBX);
        self.asm.sub_rsp_imm(self.frame as u32);
        self.asm.mov_rr(R13, RSP);

        // Save the native frame pointer passed in rdi, and publish this
        // frame's base into the NativeFrame so the GC can scan its slots.
        self.asm.mov_mr(Mem::new(R13, self.nf_slot), RDI);
        self.asm.mov_mr(Mem::new(RDI, NF_FRAME_BASE), R13);

        // Copy the parameter boxes from argv (rsi) into the local slots.
        let params = self.func.params;
        if params > 0 {
            self.asm.xor_rr(RCX, RCX);
            self.asm.mov_rr(R11, R13);
            let loop_l = self.asm.label();
            let done_l = self.asm.label();
            self.asm.bind(loop_l);
            self.asm.cmp_ri(RCX, params as i64);
            self.asm.jcc(Cond::GE, done_l);
            self.asm.mov_rm(RAX, Mem::new(RSI, 0));
            self.asm.mov_mr(Mem::new(R11, 0), RAX);
            self.asm.mov_rm(RAX, Mem::new(RSI, 8));
            self.asm.mov_mr(Mem::new(R11, 8), RAX);
            self.asm.add_ri(RSI, 16);
            self.asm.add_ri(R11, 16);
            self.asm.add_ri(RCX, 1);
            self.asm.jmp(loop_l);
            self.asm.bind(done_l);
        }
    }

    fn emit_overflow_handler(&mut self) {
        self.asm.mov_rm(R10, Mem::new(R13, self.nf_slot));
        self.asm.mov_rm(RDI, Mem::new(R10, NF_VM));
        self.asm.movabs(R11, raise_overflow_addr() as u64);
        self.asm.call_r(R11);
        self.asm.mov_ri(RAX, STATUS_ERROR as i64);
        self.asm.jmp(self.epilogue);
    }

    fn emit_epilogue(&mut self) {
        self.asm.add_ri(RSP, self.frame as i64);
        self.asm.pop(RBX);
        self.asm.pop(R15);
        self.asm.pop(R14);
        self.asm.pop(R13);
        self.asm.pop(RBP);
        self.asm.ret();
    }

    fn emit_block(&mut self, block: &Block) -> Result<(), String> {
        for insn in &block.insns {
            self.emit_insn(insn)?;
        }
        self.emit_term(block)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Instructions
    // ------------------------------------------------------------------

    fn emit_insn(&mut self, insn: &Insn) -> Result<(), String> {
        match *insn {
            Insn::ConstInt { dst, val } => {
                self.asm.mov_ri(RAX, val);
                self.store_int(dst, RAX);
            }
            Insn::ConstFloat { dst, bits } => {
                self.asm.movabs(RAX, bits);
                self.asm.movq_xmm_rr(XMM0, RAX);
                self.store_float(dst, XMM0);
            }
            Insn::ConstBool { dst, val } => {
                self.asm.mov_ri(RAX, 3 | (val as i64) << 8);
                self.store_tag(dst, RAX);
            }
            Insn::ConstChar { dst, ch } => {
                // Rust stores the char at byte offset 4 of the 16-byte slot, so
                // the tag word is `TAG | c << 32` (not `<< 8`).
                self.asm.mov_ri(RAX, 4 | (ch as u32 as i64) << 32);
                self.store_tag(dst, RAX);
            }
            Insn::ConstNull { dst } => {
                self.store_box_value(dst, 9);
            }
            Insn::ConstUnit { dst } => {
                self.store_box_value(dst, 0);
            }
            Insn::LdLocal { dst, idx } => self.emit_ld_local(dst, idx),
            Insn::StLocal { idx, src } => self.emit_st_local(idx, src),
            Insn::Bin { dst, op, a, b } => self.emit_bin(dst, op, a, b),
            Insn::Un { dst, op, a } => self.emit_un(dst, op, a)?,
            Insn::Conv { dst, to, src } => self.emit_conv(dst, to, src),
            Insn::Helper { dst, op_id, ref args } => {
                self.emit_helper(dst, op_id, args)?;
            }
        }
        Ok(())
    }

    fn emit_ld_local(&mut self, dst: VReg, idx: u16) {
        match self.func.local_types[idx as usize] {
            KType::Int => match self.home(dst) {
                Home::Gpr(h) => {
                    self.asm.mov_rm(h, self.local_payload(idx));
                }
                Home::Slot(s) => self.copy_box(self.local_mem(idx), s),
                Home::Xmm(_) => unreachable!(),
            },
            KType::Bool | KType::Char => match self.home(dst) {
                Home::Gpr(h) => {
                    self.asm.mov_rm(h, self.local_mem(idx));
                }
                Home::Slot(s) => self.copy_box(self.local_mem(idx), s),
                Home::Xmm(_) => unreachable!(),
            },
            KType::Float => match self.home(dst) {
                Home::Xmm(x) => {
                    self.asm.movsd_rm(x, self.local_payload(idx));
                }
                Home::Slot(s) => self.copy_box(self.local_mem(idx), s),
                Home::Gpr(_) => unreachable!(),
            },
            KType::Unknown => match self.home(dst) {
                Home::Slot(s) => self.copy_box(self.local_mem(idx), s),
                _ => unreachable!("unknown-typed values always live in slots"),
            },
        }
    }

    fn emit_st_local(&mut self, idx: u16, src: VReg) {
        let ty = self.func.local_types[idx as usize];
        match ty {
            KType::Int => {
                self.load_int_rax(src);
                self.asm.mov_mr(self.local_payload(idx), RAX);
                self.asm.mov_ri(RAX, 1);
                self.asm.mov_mr(self.local_mem(idx), RAX);
            }
            KType::Bool | KType::Char => {
                self.load_tag_rax(src);
                self.asm.mov_mr(self.local_mem(idx), RAX);
                self.asm.xor_rr(RAX, RAX);
                self.asm.mov_mr(self.local_payload(idx), RAX);
            }
            KType::Float => {
                self.load_float_xmm0(src);
                self.asm.mov_ri(RAX, 2);
                self.asm.mov_mr(self.local_mem(idx), RAX);
                self.asm.movq_rr_xmm(RAX, XMM0);
                self.asm.mov_mr(self.local_payload(idx), RAX);
            }
            KType::Unknown => {
                let mem = self.local_mem(idx);
                self.marshal_to(mem, src);
            }
        }
    }

    fn emit_bin(&mut self, dst: VReg, op: BinOp, a: VReg, b: VReg) {
        let ty = self.func.vtype(a);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => match ty {                KType::Int => {
                    self.load_int_rax(a);
                    match self.home(b) {
                        Home::Gpr(g) => match op {
                            BinOp::Add => self.asm.add_rr(RAX, g),
                            BinOp::Sub => self.asm.sub_rr(RAX, g),
                            BinOp::Mul => self.asm.imul_rr(RAX, g),
                            _ => unreachable!(),
                        },
                        Home::Slot(s) => {
                            let m = self.spill_payload(s);
                            match op {
                                BinOp::Add => self.asm.add_rm(RAX, m),
                                BinOp::Sub => self.asm.sub_rm(RAX, m),
                                BinOp::Mul => {
                                    self.asm.mov_rm(RDX, m);
                                    self.asm.imul_rr(RAX, RDX);
                                }
                                _ => unreachable!(),
                            }
                        }
                        Home::Xmm(_) => unreachable!(),
                    }
                    if self.func.overflow_checks {
                        self.asm.jcc(Cond::O, self.overflow);
                    }
                    self.store_int(dst, RAX);
                }
                KType::Float => {
                    self.load_float_xmm0(a);
                    match self.home(b) {
                        Home::Xmm(x) => match op {
                            BinOp::Add => self.asm.addsd_rr(XMM0, x),
                            BinOp::Sub => self.asm.subsd_rr(XMM0, x),
                            BinOp::Mul => self.asm.mulsd_rr(XMM0, x),
                            _ => unreachable!(),
                        },
                        Home::Slot(s) => {
                            let m = self.spill_payload(s);
                            match op {
                                BinOp::Add => self.asm.addsd_rm(XMM0, m),
                                BinOp::Sub => self.asm.subsd_rm(XMM0, m),
                                BinOp::Mul => self.asm.mulsd_rm(XMM0, m),
                                _ => unreachable!(),
                            }
                        }
                        Home::Gpr(_) => unreachable!(),
                    }
                    self.store_float(dst, XMM0);
                }
                _ => unreachable!("bin arithmetic only inlined for int/float"),
            },
            BinOp::Eq => match ty {
                KType::Int => {
                    self.load_int_rax(a);
                    match self.home(b) {
                        Home::Gpr(g) => self.asm.cmp_rr(RAX, g),
                        Home::Slot(s) => self.asm.cmp_rm(RAX, self.spill_payload(s)),
                        Home::Xmm(_) => unreachable!(),
                    }
                    self.setcc_bool(Cond::Z);
                    self.store_tag(dst, RAX);
                }
                KType::Bool | KType::Char => {
                    self.load_tag_rax(a);
                    match self.home(b) {
                        Home::Gpr(g) => self.asm.cmp_rr(RAX, g),
                        Home::Slot(s) => self.asm.cmp_rm(RAX, self.spill_tag(s)),
                        Home::Xmm(_) => unreachable!(),
                    }
                    self.setcc_bool(Cond::Z);
                    self.store_tag(dst, RAX);
                }
                KType::Float => {
                    self.load_float_xmm0(a);
                    self.float_cmp_operand(b);
                    self.asm.setcc(Cond::Z, RAX);
                    self.asm.setcc(Cond::P, RCX);
                    self.asm.xor_ri(RCX, 1);
                    self.asm.and_rr(RAX, RCX);
                    self.tagify_bool();
                    self.store_tag(dst, RAX);
                }
                _ => unreachable!("eq only inlined for known scalar types"),
            },
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match ty {
                KType::Int => {
                    self.load_int_rax(a);
                    match self.home(b) {
                        Home::Gpr(g) => self.asm.cmp_rr(RAX, g),
                        Home::Slot(s) => self.asm.cmp_rm(RAX, self.spill_payload(s)),
                        Home::Xmm(_) => unreachable!(),
                    }
                    let cc = match op {
                        BinOp::Lt => Cond::L,
                        BinOp::Le => Cond::LE,
                        BinOp::Gt => Cond::G,
                        BinOp::Ge => Cond::GE,
                        _ => unreachable!(),
                    };
                    self.setcc_bool(cc);
                    self.store_tag(dst, RAX);
                }
                KType::Float => {
                    self.load_float_xmm0(a);
                    self.float_cmp_operand(b);
                    match op {
                        BinOp::Lt => {
                            self.asm.setcc(Cond::B, RAX);
                            self.asm.setcc(Cond::P, RCX);
                            self.asm.xor_ri(RCX, 1);
                            self.asm.and_rr(RAX, RCX);
                        }
                        BinOp::Le => {
                            self.asm.setcc(Cond::BE, RAX);
                            self.asm.setcc(Cond::P, RCX);
                            self.asm.xor_ri(RCX, 1);
                            self.asm.and_rr(RAX, RCX);
                        }
                        BinOp::Gt => self.asm.setcc(Cond::A, RAX),
                        BinOp::Ge => self.asm.setcc(Cond::AE, RAX),
                        _ => unreachable!(),
                    }
                    self.tagify_bool();
                    self.store_tag(dst, RAX);
                }
                _ => unreachable!("ordering only inlined for int/float"),
            },
            BinOp::And | BinOp::Or => {
                self.load_tag_rax(a);
                match self.home(b) {
                    Home::Gpr(g) => match op {
                        BinOp::And => self.asm.and_rr(RAX, g),
                        BinOp::Or => self.asm.or_rr(RAX, g),
                        _ => unreachable!(),
                    },
                    Home::Slot(s) => {
                        let m = self.spill_tag(s);
                        match op {
                            BinOp::And => self.asm.and_rm(RAX, m),
                            BinOp::Or => self.asm.or_rm(RAX, m),
                            _ => unreachable!(),
                        }
                    }
                    Home::Xmm(_) => unreachable!(),
                }
                self.store_tag(dst, RAX);
            }
            BinOp::Div | BinOp::Rem => unreachable!("div/rem always route to the dispatcher"),
        }
    }

    fn emit_un(&mut self, dst: VReg, op: UnOp, a: VReg) -> Result<(), String> {
        match op {
            UnOp::Neg => match self.func.vtype(a) {
                KType::Int => {
                    self.load_int_rax(a);
                    self.asm.neg(RAX);
                    if self.func.overflow_checks {
                        self.asm.jcc(Cond::O, self.overflow);
                    }
                    self.store_int(dst, RAX);
                }
                KType::Float => {
                    self.load_float_xmm0(a);
                    self.asm.movq_rr_xmm(RAX, XMM0);
                    self.asm.movabs(RCX, 0x8000_0000_0000_0000);
                    self.asm.xor_rr(RAX, RCX);
                    self.asm.movq_xmm_rr(XMM0, RAX);
                    self.store_float(dst, XMM0);
                }
                other => {
                    return Err(format!("codegen: unary neg on {other:?}"));
                }
            },
            UnOp::Not => {
                self.emit_truthy_rax(a);
                self.asm.xor_ri(RAX, 1);
                self.asm.shl_ri(RAX, 8);
                self.asm.or_ri(RAX, 3);
                self.store_tag(dst, RAX);
            }
        }
        Ok(())
    }

    fn emit_conv(&mut self, dst: VReg, to: ConvTarget, src: VReg) {
        let src_ty = self.func.vtype(src);
        match (to, src_ty) {
            (ConvTarget::Int64 | ConvTarget::UInt64, KType::Int) => {
                self.load_int_rax(src);
                self.store_int(dst, RAX);
            }
            (ConvTarget::Int8, KType::Int) => {
                self.load_int_rax(src);
                self.asm.shl_ri(RAX, 56);
                self.asm.sar_ri(RAX, 56);
                self.store_int(dst, RAX);
            }
            (ConvTarget::UInt8, KType::Int) => {
                self.load_int_rax(src);
                self.asm.and_ri(RAX, 0xFF);
                self.store_int(dst, RAX);
            }
            (ConvTarget::Int16, KType::Int) => {
                self.load_int_rax(src);
                self.asm.shl_ri(RAX, 48);
                self.asm.sar_ri(RAX, 48);
                self.store_int(dst, RAX);
            }
            (ConvTarget::UInt16, KType::Int) => {
                self.load_int_rax(src);
                self.asm.and_ri(RAX, 0xFFFF);
                self.store_int(dst, RAX);
            }
            (ConvTarget::Int32, KType::Int) => {
                self.load_int_rax(src);
                self.asm.shl_ri(RAX, 32);
                self.asm.sar_ri(RAX, 32);
                self.store_int(dst, RAX);
            }
            (ConvTarget::UInt32, KType::Int) => {
                self.load_int_rax(src);
                self.asm.shl_ri(RAX, 32);
                self.asm.shr_ri(RAX, 32);
                self.store_int(dst, RAX);
            }
            (ConvTarget::F32, KType::Float) => {
                self.load_float_xmm0(src);
                self.asm.cvtsd2ss_rr(XMM0, XMM0);
                self.asm.cvtss2sd_rr(XMM0, XMM0);
                self.store_float(dst, XMM0);
            }
            (ConvTarget::F64, KType::Float) => {
                self.load_float_xmm0(src);
                self.store_float(dst, XMM0);
            }
            (ConvTarget::F32 | ConvTarget::F64, KType::Int) => {
                self.load_int_rax(src);
                self.asm.cvtsi2sd_rr(XMM0, RAX);
                if to == ConvTarget::F32 {
                    self.asm.cvtsd2ss_rr(XMM0, XMM0);
                    self.asm.cvtss2sd_rr(XMM0, XMM0);
                }
                self.store_float(dst, XMM0);
            }
            _ => {
                unreachable!("unsupported inlined conv {to:?} from {src_ty:?}");
            }
        }
    }

    fn emit_helper(&mut self, dst: Option<VReg>, op_id: u32, args: &[VReg]) -> Result<(), String> {
        // Marshal every argument into the frame's argument area first. This
        // may read values from caller-saved homes (rsi/rdi/r8-r11), so it must
        // happen before those registers are repurposed for the call.
        for (k, arg) in args.iter().enumerate() {
            self.marshal_to(self.marshal_mem(k), *arg);
        }

        self.asm.mov_rm(R10, Mem::new(R13, self.nf_slot));
        self.asm.mov_ri(RAX, op_id as i64);
        self.asm.mov_mr(Mem::new(R10, NF_OP), RAX);
        self.asm.lea(RSI, self.marshal_mem(0));
        self.asm.mov_ri(RDX, args.len() as i64);
        self.asm.mov_rr(RDI, R10);
        self.asm.mov_rm(R11, Mem::new(R10, NF_DISPATCHER));
        self.asm.call_r(R11);

        // Non-zero status (exception/error) returns through the epilogue.
        self.asm.test_rr(RAX, RAX);
        self.asm.jcc(Cond::NZ, self.epilogue);

        if let Some(dst) = dst {
            // The dispatcher may clobber caller-saved registers, so reload the
            // native frame pointer.
            self.asm.mov_rm(R10, Mem::new(R13, self.nf_slot));
            self.load_result(dst, R10);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Terminators
    // ------------------------------------------------------------------

    fn emit_term(&mut self, block: &Block) -> Result<(), String> {
        match &block.term {
            Term::Ret(v) => {
                let mem = self.marshal_mem(0);
                self.marshal_to(mem, *v);
                self.asm.mov_rm(R10, Mem::new(R13, self.nf_slot));
                self.asm.mov_rm(RAX, mem);
                self.asm.mov_rm(RCX, Mem::new(R13, mem.disp + 8));
                self.asm.mov_mr(Mem::new(R10, NF_RESULT), RAX);
                self.asm.mov_mr(Mem::new(R10, NF_RESULT + 8), RCX);
                self.asm.xor_rr(RAX, RAX);
                self.asm.jmp(self.epilogue);
            }
            Term::Br(target) => {
                self.emit_edge_moves(block, *target);
                self.asm.jmp(self.block_labels[*target]);
            }
            Term::BrTrue(cond, target) => {
                self.emit_truthy_rax(*cond);
                let skip = self.asm.label();
                self.asm.jcc(Cond::Z, skip);
                self.emit_edge_moves(block, *target);
                self.asm.jmp(self.block_labels[*target]);
                self.asm.bind(skip);
                if let Some(fall) = self.fall_of(block, *target) {
                    self.emit_edge_moves(block, fall);
                }
            }
            Term::BrFalse(cond, target) => {
                self.emit_truthy_rax(*cond);
                let skip = self.asm.label();
                self.asm.jcc(Cond::NZ, skip);
                self.emit_edge_moves(block, *target);
                self.asm.jmp(self.block_labels[*target]);
                self.asm.bind(skip);
                if let Some(fall) = self.fall_of(block, *target) {
                    self.emit_edge_moves(block, fall);
                }
            }
            Term::Unreachable => {}
        }
        Ok(())
    }

    fn fall_of(&self, block: &Block, target: usize) -> Option<usize> {
        block.succs.iter().copied().find(|s| *s != target)
    }

    fn emit_edge_moves(&mut self, block: &Block, succ: usize) {
        if let Some(moves) = block.edge_moves.get(&succ) {
            for (param, src) in moves {
                match self.func.vtype(*param) {
                    KType::Int => {
                        self.load_int_rax(*src);
                        self.store_int(*param, RAX);
                    }
                    KType::Bool | KType::Char => {
                        self.load_tag_rax(*src);
                        self.store_tag(*param, RAX);
                    }
                    KType::Float => {
                        self.load_float_xmm0(*src);
                        self.store_float(*param, XMM0);
                    }
                    KType::Unknown => {
                        let dst = match self.home(*param) {
                            Home::Slot(s) => s,
                            _ => unreachable!(),
                        };
                        let src_slot = match self.home(*src) {
                            Home::Slot(s) => s,
                            _ => unreachable!(),
                        };
                        let src_mem = self.spill_tag(src_slot);
                        self.asm.mov_rm(RAX, src_mem);
                        self.asm.mov_rm(RCX, Mem::new(R13, src_mem.disp + 8));
                        self.asm.mov_mr(self.spill_tag(dst), RAX);
                        self.asm.mov_mr(self.spill_payload(dst), RCX);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Load helpers
    // ------------------------------------------------------------------

    /// Load an `Int`'s payload into `rax`.
    fn load_int_rax(&mut self, v: VReg) {
        match self.home(v) {
            Home::Gpr(g) => self.asm.mov_rr(RAX, g),
            Home::Slot(s) => self.asm.mov_rm(RAX, self.spill_payload(s)),
            Home::Xmm(_) => unreachable!(),
        }
    }

    /// Load a `Bool`/`Char` tag qword into `rax`.
    fn load_tag_rax(&mut self, v: VReg) {
        match self.home(v) {
            Home::Gpr(g) => self.asm.mov_rr(RAX, g),
            Home::Slot(s) => self.asm.mov_rm(RAX, self.spill_tag(s)),
            Home::Xmm(_) => unreachable!(),
        }
    }

    /// Load a `Float`'s bits into `xmm0`.
    fn load_float_xmm0(&mut self, v: VReg) {
        match self.home(v) {
            Home::Xmm(x) => self.asm.movsd_rr(XMM0, x),
            Home::Slot(s) => self.asm.movsd_rm(XMM0, self.spill_payload(s)),
            Home::Gpr(_) => unreachable!(),
        }
    }

    /// Issue the compare against `b` for the float value currently in `xmm0`.
    fn float_cmp_operand(&mut self, b: VReg) {
        match self.home(b) {
            Home::Xmm(x) => self.asm.ucomisd_rr(XMM0, x),
            Home::Slot(s) => self.asm.ucomisd_rm(XMM0, self.spill_payload(s)),
            Home::Gpr(_) => unreachable!(),
        }
    }

    /// `setcc` plus conversion of the 0/1 result into a bool tag qword.
    fn setcc_bool(&mut self, cc: Cond) {
        self.asm.setcc(cc, RAX);
        self.tagify_bool();
    }

    /// Turn a 0/1 value in `rax` into a bool tag qword (`3 | b << 8`).
    fn tagify_bool(&mut self) {
        self.asm.shl_ri(RAX, 8);
        self.asm.or_ri(RAX, 3);
    }

    // ------------------------------------------------------------------
    // Store helpers
    // ------------------------------------------------------------------

    /// Store an `Int` payload (from `rax`) into `dst`'s home.
    fn store_int(&mut self, dst: VReg, src: Gpr) {
        match self.home(dst) {
            Home::Gpr(h) => self.asm.mov_rr(h, src),
            Home::Slot(s) => {
                self.asm.mov_mr(self.spill_payload(s), src);
                self.asm.mov_ri(RAX, 1);
                self.asm.mov_mr(self.spill_tag(s), RAX);
            }
            Home::Xmm(_) => unreachable!(),
        }
    }

    /// Store a `Bool`/`Char` tag qword (from `rax`) into `dst`'s home.
    fn store_tag(&mut self, dst: VReg, src: Gpr) {
        match self.home(dst) {
            Home::Gpr(h) => self.asm.mov_rr(h, src),
            Home::Slot(s) => {
                self.asm.mov_mr(self.spill_tag(s), src);
                self.asm.xor_rr(RAX, RAX);
                self.asm.mov_mr(self.spill_payload(s), RAX);
            }
            Home::Xmm(_) => unreachable!(),
        }
    }

    /// Store a `Float`'s bits (from `xmm0`) into `dst`'s home.
    fn store_float(&mut self, dst: VReg, src: Xmm) {
        match self.home(dst) {
            Home::Xmm(h) => self.asm.movsd_rr(h, src),
            Home::Slot(s) => {
                self.asm.mov_ri(RAX, 2);
                self.asm.mov_mr(self.spill_tag(s), RAX);
                self.asm.movq_rr_xmm(RAX, src);
                self.asm.mov_mr(self.spill_payload(s), RAX);
            }
            Home::Gpr(_) => unreachable!(),
        }
    }

    /// Write a boxed constant (`tag`, payload 0) into a slot home.
    fn store_box_value(&mut self, dst: VReg, tag: i64) {
        match self.home(dst) {
            Home::Slot(s) => {
                self.asm.mov_ri(RAX, tag);
                self.asm.mov_mr(self.spill_tag(s), RAX);
                self.asm.xor_rr(RAX, RAX);
                self.asm.mov_mr(self.spill_payload(s), RAX);
            }
            _ => unreachable!("boxed constants always live in slots"),
        }
    }

    /// Copy a 16-byte box from `from` into the slot home `to`.
    fn copy_box(&mut self, from: Mem, to: usize) {
        self.asm.mov_rm(RAX, from);
        self.asm.mov_rm(RCX, Mem::new(R13, from.disp + 8));
        self.asm.mov_mr(self.spill_tag(to), RAX);
        self.asm.mov_mr(self.spill_payload(to), RCX);
    }

    /// Marshal `v` into the full `Value` at `mem` (tag + payload).
    fn marshal_to(&mut self, mem: Mem, v: VReg) {
        match self.func.vtype(v) {
            KType::Int => match self.home(v) {
                Home::Gpr(g) => {
                    self.asm.mov_mr(Mem::new(R13, mem.disp + 8), g);
                    self.asm.mov_ri(RAX, 1);
                    self.asm.mov_mr(mem, RAX);
                }
                Home::Slot(s) => self.marshal_box(mem, s),
                Home::Xmm(_) => unreachable!(),
            },
            KType::Bool | KType::Char => match self.home(v) {
                Home::Gpr(g) => {
                    self.asm.mov_mr(mem, g);
                    self.asm.xor_rr(RAX, RAX);
                    self.asm.mov_mr(Mem::new(R13, mem.disp + 8), RAX);
                }
                Home::Slot(s) => self.marshal_box(mem, s),
                Home::Xmm(_) => unreachable!(),
            },
            KType::Float => match self.home(v) {
                Home::Xmm(x) => {
                    self.asm.mov_ri(RAX, 2);
                    self.asm.mov_mr(mem, RAX);
                    self.asm.movq_rr_xmm(RAX, x);
                    self.asm.mov_mr(Mem::new(R13, mem.disp + 8), RAX);
                }
                Home::Slot(s) => self.marshal_box(mem, s),
                Home::Gpr(_) => unreachable!(),
            },
            KType::Unknown => {
                let s = match self.home(v) {
                    Home::Slot(s) => s,
                    _ => unreachable!(),
                };
                self.marshal_box(mem, s);
            }
        }
    }

    fn marshal_box(&mut self, mem: Mem, slot: usize) {
        self.asm.mov_rm(RAX, self.spill_tag(slot));
        self.asm.mov_rm(RCX, self.spill_payload(slot));
        self.asm.mov_mr(mem, RAX);
        self.asm.mov_mr(Mem::new(R13, mem.disp + 8), RCX);
    }

    /// Load the helper result from `nf` into `dst`'s home, switching on the
    /// returned box's tag so a helper always produces the right representation
    /// regardless of which box type it chose to return.
    fn load_result(&mut self, dst: VReg, nf: Gpr) {
        self.asm.mov_rm(RAX, Mem::new(nf, NF_RESULT));
        self.asm.mov_rm(RCX, Mem::new(nf, NF_RESULT + 8));
        match self.home(dst) {
            Home::Gpr(h) => {
                let l_int = self.asm.label();
                let l_packed = self.asm.label();
                let l_done = self.asm.label();
                // Compare the low byte of the tag so bool/char (tag | data << 8)
                // are still matched.
                self.asm.mov_rr(RDX, RAX);
                self.asm.and_ri(RDX, 0xFF);
                self.asm.cmp_ri(RDX, 1);
                self.asm.jcc(Cond::Z, l_int);
                self.asm.cmp_ri(RDX, 3);
                self.asm.jcc(Cond::Z, l_packed);
                self.asm.cmp_ri(RDX, 4);
                self.asm.jcc(Cond::Z, l_packed);
                self.asm.jmp(l_int);
                self.asm.bind(l_int);
                self.asm.mov_rr(h, RCX);
                self.asm.jmp(l_done);
                self.asm.bind(l_packed);
                self.asm.mov_rr(h, RAX);
                self.asm.bind(l_done);
            }
            Home::Xmm(x) => {
                self.asm.movq_xmm_rr(x, RCX);
            }
            Home::Slot(s) => {
                self.asm.mov_mr(self.spill_tag(s), RAX);
                self.asm.mov_mr(self.spill_payload(s), RCX);
            }
        }
    }

    // ------------------------------------------------------------------
    // Truthiness
    // ------------------------------------------------------------------

    /// Compute the interpreter's `is_truthy` of `v` into `rax` (0 or 1).
    fn emit_truthy_rax(&mut self, v: VReg) {
        match self.func.vtype(v) {
            KType::Int => {
                self.load_int_rax(v);
                self.asm.test_rr(RAX, RAX);
                self.asm.setcc(Cond::NZ, RAX);
            }
            KType::Bool => {
                self.load_tag_rax(v);
                self.asm.shr_ri(RAX, 8);
            }
            KType::Char => {
                self.asm.mov_ri(RAX, 1);
            }
            KType::Float => {
                self.load_float_xmm0(v);
                // truthy = PF | !ZF  (NaN is truthy: `f != 0.0`).
                self.asm.xor_rr(RAX, RAX);
                self.asm.mov_mr(Mem::new(R13, self.zero_slot), RAX);
                self.asm.ucomisd_rm(XMM0, Mem::new(R13, self.zero_slot));
                self.asm.setcc(Cond::P, RAX);
                self.asm.setcc(Cond::Z, RCX);
                self.asm.xor_ri(RCX, 1);
                self.asm.or_rr(RAX, RCX);
            }
            KType::Unknown => {
                let s = match self.home(v) {
                    Home::Slot(s) => s,
                    _ => unreachable!(),
                };
                self.emit_box_truthy(s);
            }
        }
    }

    fn emit_box_truthy(&mut self, slot: usize) {
        let ref_l = self.asm.label();
        let packed_l = self.asm.label();
        let int_l = self.asm.label();
        let float_l = self.asm.label();
        let char_l = self.asm.label();
        let null_l = self.asm.label();
        let done_l = self.asm.label();

        self.asm.mov_rm(RAX, self.spill_tag(slot));
        self.asm.mov_rm(RCX, self.spill_payload(slot));
        self.asm.cmp_ri(RAX, 5);
        self.asm.jcc(Cond::AE, ref_l);
        self.asm.cmp_ri(RAX, 3);
        self.asm.jcc(Cond::AE, packed_l);
        self.asm.cmp_ri(RAX, 1);
        self.asm.jcc(Cond::Z, int_l);
        self.asm.cmp_ri(RAX, 2);
        self.asm.jcc(Cond::Z, float_l);
        // tag 0: unit -> falsy.
        self.asm.xor_rr(RAX, RAX);
        self.asm.jmp(done_l);

        self.asm.bind(packed_l);
        self.asm.cmp_ri(RAX, 4);
        self.asm.jcc(Cond::Z, char_l);
        // tag 3: bool -> bit 8.
        self.asm.shr_ri(RAX, 8);
        self.asm.jmp(done_l);

        self.asm.bind(char_l);
        self.asm.mov_ri(RAX, 1);
        self.asm.jmp(done_l);

        self.asm.bind(int_l);
        self.asm.test_rr(RCX, RCX);
        self.asm.setcc(Cond::NZ, RAX);
        self.asm.jmp(done_l);

        self.asm.bind(float_l);
        // `f != 0.0` (0.0 and -0.0 are falsy; NaN truthy). Payload bits shifted
        // left are 0 only for 0.0 and -0.0.
        self.asm.shl_ri(RCX, 1);
        self.asm.test_rr(RCX, RCX);
        self.asm.setcc(Cond::NZ, RAX);
        self.asm.jmp(done_l);

        self.asm.bind(ref_l);
        self.asm.cmp_ri(RAX, 9);
        self.asm.jcc(Cond::Z, null_l);
        self.asm.mov_ri(RAX, 1);
        self.asm.jmp(done_l);

        self.asm.bind(null_l);
        self.asm.xor_rr(RAX, RAX);

        self.asm.bind(done_l);
    }
}
