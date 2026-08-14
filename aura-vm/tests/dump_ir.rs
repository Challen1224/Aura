//! Scratch: dump the IR the JIT builds for the linked-list repro.

use aura_bytecode::{MethodId, Value};
use aura_compiler::compile;
use aura_vm::jit::ir::{dump, Lowerer};
use aura_vm::Vm;
use std::sync::Arc;

const LINKED_LIST: &str = r#"
class Node {
    int value;
    Node? next;
    Node(int v) { this.value = v; this.next = null; }
}
class Program {
    static int Main() {
        Node a = new Node(1);
        Node b = new Node(2);
        a.next = b;
        int steps = 0;
        int i = 0;
        while (i < 300) {
            Node? cur = a;
            while (cur != null) {
                steps = steps + 1;
                cur = cur.next;
            }
            i = i + 1;
        }
        return steps;
    }
}
"#;

#[test]
fn dump_ir() {
    let module = Arc::new(compile(LINKED_LIST, "test").unwrap());
    let entry = module.entrypoint.unwrap();
    let mut vm = Vm::new(module.clone());
    vm.enable_jit();
    vm.set_jit_threshold(1);
    vm.run().unwrap();
    println!("entry = {entry:?}");
    let lowered = Lowerer::new(&module, entry, false).unwrap().lower().unwrap();
    println!("{}", dump(&lowered));
    println!("local_types={:?}", lowered.local_types);
    println!("helper_ops={:?}", lowered.helper_ops);
    println!("main value = {:?}", Value::Int(0));
}
