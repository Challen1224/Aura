//! Multi-file compilation and module-scoped `internal`. Each source file is
//! a module; declarations share one flat namespace (no imports), and
//! `internal` members are only accessible from classes declared in the same
//! file. Single-file programs put everything in one module, so `internal`
//! keeps its previous file-wide behavior there.

use aura_bytecode::Value;
use aura_compiler::{compile_files, CompileError};
use aura_vm::{Vm, VmError};
use std::sync::Arc;

fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect()
}

fn run_files(list: &[(&str, &str)], jit: bool) -> Result<i64, VmError> {
    let module = Arc::new(compile_files(&files(list), "test").expect("compile"));
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

fn assert_parity(list: &[(&str, &str)], expected: i64) {
    let interp = run_files(list, false).expect("interpreter should run");
    assert_eq!(interp, expected, "interpreter result");
    let jit = run_files(list, true).expect("jit should run");
    assert_eq!(jit, interp, "jit must match interpreter");
}

fn assert_compile_fails(list: &[(&str, &str)], needle: &str) {
    match compile_files(&files(list), "test") {
        Ok(_) => panic!("expected compile error containing `{needle}`"),
        Err(e) => {
            let msg = match e {
                CompileError::Lex(m) | CompileError::Parse(m) | CompileError::Type(m) | CompileError::Emit(m) => m,
            };
            assert!(
                msg.contains(needle),
                "expected error containing `{needle}`, got: {msg}"
            );
        }
    }
}

const GEOMETRY: &str = r#"
class Rect {
    internal int w;
    internal int h;
    Rect(int w, int h) { this.w = w; this.h = h; }
    int Area() { return GeoUtil.Mul(this.w, this.h); }
}
class GeoUtil {
    internal static int Mul(int a, int b) { return a * b; }
}
"#;

/// Public members work across files; internal members work within their
/// own file (Rect uses GeoUtil.Mul and its own internal fields).
#[test]
fn cross_file_public_and_same_module_internal() {
    let main = r#"
class Program {
    static int Main() {
        Rect r = new Rect(3, 4);
        return r.Area();
    }
}
"#;
    assert_parity(&[("main", main), ("geometry", GEOMETRY)], 12);
}

/// The access matrix: internal fields, instance state, and static methods
/// are all rejected across modules; public stays open.
#[test]
fn cross_module_internal_is_rejected() {
    assert_compile_fails(
        &[
            (
                "main",
                r#"
class Program {
    static int Main() {
        Rect r = new Rect(1, 2);
        return r.w;
    }
}
"#,
            ),
            ("geometry", GEOMETRY),
        ],
        "field `w` on `Rect` is internal",
    );
    assert_compile_fails(
        &[
            (
                "main",
                r#"
class Program {
    static int Main() {
        return GeoUtil.Mul(2, 3);
    }
}
"#,
            ),
            ("geometry", GEOMETRY),
        ],
        "static method `Mul` on `GeoUtil` is internal",
    );
    assert_compile_fails(
        &[
            (
                "main",
                r#"
class Program {
    static int Main() {
        Helper h = new Helper();
        return h.Secret();
    }
}
"#,
            ),
            (
                "lib",
                r#"
class Helper {
    internal int Secret() { return 1; }
}
"#,
            ),
        ],
        "method `Secret` on `Helper` is internal",
    );
}

/// Single-file programs keep the old semantics: internal is accessible
/// everywhere in the file.
#[test]
fn single_file_internal_unchanged() {
    let src = r#"
class Vault {
    internal int code;
    Vault(int c) { this.code = c; }
}
class Program {
    static int Main() {
        Vault v = new Vault(77);
        return v.code;
    }
}
"#;
    assert_parity(&[("only", src)], 77);
}

/// Language features cross file boundaries: inheritance, duck typing,
/// literal unions, newtypes, and generics all resolve across modules.
#[test]
fn features_cross_files() {
    let shapes = r#"
type Kind = "circle" | "square";
newtype Id = int;
interface Shape { int Area(); }
class Circle {
    int r;
    Circle(int r) { this.r = r; }
    int Area() { return 3 * this.r * this.r; }
}
"#;
    let main = r#"
class Program {
    static <T : Shape> int Measure(T s) { return s.Area(); }
    static int Main() {
        Kind k = "circle";
        Id id = Id(9);
        int total = Measure(new Circle(2));
        if (k == "circle") { total = total + 100; }
        return total + id.Value;
    }
}
"#;
    // 12 + 100 + 9 = 121.
    assert_parity(&[("main", main), ("shapes", shapes)], 121);
}

/// Multi-file programs in a JIT hot loop crossing the threshold mid-run,
/// with cross-module calls in the hot path. 3 trials.
#[test]
fn multifile_jit_hot_loop_trials() {
    let lib = r#"
class Math2 {
    static int Double(int v) { return v * 2; }
}
"#;
    let main = r#"
class Program {
    static int Main() {
        int total = 0;
        for (var i in 1..=300) {
            total = total + Math2.Double(i);
        }
        return total;
    }
}
"#;
    // 2 * sum(1..300) = 90300.
    for trial in 0..3 {
        assert_parity(&[("main", main), ("lib", lib)], 90300);
        let module = Arc::new(
            compile_files(&files(&[("main", main), ("lib", lib)]), "test").expect("compile"),
        );
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(100);
        match vm.run().expect("jit threshold-100 run") {
            Value::Int(i) => assert_eq!(i, 90300, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
