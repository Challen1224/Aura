//! Map/Set lookup performance: proves lookups are hash-indexed, not linear
//! scans. A linear-scan backing makes total work quadratic in N, so an 8×
//! bigger workload costs ~64× the time; with hash lookup it costs ~8×. The
//! scaling assertion is deliberately generous so it stays robust on slow or
//! emulated hosts while still failing hard on O(n) lookups.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::Vm;
use std::sync::Arc;
use std::time::Instant;

fn map_program(n: u32) -> String {
    format!(
        r#"
class Program {{
    static int Main() {{
        Map<int, int> m = new Map<int, int>();
        int i = 0;
        while (i < {n}) {{ m.Put(i * 7, i); i = i + 1; }}
        int total = 0;
        int j = 0;
        while (j < {n}) {{ total = total + m.Get(j * 7); j = j + 1; }}
        int k = 0;
        while (k < {n}) {{
            if (m.ContainsKey(k * 7 + 1)) {{ total = total + 1000000; }}
            k = k + 1;
        }}
        return total;
    }}
}}
"#
    )
}

fn set_program(n: u32) -> String {
    format!(
        r#"
class Program {{
    static int Main() {{
        Set<int> s = new Set<int>();
        int i = 0;
        while (i < {n}) {{ s.Add(i * 3); i = i + 1; }}
        int hits = 0;
        int j = 0;
        while (j < {n}) {{
            if (s.Contains(j * 3)) {{ hits = hits + 1; }}
            if (s.Contains(j * 3 + 1)) {{ hits = hits + 1000000; }}
            j = j + 1;
        }}
        return hits;
    }}
}}
"#
    )
}

fn timed_run(source: &str, jit: bool) -> (i64, f64) {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(1);
    }
    let start = Instant::now();
    let result = match vm.run().expect("run") {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    };
    (result, start.elapsed().as_secs_f64())
}

fn expected_map_total(n: i64) -> i64 {
    n * (n - 1) / 2
}

#[test]
fn map_lookup_scales_like_hashing() {
    let (small_res, small_t) = timed_run(&map_program(500), false);
    assert_eq!(small_res, expected_map_total(500));
    let (big_res, big_t) = timed_run(&map_program(4000), false);
    assert_eq!(big_res, expected_map_total(4000));
    let ratio = big_t / small_t.max(1e-9);
    println!("map: n=500 {small_t:.4}s, n=4000 {big_t:.4}s, ratio {ratio:.1}");
    // 8x the workload: hash lookup ~8x time, linear scan ~64x. Allow wide
    // margin for noise; anything quadratic still fails by a distance.
    assert!(
        ratio < 30.0,
        "map lookup scales super-linearly (ratio {ratio:.1}) — still a linear scan?"
    );
}

/// Large hashed Map/Set workload inside JIT-compiled hot loops: results
/// must match the interpreter exactly, across repeated trials.
#[test]
fn large_hashed_workload_jit_parity_trials() {
    for trial in 0..3 {
        let (interp_map, _) = timed_run(&map_program(3000), false);
        let (jit_map, jit_map_t) = timed_run(&map_program(3000), true);
        assert_eq!(interp_map, expected_map_total(3000), "trial {trial} map interp");
        assert_eq!(jit_map, interp_map, "trial {trial} map jit");
        let (interp_set, _) = timed_run(&set_program(3000), false);
        let (jit_set, jit_set_t) = timed_run(&set_program(3000), true);
        assert_eq!(interp_set, 3000, "trial {trial} set interp");
        assert_eq!(jit_set, interp_set, "trial {trial} set jit");
        println!("trial {trial}: jit map {jit_map_t:.4}s, jit set {jit_set_t:.4}s");
    }
}

#[test]
fn set_lookup_scales_like_hashing() {
    let (small_res, small_t) = timed_run(&set_program(500), false);
    assert_eq!(small_res, 500);
    let (big_res, big_t) = timed_run(&set_program(4000), false);
    assert_eq!(big_res, 4000);
    let ratio = big_t / small_t.max(1e-9);
    println!("set: n=500 {small_t:.4}s, n=4000 {big_t:.4}s, ratio {ratio:.1}");
    assert!(
        ratio < 30.0,
        "set lookup scales super-linearly (ratio {ratio:.1}) — still a linear scan?"
    );
}
