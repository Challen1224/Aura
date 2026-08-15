//! First-class function types and lambdas. `Func<int, int>` / `Action<T>`
//! are structural function types; lambdas (`x => x + 1`, `(int a, int b)
//! => a + b`, block bodies) capture enclosing locals and `this` **by
//! value**. Compilation lifts each lambda to a synthesized static method
//! whose leading parameters are the captures, so lambda bodies JIT like any
//! method; a closure is a heap object pairing the lifted method with its
//! captured values (`NewClosure`), and calls go through `Invoke` — both ops
//! run natively in the interpreter and through helpers in JIT code.

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

/// The core surface: target-typed and self-inferred lambdas, captures,
/// lambdas as arguments and return values, nested lambdas capturing the
/// enclosing lambda's parameter, block bodies, and Func-typed parameters
/// invoked inside methods.
#[test]
fn lambdas_work() {
    let src = r#"
class Program {
    static int apply(Func<int, int> f, int v) { return f(v); }
    static Func<int, int> adder(int n) { return x => x + n; }
    static int Main() {
        Func<int, int> double = x => x * 2;
        int total = double(21);
        var add = (int a, int b) => a + b;
        total = total + add(3, 4) * 100;
        int base = 1000;
        Func<int, int> addBase = x => x + base;
        total = total + addBase(5) - base;
        total = total + Program.apply(x => x + 1, 9) * 1000;
        var add30 = Program.adder(30);
        total = total + add30(12) * 100000;
        Func<int, Func<int, int>> curry = a => (int b) => a * b;
        var times6 = curry(6);
        total = total + times6(7) * 10000000;
        Func<int, int> stepper = x => {
            int acc = x;
            acc = acc + 1;
            return acc * 2;
        };
        total = total + stepper(4) * 1000000;
        return total;
    }
}
"#;
    // 42 + 700 + 5 + 10000 + 4200000 + 420000000 + 10000000 = 434210747.
    assert_parity(src, 434210747);
}

/// Closures over `this` and over heap values: an instance method returns a
/// lambda reading a field through the captured `this`; a captured string
/// and a captured object stay alive (and traced) inside the closure.
#[test]
fn closures_over_this_and_heap() {
    let src = r#"
class Scaler {
    int factor;
    Scaler(int factor) { this.factor = factor; }
    Func<int, int> scale() { return x => x * this.factor; }
}
class Box {
    int v;
    Box(int v) { this.v = v; }
}
class Program {
    static int Main() {
        Scaler s = new Scaler(5);
        var f = s.scale();
        int total = f(8);
        string tag = "abcd";
        Box b = new Box(7);
        Func<int, int> mix = x => x + tag.Length + b.v;
        // Churn the heap so the captured values must survive GC.
        for (var i in 1..=300) {
            string junk = "junk {i}";
            total = total + junk.Length - junk.Length;
        }
        total = total + mix(100) * 100;
        return total;
    }
}
"#;
    // 40 + 111*100 = 11140.
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(2048);
    match vm.run().expect("run") {
        Value::Int(i) => assert_eq!(i, 11140),
        other => panic!("expected int, got {other:?}"),
    }
    assert!(vm.gc_collections() > 0, "expected GC cycles during churn");
    assert_parity(src, 11140);
}

/// Every declaration and use-site rule, pinned.
#[test]
fn lambda_rules_are_enforced() {
    let cases: Vec<(&str, &str)> = vec![
        // By-value capture: assigning to a captured local is rejected.
        (
            "class Program { static void Main() { int c = 1; \
             Action bump = () => { c = c + 1; }; bump(); } }",
            "captures `c` by value",
        ),
        // Untargeted, unannotated lambdas cannot infer.
        (
            "class Program { static void Main() { var f = x => x + 1; } }",
            "cannot infer the type of lambda parameter `x`",
        ),
        // Arity mismatch against the target type.
        (
            "class Program { static void Main() { Func<int,int> f = (a, b) => a + b; } }",
            "lambda has 2 parameter(s) but Func<int32, int32> takes 1",
        ),
        // Body return type checked against the target.
        (
            "class Program { static void Main() { Func<int,int> f = x => \"s\"; } }",
            "return type mismatch",
        ),
        // Nullable function values must be narrowed before calling.
        (
            "class Program { static void Main() { Func<int,int>? f = null; f(1); } }",
            "may be null",
        ),
        // Block-bodied lambdas need a target type.
        (
            "class Program { static void Main() { var f = (int x) => { return x; }; } }",
            "block-bodied lambda needs a target type",
        ),
        // Wrong argument type at an invoke site.
        (
            "class Program { static void Main() { Func<int,int> f = x => x; f(\"s\"); } }",
            "does not fit parameter",
        ),
        // Wrong arity at an invoke site.
        (
            "class Program { static void Main() { Func<int,int> f = x => x; f(1, 2); } }",
            "takes 1 argument(s), got 2",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// `var` infers a full Func type from an annotated lambda, and the
/// inferred closure is a first-class value: reassignable, passable, and
/// callable through a parameter.
#[test]
fn func_values_are_first_class() {
    let src = r#"
class Program {
    static int twice(Func<int, int> f, int v) { return f(f(v)); }
    static int Main() {
        var inc = (int x) => x + 1;
        int total = Program.twice(inc, 40);
        Func<int, int> f = inc;
        f = (int x) => x * 10;
        total = total + f(3) * 100;
        Action<int> sink = x => { int y = x * 2; y = y + 1; };
        sink(5);
        return total;
    }
}
"#;
    // twice(inc, 40) = 42; 30*100 = 3000.
    assert_parity(src, 3042);
}

/// Closure invocation in a JIT hot loop crossing the threshold mid-run:
/// the loop method compiles (Invoke goes through a helper), the lifted
/// lambda body itself becomes hot and compiles too. 3 trials, compiled
/// count asserted, GC churn asserted in the interpreter run.
#[test]
fn lambdas_in_jit_hot_loop_trials() {
    let src = r#"
class Program {
    static int Main() {
        int base = 3;
        Func<int, int> f = x => x * x + base;
        int total = 0;
        for (var i in 1..=200) {
            total = total + f(i);
        }
        return total;
    }
}
"#;
    // sum(i^2) + 200*3 = 2686700 + 600 = 2687300.
    for trial in 0..3 {
        assert_parity(src, 2687300);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.set_gc_threshold(2048);
        match vm.run().expect("gc-churn run") {
            Value::Int(i) => assert_eq!(i, 2687300, "trial {trial} under GC churn"),
            other => panic!("expected int, got {other:?}"),
        }
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 2687300, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
