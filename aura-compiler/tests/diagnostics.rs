//! Error-message quality: multiple-error reporting (one error per broken
//! method, checking continues across methods and classes, capped at 20),
//! did-you-mean suggestions for close misspellings of types / methods /
//! fields / variables, targeted parser hints for common newcomer
//! mistakes, and rustc-style source-context rendering via
//! `CompileError::render`.

use aura_compiler::{compile, CompileError};

fn err(source: &str) -> String {
    format!("{}", compile(source, "test").expect_err("should not compile"))
}

/// Several independent mistakes surface in one compile: one per broken
/// method, and later classes are still checked after an earlier one
/// fails.
#[test]
fn multiple_errors_in_one_pass() {
    let src = r#"
class Point {
    int x;
    int y;
    Point(int x, int y) { this.x = x; this.y = y; }
    int norm() { return this.x * this.x + this.y * this.y; }
}
class Program {
    static int helper() {
        int counter = 0;
        contuer = counter + 1;
        return counter;
    }
    static void other(Point p) {
        print(p.z);
    }
    static void Main() {
        Pont p = new Pont(1, 2);
    }
}
"#;
    let msg = err(src);
    assert!(msg.contains("line 11, in `Program.helper`"), "{msg}");
    assert!(msg.contains("line 15, in `Program.other`"), "{msg}");
    assert!(msg.contains("line 18, in `Program.Main`"), "{msg}");
    assert_eq!(
        msg.matches("unknown").count(),
        3,
        "exactly one error per broken method:\n{msg}"
    );
}

/// A single error keeps the exact single-line format older tests pin.
#[test]
fn single_error_format_is_unchanged() {
    let msg = err("class Program { static void Main() { int x = \"s\"; } }");
    assert!(!msg.contains('\n'), "single error stays one line: {msg}");
    assert!(msg.starts_with("type error: "), "{msg}");
}

/// Close misspellings get did-you-mean suggestions; distant names do not.
#[test]
fn did_you_mean_suggestions() {
    let src = r#"
class Point {
    int x;
    Point(int x) { this.x = x; }
    int norm() { return this.x; }
}
class Program {
    static void a(Point p) { int a = p.nrom(); }
    static void b(Point p) { int b = p.y; }
    static void c() {
        int counter = 0;
        contuer = 1;
    }
    static void d() { Pont q = null; }
    static void Main() { }
}
"#;
    let msg = err(src);
    assert!(msg.contains("`nrom` on `Point` — did you mean `norm`?"), "{msg}");
    // `y` is one edit from `x`, so the field suggestion fires too.
    assert!(msg.contains("`y` on `Point` — did you mean `x`?"), "{msg}");
    assert!(
        msg.contains("`contuer` — did you mean `counter`?"),
        "{msg}"
    );
    assert!(msg.contains("`Pont` — did you mean `Point`?"), "{msg}");

    let far = err("class Program { static void Main() { zzzqqq = 1; } }");
    assert!(
        !far.contains("did you mean"),
        "no suggestion for distant names: {far}"
    );
}

/// Common newcomer mistakes at top level get targeted hints instead of
/// the generic expectation message.
#[test]
fn parser_hints_for_common_mistakes() {
    for (src, needle) in [
        ("fn main() {}", "free functions are not supported"),
        ("def main(): pass", "free functions are not supported"),
        ("struct Point { }", "Aura has no `struct`"),
        ("let x = 5;", "top-level variables are not supported"),
    ] {
        let msg = err(src);
        assert!(msg.contains(needle), "for {src:?}: {msg}");
    }
}

/// `render` attaches the offending source line beneath each diagnostic
/// and reports the total count for multi-error output.
#[test]
fn render_shows_source_context() {
    let src = r#"class Program {
    static void a() {
        contuer = 1;
    }
    static void Main() {
        Pont p = null;
    }
}
"#;
    let files = vec![("test".to_string(), src.to_string())];
    let e = compile(src, "test").expect_err("should fail");
    let rendered = e.render(&files);
    assert!(rendered.contains("  3 |         contuer = 1;"), "{rendered}");
    assert!(rendered.contains("^^^^^^^^^^^^"), "{rendered}");
    assert!(rendered.contains("2 errors reported"), "{rendered}");

    // Parse errors attribute context through their file-name prefix.
    let bad = "class Program {\n    static void Main() { int x = ; }\n}\n";
    let files = vec![("test".to_string(), bad.to_string())];
    let e = compile(bad, "test").expect_err("should fail");
    let rendered = e.render(&files);
    assert!(
        rendered.contains("2 |     static void Main() { int x = ; }"),
        "{rendered}"
    );
}

/// Error collection is capped so a badly broken file stays readable.
#[test]
fn error_count_is_capped() {
    let mut src = String::from("class Program {\n");
    for i in 0..30 {
        src.push_str(&format!(
            "    static void m{i}() {{ badname{i} = 1; }}\n"
        ));
    }
    src.push_str("    static void Main() { }\n}\n");
    let msg = err(&src);
    let count = msg.matches("unknown variable").count();
    assert_eq!(count, 20, "capped at 20, got {count}:\n{msg}");
}

/// The structured error type still distinguishes phases.
#[test]
fn error_kinds_preserved() {
    let e = compile("class Program {", "test").expect_err("parse");
    assert!(matches!(e, CompileError::Parse(_)), "{e}");
    let e = compile(
        "class Program { static void Main() { unknownFn(); } }",
        "test",
    )
    .expect_err("type");
    assert!(matches!(e, CompileError::Type(_)), "{e}");
}
