//! Diagnostics: type-mismatch errors carry source locations (`line N, in
//! `Class.Method`:`) and, for known mismatch patterns, a one-line hint with
//! the fix. Location tracking flows from per-token lexer lines through
//! parser-injected `Stmt::Mark` markers; a marker must never change
//! behavior, which the whole existing suite plus the parity case below pin.

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

/// Type errors point at the failing statement's line and enclosing method.
#[test]
fn type_errors_carry_locations() {
    assert_error_has(
        "class Program {\n    static void Main() {\n        int a = 1;\n        int b = 2;\n        int c = \"oops\";\n    }\n}\n",
        &["line 5", "Program.Main", "cannot assign"],
    );
    // A different method name in the context prefix.
    assert_error_has(
        "class Calc {\n    static int Twice(int v) {\n        return \"no\";\n    }\n}\nclass Program { static void Main() { } }\n",
        &["line 3", "Calc.Twice", "return type mismatch"],
    );
}

/// Parse errors report the offending line.
#[test]
fn parse_errors_carry_lines() {
    let msg = error_of("class Program {\n    static void Main() {\n        int a = 1;\n        int b = ;\n    }\n}\n");
    assert!(msg.contains("line 4"), "expected line 4, got: {msg}");
}

/// Emitter-stage errors get the same location prefix.
#[test]
fn emit_errors_carry_locations() {
    // A bare call to an unknown static method survives the typer's
    // current-class fallback and fails in the emitter.
    let msg = error_of(
        "class Program {\n    static void Main() {\n        int a = 1;\n        Missing.Foo();\n    }\n}\n",
    );
    assert!(
        msg.contains("line 4"),
        "expected a line-4 location, got: {msg}"
    );
}

/// Each known mismatch pattern appends its fix as a hint.
#[test]
fn mismatch_hints() {
    assert_error_has(
        "class Program { static void Main() { string? m = Console.ReadLine(); string s = m; } }",
        &["hint:", "may be null", "narrow"],
    );
    assert_error_has(
        "newtype UserId = int;\nclass Program { static void Main() { UserId u = 5; } }",
        &["hint:", "wrap the value: `UserId(...)`"],
    );
    assert_error_has(
        "newtype UserId = int;\nclass Program { static void Main() { UserId u = UserId(1); int x = u; } }",
        &["hint:", "unwrap the value with `.Value`"],
    );
    assert_error_has(
        "type Direction = \"north\" | \"south\";\nclass Program { static void Main() { Direction d = \"nort\"; } }",
        &["hint:", "not a member of `Direction`", "\"north\" | \"south\""],
    );
    assert_error_has(
        "type Direction = \"north\" | \"south\" | \"east\";\ntype Compass = \"north\" | \"south\";\nclass Program { static void Main() { Direction d = \"east\"; Compass c = d; } }",
        &["hint:", "`Direction` can hold \"east\" which `Compass` cannot"],
    );
    assert_error_has(
        "interface Shape { int Area(); }\nclass Blob { int Area(string pad) { return 1; } }\nclass Program { static void Main() { Shape s = new Blob(); } }",
        &["hint:", "`Blob.Area` has signature (string) -> int32", "requires () -> int32"],
    );
    assert_error_has(
        "interface Shape { int Area(); }\nclass Empty { int Other() { return 1; } }\nclass Program { static void Main() { Shape s = new Empty(); } }",
        &["hint:", "missing method `Area () -> int32`"],
    );
    assert_error_has(
        "class Program { static void Main() { int64 big = 5; int small = big; } }",
        &["hint:", "narrowing conversion from int64 to int32"],
    );
    // Hints also fire on argument mismatches, not just assignments.
    assert_error_has(
        "newtype UserId = int;\nclass Util { static int Use(UserId u) { return u.Value; } }\nclass Program { static void Main() { int x = Util.Use(7); } }",
        &["argument type mismatch", "hint:", "wrap the value: `UserId(...)`"],
    );
}

/// Location markers must never change behavior: narrowing across
/// terminating branches (the analysis most sensitive to statement-list
/// shape) still works and matches under the JIT.
#[test]
fn markers_do_not_change_behavior() {
    let src = r#"
class Program {
    static int Grade(string? name) {
        if (name == null) {
            return 0;
        }
        return name.Length;
    }
    static int Main() {
        int total = Grade(null) * 100 + Grade("abcde");
        return total;
    }
}
"#;
    let interp = run(src, false).expect("interpreter should run");
    assert_eq!(interp, 5);
    let jit = run(src, true).expect("jit should run");
    assert_eq!(jit, interp);
}
