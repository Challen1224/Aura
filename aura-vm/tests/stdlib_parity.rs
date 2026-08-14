//! Stdlib native parity tests: every program runs under the interpreter and
//! the JIT (threshold 1, so hot methods compile immediately) and both results
//! must match an independently computed expected value. On non-x86-64 hosts
//! the JIT is a stub and the "jit" run falls back to the interpreter, so these
//! only exercise generated code when built for x86-64.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::{Vm, VmError};
use std::sync::Arc;

fn run(source: &str, jit: bool) -> Result<(i64, usize), VmError> {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    if jit {
        vm.enable_jit();
        vm.set_jit_threshold(1);
    }
    let result = vm.run()?;
    let compiled = vm.jit_compiled_count();
    match result {
        Value::Int(i) => Ok((i, compiled)),
        other => panic!("expected int, got {other:?}"),
    }
}

/// Run under interpreter and JIT and assert both equal `expected`.
fn assert_parity(source: &str, expected: i64) {
    let (interp, _) = run(source, false).expect("interpreter should run");
    assert_eq!(interp, expected, "interpreter result");
    let (jit, compiled) = run(source, true).expect("jit should run");
    assert_eq!(jit, interp, "jit must match interpreter");
    // A silent fall-back to the interpreter would make this test vacuous:
    // require that at least one method actually compiled on JIT-capable
    // targets.
    #[cfg(target_arch = "x86_64")]
    assert!(
        compiled > 0,
        "expected the JIT to compile at least one method (got {compiled})"
    );
    #[cfg(not(target_arch = "x86_64"))]
    let _ = compiled;
}

#[test]
fn list_hot_loop() {
    // Build a list in a loop, then mutate and reduce it.
    let src = r#"
class Program {
    static int Main() {
        List<int> xs = new List<int>();
        int i = 0;
        while (i < 200) {
            xs.Add(i);
            i = i + 1;
        }
        xs.Set(0, 1000);
        xs.Insert(1, 500);
        int dropped = xs.RemoveAt(2);
        int total = 0;
        int j = 0;
        while (j < xs.Count) {
            total = total + xs.Get(j);
            j = j + 1;
        }
        return total + dropped + xs.IndexOf(500);
    }
}
"#;
    // sum(0..200) = 19900; -0 (replaced by 1000) +1000, +500 inserted,
    // remove former index 1 (value 1) => 19900 - 0 + 1000 + 500 - 1 = 21399.
    // dropped = 1, IndexOf(500) = 1 => 21399 + 1 + 1 = 21401.
    assert_parity(src, 21401);
}

#[test]
fn map_hot_loop() {
    let src = r#"
class Program {
    static int Main() {
        Map<int, int> squares = new Map<int, int>();
        int i = 0;
        while (i < 100) {
            squares.Put(i, i * i);
            i = i + 1;
        }
        squares.Put(3, 42);
        bool removed = squares.Remove(4);
        int total = 0;
        List<int> keys = squares.Keys();
        int j = 0;
        while (j < keys.Count) {
            total = total + squares.Get(keys.Get(j));
            j = j + 1;
        }
        if (removed) { total = total + 7; }
        if (squares.ContainsKey(4)) { total = total + 1000000; }
        return total + squares.Count;
    }
}
"#;
    // sum(i^2, i=0..99) = 328350; -9 +42 (key 3), -16 (key 4 removed),
    // +7 (removed), +99 (count) = 328473.
    assert_parity(src, 328473);
}

#[test]
fn set_hot_loop() {
    let src = r#"
class Program {
    static int Main() {
        Set<int> seen = new Set<int>();
        int added = 0;
        int i = 0;
        while (i < 300) {
            if (seen.Add(i - (i / 3) * 3)) {
                added = added + 1;
            }
            i = i + 1;
        }
        seen.Remove(1);
        List<int> rest = seen.ToList();
        int total = 0;
        int j = 0;
        while (j < rest.Count) {
            total = total + rest.Get(j);
            j = j + 1;
        }
        if (seen.Contains(2)) { total = total + 10; }
        return total * 100 + added * 10 + seen.Count;
    }
}
"#;
    // i mod 3 cycles {0,1,2}: added = 3; after Remove(1) the set is {0,2},
    // total = 2 + 10 = 12 -> 12*100 + 3*10 + 2 = 1232.
    assert_parity(src, 1232);
}

#[test]
fn string_methods_hot_loop() {
    let src = r#"
class Program {
    static int Main() {
        string csv = "10,20,30,40";
        int total = 0;
        int i = 0;
        while (i < 50) {
            List<string> parts = csv.Split(",");
            int j = 0;
            while (j < parts.Count) {
                total = total + parts.Get(j).ToInt();
                j = j + 1;
            }
            i = i + 1;
        }
        string s = "Hello, Aura!";
        total = total + s.Length;
        total = total + s.IndexOf("Aura");
        total = total + s.Substring(7, 4).Length;
        if (s.ToUpper().StartsWith("HELLO")) { total = total + 3; }
        if (s.Replace("Aura", "x").EndsWith("x!")) { total = total + 4; }
        return total;
    }
}
"#;
    // 50 * 100 = 5000; +12 (Length) +7 (IndexOf) +4 (Substring len)
    // +3 +4 = 5030.
    assert_parity(src, 5030);
}

#[test]
fn collections_of_objects() {
    // Lists holding class instances, virtual calls on retrieved elements,
    // and a map keyed by strings.
    let src = r#"
class Shape {
    int side;
    Shape(int s) { this.side = s; }
    int Area() { return this.side * this.side; }
}
class Square : Shape {
    Square(int s) : super(s) {}
}
class Program {
    static int Main() {
        List<Shape> shapes = new List<Shape>();
        int i = 1;
        while (i <= 20) {
            shapes.Add(new Shape(i));
            shapes.Add(new Square(i));
            i = i + 1;
        }
        int total = 0;
        int j = 0;
        while (j < shapes.Count) {
            total = total + shapes.Get(j).Area();
            j = j + 1;
        }
        Map<string, int> tally = new Map<string, int>();
        tally.Put("total", total);
        return tally.Get("total");
    }
}
"#;
    // 2 * sum(i^2, 1..20) = 2 * 2870 = 5740.
    assert_parity(src, 5740);
}

#[test]
fn parity_is_stable_across_runs() {
    // Register-allocation bugs are often non-deterministic; run a mixed
    // workload several times under both tiers.
    let src = r#"
class Program {
    static int Main() {
        Map<string, int> counts = new Map<string, int>();
        string text = "the quick brown fox the lazy dog the end";
        List<string> words = text.Split(" ");
        int i = 0;
        while (i < words.Count) {
            string w = words.Get(i);
            if (counts.ContainsKey(w)) {
                counts.Put(w, counts.Get(w) + 1);
            } else {
                counts.Put(w, 1);
            }
            i = i + 1;
        }
        return counts.Get("the") * 100 + counts.Count;
    }
}
"#;
    for _ in 0..5 {
        assert_parity(src, 307);
    }
}
