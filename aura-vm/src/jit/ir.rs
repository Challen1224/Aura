//! JIT intermediate representation.
//!
//! Bytecode is lowered into a lightweight SSA-like IR: virtual registers with
//! an abstract type per virtual, basic blocks with parameters, and phi values
//! resolved by copy moves on control-flow edges (the virtuals are renamed so
//! the moves are always parallel-copy safe).
//!
//! Only the subset of the instruction set the JIT can compile natively is
//! represented here. Everything else (heap allocation, calls, field access,
//! exceptions, dynamic dispatch) is a [`Insn::Helper`] that delegates to a VM
//! helper stub, keeping semantics identical to the interpreter.

use aura_bytecode::{ClassId, EnumId, MethodId, MethodDef, Module, Op, TypeDesc};
use std::collections::HashMap;

/// Virtual register.
pub type VReg = u32;

/// Abstract type of a virtual register.
///
/// Known types can live in machine registers; [`KType::Unknown`] values are
/// kept as full 16-byte value slots and handled dynamically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KType {
    /// Integer payload in a GPR.
    Int,
    /// f64 payload in an XMM register.
    Float,
    /// Tag qword (bit 8 = truth) in a GPR.
    Bool,
    /// Tag qword (bits 8+ = code point) in a GPR.
    Char,
    /// Unboxed at runtime; stored as a full value slot.
    Unknown,
}

impl KType {
    pub fn from_typedesc(t: &TypeDesc) -> KType {
        if t.is_int() {
            KType::Int
        } else if t.is_float() {
            KType::Float
        } else if *t == TypeDesc::Bool {
            KType::Bool
        } else if *t == TypeDesc::Char {
            KType::Char
        } else {
            KType::Unknown
        }
    }

    /// Is this a primitive scalar that can live in a machine register?
    pub fn is_known(self) -> bool {
        !matches!(self, KType::Unknown)
    }
}

/// Join of two abstract types: equal types join to themselves; anything mixed
/// with a different type degrades to [`KType::Unknown`].
pub fn kjoin(a: KType, b: KType) -> KType {
    if a == KType::Unknown {
        b
    } else if b == KType::Unknown {
        a
    } else if a == b {
        a
    } else {
        KType::Unknown
    }
}

/// Arithmetic / comparison operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Bitwise/identity equality (produces a Bool).
    Eq,
    /// Boolean and/or of two truth values (both operands Bool).
    And,
    Or,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    fn is_cmp(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }
    fn is_divrem(self) -> bool {
        matches!(self, BinOp::Div | BinOp::Rem)
    }
}

/// Negation / logical not (result type depends on operand type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// Numeric conversion target for natively-inlined conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvTarget {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    F32,
    F64,
}

impl ConvTarget {
    pub fn from_typedesc(t: &TypeDesc) -> Option<ConvTarget> {
        Some(match t {
            TypeDesc::Int8 => ConvTarget::Int8,
            TypeDesc::Int16 => ConvTarget::Int16,
            TypeDesc::Int32 => ConvTarget::Int32,
            TypeDesc::Int64 => ConvTarget::Int64,
            TypeDesc::UInt8 => ConvTarget::UInt8,
            TypeDesc::UInt16 => ConvTarget::UInt16,
            TypeDesc::UInt32 => ConvTarget::UInt32,
            TypeDesc::UInt64 => ConvTarget::UInt64,
            TypeDesc::Float32 => ConvTarget::F32,
            TypeDesc::Float64 => ConvTarget::F64,
            _ => return None,
        })
    }

    pub fn result_ktype(self) -> KType {
        if self.is_int() {
            KType::Int
        } else {
            KType::Float
        }
    }

    pub fn is_int(self) -> bool {
        !matches!(self, ConvTarget::F32 | ConvTarget::F64)
    }

    /// True if this conversion narrows an integer (may overflow).
    fn is_narrowing_int(self) -> bool {
        matches!(
            self,
            ConvTarget::Int8 | ConvTarget::Int16 | ConvTarget::Int32
                | ConvTarget::UInt8 | ConvTarget::UInt16 | ConvTarget::UInt32
        )
    }
}

/// A single instruction, defining at most one virtual register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insn {
    /// Integer constant.
    ConstInt { dst: VReg, val: i64 },
    /// Float constant (raw f64 bits).
    ConstFloat { dst: VReg, bits: u64 },
    /// Boolean constant (tag qword: `3 | b << 8`).
    ConstBool { dst: VReg, val: bool },
    /// Character constant (tag qword: `4 | c << 8`).
    ConstChar { dst: VReg, ch: char },
    /// Null reference constant (a full value slot).
    ConstNull { dst: VReg },
    /// Unit constant (a full value slot).
    ConstUnit { dst: VReg },
    /// Load local slot (payload for known types, full slot otherwise).
    LdLocal { dst: VReg, idx: u16 },
    /// Store into a local slot.
    StLocal { idx: u16, src: VReg },
    /// Arithmetic / comparison on two same-typed scalars.
    Bin { dst: VReg, op: BinOp, a: VReg, b: VReg },
    /// Negation / logical not on one scalar.
    Un { dst: VReg, op: UnOp, a: VReg },
    /// Numeric conversion of a scalar.
    Conv { dst: VReg, to: ConvTarget, src: VReg },
    /// Delegate to a VM helper stub. `op_id` indexes
    /// [`Function::helper_ops`]; the helper receives `args` in order.
    Helper { dst: Option<VReg>, op_id: u32, args: Vec<VReg> },
}

impl Insn {
    pub(crate) fn defined(&self) -> Option<VReg> {
        match self {
            Insn::ConstInt { dst, .. }
            | Insn::ConstFloat { dst, .. }
            | Insn::ConstBool { dst, .. }
            | Insn::ConstChar { dst, .. }
            | Insn::ConstNull { dst }
            | Insn::ConstUnit { dst }
            | Insn::LdLocal { dst, .. }
            | Insn::Bin { dst, .. }
            | Insn::Un { dst, .. }
            | Insn::Conv { dst, .. } => Some(*dst),
            Insn::Helper { dst: Some(dst), .. } => Some(*dst),
            _ => None,
        }
    }

    pub(crate) fn uses(&self) -> Vec<VReg> {
        match self {
            Insn::StLocal { src, .. } => vec![*src],
            Insn::Bin { a, b, .. } => vec![*a, *b],
            Insn::Un { a, .. } => vec![*a],
            Insn::Conv { src, .. } => vec![*src],
            Insn::Helper { args, .. } => args.clone(),
            _ => Vec::new(),
        }
    }

    /// True if this instruction has side effects and must never be removed.
    fn has_side_effect(&self) -> bool {
        matches!(self, Insn::Helper { .. } | Insn::StLocal { .. })
    }
}

/// Terminator of a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// Return the value.
    Ret(VReg),
    /// Branch to a block.
    Br(usize),
    /// Branch to a block if the condition is truthy.
    BrTrue(VReg, usize),
    /// Branch to a block if the condition is falsy.
    BrFalse(VReg, usize),
    /// Control never reaches here.
    Unreachable,
}

/// A basic block.
#[derive(Debug, Clone)]
pub struct Block {
    /// Block id (index into [`Function::blocks`]).
    pub id: usize,
    /// Entry parameters (phi targets), one per stack slot at entry.
    pub params: Vec<VReg>,
    /// Instructions executed on entry before the terminator.
    pub insns: Vec<Insn>,
    /// Control-flow terminator.
    pub term: Term,
    /// Predecessor block ids.
    pub preds: Vec<usize>,
    /// Successor block ids.
    pub succs: Vec<usize>,
    /// Copy moves to execute on each edge to a successor: maps successor id
    /// to `(param_vreg, source_vreg)` pairs.
    pub edge_moves: HashMap<usize, Vec<(VReg, VReg)>>,
    /// Reachable from the entry point.
    pub reachable: bool,
}

/// A lowered method.
#[derive(Debug, Clone)]
pub struct Function {
    /// Original method id.
    pub method_id: MethodId,
    /// Human-readable name (for diagnostics).
    pub name: String,
    /// Blocks in layout order.
    pub blocks: Vec<Block>,
    /// Abstract type of each virtual register.
    pub types: Vec<KType>,
    /// Stable copies of the bytecode operations each helper delegates to,
    /// indexed by `Insn::Helper::op_id`.
    pub helper_ops: Vec<Op>,
    /// Number of local slots (parameters first).
    pub locals: u16,
    /// Number of parameters.
    pub params: u16,
    /// Abstract type of each local slot.
    pub local_types: Vec<KType>,
    /// Maximum stack depth used (for the argument marshalling area).
    pub max_stack: u16,
    /// Total spill area size in bytes, assigned by register allocation.
    pub spill_bytes: usize,
    /// Overflow-checking semantics captured at compile time.
    pub overflow_checks: bool,
}

