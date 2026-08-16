//! Source-level debugging support.
//!
//! Debug information is a custom format carried in the module (DWARF
//! targets native code generation and is deliberately not used here):
//! each `MethodDef` holds a sorted `(first_op_index, line)` table and a
//! slot-indexed local-name table, both produced by the emitter from the
//! parser's line marks and declaration names.
//!
//! The debugger drives the interpreter tier only: installing one
//! suppresses JIT tier-up for the run (compiled frames cannot stop). The
//! VM stops at source-line boundaries — a breakpoint hit, a single step,
//! or a step-over — and hands the [`Debugger`] a [`DebugStop`] snapshot
//! (location, named locals rendered as strings, backtrace). The
//! returned [`DebugCommand`] applies breakpoint edits and chooses how to
//! resume.

use aura_bytecode::MethodDef;

/// The source line for a pc, from the method's debug line table: the
/// last entry at or before `pc`, or `None` when no debug info exists.
pub fn line_for_pc(method: &MethodDef, pc: usize) -> Option<u32> {
    let table = &method.line_starts;
    if table.is_empty() {
        return None;
    }
    let idx = table.partition_point(|(op, _)| *op as usize <= pc);
    idx.checked_sub(1).map(|i| table[i].1)
}

/// Everything the debugger sees at a stop, snapshotted so the callback
/// borrows nothing from the VM.
#[derive(Debug, Clone)]
pub struct DebugStop {
    /// `Class.Method` label of the stopped frame.
    pub method: String,
    /// Source line the VM is stopped at (about to execute).
    pub line: u32,
    /// Call depth (1 = entry frame).
    pub depth: usize,
    /// Named locals of the stopped frame: `(name, rendered value)`.
    /// Compiler temporaries (`__`-prefixed) are filtered out.
    pub locals: Vec<(String, String)>,
    /// Backtrace lines, innermost frame last.
    pub backtrace: Vec<String>,
}

/// How to resume after a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugResume {
    /// Run until the next breakpoint.
    Continue,
    /// Stop at the next source line, entering calls (step into).
    Step,
    /// Stop at the next source line at the current depth or shallower
    /// (step over).
    Next,
    /// Terminate the program (`VmError` with the message
    /// "debugger: quit").
    Quit,
}

/// The debugger's response to a stop: breakpoint edits plus a resume
/// mode. Breakpoints are source lines.
#[derive(Debug, Clone, Default)]
pub struct DebugCommand {
    /// Lines to add as breakpoints before resuming.
    pub add_breakpoints: Vec<u32>,
    /// Lines to remove before resuming.
    pub remove_breakpoints: Vec<u32>,
    /// Resume mode (defaults to `Continue`).
    pub resume: Option<DebugResume>,
}

/// A debug controller installed via `Vm::set_debugger`. Called at every
/// stop; execution is paused for the duration of the callback.
pub trait Debugger {
    /// Handle a stop and choose how to resume.
    fn on_stop(&mut self, stop: &DebugStop) -> DebugCommand;
}

/// The VM's stepping state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepMode {
    /// Only breakpoints stop execution.
    Running,
    /// Stop at the next line boundary anywhere.
    Step,
    /// Stop at the next line boundary at depth <= the recorded depth.
    Next(usize),
}
