//! Async/await: cooperative coroutines on interpreter frames. An `async
//! Task<T>` method's calls spawn hot tasks (queued, lazy execution — they
//! progress only while the scheduler runs at an `await` or `wait()`);
//! `await` suspends the current task's frame until the awaited task
//! completes; `t.wait()` drives the scheduler from sync code. Exceptions
//! inside tasks surface (catchably) at await/wait sites. Task ops are
//! interpreter-only — methods touching them fall back from the JIT, while
//! everything else still tiers up.

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

fn assert_compile_fails(source: &str, needle: &str) {
    match compile(source, "test") {
        Ok(_) => panic!("expected compile error containing `{needle}`"),
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains(needle),
                "expected error containing `{needle}`, got: {msg}"
            );
        }
    }
}

/// Deterministic cooperative interleaving: two tasks record their step
/// order into a shared static; `Tasks.pause()` yields between steps. The
/// exact interleaving (1a 2a 1b 2b) is asserted through positional
/// weighting, under both tiers.
#[test]
fn interleaving_is_deterministic() {
    let src = r#"
class Program {
    static int order = 0;
    static void mark(int step) { Program.order = Program.order * 10 + step; }
    static async Task<int> worker(int id) {
        Program.mark(id);
        await Tasks.pause();
        Program.mark(id + 4);
        return id * 100;
    }
    static async Task<int> gather() {
        var a = Program.worker(1);
        var b = Program.worker(2);
        return await a + await b;
    }
    static int Main() {
        int total = Program.gather().wait();
        return total * 10000 + Program.order;
    }
}
"#;
    // Hot tasks + FIFO: worker1 runs to its pause, worker2 likewise, then
    // both resume in order: marks 1,2,5,6 -> order 1256; results 100+200.
    assert_parity(src, 3001256);
}

/// Awaited results flow, awaiting an already-completed task is immediate,
/// two awaiters of one task both wake, and `wait()` on a finished task
/// re-reads its result.
#[test]
fn task_results_flow() {
    let src = r#"
class Program {
    static async Task<int> source() {
        await Tasks.pause();
        return 21;
    }
    static async Task<int> twice(Task<int> t) {
        return await t * 2;
    }
    static int Main() {
        var s = Program.source();
        var a = Program.twice(s);
        var b = Program.twice(s);
        int total = a.wait() + b.wait();
        total = total + s.wait();
        return total;
    }
}
"#;
    // 42 + 42 + 21 = 105.
    assert_parity(src, 105);
}

/// Exceptions inside tasks surface catchably at both await and wait
/// sites, and cause/chaining machinery works across the boundary.
#[test]
fn task_exceptions_surface() {
    let src = r#"
class WorkError : Exception { WorkError(string m) { this.message = m; } }
class Program {
    static async Task<int> risky(int n) {
        if (n == 2) { throw new WorkError("boom"); }
        return n;
    }
    static async Task<int> gather() {
        int total = await Program.risky(1);
        try {
            total = total + await Program.risky(2);
        } catch (WorkError e) {
            total = total + e.message.Length * 10;
        }
        return total;
    }
    static int Main() {
        int total = Program.gather().wait();
        var bad = Program.risky(2);
        try {
            total = total + bad.wait();
        } catch (WorkError e) {
            total = total + 1000;
        }
        // A failed task keeps failing on every wait.
        try {
            total = total + bad.wait();
        } catch (WorkError e) {
            total = total + 100000;
        }
        return total;
    }
}
"#;
    // 1 + 40 + 1000 + 100000 = 101041.
    assert_parity(src, 101041);
}

/// An await cycle is reported as a deadlock, not a hang.
#[test]
fn await_cycle_is_deadlock() {
    let src = r#"
class Program {
    static Task<int>? other;
    static async Task<int> a() {
        var o = Program.other;
        if (o != null) { return await o; }
        return 0;
    }
    static void Main() {
        var t1 = Program.a();
        Program.other = t1;
        var t2 = Program.a();
        t2.wait();
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let err = vm.run().expect_err("should deadlock");
    assert!(
        format!("{err}").contains("deadlock"),
        "got: {err}"
    );
}

/// Every declaration and use-site rule, pinned.
#[test]
fn async_rules_are_enforced() {
    let prog = "class Program { static void Main() { } }";
    let cases: Vec<(String, &str)> = vec![
        (
            "class Program { static void Main() { var t = Tasks.pause(); var x = await t; } }"
                .to_string(),
            "`await` is only allowed inside `async` methods",
        ),
        (
            format!("class C {{ static async int bad() {{ return 1; }} }} {prog}"),
            "must return Task<T>",
        ),
        (
            format!("class C {{ async Task<int> m() {{ return 1; }} }} {prog}"),
            "must be static",
        ),
        (
            format!(
                "class E2 : Exception {{ }} \
                 class C {{ static async Task<int> f() throws E2 {{ return 1; }} }} {prog}"
            ),
            "cannot declare `throws`",
        ),
        (
            format!(
                "class C {{ static async Task<int> f() {{ int x = await 5; return x; }} }} {prog}"
            ),
            "`await` needs a Task<T>",
        ),
        // Async bodies return the element type, checked as such.
        (
            format!(
                "class C {{ static async Task<int> f() {{ return \"s\"; }} }} {prog}"
            ),
            "return type mismatch",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(&src, needle);
    }
}

/// Suspended task frames are GC roots: objects held only by a parked
/// frame survive collection churn.
#[test]
fn suspended_frames_are_gc_roots() {
    let src = r#"
class Box { string tag; Box(string tag) { this.tag = tag; } }
class Program {
    static async Task<int> holder() {
        Box b = new Box("keepme");
        await Tasks.pause();
        await Tasks.pause();
        return b.tag.Length;
    }
    static int Main() {
        var h = Program.holder();
        int churn = 0;
        for (var i in 1..=400) {
            string junk = "junk {i}";
            churn = churn + junk.Length - junk.Length;
        }
        return h.wait() + churn;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(2048);
    match vm.run().expect("run") {
        Value::Int(i) => assert_eq!(i, 6),
        other => panic!("expected int, got {other:?}"),
    }
    assert!(vm.gc_collections() > 0, "expected GC cycles during churn");
}

/// Task ops are interpreter-only, but the rest of the program still tiers
/// up: a hot pure-computation method compiles while async orchestration
/// runs around it. 3 trials, compiled count asserted.
#[test]
fn async_alongside_jit_trials() {
    let src = r#"
class Program {
    static int square(int i) { return i * i; }
    static async Task<int> sumRange(int from, int to) {
        int total = 0;
        int i = from;
        while (i <= to) {
            total = total + Program.square(i);
            if (i % 50 == 0) { await Tasks.pause(); }
            i = i + 1;
        }
        return total;
    }
    static int Main() {
        var a = Program.sumRange(1, 100);
        var b = Program.sumRange(101, 200);
        return a.wait() + b.wait();
    }
}
"#;
    // sum(i^2, 1..200) = 2686700.
    for trial in 0..3 {
        assert_parity(src, 2686700);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(40);
        match vm.run().expect("jit threshold-40 run") {
            Value::Int(i) => assert_eq!(i, 2686700, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(
            vm.jit_compiled_count() > 0,
            "trial {trial}: square should have compiled"
        );
    }
}
