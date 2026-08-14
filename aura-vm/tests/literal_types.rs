//! String-literal union types: `type Direction = "north" | "south" | ...;`.
//! A literal is assignable to the union exactly when it is a declared
//! member; union values widen to `string` (and get string methods) but
//! plain strings and other unions never flow in. Erased to string at
//! runtime.

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

/// Members flow in as literals; union values pass as arguments, return
/// from methods, and compare against member literals.
#[test]
fn members_assign_compare_dispatch() {
    let src = r#"
type Direction = "north" | "south" | "east" | "west";
class Program {
    static int Score(Direction d) {
        if (d == "north") { return 1; }
        if (d == "south") { return 2; }
        if (d == "east") { return 3; }
        return 4;
    }
    static Direction Turn(Direction d) {
        if (d == "north") { return "east"; }
        return "north";
    }
    static int Main() {
        Direction d = "north";
        Direction e = Turn(d);
        int total = Score(d) * 1000 + Score(e) * 100 + Score("west");
        if (d == e) { total = total + 50000; }
        Direction f = "north";
        if (d == f) { total = total + 100000; }
        return total;
    }
}
"#;
    // 1*1000 + 3*100 + 4 + 100000 = 101304.
    assert_parity(src, 101304);
}

/// The checking rules, pinned: non-member literal, plain string, another
/// union (even with overlapping members), non-member comparison, widening
/// to non-string, and duplicate members in the declaration.
#[test]
fn union_rules_are_enforced() {
    let cases: &[(&str, &str)] = &[
        (
            r#"
type Direction = "north" | "south";
class Program { static void Main() { Direction d = "up"; } }
"#,
            "cannot assign",
        ),
        (
            r#"
type Direction = "north" | "south";
class Program { static void Main() { string s = "north"; Direction d = s; } }
"#,
            "cannot assign",
        ),
        (
            r#"
type Direction = "north" | "south" | "east" | "west";
type Compass = "north" | "south";
class Program { static void Main() { Compass c = "north"; Direction d = c; } }
"#,
            "cannot assign",
        ),
        (
            r#"
type Direction = "north" | "south";
class Program { static void Main() { Direction d = "north"; bool b = d == "up"; } }
"#,
            "cannot compare",
        ),
        (
            r#"
type Direction = "north" | "south";
class Program { static void Main() { Direction d = "north"; int n = d; } }
"#,
            "cannot assign",
        ),
        (
            r#"
type Direction = "north" | "north";
class Program { static void Main() { } }
"#,
            "duplicate literal",
        ),
    ];
    for (src, needle) in cases {
        assert_compile_fails(src, needle);
    }
}

/// Widening: a union value is a string — assignable to string, usable with
/// string methods and Length directly.
#[test]
fn widens_to_string_with_methods() {
    let src = r#"
type Direction = "north" | "south";
class Program {
    static int Main() {
        Direction d = "north";
        string s = d;
        int total = s.Length;
        total = total + d.Length * 100;
        if (d.ToUpper() == "NORTH") { total = total + 1000; }
        if (d.StartsWith("no")) { total = total + 10000; }
        return total;
    }
}
"#;
    // 5 + 500 + 1000 + 10000 = 11505.
    assert_parity(src, 11505);
}

/// Erasure: literal unions work as hashed Map keys, List elements, foreach
/// variables, and nullable (`Direction?`) with narrowing.
#[test]
fn unions_in_collections_and_nullable() {
    let src = r#"
type Direction = "north" | "south" | "east" | "west";
class Program {
    static int Main() {
        Map<Direction, int> steps = new Map<Direction, int>();
        steps.Put("north", 3);
        steps.Put("east", 7);
        List<Direction> path = new List<Direction>();
        path.Add("north");
        path.Add("east");
        path.Add("north");
        int total = 0;
        for (Direction p in path) {
            total = total + steps.Get(p);
        }
        Direction? maybe = null;
        if (maybe == null) { total = total + 100; }
        maybe = "west";
        if (maybe != null) {
            if (maybe == "west") { total = total + 1000; }
        }
        return total;
    }
}
"#;
    // 3+7+3 = 13; +100 +1000 = 1113.
    assert_parity(src, 1113);
}

/// Literal-typed dispatch in a method crossing the JIT threshold mid-run,
/// 3 trials, compiled count asserted.
#[test]
fn literal_dispatch_in_jit_hot_loop_trials() {
    let src = r#"
type Direction = "north" | "south" | "east" | "west";
class Program {
    static int Score(Direction d) {
        if (d == "north") { return 1; }
        if (d == "south") { return 2; }
        if (d == "east") { return 3; }
        return 4;
    }
    static int Main() {
        List<Direction> ring = new List<Direction>();
        ring.Add("north");
        ring.Add("south");
        ring.Add("east");
        ring.Add("west");
        int total = 0;
        int round = 0;
        while (round < 100) {
            for (Direction d in ring) {
                total = total + Score(d);
            }
            round = round + 1;
        }
        return total;
    }
}
"#;
    // 100 * (1+2+3+4) = 1000.
    for trial in 0..3 {
        assert_parity(src, 1000);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(50);
        match vm.run().expect("jit threshold-50 run") {
            Value::Int(i) => assert_eq!(i, 1000, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