impl Function {
    /// Number of bytes needed for the local slots (16 bytes each).
    pub fn locals_bytes(&self) -> usize {
        self.locals as usize * 16
    }

    /// Type of a virtual register.
    pub fn vtype(&self, v: VReg) -> KType {
        self.types[v as usize]
    }
}

/// Per-block analysis state (entry stack height, abstract types, and object
/// class of each stack slot where known).
#[derive(Debug, Clone)]
struct BlockState {
    height: Option<usize>,
    types: Vec<KType>,
    /// Class of each entry stack slot: `Some` when the value provably belongs
    /// to a single class (from `new`, `this`, or a typed field), `None` when
    /// unknown or polymorphic.
    classes: Vec<Option<ClassId>>,
}

impl Default for BlockState {
    fn default() -> Self {
        BlockState { height: None, types: Vec::new(), classes: Vec::new() }
    }
}

/// Lowers bytecode into the IR.
pub struct Lowerer<'a> {
    module: &'a Module,
    method: MethodDef,
    method_id: MethodId,
    overflow_checks: bool,

    // CFG
    ranges: Vec<(usize, usize)>,
    succs: Vec<Vec<usize>>,
    preds: Vec<Vec<usize>>,
    block_at: HashMap<usize, usize>,

    // Analysis
    state: Vec<BlockState>,
    local_types: Vec<KType>,
    /// Class of each local (parallel to `local_types`): `Some` when the value
    /// provably belongs to a single class.
    local_classes: Vec<Option<ClassId>>,

    // Lowering
    func: Function,
    next_vreg: VReg,
    block_params: Vec<Vec<VReg>>,
    /// Class of each pre-allocated block param (parallel to `block_params`).
    block_param_classes: Vec<Vec<Option<ClassId>>>,
}

impl<'a> Lowerer<'a> {
    pub fn new(module: &'a Module, method_id: MethodId, overflow_checks: bool) -> Result<Self, String> {
        let method = module
            .method(method_id)
            .ok_or_else(|| format!("unknown method {method_id:?}"))?
            .clone();
        Ok(Lowerer {
            module,
            method,
            method_id,
            overflow_checks,
            ranges: Vec::new(),
            succs: Vec::new(),
            preds: Vec::new(),
            block_at: HashMap::new(),
            state: Vec::new(),
            local_types: Vec::new(),
            local_classes: Vec::new(),
            func: Function {
                method_id,
                name: String::new(),
                blocks: Vec::new(),
                types: Vec::new(),
                helper_ops: Vec::new(),
                locals: 0,
                params: 0,
                local_types: Vec::new(),
                max_stack: 0,
                spill_bytes: 0,
                overflow_checks,
            },
            next_vreg: 0,
            block_params: Vec::new(),
            block_param_classes: Vec::new(),
        })
    }

