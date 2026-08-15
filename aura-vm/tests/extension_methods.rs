//! Extension methods: `extension StringExtensions on string { bool
//! isPalindrome() { ... } }` desugars to a static class whose methods take
//! the receiver as a leading `__self` parameter (`this` in bodies).
//! `x.Foo()` resolves to the extension when `x`'s type has no real `Foo` —
//! a real method always wins, and an extension shadowed at declaration time
//! is rejected outright. Targets: `string` or a non-generic user class.

use aura_bytecode::Value;
use aura_compiler::{compile, compile_files};
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

/// The TODO example: string extensions, on locals, literals and inferred
/// receivers, chained with intrinsic methods, plus the desugared static
/// form called directly.
#[test]
fn string_extensions_work() {
    let src = r#"
extension StringExtensions on string {
    bool isPalindrome() {
        int i = 0;
        int j = this.Length - 1;
        while (i < j) {
            if (this.Substring(i, 1) != this.Substring(j, 1)) { return false; }
            i = i + 1;
            j = j - 1;
        }
        return true;
    }
    string doubled() { return "{this}{this}"; }
}
class Program {
    static int Main() {
        int total = 0;
        string s = "racecar";
        if (s.isPalindrome()) { total = total + 1; }
        if ("aura".isPalindrome()) { total = total + 10; }
        var v = "abba";
        if (v.isPalindrome()) { total = total + 100; }
        total = total + "xy".doubled().Length * 1000;
        if (StringExtensions.isPalindrome("noon")) { total = total + 10000; }
        if (s.doubled().isPalindrome()) { total = total + 100000; }
        return total;
    }
}
"#;
    // 1 + 100 + 4000 + 10000 + 100000 ("racecarracecar" reversed is
    // itself, since "racecar" is a palindrome concatenated with itself).
    assert_parity(src, 114101);
}

/// Class extensions: `this` reads fields and calls real methods, extensions
/// call other extensions through `this`, results chain, and an extension on
/// a base class applies to subclass receivers. A real method declared on a
/// subclass wins over a base-class extension.
#[test]
fn class_extensions_work() {
    let src = r#"
class Shape {
    int w;
    int h;
    Shape(int w, int h) { this.w = w; this.h = h; }
    int Area() { return this.w * this.h; }
}
class Square : Shape {
    Square(int side) : super(side, side) { }
    int describe() { return 777; }
}
extension ShapeExtensions on Shape {
    int perimeter() { return 2 * (this.w + this.h); }
    int describe() { return this.Area() * 100 + this.perimeter(); }
    Shape swapped() { return new Shape(this.h, this.w); }
}
class Program {
    static int Main() {
        Shape s = new Shape(3, 4);
        int total = s.describe();
        Shape sq = new Square(5);
        total = total + sq.perimeter();
        Square q = new Square(2);
        total = total + q.describe();
        total = total + s.swapped().perimeter() * 10000;
        return total;
    }
}
"#;
    // s.describe() = 12*100 + 14 = 1214; sq.perimeter() = 20;
    // q.describe() = real method = 777; s.swapped().perimeter() = 14 -> 140000.
    assert_parity(src, 142011);
}

