//! Generational GC: a logical nursery over the handle-stable heap. New
//! objects are young; minor collections trace only the nursery, seeded by
//! the roots plus the remembered set — old objects logged by the
//! `heap.get_mut` write barrier (the single mutation gateway), the only
//! old objects that can point into the nursery. Survivors promote in
//! place; full-heap pressure escalates to a major mark-and-sweep.

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

/// Minor collections actually run and reclaim young garbage while an old
/// structure keeps its data intact.
#[test]
fn minors_reclaim_young_garbage()  {
    let src = r#"
class Item { string name; Item(string name) { this.name = name; } }
class Program {
    static int Main() {
        var keep = new List<Item>();
        for (var i in 1..=20) {
            keep.Add(new Item("kept item number {i}"));
        }
        // Churn: short-lived strings and objects, nothing retained.
        int total = 0;
        for (var i in 1..=800) {
            Item tmp = new Item("temporary {i} with some padding to add bytes");
            total = total + tmp.name.Length - tmp.name.Length;
        }
        for (var k in keep) { total = total + k.name.Length; }
        return total;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(4096);
    let result = match vm.run().expect("run") {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    // "kept item number N": 18 chars for 1..=9, 19 for 10..=20.
    assert_eq!(result, 9 * 18 + 11 * 19);
    assert!(
        vm.gc_minor_collections() > 0,
        "expected minor collections, got {} minor / {} major",
        vm.gc_minor_collections(),
        vm.gc_major_collections()
    );
    // Compatibility: the combined counter still ticks.
    assert!(vm.gc_collections() >= vm.gc_minor_collections());
}

/// The write barrier: young objects reachable *only* through a mutated
/// old object survive minor collections (a long old->young chain built
/// under churn stays fully intact).
#[test]
fn write_barrier_keeps_old_to_young_alive() {
    let src = r#"
class Node {
    string tag;
    Node? next;
    Node(string tag) { this.tag = tag; }
}
class Program {
    static int Main() {
        Node head = new Node("head");
        Node cur = head;
        int i = 0;
        while (i < 40) {
            Node n = new Node("old {i}");
            cur.next = n;
            cur = n;
            i = i + 1;
        }
        int j = 0;
        while (j < 600) {
            Node fresh = new Node("young {j}");
            cur.next = fresh;
            cur = fresh;
            string junk = "junk {j} padding padding padding padding";
            j = j + 1;
        }
        int count = 0;
        Node? p = head;
        while (p != null) {
            count = count + 1;
            p = p.next;
        }
        return count;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(4096);
    match vm.run().expect("run") {
        Value::Int(i) => assert_eq!(i, 641),
        other => panic!("expected int, got {other:?}"),
    }
    assert!(vm.gc_minor_collections() > 0, "expected minor collections");
    assert_parity(src, 641);
}

/// Promotion: a young survivor keeps its identity across many minors and
/// an eventual major; statics, suspended task frames, and closures all
/// stay rooted through the generational passes.
#[test]
fn promotion_and_special_roots() {
    let src = r#"
class Box { string tag; Box(string tag) { this.tag = tag; } }
class Program {
    static Box? pinned;
    static async Task<int> holder() {
        Box b = new Box("frame-held");
        await Tasks.pause();
        await Tasks.pause();
        return b.tag.Length;
    }
    static int Main() {
        Program.pinned = new Box("static-held");
        var h = Program.holder();
        int captured = 77;
        Func<int, int> f = x => x + captured;
        int churn = 0;
        for (var i in 1..=800) {
            string junk = "junk {i} padding padding padding";
            churn = churn + junk.Length - junk.Length;
        }
        var p = Program.pinned;
        int total = churn + h.wait() + f(0);
        if (p != null) { total = total + p.tag.Length; }
        return total;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(4096);
    match vm.run().expect("run") {
        // 10 ("frame-held") + 77 + 11 ("static-held") = 98.
        Value::Int(i) => assert_eq!(i, 98),
        other => panic!("expected int, got {other:?}"),
    }
    assert!(vm.gc_collections() > 0, "expected collections");
}

/// Heavy retention escalates to major collections (the nursery alone
/// cannot relieve the pressure), and results stay correct under both
/// tiers. 3 trials.
#[test]
fn escalation_to_major_trials() {
    let src = r#"
class Item { string name; Item(string name) { this.name = name; } }
class Program {
    static int Main() {
        var keep = new List<Item>();
        for (var i in 1..=300) {
            keep.Add(new Item("retained payload number {i} padding"));
        }
        int total = 0;
        for (var k in keep) { total = total + 1; }
        return total;
    }
}
"#;
    for _ in 0..3 {
        let module = Arc::new(compile(src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.set_gc_threshold(2048);
        match vm.run().expect("run") {
            Value::Int(i) => assert_eq!(i, 300),
            other => panic!("expected int, got {other:?}"),
        }
        assert!(
            vm.gc_major_collections() > 0,
            "retention should force major collections"
        );
    }
    assert_parity(src, 300);
}