    pub fn lower(mut self) -> Result<Function, String> {
        if !self.method.handlers.is_empty() {
            return Err("exception handlers are not supported by the JIT yet".into());
        }
        self.func.name = self
            .module
            .method(self.method_id)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("{:?}", self.method_id));
        self.func.locals = self.method.locals;
        self.func.params = self.method.params.len() as u16
            + if self.method.is_instance { 1 } else { 0 };
        self.func.max_stack = self.method.max_stack;
        self.build_cfg()?;
        self.analyze()?;
        self.func.local_types = self.local_types.clone();
        self.preallocate_params()?;
        self.lower_blocks()?;
        if self.func.blocks.is_empty() {
            return Err("no code".into());
        }
        Ok(self.func)
    }

    // ------------------------------------------------------------------
    // CFG construction
    // ------------------------------------------------------------------

    fn build_cfg(&mut self) -> Result<(), String> {
        let body = &self.method.body;
        let n = body.len();
        let mut is_start = vec![false; n + 1];
        is_start[0] = true;
        for (i, op) in body.iter().enumerate() {
            match op {
                Op::Br(t) | Op::BrTrue(t) | Op::BrFalse(t) => {
                    if (*t as usize) < n {
                        is_start[*t as usize] = true;
                    }
                    if i + 1 < n {
                        is_start[i + 1] = true;
                    }
                }
                Op::Ret | Op::Throw => {
                    if i + 1 < n {
                        is_start[i + 1] = true;
                    }
                }
                _ => {}
            }
        }
        let starts: Vec<usize> = (0..=n).filter(|i| is_start[*i]).collect();
        if starts.is_empty() {
            return Err("no basic blocks".into());
        }
        self.ranges = Vec::with_capacity(starts.len());
        for w in starts.windows(2) {
            self.ranges.push((w[0], w[1]));
        }
        let last = starts[starts.len() - 1];
        self.ranges.push((last, n));
        for (id, &(s, _)) in self.ranges.iter().enumerate() {
            self.block_at.insert(s, id);
        }

        self.succs = vec![Vec::new(); self.ranges.len()];
        self.preds = vec![Vec::new(); self.ranges.len()];
        for (id, &(s, e)) in self.ranges.iter().enumerate() {
            let t = self.find_terminator(s, e);
            let succs: Vec<usize> = if t >= e {
                match self.block_at.get(&e) {
                    Some(&f) => vec![f],
                    None => Vec::new(),
                }
            } else {
                match &body[t] {
                    Op::Br(off) => vec![self.block((*off) as usize)?],
                    Op::BrFalse(off) | Op::BrTrue(off) => {
                        let target = self.block((*off) as usize)?;
                        let mut v = vec![target];
                        let fall = t + 1;
                        if fall < n {
                            if let Some(f) = self.block_at.get(&fall).copied() {
                                if f != target {
                                    v.push(f);
                                }
                            }
                        }
                        v
                    }
                    _ => Vec::new(),
                }
            };
            self.succs[id] = succs.clone();
            for &s2 in &succs {
                self.preds[s2].push(id);
            }
        }
        Ok(())
    }

    fn block(&self, pc: usize) -> Result<usize, String> {
        self.block_at.get(&pc).copied().ok_or_else(|| format!("branch target {pc} is not a block"))
    }

    /// Index of the control-flow instruction terminating block `[s, e)`.
    fn find_terminator(&self, s: usize, e: usize) -> usize {
        let body = &self.method.body;
        let mut t = e;
        while t > s && !is_control(&body[t - 1]) {
            t -= 1;
        }
        if t == s {
            // No control op in [s, e): the block falls through to the next one.
            return e;
        }
        t - 1
    }

    // ------------------------------------------------------------------
    // Stack-height / type analysis
    // ------------------------------------------------------------------

    fn analyze(&mut self) -> Result<(), String> {
        let nb = self.ranges.len();
        self.state = (0..nb).map(|_| BlockState::default()).collect();
        self.local_types = (0..self.method.locals as usize)
            .map(|i| {
                let param_idx = i as i64 - if self.method.is_instance { 1 } else { 0 };
                if param_idx >= 0 && (param_idx as usize) < self.method.params.len() {
                    KType::from_typedesc(&self.method.params[param_idx as usize])
                } else {
                    KType::Unknown
                }
            })
            .collect();

        let mut local_types_changed = true;
        let mut iterations = 0;
        let limit = (nb * (self.method.max_stack as usize + 1) + 8).max(64);
        while local_types_changed {
            local_types_changed = false;
            self.state = (0..nb).map(|_| BlockState::default()).collect();
            self.state[0].height = Some(0);
            let mut queue: Vec<usize> = vec![0];
            let mut in_queue = vec![false; nb];
            in_queue[0] = true;
            let mut guard = 0;
            while let Some(b) = queue.pop() {
                in_queue[b] = false;
                let (mut h, mut stack) = match self.state[b].height {
                    Some(hh) => (hh, self.state[b].types.clone()),
                    None => continue,
                };
                let (s, e) = self.ranges[b];
                let t = self.find_terminator(s, e);
                for i in s..t {
                    self.sim_op(&mut stack, &mut h, &self.method.body[i].clone())?;
                }
                let mut candidates: Vec<(usize, usize, Vec<KType>)> = Vec::new();
                if t >= e {
                    // Straight-line block that falls through to the next one.
                    if let Some(f) = self.block_at.get(&e).copied() {
                        candidates.push((f, h, stack.clone()));
                    }
                } else {
                    let term_op = self.method.body[t].clone();
                    // A conditional branch pops its condition before either edge.
                    if matches!(term_op, Op::BrFalse(_) | Op::BrTrue(_)) {
                        if stack.is_empty() {
                            return Err("stack underflow at branch".into());
                        }
                        stack.pop();
                        h -= 1;
                    }
                    match &term_op {
                        Op::Br(off) => candidates.push((self.block((*off) as usize)?, h, stack.clone())),
                        Op::BrFalse(off) | Op::BrTrue(off) => {
                            let target = self.block((*off) as usize)?;
                            candidates.push((target, h, stack.clone()));
                            let fall = t + 1;
                            if let Some(f) = self.block_at.get(&fall).copied() {
                                if f != target {
                                    candidates.push((f, h, stack.clone()));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for (succ, hh, types) in candidates {
                    let changed = self.merge_state(succ, hh, types)?;
                    if changed && !in_queue[succ] {
                        in_queue[succ] = true;
                        queue.push(succ);
                    }
                }
                guard += 1;
                if guard > nb * 4 + 16 {
                    return Err("analysis did not converge".into());
                }
            }
            iterations += 1;
            if iterations > limit {
                return Err("local type analysis did not converge".into());
            }
        }
        Ok(())
    }

    /// Merge a predecessor's outgoing (height, types) into a block's state.
    /// Returns true if the block's entry types changed.
    fn merge_state(&mut self, block: usize, height: usize, types: Vec<KType>) -> Result<bool, String> {
        let st = &mut self.state[block];
        match st.height {
            None => {
                st.height = Some(height);
                st.types = types;
                Ok(true)
            }
            Some(h) => {
                if h != height {
                    return Err(format!(
                        "inconsistent stack height for block {block}: {h} vs {height}"
                    ));
                }
                if st.types.len() != types.len() {
                    return Err(format!(
                        "inconsistent stack depth for block {block}: {} vs {}",
                        st.types.len(),
                        types.len()
                    ));
                }
                let mut changed = false;
                for i in 0..st.types.len() {
                    let joined = kjoin(st.types[i], types[i]);
                    if joined != st.types[i] {
                        st.types[i] = joined;
                        changed = true;
                    }
                }
                Ok(changed)
            }
        }
    }

    /// Apply one op's stack effect to `(stack_types, height)`. Mutates
    /// `local_types` for stores so reads in later iterations see the type.
    fn sim_op(&mut self, stack: &mut Vec<KType>, h: &mut usize, op: &Op) -> Result<(), String> {
        let pop = |stack: &mut Vec<KType>, h: &mut usize| -> Result<KType, String> {
            if *h == 0 {
                return Err("stack underflow".into());
            }
            *h -= 1;
            Ok(stack.pop().unwrap_or(KType::Unknown))
        };
        let push = |stack: &mut Vec<KType>, h: &mut usize, ty: KType| {
            *h += 1;
            stack.push(ty);
        };
        match op {
            Op::Nop => {}
            Op::LdInt(_) | Op::LdInt64(_) => push(stack, h, KType::Int),
            Op::LdFloat(_) => push(stack, h, KType::Float),
            Op::Conv(ty) => {
                pop(stack, h)?;
                push(stack, h, KType::from_typedesc(ty));
            }
            Op::LdBool(_) => push(stack, h, KType::Bool),
            Op::LdStr(_) => push(stack, h, KType::Unknown),
            Op::LdChar(_) => push(stack, h, KType::Char),
            Op::LdNull => push(stack, h, KType::Unknown),
            Op::Ldloc(idx) | Op::Ldarg(idx) => {
                let k = self.local_types[*idx as usize];
                push(stack, h, k);
            }
            Op::Stloc(idx) => {
                let k = pop(stack, h)?;
                if let Some(slot) = self.local_types.get_mut(*idx as usize) {
                    *slot = kjoin(*slot, k);
                }
            }
            Op::Dup => {
                let k = stack.last().copied().unwrap_or(KType::Unknown);
                push(stack, h, k);
            }
            Op::Pop => {
                pop(stack, h)?;
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Eq | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let b = pop(stack, h)?;
                let a = pop(stack, h)?;
                let (op, ty) = bin_kind(op);
                push(stack, h, bin_result_kind(op, a, b, ty, self.overflow_checks));
            }
            Op::Neg | Op::Not => {
                let a = pop(stack, h)?;
                push(stack, h, un_result_kind(matches!(op, Op::Neg), a, self.overflow_checks));
            }
            Op::ValueEq | Op::And | Op::Or => {
                pop(stack, h)?;
                pop(stack, h)?;
                push(stack, h, KType::Bool);
            }
            Op::Hash | Op::EnumTag => {
                pop(stack, h)?;
                push(stack, h, KType::Int);
            }
            Op::NewClosure(_, count) => {
                for _ in 0..*count {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::Invoke(argc) => {
                for _ in 0..(*argc as usize + 1) {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::Br(_) => {}
            Op::BrFalse(_) | Op::BrTrue(_) => {
                pop(stack, h)?;
            }
            Op::Call(id) => {
                let params = self
                    .module
                    .method(*id)
                    .map(|m| m.params.len())
                    .ok_or("unknown method")?;
                for _ in 0..params {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::CallVirt(name) => {
                let arity = call_virt_arity(self.module, name)
                    .ok_or_else(|| format!("cannot determine arity of virtual call `{name}`"))?;
                for _ in 0..arity + 1 {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::CallSuper(id) => {
                let params = self
                    .module
                    .method(*id)
                    .map(|m| m.params.len())
                    .ok_or("unknown method")?;
                for _ in 0..params + 1 {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::NewObj(..) => push(stack, h, KType::Unknown),
            Op::Ldfld(_) => {
                pop(stack, h)?;
                push(stack, h, KType::Unknown);
            }
            Op::Stfld(_) => {
                pop(stack, h)?;
                pop(stack, h)?;
            }
            Op::Ldsfld(..) => push(stack, h, KType::Unknown),
            Op::Stsfld(..) => {
                pop(stack, h)?;
            }
            Op::NativeCall(id) => {
                let native = aura_bytecode::natives::NativeId::from_u16(*id)
                    .ok_or_else(|| format!("unknown native id {id}"))?;
                for _ in 0..native.arity() {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::IsInst(_) => {
                pop(stack, h)?;
                push(stack, h, KType::Bool);
            }
            Op::NewEnum(eid, _) => {
                let count = enum_field_count(self.module, *eid);
                for _ in 0..count {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::EnumField(_) => {
                pop(stack, h)?;
                push(stack, h, KType::Unknown);
            }
            Op::NewTuple(count) => {
                for _ in 0..*count {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::TupleField(_) => {
                pop(stack, h)?;
                push(stack, h, KType::Unknown);
            }
            Op::Print | Op::Throw => {
                pop(stack, h)?;
            }
            Op::PrintLn => {}
            Op::StringConcat(count) => {
                for _ in 0..*count {
                    pop(stack, h)?;
                }
                push(stack, h, KType::Unknown);
            }
            Op::Ret => {
                pop(stack, h)?;
            }
            Op::Break | Op::Continue => return Err("Break/Continue should have been resolved".into()),
            Op::EndFinally => return Err("EndFinally without handlers".into()),
        }
        Ok(())
    }

    fn preallocate_params(&mut self) -> Result<(), String> {
        let nb = self.ranges.len();
        let mut reachable = vec![false; nb];
        let mut queue = vec![0usize];
        reachable[0] = true;
        while let Some(b) = queue.pop() {
            for &s in &self.succs[b] {
                if !reachable[s] {
                    reachable[s] = true;
                    queue.push(s);
                }
            }
        }
        self.block_params = vec![Vec::new(); nb];
        for id in 0..nb {
            if !reachable[id] {
                continue;
            }
            let ty_count = self.state[id].types.len();
            for _ in 0..ty_count {
                let v = self.new_vreg(KType::Unknown);
                self.block_params[id].push(v);
            }
            // Assign real types to the pre-allocated param vregs.
            let types = self.state[id].types.clone();
            for (j, t) in types.iter().enumerate() {
                self.func.types[self.block_params[id][j] as usize] = *t;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Lowering to IR
    // ------------------------------------------------------------------

    fn new_vreg(&mut self, ty: KType) -> VReg {
        let v = self.next_vreg;
        self.next_vreg += 1;
        self.func.types.push(ty);
        v
    }

    fn helper_op(&mut self, op: Op) -> u32 {
        let id = self.func.helper_ops.len() as u32;
        self.func.helper_ops.push(op);
        id
    }

    fn lower_blocks(&mut self) -> Result<(), String> {
        let nb = self.ranges.len();
        let mut reachable = vec![false; nb];
        let mut queue = vec![0usize];
        reachable[0] = true;
        while let Some(b) = queue.pop() {
            for &s in &self.succs[b] {
                if !reachable[s] {
                    reachable[s] = true;
                    queue.push(s);
                }
            }
        }

        for id in 0..nb {
            if !reachable[id] {
                // Dead bytecode (e.g. the no-match throw of a match whose
                // last arm is irrefutable) still owns a block id. Downstream
                // passes index `func.blocks` by id and filter on
                // `reachable`, so an inert placeholder must occupy the slot
                // — skipping it would leave ids pointing past the end of a
                // shorter vec.
                self.func.blocks.push(Block {
                    id,
                    params: Vec::new(),
                    insns: Vec::new(),
                    term: Term::Unreachable,
                    preds: Vec::new(),
                    succs: Vec::new(),
                    edge_moves: HashMap::new(),
                    reachable: false,
                });
                continue;
            }
            let (s, e) = self.ranges[id];
            let t = self.find_terminator(s, e);
            let mut block = Block {
                id,
                params: self.block_params[id].clone(),
                insns: Vec::new(),
                term: Term::Unreachable,
                preds: self.preds[id].clone(),
                succs: self.succs[id].clone(),
                edge_moves: HashMap::new(),
                reachable: true,
            };

            let mut stack: Vec<VReg> = block.params.clone();

            for i in s..t {
                self.lower_op(&mut block, &mut stack, &self.method.body[i].clone())?;
            }

            let term = if t >= e {
                match self.block_at.get(&e) {
                    Some(&f) => {
                        self.finish_edge(&mut block, &mut stack, f);
                        Term::Br(f)
                    }
                    None => Term::Unreachable,
                }
            } else {
                let term_op = self.method.body[t].clone();
                match &term_op {
                    Op::Br(off) => {
                        let target = self.block((*off) as usize)?;
                        self.finish_edge(&mut block, &mut stack, target);
                        Term::Br(target)
                    }
                    Op::BrFalse(off) | Op::BrTrue(off) => {
                        let target = self.block((*off) as usize)?;
                        let cond = stack.pop().ok_or("stack underflow at branch")?;
                        self.finish_edge(&mut block, &mut stack, target);
                        let fall = t + 1;
                        if let Some(f) = self.block_at.get(&fall).copied() {
                            if f != target {
                                self.finish_edge(&mut block, &mut stack, f);
                            }
                        }
                        if matches!(term_op, Op::BrTrue(_)) {
                            Term::BrTrue(cond, target)
                        } else {
                            Term::BrFalse(cond, target)
                        }
                    }
                    Op::Ret => {
                        let result = stack.pop().ok_or("stack underflow at return")?;
                        Term::Ret(result)
                    }
                    // `throw` terminates the block through the helper's
                    // exception exit (the helper never returns normally);
                    // dropping it here — the old behavior — silently fell
                    // through into the next block.
                    Op::Throw => {
                        self.lower_op(&mut block, &mut stack, &term_op)?;
                        Term::Unreachable
                    }
                    _ => Term::Unreachable,
                }
            };
            block.term = term;
            self.func.blocks.push(block);
        }
        self.func.blocks.sort_by_key(|b| b.id);
        Ok(())
    }

    /// Record the copy moves required for `succ` given the current tail stack.
    fn finish_edge(&mut self, block: &mut Block, stack: &mut Vec<VReg>, succ: usize) {
        let param_count = self.state[succ].types.len();
        let mut moves = Vec::with_capacity(param_count);
        for j in 0..param_count {
            let src = stack.get(j).copied().unwrap_or_else(|| {
                // Stack slot not filled by this predecessor (dead edge); use a
                // fresh unknown so the move is harmless.
                let v = self.new_vreg(KType::Unknown);
                block.insns.push(Insn::ConstNull { dst: v });
                v
            });
            let param = self.block_params[succ][j];
            moves.push((param, src));
        }
        block.edge_moves.insert(succ, moves);
    }

    fn lower_op(&mut self, block: &mut Block, stack: &mut Vec<VReg>, op: &Op) -> Result<(), String> {
        let pop = |stack: &mut Vec<VReg>| -> Result<VReg, String> { Ok(stack.pop().ok_or("stack underflow")?) };

        macro_rules! helper {
            ($dst:expr, $op:expr, $args:expr) => {{
                let op_id = self.helper_op($op);
                block.insns.push(Insn::Helper {
                    dst: $dst,
                    op_id,
                    args: $args,
                });
            }};
        }

        match op {
            Op::Nop => {}
            Op::LdInt(i) => {
                let dst = self.new_vreg(KType::Int);
                block.insns.push(Insn::ConstInt { dst, val: *i as i64 });
                stack.push(dst);
            }
            Op::LdInt64(i) => {
                let dst = self.new_vreg(KType::Int);
                block.insns.push(Insn::ConstInt { dst, val: *i });
                stack.push(dst);
            }
            Op::LdFloat(idx) => {
                let s = self
                    .module
                    .constant_pool
                    .get(*idx as usize)
                    .ok_or("float constant out of range")?;
                let f = s.parse::<f64>().map_err(|e| format!("bad float constant: {e}"))?;
                let dst = self.new_vreg(KType::Float);
                block.insns.push(Insn::ConstFloat { dst, bits: f.to_bits() });
                stack.push(dst);
            }
            Op::Conv(ty) => {
                let src = pop(stack)?;
                let src_ty = self.func.types[src as usize];
                let target = match ConvTarget::from_typedesc(ty) {
                    Some(t) => t,
                    None => {
                        let dst = self.new_vreg(KType::Unknown);
                        helper!(Some(dst), op.clone(), vec![src]);
                        stack.push(dst);
                        return Ok(());
                    }
                };
                // float -> int always goes through the helper (saturating
                // semantics cannot be expressed with cvttsd2si).
                let needs_helper = (target.is_int() && src_ty == KType::Float)
                    || (src_ty == KType::Int && self.overflow_checks && target.is_narrowing_int())
                    || !src_ty.is_known();
                if needs_helper {
                    let dst = self.new_vreg(target.result_ktype());
                    helper!(Some(dst), op.clone(), vec![src]);
                    stack.push(dst);
                } else {
                    let dst = self.new_vreg(target.result_ktype());
                    block.insns.push(Insn::Conv { dst, to: target, src });
                    stack.push(dst);
                }
            }
            Op::LdBool(b) => {
                let dst = self.new_vreg(KType::Bool);
                block.insns.push(Insn::ConstBool { dst, val: *b });
                stack.push(dst);
            }
            Op::LdStr(_) => {
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), vec![]);
                stack.push(dst);
            }
            Op::LdChar(c) => {
                let ch = char::from_u32(*c).unwrap_or('\0');
                let dst = self.new_vreg(KType::Char);
                block.insns.push(Insn::ConstChar { dst, ch });
                stack.push(dst);
            }
            Op::LdNull => {
                let dst = self.new_vreg(KType::Unknown);
                block.insns.push(Insn::ConstNull { dst });
                stack.push(dst);
            }
            Op::Ldloc(idx) | Op::Ldarg(idx) => {
                let ty = self.local_types[*idx as usize];
                let dst = self.new_vreg(ty);
                block.insns.push(Insn::LdLocal { dst, idx: *idx });
                stack.push(dst);
            }
            Op::Stloc(idx) => {
                let src = pop(stack)?;
                block.insns.push(Insn::StLocal { idx: *idx, src });
            }
            Op::Dup => {
                let v = *stack.last().ok_or("stack underflow at dup")?;
                stack.push(v);
            }
            Op::Pop => {
                pop(stack)?;
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Eq | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let b = pop(stack)?;
                let a = pop(stack)?;
                let (ta, tb) = (self.func.types[a as usize], self.func.types[b as usize]);
                let (bop, tdesc) = bin_kind(op);
                let result_ty = bin_result_kind(bop, ta, tb, tdesc, self.overflow_checks);
                if bin_use_helper(bop, ta, tb, self.overflow_checks) {
                    let dst = self.new_vreg(result_ty);
                    helper!(Some(dst), op.clone(), vec![a, b]);
                    stack.push(dst);
                } else {
                    let dst = self.new_vreg(result_ty);
                    block.insns.push(Insn::Bin { dst, op: bop, a, b });
                    stack.push(dst);
                }
            }
            Op::Neg | Op::Not => {
                let a = pop(stack)?;
                let ta = self.func.types[a as usize];
                let is_neg = matches!(op, Op::Neg);
                let result_ty = un_result_kind(is_neg, ta, self.overflow_checks);
                if un_use_helper(is_neg, ta, self.overflow_checks) {
                    let dst = self.new_vreg(result_ty);
                    helper!(Some(dst), op.clone(), vec![a]);
                    stack.push(dst);
                } else {
                    let dst = self.new_vreg(result_ty);
                    let uop = if is_neg { UnOp::Neg } else { UnOp::Not };
                    block.insns.push(Insn::Un { dst, op: uop, a });
                    stack.push(dst);
                }
            }
            Op::ValueEq | Op::And | Op::Or => {
                let b = pop(stack)?;
                let a = pop(stack)?;
                let (ta, tb) = (self.func.types[a as usize], self.func.types[b as usize]);
                if matches!(op, Op::And | Op::Or) && ta == KType::Bool && tb == KType::Bool {
                    let dst = self.new_vreg(KType::Bool);
                    let bop = if matches!(op, Op::And) { BinOp::And } else { BinOp::Or };
                    block.insns.push(Insn::Bin { dst, op: bop, a, b });
                    stack.push(dst);
                } else {
                    let dst = self.new_vreg(KType::Bool);
                    helper!(Some(dst), op.clone(), vec![a, b]);
                    stack.push(dst);
                }
            }
            Op::Br(_) | Op::BrFalse(_) | Op::BrTrue(_) | Op::Ret => {
                unreachable!("control flow handled in terminator")
            }
            Op::Call(id) => {
                let params = self
                    .module
                    .method(*id)
                    .map(|m| m.params.len())
                    .ok_or("unknown method")?;
                let mut args = Vec::with_capacity(params);
                for _ in 0..params {
                    args.push(pop(stack)?);
                }
                args.reverse();
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), args);
                stack.push(dst);
            }
            Op::CallVirt(name) => {
                let arity = call_virt_arity(self.module, name)
                    .ok_or_else(|| format!("cannot determine arity of virtual call `{name}`"))?;
                let instance = pop(stack)?;
                let mut args = Vec::with_capacity(arity + 1);
                for _ in 0..arity {
                    args.push(pop(stack)?);
                }
                args.reverse();
                args.insert(0, instance);
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), args);
                stack.push(dst);
            }
            Op::CallSuper(id) => {
                let params = self
                    .module
                    .method(*id)
                    .map(|m| m.params.len())
                    .ok_or("unknown method")?;
                let instance = pop(stack)?;
                let mut args = Vec::with_capacity(params + 1);
                for _ in 0..params {
                    args.push(pop(stack)?);
                }
                args.reverse();
                args.insert(0, instance);
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), args);
                stack.push(dst);
            }
            Op::NewObj(..) => {
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), vec![]);
                stack.push(dst);
            }
            Op::Ldfld(_) => {
                let instance = pop(stack)?;
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), vec![instance]);
                stack.push(dst);
            }
            Op::Stfld(_) => {
                let instance = pop(stack)?;
                let value = pop(stack)?;
                helper!(None, op.clone(), vec![instance, value]);
            }
            Op::Ldsfld(..) => {
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), vec![]);
                stack.push(dst);
            }
            Op::NativeCall(id) => {
                let native = aura_bytecode::natives::NativeId::from_u16(*id)
                    .ok_or_else(|| format!("unknown native id {id}"))?;
                let mut args = Vec::with_capacity(native.arity());
                for _ in 0..native.arity() {
                    args.push(pop(stack)?);
                }
                args.reverse();
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), args);
                stack.push(dst);
            }
            Op::IsInst(_) => {
                let v = pop(stack)?;
                let dst = self.new_vreg(KType::Bool);
                helper!(Some(dst), op.clone(), vec![v]);
                stack.push(dst);
            }
            Op::NewClosure(_, count) => {
                let mut caps = Vec::with_capacity(*count as usize);
                for _ in 0..*count {
                    caps.push(pop(stack)?);
                }
                caps.reverse();
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), caps);
                stack.push(dst);
            }
            Op::Invoke(argc) => {
                // Stack: args (in order), closure on top; the helper wants
                // [closure, args...].
                let closure = pop(stack)?;
                let mut args = Vec::with_capacity(*argc as usize + 1);
                for _ in 0..*argc {
                    args.push(pop(stack)?);
                }
                args.reverse();
                args.insert(0, closure);
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), args);
                stack.push(dst);
            }
            Op::Stsfld(..) => {
                let value = pop(stack)?;
                helper!(None, op.clone(), vec![value]);
            }
            Op::NewEnum(eid, _) => {
                let count = enum_field_count(self.module, *eid);
                let mut fields = Vec::with_capacity(count);
                for _ in 0..count {
                    fields.push(pop(stack)?);
                }
                fields.reverse();
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), fields);
                stack.push(dst);
            }
            Op::EnumTag => {
                let e = pop(stack)?;
                let dst = self.new_vreg(KType::Int);
                helper!(Some(dst), op.clone(), vec![e]);
                stack.push(dst);
            }
            Op::Hash => {
                let v = pop(stack)?;
                let dst = self.new_vreg(KType::Int);
                helper!(Some(dst), op.clone(), vec![v]);
                stack.push(dst);
            }
            Op::EnumField(idx) => {
                let ty = enum_field_common_type(self.module, *idx).unwrap_or(KType::Unknown);
                let e = pop(stack)?;
                let dst = self.new_vreg(ty);
                helper!(Some(dst), op.clone(), vec![e]);
                stack.push(dst);
            }
            Op::NewTuple(count) => {
                let mut elems = Vec::with_capacity(*count as usize);
                for _ in 0..*count {
                    elems.push(pop(stack)?);
                }
                elems.reverse();
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), elems);
                stack.push(dst);
            }
            Op::TupleField(_) => {
                let t = pop(stack)?;
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), vec![t]);
                stack.push(dst);
            }
            Op::Print => {
                let v = pop(stack)?;
                helper!(None, op.clone(), vec![v]);
            }
            Op::PrintLn => {
                helper!(None, op.clone(), vec![]);
            }
            Op::StringConcat(count) => {
                let mut parts = Vec::with_capacity(*count as usize);
                for _ in 0..*count {
                    parts.push(pop(stack)?);
                }
                parts.reverse();
                let dst = self.new_vreg(KType::Unknown);
                helper!(Some(dst), op.clone(), parts);
                stack.push(dst);
            }
            Op::Throw => {
                let exc = pop(stack)?;
                helper!(None, op.clone(), vec![exc]);
            }
            Op::Break | Op::Continue => return Err("Break/Continue should have been resolved".into()),
            Op::EndFinally => return Err("EndFinally without handlers".into()),
        }
        Ok(())
    }
}

