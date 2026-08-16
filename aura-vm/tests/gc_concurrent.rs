//! Concurrent GC (opt-in via `Vm::set_gc_concurrent` / `--gc-concurrent`).
//!
//! Majors become snapshot-at-the-beginning cycles: the stop-the-world
//! cost is a deep clone of the object map (no tracing), the entire mark
//! runs on a background thread over the snapshot, and the dead set is
//! swept in bounded chunks at later safepoints. Soundness rests on
//! monotonic, never-reused handles: an object unreachable in the
//! snapshot is unreachable forever, and anything allocated after the
//! snapshot is implicitly live — so no marking write barrier is needed
//! beyond the snapshot itself, and no read barrier at all (the collector
//! never moves objects). Minors stay stop-the-world (already
//! pause-bounded) and keep the nursery collected while a cycle runs.
//! Hard-limit pressure joins the cycle synchronously so `--gc-max-heap`
//! stays exact; a 2x-threshold backpressure stall bounds heap growth if
//! the mutator outruns the marker.
//!
//! Reclamation *timing* under concurrent mode depends on marker speed —
//! inherent to every concurrent collector — which is why the
//! deterministic stop-the-world collector remains the default. Programs
//! whose results don't observe reclamation timing behave identically;
//! reference-observing programs get eventual guarantees (tested below
//! with bounded spin-waits).

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::Vm;
use std::sync::Arc;

fn run_mode(source: &str, jit: bool, concurrent: bool, threshold: usize) -> (i64, Vm) {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(threshold);
    vm.set_gc_concurrent(concurrent);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(1);
    }
    let result = match vm.run().expect("run") {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    #[cfg(target_arch = "x86_64")]
    if jit {
        assert!(vm.jit_compiled_count() > 0, "jit compiled nothing");
    }
    vm.gc_settle();
    (result, vm)
}

/// A program whose result is independent of reclamation timing computes
/// the same answer under stop-the-world and concurrent collection, on
/// both tiers — and the concurrent runs actually run concurrent cycles
/// while minors keep the nursery collected. 3 trials.
#[test]
fn concurrent_matches_stw_results() {
    let src = r#"
class Node { string tag; Node? next; Node(string tag) { this.tag = tag; } }
class Program {
    static int pad(int i) { return i - i; }
    static int Main() {
        Node head = new Node("head");
        Node cur = head;
        for (var i in 1..=200) {
            Node n = new Node("kept {i} padding padding");
            cur.next = n;
            cur = n;
        }
        int total = 0;
        for (var i in 1..=20000) {
            string junk = "throwaway {i} padding padding padding";
            total = total + junk.Length - junk.Length + Program.pad(i);
        }
        int count = 0;
        Node? p = head;
        while (p != null) { count = count + 1; p = p.next; }
        return total + count;
    }
}
"#;
    for _ in 0..3 {
        let (stw, _) = run_mode(src, false, false, 8192);
        assert_eq!(stw, 201, "stop-the-world baseline");
        let (conc, vm) = run_mode(src, false, true, 8192);
        assert_eq!(conc, stw, "concurrent interpreter");
        let stats = vm.gc_stats();
        assert!(stats.concurrent_cycles > 0, "expected concurrent cycles");
        assert!(
            stats.minor_collections > 0,
            "minors should keep running during cycles"
        );
        let (conc_jit, vm_jit) = run_mode(src, true, true, 8192);
        assert_eq!(conc_jit, stw, "concurrent jit");
        #[cfg(target_arch = "x86_64")]
        assert!(vm_jit.gc_stats().concurrent_cycles > 0);
        let _ = vm_jit;
    }
}

