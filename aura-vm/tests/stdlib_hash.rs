//! Hash-index semantics tests for Map/Set: deliberate collisions, the
//! hash/eq agreement fix for float zero, removal fixups under collision,
//! and interpreter/JIT parity on a large hashed workload.
//!
//! `value_hash` truncates ints to i32, so `k` and `k + 2^32` collide by
//! construction — these tests force multi-entry buckets on purpose, since
//! collision resolution is exactly where a naive hash rewrite breaks.

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

/// Keys 1, 2^32+1 and 2*2^32+1 all hash to 1 (i32 truncation) but are
/// distinct values: three entries in one bucket. Every lookup must resolve
/// through value_eq, and removing the middle one must not disturb the rest.
#[test]
fn colliding_int_keys_resolve_correctly() {
    let src = r#"
class Program {
    static int Main() {
        Map<int64, int> m = new Map<int64, int>();
        int64 a = 1;
        int64 b = 4294967297;
        int64 c = 8589934593;
        m.Put(a, 100);
        m.Put(b, 200);
        m.Put(c, 300);
        int total = 0;
        if (m.Count == 3) { total = total + 1; }
        if (m.Get(a) == 100) { total = total + 10; }
        if (m.Get(b) == 200) { total = total + 100; }
        if (m.Get(c) == 300) { total = total + 1000; }
        // Overwrite through a collision bucket.
        m.Put(b, 250);
        if (m.Get(b) == 250) { total = total + 10000; }
        if (m.Count == 3) { total = total + 100000; }
        // Remove the middle colliding key; neighbors must survive.
        if (m.Remove(b)) { total = total + 1000000; }
        if (m.ContainsKey(b)) { total = total + 77777777; }
        if (m.Get(a) == 100) { total = total + 10000000; }
        if (m.Get(c) == 300) { total = total + 100000000; }
        return total;
    }
}
"#;
    assert_parity(src, 111111111);
}

/// Same collision construction for Set membership.
#[test]
fn colliding_set_elements_resolve_correctly() {
    let src = r#"
class Program {
    static int Main() {
        Set<int64> s = new Set<int64>();
        int64 a = 7;
        int64 b = 4294967303;
        int total = 0;
        if (s.Add(a)) { total = total + 1; }
        if (s.Add(b)) { total = total + 10; }
        if (s.Add(a)) { total = total + 55555; }
        if (s.Contains(a)) { total = total + 100; }
        if (s.Contains(b)) { total = total + 1000; }
        if (s.Remove(a)) { total = total + 10000; }
        if (s.Contains(a)) { total = total + 55555; }
        if (s.Contains(b)) { total = total + 100000; }
        if (s.Count == 1) { total = total + 1000000; }
        return total;
    }
}
"#;
    assert_parity(src, 1111111);
}

/// value_eq treats 0.0 and -0.0 as equal, so they must be one map key
/// (this exercises the value_hash negative-zero normalization fix).
#[test]
fn float_zero_keys_are_one_key() {
    let src = r#"
class Program {
    static int Main() {
        Map<float, int> m = new Map<float, int>();
        float z = 0.0;
        float nz = -z;
        m.Put(z, 1);
        m.Put(nz, 2);
        int total = 0;
        if (m.Count == 1) { total = total + 1; }
        if (m.Get(z) == 2) { total = total + 10; }
        if (m.Get(nz) == 2) { total = total + 100; }
        Set<float> s = new Set<float>();
        s.Add(nz);
        if (s.Contains(z)) { total = total + 1000; }
        if (s.Add(z)) { total = total + 55555; }
        if (s.Count == 1) { total = total + 10000; }
        return total;
    }
}
"#;
    assert_parity(src, 11111);
}

/// Bulk churn where every bucket holds a colliding pair, with interleaved
/// removals — stresses index position fixups — and insertion-order Keys()
/// must survive it all. Runs hot under the JIT; 3 trials.
#[test]
fn collision_churn_parity_across_trials() {
    let src = r#"
class Program {
    static int Main() {
        Map<int64, int> m = new Map<int64, int>();
        int64 shift = 4294967296;
        int i = 0;
        while (i < 200) {
            m.Put(i, i);
            m.Put(i + shift, i * 2);
            i = i + 1;
        }
        // Remove every low twin; high twins must survive in order.
        int j = 0;
        while (j < 200) {
            m.Remove(j);
            j = j + 1;
        }
        int total = 0;
        List<int64> keys = m.Keys();
        int k = 0;
        while (k < keys.Count) {
            // Insertion order: high twins in ascending i order.
            if (keys.Get(k) == k + shift) { total = total + 1; }
            total = total + m.Get(keys.Get(k));
            k = k + 1;
        }
        if (m.Count == 200) { total = total + 100000; }
        return total;
    }
}
"#;
    // sum(i*2, 0..199) = 39800, +200 order checks, +100000 count check.
    for trial in 0..3 {
        let interp = run(src, false).expect("interpreter should run");
        assert_eq!(interp, 140000, "trial {trial} interpreter");
        let jit = run(src, true).expect("jit should run");
        assert_eq!(jit, interp, "trial {trial} jit");
    }
}
