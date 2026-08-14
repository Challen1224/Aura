//! Newtype pattern: `newtype UserId = int;` declares a distinct nominal
//! wrapper over a primitive. No implicit conversion in either direction,
//! construction via `UserId(expr)`, unwrap via `.Value`, full erasure at
//! runtime (zero cost — the JIT sees plain primitives).

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

/// Wrap, unwrap, pass as parameters, compare, and re-wrap arithmetic.
#[test]
fn wrap_unwrap_roundtrip() {
    let src = r#"
newtype UserId = int;
class Program {
    static int Bump(UserId id) {
        return id.Value + 1;
    }
    static int Main() {
        UserId a = UserId(41);
        UserId b = UserId(41);
        int total = Bump(a);
        if (a == b) { total = total + 100; }
        UserId c = UserId(a.Value * 2);
        return total + c.Value;
    }
}
"#;
    // Bump(41) = 42, +100 (equal), + 82 = 224.
    assert_parity(src, 224);
}

/// The point of the feature: no implicit conversion in any direction, and
/// distinct newtypes over the same primitive do not interconvert.
#[test]
fn distinctness_is_enforced() {
    let cases: &[(&str, &str)] = &[
        // underlying -> newtype
        (
            r#"
newtype UserId = int;
class Program { static void Main() { UserId id = 42; } }
"#,
            "cannot assign",
        ),
        // newtype -> underlying
        (
            r#"
newtype UserId = int;
class Program { static void Main() { int x = UserId(1); } }
"#,
            "cannot assign",
        ),
        // cross-newtype over the same primitive
        (
            r#"
newtype UserId = int;
newtype OrderId = int;
class Program { static void Main() { UserId a = OrderId(1); } }
"#,
            "cannot assign",
        ),
        // arithmetic without unwrapping
        (
            r#"
newtype UserId = int;
class Program { static void Main() { int y = UserId(1) + 1; } }
"#,
            "cannot operate",
        ),
        // cross-newtype comparison
        (
            r#"
newtype UserId = int;
newtype OrderId = int;
class Program { static void Main() { bool b = UserId(1) == OrderId(1); } }
"#,
            "cannot compare",
        ),
        // string methods do not leak through a string-backed newtype
        (
            r#"
newtype Email = string;
class Program { static void Main() { int n = Email("x").Length; } }
"#,
            "only `Value` is available",
        ),
        // wrong constructor argument type
        (
            r#"
newtype UserId = int;
class Program { static void Main() { UserId id = UserId("no"); } }
"#,
            "cannot construct",
        ),
        // newtype must wrap a primitive
        (
            r#"
class Box { int v; }
newtype Handle = Box;
class Program { static void Main() { } }
"#,
            "must wrap a primitive",
        ),
        // newtype of newtype is rejected (underlying must be primitive)
        (
            r#"
newtype A = int;
newtype B = A;
class Program { static void Main() { } }
"#,
            "must wrap a primitive",
        ),
    ];
    for (src, needle) in cases {
        assert_compile_fails(src, needle);
    }
}

/// Newtypes work as collection element and key types (erased to the
/// underlying primitive, so hashing/equality follow it).
#[test]
fn newtypes_in_collections() {
    let src = r#"
newtype UserId = int;
class Program {
    static int Main() {
        Map<UserId, int> scores = new Map<UserId, int>();
        List<UserId> order = new List<UserId>();
        int i = 0;
        while (i < 50) {
            UserId id = UserId(i * 7);
            scores.Put(id, i);
            order.Add(id);
            i = i + 1;
        }
        int total = 0;
        for (UserId id in order) {
            total = total + scores.Get(id) + id.Value;
        }
        if (scores.ContainsKey(UserId(7))) { total = total + 1000; }
        return total;
    }
}
"#;
    // sum(i) + sum(7i) for i in 0..49 = 1225 + 8575 = 9800; +1000 = 10800.
    assert_parity(src, 10800);
}

/// Nullable newtype (`UserId?`) composes with narrowing and `!`.
#[test]
fn nullable_newtype_narrows() {
    let src = r#"
newtype UserId = int;
class Program {
    static UserId? Find(int key) {
        if (key > 0) { return UserId(key); }
        return null;
    }
    static int Main() {
        UserId? found = Find(5);
        int total = 0;
        if (found != null) {
            total = total + found.Value;
        }
        UserId? missing = Find(-1);
        if (missing == null) {
            total = total + 100;
        }
        return total + Find(2)!.Value;
    }
}
"#;
    assert_parity(src, 107);
}

/// Erasure means zero runtime cost: a hot loop over newtype values must
/// compile to the same JIT-friendly primitive code. 3 trials, mid-run tier
/// transition, compiled count asserted.
#[test]
fn newtype_erasure_in_jit_hot_loop_trials() {
    let src = r#"
newtype Meters = int;
class Program {
    static Meters Add(Meters a, Meters b) {
        return Meters(a.Value + b.Value);
    }
    static int Main() {
        Meters total = Meters(0);
        int i = 0;
        while (i < 500) {
            total = Add(total, Meters(i));
            i = i + 1;
        }
        return total.Value;
    }
}
"#;
    // sum(0..499) = 124750.
    for trial in 0..3 {
        assert_parity(src, 124750);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(100);
        match vm.run().expect("jit threshold-100 run") {
            Value::Int(i) => assert_eq!(i, 124750, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
