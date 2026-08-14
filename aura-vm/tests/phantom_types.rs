//! Phantom types: a generic parameter used only as a compile-time tag.
//! For the idiom to be trustworthy the type system must make tags
//! un-dodgeable: exact type-argument arity (no raw references to generic
//! classes), invariant argument checking, and enforced `<T : IFace>`
//! constraints (previously parsed but ignored). Tags are fully erased at
//! runtime.

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
    let jit = run(source, true).expect("jit must match");
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

/// A file-handle state machine: operations are only available in the
/// right state, transitions consume one tag and produce the other.
const STATE_MACHINE: &str = r#"
interface FileState { int Marker(); }
class Open { int Marker() { return 1; } }
class Closed { int Marker() { return 0; } }

class FileHandle<S : FileState> {
    int fd;
    FileHandle(int fd) { this.fd = fd; }
}

class Files {
    static FileHandle<Open> OpenFile(int fd) {
        return new FileHandle<Open>(fd);
    }
    static int Read(FileHandle<Open> h) {
        return h.fd * 10;
    }
    static FileHandle<Closed> Close(FileHandle<Open> h) {
        return new FileHandle<Closed>(h.fd);
    }
}
"#;

#[test]
fn state_machine_idiom_works() {
    let src = format!(
        "{STATE_MACHINE}
class Program {{
    static int Main() {{
        FileHandle<Open> f = Files.OpenFile(4);
        int data = Files.Read(f);
        FileHandle<Closed> done = Files.Close(f);
        return data + done.fd;
    }}
}}
"
    );
    assert_parity(&src, 44);
}

/// The tag must be un-dodgeable: every escape hatch is a compile error.
#[test]
fn tags_cannot_be_dodged() {
    let cases: &[(&str, &str)] = &[
        // Reading a closed handle.
        (
            "FileHandle<Closed> c = new FileHandle<Closed>(1); int d = Files.Read(c);",
            "argument type mismatch",
        ),
        // Cross-tag assignment.
        (
            "FileHandle<Open> o = Files.OpenFile(1); FileHandle<Closed> c = o;",
            "cannot assign",
        ),
        // Raw instantiation dodges the tag.
        (
            "FileHandle<Open> o = new FileHandle(1);",
            "expects 1 type argument(s), got 0",
        ),
        // Raw type reference dodges the tag.
        (
            "FileHandle raw = new FileHandle<Open>(1);",
            "expects 1 type argument(s), got 0",
        ),
        // Wrong arity.
        (
            "FileHandle<Open, Closed> h = new FileHandle<Open>(1);",
            "expects 1 type argument(s), got 2",
        ),
        // Type arguments on a non-generic class.
        (
            "Open<int> o = new Open();",
            "expects 0 type argument(s), got 1",
        ),
        // Raw generic nested inside another type argument.
        (
            "List<FileHandle> xs = new List<FileHandle>();",
            "expects 1 type argument(s), got 0",
        ),
        // Constraint violation: int is no FileState.
        (
            "FileHandle<int> h = new FileHandle<int>(1);",
            "does not satisfy constraint FileState",
        ),
    ];
    for (stmt, needle) in cases {
        let src = format!(
            "{STATE_MACHINE}
class Program {{ static void Main() {{ {stmt} }} }}
"
        );
        assert_compile_fails(&src, needle);
    }
}

/// Constraints accept structural (duck-typed) satisfaction: Open/Closed
/// never declare `: FileState`, yet satisfy it by shape. A class missing
/// the shape is rejected.
#[test]
fn constraints_compose_with_duck_typing() {
    let good = format!(
        "{STATE_MACHINE}
class Program {{
    static int Main() {{
        FileHandle<Open> f = new FileHandle<Open>(9);
        return f.fd;
    }}
}}
"
    );
    assert_parity(&good, 9);

    let bad = r#"
interface FileState { int Marker(); }
class Wrong { int Other() { return 0; } }
class FileHandle<S : FileState> { int fd; FileHandle(int fd) { this.fd = fd; } }
class Program {
    static void Main() {
        FileHandle<Wrong> h = new FileHandle<Wrong>(1);
    }
}
"#;
    assert_compile_fails(bad, "does not satisfy constraint");
}

/// Erasure: phantom-tagged handles in a hot loop cost nothing — the JIT
/// sees plain objects. Mid-run tier transition, compiled count asserted,
/// 3 trials.
#[test]
fn phantom_tags_erased_in_jit_hot_loop_trials() {
    let src = format!(
        "{STATE_MACHINE}
class Program {{
    static int Cycle(int fd) {{
        FileHandle<Open> f = Files.OpenFile(fd);
        int data = Files.Read(f);
        FileHandle<Closed> done = Files.Close(f);
        return data + done.fd;
    }}
    static int Main() {{
        int total = 0;
        int i = 0;
        while (i < 300) {{
            total = total + Cycle(i);
            i = i + 1;
        }}
        return total;
    }}
}}
"
    );
    // sum over i of (10i + i) = 11 * sum(0..299) = 11 * 44850 = 493350.
    for trial in 0..3 {
        assert_parity(&src, 493350);
        let module = Arc::new(compile(&src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(100);
        match vm.run().expect("jit threshold-100 run") {
            Value::Int(i) => assert_eq!(i, 493350, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
