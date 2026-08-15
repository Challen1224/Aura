//! Operator overloading: `Vector operator+(Vector other) { ... }` declares
//! an instance method named `operator+`; `a + b` lowers to a `CallVirt` on
//! the left operand (arguments before receiver, with a temp preserving
//! left-to-right evaluation). `!=` is always the negation of `operator==`;
//! `&&`/`||` are never overloadable. Comparison operators must return bool.

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

/// Arithmetic operators with precedence, chaining, a scalar (non-class)
/// right operand, and field access on an operator result.
#[test]
fn vector_arithmetic_works() {
    let src = r#"
class Vec {
    int x;
    int y;
    Vec(int x, int y) { this.x = x; this.y = y; }
    Vec operator+(Vec other) { return new Vec(this.x + other.x, this.y + other.y); }
    Vec operator-(Vec other) { return new Vec(this.x - other.x, this.y - other.y); }
    Vec operator*(int s) { return new Vec(this.x * s, this.y * s); }
}
class Program {
    static int Main() {
        Vec a = new Vec(1, 2);
        Vec b = new Vec(30, 40);
        Vec c = a + b * 2;
        Vec d = (c - a) * 10;
        return d.x * 100000 + d.y + (a + b).y;
    }
}
"#;
    // c = (61, 82); d = (600, 800); 600*100000 + 800 + 42 = 60000842.
    assert_parity(src, 60000842);
}

/// `==`, derived `!=`, and the four ordering operators; a class without
/// `operator==` keeps reference equality.
#[test]
fn equality_and_ordering_overloads() {
    let src = r#"
class Money {
    int cents;
    Money(int cents) { this.cents = cents; }
    bool operator==(Money other) { return this.cents == other.cents; }
    bool operator<(Money other) { return this.cents < other.cents; }
    bool operator<=(Money other) { return this.cents <= other.cents; }
    bool operator>(Money other) { return this.cents > other.cents; }
    bool operator>=(Money other) { return this.cents >= other.cents; }
}
class Plain {
    int v;
    Plain(int v) { this.v = v; }
}
class Program {
    static int Main() {
        Money a = new Money(100);
        Money b = new Money(100);
        Money c = new Money(250);
        int total = 0;
        if (a == b) { total = total + 1; }
        if (a != c) { total = total + 10; }
        if (a < c) { total = total + 100; }
        if (a <= b) { total = total + 1000; }
        if (c > a) { total = total + 10000; }
        if (c >= c) { total = total + 100000; }
        Plain p = new Plain(7);
        Plain q = new Plain(7);
        Plain r = p;
        if (p == q) { total = total + 1000000; }
        if (p == r) { total = total + 10000000; }
        return total;
    }
}
"#;
    // All Money checks pass; p==q is reference equality (false), p==r true.
    assert_parity(src, 10111111);
}

/// Operators on generic receivers substitute the receiver's type arguments
/// (`Box<string> + Box<string>` returns `string`), operators are inherited
/// by subclasses, and operands evaluate left to right.
#[test]
fn generics_inheritance_and_evaluation_order() {
    let src = r#"
class Box<T> {
    T v;
    int rank;
    Box(T v, int rank) { this.v = v; this.rank = rank; }
    T operator+(Box<T> other) {
        if (this.rank >= other.rank) { return this.v; }
        return other.v;
    }
}
class Base {
    int n;
    Base(int n) { this.n = n; }
    int operator*(Base other) { return this.n * other.n; }
}
class Derived : Base {
    Derived(int n) : super(n) { }
}
class Program {
    static int order = 0;
    static Base Mk(int n) {
        Program.order = Program.order * 10 + n;
        return new Base(n);
    }
    static int Main() {
        Box<string> a = new Box<string>("low", 1);
        Box<string> b = new Box<string>("high", 9);
        string winner = a + b;
        Derived d = new Derived(6);
        Derived e = new Derived(7);
        int viaInherited = d * e;
        int product = Program.Mk(3) * Program.Mk(4);
        return winner.Length * 1000000 + viaInherited * 1000 + product * 10 + Program.order;
    }
}
"#;
    // "high".Length=4; 6*7=42; 3*4=12; order must be 34 (left first).
    assert_parity(src, 4042154);
}

