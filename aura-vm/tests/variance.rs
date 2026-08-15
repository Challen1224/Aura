//! Variance annotations: `interface IProducer<out T>` (covariant) and
//! `interface IComparer<in T>` (contravariant). This feature also made
//! generic-interface implementation real: `class CatFactory :
//! IProducer<Cat>` declares an instantiation, conformance is checked at it
//! (interface signatures substituted), and assignability compares declared
//! instantiations variance-aware — `out` widens, `in` narrows, unannotated
//! parameters must match exactly. Soundness: `out T` may not occur
//! (anywhere, however nested) in parameter types; `in T` may not occur in
//! return types; annotations are interface-only.

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

fn assert_compile_fails(source: &str, needle: &str) {
    match compile(source, "test") {
        Ok(_) => panic!("expected compile error containing `{needle}`"),
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains(needle),
                "expected error containing `{needle}`, got: {msg}"
            );
        }
    }
}

const ZOO: &str = r#"
class Animal {
    virtual int legs() { return 4; }
}
class Cat : Animal {
    override int legs() { return 40; }
}
interface IProducer<out T> {
    T produce();
}
interface IComparer<in T> {
    int compare(T a, T b);
}
interface IBox<T> {
    T get();
    void put(T v);
}
class CatFactory : IProducer<Cat> {
    Cat produce() { return new Cat(); }
}
class LegComparer : IComparer<Animal> {
    int compare(Animal a, Animal b) { return a.legs() - b.legs(); }
}
class CatBox : IBox<Cat> {
    Cat item;
    CatBox(Cat c) { this.item = c; }
    Cat get() { return this.item; }
    void put(Cat v) { this.item = v; }
}
"#;

/// Covariant widening, contravariant narrowing, dispatch through both, and
/// exact invariant use — under both tiers.
#[test]
fn variance_directions_work() {
    let src = format!(
        "{ZOO}
class Program {{
    static int rate(IComparer<Cat> c) {{ return c.compare(new Cat(), new Cat()); }}
    static int Main() {{
        IProducer<Cat> pc = new CatFactory();
        IProducer<Animal> pa = pc;
        int total = pa.produce().legs();
        IComparer<Animal> ca = new LegComparer();
        IComparer<Cat> cc = ca;
        total = total + cc.compare(new Cat(), new Cat()) + 1;
        total = total + Program.rate(ca) * 100 + 200;
        IBox<Cat> box = new CatBox(new Cat());
        box.put(new Cat());
        total = total + box.get().legs() * 1000;
        return total;
    }}
}}"
    );
    // 40 + 0 + 1 + 0 + 200 + 40000 = 40241.
    assert_parity(&src, 40241);
}

/// Generic-interface conformance is checked at the declared instantiation,
/// including generic implementors sharing their own parameter
/// (`class Pipe<E> : IProducer<E>`).
#[test]
fn generic_implementor_conformance() {
    let src = r#"
interface IProducer<out T> {
    T produce();
}
class Repeat<E> : IProducer<E> {
    E v;
    Repeat(E v) { this.v = v; }
    E produce() { return this.v; }
}
class Program {
    static int Main() {
        IProducer<int> p = new Repeat<int>(21);
        IProducer<string> q = new Repeat<string>("abcde");
        return p.produce() * 10 + q.produce().Length;
    }
}
"#;
    assert_parity(src, 215);
}

/// Every direction and soundness rule, pinned.
#[test]
fn variance_rules_are_enforced() {
    let prog = "class Program { static void Main() { } }";
    let cases: Vec<(String, &str)> = vec![
        // Wrong-direction assignments.
        (
            format!(
                "{ZOO} class Program {{ static void Main() {{ \
                 IProducer<Animal> pa = new CatFactory(); IProducer<Cat> pc = pa; }} }}"
            ),
            "cannot assign IProducer<Animal>",
        ),
        (
            format!(
                "{ZOO} class Program {{ static void Main() {{ \
                 IComparer<Cat> cc = new LegComparer(); IComparer<Animal> ca = cc; }} }}"
            ),
            "cannot assign IComparer<Cat>",
        ),
        // Unannotated parameters are invariant across instantiations.
        (
            format!(
                "{ZOO} class Program {{ static void Main() {{ \
                 IBox<Animal> ba = new CatBox(new Cat()); }} }}"
            ),
            "cannot assign CatBox",
        ),
        // Soundness: out in input positions (direct and nested).
        (
            format!("interface IBad<out T> {{ void put(T v); }} {prog}"),
            "covariant `out T` cannot appear in parameter",
        ),
        (
            format!("interface IBad<out T> {{ void fill(List<T> xs); }} {prog}"),
            "covariant `out T` cannot appear in parameter",
        ),
        // Soundness: in as output.
        (
            format!("interface IBad<in T> {{ T get(); }} {prog}"),
            "contravariant `in T` cannot appear in the return type",
        ),
        // Interface-only.
        (
            format!("class Bad<out T> {{ T v; }} {prog}"),
            "only allowed on interface type parameters",
        ),
        (
            format!("class C {{ static <out T> T id(T v) {{ return v; }} }} {prog}"),
            "only allowed on interface type parameters",
        ),
        // A generic interface needs its instantiation declared.
        (
            format!(
                "interface IP<out T> {{ T produce(); }} \
                 class Bare : IP {{ int produce() {{ return 1; }} }} {prog}"
            ),
            "expects 1 type argument(s)",
        ),
        // Conformance is at the declared instantiation.
        (
            format!(
                "interface IP<out T> {{ T produce(); }} \
                 class Wrong : IP<int> {{ string produce() {{ return \"x\"; }} }} {prog}"
            ),
            "different signature",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// Variant interface dispatch in a JIT hot loop crossing the threshold
/// mid-run: producers widened to base, comparers narrowed to derived.
/// 3 trials, compiled count asserted.
#[test]
fn variance_in_jit_hot_loop_trials() {
    let src = format!(
        "{ZOO}
class Program {{
    static int Main() {{
        IProducer<Animal> pa = new CatFactory();
        IComparer<Cat> cc = new LegComparer();
        int total = 0;
        for (var i in 1..=200) {{
            total = total + pa.produce().legs() + cc.compare(new Cat(), new Cat()) + i;
        }}
        return total;
    }}
}}"
    );
    // 200*40 + 0 + sum(1..200) = 8000 + 20100 = 28100.
    for trial in 0..3 {
        assert_parity(&src, 28100);
        let module = Arc::new(compile(&src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 28100, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
