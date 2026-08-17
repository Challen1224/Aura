//! Operator precedence parsing: expression parsing is one
//! precedence-climbing loop over a table (replacing the old eight-layer
//! recursive-descent chain), and custom operator definitions — new
//! symbols starting with `|`, `&`, or `^` — declared exactly like the
//! built-in overloads (`float operator|>(Vec2 o)`), all sharing one
//! precedence tier (looser than arithmetic, tighter than
//! ranges/comparisons) with left associativity.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::Vm;
use std::sync::Arc;

fn run(source: &str, jit: bool) -> i64 {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(1);
    }
    let result = match vm.run().expect("run") {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    #[cfg(target_arch = "x86_64")]
    if jit {
        assert!(vm.jit_compiled_count() > 0, "jit compiled nothing");
    }
    result
}

fn assert_parity(source: &str, expected: i64) {
    assert_eq!(run(source, false), expected, "interpreter");
    assert_eq!(run(source, true), expected, "jit");
}

/// The precedence-climbing table groups every tier exactly as the old
/// layered chain did: a matrix of mixed expressions with hand-computed
/// values, evaluated on both tiers.
#[test]
fn precedence_table_parity() {
    let src = r#"
class Program {
    static int pad(int i) { return i - i; }
    static int Main() {
        int r = 0;
        for (var i in 1..=50) { r = r + Program.pad(i); }
        // multiplicative over additive
        r = r + (1 + 2 * 3);              // 7
        r = r + (10 - 4 / 2);             // +8 = 15
        r = r + (7 % 3 + 2 * 2);          // +5 = 20
        // relational over equality, both over logical
        bool b1 = 1 + 1 == 2;
        bool b2 = 2 * 3 > 5 && 1 < 2;
        bool b3 = false || 3 - 1 == 2;
        if (b1) { r = r + 100; }          // 120
        if (b2) { r = r + 200; }          // 320
        if (b3) { r = r + 400; }          // 720
        // null-coalesce loosest
        int? n = null;
        int v = n ?? 2 + 3;               // 5
        r = r + v;                        // 725
        // ternary above the binary table
        r = r + (1 < 2 ? 10 : 20);        // 735
        // ranges bind looser than additive: 1..2+1 is 1..3
        int c = 0;
        for (var i in 1..2+1) { c = c + 1; }
        r = r + c * 1000;                 // 2735
        return r;
    }
}
"#;
    assert_parity(src, 2735);
}

/// Custom operators: declaration like any overload, resolution to the
/// left operand's method, chaining (left-assoc), and the documented
/// precedence tier — looser than arithmetic, tighter than comparison.
#[test]
fn custom_operators_end_to_end() {
    let src = r#"
class Vec2 {
    int x;
    int y;
    Vec2(int x, int y) { this.x = x; this.y = y; }
    Vec2 operator+(Vec2 o) { return new Vec2(this.x + o.x, this.y + o.y); }
    int operator|>(Vec2 o) { return this.x * o.x + this.y * o.y; }
    Vec2 operator^^(int s) { return new Vec2(this.x * s, this.y * s); }
}
class Acc {
    int total;
    Acc(int total) { this.total = total; }
    Acc operator&+(int step) { return new Acc(this.total * 10 + step); }
}
class Program {
    static int pad(int i) { return i - i; }
    static int Main() {
        int r = 0;
        for (var i in 1..=50) { r = r + Program.pad(i); }
        Vec2 a = new Vec2(1, 2);
        Vec2 b = new Vec2(3, 4);
        // custom binds looser than `+`: (a + b) |> (b ^^ 2)
        int dot = a + b |> (b ^^ 2);      // (4,6)·(6,8) = 72
        r = r + dot;
        // left-assoc chaining: ((1 &+ 2) &+ 3) &+ 4
        Acc acc = new Acc(1) &+ 2 &+ 3 &+ 4;
        r = r + acc.total;                // 72 + 1234 = 1306
        // custom binds tighter than comparison
        bool hit = a |> b == 11;          // (a|>b) == 11 -> true
        if (hit) { r = r + 10000; }       // 11306
        return r;
    }
}
"#;
    assert_parity(src, 11306);
}

/// Diagnostics: undeclared custom operators, reserved symbols, and
/// range chaining all fail with clear messages.
#[test]
fn custom_operator_errors() {
    let undeclared = compile(
        "class Program { static int Main() { return 1 |> 2; } }",
        "test",
    )
    .expect_err("undeclared");
    let msg = format!("{undeclared}");
    assert!(msg.contains("no `operator|>` declared on `int"), "{msg}");

    let ne = compile(
        "class A { A operator!=(A o) { return o; } }",
        "test",
    )
    .expect_err("operator!= is derived");
    assert!(format!("{ne}").contains("operator!="), "{ne}");

    let chained = compile(
        "class Program { static void Main() { var r = 1..2..3; } }",
        "test",
    )
    .expect_err("range chain");
    assert!(
        format!("{chained}").contains("range expressions cannot be chained"),
        "{chained}"
    );

    // Static custom operators are rejected by the existing overload rule.
    let stat = compile(
        "class A { static A operator|>(A o) { return o; } }",
        "test",
    )
    .expect_err("static operator");
    assert!(format!("{stat}").contains("cannot be static"), "{stat}");
}

/// The custom-operator lexer rule leaves every existing use of `|`, `&&`
/// and `||` untouched: literal-union types and logical operators still
/// parse, including with no surrounding whitespace.
#[test]
fn lexing_stays_compatible() {
    let src = r#"
type Mode = "fast" | "slow";
class Program {
    static int Main() {
        Mode m = "fast";
        bool a = true&&false;
        bool b = true||false;
        int r = 0;
        if (m == "fast" && !a && b) { r = 7; }
        return r;
    }
}
"#;
    assert_parity(src, 7);
}
