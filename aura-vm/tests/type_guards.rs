//! Type guards beyond null checks: `expr is Type [binding]` runtime tests
//! with flow-sensitive bindings, and compound-condition narrowing through
//! `&&`, `||` and `!` — which requires (and pins) short-circuit evaluation
//! of the logical operators.

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

const SHAPES: &str = r#"
interface Shape { int Area(); }
class Circle {
    int r;
    Circle(int r) { this.r = r; }
    int Area() { return 3 * this.r * this.r; }
    int Radius() { return this.r; }
}
class Square {
    int side;
    Square(int s) { this.side = s; }
    int Area() { return this.side * this.side; }
}
"#;

/// `is` with binding dispatches per runtime type through interface refs,
/// subclass relations included; null subjects test false.
#[test]
fn is_checks_and_bindings() {
    let src = format!(
        "{SHAPES}
class Program {{
    static int Inspect(Shape s) {{
        if (s is Circle c) {{ return c.Radius(); }}
        if (s is Square sq) {{ return sq.Area() * 10; }}
        return -1;
    }}
    static int Main() {{
        int total = Inspect(new Circle(6)) + Inspect(new Square(4));
        Shape s = new Circle(2);
        if (s is Circle) {{ total = total + 1000; }}
        if (s is Square) {{ total = total + 777777; }}
        Circle? gone = null;
        if (gone is Circle) {{ total = total + 555555; }}
        return total;
    }}
}}
"
    );
    // 6 + 160 + 1000 = 1166.
    assert_parity(&src, 1166);
}

/// Compound `&&`: null check plus member access in one condition; `is`
/// binding usable in the rhs of the same condition.
#[test]
fn compound_and_narrowing() {
    let src = format!(
        "{SHAPES}
class Program {{
    static int Main() {{
        int total = 0;
        Circle? maybe = new Circle(5);
        if (maybe != null && maybe.Radius() > 3) {{ total = total + 1; }}
        Circle? empty = null;
        if (empty != null && empty.Radius() > 3) {{ total = total + 100000; }}
        Shape s = new Circle(7);
        if (s is Circle c && c.Radius() > 5) {{ total = total + 10; }}
        if (s is Square sq && sq.Area() > 0) {{ total = total + 100000; }}
        return total;
    }}
}}
"
    );
    assert_parity(&src, 11);
}

/// `||` narrows its rhs by the lhs' negation, and `!` flips polarity —
/// including the else-branch and after-terminating-branch paths.
#[test]
fn or_and_not_narrowing() {
    let src = format!(
        "{SHAPES}
class Program {{
    static int Check(Circle? m) {{
        if (m == null || m.Radius() < 3) {{ return 0; }}
        return m.Radius();
    }}
    static int Main() {{
        int total = Check(null) + Check(new Circle(1)) * 10 + Check(new Circle(9));
        Circle? m = new Circle(4);
        if (!(m == null)) {{ total = total + m.Radius() * 1000; }}
        return total;
    }}
}}
"
    );
    // 0 + 0 + 9 + 4000 = 4009.
    assert_parity(&src, 4009);
}

/// Short-circuit semantics, observed through side effects: the rhs of `&&`
/// must not run when the lhs is false, nor the rhs of `||` when the lhs is
/// true. The narrowing rules are only sound because of this.
#[test]
fn logical_operators_short_circuit() {
    let src = r#"
class Counter {
    int calls;
    Counter() { this.calls = 0; }
    bool Bump() { this.calls = this.calls + 1; return true; }
}
class Program {
    static int Main() {
        Counter c = new Counter();
        bool a = false && c.Bump();
        bool b = true || c.Bump();
        bool d = true && c.Bump();
        bool e = false || c.Bump();
        if (a) { return -1; }
        if (!b) { return -2; }
        if (!d) { return -3; }
        if (!e) { return -4; }
        return c.calls;
    }
}
"#;
    // Only the two non-short-circuited calls run.
    assert_parity(src, 2);
}

/// The rules, pinned: bindings are scoped to the true-context only, wrong
/// guard order fails, impossible tests fail, primitives are not testable.
#[test]
fn guard_rules_are_enforced() {
    let cases: &[(&str, &str)] = &[
        (
            "if (s is Circle c) { } else { int x = c.Radius(); }",
            "unknown variable `c`",
        ),
        (
            "bool b = s is Circle c2; int x = c2.Radius();",
            "unknown variable `c2`",
        ),
        (
            "Circle? m = new Circle(1); if (m.Radius() > 0 && m != null) { }",
            "may be null",
        ),
        (
            "Circle onlyc = new Circle(1); if (onlyc is Square) { }",
            "can never succeed",
        ),
        ("int i = 3; if (i is Circle) { }", "class-typed subject"),
        (
            "List<int> xs = new List<int>(); if (xs is Circle) { }",
            "can never succeed",
        ),
    ];
    for (stmt, needle) in cases {
        let src = format!(
            "{SHAPES}
class Program {{ static void Main() {{ Shape s = new Circle(1); {stmt} }} }}
"
        );
        assert_compile_fails(&src, needle);
    }
}

/// Guards in a hot loop crossing the JIT threshold mid-run: heterogeneous
/// shapes dispatched via `is` chains, compound null guards in the loop
/// condition. 3 trials, compiled count asserted.
#[test]
fn guards_in_jit_hot_loop_trials() {
    let src = format!(
        "{SHAPES}
class Program {{
    static int Classify(Shape s) {{
        if (s is Circle c && c.Radius() > 2) {{ return 3; }}
        if (s is Circle) {{ return 2; }}
        if (s is Square sq) {{ return sq.Area(); }}
        return 0;
    }}
    static int Main() {{
        List<Shape> shapes = new List<Shape>();
        shapes.Add(new Circle(1));
        shapes.Add(new Circle(5));
        shapes.Add(new Square(3));
        int total = 0;
        int round = 0;
        while (round < 90) {{
            for (Shape s in shapes) {{
                total = total + Classify(s);
            }}
            round = round + 1;
        }}
        return total;
    }}
}}
"
    );
    // 90 * (2 + 3 + 9) = 1260.
    for trial in 0..3 {
        assert_parity(&src, 1260);
        let module = Arc::new(compile(&src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(40);
        match vm.run().expect("jit threshold-40 run") {
            Value::Int(i) => assert_eq!(i, 1260, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
