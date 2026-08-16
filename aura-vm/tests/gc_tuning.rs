//! GC tuning: configurable heap/nursery sizes and a hard heap limit,
//! throughput/latency disposition presets, a best-effort soft target for
//! minor pause times (adaptive nursery sizing — majors stay stop-the-world
//! and unbounded), and a stats snapshot (counts, live set, bytes freed,
//! pause totals/maxima).

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::heap::GcMode;
use aura_vm::Vm;
use std::sync::Arc;

const CHURN: &str = r#"
class Item { string name; Item(string name) { this.name = name; } }
class Program {
    static int Main() {
        var keep = new List<Item>();
        for (var i in 1..=100) {
            keep.Add(new Item("retained payload number {i} padding"));
        }
        int total = 0;
        for (var i in 1..=3000) {
            Item tmp = new Item("temporary {i} with some padding to add bytes");
            total = total + tmp.name.Length - tmp.name.Length;
        }
        return total + keep.Count;
    }
}
"#;

fn run_with(configure: impl FnOnce(&mut Vm)) -> (i64, Vm) {
    let module = Arc::new(compile(CHURN, "test").expect("compile"));
    let mut vm = Vm::new(module);
    configure(&mut vm);
    let result = match vm.run().expect("run") {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    (result, vm)
}

/// An explicit nursery size changes collection behavior: a tiny nursery
/// produces strictly more minor collections than a large one, with
/// identical program results.
#[test]
fn nursery_size_is_configurable() {
    let (r_small, vm_small) = run_with(|vm| {
        vm.set_gc_threshold(64 * 1024);
        vm.set_gc_nursery_size(Some(2 * 1024));
    });
    let (r_large, vm_large) = run_with(|vm| {
        vm.set_gc_threshold(64 * 1024);
        vm.set_gc_nursery_size(Some(48 * 1024));
    });
    assert_eq!(r_small, 100);
    assert_eq!(r_large, 100);
    assert!(
        vm_small.gc_minor_collections() > vm_large.gc_minor_collections(),
        "2KB nursery ran {} minors, 48KB ran {}",
        vm_small.gc_minor_collections(),
        vm_large.gc_minor_collections()
    );
}

/// Mode presets trade pause count for pause size: latency mode collects
/// strictly more often than throughput mode on the same workload.
#[test]
fn modes_trade_frequency_for_pause_size() {
    let (r_lat, vm_lat) = run_with(|vm| {
        vm.set_gc_mode(GcMode::Latency);
        vm.set_gc_threshold(64 * 1024);
    });
    let (r_thr, vm_thr) = run_with(|vm| {
        vm.set_gc_mode(GcMode::Throughput);
        vm.set_gc_threshold(64 * 1024);
    });
    assert_eq!(r_lat, 100);
    assert_eq!(r_thr, 100);
    assert!(
        vm_lat.gc_collections() > vm_thr.gc_collections(),
        "latency ran {} collections, throughput ran {}",
        vm_lat.gc_collections(),
        vm_thr.gc_collections()
    );
}

/// A hard heap limit surfaces as a runtime error once a full collection
/// cannot get the live set under it.
#[test]
fn max_heap_limit_is_enforced() {
    let module = Arc::new(compile(CHURN, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(2048);
    vm.set_gc_max_heap(Some(1024));
    let err = vm.run().expect_err("should exceed the heap limit");
    assert!(
        format!("{err}").contains("heap limit exceeded"),
        "got: {err}"
    );

    // A generous limit changes nothing.
    let (r, _) = run_with(|vm| {
        vm.set_gc_threshold(4096);
        vm.set_gc_max_heap(Some(64 * 1024 * 1024));
    });
    assert_eq!(r, 100);
}

/// The pause-target feedback loop adapts the nursery: an unmeetable 0ms
/// target drives it down to the floor (more, smaller minors); a generous
/// target leaves room to grow it.
#[test]
fn pause_target_adapts_nursery() {
    let (r, vm) = run_with(|vm| {
        vm.set_gc_threshold(64 * 1024);
        vm.set_gc_pause_target_ms(Some(0));
    });
    assert_eq!(r, 100);
    let stats = vm.gc_stats();
    assert!(stats.minor_collections > 0, "expected minors");
    assert_eq!(
        stats.nursery_size,
        8 * 1024,
        "an unmeetable target should shrink the nursery to its floor"
    );
}

/// The stats snapshot is populated and internally consistent.
#[test]
fn stats_snapshot_is_consistent() {
    let (_, vm) = run_with(|vm| {
        vm.set_gc_threshold(4096);
    });
    let s = vm.gc_stats();
    assert_eq!(s.collections, s.minor_collections + s.major_collections);
    assert!(s.collections > 0, "expected collections");
    assert!(s.total_allocations > 3000, "allocations counted");
    assert!(s.bytes_freed > 0, "transient churn should free bytes");
    assert!(s.live_objects > 0 && s.live_bytes > 0, "retained set live");
    assert!(
        s.minor_pause_total + s.major_pause_total > std::time::Duration::ZERO,
        "pause time recorded"
    );
    assert!(s.nursery_size > 0 && s.threshold > 0);
}
