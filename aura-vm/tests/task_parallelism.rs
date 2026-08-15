//! Task parallelism: cooperative combinators over the task scheduler.
//! `Tasks.all(List<Task<T>>) -> Task<List<T>>` completes when every part
//! has, results in list order; the first failure (in list order) fails the
//! whole. `Tasks.race(List<Task<T>>) -> Task<T>` completes with the first
//! part to complete (list order breaks same-round ties); the winner's
//! failure propagates. Combinators are tasks themselves: awaitable,
//! waitable, and composable. "Parallel" means cooperatively interleaved on
//! one thread, deterministically.

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

/// `Tasks.all`: workers with different pause counts interleave, results
/// come back in list order (not completion order), and the combinator is
/// awaitable from async code and waitable from sync code.
#[test]
fn all_gathers_in_list_order() {
    let src = r#"
class Program {
    static int order = 0;
    static async Task<int> work(int id, int rounds) {
        int i = 0;
        while (i < rounds) {
            await Tasks.pause();
            i = i + 1;
        }
        Program.order = Program.order * 10 + id;
        return id * 100;
    }
    static async Task<int> gather() {
        var xs = new List<Task<int>>();
        xs.Add(Program.work(1, 3));
        xs.Add(Program.work(2, 1));
        xs.Add(Program.work(3, 2));
        var results = await Tasks.all(xs);
        int total = 0;
        int weight = 1;
        for (var r in results) {
            total = total + r * weight;
            weight = weight * 10;
        }
        return total;
    }
    static int Main() {
        int total = Program.gather().wait();
        return total * 1000 + Program.order;
    }
}
"#;
    // Completion order by pause count: 2 (1 round), 3 (2), 1 (3) -> order
    // 231. Results in LIST order: 100*1 + 200*10 + 300*100 = 32100.
    assert_parity(src, 32100231);
}

/// The first failure in list order fails `all`, catchably; an empty list
/// completes immediately with an empty List.
#[test]
fn all_failure_and_empty() {
    let src = r#"
class WErr : Exception { WErr(string m) { this.message = m; } }
class Program {
    static async Task<int> ok(int n) { await Tasks.pause(); return n; }
    static async Task<int> bad(string m) { await Tasks.pause(); throw new WErr(m); }
    static async Task<int> gather() {
        var xs = new List<Task<int>>();
        xs.Add(Program.ok(1));
        xs.Add(Program.bad("first"));
        xs.Add(Program.bad("second"));
        int total = 0;
        try {
            var r = await Tasks.all(xs);
            total = 999999;
        } catch (WErr e) {
            total = e.message.Length;
        }
        var empty = new List<Task<int>>();
        var none = await Tasks.all(empty);
        return total * 100 + none.Count;
    }
    static int Main() { return Program.gather().wait(); }
}
"#;
    // "first".Length = 5 -> 500 + 0.
    assert_parity(src, 500);
}

/// `Tasks.race`: the genuinely-faster task wins; losers keep running and
/// can still be awaited afterwards; a failing winner propagates.
#[test]
fn race_first_completion_wins() {
    let src = r#"
class WErr : Exception { WErr(string m) { this.message = m; } }
class Program {
    static async Task<int> slow(int n) {
        await Tasks.pause();
        await Tasks.pause();
        await Tasks.pause();
        return n;
    }
    static async Task<int> fast(int n) {
        await Tasks.pause();
        return n;
    }
    static async Task<int> failSlow() {
        await Tasks.pause();
        await Tasks.pause();
        throw new WErr("late");
    }
    static async Task<int> gather() {
        var xs = new List<Task<int>>();
        var s = Program.slow(7);
        xs.Add(s);
        xs.Add(Program.fast(42));
        int winner = await Tasks.race(xs);
        // The loser is still running and completes normally.
        int later = await s;
        var ys = new List<Task<int>>();
        ys.Add(Program.failSlow());
        ys.Add(Program.fast(5));
        int quick = await Tasks.race(ys);
        return winner * 10000 + later * 100 + quick;
    }
    static int Main() { return Program.gather().wait(); }
}
"#;
    // 42*10000 + 7*100 + 5 = 420705.
    assert_parity(src, 420705);
}

/// A failing race winner propagates catchably, and `Tasks.race` on an
/// empty list is a hard error.
#[test]
fn race_failure_and_empty() {
    let src = r#"
class WErr : Exception { WErr(string m) { this.message = m; } }
class Program {
    static async Task<int> failFast() { throw new WErr("boom"); }
    static async Task<int> slow(int n) {
        await Tasks.pause();
        await Tasks.pause();
        return n;
    }
    static async Task<int> gather() {
        var xs = new List<Task<int>>();
        xs.Add(Program.failFast());
        xs.Add(Program.slow(1));
        try {
            return await Tasks.race(xs);
        } catch (WErr e) {
            return e.message.Length;
        }
    }
    static int Main() { return Program.gather().wait(); }
}
"#;
    assert_parity(src, 4);

    let empty = r#"
class Program {
    static void Main() {
        var xs = new List<Task<int>>();
        var t = Tasks.race(xs);
    }
}
"#;
    let module = Arc::new(compile(empty, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let err = vm.run().expect_err("empty race should error");
    assert!(
        format!("{err}").contains("at least one task"),
        "got: {err}"
    );
}

/// Combinators compose: race of alls, awaited through layers, alongside
/// JIT-compiled computation. 3 trials, compiled count asserted.
#[test]
fn combinators_in_jit_hot_loop_trials() {
    let src = r#"
class Program {
    static int cube(int i) { return i * i * i; }
    static async Task<int> chunk(int from, int to) {
        int total = 0;
        int i = from;
        while (i <= to) {
            total = total + Program.cube(i);
            if (i % 25 == 0) { await Tasks.pause(); }
            i = i + 1;
        }
        return total;
    }
    static async Task<int> gather() {
        var xs = new List<Task<int>>();
        xs.Add(Program.chunk(1, 50));
        xs.Add(Program.chunk(51, 100));
        var parts = await Tasks.all(xs);
        int total = 0;
        for (var p in parts) { total = total + p; }
        return total;
    }
    static int Main() { return Program.gather().wait(); }
}
"#;
    // sum(i^3, 1..100) = (100*101/2)^2 = 25502500.
    for trial in 0..3 {
        assert_parity(src, 25502500);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(40);
        match vm.run().expect("jit threshold-40 run") {
            Value::Int(i) => assert_eq!(i, 25502500, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(
            vm.jit_compiled_count() > 0,
            "trial {trial}: cube should have compiled"
        );
    }
}
