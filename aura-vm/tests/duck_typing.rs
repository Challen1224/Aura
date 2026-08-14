//! Structural interface satisfaction (duck typing): a class with the right
//! method shapes satisfies an interface without declaring it. Covers the
//! satisfaction rules, near-miss rejections, default-method inheritance
//! through the recorded interface link, runtime agreement (catch matching),
//! and interpreter/JIT parity in hot loops.

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

/// Core satisfaction: assignment, argument passing, and dispatch through a
/// structurally-typed interface reference — no `: IFace` anywhere.
#[test]
fn structural_satisfaction_and_dispatch() {
    let src = r#"
interface Scorer {
    int Score(int x);
}
class Doubler {
    int Score(int x) { return x * 2; }
}
class Tripler {
    int Score(int x) { return x * 3; }
}
class Program {
    static int Apply(Scorer s, int x) { return s.Score(x); }
    static int Main() {
        Scorer a = new Doubler();
        Scorer b = new Tripler();
        return Apply(a, 10) + Apply(b, 10) + a.Score(1);
    }
}
"#;
    assert_parity(src, 52);
}

/// Interface default methods are inherited by structural implementers via
/// the emitter-recorded interface link.
#[test]
fn default_methods_reach_structural_implementers() {
    let src = r#"
interface Describable {
    int Id();
    int Describe() { return this.Id() + 1000; }
}
class Widget {
    int Id() { return 7; }
}
class Program {
    static int Main() {
        Describable d = new Widget();
        return d.Describe();
    }
}
"#;
    assert_parity(src, 1007);
}

/// Satisfying an interface requires the full extends-closure.
#[test]
fn extends_closure_required() {
    let good = r#"
interface Named { int NameLen(); }
interface Labeled : Named { int Label(); }
class Tag {
    int NameLen() { return 3; }
    int Label() { return 9; }
}
class Program {
    static int Main() {
        Labeled l = new Tag();
        return l.Label() + l.NameLen();
    }
}
"#;
    assert_parity(good, 12);

    let bad = r#"
interface Named { int NameLen(); }
interface Labeled : Named { int Label(); }
class Tag {
    int Label() { return 9; }
}
class Program {
    static int Main() {
        Labeled l = new Tag();
        return 0;
    }
}
"#;
    assert_compile_fails(bad, "cannot assign");
}

/// Near misses must not satisfy: missing method, wrong parameter type,
/// wrong return type, static instead of instance, non-public method, and
/// generic interfaces (excluded from structural matching).
#[test]
fn near_misses_are_rejected() {
    let cases: &[(&str, &str)] = &[
        (
            r#"
interface I { int F(int x); }
class C { int G(int x) { return x; } }
class Program { static int Main() { I i = new C(); return 0; } }
"#,
            "cannot assign",
        ),
        (
            r#"
interface I { int F(int x); }
class C { int F(string x) { return 1; } }
class Program { static int Main() { I i = new C(); return 0; } }
"#,
            "cannot assign",
        ),
        (
            r#"
interface I { int F(int x); }
class C { string F(int x) { return "no"; } }
class Program { static int Main() { I i = new C(); return 0; } }
"#,
            "cannot assign",
        ),
        (
            r#"
interface I { int F(int x); }
class C { static int F(int x) { return x; } }
class Program { static int Main() { I i = new C(); return 0; } }
"#,
            "cannot assign",
        ),
        (
            r#"
interface I { int F(int x); }
class C { private int F(int x) { return x; } }
class Program { static int Main() { I i = new C(); return 0; } }
"#,
            "cannot assign",
        ),
        (
            r#"
interface Box<T> { T Get(); }
class C { int Get() { return 1; } }
class Program { static int Main() { Box<int> b = new C(); return 0; } }
"#,
            "cannot assign",
        ),
    ];
    for (src, needle) in cases {
        assert_compile_fails(src, needle);
    }
}

/// Documented consequence of the rules: an interface with only default
/// methods has no abstract requirements, so any class satisfies it.
#[test]
fn default_only_interface_is_satisfied_vacuously() {
    let src = r#"
interface Pingable {
    int Ping() { return 42; }
}
class Anything { int Unrelated() { return 0; } }
class Program {
    static int Main() {
        Pingable p = new Anything();
        return p.Ping();
    }
}
"#;
    assert_parity(src, 42);
}

/// Runtime agreement: catch-by-interface must catch a thrown class that
/// only structurally satisfies the interface, because the emitter records
/// the implementation into the class's runtime metadata.
#[test]
fn catch_by_structural_interface() {
    let src = r#"
interface AppError {
    int Code();
}
class TimeoutError {
    int Code() { return 408; }
}
class Program {
    static int Main() {
        try {
            throw new TimeoutError();
        } catch (AppError e) {
            return e.Code();
        }
        return -1;
    }
}
"#;
    assert_parity(src, 408);
}

/// Structural dispatch inside a method crossing the JIT threshold mid-run;
/// heterogeneous receivers keep virtual dispatch honest. 3 trials.
#[test]
fn structural_dispatch_in_jit_hot_loop_trials() {
    let src = r#"
interface Scorer {
    int Score(int x);
}
class Doubler {
    int Score(int x) { return x * 2; }
}
class Adder {
    int Score(int x) { return x + 100; }
}
class Program {
    static int Apply(Scorer s, int x) { return s.Score(x); }
    static int Main() {
        List<Scorer> scorers = new List<Scorer>();
        scorers.Add(new Doubler());
        scorers.Add(new Adder());
        int total = 0;
        int round = 0;
        while (round < 80) {
            for (Scorer s in scorers) {
                total = total + Apply(s, round);
            }
            round = round + 1;
        }
        return total;
    }
}
"#;
    // sum over rounds 0..79 of (2r) + (r+100) = 3*3160 + 80*100 = 17480.
    for trial in 0..3 {
        assert_parity(src, 17480);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(30);
        match vm.run().expect("jit threshold-30 run") {
            Value::Int(i) => assert_eq!(i, 17480, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}