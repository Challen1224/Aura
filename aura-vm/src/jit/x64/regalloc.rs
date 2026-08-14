//! Linear-scan register allocation over the IR.
//!
//! Each virtual register gets a fixed *home*: a general purpose register
//! (`Int`/`Bool`/`Char`), an SSE register (`Float`), or a 16-byte spill slot.
//! Unknown-typed virtuals always live in slots (they hold full value boxes).
//!
//! Register selection:
//! * Values live across a [`Insn::Helper`] call are placed in callee-saved
//!   registers (the helper preserves them), so no save/restore is needed.
//! * Values that never cross a call may use caller-saved registers as well.
//! * Overflow (register pressure) spills to dedicated slots.
//!
//! Allocation is a greedy first-fit over live intervals sorted with
//! cross-call intervals first, so scarce callee-saved registers go to the
//! values that actually need them.

use super::asm::regs::{R8, R9, R10, R11, RBX, RDI, RSI, R14, R15};
use super::asm::xmms::{
    XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7, XMM8, XMM9, XMM10, XMM11, XMM12, XMM13, XMM14,
    XMM15,
};
use super::asm::{Gpr, Xmm};
use crate::jit::ir::{Function, Insn, Term, VReg};
use std::collections::BTreeSet;

/// Size of every frame slot in bytes (locals and spills).
pub const SLOT_BYTES: usize = 16;

/// Callee-saved GPRs; safe to keep values across helper calls.
const CALLEE_GPR: [Gpr; 3] = [RBX, R14, R15];
/// Caller-saved GPRs; only for values that never cross a helper call.
const CALLER_GPR: [Gpr; 6] = [RSI, RDI, R8, R9, R10, R11];
/// Callee-saved XMM registers (System V).
const CALLEE_XMM: [Xmm; 8] = [XMM8, XMM9, XMM10, XMM11, XMM12, XMM13, XMM14, XMM15];
/// Caller-saved XMM registers (System V). XMM0 is reserved as scratch.
const CALLER_XMM: [Xmm; 7] = [XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7];

/// Where a virtual register's value lives after allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// 64-bit payload in a general purpose register (`Int`/`Bool`/`Char`).
    Gpr(Gpr),
    /// f64 payload in an SSE register (`Float`).
    Xmm(Xmm),
    /// 16-byte spill slot; byte offset is `slot * SLOT_BYTES`.
    Slot(usize),
}

/// Result of register allocation.
#[derive(Debug, Clone)]
pub struct Allocation {
    /// Home of every virtual register.
    pub homes: Vec<Home>,
    /// Number of 16-byte spill slots required.
    pub slot_count: usize,
}

/// The terminator's value use, if any.
fn term_use(block: &crate::jit::ir::Block) -> Option<VReg> {
    match &block.term {
        Term::Ret(v) | Term::BrTrue(v, _) | Term::BrFalse(v, _) => Some(*v),
        _ => None,
    }
}

