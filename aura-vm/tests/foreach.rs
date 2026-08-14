//! for-in iteration over List/Set: equivalence with the manual Count/Get
//! pattern, the documented mutation-during-iteration semantics, empty
//! collections, break/continue, and interpreter/JIT parity in hot loops.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::{Vm, VmError};
use std::sync::Arc;

fn run(source: &str, jit: bool) -> Result<i64, VmError> {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(1);
    }
    match vm.run()? {
        Value::Int(i) => Ok(i),
        other => panic!("expected int, got {other:?}"),
    }
}

fn assert_parity(source: &str, expected: i64) {
    let interp = run(source, false).expect("interpreter should run");
    assert_eq!(interp, expected, "interpreter result");
    let jit = run(source, true).expect("jit should run");
    assert_eq!(jit, interp, "jit must match interpreter");
}

/// The core bar: foreach must compute exactly what the manual loop does.
#[test]
fn foreach_matches_manual_loop() {
    let src = r#"
class Program {
    static int Main() {
        List<int> xs = new List<int>();
        int i = 0;
        while (i < 50) { xs.Add(i * 3); i = i + 1; }

        int manual = 0;
        int j = 0;
        while (j < xs.Count) { manual = manual + xs.Get(j); j = j + 1; }

        int foreach_total = 0;
        for (int x in xs) { foreach_total = foreach_total + x; }

        Set<int> s = new Set<int>();
        s.Add(7); s.Add(11); s.Add(13);
        int set_total = 0;
        for (int e in s) { set_total = set_total + e; }

        if (manual == foreach_total) {
            return foreach_total * 1000 + set_total;
        }
        return -1;
    }
}
"#;
    // sum(i*3, 0..49) = 3675; set 7+11+13 = 31.
    assert_parity(src, 3675031);
}

/// Documented semantics: the List count is snapshotted at loop entry, so
/// elements appended during iteration are not visited.
#[test]
fn list_append_during_foreach_not_visited() {
    let src = r#"
class Program {
    static int Main() {
        List<int> xs = new List<int>();
        xs.Add(1); xs.Add(2); xs.Add(3);
        int visited = 0;
        for (int x in xs) {
            visited = visited + 1;
            xs.Add(99);
        }
        return visited * 100 + xs.Count;
    }
}
"#;
    // 3 visits (snapshot), list grew to 6.
    assert_parity(src, 306);
}

/// Documented semantics: removing List elements during iteration can make a
/// later Get read past the shrunken end — a runtime error, not silence.
#[test]
fn list_remove_during_foreach_errors() {
    let src = r#"
class Program {
    static int Main() {
        List<int> xs = new List<int>();
        xs.Add(1); xs.Add(2); xs.Add(3);
        for (int x in xs) {
            int dropped = xs.RemoveAt(0);
        }
        return 0;
    }
}
"#;
    for jit in [false, true] {
        let err = run(src, jit).expect_err("removal during foreach must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("out of range"),
            "expected out-of-range error, got: {msg} (jit={jit})"
        );
    }
}

/// Documented semantics: Set iteration walks a ToList() snapshot, so
/// mutating the Set during iteration never affects the iteration.
#[test]
fn set_mutation_during_foreach_is_isolated() {
    let src = r#"
class Program {
    static int Main() {
        Set<int> s = new Set<int>();
        s.Add(1); s.Add(2); s.Add(3);
        int visited = 0;
        for (int e in s) {
            visited = visited + 1;
            s.Add(e + 100);
            s.Remove(2);
        }
        return visited * 100 + s.Count;
    }
}
"#;
    // Snapshot of {1,2,3}: 3 visits regardless of mutations. Final set:
    // {1,3} + {101,102,103} = 5 elements.
    assert_parity(src, 305);
}

#[test]
fn foreach_empty_and_control_flow() {
    let src = r#"
class Program {
    static int Main() {
        List<int> empty = new List<int>();
        int touched = 0;
        for (int e in empty) { touched = touched + 1; }

        List<int> xs = new List<int>();
        int i = 0;
        while (i < 10) { xs.Add(i); i = i + 1; }
        int total = 0;
        for (int x in xs) {
            if (x == 3) { continue; }
            if (x == 7) { break; }
            total = total + x;
        }
        // 0+1+2+4+5+6 = 18
        return touched * 1000 + total;
    }
}
"#;
    assert_parity(src, 18);
}

/// foreach inside a method that crosses the JIT threshold mid-run; results
/// must match the interpreter across repeated trials.
#[test]
fn foreach_in_jit_hot_loop_trials() {
    let src = r#"
class Program {
    static int SumAll(List<int> xs) {
        int total = 0;
        for (int x in xs) { total = total + x; }
        return total;
    }
    static int Main() {
        List<int> xs = new List<int>();
        int i = 0;
        while (i < 40) { xs.Add(i); i = i + 1; }
        int grand = 0;
        int round = 0;
        while (round < 60) {
            grand = grand + SumAll(xs);
            round = round + 1;
        }
        return grand;
    }
}
"#;
    // 60 * sum(0..39) = 60 * 780 = 46800. Threshold 1 compiles Main and
    // SumAll immediately; also run at a mid-run tier-transition threshold.
    for trial in 0..3 {
        assert_parity(src, 46800);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(20);
        match vm.run().expect("jit threshold-20 run") {
            Value::Int(i) => assert_eq!(i, 46800, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
