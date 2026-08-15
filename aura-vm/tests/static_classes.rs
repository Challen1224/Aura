//! Static classes (the namespace idiom) and constant static field
//! initializers. A `static class` has no instances, no inheritance in
//! either direction, only static members, and cannot be used as a type.
//! Static fields (on any class) accept constant-literal initializers,
//! applied at VM startup; instance field initializers are not supported.

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

/// The namespace idiom: constants of every literal kind plus static
/// methods, consumed across the program.
#[test]
fn namespace_idiom_works() {
    let src = r#"
static class MathUtil {
    static int Answer = 42;
    static float Half = 0.5;
    static bool Enabled = true;
    static char Sep = ':';
    static string Name = "aura";
    static int Offset = -7;

    static int Max(int a, int b) {
        if (a > b) { return a; }
        return b;
    }
}
class Program {
    static int Main() {
        int total = MathUtil.Max(3, 9) * 100;
        total = total + MathUtil.Answer;
        if (MathUtil.Enabled) { total = total + 1000; }
        if (MathUtil.Half < 1.0) { total = total + 10000; }
        if (MathUtil.Sep == ':') { total = total + 100000; }
        total = total + MathUtil.Name.Length;
        total = total + MathUtil.Offset;
        return total;
    }
}
"#;
    // 900 + 42 + 1000 + 10000 + 100000 + 4 - 7 = 111939.
    assert_parity(src, 111939);
}

/// Static fields stay mutable state (initializers are starting values,
/// not consts): writes persist across static method calls.
#[test]
fn initialized_statics_are_mutable() {
    let src = r#"
static class Counter {
    static int Count = 100;
    static void Bump() { Counter.Count = Counter.Count + 1; }
}
class Program {
    static int Main() {
        Counter.Bump();
        Counter.Bump();
        return Counter.Count;
    }
}
"#;
    assert_parity(src, 102);
}

/// Every rule, pinned.
#[test]
fn static_class_rules_are_enforced() {
    let cases: &[(&str, &str)] = &[
        (
            "static class M { static int X = 1; } class P : M { int y; } class Program { static void Main() { } }",
            "cannot inherit from static class",
        ),
        (
            "static class M { static int X = 1; } class Program { static void Main() { M v = null; } }",
            "cannot be used as a type",
        ),
        (
            "static class M { int y; } class Program { static void Main() { } }",
            "all members must be static",
        ),
        (
            "static class M { int Y() { return 1; } } class Program { static void Main() { } }",
            "all members must be static",
        ),
        (
            "static class M { M() { } } class Program { static void Main() { } }",
            "cannot declare a constructor",
        ),
        (
            "static class M : Program { static int X = 1; } class Program { static void Main() { } }",
            "cannot inherit or implement",
        ),
        (
            "class C { static int X = 1 + 2; } class Program { static void Main() { } }",
            "must be a constant literal",
        ),
        (
            "class C { int x = 5; } class Program { static void Main() { } }",
            "cannot have an initializer yet",
        ),
        (
            "class C { static int X = \"no\"; } class Program { static void Main() { } }",
            "cannot initialize static field",
        ),
        (
            "class C { static int8 X = 300; } class Program { static void Main() { } }",
            "cannot initialize static field",
        ),
    ];
    for (src, needle) in cases {
        assert_compile_fails(src, needle);
    }
}

/// Non-static classes keep constant initializers too, and the string
/// constants survive GC (they live in static-field roots).
#[test]
fn initializers_survive_gc() {
    let src = r#"
class Config {
    static string Greeting = "hello";
}
class Program {
    static int Main() {
        int churn = 0;
        for (var i in 1..=500) {
            string tmp = "junk {i}";
            churn = churn + tmp.Length - tmp.Length;
        }
        return Config.Greeting.Length + churn;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(2048);
    match vm.run().expect("run") {
        Value::Int(i) => assert_eq!(i, 5),
        other => panic!("expected int, got {other:?}"),
    }
    assert!(vm.gc_collections() > 0, "expected GC cycles during churn");
}

/// Static-class consts and methods in a JIT hot loop crossing the
/// threshold mid-run. 3 trials, compiled count asserted.
#[test]
fn static_class_in_jit_hot_loop_trials() {
    let src = r#"
static class Phys {
    static int Gravity = 10;
    static int Fall(int t) { return Phys.Gravity * t * t; }
}
class Program {
    static int Main() {
        int total = 0;
        for (var t in 1..=150) {
            total = total + Phys.Fall(t);
        }
        return total;
    }
}
"#;
    // 10 * sum(t^2, 1..150) = 10 * 1136275 = 11362750.
    for trial in 0..3 {
        assert_parity(src, 11362750);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 11362750, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