/// Allocate homes for every virtual register in `func`.
pub fn allocate(func: &mut Function) -> Allocation {
    let nb = func.blocks.len();
    let nv = func.types.len();

    // Per-block use/def sets and live-in/live-out fixpoint.
    let mut uses: Vec<BTreeSet<VReg>> = vec![BTreeSet::new(); nb];
    let mut defs: Vec<BTreeSet<VReg>> = vec![BTreeSet::new(); nb];
    for (b, block) in func.blocks.iter().enumerate() {
        if !block.reachable {
            continue;
        }
        for p in &block.params {
            defs[b].insert(*p);
        }
        for insn in &block.insns {
            if let Some(d) = insn.defined() {
                defs[b].insert(d);
            }
            for u in insn.uses() {
                uses[b].insert(u);
            }
        }
        if let Some(v) = term_use(block) {
            uses[b].insert(v);
        }
        for moves in block.edge_moves.values() {
            for (_, src) in moves {
                uses[b].insert(*src);
            }
        }
    }

    let mut live_in: Vec<BTreeSet<VReg>> = vec![BTreeSet::new(); nb];
    let mut live_out: Vec<BTreeSet<VReg>> = vec![BTreeSet::new(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..nb {
            if !func.blocks[b].reachable {
                continue;
            }
            let mut out = BTreeSet::new();
            for &s in &func.blocks[b].succs {
                if func.blocks[s].reachable {
                    out.extend(live_in[s].iter());
                }
            }
            if out != live_out[b] {
                live_out[b] = out;
                changed = true;
            }
            let mut inp: BTreeSet<VReg> = uses[b].clone();
            for v in &live_out[b] {
                if !defs[b].contains(v) {
                    inp.insert(*v);
                }
            }
            if inp != live_in[b] {
                live_in[b] = inp;
                changed = true;
            }
        }
    }

    // Flat positions: [entry .. entry+1+insns .. exit] per reachable block.
    let mut entry_pos = vec![0usize; nb];
    let mut exit_pos = vec![0usize; nb];
    let mut cur = 0usize;
    for b in 0..nb {
        if !func.blocks[b].reachable {
            continue;
        }
        entry_pos[b] = cur;
        cur += 1;
        cur += func.blocks[b].insns.len();
        exit_pos[b] = cur;
        cur += 1;
    }

    // Live intervals (half-open [start, end)).
    let mut start = vec![usize::MAX; nv];
    let mut end = vec![0usize; nv];
    for b in 0..nb {
        let block = &func.blocks[b];
        if !block.reachable {
            continue;
        }
        for p in &block.params {
            let mut s = usize::MAX;
            for &pr in &block.preds {
                if func.blocks[pr].reachable {
                    s = s.min(exit_pos[pr]);
                }
            }
            if s == usize::MAX {
                s = entry_pos[b];
            }
            start[*p as usize] = start[*p as usize].min(s);
        }
        for (i, insn) in block.insns.iter().enumerate() {
            let pos = entry_pos[b] + 1 + i;
            if let Some(d) = insn.defined() {
                start[d as usize] = start[d as usize].min(pos);
            }
            for u in insn.uses() {
                end[u as usize] = end[u as usize].max(pos + 1);
            }
        }
        if let Some(v) = term_use(block) {
            end[v as usize] = end[v as usize].max(exit_pos[b] + 1);
        }
        for moves in block.edge_moves.values() {
            for (_, src) in moves {
                end[*src as usize] = end[*src as usize].max(exit_pos[b] + 1);
            }
        }
    }

    // Whether each virtual is live at a helper call (needs callee-saved or a slot).
    let mut crosses_call = vec![false; nv];
    for b in 0..nb {
        let block = &func.blocks[b];
        if !block.reachable {
            continue;
        }
        let mut live = live_out[b].clone();
        if let Some(v) = term_use(block) {
            live.insert(v);
        }
        for i in (0..block.insns.len()).rev() {
            let insn = &block.insns[i];
            if let Insn::Helper { .. } = insn {
                for v in &live {
                    crosses_call[*v as usize] = true;
                }
            }
            if let Some(d) = insn.defined() {
                live.remove(&d);
            }
            for u in insn.uses() {
                live.insert(u);
            }
        }
    }

    // First-fit assignment. `taken` maps a register to the virtuals already
    // using it, so we can detect interval overlap.
    let mut taken_gpr: Vec<Vec<VReg>> = vec![Vec::new(); 16];
    let mut taken_xmm: Vec<Vec<VReg>> = vec![Vec::new(); 16];

    let mut homes: Vec<Option<Home>> = vec![None; nv];
    let mut slot_for = vec![None; nv];
    let mut next_slot = 0usize;
    let mut take_slot = |v: VReg,
                         slot_for: &mut Vec<Option<usize>>,
                         next_slot: &mut usize|
     -> usize {
        if let Some(s) = slot_for[v as usize] {
            return s;
        }
        let s = *next_slot;
        *next_slot += 1;
        slot_for[v as usize] = Some(s);
        s
    };

    let overlaps = |a: usize, b: usize, c: usize, d: usize| a < d && c < b;

    // Cross-call (callee-saved) known virtuals first, then the rest. Every
    // known-typed vreg is included, even ones with no live interval (e.g.
    // phantoms left behind by constant folding): they are dead and get a slot.
    let mut known: Vec<VReg> = (0..nv)
        .map(|v| v as VReg)
        .filter(|v| func.types[*v as usize].is_known())
        .collect();
    known.sort_by_key(|v| (std::cmp::Reverse(crosses_call[*v as usize]), start[*v as usize]));

    for v in known {
        let vs = start[v as usize];
        let ve = end[v as usize];
        let is_float = matches!(func.types[v as usize], crate::jit::ir::KType::Float);
        let callee_only = crosses_call[v as usize];
        // Dead value (defined, never used): don't waste a register.
        if ve == 0 {
            let s = take_slot(v, &mut slot_for, &mut next_slot);
            homes[v as usize] = Some(Home::Slot(s));
            continue;
        }
        let mut chosen: Option<Home> = None;
        if is_float {
            for r in CALLEE_XMM.iter().chain(CALLER_XMM.iter()) {
                if callee_only && !CALLEE_XMM.contains(r) {
                    continue;
                }
                let mut used = taken_xmm[r.0 as usize].iter().copied();
                let free = !used.any(|u| overlaps(vs, ve, start[u as usize], end[u as usize]));
                if free {
                    taken_xmm[r.0 as usize].push(v);
                    chosen = Some(Home::Xmm(*r));
                    break;
                }
            }
        } else {
            for r in CALLEE_GPR.iter().chain(CALLER_GPR.iter()) {
                if callee_only && !CALLEE_GPR.contains(r) {
                    continue;
                }
                let mut used = taken_gpr[r.0 as usize].iter().copied();
                let free = !used.any(|u| overlaps(vs, ve, start[u as usize], end[u as usize]));
                if free {
                    taken_gpr[r.0 as usize].push(v);
                    chosen = Some(Home::Gpr(*r));
                    break;
                }
            }
        }
        match chosen {
            Some(h) => homes[v as usize] = Some(h),
            None => {
                let s = take_slot(v, &mut slot_for, &mut next_slot);
                homes[v as usize] = Some(Home::Slot(s));
            }
        }
    }

    // Unknown-typed virtuals always live in slots.
    let mut unknown: Vec<VReg> = (0..nv).map(|v| v as VReg).filter(|v| !func.types[*v as usize].is_known()).collect();
    unknown.sort_by_key(|v| *v);
    for v in unknown {
        let s = take_slot(v, &mut slot_for, &mut next_slot);
        homes[v as usize] = Some(Home::Slot(s));
    }

    let homes: Vec<Home> = homes
        .into_iter()
        .map(|h| h.expect("every vreg is assigned a home"))
        .collect();
    debug_assert_eq!(homes.len(), nv);
    func.spill_bytes = next_slot * SLOT_BYTES;
    Allocation { homes, slot_count: next_slot }
}
