//! HM-style type inference: `var` locals inferred from initializers, and
//! generic-method call-site type-argument inference by structural
//! unification of parameter types against argument types (bindings join
//! through assignability; conflicts and unsatisfied constraints are
//! errors). Full Hindley-Milner is deliberately out of scope — it does not
//! coexist with subtyping — so inference is local, like C#/Kotlin.

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

/// `var` infers from initializers: literals resolve to carrier types,
/// objects/collections/nullables keep their full types, and inferred
/// locals behave identically to annotated ones (dispatch, narrowing,
/// foreach).
#[test]
fn var_infers_from_initializers() {
    let src = r#"
class Point {
    int x;
    Point(int x) { this.x = x; }
    int X() { return this.x; }
}
class Program {
    static Point? Find(int key) {
        if (key > 0) { return new Point(100); }
        return null;
    }
    static int Main() {
        var i = 40;
        var s = "hello";
        var p = new Point(7);
        var xs = new List<int>();
        xs.Add(i);
        xs.Add(2);
        var total = i + s.Length + p.X();
        for (var e in xs) { total = total + e; }
        for (var r in 1..=3) { total = total + r; }
        // Nullable inference + narrowing on a var local.
        var maybe = Find(total);
        if (maybe != null) { total = total + maybe.X(); }
        return total;
    }
}
"#;
    // 40 + 5 + 7 = 52; +40 +2 = 94; +1+2+3 = 100; +100 (narrowed var) = 200.
    assert_parity(src, 200);
}

/// `var` requires an inferable initializer.
#[test]
fn var_error_cases() {
    assert_compile_fails(
        "class Program { static void Main() { var x; } }",
        "requires an initializer",
    );
    assert_compile_fails(
        "class Program { static void Main() { var x = null; } }",
        "cannot infer a type",
    );
    assert_compile_fails(
        r#"
class Program {
    static void Side() { }
    static void Main() { var x = Side(); }
}
"#,
        "cannot infer a type",
    );
}

/// Call-site unification: flat and nested parameter shapes, chained
/// dispatch on inferred returns, literal joining across arguments, and
/// per-call-site independence for instance generic methods.
#[test]
fn generic_method_inference() {
    let src = r#"
class Util {
    static <T> T Pick(T a, T b) { return b; }
    static <T> List<T> Pair(T a, T b) {
        List<T> xs = new List<T>();
        xs.Add(a);
        xs.Add(b);
        return xs;
    }
    static <T> T First(List<T> xs) { return xs.Get(0); }
    <U> U Echo(U v) { return v; }
}
class Program {
    static int Main() {
        int total = Util.Pick(3, 9);
        total = total + Util.Pick("aa", "bcde").Length;
        var pair = Util.Pair(5, 6);
        total = total + Util.First(pair) * 100;
        var u = new Util();
        total = total + u.Echo(1000);
        total = total + u.Echo("xy").Length;
        return total;
    }
}
"#;
    // 9 + 4 + 500 + 1000 + 2 = 1515.
    assert_parity(src, 1515);
}

/// Constraints on method generics are enforced against inferred bindings,
/// and a constrained parameter's members are callable in the body
/// (bounded polymorphism) — structural satisfaction included.
#[test]
fn constrained_method_generics() {
    let good = r#"
interface Sized { int Size(); }
class Box {
    int v;
    Box(int v) { this.v = v; }
    int Size() { return this.v; }
}
class Util {
    static <T : Sized> int Measure(T x) { return x.Size(); }
}
class Program {
    static int Main() {
        return Util.Measure(new Box(23));
    }
}
"#;
    assert_parity(good, 23);

    assert_compile_fails(
        r#"
interface Sized { int Size(); }
class Util {
    static <T : Sized> int Measure(T x) { return x.Size(); }
}
class Program { static void Main() { int m = Util.Measure(5); } }
"#,
        "does not satisfy constraint",
    );
    assert_compile_fails(
        r#"
class Util {
    static <T> int Bad(T x) { return x.Size(); }
}
class Program { static void Main() { } }
"#,
        "no bound declares it",
    );
}

/// Irreconcilable bindings are a dedicated error, and assignable bindings
/// join to the more general type (subclass + superclass infer the base).
#[test]
fn binding_conflicts_and_joins() {
    assert_compile_fails(
        r#"
class Util { static <T> T Pick(T a, T b) { return b; } }
class Program { static void Main() { int x = Util.Pick(1, "s"); } }
"#,
        "conflicting argument types",
    );

    let join = r#"
class Animal {
    int legs;
    Animal(int legs) { this.legs = legs; }
    int Legs() { return this.legs; }
}
class Dog : Animal {
    Dog() : super(4) {}
}
class Util { static <T> T Pick(T a, T b) { return b; } }
class Program {
    static int Main() {
        Animal a = new Animal(2);
        Dog d = new Dog();
        // T joins to Animal (Dog is assignable to it).
        var picked = Util.Pick(a, d);
        return picked.Legs();
    }
}
"#;
    assert_parity(join, 4);
}

/// Inference-heavy code in a hot loop crossing the JIT threshold mid-run:
/// var locals + inferred generic calls erase to the same bytecode as
/// annotated code. 3 trials, compiled count asserted.
#[test]
fn inference_in_jit_hot_loop_trials() {
    let src = r#"
class Util {
    static <T> T Pick(T a, T b) { return b; }
}
class Program {
    static int Round(int i) {
        var doubled = Util.Pick(i, i * 2);
        var s = Util.Pick("a", "bc");
        return doubled + s.Length;
    }
    static int Main() {
        var total = 0;
        for (var i in 1..=200) {
            total = total + Round(i);
        }
        return total;
    }
}
"#;
    // sum(2i + 2, i=1..200) = 2*20100 + 400 = 40600.
    for trial in 0..3 {
        assert_parity(src, 40600);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(80);
        match vm.run().expect("jit threshold-80 run") {
            Value::Int(i) => assert_eq!(i, 40600, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
