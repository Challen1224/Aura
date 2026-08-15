//! Generic constraints: `where` clauses with three constraint forms —
//! subtype bounds (`where T : IComparable`), unions (`where T : int |
//! float`), and `new()` (default-constructible). Inline forms
//! (`<T : int | float>`) parse the same constraints. Bounds gate member
//! access (existing bounded polymorphism); all-numeric unions additionally
//! license arithmetic and ordering between two `T` values (the runtime ops
//! are dynamic); `new()` is enforced at instantiation and call sites —
//! `new T()` itself needs reified generics and is rejected with a targeted
//! error.

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

/// All three constraint forms working together under both tiers: a bound
/// gating member access, numeric unions on classes and methods (int and
/// float instantiations, arithmetic + ordering in bodies), `new()`
/// accepting both a ctor-less class and one with a parameterless ctor, and
/// the inline union form.
#[test]
fn constraint_forms_work() {
    let src = r#"
interface IShow {
    int show();
}
class Badge : IShow {
    int n;
    Badge(int n) { this.n = n; }
    int show() { return this.n; }
}
class Box<T> where T : IShow {
    T item;
    Box(T item) { this.item = item; }
    int reveal() { return this.item.show(); }
}
class Pair<T> where T : int | float {
    T a;
    T b;
    Pair(T a, T b) { this.a = a; this.b = b; }
    T sum() { return this.a + this.b; }
    T span() { return this.b - this.a; }
    bool ordered() { return this.a < this.b; }
}
class Inline<T : int | float> {
    T v;
    Inline(T v) { this.v = v; }
    T twice() { return this.v + this.v; }
}
class NoCtor {
    int v;
}
class ZeroCtor {
    int v;
    ZeroCtor() { this.v = 5; }
}
class Reg<T> where T : new() {
    int tag;
    Reg(int tag) { this.tag = tag; }
}
class Program {
    static <T> T pick(T a, T b, bool first) where T : int | float {
        if (first) { return a; }
        return b;
    }
    static int Main() {
        int total = new Box<Badge>(new Badge(7)).reveal();
        Pair<int> pi = new Pair<int>(3, 9);
        total = total + pi.sum() * 10;
        if (pi.ordered()) { total = total + 1000; }
        Pair<float> pf = new Pair<float>(1.5, 4.0);
        if (pf.sum() > pf.span()) { total = total + 10000; }
        total = total + new Inline<int>(4).twice() * 100000;
        total = total + new Reg<NoCtor>(1).tag + new Reg<ZeroCtor>(2).tag;
        total = total + Program.pick(30, 40, true);
        return total;
    }
}
"#;
    // 7 + 120 + 1000 + 10000 + 800000 + 3 + 30 = 811160.
    assert_parity(src, 811160);
}

/// Every constraint-violation and syntax rule, pinned.
#[test]
fn constraint_rules_are_enforced() {
    let pre = "interface IShow { int show(); } \
               class NoShow { int v; } \
               class Box<T> where T : IShow { T x; Box(T x) { this.x = x; } } \
               class Pair<T> where T : int | float { T a; Pair(T a) { this.a = a; } } \
               class Reg<T> where T : new() { int t; Reg(int t) { this.t = t; } } \
               class OnlyCtor { int v; OnlyCtor(int v) { this.v = v; } } \
               abstract class Abs { } \
               class Util { static <T> T pick(T a, T b) where T : int | float { return a; } }";
    let main = |body: &str| format!("{pre} class Program {{ static void Main() {{ {body} }} }}");
    let cases: Vec<(String, &str)> = vec![
        (main("Box<NoShow> b = null;"), "does not satisfy constraint IShow"),
        (main("Pair<string> p = null;"), "must be one of int32 | float64"),
        (main("Reg<OnlyCtor> r = null;"), "does not satisfy `new()`"),
        (main("Reg<Abs> r = null;"), "does not satisfy `new()`"),
        (main("Reg<int> r = null;"), "does not satisfy `new()`"),
        (
            main("string s = Util.pick(\"a\", \"b\");"),
            "inferred type string for `T` in `pick` must be one of int32 | float64",
        ),
        // `new T()` needs reified generics.
        (
            format!(
                "{pre} class Program {{ static <U> U mk() where U : new() {{ return new U(); }} \
                 static void Main() {{ }} }}"
            ),
            "cannot construct type parameter `U`",
        ),
        // `where` must name a declared parameter.
        (
            "class C<T> where U : int { int v; } class Program { static void Main() { } }"
                .to_string(),
            "no type parameter of that name is declared",
        ),
        // Unions stand alone: a bound cannot join one. (Message changed
        // when multiple bounds landed; the rejection is unchanged.)
        (
            "class C<T : int | float> where T : IShow { int v; } \
             interface IShow { int show(); } class Program { static void Main() { } }"
                .to_string(),
            "cannot be combined with other constraints",
        ),
        // Arithmetic on T needs a numeric union...
        (
            format!(
                "{pre} class Bad<T> {{ T a; Bad(T a) {{ this.a = a; }} \
                 T dbl() {{ return this.a + this.a; }} }} \
                 class Program {{ static void Main() {{ }} }}"
            ),
            "cannot apply `+` to type parameter `T`",
        ),
        // ...and both operands must be the same T.
        (
            format!(
                "{pre} class Bad<T> where T : int | float {{ T a; Bad(T a) {{ this.a = a; }} \
                 T off() {{ return this.a + 1; }} }} \
                 class Program {{ static void Main() {{ }} }}"
            ),
            "use two `T` operands",
        ),
        // Ordering on T likewise.
        (
            format!(
                "{pre} class Bad<T> {{ T a; Bad(T a) {{ this.a = a; }} \
                 bool lt(T o) {{ return this.a < o; }} }} \
                 class Program {{ static void Main() {{ }} }}"
            ),
            "cannot order type parameter `T`",
        ),
        // Unions of non-numeric alternatives give no arithmetic license.
        (
            format!(
                "{pre} class Bad<T> where T : int | string {{ T a; Bad(T a) {{ this.a = a; }} \
                 T dbl() {{ return this.a + this.a; }} }} \
                 class Program {{ static void Main() {{ }} }}"
            ),
            "cannot apply `+` to type parameter `T`",
        ),
        // `new()` cannot join a union.
        (
            "class C<T> where T : int | float, new() { int v; } class Program { static void Main() { } }"
                .to_string(),
            "`new()` cannot be combined",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// Union-constrained arithmetic in a JIT hot loop crossing the threshold
/// mid-run, instantiated at both int and float. 3 trials, compiled count
/// asserted.
#[test]
fn union_arithmetic_in_jit_hot_loop_trials() {
    let src = r#"
class Acc<T> where T : int | float {
    T total;
    Acc(T zero) { this.total = zero; }
    void add(T v) { this.total = this.total + v; }
    bool below(T cap) { return this.total < cap; }
}
class Program {
    static int Main() {
        Acc<int> ints = new Acc<int>(0);
        int i = 1;
        while (i <= 200 && ints.below(1000000)) {
            ints.add(i * i);
            i = i + 1;
        }
        Acc<float> floats = new Acc<float>(0.0);
        float f = 0.5;
        int k = 0;
        while (k < 100) {
            floats.add(f);
            k = k + 1;
        }
        int fl = 0;
        if (floats.below(51.0)) { fl = 1; }
        return ints.total * 10 + fl;
    }
}
"#;
    // The cap binds first: sum(i^2) passes 1_000_000 at i=144
    // (984_984 + 144^2 = 1_005_720), so the loop stops there.
    for trial in 0..3 {
        assert_parity(src, 10057201);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 10057201, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
