//! Manual GC benchmark (ignored by default): big stable old heap + heavy
//! short-lived churn under a pinned threshold. Run with
//! `cargo test --release -p aura-vm --test gc_bench -- --ignored --nocapture`.

use aura_compiler::compile;
use aura_vm::Vm;
use std::sync::Arc;

#[test]
#[ignore]
fn churn_timing() {
    let src = r#"
class Item { string name; Item(string name) { this.name = name; } }
class Program {
    static void Main() {
        var keep = new List<Item>();
        for (var i in 1..=8000) {
            keep.Add(new Item("retained payload {i} with padding padding padding"));
        }
        int total = 0;
        for (var i in 1..=150000) {
            Item tmp = new Item("temp {i} padding padding");
            total = total + tmp.name.Length - tmp.name.Length;
        }
        print(total + keep.Count); println();
    }
}
"#;
    let module = Arc::new(compile(src, "bench").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(1 << 20); // 1MB pinned: old heap ~ threshold, frequent pressure
    let start = std::time::Instant::now();
    vm.run().expect("run");
    let elapsed = start.elapsed();
    println!(
        "elapsed: {:?}  collections: {} (minor {}, major {})",
        elapsed,
        vm.gc_collections(),
        vm.gc_minor_collections(),
        vm.gc_major_collections()
    );
}