fn is_control(op: &Op) -> bool {
    matches!(op, Op::Br(_) | Op::BrFalse(_) | Op::BrTrue(_) | Op::Ret | Op::Throw)
}

/// Translate a bytecode arithmetic op to a [`BinOp`].
fn bin_kind(op: &Op) -> (BinOp, &'static str) {
    match op {
        Op::Add => (BinOp::Add, "int or float"),
        Op::Sub => (BinOp::Sub, "int or float"),
        Op::Mul => (BinOp::Mul, "int or float"),
        Op::Div => (BinOp::Div, "int or float"),
        Op::Rem => (BinOp::Rem, "int or float"),
        Op::Eq => (BinOp::Eq, "comparable"),
        Op::Lt => (BinOp::Lt, "int or float"),
        Op::Le => (BinOp::Le, "int or float"),
        Op::Gt => (BinOp::Gt, "int or float"),
        Op::Ge => (BinOp::Ge, "int or float"),
        _ => unreachable!(),
    }
}

/// Abstract result type of a binary op for the given operand types. The
/// `desc` is only used to disambiguate Eq from ordered comparisons.
fn bin_result_kind(op: BinOp, a: KType, b: KType, _desc: &str, _checks: bool) -> KType {
    if op.is_cmp() {
        return KType::Bool;
    }
    if a == b {
        match a {
            KType::Int => {
                if op.is_divrem() {
                    // The helper path returns an untyped value.
                    KType::Unknown
                } else {
                    KType::Int
                }
            }
            KType::Float => {
                if op.is_divrem() {
                    KType::Unknown
                } else {
                    KType::Float
                }
            }
            _ => KType::Unknown,
        }
    } else {
        KType::Unknown
    }
}

