//! Generic methods with multiple type parameters, including the C#-style
//! two-pass call-site inference the TODO sketch implies: `transform(21,
//! x => x * 2)` binds `T1` from the non-lambda argument first, the lambda
//! is then target-typed at `Func<int, T2>` with the return left open, and
//! its body determines `T2`. Annotated lambdas and explicit type arguments
//! also work; a lambda that nothing constrains still asks for annotations.

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

/// The TODO sketch end-to-end: multiple type parameters inferred from
/// values, annotated lambdas, unannotated lambdas via two-pass inference
/// (in both directions of the type change), explicit type arguments, and
/// captures inside inferred lambdas.
#[test]
fn multi_param_inference_works() {
    let src = r#"
class Util {
    static <T1, T2> T2 transform(T1 value, Func<T1, T2> converter) {
        return converter(value);
    }
    static <K, V> int weigh(K k, V v, Func<K, int> kw, Func<V, int> vw) {
        return kw(k) + vw(v);
    }
}
class Program {
    static int Main() {
        int total = Util.transform(21, x => x * 2);
        total = total + Util.transform("hello", s => s.Length) * 100;
        total = total + Util.transform(7, (int n) => n + 1) * 1000;
        total = total + Util.transform<int, int>(5, (int n) => n * n) * 10000;
        int bonus = 3;
        total = total + Util.transform(10, n => n + bonus) * 100000;
        total = total + Util.weigh("ab", 40, s => s.Length, n => n + 2) * 10000000;
        return total;
    }
}
"#;
    // 42 + 500 + 8000 + 250000 + 1300000 + 440000000 = 441558542.
    assert_parity(src, 441558542);
}

/// The lambda's own result type flows onward: T2 binds to a string built
/// in the lambda (captures included) and the caller uses it as a string.
#[test]
fn lambda_result_binds_and_flows() {
    let src = r#"
class Util {
    static <T1, T2> T2 transform(T1 value, Func<T1, T2> f) { return f(value); }
}
class Program {
    static int Main() {
        string tail = "!!";
        string s = Util.transform(7, n => "n={n}{tail}");
        return s.Length;
    }
}
"#;
    // "n=7!!" -> 5.
    assert_parity(src, 5);
}

/// Unresolvable inference still asks for annotations, and mismatches
/// against inferred bindings are caught.
#[test]
fn multi_param_inference_rules() {
    let pre = "class U { static <T1, T2> T2 t(T1 v, Func<T1, T2> f) { return f(v); } }";
    let cases: Vec<(String, &str)> = vec![
        // The lambda comes first, so nothing has bound T1 yet.
        (
            format!("{pre} class Program {{ static void Main() {{ var r = U.t(x => x, 1); }} }}"),
            "cannot infer the type of lambda parameter `x`",
        ),
        // Annotated lambda conflicting with the value argument.
        (
            format!(
                "{pre} class Program {{ static void Main() {{ \
                 var r = U.t(1, (string s) => s.Length); }} }}"
            ),
            "conflicting argument types",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// Inferred generic lambdas in a JIT hot loop crossing the threshold
/// mid-run. 3 trials, compiled count asserted.
#[test]
fn multi_param_generics_in_jit_hot_loop_trials() {
    let src = r#"
class Util {
    static <T1, T2> T2 transform(T1 value, Func<T1, T2> f) { return f(value); }
}
class Program {
    static int Main() {
        int total = 0;
        for (var i in 1..=150) {
            total = total + Util.transform(i, x => x * x);
        }
        return total;
    }
}
"#;
    // sum(i^2, 1..150) = 1136275.
    for trial in 0..3 {
        assert_parity(src, 1136275);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 1136275, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
