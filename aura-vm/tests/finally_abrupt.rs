//! Regression: `finally` bodies were skipped on abrupt completion. The
//! emitter's finally entry was reachable only via the fall-through after
//! the try body, the jump after each catch, and the exception handler —
//! `return` walked straight out of the frame, and `break`/`continue`
//! jumped over it. The fix inlines the enclosing finally bodies (and
//! `using` Dispose calls) before every abrupt jump, innermost first,
//! draining down to the target loop for `break`/`continue` — a pure
//! bytecode-level fix, so both tiers agree by construction. The return
//! value is computed *before* the finallys run (C# order).

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

/// `return` inside try and inside catch both run the finally; the return
/// value is computed before the finally's side effects (order pinned);
/// nested finallys run innermost first.
#[test]
fn return_runs_finallys() {
    let src = r#"
class E : Exception { E(string m) { this.message = m; } }
class Program {
    static int order = 0;
    static int mark(int v) { Program.order = Program.order * 10 + v; return v; }
    static int fromTry() {
        try {
            return Program.mark(1);
        } finally {
            Program.mark(2);
        }
    }
    static int fromCatch() {
        try {
            throw new E("x");
        } catch (E e) {
            return Program.mark(3);
        } finally {
            Program.mark(4);
        }
    }
    static int nested() {
        try {
            try {
                return Program.mark(5);
            } finally {
                Program.mark(6);
            }
        } finally {
            Program.mark(7);
        }
    }
    static int voidish() {
        try {
            return 0;
        } finally {
            Program.mark(8);
        }
    }
    static int Main() {
        int r = Program.fromTry() + Program.fromCatch() * 10 + Program.nested() * 100;
        int v = Program.voidish();
        return r * 1000000 + v * 100000 + Program.order;
    }
}
"#;
    // mark order: 1,2 (fromTry) 3,4 (fromCatch) 5,6,7 (nested) 8 -> 12345678.
    // r = 1 + 30 + 500 = 531; 531*1_000_000 + 12_345_678 = 543_345_678.
    assert_parity(src, 543345678);
}

/// `break` and `continue` drain finallys down to their loop — including
/// labeled break exiting through two try-finally layers.
#[test]
fn break_continue_run_finallys() {
    let src = r#"
class Program {
    static int order = 0;
    static void mark(int v) { Program.order = Program.order * 10 + v; }
    static int plain() {
        int total = 0;
        for (var i in 1..=3) {
            try {
                if (i == 2) { break; }
                if (i == 1) { total = total + 1; continue; }
                total = total + 100;
            } finally {
                Program.mark(i);
            }
        }
        return total;
    }
    static int labeled() {
        int total = 0;
        outer: for (var i in 1..=2) {
            try {
                for (var j in 1..=2) {
                    try {
                        if (i == 1 && j == 1) { break outer; }
                        total = total + 100;
                    } finally {
                        Program.mark(8);
                    }
                }
            } finally {
                Program.mark(9);
            }
        }
        return total;
    }
    static int Main() {
        int a = Program.plain();
        int b = Program.labeled();
        return (a * 100 + b) * 10000 + Program.order;
    }
}
"#;
    // plain: i=1 continue (finally 1), i=2 break (finally 2) -> total 1,
    // marks 1,2. labeled: break outer runs inner(8) then outer(9) -> total
    // 0, marks 8,9. order = 1289. (1*100+0)*10000 + 1289.
    assert_parity(src, 1001289);
}

/// `return` inside `using` still disposes, and the exception path through
/// finally (the pre-existing behavior) keeps working alongside.
#[test]
fn using_and_exception_paths() {
    let src = r#"
class E : Exception { E(string m) { this.message = m; } }
class Res {
    static int disposed = 0;
    void Dispose() { Res.disposed = Res.disposed + 1; }
}
class Program {
    static int viaUsing() {
        using (Res r = new Res()) {
            return 7;
        }
    }
    static int viaThrow() {
        int marker = 0;
        try {
            try {
                throw new E("boom");
            } finally {
                marker = marker + 1;
            }
        } catch (E e) {
            return marker * 10 + e.message.Length;
        }
        return -1;
    }
    static int Main() {
        int u = Program.viaUsing();
        return u * 10000 + Res.disposed * 1000 + Program.viaThrow();
    }
}
"#;
    // 7*10000 + 1*1000 + (1*10 + 4) = 71014.
    assert_parity(src, 71014);
}

/// Abrupt finallys inside async bodies (across a suspension) and in a JIT
/// hot loop. 3 trials, compiled count asserted.
#[test]
fn finallys_in_async_and_jit_trials() {
    let src = r#"
class Program {
    static int cleanup = 0;
    static async Task<int> job(int n) {
        try {
            await Tasks.pause();
            return n * 2;
        } finally {
            Program.cleanup = Program.cleanup + 1;
        }
    }
    static int hot(int i) {
        try {
            if (i % 7 == 0) { return 0; }
            return i;
        } finally {
            Program.cleanup = Program.cleanup + 1;
        }
    }
    static int Main() {
        int total = Program.job(21).wait();
        for (var i in 1..=140) {
            total = total + Program.hot(i);
        }
        return total * 1000 + Program.cleanup;
    }
}
"#;
    // job: 42, cleanup 1. hot: sum(1..140) - multiples of 7 (7+...+140 =
    // 7*sum(1..20)=1470) = 9870 - 1470 = 8400; +42 = 8442. cleanup 141.
    for trial in 0..3 {
        assert_parity(src, 8442141);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(50);
        match vm.run().expect("jit threshold-50 run") {
            Value::Int(i) => assert_eq!(i, 8442141, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
    }
}