/// True if a binary op must be delegated to a helper for these operand types.
fn bin_use_helper(op: BinOp, a: KType, b: KType, _checks: bool) -> bool {
    if op.is_cmp() {
        return !(a == b && matches!(a, KType::Int | KType::Float | KType::Bool | KType::Char));
    }
    if op.is_divrem() {
        // Div/rem always go through the dispatcher so division by zero,
        // overflow and float `fmod` share the interpreter's semantics.
        return true;
    }
    !(a == b && matches!(a, KType::Int | KType::Float))
}

/// Abstract result type of a unary op.
fn un_result_kind(is_neg: bool, a: KType, _checks: bool) -> KType {
    if is_neg {
        match a {
            KType::Int | KType::Float => a,
            _ => KType::Unknown,
        }
    } else {
        KType::Bool
    }
}

/// True if a unary op must be delegated to a helper.
fn un_use_helper(is_neg: bool, a: KType, _checks: bool) -> bool {
    if is_neg {
        !matches!(a, KType::Int | KType::Float)
    } else {
        !a.is_known()
    }
}

fn enum_field_count(module: &Module, eid: EnumId) -> usize {
    module
        .enums
        .get(&eid)
        .and_then(|e| e.variants.first())
        .map(|v| v.fields.len())
        .unwrap_or(0)
}

