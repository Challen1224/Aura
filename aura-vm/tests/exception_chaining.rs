//! Exception chaining: the builtin `Exception` carries a `cause` field
//! (`Exception?`, null by default). Wrap-and-rethrow keeps the original:
//! `throw new AppError("context", e)` where the constructor stores
//! `this.cause = e`. The uncaught-exception report walks the chain
//! (`caused by: ...`, depth-capped against cycles). Reading `cause` follows
//! the normal nullable rules — narrow before use.

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

const EXC: &str = r#"
class DbError : Exception {
    DbError(string m) { this.message = m; }
}
class AppError : Exception {
    AppError(string m, Exception c) { this.message = m; this.cause = c; }
}
"#;

/// Wrap-and-rethrow through two layers under both tiers: the cause chain
/// survives the catches, `cause` is null on fresh exceptions, and narrowed
/// cause access reads the original message.
#[test]
fn chaining_works() {
    let src = format!(
        "{EXC}
class Program {{
    static int load() throws DbError {{
        throw new DbError(\"connection refused\");
    }}
    static int boot() throws AppError {{
        try {{
            return Program.load();
        }} catch (DbError e) {{
            throw new AppError(\"boot failed\", e);
        }}
    }}
    static int Main() {{
        int total = 0;
        try {{
            total = Program.boot();
        }} catch (AppError e) {{
            total = total + e.message.Length;
            var c = e.cause;
            if (c != null) {{
                total = total + c.message.Length * 100;
                if (c is DbError) {{ total = total + 10000; }}
                if (c.cause == null) {{ total = total + 100000; }}
            }}
        }}
        var fresh = new DbError(\"x\");
        if (fresh.cause == null) {{ total = total + 1000000; }}
        return total;
    }}
}}"
    );
    // "boot failed".len=11 + "connection refused".len(18)*100 + 10000
    // + 100000 + 1000000 = 1111811.
    assert_parity(&src, 1111811);
}

/// The uncaught-exception report renders the full chain with
/// `caused by:` lines.
#[test]
fn uncaught_report_walks_chain() {
    let src = format!(
        "{EXC}
class Program {{
    static void Main() {{
        DbError root = new DbError(\"disk full\");
        AppError mid = new AppError(\"save failed\", root);
        AppError top = new AppError(\"request failed\", mid);
        throw top;
    }}
}}"
    );
    let module = Arc::new(compile(&src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let err = vm.run().expect_err("should be uncaught");
    let msg = format!("{err}");
    assert!(msg.contains("AppError: request failed"), "got: {msg}");
    assert!(msg.contains("caused by: AppError: save failed"), "got: {msg}");
    assert!(msg.contains("caused by: DbError: disk full"), "got: {msg}");
}

/// A cyclic cause chain cannot hang the reporter (depth-capped).
#[test]
fn cyclic_chain_is_capped() {
    let src = format!(
        "{EXC}
class Program {{
    static void Main() {{
        DbError a = new DbError(\"a\");
        AppError b = new AppError(\"b\", a);
        a.cause = b;
        throw b;
    }}
}}"
    );
    let module = Arc::new(compile(&src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let err = vm.run().expect_err("should be uncaught");
    let msg = format!("{err}");
    assert!(msg.contains("chain truncated"), "got: {msg}");
}

/// Chained wrap-and-rethrow across the JIT tier boundary in a hot loop.
/// 3 trials, compiled count asserted.
#[test]
fn chaining_in_jit_hot_loop_trials() {
    let src = format!(
        "{EXC}
class Program {{
    static int inner(int i) throws DbError {{
        if (i % 5 == 0) {{
            throw new DbError(\"db\");
        }}
        return i;
    }}
    static int outer(int i) throws AppError {{
        try {{
            return Program.inner(i);
        }} catch (DbError e) {{
            throw new AppError(\"app\", e);
        }}
    }}
    static int Main() {{
        int total = 0;
        for (var i in 1..=200) {{
            try {{
                total = total + Program.outer(i);
            }} catch (AppError e) {{
                var c = e.cause;
                if (c != null) {{ total = total + c.message.Length; }}
            }}
        }}
        return total;
    }}
}}"
    );
    // sum(1..200) - sum(multiples of 5) + 40*2 = 20100 - 4100 + 80 = 16080.
    for trial in 0..3 {
        assert_parity(&src, 16080);
        let module = Arc::new(compile(&src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 16080, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