/// Eventual reclamation: a weak target with no strong root is reclaimed
/// by the concurrent collector in bounded time — a spin-wait that
/// allocates (driving safepoints) observes the flip on both tiers.
#[test]
fn eventual_reclamation_under_concurrent() {
    let src = r#"
class Data { string tag; Data(string tag) { this.tag = tag; } }
class Program {
    static int pad(int i) { return i - i; }
    static WeakRef<Data> makeDoomed() {
        // The only frame to ever hold the target.
        return new WeakRef(new Data("doomed"));
    }
    static int Main() {
        var doomed = Program.makeDoomed();
        int spins = 0;
        while (doomed.isAlive() && spins < 200000) {
            string junk = "spin {spins} padding padding padding";
            spins = spins + 1 + Program.pad(spins);
        }
        if (doomed.isAlive()) { return -1; }
        if (doomed.get() != null) { return -2; }
        return 1;
    }
}
"#;
    for jit in [false, true] {
        let (r, vm) = run_mode(src, jit, true, 4096);
        assert_eq!(r, 1, "weak target should be reclaimed (jit={jit})");
        assert!(vm.gc_stats().collections > 0);
    }
}

/// `--gc-max-heap` stays exact under concurrent mode: pressure joins the
/// in-flight cycle synchronously, soft references are still cleared
/// before the limit errors, and a genuine strong overflow still fails.
#[test]
fn max_heap_stays_exact_under_concurrent() {
    let soft_src = r#"
class Blob {
    List<string> data;
    Blob(int n) {
        this.data = new List<string>();
        for (var i in 1..=n) { this.data.Add("blob payload {i} padding padding padding"); }
    }
}
class Program {
    static SoftRef<Blob> makeSoft() { return new SoftRef(new Blob(60)); }
    static int Main() {
        var soft = Program.makeSoft();
        var keep = new List<string>();
        for (var i in 1..=150) {
            keep.Add("retained {i} padding padding padding");
        }
        int score = 0;
        if (soft.isAlive()) { score = score + 100; }
        if (soft.get() == null) { score = score + 10; }
        return score + keep.Count;
    }
}
"#;
    let module = Arc::new(compile(soft_src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(4096);
    vm.set_gc_concurrent(true);
    vm.set_gc_max_heap(Some(8 * 1024));
    match vm.run().expect("run") {
        // Soft cleared under pressure: 10 + 150.
        Value::Int(i) => assert_eq!(i, 160),
        other => panic!("expected int, got {other:?}"),
    }

    let overflow_src = r#"
class Program {
    static int Main() {
        var keep = new List<string>();
        for (var i in 1..=1500) {
            keep.Add("retained {i} padding padding padding");
        }
        return keep.Count;
    }
}
"#;
    let module = Arc::new(compile(overflow_src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(4096);
    vm.set_gc_concurrent(true);
    vm.set_gc_max_heap(Some(8 * 1024));
    let err = vm.run().expect_err("strong overflow should still error");
    assert!(
        format!("{err}").contains("heap limit exceeded"),
        "got: {err}"
    );
}

/// Stats stay internally consistent in concurrent mode: cycles count as
/// majors, the combined counter adds up, and background mark time is
/// recorded (off-thread, distinct from pauses).
#[test]
fn concurrent_stats_are_consistent() {
    let src = r#"
class Item { string name; Item(string name) { this.name = name; } }
class Program {
    static int Main() {
        var keep = new List<Item>();
        for (var i in 1..=100) {
            keep.Add(new Item("retained payload number {i} padding"));
        }
        int total = 0;
        for (var i in 1..=8000) {
            Item tmp = new Item("temporary {i} with some padding to add bytes");
            total = total + tmp.name.Length - tmp.name.Length;
        }
        return total + keep.Count;
    }
}
"#;
    let (r, vm) = run_mode(src, false, true, 8192);
    assert_eq!(r, 100);
    let s = vm.gc_stats();
    assert_eq!(s.collections, s.minor_collections + s.major_collections);
    assert!(
        s.concurrent_cycles > 0 && s.concurrent_cycles <= s.major_collections,
        "cycles {} vs majors {}",
        s.concurrent_cycles,
        s.major_collections
    );
    assert!(
        s.concurrent_mark_total > std::time::Duration::ZERO,
        "background mark time should be recorded"
    );
    assert!(s.bytes_freed > 0, "churn should be reclaimed");
}