/// Every declaration and use-site rule, pinned.
#[test]
fn operator_rules_are_enforced() {
    let decl = |body: &str| {
        format!(
            "class V {{ int n; V(int n) {{ this.n = n; }} {} }} \
             class Program {{ static void Main() {{ }} }}",
            body
        )
    };
    let cases: Vec<(String, &str)> = vec![
        (decl("static V operator+(V o) { return o; }"), "cannot be static"),
        (decl("virtual V operator+(V o) { return o; }"), "cannot be virtual"),
        (decl("private bool operator==(V o) { return true; }"), "must be public"),
        (decl("<T> V operator+(V o) { return o; }"), "cannot have generic parameters"),
        (decl("V operator+(V a, V b) { return a; }"), "must take exactly one parameter"),
        (decl("void operator+(V o) { }"), "must return a value"),
        (decl("int operator==(V o) { return 1; }"), "must return bool"),
        (decl("V operator<(V o) { return o; }"), "must return bool"),
        (
            decl("bool operator!=(V o) { return false; }"),
            "derived as its negation",
        ),
        (
            "interface I { bool operator==(I o); } class Program { static void Main() { } }"
                .to_string(),
            "cannot declare `operator==`",
        ),
        (
            decl("bool operator==(V o) { return this.n == o.n; } \
                  static bool Cmp(V a, V b) { return a < b; }"),
            "does not define `operator<`",
        ),
        (
            "class A { int n; A(int n) { this.n = n; } } \
             class Program { static void Main() { A x = new A(1); A y = new A(2); \
             A z = x + y; } }"
                .to_string(),
            "does not define `operator+`",
        ),
        (
            decl("V operator+(V o) { return o; } \
                  static V Bad(V a) { return a + 3; }"),
            "right operand does not fit",
        ),
        (
            decl("V operator+(V o) { return o; } \
                  static V Bad(V? a, V b) { return a + b; }"),
            "may be null",
        ),
        (
            decl("bool operator==(V o) { return this.n == o.n; } \
                  static bool Bad(V? a, V b) { return a == b; }"),
            "may be null",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// The parser regression the feature exposed: `this.n < other.n` — a field
/// access compared against an identifier — used to be mis-guessed as
/// generic type arguments. Both the comparison and real generic method
/// calls must parse.
#[test]
fn field_comparison_and_generic_calls_parse() {
    let src = r#"
class P {
    int n;
    P(int n) { this.n = n; }
    bool Less(P other) { return this.n < other.n; }
}
class Util {
    static <T> T Pick(T a, T b, bool first) {
        if (first) { return a; }
        return b;
    }
}
class Program {
    static int Main() {
        P a = new P(3);
        P b = new P(9);
        int total = 0;
        if (a.Less(b)) { total = total + 1; }
        if (a.n < b.n) { total = total + 10; }
        total = total + Util.Pick<int>(100, 200, true);
        return total;
    }
}
"#;
    assert_parity(src, 111);
}

/// Operator-heavy hot loop crossing the JIT threshold mid-run, allocating a
/// fresh object per iteration (exercises GC under churn in the interpreter
/// run). 3 trials, compiled count asserted.
#[test]
fn operators_in_jit_hot_loop_trials() {
    let src = r#"
class Acc {
    int sum;
    Acc(int sum) { this.sum = sum; }
    Acc operator+(Acc other) { return new Acc(this.sum + other.sum); }
    bool operator<(Acc other) { return this.sum < other.sum; }
}
class Program {
    static int Main() {
        Acc total = new Acc(0);
        Acc limit = new Acc(1000000000);
        int i = 1;
        while (i <= 200 && total < limit) {
            total = total + new Acc(i * i);
            i = i + 1;
        }
        return total.sum;
    }
}
"#;
    // sum(i^2, 1..200) = 2686700.
    for trial in 0..3 {
        assert_parity(src, 2686700);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.set_gc_threshold(2048);
        match vm.run().expect("gc-churn run") {
            Value::Int(i) => assert_eq!(i, 2686700, "trial {trial} under GC churn"),
            other => panic!("expected int, got {other:?}"),
        }
        assert!(vm.gc_collections() > 0, "trial {trial}: expected GC cycles");
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 2686700, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
