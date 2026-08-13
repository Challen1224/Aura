//! Placeholder JIT for non-x86_64 targets. Keeps the public API identical so
//! the VM compiles everywhere; compilation always reports
//! [`CompileError::Unsupported`].

use super::{CompileError, FrameResult};
use aura_bytecode::{MethodId, Module, Value};
use crate::VmError;
use std::sync::Arc;

/// Default compilation threshold (unused on this architecture).
pub const JIT_THRESHOLD: u64 = 100;

pub struct Jit;

impl Jit {
    pub fn new() -> Self {
        Jit
    }

    pub fn get(&self, _method_id: MethodId, _overflow_checks: bool) -> Option<Arc<CompiledMethod>> {
        None
    }

    pub fn compile(
        &mut self,
        _module: &Module,
        _method_id: MethodId,
        _overflow_checks: bool,
    ) -> Result<Arc<CompiledMethod>, CompileError> {
        Err(CompileError::Unsupported(
            "jit requires a x86_64 target".into(),
        ))
    }

    pub fn compiled_count(&self) -> usize {
        0
    }

    pub fn clear(&mut self) {}
}

pub struct CompiledMethod;

impl CompiledMethod {
    pub fn method_id(&self) -> MethodId {
        unreachable!("no compiled method on non-x86_64")
    }

    pub fn run(
        &self,
        _vm: &mut crate::Vm,
        _args: &[Value],
    ) -> Result<FrameResult, VmError> {
        Err(VmError::Runtime(
            "jit is not supported on this architecture".into(),
        ))
    }
}
