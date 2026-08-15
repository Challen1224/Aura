//! Generic type inference at construction: `new Box(42)` infers `Box<int>`
//! by unifying constructor parameters against the argument types, exactly
//! like generic-method call-site inference — bindings from several
//! arguments join through assignability, conflicts are errors, and the
//! inferred arguments pass the same constraint checks as explicit ones.

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

/// The TODO example and friends: single and multi parameter classes,
/// records, nested construction, `var` and explicitly-typed targets,
/// literal joining across arguments, and chained member access on the
/// inferred instantiation (fields and methods both substitute).
#[test]
fn construction_inference_works() {
    let src = r#"
class Box<T> {
    T v;
    Box(T v) { this.v = v; }
    T get() { return this.v; }
}
class Pair<K, V> {
    K k;
    V v;
    Pair(K k, V v) { this.k = k; this.v = v; }
}
record Point<T>(T x, T y);
class Base {
    virtual int tag() { return 1; }
}
class Derived : Base {
    override int tag() { return 2; }
}
class Program {
    static int Main() {
        var b = new Box(42);
        int total = b.get();
        Box<string> s = new Box("hello");
        total = total + s.get().Length * 10;
        var p = new Pair("key", 30);
        total = total + p.k.Length * 100 + p.v;
        var pt = new Point(2, 9);
        total = total + (pt.x + pt.y) * 1000;
        var nested = new Box(new Box(7));
        total = total + nested.get().get() * 10000;
        // Joining: Derived and Base arguments bind T to the wider Base.
        var join = new Pair(new Derived(), new Base());
        Pair<Derived, Base> _typed = join;
        total = total + join.k.tag() * 100000;
        return total;
    }
}
"#;
    // 42 + 50 + 300 + 30 + 11000 + 70000 + 200000 = 281422.
    assert_parity(src, 281422);
}

/// Inference failures and interactions, pinned.
#[test]
fn construction_inference_rules() {
    let cases: Vec<(String, &str)> = vec![
        // The inferred instantiation is real: mismatched assignment caught.
        (
            "class Box<T> { T v; Box(T v) { this.v = v; } } \
             class Program { static void Main() { var b = new Box(1); Box<string> s = b; } }"
                .to_string(),
            "cannot assign Box<int32>",
        ),
        // A constructor that never mentions T cannot determine it.
        (
            "class W<T> { int n; W(int n) { this.n = n; } } \
             class Program { static void Main() { var w = new W(5); } }"
                .to_string(),
            "cannot infer type arguments for `W`",
        ),
        // Irreconcilable bindings are an error, not first-wins.
        (
            "class B<T> { T a; B(T a, T b) { this.a = a; } } \
             class Program { static void Main() { var x = new B(1, \"s\"); } }"
                .to_string(),
            "conflicting argument types int32 and string",
        ),
        // Constraints apply to inferred arguments exactly as to explicit ones.
        (
            "class P<T> where T : int | float { T v; P(T v) { this.v = v; } } \
             class Program { static void Main() { var p = new P(\"s\"); } }"
                .to_string(),
            "must be one of int32 | float64",
        ),
        (
            "interface IShow { int show(); } \
             class C<T> where T : IShow { T v; C(T v) { this.v = v; } } \
             class Plain { int x; Plain(int x) { this.x = x; } } \
             class Program { static void Main() { var c = new C(new Plain(1)); } }"
                .to_string(),
            "does not satisfy constraint IShow",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// The pre-existing generic-field-typing bug this feature exposed: field
/// access on a generic instantiation (`p.k.Length`) failed to compile
/// (emit error `unknown field Length on K`) even with explicit type
/// arguments — the emitter never substituted the receiver's instantiation
/// into field types. Methods worked; fields now do too.
#[test]
fn generic_field_access_types_correctly() {
    let src = r#"
class Pair<K, V> {
    K k;
    V v;
    Pair(K k, V v) { this.k = k; this.v = v; }
}
class Box<T> {
    T v;
    Box(T v) { this.v = v; }
}
class Program {
    static int Main() {
        Pair<string, float> p = new Pair<string, float>("abc", 1.5);
        int total = p.k.Length;
        Box<Box<int>> bb = new Box<Box<int>>(new Box<int>(9));
        total = total + bb.v.v * 10;
        var q = new Pair("wxyz", new Box(5));
        total = total + q.k.Length * 100 + q.v.v * 1000;
        return total;
    }
}
"#;
    // 3 + 90 + 400 + 5000 = 5493.
    assert_parity(src, 5493);
}

/// Inferred construction in a JIT hot loop crossing the threshold mid-run,
/// with GC churn asserted in the interpreter run. 3 trials, compiled count
/// asserted.
#[test]
fn inference_in_jit_hot_loop_trials() {
    let src = r#"
class Cell<T> {
    T v;
    Cell(T v) { this.v = v; }
    T get() { return this.v; }
}
class Program {
    static int Main() {
        int total = 0;
        for (var i in 1..=200) {
            var c = new Cell(i * 3);
            total = total + c.get();
        }
        return total;
    }
}
"#;
    // 3 * sum(1..200) = 3 * 20100 = 60300.
    for trial in 0..3 {
        assert_parity(src, 60300);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.set_gc_threshold(2048);
        match vm.run().expect("gc-churn run") {
            Value::Int(i) => assert_eq!(i, 60300, "trial {trial} under GC churn"),
            other => panic!("expected int, got {other:?}"),
        }
        assert!(vm.gc_collections() > 0, "trial {trial}: expected GC cycles");
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 60300, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
