//! Weak references: `WeakRef<T>` observes an object without keeping it
//! alive. Handles are allocated monotonically and never reused, so
//! liveness is exactly heap membership — no sweep-phase clearing exists
//! in either generation, and a dead target can never be confused with a
//! recycled handle. `isAlive()` reports liveness; `get()` returns the
//! target or null once it has been collected.
//!
//! Reclamation promptness caveat: the JIT roots its frames by scanning
//! slot memory conservatively, so an object can outlive its last strong
//! reference while a compiled frame that ever handled it is still on the
//! stack. The guarantee both tiers share is the weak one that matters: a
//! weak reference alone never keeps its target alive once every frame
//! that touched the target has exited. Tests therefore create doomed
//! targets in helper frames that return before the collection they are
//! measured against — exactly how weak-ref code behaves in practice.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::Vm;
use std::sync::Arc;

fn run_gc(source: &str, jit: bool, threshold: usize) -> i64 {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(threshold);
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
    result
}

fn assert_parity_gc(source: &str, expected: i64, threshold: usize) {
    assert_eq!(run_gc(source, false, threshold), expected, "interpreter");
    assert_eq!(run_gc(source, true, threshold), expected, "jit");
}

/// The core contract: a weak reference does not root its target. Once the
/// last strong reference is gone and the collector has run, `isAlive()`
/// flips to false and `get()` returns null — while an identical weak
/// reference whose target stays strongly held keeps answering with the
/// live object. 3 trials per tier.
#[test]
fn weak_does_not_keep_target_alive() {
    let src = r#"
class Data { string tag; Data(string tag) { this.tag = tag; } }
class Program {
    static int pad(int i) { return i - i; }
    static WeakRef<Data> makeDoomed() {
        // This frame is the only one to ever hold the target; it exits
        // before any collection the test measures.
        return new WeakRef(new Data("doomed"));
    }
    static int Main() {
        Data strong = new Data("held");
        var alive = new WeakRef(strong);
        var doomed = Program.makeDoomed();
        int churn = 0;
        for (var i in 1..=800) {
            string junk = "junk {i} padding padding padding";
            churn = churn + junk.Length - junk.Length + Program.pad(i);
        }
        int score = churn;
        if (doomed.isAlive()) { score = score + 1; }
        if (doomed.get() != null) { score = score + 2; }
        if (alive.isAlive()) { score = score + 4; }
        var got = alive.get();
        if (got != null) { score = score + got.tag.Length; }
        // Keep `strong` live past the checks.
        score = score + strong.tag.Length;
        return score;
    }
}
"#;
    // 4 (alive) + 4 ("held") + 4 ("held") = 12; doomed contributes nothing.
    for _ in 0..3 {
        assert_parity_gc(src, 12, 4096);
    }
}

/// A weak target that survives promotion into the old generation and a
/// major collection stays reachable through the weak reference; dropping
/// the strong root afterwards still lets a later collection reclaim it.
#[test]
fn weak_across_generations() {
    let src = r#"
class Data { string tag; Data(string tag) { this.tag = tag; } }
class Program {
    static Data? pinned;
    static int pad(int i) { return i - i; }
    static int churn(int rounds) {
        int total = 0;
        for (var i in 1..=rounds) {
            string junk = "junk {i} padding padding padding";
            total = total + junk.Length - junk.Length + Program.pad(i);
        }
        return total;
    }
    static WeakRef<Data> setup() {
        // The target's only lasting root is the static: this frame's
        // locals die with the call.
        Data d = new Data("promoted");
        Program.pinned = d;
        return new WeakRef(d);
    }
    static int probeAlive(WeakRef<Data> weak) {
        // Touches the live target in its own frame, which exits before
        // the reclamation phase.
        int score = 0;
        if (weak.isAlive()) { score = score + 1; }
        var got = weak.get();
        if (got != null) { score = score + got.tag.Length; }
        return score;
    }
    static int Main() {
        var weak = Program.setup();
        int score = Program.churn(800);
        // Target promoted through minors (and possibly a major): alive.
        score = score + Program.probeAlive(weak);
        // Drop the only strong root. Minor collections never touch the
        // old generation, so force a major with retained pressure.
        Program.pinned = null;
        var press = new List<string>();
        for (var i in 1..=300) {
            press.Add("retained pressure {i} padding padding");
        }
        score = score + Program.churn(800) + press.Count - 300;
        if (weak.isAlive()) { score = score + 100; }
        if (weak.get() == null) { score = score + 10; }
        return score;
    }
}
"#;
    // 1 + 8 ("promoted") + 10 (reclaimed) = 19.
    assert_parity_gc(src, 19, 4096);
}

/// Construction-site inference: `new WeakRef(obj)` infers `T` from the
/// argument, so `get()` comes back as `T?` and members are usable after
/// a null-check without any annotation.
#[test]
fn constructor_inference() {
    let src = r#"
class Data { int n; Data(int n) { this.n = n; } }
class Program {
    static int pad(int i) { return i - i; }
    static int Main() {
        int warm = 0;
        for (var i in 1..=100) { warm = warm + Program.pad(i); }
        Data d = new Data(41);
        var weak = new WeakRef(d);
        var got = weak.get();
        int r = 0;
        if (got != null) { r = got.n + 1; }
        return r + d.n - d.n + warm;
    }
}
"#;
    assert_parity_gc(src, 42, 64 * 1024);
}

/// A weak reference is itself an ordinary object: it participates in
/// collection (an unrooted WeakRef dies) and stays valid when only it —
/// not its target — survives churn inside a data structure.
#[test]
fn weakref_inside_structure() {
    let src = r#"
class Data { string tag; Data(string tag) { this.tag = tag; } }
class Program {
    static int pad(int i) { return i - i; }
    static void fill(List<WeakRef<Data>> cache, Data keep) {
        cache.Add(new WeakRef(keep));
        cache.Add(new WeakRef(new Data("transient")));
    }
    static int Main() {
        var cache = new List<WeakRef<Data>>();
        Data keep = new Data("keeper");
        Program.fill(cache, keep);
        int churn = 0;
        for (var i in 1..=800) {
            string junk = "junk {i} padding padding padding";
            churn = churn + junk.Length - junk.Length + Program.pad(i);
        }
        int liveCount = 0;
        for (var w in cache) {
            if (w.isAlive()) { liveCount = liveCount + 1; }
        }
        return churn + liveCount * 10 + keep.tag.Length;
    }
}
"#;
    // One of two cache entries alive: 10 + 6 ("keeper") = 16.
    assert_parity_gc(src, 16, 4096);
}

/// Passing a non-object where the target must be an object is rejected —
/// the typer stops `new WeakRef(5)` only if T=int fails the runtime
/// object requirement, which surfaces as a runtime error naming the
/// native and the offending type.
#[test]
fn non_object_target_is_an_error() {
    let src = r#"
class Program {
    static int Main() {
        var weak = new WeakRef(5);
        return 0;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    let err = vm.run().expect_err("int target should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("weak reference target must be an object"),
        "got: {msg}"
    );
}
