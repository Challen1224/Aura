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

use crate::Vm;
use aura_bytecode::{AuraObject, MethodDef, Value};
use std::sync::atomic::{AtomicPtr, Ordering};

/// The VM currently executing on any thread, for out-of-process debuggers
/// (the GDB extension in tools/gdb/aura_gdb.py): set on `Vm::run` entry,
/// cleared on exit, still pointing at the VM in a crash or a hang. Read
/// only by external tools — never dereferenced from Rust. `no_mangle`
/// keeps it findable as a plain ELF symbol (DWARF drops never-read
/// statics).
#[no_mangle]
pub static AURA_CURRENT_VM: AtomicPtr<Vm> = AtomicPtr::new(std::ptr::null_mut());

/// RAII registration of the running VM in [`CURRENT_VM`].
pub(crate) struct CurrentVmGuard;

impl CurrentVmGuard {
    pub(crate) fn register(vm: &mut Vm) -> Self {
        AURA_CURRENT_VM.store(vm as *mut Vm, Ordering::SeqCst);
        Self
    }
}

impl Drop for CurrentVmGuard {
    fn drop(&mut self) {
        AURA_CURRENT_VM.store(std::ptr::null_mut(), Ordering::SeqCst);
    }
}

/// One method's debug info in a flat, DWARF-friendly layout so external
/// debuggers never have to walk a Rust `HashMap` (whose hashbrown
/// internals are unstable). Built once per VM; read via GDB.
#[derive(Debug)]
pub struct GdbMethodInfo {
    /// Raw method id (matches `Frame.method_id`).
    pub method_id: u32,
    /// `Class.Method` label.
    pub label: String,
    /// Sorted `(first_op_index, line)` pairs (copy of the module table).
    pub line_starts: Vec<(u32, u32)>,
    /// Slot-indexed local names (copy of the module table).
    pub local_names: Vec<String>,
}

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
    /// Bytecode index of the op about to execute (for bytecode-level
    /// breakpoints and disassembly).
    pub pc: usize,
    /// Watch expressions evaluated at this stop: `(path, rendered value
    /// or error text)`.
    pub watches: Vec<(String, String)>,
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
    /// Run until the current frame returns; stop in the caller
    /// (step out).
    Out,
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
    /// Bytecode breakpoints to add: (`Class.Method` label, op index).
    /// Labels that resolve to no method are ignored (check first with
    /// [`DebugView::resolve_method_label`]).
    pub add_bytecode_breakpoints: Vec<(String, u32)>,
    /// Bytecode breakpoints to remove.
    pub remove_bytecode_breakpoints: Vec<(String, u32)>,
    /// Watch expressions (local-rooted paths like `p.x` or `xs[0]`) to
    /// evaluate and report at every subsequent stop.
    pub add_watches: Vec<String>,
    /// Watch expressions to stop reporting.
    pub remove_watches: Vec<String>,
    /// Resume mode (defaults to `Continue`).
    pub resume: Option<DebugResume>,
}

/// A debug controller installed via `Vm::set_debugger`. Called at every
/// stop; execution is paused for the duration of the callback. The
/// [`DebugView`] gives live (read-only) access to the paused VM for
/// evaluation beyond the snapshot.
pub trait Debugger {
    /// Handle a stop and choose how to resume.
    fn on_stop(&mut self, view: &DebugView<'_>, stop: &DebugStop) -> DebugCommand;
}

/// Read-only window into the paused VM, valid for the duration of one
/// `on_stop` callback.
pub struct DebugView<'a> {
    pub(crate) vm: &'a Vm,
}