/// Arity of a virtual method call by name. Every class in the module that can
/// resolve the method must agree on the parameter count (they do unless the
/// method is overloaded with different arities).
pub fn call_virt_arity(module: &Module, name: &str) -> Option<usize> {
    let mut arity: Option<usize> = None;
    let mut visited: Vec<u32> = Vec::new();
    let class_ids: Vec<ClassId> = module.classes.keys().copied().collect();
    for cid in class_ids {
        visited.clear();
        if let Some(m) = resolve_method_by_name(module, cid, name, &mut visited) {
            let a = m.params.len();
            match arity {
                None => arity = Some(a),
                Some(a0) if a0 != a => return None,
                _ => {}
            }
        }
    }
    arity
}

fn resolve_method_by_name<'m>(
    module: &'m Module,
    class_id: ClassId,
    name: &str,
    visited: &mut Vec<u32>,
) -> Option<&'m MethodDef> {
    if visited.contains(&class_id.0) {
        return None;
    }
    visited.push(class_id.0);
    let class = module.classes.get(&class_id)?;
    for (_, m) in class.methods.iter().chain(class.static_methods.iter()) {
        if m.name == name {
            return Some(m);
        }
    }
    if let Some(sup) = class.super_class {
        if let Some(m) = resolve_method_by_name(module, sup, name, visited) {
            return Some(m);
        }
    }
    for iface in &class.interfaces {
        if let Some(m) = resolve_method_by_name(module, *iface, name, visited) {
            return Some(m);
        }
    }
    None
}

/// Common abstract type of an enum field across all variants; `None` if the
/// variants disagree (falls back to dynamic handling).
fn enum_field_common_type(module: &Module, idx: u16) -> Option<KType> {
    let mut result: Option<KType> = None;
    for e in module.enums.values() {
        for v in &e.variants {
            if let Some(f) = v.fields.get(idx as usize) {
                let k = KType::from_typedesc(&f.ty);
                match result {
                    None => result = Some(k),
                    Some(k0) if k0 != k => return None,
                    _ => {}
                }
            }
        }
    }
    result.filter(|k| k.is_known())
}

// ---------------------------------------------------------------------
// Optimization
// ---------------------------------------------------------------------

/// Run optimization passes: constant folding, dead code elimination, and
/// trivial branch folding.
pub fn optimize(func: &mut Function) {
    constant_fold(func);
    fold_trivial_branches(func);
    dead_code(func);
}

fn const_int(func: &Function, v: VReg) -> Option<i64> {
    for b in &func.blocks {
        for i in &b.insns {
            if let Insn::ConstInt { dst, val } = i {
                if *dst == v {
                    return Some(*val);
                }
            }
        }
    }
    None
}

fn const_float(func: &Function, v: VReg) -> Option<u64> {
    for b in &func.blocks {
        for i in &b.insns {
            if let Insn::ConstFloat { dst, bits } = i {
                if *dst == v {
                    return Some(*bits);
                }
            }
        }
    }
    None
}

fn const_bool(func: &Function, v: VReg) -> Option<bool> {
    for b in &func.blocks {
        for i in &b.insns {
            if let Insn::ConstBool { dst, val } = i {
                if *dst == v {
                    return Some(*val);
                }
            }
        }
    }
    None
}

/// Fold constant arithmetic into constants.
fn constant_fold(func: &mut Function) {
    let mut changed = true;
    let guard = 1 + func.blocks.len() * 2;
    let mut rounds = 0;
    while changed && rounds < guard {
        changed = false;
        rounds += 1;
        for bi in 0..func.blocks.len() {
            for j in 0..func.blocks[bi].insns.len() {
                let key = func.blocks[bi].insns[j].clone();
                let fold = match key {
                    Insn::Bin { dst, op, a, b } => fold_bin(func, dst, op, a, b),
                    Insn::Un { dst, op, a } => fold_un(func, dst, op, a),
                    _ => None,
                };
                if let Some(new_insn) = fold {
                    func.blocks[bi].insns[j] = new_insn;
                    changed = true;
                }
            }
        }
    }
}

fn fold_bin(func: &Function, dst: VReg, op: BinOp, a: VReg, b: VReg) -> Option<Insn> {
    let f64_op = |f: fn(f64, f64) -> f64| {
        let (x, y) = (const_float(func, a)?, const_float(func, b)?);
        Some(Insn::ConstFloat { dst, bits: f(f64::from_bits(x), f64::from_bits(y)).to_bits() })
    };
    let cmp = |pred: fn(i64, i64) -> bool| {
        let (x, y) = (const_int(func, a)?, const_int(func, b)?);
        Some(Insn::ConstBool { dst, val: pred(x, y) })
    };
    let logical = |pred: fn(bool, bool) -> bool| {
        let (x, y) = (const_bool(func, a)?, const_bool(func, b)?);
        Some(Insn::ConstBool { dst, val: pred(x, y) })
    };
    match op {
        BinOp::Add => {
            if let (Some(x), Some(y)) = (const_int(func, a), const_int(func, b)) {
                let r = if func.overflow_checks { x.checked_add(y) } else { Some(x.wrapping_add(y)) };
                return r.map(|v| Insn::ConstInt { dst, val: v });
            }
            f64_op(|x, y| x + y)
        }
        BinOp::Sub => {
            if let (Some(x), Some(y)) = (const_int(func, a), const_int(func, b)) {
                let r = if func.overflow_checks { x.checked_sub(y) } else { Some(x.wrapping_sub(y)) };
                return r.map(|v| Insn::ConstInt { dst, val: v });
            }
            f64_op(|x, y| x - y)
        }
        BinOp::Mul => {
            if let (Some(x), Some(y)) = (const_int(func, a), const_int(func, b)) {
                let r = if func.overflow_checks { x.checked_mul(y) } else { Some(x.wrapping_mul(y)) };
                return r.map(|v| Insn::ConstInt { dst, val: v });
            }
            f64_op(|x, y| x * y)
        }
        BinOp::Div => {
            if let (Some(x), Some(y)) = (const_int(func, a), const_int(func, b)) {
                if y == 0 {
                    return None;
                }
                let r = if func.overflow_checks { x.checked_div(y) } else { Some(x.wrapping_div(y)) };
                return r.map(|v| Insn::ConstInt { dst, val: v });
            }
            if let (Some(x), Some(y)) = (const_float(func, a), const_float(func, b)) {
                if f64::from_bits(y) == 0.0 {
                    return None;
                }
                return Some(Insn::ConstFloat { dst, bits: (f64::from_bits(x) / f64::from_bits(y)).to_bits() });
            }
            None
        }
        BinOp::Rem => {
            if let (Some(x), Some(y)) = (const_int(func, a), const_int(func, b)) {
                if y == 0 {
                    return None;
                }
                let r = if func.overflow_checks { x.checked_rem(y) } else { Some(x.wrapping_rem(y)) };
                return r.map(|v| Insn::ConstInt { dst, val: v });
            }
            None
        }
        BinOp::Eq => {
            if let (Some(x), Some(y)) = (const_int(func, a), const_int(func, b)) {
                return Some(Insn::ConstBool { dst, val: x == y });
            }
            if let (Some(x), Some(y)) = (const_float(func, a), const_float(func, b)) {
                return Some(Insn::ConstBool { dst, val: f64::from_bits(x) == f64::from_bits(y) });
            }
            if let (Some(x), Some(y)) = (const_bool(func, a), const_bool(func, b)) {
                return Some(Insn::ConstBool { dst, val: x == y });
            }
            None
        }
        BinOp::Lt => cmp(|x, y| x < y),
        BinOp::Le => cmp(|x, y| x <= y),
        BinOp::Gt => cmp(|x, y| x > y),
        BinOp::Ge => cmp(|x, y| x >= y),
        BinOp::And => logical(|x, y| x && y),
        BinOp::Or => logical(|x, y| x || y),
    }
}

