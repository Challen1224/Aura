//! Checked exceptions (opt-in): exceptions stay unchecked by default, but a
//! method declaring `throws IOException` binds its callers — each declared
//! exception must be caught by an enclosing `try` (a catch of the type or a
//! supertype) or re-declared by the calling method. Overrides and interface
//! implementations may not add throws their base declaration lacks. Purely
//! compile-time: no bytecode or runtime changes.

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

const EXC: &str = r#"
class IOException : Exception {
    IOException(string m) { this.message = m; }
}
class ParseError : Exception {
    ParseError(string m) { this.message = m; }
}
"#;

/// The contract end-to-end under both tiers: declaring, propagating
/// through a re-declaring caller, catching the exact type, catching a
/// supertype, multiple declared exceptions, and the actual throw/catch
/// runtime path through declared methods.
#[test]
fn checked_contracts_work() {
    let src = format!(
        "{EXC}
class Files {{
    static int read(string path) throws IOException {{
        if (path.Length == 0) {{
            throw new IOException(\"empty\");
        }}
        return path.Length;
    }}
    static int parse(string path) throws IOException, ParseError {{
        if (path == \"bad\") {{
            throw new ParseError(\"syntax\");
        }}
        return Files.read(path);
    }}
}}
class Program {{
    static int chain(string p) throws IOException, ParseError {{
        return Files.parse(p) * 2;
    }}
    static int Main() {{
        int total = 0;
        try {{
            total = total + Files.read(\"data.txt\");
            total = total + Files.parse(\"cfg\") * 10;
            total = total + Program.chain(\"ab\") * 100;
        }} catch (IOException e) {{
            total = total + 1000000;
        }} catch (ParseError e) {{
            total = total + 2000000;
        }}
        try {{
            total = total + Files.read(\"\");
        }} catch (Exception e) {{
            total = total + e.message.Length * 10000;
        }}
        try {{
            total = total + Files.parse(\"bad\");
        }} catch (IOException e) {{
            total = total + 7000000;
        }} catch (ParseError e) {{
            total = total + e.message.Length * 100000;
        }}
        return total;
    }}
}}"
    );
    // 8 + 30 + 400 + 50000 ("empty".Length=5) + 600000 ("syntax".Length=6).
    assert_parity(&src, 650438);
}

/// Every contract rule, pinned.
#[test]
fn checked_exception_rules() {
    let pre = "class IOException : Exception { IOException(string m) { this.message = m; } } \
               class Files { static int op() throws IOException { \
               throw new IOException(\"x\"); } }";
    let prog = "class Program { static void Main() { } }";
    let cases: Vec<(String, &str)> = vec![
        // Uncaught, undeclared call.
        (
            format!("{pre} class Program {{ static void Main() {{ Files.op(); }} }}"),
            "catch it or declare `throws IOException`",
        ),
        // A catch body is not covered by its own try's catches.
        (
            format!(
                "{pre} class Program {{ static void Main() {{ \
                 try {{ int x = 1; }} catch (IOException e) {{ Files.op(); }} }} }}"
            ),
            "catch it or declare",
        ),
        // Catching an unrelated exception does not cover it.
        (
            format!(
                "{pre} class Other : Exception {{ }} \
                 class Program {{ static void Main() {{ \
                 try {{ Files.op(); }} catch (Other e) {{ }} }} }}"
            ),
            "catch it or declare",
        ),
        // `throws` must name an exception class.
        (
            format!("class NotExc {{ int v; }} class C {{ static void f() throws NotExc {{ }} }} {prog}"),
            "must derive from `Exception`",
        ),
        (
            format!("class C {{ static void f() throws Missing {{ }} }} {prog}"),
            "must derive from `Exception`",
        ),
        // Overrides cannot add throws.
        (
            format!(
                "{EXC} class Base {{ virtual int m() {{ return 1; }} }} \
                 class D : Base {{ override int m() throws IOException {{ return 2; }} }} {prog}"
            ),
            "could not know to handle it",
        ),
        // Interface implementations cannot add throws.
        (
            format!(
                "{EXC} interface IRun {{ int m(); }} \
                 class R : IRun {{ int m() throws IOException {{ return 2; }} }} {prog}"
            ),
            "could not know to handle it",
        ),
        // Duplicate names in one clause.
        (
            format!(
                "{EXC} class C {{ static void f() throws IOException, IOException {{ }} }} {prog}"
            ),
            "more than once",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// Catching a supertype (including `Exception`) satisfies the contract,
/// and a caller declaring a supertype of the callee's throw is covered.
#[test]
fn supertype_coverage() {
    let src = format!(
        "{EXC}
class Net {{
    static int fetch() throws IOException {{ return 4; }}
}}
class Program {{
    static int relay() throws Exception {{
        return Net.fetch() * 10;
    }}
    static int Main() {{
        try {{
            return Program.relay() + Net.fetch();
        }} catch (Exception e) {{
            return -1;
        }}
    }}
}}"
    );
    assert_parity(&src, 44);
}

/// Declared methods throwing across the JIT tier boundary: a hot loop
/// calling a declared thrower with catches, crossing the threshold
/// mid-run. 3 trials, compiled count asserted.
#[test]
fn checked_exceptions_in_jit_hot_loop_trials() {
    let src = format!(
        "{EXC}
class Gate {{
    static int pass(int i) throws IOException {{
        if (i % 10 == 0) {{
            throw new IOException(\"blocked\");
        }}
        return i;
    }}
}}
class Program {{
    static int Main() {{
        int total = 0;
        for (var i in 1..=200) {{
            try {{
                total = total + Gate.pass(i);
            }} catch (IOException e) {{
                total = total + 1;
            }}
        }}
        return total;
    }}
}}"
    );
    // sum(1..200) - (10+20+...+200) + 20 = 20100 - 2100 + 20 = 18020.
    for trial in 0..3 {
        assert_parity(&src, 18020);
        let module = Arc::new(compile(&src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 18020, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
