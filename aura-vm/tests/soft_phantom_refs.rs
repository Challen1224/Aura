//! Soft and phantom references.
//!
//! `SoftRef<T>` *traces* its target — unlike a weak reference it keeps it
//! alive — until the collector clears it under memory pressure. Pressure
//! in this VM means a hard heap limit: when a full collection still
//! leaves the heap over `--gc-max-heap`, every soft reference is cleared
//! and the collection re-runs before a heap-limit error is considered
//! (softly-held memory is the last thing to go before out-of-memory).
//! With no limit configured there is no pressure signal, so soft
//! references behave like strong ones — documented, not implied away.
//!
//! `PhantomRef<T>` is untraced like a weak reference, but the target can
//! never be retrieved: there is no `get()`, only `isReclaimed()`. The
//! language has no finalizers or reference queues, so detection is
//! poll-based. The same JIT promptness caveat as weak references applies
//! (conservative frame scans), so doomed targets are created in helper
//! frames that exit before the collection they are measured against.

use aura_bytecode::Value;
use aura_compiler::compile;
use aura_vm::Vm;
use std::sync::Arc;

fn run_configured(source: &str, jit: bool, configure: impl Fn(&mut Vm)) -> i64 {
    let module = Arc::new(compile(source, "test").expect("compile"));
    let mut vm = Vm::new(module);
    configure(&mut vm);
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

fn assert_parity(source: &str, expected: i64, configure: impl Fn(&mut Vm)) {
    assert_eq!(
        run_configured(source, false, &configure),
        expected,
        "interpreter"
    );
    assert_eq!(run_configured(source, true, &configure), expected, "jit");
}

/// Without a heap limit there is no memory pressure, so a soft reference
/// keeps its target alive through heavy churn and major collections even
/// with no strong root anywhere. 3 trials per tier.
#[test]
fn soft_keeps_target_alive_without_pressure() {
    let src = r#"
class Data { string tag; Data(string tag) { this.tag = tag; } }
class Program {
    static int pad(int i) { return i - i; }
    static SoftRef<Data> makeSoft() {
        // No strong root survives this frame; only the soft reference
        // keeps the target reachable.
        return new SoftRef(new Data("soft-held"));
    }
    static int Main() {
        var soft = Program.makeSoft();
        int churn = 0;
        for (var i in 1..=800) {
            string junk = "junk {i} padding padding padding";
            churn = churn + junk.Length - junk.Length + Program.pad(i);
        }
        // Force majors too: retained pressure escalates past the nursery.
        var press = new List<string>();
        for (var i in 1..=300) {
            press.Add("retained pressure {i} padding padding");
        }
        int score = churn + press.Count - 300;
        if (soft.isAlive()) { score = score + 1; }
        var got = soft.get();
        if (got != null) { score = score + got.tag.Length; }
        return score;
    }
}
"#;
    // 1 + 9 ("soft-held") = 10.
    for _ in 0..3 {
        assert_parity(src, 10, |vm| vm.set_gc_threshold(4096));
    }
}

/// Under a hard heap limit, memory pressure clears the soft reference
/// instead of erroring: the program completes with its strong data
/// intact and the soft target reclaimed.
#[test]
fn soft_cleared_under_pressure() {
    let src = r#"
class Blob {
    List<string> data;
    Blob(int n) {
        this.data = new List<string>();
        for (var i in 1..=n) { this.data.Add("blob payload {i} padding padding padding"); }
    }
}
class Program {
    static int pad(int i) { return i - i; }
    static SoftRef<Blob> makeSoft() { return new SoftRef(new Blob(60)); }
    static int Main() {
        var soft = Program.makeSoft();
        int score = 0;
        if (soft.isAlive()) { score = score + 1; }
        // Strong retention pushes the heap toward the limit; the strong
        // data alone fits, strong + soft target does not.
        var keep = new List<string>();
        for (var i in 1..=150) {
            keep.Add("retained {i} padding padding padding");
            score = score + Program.pad(i);
        }
        if (soft.isAlive()) { score = score + 100; }
        if (soft.get() == null) { score = score + 10; }
        return score + keep.Count;
    }
}
"#;
    // 1 (alive before) + 10 (cleared after) + 150 = 161.
    assert_parity(src, 161, |vm| {
        vm.set_gc_threshold(4096);
        vm.set_gc_max_heap(Some(8 * 1024));
    });
}

/// Clearing soft references is a reprieve, not an exemption: when the
/// strongly-held data alone exceeds the limit, the heap-limit error
/// still fires after softs are gone.
#[test]
fn strong_overflow_still_errors_after_soft_clearing() {
    let src = r#"
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
        for (var i in 1..=1500) {
            keep.Add("retained {i} padding padding padding");
        }
        return keep.Count;
    }
}
"#;
    let module = Arc::new(compile(src, "test").expect("compile"));
    let mut vm = Vm::new(module);
    vm.set_gc_threshold(4096);
    vm.set_gc_max_heap(Some(8 * 1024));
    let err = vm.run().expect_err("strong data alone exceeds the limit");
    assert!(
        format!("{err}").contains("heap limit exceeded"),
        "got: {err}"
    );
}