fn fold_un(func: &Function, dst: VReg, op: UnOp, a: VReg) -> Option<Insn> {
    match op {
        UnOp::Neg => {
            if let Some(x) = const_int(func, a) {
                let r = if func.overflow_checks { x.checked_neg() } else { Some(x.wrapping_neg()) };
                return r.map(|v| Insn::ConstInt { dst, val: v });
            }
            const_float(func, a).map(|x| Insn::ConstFloat { dst, bits: (-f64::from_bits(x)).to_bits() })
        }
        UnOp::Not => {
            if let Some(x) = const_int(func, a) {
                return Some(Insn::ConstBool { dst, val: x == 0 });
            }
            if let Some(x) = const_float(func, a) {
                return Some(Insn::ConstBool { dst, val: f64::from_bits(x) == 0.0 });
            }
            if let Some(x) = const_bool(func, a) {
                return Some(Insn::ConstBool { dst, val: !x });
            }
            None
        }
    }
}

/// Fold branches on constant conditions.
fn fold_trivial_branches(func: &mut Function) {
    let mut i = 0;
    while i < func.blocks.len() {
        let (cond, target, is_true) = match &func.blocks[i].term {
            Term::BrTrue(c, t) => (*c, *t, true),
            Term::BrFalse(c, t) => (*c, *t, false),
            _ => {
                i += 1;
                continue;
            }
        };
        let truthy = const_bool(func, cond).or_else(|| const_int(func, cond).map(|v| v != 0));
        let Some(tv) = truthy else {
            i += 1;
            continue;
        };
        let take = if is_true { tv } else { !tv };
        let succs = func.blocks[i].succs.clone();
        let fall = succs.iter().copied().find(|s| *s != target);
        let taken = if take { Some(target) } else { fall };
        if let Some(taken) = taken {
            // Drop the dead successor.
            for &s in &succs {
                if s != taken {
                    if let Some(b) = func.blocks.get_mut(s) {
                        b.preds.retain(|p| *p != i);
                    }
                }
            }
            func.blocks[i].succs.retain(|s| *s == taken);
            func.blocks[i].edge_moves.retain(|k, _| *k == taken);
            func.blocks[i].term = Term::Br(taken);
        } else {
            // No live successor (e.g. BrFalse to self with no fallthrough);
            // keep a branch to the only edge we know.
            func.blocks[i].term = Term::Br(target);
        }
    }
}

/// Remove pure instructions whose results are never used.
fn dead_code(func: &mut Function) {
    for _round in 0..4 {
        // Roots: everything a side-effecting insn, terminator, or edge move
        // uses, plus the values defined by side-effecting instructions
        // themselves (kept regardless).
        let mut roots: Vec<VReg> = Vec::new();
        let mut marked: Vec<bool> = vec![false; func.types.len()];
        for b in &func.blocks {
            for insn in &b.insns {
                if insn.has_side_effect() {
                    for u in insn.uses() {
                        roots.push(u);
                    }
                }
            }
            match &b.term {
                Term::Ret(v) => roots.push(*v),
                Term::BrTrue(v, _) | Term::BrFalse(v, _) => roots.push(*v),
                _ => {}
            }
            for (_, moves) in &b.edge_moves {
                for (_, src) in moves {
                    roots.push(*src);
                }
            }
        }
        let mut stack = roots;
        while let Some(v) = stack.pop() {
            if v as usize >= marked.len() || marked[v as usize] {
                continue;
            }
            marked[v as usize] = true;
            for b in &func.blocks {
                for insn in &b.insns {
                    if insn.defined() == Some(v) {
                        for u in insn.uses() {
                            stack.push(u);
                        }
                    }
                }
            }
        }
        let mut removed = false;
        for b in &mut func.blocks {
            let mut kept: Vec<Insn> = Vec::with_capacity(b.insns.len());
            for insn in b.insns.drain(..) {
                let keep = match insn.defined() {
                    None => true,
                    Some(v) => insn.has_side_effect() || marked[v as usize],
                };
                if keep {
                    kept.push(insn);
                } else {
                    removed = true;
                }
            }
            b.insns = kept;
        }
        if !removed {
            break;
        }
    }
}

