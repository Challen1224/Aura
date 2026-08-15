//! Regression: bytecode with unreachable blocks (dead code) crashed the
//! x86-64 JIT with an index-out-of-bounds panic in the register allocator.
//! `lower_blocks` skipped unreachable blocks, leaving a dense `func.blocks`
//! vec while block ids and successor lists kept the sparse numbering that
//! regalloc/codegen index by. The fix keeps inert placeholder blocks in the
//! id slots. The simplest trigger: a match whose last arm is irrefutable
//! emits a no-match throw that nothing branches to.

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

/// The original panic: a single-arm record match. Its pattern is
/// irrefutable, so the emitted no-match throw block has no predecessors.
#[test]
fn single_arm_match_compiles() {
    let src = r#"
record P(int x, int y);
class Program {
    static int Main() {
        P p = new P(9, 1);
        return match (p) { P(x, _) => x + 500, };
    }
}
"#;
    assert_parity(src, 509);
}

/// Dead code after an early return is another unreachable-block shape.
#[test]
fn code_after_return_compiles() {
    let src = r#"
class Program {
    static int Twice(int v) {
        return v * 2;
        v = v + 1;
        return v;
    }
    static int Main() { return Program.Twice(21); }
}
"#;
    assert_parity(src, 42);
}

/// Dead blocks inside a hot method that actually tiers up: the method must
/// really JIT-compile (asserted), match the interpreter, and survive 3
/// trials.
#[test]
fn dead_blocks_in_jit_hot_loop_trials() {
    let src = r#"
record Score(int base, int bonus);
class Program {
    static int Rate(Score s) {
        return match (s) { Score(b, extra) => b * 10 + extra, };
    }
    static int Main() {
        int total = 0;
        for (var i in 1..=150) {
            total = total + Program.Rate(new Score(i, 3));
        }
        return total;
    }
}
"#;
    // sum(10i + 3, 1..150) = 10*11325 + 450 = 113700.
    for trial in 0..3 {
        assert_parity(src, 113700);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 113700, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}

/// Regression: `throw` was classified as a block terminator but the
/// terminator lowering only knew Br/BrTrue/BrFalse/Ret — the throw op was
/// silently dropped and compiled code fell through past it, returning
/// normal values instead of raising. A compiled method's throw must cross
/// the tier boundary as an exception.
#[test]
fn compiled_throw_raises() {
    let src = r#"
class E : Exception {
    E(string m) { this.message = m; }
}
class Program {
    static int f(int i) {
        if (i % 3 == 0) {
            throw new E("boom");
        }
        return i;
    }
    static int Main() {
        int total = 0;
        for (var i in 1..=100) {
            try {
                total = total + Program.f(i);
            } catch (Exception e) {
                total = total + e.message.Length;
            }
        }
        return total;
    }
}
"#;
    // sum(1..100) - sum(multiples of 3) + 33*4 = 5050 - 1683 + 132 = 3499.
    for trial in 0..3 {
        assert_parity(src, 3499);
    }
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.enable_jit();
    vm.set_jit_threshold(30);
    match vm.run().expect("jit threshold-30 run") {
        Value::Int(i) => assert_eq!(i, 3499, "tier transition"),
        other => panic!("expected int, got {other:?}"),
    }
    #[cfg(target_arch = "x86_64")]
    assert!(vm.jit_compiled_count() > 0, "nothing compiled");
}