/// A phantom reference never roots its target and can only observe its
/// reclamation: `isReclaimed()` flips to true once the collector runs,
/// while a strongly-held target's phantom stays false.
#[test]
fn phantom_observes_reclamation() {
    let src = r#"
class Data { string tag; Data(string tag) { this.tag = tag; } }
class Program {
    static int pad(int i) { return i - i; }
    static PhantomRef<Data> makeDoomed() {
        // This frame is the only one to ever hold the target.
        return new PhantomRef(new Data("doomed"));
    }
    static int Main() {
        Data strong = new Data("held");
        var held = new PhantomRef(strong);
        var doomed = Program.makeDoomed();
        int churn = 0;
        for (var i in 1..=800) {
            string junk = "junk {i} padding padding padding";
            churn = churn + junk.Length - junk.Length + Program.pad(i);
        }
        int score = churn;
        if (doomed.isReclaimed()) { score = score + 1; }
        if (held.isReclaimed()) { score = score + 100; }
        score = score + strong.tag.Length;
        return score;
    }
}
"#;
    // 1 (doomed reclaimed) + 4 ("held") = 5.
    for _ in 0..3 {
        assert_parity(src, 5, |vm| vm.set_gc_threshold(4096));
    }
}

/// A phantom reference has no `get()`: the target is unrecoverable by
/// construction, enforced at compile time.
#[test]
fn phantom_has_no_get() {
    let src = r#"
class Data { int n; Data(int n) { this.n = n; } }
class Program {
    static int Main() {
        var p = new PhantomRef(new Data(1));
        var got = p.get();
        return 0;
    }
}
"#;
    let err = compile(src, "test").expect_err("get() must not exist");
    let msg = format!("{err}");
    assert!(
        msg.contains("get") && msg.contains("PhantomRef"),
        "got: {msg}"
    );
}

/// Construction-site inference works for both: `new SoftRef(obj)` and
/// `new PhantomRef(obj)` infer `T` from the argument, and non-object
/// targets are runtime errors naming the reference kind.
#[test]
fn inference_and_non_object_targets() {
    let src = r#"
class Data { int n; Data(int n) { this.n = n; } }
class Program {
    static int pad(int i) { return i - i; }
    static int Main() {
        int warm = 0;
        for (var i in 1..=100) { warm = warm + Program.pad(i); }
        Data d = new Data(41);
        var soft = new SoftRef(d);
        var phantom = new PhantomRef(d);
        var got = soft.get();
        int r = 0;
        if (got != null) { r = got.n + 1; }
        if (phantom.isReclaimed()) { r = r + 100; }
        return r + warm + d.n - d.n;
    }
}
"#;
    assert_parity(src, 42, |vm| vm.set_gc_threshold(64 * 1024));

    for (source, needle) in [
        (
            "class Program { static int Main() { var s = new SoftRef(5); return 0; } }",
            "soft reference target must be an object",
        ),
        (
            "class Program { static int Main() { var p = new PhantomRef(true); return 0; } }",
            "phantom reference target must be an object",
        ),
    ] {
        let module = Arc::new(compile(source, "test").expect("compile"));
        let mut vm = Vm::new(module);
        let err = vm.run().expect_err("non-object target should fail");
        let msg = format!("{err}");
        assert!(msg.contains(needle), "got: {msg}");
    }
}
