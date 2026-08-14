//! Type-level computation v1: literal-union algebra. Union declarations
//! compose other unions (`type Direction = Horizontal | Vertical;`),
//! members merge in declaration order with silent dedup across operands,
//! and subset widening lets a union flow into any union containing all its
//! members. Sum types are untouched: an all-bare-variant declaration is
//! only reinterpreted when every name resolves to a literal union, and
//! mixing the two is a hard error instead of a silently-wrong enum.

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

/// Composition (all-ident and mixed forms), nesting, membership of every
/// composed member, and subset widening in both legal directions.
#[test]
fn composition_and_subset_widening() {
    let src = r#"
type Horizontal = "east" | "west";
type Vertical = "north" | "south";
type Direction = Horizontal | Vertical;
type Extended = Direction | "up" | "down";
class Program {
    static int Rank(Extended e) {
        if (e == "east") { return 1; }
        if (e == "west") { return 2; }
        if (e == "north") { return 3; }
        if (e == "south") { return 4; }
        if (e == "up") { return 5; }
        return 6;
    }
    static int Main() {
        int total = 0;
        Horizontal h = "west";
        Direction d = h;
        Extended e = d;
        total = total + Rank(e) * 100;
        total = total + Rank("up") * 10;
        Vertical v = "south";
        total = total + Rank(v);
        return total;
    }
}
"#;
    // west=2 -> 200, up=5 -> 50, south=4 -> 4.
    assert_parity(src, 254);
}

/// Dedup across composed operands is silent and order-preserving; the
/// resulting union behaves identically to a hand-flattened one.
#[test]
fn overlap_dedups_silently() {
    let src = r#"
type Warm = "red" | "orange";
type Bright = "orange" | "yellow";
type Palette = Warm | Bright | "red" | "blue";
class Program {
    static int Main() {
        int total = 0;
        Palette p = "orange";
        if (p == "orange") { total = total + 1; }
        Palette q = "blue";
        if (q == "blue") { total = total + 10; }
        Warm w = "red";
        Palette widened = w;
        if (widened == "red") { total = total + 100; }
        return total;
    }
}
"#;
    assert_parity(src, 111);
}

/// The rules, pinned: superset-into-subset rejected, non-members rejected,
/// mixing unions with enum variants is an error, cycles are errors,
/// unknown names are errors — and ordinary bare-variant sum types still
/// work exactly as before.
#[test]
fn algebra_rules_are_enforced() {
    assert_compile_fails(
        r#"
type Direction = "north" | "south" | "east";
type Compass = "north" | "south";
class Program { static void Main() { Direction d = "east"; Compass c = d; } }
"#,
        "cannot assign",
    );
    assert_compile_fails(
        r#"
type Horizontal = "east" | "west";
type Direction = Horizontal | "up";
class Program { static void Main() { Direction d = "left"; } }
"#,
        "cannot assign",
    );
    assert_compile_fails(
        r#"
type Direction = "north" | "south";
type Bad = Direction | Err;
class Program { static void Main() { } }
"#,
        "mixes literal-union types and enum variants",
    );
    assert_compile_fails(
        r#"
type A = B | "x";
type B = A | "y";
class Program { static void Main() { } }
"#,
        "circular literal type",
    );
    assert_compile_fails(
        r#"
type D = "x" | Missing;
class Program { static void Main() { } }
"#,
        "is not a literal type",
    );

    // Bare-variant sum types keep their meaning when no name resolves to
    // a union.
    let sum = r#"
type Color = Red | Green | Blue;
class Program {
    static int Main() {
        Color c = Color.Green;
        return match (c) {
            Color.Red => 1,
            Color.Green => 2,
            Color.Blue => 3,
        };
    }
}
"#;
    assert_parity(sum, 2);
}

/// Composed unions in a hot loop crossing the JIT threshold mid-run:
/// erased to strings, dispatched via guards. 3 trials, compiled asserted.
#[test]
fn composed_unions_in_jit_hot_loop_trials() {
    let src = r#"
type Horizontal = "east" | "west";
type Vertical = "north" | "south";
type Direction = Horizontal | Vertical;
class Program {
    static int Step(Direction d) {
        if (d == "east") { return 1; }
        if (d == "west") { return -1; }
        if (d == "north") { return 10; }
        return -10;
    }
    static int Main() {
        List<Direction> path = new List<Direction>();
        path.Add("east");
        path.Add("north");
        path.Add("east");
        path.Add("south");
        int pos = 0;
        int round = 0;
        while (round < 120) {
            for (Direction d in path) {
                pos = pos + Step(d);
            }
            round = round + 1;
        }
        return pos;
    }
}
"#;
    // per round: 1 + 10 + 1 - 10 = 2; 120 * 2 = 240.
    for trial in 0..3 {
        assert_parity(src, 240);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 240, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
