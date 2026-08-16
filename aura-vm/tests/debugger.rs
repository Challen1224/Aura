//! Source-level debugging: custom debug info (per-method line tables and
//! local-name tables emitted alongside bytecode — DWARF targets native
//! code and is deliberately not used), line-number mapping for stops and
//! error stack traces, and variable inspection through the `Debugger`
//! controller API that drives the interpreter tier (installing a
//! debugger suppresses JIT tier-up: compiled frames cannot stop).

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::{DebugCommand, DebugResume, DebugStop, Debugger, Vm};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

/// Line layout is load-bearing: tests below assert on these numbers.
const SRC: &str = r#"
class Point {
    int x;
    int y;
    Point(int x, int y) { this.x = x; this.y = y; }
    int magnitude() {
        int m = this.x * this.x + this.y * this.y;
        return m;
    }
}
class Program {
    static int Main() {
        int a = 41;
        Point p = new Point(3, 4);
        int mag = p.magnitude();
        a = a + 1;
        return a + mag;
    }
}
"#;

/// A scripted controller: replays a fixed command sequence and records
/// every stop for the test to assert on.
struct Script {
    commands: VecDeque<DebugCommand>,
    log: Rc<RefCell<Vec<DebugStop>>>,
}

impl Debugger for Script {
    fn on_stop(&mut self, stop: &DebugStop) -> DebugCommand {
        self.log.borrow_mut().push(stop.clone());
        self.commands.pop_front().unwrap_or(DebugCommand {
            resume: Some(DebugResume::Continue),
            ..Default::default()
        })
    }
}

fn resume(r: DebugResume) -> DebugCommand {
    DebugCommand {
        resume: Some(r),
        ..Default::default()
    }
}

fn run_scripted(src: &str, commands: Vec<DebugCommand>) -> (i64, Vec<DebugStop>) {
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let log = Rc::new(RefCell::new(Vec::new()));
    vm.set_debugger(Box::new(Script {
        commands: commands.into(),
        log: Rc::clone(&log),
    }));
    let result = match vm.run().expect("run") {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    let stops = log.borrow().clone();
    (result, stops)
}

/// The emitter records debug info: a sorted, non-empty line table and
/// the declared local names (parameters first, `this` for instance
/// methods, compiler temps `__`-prefixed).
#[test]
fn line_tables_and_local_names_are_emitted() {
    let module = compile(SRC, "test").expect("compile");
    let mut checked = 0;
    for class in module.classes.values() {
        for (name, m) in class
            .methods
            .iter()
            .chain(class.static_methods.iter())
            .map(|(_, m)| (m.name.as_str(), m))
        {
            if m.body.is_empty() {
                continue;
            }
            // Synthesized/injected methods (the runtime Exception class)
            // legitimately carry no line info; user methods must.
            if ["Main", "magnitude", "Point"].contains(&name) {
                assert!(!m.line_starts.is_empty(), "{name}: line table missing");
                checked += 1;
            }
            assert!(
                m.line_starts.windows(2).all(|w| w[0].0 < w[1].0),
                "{name}: line table op indexes not strictly sorted"
            );
            if name == "Main" {
                for var in ["a", "p", "mag"] {
                    assert!(
                        m.local_names.iter().any(|n| n == var),
                        "Main: local `{var}` missing from {:?}",
                        m.local_names
                    );
                }
            }
            if name == "magnitude" {
                assert_eq!(m.local_names.first().map(String::as_str), Some("this"));
                assert!(m.local_names.iter().any(|n| n == "m"));
            }
        }
    }
    assert!(checked >= 3, "expected several bodied methods");
}

/// A breakpoint stops at its line with the frame's named locals visible
/// and correct; execution then completes normally.
#[test]
fn breakpoint_stops_with_locals() {
    let bp = DebugCommand {
        add_breakpoints: vec![16],
        resume: Some(DebugResume::Continue),
        ..Default::default()
    };
    let (result, stops) = run_scripted(SRC, vec![bp]);
    assert_eq!(result, 67);
    // Stop 1: debugger starts in step mode -> first line of Main.
    assert_eq!(stops[0].line, 13);
    assert_eq!(stops[0].method, "Program.Main");
    // Stop 2: the breakpoint, after `mag` is assigned.
    assert_eq!(stops[1].line, 16);
    let locals = &stops[1].locals;
    let get = |n: &str| {
        locals
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("local `{n}` missing from {locals:?}"))
    };
    assert_eq!(get("a"), "41");
    assert_eq!(get("mag"), "25");
    assert!(!locals.iter().any(|(n, _)| n.starts_with("__")), "temps leaked");
    assert_eq!(stops.len(), 2, "no further stops after continue");
}

