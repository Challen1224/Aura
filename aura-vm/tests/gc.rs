//! Garbage-collection tests: prove that collection actually runs, actually
//! reclaims garbage, and never frees objects still reachable — including
//! objects whose only references live in JIT native-frame slots.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::{Vm, VmError};
use std::sync::Arc;

struct RunStats {
    result: i64,
    collections: u64,
    live_after: usize,
    total_allocated: u64,
    compiled: usize,
}

fn run(source: &str, jit: bool, gc_threshold: usize) -> Result<RunStats, VmError> {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(gc_threshold);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(1);
    }
    let result = match vm.run()? {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    Ok(RunStats {
        result,
        collections: vm.gc_collections(),
        live_after: vm.heap().live_count(),
        total_allocated: vm.heap().total_allocations(),
        compiled: vm.jit_compiled_count(),
    })
}

/// Creates string garbage every iteration while keeping a long-lived list;
/// the final result depends on survivors being intact after many cycles.
const STRING_CHURN: &str = r#"
class Program {
    static int Main() {
        List<int> keep = new List<int>();
        int i = 0;
        while (i < 2000) {
            string s = "hello world number {i}";
            string t = s.ToUpper();
            if (i - (i / 100) * 100 == 0) {
                keep.Add(t.Length);
            }
            i = i + 1;
        }
        int total = 0;
        int j = 0;
        while (j < keep.Count) {
            total = total + keep.Get(j);
            j = j + 1;
        }
        return total;
    }
}
"#;

// keep gets i = 0, 100, ..., 1900: lengths of "HELLO WORLD NUMBER {i}".
// Base "HELLO WORLD NUMBER " = 19 chars; digits: 1 (i=0), 3 (100..900),
// 4 (1000..1900) => 20 + 9*22 + 10*23 = 448.
const STRING_CHURN_EXPECTED: i64 = 448;

#[test]
fn interpreter_collects_and_reclaims() {
    let stats = run(STRING_CHURN, false, 4096).expect("interpreter should run");
    assert_eq!(stats.result, STRING_CHURN_EXPECTED, "survivors must be intact");
    assert!(
        stats.collections >= 3,
        "expected several collections, got {}",
        stats.collections
    );
    // Reclamation, not just survival: the loop allocates thousands of
    // temporary strings; almost all must be gone at the end.
    assert!(
        (stats.live_after as u64) * 10 < stats.total_allocated,
        "live {} vs total allocated {} — collection did not reclaim",
        stats.live_after,
        stats.total_allocated
    );
    println!(
        "interpreter: collections={} live_after={} total_allocated={}",
        stats.collections, stats.live_after, stats.total_allocated
    );
}

/// Builds and abandons a fresh linked list every round inside a hot loop, so
/// collections trigger at helper safepoints while JIT-compiled code is on
/// the native stack and the only references to live nodes are JIT frame
/// slots. A missed root here shows up as a dangling-ref error or wrong sum.
const JIT_GRAPH_CHURN: &str = r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        int total = 0;
        int round = 0;
        while (round < 40) {
            Node? head = null;
            int i = 0;
            while (i < 30) {
                Node n = new Node(i);
                n.next = head;
                head = n;
                i = i + 1;
            }
            Node? cur = head;
            while (cur != null) {
                total = total + cur.value;
                cur = cur.next;
            }
            round = round + 1;
        }
        return total;
    }
}
"#;

// 40 rounds * sum(0..29) = 40 * 435.
const JIT_GRAPH_EXPECTED: i64 = 17400;

#[test]
fn gc_runs_under_jit_frames() {
    let interp = run(JIT_GRAPH_CHURN, false, 2048).expect("interpreter should run");
    assert_eq!(interp.result, JIT_GRAPH_EXPECTED);
    assert!(
        interp.collections >= 2,
        "interpreter: expected collections, got {}",
        interp.collections
    );

    let jit = run(JIT_GRAPH_CHURN, true, 2048).expect("jit should run");
    assert_eq!(jit.result, interp.result, "jit must match interpreter");
    assert!(
        jit.collections >= 2,
        "jit: expected collections while JIT frames were live, got {}",
        jit.collections
    );
    #[cfg(target_arch = "x86_64")]
    assert!(jit.compiled > 0, "expected Main to compile (got 0)");
    #[cfg(not(target_arch = "x86_64"))]
    let _ = jit.compiled;
    println!(
        "jit: collections={} live_after={} total_allocated={} compiled={}",
        jit.collections, jit.live_after, jit.total_allocated, jit.compiled
    );
}

#[test]
fn gc_under_jit_is_stable_across_trials() {
    for trial in 0..5 {
        let jit = run(JIT_GRAPH_CHURN, true, 2048).expect("jit should run");
        assert_eq!(jit.result, JIT_GRAPH_EXPECTED, "trial {trial}");
        assert!(jit.collections >= 2, "trial {trial}: no collections");
        let strings = run(STRING_CHURN, true, 4096).expect("jit strings should run");
        assert_eq!(strings.result, STRING_CHURN_EXPECTED, "trial {trial} strings");
        assert!(
            (strings.live_after as u64) * 10 < strings.total_allocated,
            "trial {trial}: no reclamation under JIT"
        );
    }
}
