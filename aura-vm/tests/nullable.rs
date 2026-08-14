//! Strict nullable types: `T?`, narrowing, `!`, `??`/`?.` typing, and the
//! compile-time rejections that make plain `T` actually non-nullable.

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

/// The given source must be rejected at compile time with a message
/// containing `needle`.
fn assert_compile_error(source: &str, needle: &str) {
    match compile(source, "test") {
        Ok(_) => panic!("expected compile error containing `{needle}`, but compilation succeeded"),
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains(needle),
                "expected error containing `{needle}`, got: {msg}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Positive semantics
// ---------------------------------------------------------------------------

#[test]
fn narrowing_and_operators_work() {
    let src = r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        Node b = new Node(20);
        a.next = b;
        int total = 0;

        // while-narrowing
        Node? cur = a;
        while (cur != null) {
            total = total + cur.value;
            cur = cur.next;
        }

        // if-narrowing with else-complement
        Node? maybe = a.next;
        if (maybe != null) {
            total = total + maybe.value;
        } else {
            total = total + 100000;
        }
        Node? nothing = b.next;
        if (nothing == null) {
            total = total + 300;
        } else {
            total = total + nothing.value;
        }

        // early-exit narrowing
        Node? head = a.next;
        if (head == null) { return -1; }
        total = total + head.value;

        // assertion and coalescing
        total = total + a.next!.value;
        Node fallback = b.next ?? a;
        total = total + fallback.value;
        total = total + (b.next?.value ?? 4000);

        return total;
    }
}
"#;
    // walk 21, maybe 20, nothing 300, head 20, assert 20, fallback 1,
    // coalesce 4000 => 4382.
    assert_parity(src, 4382);
}

#[test]
fn nullable_value_types_work() {
    // int? holds either an int or null; == null works on value types.
    let src = r#"
class Program {
    static int? FindEven(List<int> xs) {
        for (int x in xs) {
            if (x - (x / 2) * 2 == 0) { return x; }
        }
        return null;
    }
    static int Main() {
        List<int> odds = new List<int>();
        odds.Add(1); odds.Add(3);
        List<int> mixed = new List<int>();
        mixed.Add(1); mixed.Add(8);
        int total = 0;
        int? found = FindEven(mixed);
        if (found != null) { total = total + found; }
        total = total + (FindEven(odds) ?? -100);
        return total;
    }
}
"#;
    assert_parity(src, -92);
}

/// A failed `!` assertion is a runtime error under both tiers.
#[test]
fn failed_assertion_errors_at_runtime() {
    let src = r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        return a.next!.value;
    }
}
"#;
    for jit in [false, true] {
        let err = run(src, jit).expect_err("assertion on null must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("null assertion failed"),
            "expected assertion error, got: {msg} (jit={jit})"
        );
    }
}

/// Narrowing survives JIT hot loops: the linked-list walk pattern compiled
/// and re-run across trials.
#[test]
fn narrowed_walk_jit_parity_trials() {
    let src = r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Walk(Node start) {
        int total = 0;
        Node? cur = start;
        while (cur != null) {
            total = total + cur.value;
            cur = cur.next;
        }
        return total;
    }
    static int Main() {
        Node? head = null;
        int i = 1;
        while (i <= 25) {
            Node n = new Node(i);
            n.next = head;
            head = n;
            i = i + 1;
        }
        if (head == null) { return -1; }
        int grand = 0;
        int round = 0;
        while (round < 80) {
            grand = grand + Walk(head);
            round = round + 1;
        }
        return grand;
    }
}
"#;
    // 80 * sum(1..25) = 80 * 325 = 26000.
    for trial in 0..3 {
        assert_parity(src, 26000);
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(30);
        match vm.run().expect("tier-transition run") {
            Value::Int(i) => assert_eq!(i, 26000, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}

// ---------------------------------------------------------------------------
// Strictness: what must NOT compile
// ---------------------------------------------------------------------------

#[test]
fn null_not_assignable_to_plain_types() {
    assert_compile_error(
        r#"class Program { static void Main() { string s = null; } }"#,
        "cannot assign null",
    );
    assert_compile_error(
        r#"
class Node { int value; Node(int v) { this.value = v; } }
class Program { static void Main() { Node n = null; } }
"#,
        "cannot assign null",
    );
}

#[test]
fn nullable_not_assignable_to_plain_without_narrowing() {
    assert_compile_error(
        r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static void Main() {
        Node a = new Node(1);
        Node plain = a.next;
    }
}
"#,
        "cannot assign Node?",
    );
    // ReadLine returns string? and no longer fits a plain string.
    assert_compile_error(
        r#"class Program { static void Main() { string s = Console.ReadLine(); } }"#,
        "cannot assign string?",
    );
}

#[test]
fn member_access_on_nullable_rejected() {
    assert_compile_error(
        r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        Node? cur = a.next;
        return cur.value;
    }
}
"#,
        "may be null",
    );
    assert_compile_error(
        r#"
class Program {
    static int Main() {
        string? s = Console.ReadLine();
        return s.Length;
    }
}
"#,
        "may be null",
    );
}

#[test]
fn assignment_inside_narrowed_scope_rewidens() {
    // After `cur = cur.next` the variable is nullable again, so a direct
    // member access must be rejected even inside the narrowed branch.
    assert_compile_error(
        r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        Node? cur = a;
        if (cur != null) {
            cur = cur.next;
            return cur.value;
        }
        return 0;
    }
}
"#,
        "may be null",
    );
}

#[test]
fn uninitialized_reference_locals_rejected() {
    assert_compile_error(
        r#"
class Node { int value; Node(int v) { this.value = v; } }
class Program { static void Main() { Node n; } }
"#,
        "must be initialized",
    );
}
