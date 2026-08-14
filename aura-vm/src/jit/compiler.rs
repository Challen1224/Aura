//! Tiered-JIT driver: compiles hot methods and runs them on the native stack.
//!
//! [`Jit`] owns the cache of compiled methods (keyed by method id and the
//! overflow-checking mode, since both affect codegen). [`CompiledMethod::run`]
//! sets up a [`NativeFrame`] on the Rust stack, passes it to the generated
//! entry point in `rdi` together with the argument array in `rsi` and its
//! length in `rdx`, then translates the returned status into a [`FrameResult`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aura_bytecode::{MethodId, Module, Op, Value};
use crate::{Frame, Vm, VmError};

use super::codegen::emit_function;
use super::helpers::{dispatcher_addr, NativeFrame};
use super::ir::Lowerer;
use super::mem::JitFunction;
use super::regalloc::allocate;
use super::{CompileError, FrameResult};

/// Default number of interpreter invocations before a method is compiled.
pub const JIT_THRESHOLD: u64 = 100;

/// Compilation cache and count of compiled methods.
pub struct Jit {
    cache: Mutex<HashMap<(MethodId, bool), Arc<CompiledMethod>>>,
    compiled_count: usize,
}

impl Jit {
    /// Create an empty JIT.
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            compiled_count: 0,
        }
    }

    /// Return the compiled version of `method_id`, if present in the cache.
    pub fn get(&self, method_id: MethodId, overflow_checks: bool) -> Option<Arc<CompiledMethod>> {
        self.cache
            .lock()
            .unwrap()
            .get(&(method_id, overflow_checks))
            .cloned()
    }

    /// Compile `method_id` if it is not already cached.
    pub fn compile(
        &mut self,
        module: &Module,
        method_id: MethodId,
        overflow_checks: bool,
    ) -> Result<Arc<CompiledMethod>, CompileError> {
        if let Some(c) = self.get(method_id, overflow_checks) {
            return Ok(c);
        }
        let mut func = Lowerer::new(module, method_id, overflow_checks)
            .and_then(|l| l.lower())
            .map_err(CompileError::Unsupported)?;
        super::ir::optimize(&mut func);
        if std::env::var("AURA_DUMP_IR").is_ok() {
            eprintln!("=== IR for method {method_id:?} ===\n{}", super::ir::dump(&func));
            eprintln!("=== local_types={:?} helper_ops={:?}", func.local_types, func.helper_ops);
        }
        let alloc = allocate(&mut func);
        let code = emit_function(&func, &alloc).map_err(CompileError::Unsupported)?;
        let exec = JitFunction::new(code).map_err(|_| CompileError::OutOfMemory)?;
        let compiled = Arc::new(CompiledMethod {
            method_id,
            locals_count: func.locals,
            helpers: func.helper_ops,
            code: exec,
        });
        self.cache
            .lock()
            .unwrap()
            .insert((method_id, overflow_checks), compiled.clone());
        self.compiled_count += 1;
        Ok(compiled)
    }

    /// Number of distinct methods compiled so far.
    pub fn compiled_count(&self) -> usize {
        self.compiled_count
    }

    /// Drop all compiled code and reset the counter.
    pub fn clear(&mut self) {
        self.cache.lock().unwrap().clear();
        self.compiled_count = 0;
    }
}

/// A single JIT-compiled method.
pub struct CompiledMethod {
    method_id: MethodId,
    locals_count: u16,
    /// Ops the generated code dispatches to by index (`NativeFrame.helpers`).
    helpers: Vec<Op>,
    code: JitFunction,
}

impl CompiledMethod {
    /// The method this was compiled from.
    pub fn method_id(&self) -> MethodId {
        self.method_id
    }

    /// Run the compiled method with the given arguments.
    ///
    /// A placeholder frame is pushed while the native code executes so stack
    /// traces taken by helpers include this method. The frame is popped before
    /// this returns.
    pub fn run(&self, vm: &mut Vm, args: &[Value]) -> Result<FrameResult, VmError> {
        vm.call_stack.push(Frame::for_jit(self.method_id, self.locals_count));

        // The prologue copies these boxes into local slots as raw qwords, so
        // they must be in canonical slot form (see `value::canonicalize`).
        let mut argv: Vec<Value> = args.to_vec();
        for v in argv.iter_mut() {
            unsafe { super::value::canonicalize(v) };
        }
        let mut nf = NativeFrame {
            vm: vm as *mut Vm,
            op_id: 0,
            arg_base: std::ptr::null(),
            arg_count: 0,
            result: Value::Unit,
            method_id: self.method_id,
            helpers: &self.helpers as *const Vec<Op>,
            dispatcher: dispatcher_addr(),
        };

        let status = unsafe {
            self.code.call_abi(&[
                &mut nf as *mut NativeFrame as u64,
                argv.as_ptr() as u64,
                argv.len() as u64,
            ])
        };

        vm.call_stack.pop();

        if let Some(e) = vm.jit_error.take() {
            return Err(e);
        }

        let result = nf.result;
        match status {
            super::helpers::STATUS_OK => Ok(FrameResult::Normal(result)),
            super::helpers::STATUS_EXCEPTION => Ok(FrameResult::Exception(result)),
            other => Err(VmError::Runtime(format!("jit: unknown return status {other}"))),
        }
    }
}