/// Step semantics: `Step` enters a callee (line in the callee, deeper
/// frame, `this` visible); `Next` steps over back to the caller's next
/// line without stopping inside further calls.
#[test]
fn step_into_and_step_over() {
    let commands = vec![
        resume(DebugResume::Next), // 13 -> 14 (skips Point ctor)
        resume(DebugResume::Next), // 14 -> 15
        resume(DebugResume::Step), // 15 -> into magnitude (line 7)
        resume(DebugResume::Next), // 7 -> 8
        resume(DebugResume::Next), // 8 -> return -> back at 16
        resume(DebugResume::Continue),
    ];
    let (result, stops) = run_scripted(SRC, commands);
    assert_eq!(result, 67);
    let path: Vec<(String, u32, usize)> = stops
        .iter()
        .map(|s| (s.method.clone(), s.line, s.depth))
        .collect();
    assert_eq!(path[0], ("Program.Main".into(), 13, 1));
    assert_eq!(path[1], ("Program.Main".into(), 14, 1), "next stays in frame");
    assert_eq!(path[2], ("Program.Main".into(), 15, 1), "next skips ctor");
    assert_eq!(path[3], ("Point.magnitude".into(), 7, 2), "step enters callee");
    assert_eq!(path[4], ("Point.magnitude".into(), 8, 2));
    assert_eq!(path[5], ("Program.Main".into(), 16, 1), "returns to caller");
    // Inside the callee, `this` and the backtrace show the full chain.
    assert!(stops[3].locals.iter().any(|(n, _)| n == "this"));
    assert_eq!(stops[3].backtrace.len(), 2);
    assert!(stops[3].backtrace[0].contains("Program.Main (line 15)"));
    assert!(stops[3].backtrace[1].contains("Point.magnitude (line 7)"));
}

/// `Quit` terminates the run with the sentinel error.
#[test]
fn quit_terminates() {
    let module = Arc::new(compile(SRC, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let log = Rc::new(RefCell::new(Vec::new()));
    vm.set_debugger(Box::new(Script {
        commands: vec![resume(DebugResume::Quit)].into(),
        log,
    }));
    let err = vm.run().expect_err("quit should abort");
    assert!(format!("{err}").contains("debugger: quit"), "got: {err}");
}

/// Runtime errors leave the call stack intact, and the line-enriched
/// trace names every frame with the exact faulting line.
#[test]
fn error_stack_trace_has_lines() {
    let src = r#"
class Program {
    static int inner(List<int> xs) {
        return xs.Get(5);
    }
    static int outer() {
        var xs = new List<int>();
        xs.Add(1);
        return Program.inner(xs);
    }
    static int Main() {
        return Program.outer();
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let err = vm.run().expect_err("out of range");
    assert!(format!("{err}").contains("out of range"));
    let trace = vm.stack_trace();
    assert_eq!(
        trace,
        vec![
            "at Program.Main (line 12)",
            "at Program.outer (line 9)",
            "at Program.inner (line 4)",
        ]
    );
}

/// Installing a debugger suppresses JIT tier-up: the whole run stays on
/// the interpreter (where stops are possible), even with the JIT enabled
/// and a hot method.
#[test]
fn debugger_suppresses_jit() {
    let src = r#"
class Program {
    static int pad(int i) { return i - i; }
    static int Main() {
        int total = 0;
        for (var i in 1..=500) { total = total + Program.pad(i); }
        return total + 7;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.enable_jit();
    vm.set_jit_threshold(1);
    let log = Rc::new(RefCell::new(Vec::new()));
    vm.set_debugger(Box::new(Script {
        commands: Vec::new().into(),
        log,
    }));
    match vm.run().expect("run") {
        Value::Int(i) => assert_eq!(i, 7),
        other => panic!("expected int, got {other:?}"),
    }
    assert_eq!(
        vm.jit_compiled_count(),
        0,
        "debugger must keep execution on the interpreter tier"
    );
}