/// Render the IR as text (for diagnostics and tests).
pub fn dump(func: &Function) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "method {} ({} blocks)\n",
        func.name,
        func.blocks.len()
    ));
    for b in &func.blocks {
        out.push_str(&format!("b{}: params={:?}\n", b.id, b.params));
        for insn in &b.insns {
            out.push_str(&format!("  {insn:?}\n"));
        }
        out.push_str(&format!("  term: {:?}\n", b.term));
        for (s, m) in &b.edge_moves {
            out.push_str(&format!("  edge -> b{s}: {m:?}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_bytecode::{ClassDef, ExceptionHandler};
    use std::collections::HashMap;

    fn module(body: Vec<Op>, params: Vec<TypeDesc>, locals: u16, max_stack: u16) -> Module {
        let mut methods = HashMap::new();
        methods.insert(
            MethodId(1),
            MethodDef {
                name: "test".into(),
                return_ty: TypeDesc::Unit,
                params,
                generic_params: Vec::new(),
                is_instance: false,
                body,
                handlers: Vec::new(),
                max_stack,
                locals,
            },
        );
        let mut classes = HashMap::new();
        classes.insert(
            ClassId(0),
            ClassDef {
                name: "Test".into(),
                generic_params: Vec::new(),
                super_class: None,
                interfaces: Vec::new(),
                is_interface: false,
                is_abstract: false,
                is_record: false,
                fields: Vec::new(),
                static_fields: Vec::new(),
                methods: HashMap::new(),
                static_methods: methods,
            },
        );
        Module {
            name: "test".into(),
            classes,
            enums: HashMap::new(),
            entrypoint: None,
            constant_pool: vec!["1.5".into(), "3.25".into()],
        }
    }

    fn lower(body: Vec<Op>, params: Vec<TypeDesc>, locals: u16, max_stack: u16) -> Function {
        let m = module(body, params, locals, max_stack);
        Lowerer::new(&m, MethodId(1), false).unwrap().lower().unwrap()
    }

    fn instance_module(body: Vec<Op>, params: Vec<TypeDesc>, locals: u16, max_stack: u16) -> Module {
        let mut methods = HashMap::new();
        methods.insert(
            MethodId(1),
            MethodDef {
                name: "test".into(),
                return_ty: TypeDesc::Unit,
                params,
                generic_params: Vec::new(),
                is_instance: true,
                body,
                handlers: Vec::new(),
                max_stack,
                locals,
            },
        );
        let mut classes = HashMap::new();
        classes.insert(
            ClassId(0),
            ClassDef {
                name: "Test".into(),
                generic_params: Vec::new(),
                super_class: None,
                interfaces: Vec::new(),
                is_interface: false,
                is_abstract: false,
                is_record: false,
                fields: Vec::new(),
                static_fields: Vec::new(),
                methods,
                static_methods: HashMap::new(),
            },
        );
        Module {
            name: "test".into(),
            classes,
            enums: HashMap::new(),
            entrypoint: None,
            constant_pool: vec![],
        }
    }

    #[test]
    fn instance_param_local_types_include_this() {
        let m = instance_module(
            vec![Op::Ldarg(0), Op::Ldarg(1), Op::Mul, Op::Ret],
            vec![TypeDesc::Float64, TypeDesc::Int64],
            3,
            2,
        );
        let f = Lowerer::new(&m, MethodId(1), false).unwrap().lower().unwrap();
        assert_eq!(f.params, 3, "this + two params");
        assert_eq!(f.local_types[0], KType::Unknown, "local 0 is `this`");
        assert_eq!(f.local_types[1], KType::Float, "local 1 is the float param");
        assert_eq!(f.local_types[2], KType::Int, "local 2 is the int param");
        // Mixed float x int cannot inline; it must route through a helper.
        assert!(!f.helper_ops.is_empty());
    }

    #[test]
    fn lowers_loop_with_fallthrough_block() {
        // int r = 0; while (r < 2) { r = r + 1; } return r;
        let body = vec![
            Op::LdInt(0),
            Op::Stloc(0),
            Op::Ldloc(0),
            Op::LdInt(2),
            Op::Lt,
            Op::BrFalse(11),
            Op::Ldloc(0),
            Op::LdInt(1),
            Op::Add,
            Op::Stloc(0),
            Op::Br(2),
            Op::Ldloc(0),
            Op::Ret,
        ];
        let f = lower(body, vec![], 1, 2);
        let reachable: Vec<_> = f.blocks.iter().filter(|b| b.reachable).collect();
        assert_eq!(reachable.len(), 4);
        // b0 is the pre-header; it ends without a control op and must fall
        // through to b1 (the loop head) instead of dropping its instructions.
        assert_eq!(f.blocks[0].insns.len(), 2);
        assert!(matches!(f.blocks[0].term, Term::Br(1)));
        assert_eq!(f.blocks[0].succs, vec![1]);
        assert_eq!(f.blocks[0].preds, vec![]);
        // b1 (loop head) branches to b3 (exit) or falls into b2 (body).
        assert!(matches!(f.blocks[1].term, Term::BrFalse(_, 3)));
        assert_eq!(f.blocks[1].succs, vec![3, 2]);
        assert_eq!(f.blocks[2].preds, vec![1]);
        assert_eq!(f.blocks[2].succs, vec![1], "body loops back to the head");
        assert!(matches!(f.blocks[3].term, Term::Ret(_)));
        assert_eq!(f.blocks[3].preds, vec![1]);
    }

    #[test]
    fn int_mul_inlines() {
        let f = lower(vec![Op::LdInt(7), Op::LdInt(2), Op::Mul, Op::Ret], vec![], 0, 2);
        assert!(f.helper_ops.is_empty(), "int mul must not route to a helper");
        let muls = f.blocks[0]
            .insns
            .iter()
            .filter(|i| matches!(i, Insn::Bin { op: BinOp::Mul, .. }))
            .count();
        assert_eq!(muls, 1);
        match &f.blocks[0].term {
            Term::Ret(v) => assert_eq!(f.vtype(*v), KType::Int),
            other => panic!("expected Ret, got {other:?}"),
        }
    }

    #[test]
    fn lowers_int_add() {
        let f = lower(vec![Op::LdInt(3), Op::LdInt(4), Op::Add, Op::Ret], vec![], 0, 2);
        assert_eq!(f.params, 0);
        assert_eq!(f.locals, 0);
        assert!(f.helper_ops.is_empty());
        assert_eq!(f.blocks.len(), 1);
        let block = &f.blocks[0];
        assert!(block.reachable);
        assert_eq!(block.preds, vec![]);
        assert_eq!(block.succs, vec![]);
        let adds: Vec<_> = block
            .insns
            .iter()
            .filter_map(|i| match i {
                Insn::Bin { op: BinOp::Add, a, b, .. } => Some((*a, *b)),
                _ => None,
            })
            .collect();
        assert_eq!(adds.len(), 1);
        let (a, b) = adds[0];
        assert_eq!(f.vtype(a), KType::Int);
        assert_eq!(f.vtype(b), KType::Int);
        match &block.term {
            Term::Ret(v) => assert_eq!(f.vtype(*v), KType::Int),
            other => panic!("expected Ret, got {other:?}"),
        }
    }

    #[test]
    fn lowers_params_and_locals() {
        let f = lower(
            vec![Op::Ldarg(0), Op::Ldarg(1), Op::Add, Op::Stloc(2), Op::Ldloc(2), Op::Ret],
            vec![TypeDesc::Int64, TypeDesc::Int32],
            3,
            2,
        );
        assert_eq!(f.params, 2);
        assert_eq!(f.locals, 3);
        assert_eq!(f.local_types[0], KType::Int);
        assert_eq!(f.local_types[1], KType::Int);
        // Slot 2 starts Unknown and receives an Int store, so it becomes Int
        // (Unknown is the lattice bottom: `Unknown ⊔ Int = Int`). Reads after a
        // store therefore see the stored type instead of routing every local op
        // through the dispatcher as an untyped box.
        assert_eq!(f.local_types[2], KType::Int);
        let loads: Vec<u16> = f.blocks[0]
            .insns
            .iter()
            .filter_map(|i| match i {
                Insn::LdLocal { idx, .. } => Some(*idx),
                Insn::StLocal { idx, .. } => Some(*idx),
                _ => None,
            })
            .collect();
        assert_eq!(loads, vec![0, 1, 2, 2]);
    }

    #[test]
    fn lowers_float_add() {
        let f = lower(vec![Op::LdFloat(0), Op::LdFloat(1), Op::Add, Op::Ret], vec![], 0, 2);
        let adds: Vec<_> = f.blocks[0]
            .insns
            .iter()
            .filter_map(|i| match i {
                Insn::Bin { op: BinOp::Add, a, b, .. } => Some((*a, *b)),
                _ => None,
            })
            .collect();
        assert_eq!(adds.len(), 1);
        assert_eq!(f.vtype(adds[0].0), KType::Float);
        match &f.blocks[0].term {
            Term::Ret(v) => assert_eq!(f.vtype(*v), KType::Float),
            other => panic!("expected Ret, got {other:?}"),
        }
    }

    #[test]
    fn lowers_branch_with_edge_moves() {
        let body = vec![
            Op::LdInt(1),
            Op::LdInt(2),
            Op::Lt,
            Op::BrFalse(6),
            Op::LdInt(9),
            Op::Ret,
            Op::LdInt(10),
            Op::Ret,
        ];
        let f = lower(body, vec![], 0, 2);
        let reachable: Vec<_> = f.blocks.iter().filter(|b| b.reachable).collect();
        // b0: LdInt/Lt/BrFalse; b1: then-path (LdInt 9, Ret); b2: else-path.
        assert_eq!(reachable.len(), 3);
        let b0 = &f.blocks[0];
        assert!(matches!(b0.term, Term::BrFalse(_, _)));
        assert_eq!(b0.succs, vec![2, 1]);
        assert_eq!(f.blocks[1].preds, vec![0]);
        assert_eq!(f.blocks[2].preds, vec![0]);
        assert!(matches!(f.blocks[1].term, Term::Ret(_)));
        assert!(matches!(f.blocks[2].term, Term::Ret(_)));
    }

    #[test]
    fn rejects_exception_handlers() {
        let m = module(vec![Op::LdInt(1), Op::Ret], vec![], 0, 1);
        let mut m = m;
        for class in m.classes.values_mut() {
            for method in class.static_methods.values_mut() {
                method.handlers = vec![ExceptionHandler {
                    start: 0,
                    end: 2,
                    catch_type: None,
                    handler_pc: 2,
                    catch_local: 0,
                }];
            }
        }
        let err = Lowerer::new(&m, MethodId(1), false).unwrap().lower().unwrap_err();
        assert!(err.contains("exception handlers"), "unexpected error: {err}");
    }

    #[test]
    fn div_rem_route_to_helpers() {
        for op in [Op::Div, Op::Rem] {
            let f = lower(vec![Op::LdInt(7), Op::LdInt(2), op.clone(), Op::Ret], vec![], 0, 2);
            assert!(!f.helper_ops.is_empty(), "{op:?} must delegate to a helper");
            let helpers = f.blocks[0].insns.iter().filter(|i| matches!(i, Insn::Helper { .. })).count();
            assert_eq!(helpers, 1, "exactly one helper insn for {op:?}");
            match &f.blocks[0].term {
                // Helper results are untyped boxes.
                Term::Ret(v) => assert_eq!(f.vtype(*v), KType::Unknown),
                other => panic!("expected Ret, got {other:?}"),
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn pipeline_lower_optimize_allocate() {
        // A `div` lowers to a helper call whose result is an untyped box, so the
        // allocator must reserve a 16-byte spill slot even though the rest of the
        // arithmetic fits in registers.
        let mut f = lower(
            vec![
                Op::LdInt(6),
                Op::LdInt(3),
                Op::Div,
                Op::Pop,
                Op::LdInt(1),
                Op::LdInt(2),
                Op::Add,
                Op::Stloc(0),
                Op::Ldloc(0),
                Op::Ret,
            ],
            vec![],
            1,
            2,
        );
        super::optimize(&mut f);
        let alloc = crate::jit::regalloc::allocate(&mut f);
        assert_eq!(alloc.homes.len(), f.types.len(), "every vreg must have a home");
        assert!(f.spill_bytes > 0, "expected a spill slot for the untyped helper result");
    }
}
