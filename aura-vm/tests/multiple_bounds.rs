//! Generic constraints with multiple bounds: `where T : Entity,
//! ICloneable` — the type argument must satisfy every bound, and members
//! of every bound are callable on `T` values (the first bound declaring a
//! method answers). Bounds may combine with `new()`; a union constraint
//! may not combine with anything.

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
class Entity {
    int id;
    Entity(int id) { this.id = id; }
    int getId() { return this.id; }
}
interface ICloneable {
    int cloneTag();
}
interface INamed {
    string name();
}
class User : Entity, ICloneable, INamed {
    User(int id) : super(id) { }
    int cloneTag() { return 77; }
    string name() { return "user"; }
}
"#;

/// The TODO example: a class bound plus interface bounds on classes and
/// methods, with members of *each* bound used on `T` — including chained
/// calls on a bound's result (`v.name().Length`).
#[test]
fn multiple_bounds_work() {
    let src = format!(
        "{ZOO}
class Repository<T> where T : Entity, ICloneable {{
    T item;
    Repository(T item) {{ this.item = item; }}
    int describe() {{ return this.item.getId() * 100 + this.item.cloneTag(); }}
}}
class Program {{
    static <T> int triple(T v) where T : Entity, ICloneable, INamed {{
        return v.getId() + v.cloneTag() + v.name().Length;
    }}
    static int Main() {{
        Repository<User> r = new Repository<User>(new User(5));
        int total = r.describe();
        total = total + Program.triple(new User(10)) * 10000;
        return total;
    }}
}}"
    );
    // 577 + 91*10000 = 910577.
    assert_parity(&src, 910577);
}

/// Bounds may combine with `new()`, and all parts are enforced.
#[test]
fn bounds_combine_with_new() {
    let src = format!(
        "{ZOO}
class Blank : ICloneable {{
    int v;
    int cloneTag() {{ return 3; }}
}}
class Pool<T> where T : ICloneable, new() {{
    int tag;
    Pool(int tag) {{ this.tag = tag; }}
}}
class Program {{
    static int Main() {{
        Pool<Blank> p = new Pool<Blank>(9);
        return p.tag;
    }}
}}"
    );
    assert_parity(&src, 9);
}

/// Each bound is enforced independently, member lookup failures name the
/// bounds, and unions cannot join a bound list.
#[test]
fn multiple_bound_rules() {
    let prog = "class Program { static void Main() { } }";
    let cases: Vec<(String, &str)> = vec![
        // Missing the class bound.
        (
            format!(
                "{ZOO} class Plain {{ int x; Plain(int x) {{ this.x = x; }} \
                 int cloneTag() {{ return 1; }} }} \
                 class Repo<T> where T : Entity, ICloneable {{ T v; Repo(T v) {{ this.v = v; }} }} \
                 class Program {{ static void Main() {{ Repo<Plain> r = null; }} }}"
            ),
            "does not satisfy constraint Entity",
        ),
        // Missing an interface bound.
        (
            format!(
                "{ZOO} class OnlyE : Entity {{ OnlyE(int i) : super(i) {{ }} }} \
                 class Repo<T> where T : Entity, ICloneable {{ T v; Repo(T v) {{ this.v = v; }} }} \
                 class Program {{ static void Main() {{ Repo<OnlyE> r = null; }} }}"
            ),
            "does not satisfy constraint ICloneable",
        ),
        // A method no bound declares.
        (
            format!(
                "{ZOO} class Repo<T> where T : Entity, ICloneable {{ T v; Repo(T v) {{ this.v = v; }} \
                 int bad() {{ return this.v.vanish(); }} }} {prog}"
            ),
            "no bound declares it",
        ),
        // Unions cannot combine with bounds.
        (
            format!("class C<T> where T : int | float, string {{ int v; }} {prog}"),
            "cannot be combined with other constraints",
        ),
        (
            format!("class C<T> where T : Entity, int | float {{ int v; }} {ZOO} {prog}"),
            "cannot be combined with other constraints",
        ),
        // Method-level inference checks every bound too.
        (
            format!(
                "{ZOO} class OnlyE : Entity {{ OnlyE(int i) : super(i) {{ }} }} \
                 class U {{ static <T> int f(T v) where T : Entity, ICloneable {{ \
                 return v.getId(); }} }} \
                 class Program {{ static void Main() {{ U.f(new OnlyE(1)); }} }}"
            ),
            "does not satisfy constraint ICloneable",
        ),
    ];
    for (src, needle) in &cases {
        assert_compile_fails(src, needle);
    }
}

/// Multi-bound member dispatch in a JIT hot loop crossing the threshold
/// mid-run. 3 trials, compiled count asserted.
#[test]
fn multiple_bounds_in_jit_hot_loop_trials() {
    let src = format!(
        "{ZOO}
class Program {{
    static <T> int weight(T v) where T : Entity, ICloneable {{
        return v.getId() + v.cloneTag();
    }}
    static int Main() {{
        int total = 0;
        for (var i in 1..=200) {{
            total = total + Program.weight(new User(i));
        }}
        return total;
    }}
}}"
    );
    // sum(1..200) + 200*77 = 20100 + 15400 = 35500.
    for trial in 0..3 {
        assert_parity(&src, 35500);
        let module = Arc::new(compile(&src, "test").expect("compile"));
        let mut vm = Vm::new(module);
        vm.enable_jit();
        vm.set_jit_threshold(60);
        match vm.run().expect("jit threshold-60 run") {
            Value::Int(i) => assert_eq!(i, 35500, "trial {trial} tier transition"),
            other => panic!("expected int, got {other:?}"),
        }
        #[cfg(target_arch = "x86_64")]
        assert!(vm.jit_compiled_count() > 0, "trial {trial}: nothing compiled");
    }
}
