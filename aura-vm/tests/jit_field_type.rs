//! End-to-end tests: compile Aura source, run it through the interpreter and
//! the JIT, and assert the results agree.
//!
//! These guard the canonical value-slot layout at the Rust/JIT boundary:
//! helper results and method arguments written by Rust must be canonicalized
//! before generated code reads their tag/payload qwords (see
//! `jit::value::to_parts`). The original failure: `cur != null` loop guards
//! mis-evaluated because `Value::Bool` results carried stale upper tag bytes,
//! so walks over a null-terminated list either crashed with "type mismatch:
//! expected object, got null" or never terminated.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::{Vm, VmError};
use std::sync::Arc;

const LINKED_LIST: &str = r#"
class Node {
    int value;
    Node next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        Node b = new Node(2);
        a.next = b;
        int steps = 0;
        int i = 0;
        while (i < 300) {
            Node cur = a;
            while (cur != null) {
                steps = steps + 1;
                cur = cur.next;
            }
            i = i + 1;
        }
        return steps;
    }
}
"#;

/// No field reads at all: a local reassigned to null inside a loop guarded by
/// `!= null`. Before the canonicalization fix this never terminated.
const NULL_REASSIGN_LOOP: &str = r#"
class Node {
    int value;
    Node next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        int steps = 0;
        int i = 0;
        while (i < 3) {
            Node cur = a;
            while (cur != null) {
                steps = steps + 1;
                cur = null;
            }
            i = i + 1;
        }
        return steps;
    }
}
"#;

/// A primitive-typed field read in a hot loop.
const PRIM_FIELD_LOOP: &str = r#"
class Node {
    int value;
    Node next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(7);
        int steps = 0;
        int i = 0;
        while (i < 300) {
            steps = steps + a.value;
            i = i + 1;
        }
        return steps;
    }
}
"#;

/// Virtual method calls on an object reached through a field chain.
const VIRT_CALL_THROUGH_FIELD: &str = r#"
class Node {
    int value;
    Node next;
    Node(int v) { this.value = v; this.next = null; }
    int Compute(int x) { return this.value * x; }
}
class Program {
    static int Main() {
        Node a = new Node(3);
        Node b = new Node(5);
        a.next = b;
        int total = 0;
        int i = 0;
        while (i < 100) {
            Node cur = a;
            while (cur.next != null) {
                total = total + cur.next.Compute(2);
                cur = cur.next;
            }
            i = i + 1;
        }
        return total;
    }
}
"#;

fn run_compiled(source: &str, jit: bool, threshold: u64) -> Result<i64, VmError> {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(threshold);
    }
    match vm.run()? {
        Value::Int(i) => Ok(i),
        other => panic!("expected int, got {other:?}"),
    }
}

/// Run `source` under interpreter and JIT and assert both produce `expected`.
fn assert_jit_matches(source: &str, expected: i64) {
    let interp = run_compiled(source, false, 1).expect("interpreter should run");
    let jit = run_compiled(source, true, 1).expect("jit should run");
    assert_eq!(interp, expected, "interpreter result");
    assert_eq!(jit, interp, "jit result must match interpreter exactly");
}

#[test]
fn linked_list_walk_matches_interpreter() {
    assert_jit_matches(LINKED_LIST, 600);
}

#[test]
fn null_reassign_loop_matches_interpreter() {
    assert_jit_matches(NULL_REASSIGN_LOOP, 3);
}

#[test]
fn primitive_field_hot_loop_matches_interpreter() {
    assert_jit_matches(PRIM_FIELD_LOOP, 2100);
}

#[test]
fn virtual_call_through_field_chain_matches_interpreter() {
    assert_jit_matches(VIRT_CALL_THROUGH_FIELD, 1000);
}