/// Every declaration and use-site rule, pinned.
#[test]
fn extension_rules_are_enforced() {
    let prog = "class Program { static void Main() { } }";
    let point =
        "class Point { int x; Point(int x) { this.x = x; } int norm() { return this.x; } }";
    let cases: Vec<(String, &str)> = vec![
        (
            format!("extension E on Point {{ int v; }} {point} {prog}"),
            "can only contain methods",
        ),
        (
            format!("extension E on Point {{ E() {{ }} }} {point} {prog}"),
            "cannot declare a constructor",
        ),
        (
            format!("extension E on Point {{ static int f() {{ return 1; }} }} {point} {prog}"),
            "cannot be static",
        ),
        (
            format!("extension E on Point {{ virtual int f() {{ return 1; }} }} {point} {prog}"),
            "cannot be virtual",
        ),
        (
            format!("extension E on Point {{ private int f() {{ return 1; }} }} {point} {prog}"),
            "must be public",
        ),
        (
            format!(
                "extension E on Point {{ bool operator==(Point o) {{ return true; }} }} {point} {prog}"
            ),
            "operator overloads must be declared on the class itself",
        ),
        (
            format!("extension E on Missing {{ int f() {{ return 1; }} }} {prog}"),
            "targets unknown type",
        ),
        (
            format!("enum Color {{ Red, Green }} extension E on Color {{ int f() {{ return 1; }} }} {prog}"),
            "extensions on enum",
        ),
        (
            format!("static class M {{ static int X = 1; }} extension E on M {{ int f() {{ return 1; }} }} {prog}"),
            "has no instances",
        ),
        (
            format!("extension E on List<int> {{ int f() {{ return 1; }} }} {prog}"),
            "generic extension targets are not supported yet",
        ),
        (
            format!("extension E on List {{ int f() {{ return 1; }} }} {prog}"),
            "built-in class",
        ),
        (
            format!("extension E on int {{ int f() {{ return 1; }} }} {prog}"),
            "must be `string` or a class",
        ),
        (
            format!("{point} extension E on Point {{ int norm() {{ return 1; }} }} {prog}"),
            "conflicts with an existing method",
        ),
        (
            format!("extension E on string {{ string ToUpper() {{ return \"x\"; }} }} {prog}"),
            "conflicts with a built-in string method",
        ),
        (
            format!(
                "{point} extension A on Point {{ int f() {{ return 1; }} }} \
                 extension B on Point {{ int f() {{ return 2; }} }} {prog}"
            ),
            "duplicate extension method",
        ),
        (
            format!(
                "{point} extension E on Point {{ int f() {{ return 1; }} }} \
                 class Q {{ static int g(Point? p) {{ return p.f(); }} }} {prog}"
            ),
            "may be null",
        ),
        (
            format!("{point} class Q {{ static int g(Point p) {{ return p.nope(); }} }} {prog}"),
            "unknown method `nope`",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
    // The conflict check walks the target's inheritance chain: an extension
    // on a subclass conflicts with a method inherited from its base.
    let inherited_conflict = format!(
        "class B2 {{ int norm() {{ return 1; }} }} class S2 : B2 {{ int y; }} \
         extension E on S2 {{ int norm() {{ return 2; }} }} {prog}"
    );
    assert_compile_fails(&inherited_conflict, "conflicts with an existing method");
}

/// Extensions live in ordinary modules: declared in one file, used from
/// another (flat namespace, `internal` scoping untouched).
#[test]
fn extensions_work_across_files() {
    let files = vec![
        (
            "geometry".to_string(),
            r#"
class Point {
    int x;
    int y;
    Point(int x, int y) { this.x = x; this.y = y; }
}
extension PointExtensions on Point {
    int manhattan() {
        int ax = this.x; if (ax < 0) { ax = 0 - ax; }
        int ay = this.y; if (ay < 0) { ay = 0 - ay; }
        return ax + ay;
    }
}
"#
            .to_string(),
        ),
        (
            "main".to_string(),
            r#"
class Program {
    static int Main() {
        Point p = new Point(-3, 7);
        return p.manhattan();
    }
}
"#
            .to_string(),
        ),
    ];
    let module = Arc::new(compile_files(&files, "test").expect("compile"));
    let mut vm = Vm::new(module);
    match vm.run().expect("run") {
        Value::Int(i) => assert_eq!(i, 10),
        other => panic!("expected int, got {other:?}"),
    }
}

/// Extension calls in a JIT hot loop crossing the threshold mid-run, with
/// object allocation per iteration (GC churn asserted in the interpreter
/// run). 3 trials, compiled count asserted.
#[test]
fn extensions_in_jit_hot_loop_trials() {
    let src = r#"
class Cell {
    int v;
    Cell(int v) { this.v = v; }
}
extension CellExtensions on Cell {
    int tripled() { return this.v * 3; }
    Cell next() { return new Cell(this.v + 1); }
}
extension StringExtensions on string {
    int weight() { return this.Length * 2; }
}
class Program {
    static int Main() {
        Cell c = new Cell(0);
        int total = 0;
        while (c.v < 300) {
            total = total + c.tripled();
            c = c.next();
        }
        return total + "aura".weight();
    }
}
"#;
    // 3 * sum(0..299) = 3 * 44850 = 134550; + 8 = 134558.
    for trial in 0..3 {
        assert_parity(src, 134558);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.set_gc_threshold(2048);
        match vm.run().expect("gc-churn run") {
            Value::Int(i) => assert_eq!(i, 134558, "trial {trial} under GC churn"),
            other => panic!("expected int, got {other:?}"),
        }
        assert!(vm.gc_collections() > 0, "trial {trial}: expected GC cycles");
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 134558, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
