//! Typed holes: `_` in expression position is always a compile error, but
//! one that reports the type expected at that position, the in-scope
//! values that fit, and a construction suggestion where the type has an
//! obvious one. Positions with a known expected type: declaration
//! initializers, assignments, return values, and call arguments; anywhere
//! else gets an honest "cannot determine the expected type" message.

use aura_compiler::compile;

fn error_of(source: &str) -> String {
    match compile(source, "test") {
        Ok(_) => panic!("expected a compile error"),
        Err(e) => format!("{e}"),
    }
}

fn assert_error_has(source: &str, needles: &[&str]) {
    let msg = error_of(source);
    for needle in needles {
        assert!(
            msg.contains(needle),
            "expected error containing `{needle}`, got: {msg}"
        );
    }
}

/// Declaration initializers report the declared type and matching locals
/// (and only matching ones — the string local must not be suggested).
#[test]
fn hole_in_initializer_lists_candidates() {
    let msg = error_of(
        "class Program { static void Main() { int a = 3; int b = 7; string s = \"x\"; int c = _; } }",
    );
    for needle in ["type hole", "int32 is needed", "`a` (int32)", "`b` (int32)"] {
        assert!(msg.contains(needle), "missing `{needle}` in: {msg}");
    }
    assert!(!msg.contains("`s`"), "string local wrongly suggested: {msg}");
}

/// Arguments and returns use their exact expected types.
#[test]
fn hole_in_argument_and_return() {
    assert_error_has(
        "class Util { static int Twice(int v) { return v * 2; } }\nclass Program { static void Main() { int n = 5; int r = Util.Twice(_); } }",
        &["type hole", "int32 is needed", "`n` (int32)"],
    );
    assert_error_has(
        "class Program { static int Calc(int seed) { int acc = seed * 2; return _; } }",
        &["type hole", "int32 is needed", "`acc` (int32)", "`seed` (int32)"],
    );
    // Assignment to an existing local.
    assert_error_has(
        "class Program { static void Main() { string t = \"a\"; t = _; } }",
        &["type hole", "string is needed", "`t` (string)"],
    );
}

/// Type-specific construction suggestions: union members, newtype and
/// class constructors, bool literals.
#[test]
fn hole_construction_suggestions() {
    assert_error_has(
        "type Direction = \"north\" | \"south\";\nclass Program { static void Main() { Direction d = _; } }",
        &["Direction is needed", "members: \"north\" | \"south\""],
    );
    assert_error_has(
        "newtype UserId = int;\nclass Program { static void Main() { UserId u = _; } }",
        &["UserId is needed", "construct one: `UserId(...)`"],
    );
    assert_error_has(
        "class Point { int x; Point(int x) { this.x = x; } }\nclass Program { static void Main() { Point p = _; } }",
        &["Point is needed", "construct one: `new Point(...)`"],
    );
    assert_error_has(
        "class Program { static void Main() { bool b = _; } }",
        &["bool is needed", "`true` / `false`"],
    );
}

/// Positions without a determinable expected type say so honestly, and a
/// `var` hole points at the annotation fix.
#[test]
fn hole_fallbacks() {
    assert_error_has(
        "class Program { static void Main() { print(1 + _); } }",
        &["cannot be determined at this position"],
    );
    assert_error_has(
        "class Program { static void Main() { var v = _; } }",
        &["cannot infer from a hole; annotate the type"],
    );
}

/// `_` used as a declared variable name still parses (write-only discard);
/// a hole never reaches execution.
#[test]
fn underscore_declarations_still_parse() {
    // Declaring `_` is fine; READING it back is a hole, which is the
    // standard discard convention.
    let src = "class Program { static void Main() { int _ = 5; } }";
    aura_compiler::compile(src, "test").expect("discard declaration should compile");
}