impl DebugView<'_> {
    /// Number of frames on the call stack (innermost last).
    pub fn frame_count(&self) -> usize {
        self.vm.call_stack_len()
    }

    /// Named locals of the frame at `index` (0 = outermost), rendered.
    pub fn frame_locals(&self, index: usize) -> Vec<(String, String)> {
        self.vm.debug_frame_locals(index)
    }

    /// Structured frames: (`Class.Method`, line) from outermost to
    /// innermost; line 0 when unmapped.
    pub fn frames(&self) -> Vec<(String, u32)> {
        self.vm.debug_frames()
    }

    /// Whether a `Class.Method` label names a real method (for
    /// validating bytecode breakpoints before submitting them).
    pub fn resolve_method_label(&self, label: &str) -> bool {
        self.vm.resolve_method_label(label).is_some()
    }

    /// Evaluate a local-rooted path (`name`, `p.x`, `xs[0]`, `a.b[2].c`)
    /// against the frame at `index` (0 = outermost). Statics and
    /// arbitrary expressions are not supported — this is inspection, not
    /// an evaluator.
    pub fn eval_path(&self, index: usize, path: &str) -> Result<String, String> {
        self.vm.debug_eval_path(index, path)
    }

    /// Disassembly window around the stopped frame's pc:
    /// `(op_index, rendered op, is_current)`.
    pub fn disassemble(&self, window: usize) -> Vec<(usize, String, bool)> {
        self.vm.debug_disassemble(window)
    }
}

/// Parse a watch/inspection path into its root name and steps.
pub(crate) fn parse_path(path: &str) -> Result<(String, Vec<PathStep>), String> {
    let bytes = path.as_bytes();
    let mut i = 0;
    let ident = |i: &mut usize| -> Result<String, String> {
        let start = *i;
        while *i < bytes.len() && (bytes[*i].is_ascii_alphanumeric() || bytes[*i] == b'_') {
            *i += 1;
        }
        if *i == start {
            return Err(format!("`{path}`: expected a name at offset {start}"));
        }
        Ok(path[start..*i].to_string())
    };
    let root = ident(&mut i)?;
    let mut steps = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                i += 1;
                steps.push(PathStep::Field(ident(&mut i)?));
            }
            b'[' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start || i >= bytes.len() || bytes[i] != b']' {
                    return Err(format!("`{path}`: malformed index"));
                }
                let idx: usize = path[start..i].parse().map_err(|_| {
                    format!("`{path}`: index out of range")
                })?;
                i += 1;
                steps.push(PathStep::Index(idx));
            }
            _ => return Err(format!("`{path}`: unexpected `{}`", &path[i..i + 1])),
        }
    }
    Ok((root, steps))
}

/// One step of an inspection path.
pub(crate) enum PathStep {
    /// `.name` — instance field access.
    Field(String),
    /// `[n]` — list/tuple element access.
    Index(usize),
}

/// Walk one path step from `value`.
pub(crate) fn walk_step(vm: &Vm, value: &Value, step: &PathStep, path: &str) -> Result<Value, String> {
    let Value::Object(handle) = value else {
        return Err(format!("`{path}`: not an object"));
    };
    let obj = vm
        .heap()
        .get(*handle)
        .map_err(|e| format!("`{path}`: {e}"))?;
    match (step, obj) {
        (PathStep::Field(name), AuraObject::Instance { class_id, fields }) => {
            let class = vm
                .module()
                .classes
                .get(class_id)
                .ok_or_else(|| format!("`{path}`: unknown class"))?;
            let pos = class
                .fields
                .iter()
                .position(|f| &f.name == name)
                .ok_or_else(|| format!("`{path}`: no field `{name}` on {}", class.name))?;
            fields
                .get(pos)
                .cloned()
                .ok_or_else(|| format!("`{path}`: field slot missing"))
        }
        (PathStep::Field(name), _) => Err(format!("`{path}`: no field `{name}` here")),
        (PathStep::Index(i), AuraObject::Array { elements }) => elements
            .get(*i)
            .cloned()
            .ok_or_else(|| format!("`{path}`: index {i} out of range ({} elements)", elements.len())),
        (PathStep::Index(i), AuraObject::Tuple(t)) => t
            .elements
            .get(*i)
            .cloned()
            .ok_or_else(|| format!("`{path}`: index {i} out of range ({} elements)", t.elements.len())),
        (PathStep::Index(_), _) => Err(format!("`{path}`: not indexable")),
    }
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
    /// Stop at the next line boundary strictly shallower than the
    /// recorded depth (after the frame returns).
    Out(usize),
}
