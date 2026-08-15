//! Nested classes: `class Outer { class Inner { } }`. A compile-time
//! desugar — nested classes hoist to top level under mangled names
//! (`Outer.Inner`) with references resolved through the enclosing scope
//! chain: unqualified `Inner` from inside `Outer`, qualified `Outer.Inner`
//! from outside (types, `new`, static member access, patterns). A nested
//! class can read its enclosing class's private members; the reverse is
//! denied, as in C#.

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

/// The basics under both tiers: qualified construction and types from
/// outside, unqualified sibling references inside, instance and static
/// members on nested classes (including a nested static class), and a
/// static store through a class path.
#[test]
fn nested_class_basics() {
    let src = r#"
class Outer {
    static int counter = 100;

    class Inner {
        int v;
        Inner(int v) { this.v = v; }
        int bump() {
            Outer.counter = Outer.counter + 1;
            return this.v * 2;
        }
    }

    static class Names {
        static int Offset = 7;
        static int Tag(int i) { return Names.Offset + i; }
    }

    class Pair {
        Inner a;
        Inner b;
        Pair(Inner a, Inner b) { this.a = a; this.b = b; }
        int sum() { return this.a.v + this.b.v; }
    }

    static Pair MakePair() { return new Pair(new Inner(3), new Inner(4)); }
}
class Program {
    static int Main() {
        Outer.Inner i = new Outer.Inner(21);
        int total = i.bump();
        total = total + Outer.counter;
        total = total + Outer.Names.Tag(10) * 1000;
        total = total + Outer.MakePair().sum() * 100000;
        Outer.Names.Offset = 50;
        total = total + Outer.Names.Tag(0) * 1000000;
        if (i is Outer.Inner) { total = total + 10000000; }
        return total;
    }
}
"#;
    // 42 + 101 + 17000 + 700000 + 50000000 + 10000000 = 60717143.
    assert_parity(src, 60717143);
}

/// Scoping: a nested class shadows a top-level class of the same name from
/// inside its outer class; deep nesting resolves through every level and
/// reaches the outermost class's private statics; generic nested classes
/// substitute normally.
#[test]
fn scoping_shadowing_and_depth() {
    let src = r#"
class Helper {
    static int Tag() { return 1; }
}
class Outer {
    private static int secret = 7;

    class Helper {
        static int Tag() { return 2; }
    }

    class Mid {
        class Leaf {
            int leafVal() { return Outer.secret * 10; }
        }
        Leaf mk() { return new Leaf(); }
    }

    class Cell<T> {
        T v;
        Cell(T v) { this.v = v; }
        T get() { return this.v; }
    }

    static int UseHelper() { return Helper.Tag(); }
}
class Program {
    static int Main() {
        int total = Outer.UseHelper() * 10 + Helper.Tag();
        Outer.Mid m = new Outer.Mid();
        total = total + m.mk().leafVal() * 100;
        Outer.Cell<string> c = new Outer.Cell<string>("deep");
        total = total + c.get().Length * 10000;
        return total;
    }
}
"#;
    // 2*10 + 1 + 70*100 + 4*10000 = 47021.
    assert_parity(src, 47021);
}

/// Nested records: construction and pattern matching, unqualified inside
/// and dotted (`Outer.P(x, _)`) outside; inheritance from a nested base.
#[test]
fn nested_records_and_inheritance() {
    let src = r#"
class Outer {
    record P(int x, int y);

    class Base {
        virtual int kind() { return 1; }
    }
    class Derived : Base {
        override int kind() { return 2; }
    }

    static int MatchP(P p) {
        return match (p) {
            P(0, y) => y,
            P(x, _) => x * 1000,
        };
    }
    static Base MakeDerived() { return new Derived(); }
}
class Program {
    static int Main() {
        int total = Outer.MatchP(new Outer.P(0, 42));
        Outer.P p = new Outer.P(9, 1);
        total = total + Outer.MatchP(p);
        total = total + match (p) { Outer.P(x, _) => x + 500, };
        total = total + Outer.MakeDerived().kind() * 100000;
        return total;
    }
}
"#;
    // 42 + 9000 + 509 + 200000 = 209551.
    assert_parity(src, 209551);
}

/// Access rules and name resolution errors, pinned.
#[test]
fn nested_class_rules_are_enforced() {
    let prog = "class Program { static void Main() { } }";
    let cases: Vec<(String, &str)> = vec![
        // Outer cannot reach a nested class's private members (C# rule)...
        (
            format!(
                "class O {{ class I {{ private int s; I() {{ this.s = 1; }} }} \
                 static int Bad(I i) {{ return i.s; }} }} {prog}"
            ),
            "is private",
        ),
        // ...while unrelated classes cannot reach Outer's privates either.
        (
            format!(
                "class O {{ private static int s = 5; class I {{ }} }} \
                 class Q {{ static int Bad() {{ return O.s; }} }} {prog}"
            ),
            "is private",
        ),
        // Unqualified nested names do not leak out of their outer class.
        (
            "class O { class I { int v; } } \
             class Program { static void Main() { I x = null; } }"
                .to_string(),
            "unknown type `I`",
        ),
        // Duplicate nested names collide under the mangled name.
        (
            format!("class O {{ class I {{ int a; }} class I {{ int b; }} }} {prog}"),
            "duplicate class `O.I`",
        ),
        // Interfaces cannot declare nested classes.
        (
            format!("interface IF {{ class X {{ int v; }} }} {prog}"),
            "cannot declare nested class",
        ),
        // A nested class is a real class: instantiating an abstract nested
        // class is still rejected.
        (
            format!(
                "class O {{ abstract class A {{ }} static void Bad() {{ new A(); }} }} {prog}"
            ),
            "cannot instantiate",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// A nested class can read the enclosing class's private statics from any
/// nesting depth (verified positively in scoping_shadowing_and_depth), and
/// extension methods can target nested classes.
#[test]
fn extensions_on_nested_classes() {
    let src = r#"
class Outer {
    class Inner {
        int v;
        Inner(int v) { this.v = v; }
    }
    static Inner Make(int v) { return new Inner(v); }
}
extension InnerExtensions on Outer.Inner {
    int doubled() { return this.v * 2; }
}
class Program {
    static int Main() {
        Outer.Inner i = Outer.Make(21);
        return i.doubled();
    }
}
"#;
    assert_parity(src, 42);
}

/// Nested-class allocation and dispatch in a JIT hot loop crossing the
/// threshold mid-run, with GC churn asserted in the interpreter run.
/// 3 trials, compiled count asserted.
#[test]
fn nested_classes_in_jit_hot_loop_trials() {
    let src = r#"
class Sim {
    static int ticks = 0;

    class Body {
        int m;
        Body(int m) { this.m = m; }
        int energy() {
            Sim.ticks = Sim.ticks + 1;
            return this.m * this.m;
        }
    }

    static Body Spawn(int m) { return new Body(m); }
}
class Program {
    static int Main() {
        int total = 0;
        for (var i in 1..=200) {
            total = total + Sim.Spawn(i).energy();
        }
        return total + Sim.ticks;
    }
}
"#;
    // sum(i^2, 1..200) + 200 = 2686700 + 200 = 2686900.
    for trial in 0..3 {
        assert_parity(src, 2686900);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.set_gc_threshold(2048);
        match vm.run().expect("gc-churn run") {
            Value::Int(i) => assert_eq!(i, 2686900, "trial {trial} under GC churn"),
            other => panic!("expected int, got {other:?}"),
        }
        assert!(vm.gc_collections() > 0, "trial {trial}: expected GC cycles");
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 2686900, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
