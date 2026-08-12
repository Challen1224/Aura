//! Aura stack-based virtual machine.
//!
//! The VM executes a [`Module`](aura_bytecode::Module) consisting of classes
//! and methods. It maintains a call stack of frames, an evaluation stack per
//! frame, a managed heap, and a mark-and-sweep GC.

#![warn(missing_docs)]

pub mod heap;

use aura_bytecode::{AuraObject, ClassId, EnumId, ExceptionHandler, GcRef, MethodDef, MethodId, Module, Op, TypeDesc, Value};
use heap::{Heap, HeapError};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// FNV-1a 32-bit hash of a string, used for string hashing.
fn fnv_hash(s: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for byte in s.as_bytes() {
        h ^= *byte as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h as i32
}

/// Default (zero) value for a field of the given type.
fn default_value(ty: &TypeDesc) -> Value {
    match ty {
        TypeDesc::Int8
        | TypeDesc::Int16
        | TypeDesc::Int32
        | TypeDesc::Int64
        | TypeDesc::UInt8
        | TypeDesc::UInt16
        | TypeDesc::UInt32
        | TypeDesc::UInt64 => Value::Int(0),
        TypeDesc::Float32 | TypeDesc::Float64 => Value::Float(0.0),
        TypeDesc::Bool => Value::Bool(false),
        TypeDesc::Char => Value::Char('\0'),
        _ => Value::Null,
    }
}

/// Human-readable description of a value, used in error messages.
fn describe_value(vm: &Vm, v: &Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Char(c) => format!("'{}'", c),
        Value::Null => "null".to_string(),
        Value::Unit => "unit".to_string(),
        Value::String(s) => vm.heap.get_string(*s).unwrap_or("<string>").to_string(),
        Value::Enum(enum_id, variant_idx, fields) => {
            let (name, variant) = vm
                .module
                .enums
                .get(&EnumId(*enum_id))
                .map(|e| {
                    (
                        e.name.clone(),
                        e.variants
                            .get(*variant_idx as usize)
                            .map(|v| v.name.clone())
                            .unwrap_or_else(|| format!("#{}", variant_idx)),
                    )
                })
                .unwrap_or_else(|| (format!("enum#{}", enum_id), format!("#{}", variant_idx)));
            if fields.is_empty() {
                format!("{}.{}", name, variant)
            } else {
                format!("{}.{}(...)", name, variant)
            }
        }
        Value::Object(handle) => vm
            .heap
            .get(*handle)
            .ok()
            .and_then(|o| match o {
                AuraObject::Instance { class_id, .. } => {
                    vm.module.classes.get(class_id).map(|c| c.name.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| "instance".to_string()),
        Value::Tuple(_) => "tuple".to_string(),
    }
}

/// Substitute generic parameters in a TypeDesc with concrete types
fn substitute_type_desc(ty: &TypeDesc, generic_params: &[aura_bytecode::GenericParam], type_args: &[TypeDesc]) -> TypeDesc {
    match ty {
        TypeDesc::GenericParam(idx) => {
            // Look up the parameter name by index, then find the corresponding type argument
            if let Some(param) = generic_params.get(*idx as usize) {
                if let Some(arg_idx) = generic_params.iter().position(|p| p.name == param.name) {
                    if let Some(arg) = type_args.get(arg_idx) {
                        return arg.clone();
                    }
                }
            }
            ty.clone()
        }
        TypeDesc::Class(id, args) => {
            let substituted_args: Vec<TypeDesc> = args.iter()
                .map(|arg| substitute_type_desc(arg, generic_params, type_args))
                .collect();
            TypeDesc::Class(*id, substituted_args)
        }
        TypeDesc::Tuple(types) => {
            let substituted_types: Vec<TypeDesc> = types.iter()
                .map(|t| substitute_type_desc(t, generic_params, type_args))
                .collect();
            TypeDesc::Tuple(substituted_types)
        }
        _ => ty.clone(),
    }
}

/// Errors raised during VM execution.
#[derive(Debug, thiserror::Error)]
pub enum VmError {
    /// The module has no entrypoint.
    #[error("no entrypoint defined")]
    NoEntrypoint,
    /// Unknown method id.
    #[error("unknown method {0:?}")]
    UnknownMethod(MethodId),
    /// Unknown class id.
    #[error("unknown class {0:?}")]
    UnknownClass(ClassId),
    /// Stack underflow.
    #[error("stack underflow")]
    StackUnderflow,
    /// Type mismatch at runtime.
    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch { expected: &'static str, got: String },
    /// Heap access failure.
    #[error("heap error: {0}")]
    Heap(#[from] HeapError),
    /// Division by zero.
    #[error("divide by zero")]
    DivideByZero,
    /// Generic runtime error.
    #[error("runtime error: {0}")]
    Runtime(String),
    /// Integer overflow detected (only when overflow checking is enabled).
    #[error("integer overflow")]
    Overflow,
}

/// The outcome of executing a frame.
enum FrameResult {
    /// The frame returned normally with a value.
    Normal(Value),
    /// An exception propagated out of the frame without being handled.
    Exception(Value),
}

/// A single call frame.
#[derive(Debug)]
pub struct Frame {
    method_id: MethodId,
    pc: usize,
    locals: Vec<Value>,
    stack: Vec<Value>,
}

impl Frame {
    fn new(method_id: MethodId, params: Vec<Value>, locals_count: u16) -> Self {
        let mut locals = Vec::with_capacity(locals_count as usize);
        locals.extend(params);
        locals.resize(locals_count as usize, Value::Unit);
        Self {
            method_id,
            pc: 0,
            locals,
            stack: Vec::new(),
        }
    }

    fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn peek(&self) -> Result<&Value, VmError> {
        self.stack.last().ok_or(VmError::StackUnderflow)
    }
}

/// A virtual machine execution context.
pub struct Vm {
    module: Arc<Module>,
    heap: Heap,
    call_stack: Vec<Frame>,
    static_fields: HashMap<ClassId, Vec<Value>>,
    /// When true, integer arithmetic and narrowing conversions detect overflow
    /// and raise a runtime error; when false they wrap (unchecked semantics).
    overflow_checks: bool,
}

impl Vm {
    /// Create a VM for the given module.
    pub fn new(module: Arc<Module>) -> Self {
        let mut static_fields = HashMap::new();
        for (id, class) in &module.classes {
            static_fields.insert(
                *id,
                class.static_fields.iter().map(|f| default_value(&f.ty)).collect(),
            );
        }
        Self {
            module,
            heap: Heap::new(),
            call_stack: Vec::new(),
            static_fields,
            overflow_checks: false,
        }
    }

    /// Enable or disable runtime overflow checking. When enabled, integer
    /// overflow in arithmetic and narrowing conversions raise
    /// [`VmError::Overflow`]; when disabled, results wrap around.
    pub fn set_overflow_checks(&mut self, enabled: bool) {
        self.overflow_checks = enabled;
    }

    /// Run the module entrypoint and return the resulting value.
    pub fn run(&mut self) -> Result<Value, VmError> {
        let entry = self.module.entrypoint.ok_or(VmError::NoEntrypoint)?;
        self.invoke(entry, vec![])
    }

    /// Invoke a method by id with the supplied arguments.
    pub fn invoke(&mut self, method_id: MethodId, args: Vec<Value>) -> Result<Value, VmError> {
        match self.invoke_frame(method_id, args)? {
            FrameResult::Normal(v) => Ok(v),
            FrameResult::Exception(e) => Err(VmError::Runtime(format!(
                "uncaught exception: {}",
                self.describe_exception(&e)
            ))),
        }
    }

    /// Push a frame for `method_id` and run it. The frame is popped before this
    /// returns, whether the method returns normally or throws.
    fn invoke_frame(&mut self, method_id: MethodId, args: Vec<Value>) -> Result<FrameResult, VmError> {
        let locals = self.resolve_method(method_id)?.locals;
        let frame = Frame::new(method_id, args, locals);
        self.call_stack.push(frame);
        self.run_frame()
    }

    fn run_frame(&mut self) -> Result<FrameResult, VmError> {
        let method_id = self.current_frame().method_id;
        let method = self.resolve_method(method_id)?.clone();
        self.execute_frame(method)
    }

    /// Access the heap directly.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    fn execute_frame(&mut self, method: MethodDef) -> Result<FrameResult, VmError> {
        let mut pending: Option<Value> = None;
        // Captured once: overflow_checks only changes between runs.
        let overflow_checks = self.overflow_checks;
        let mut after_finally: Option<Value> = None;
        loop {
            if let Some(exc) = pending.take() {
                let instr_pc = self.current_frame().pc.saturating_sub(1) as u32;
                let handler = self.find_handler(&method.handlers, instr_pc, &exc);
                match handler {
                    Some(h) if h.catch_type.is_some() => {
                        let frame = self.current_frame();
                        if let Some(slot) = frame.locals.get_mut(h.catch_local as usize) {
                            *slot = exc;
                        }
                        frame.pc = h.handler_pc as usize;
                    }
                    Some(h) => {
                        after_finally = Some(exc);
                        self.current_frame().pc = h.handler_pc as usize;
                    }
                    None => {
                        self.call_stack.pop();
                        return Ok(FrameResult::Exception(exc));
                    }
                }
                continue;
            }

            let op = {
                let frame = self.current_frame();
                let op = method
                    .body
                    .get(frame.pc)
                    .ok_or_else(|| VmError::Runtime(format!("pc out of bounds")))?
                    .clone();
                frame.pc += 1;
                op
            };

            if op == Op::Ret {
                let result = self.pop().unwrap_or(Value::Unit);
                self.call_stack.pop();
                return Ok(FrameResult::Normal(result));
            }

            match op {
                Op::Nop => {}

                Op::LdInt(i) => self.push(Value::Int(i as i64)),
                Op::LdInt64(i) => self.push(Value::Int(i)),
                Op::Conv(ty) => {
                    let v = self.pop()?;
                    self.push(self.convert(v, &ty)?);
                }
                Op::LdFloat(idx) => {
                    let s = self
                        .module
                        .constant_pool
                        .get(idx as usize)
                        .ok_or_else(|| VmError::Runtime("float constant out of range".into()))?;
                    let f = s.parse::<f64>().map_err(|e| VmError::Runtime(e.to_string()))?;
                    self.push(Value::Float(f));
                }
                Op::LdBool(b) => self.push(Value::Bool(b)),
                Op::LdChar(c) => {
                    let ch = char::from_u32(c).unwrap_or('\0');
                    self.push(Value::Char(ch));
                }
                Op::LdStr(idx) => {
                    let s = self
                        .module
                        .constant_pool
                        .get(idx as usize)
                        .ok_or_else(|| VmError::Runtime("string constant out of range".into()))?
                        .clone();
                    let handle = self.heap.allocate(AuraObject::String(s));
                    self.push(Value::String(handle));
                }
                Op::LdNull => self.push(Value::Null),

                Op::Ldloc(idx) => {
                    let v = self.local(idx)?.clone();
                    self.push(v);
                }
                Op::Stloc(idx) => {
                    let v = self.pop()?;
                    *self.local_mut(idx)? = v;
                }
                Op::Ldarg(idx) => {
                    let v = self.arg(idx)?.clone();
                    self.push(v);
                }
                Op::Dup => {
                    let v = self.current_frame().peek()?.clone();
                    self.push(v);
                }
                Op::Pop => {
                    self.pop()?;
                }

                Op::Add => self.binary_int_float(
                    |a, b| {
                        if overflow_checks {
                            a.checked_add(b).ok_or(VmError::Overflow)
                        } else {
                            Ok(a.wrapping_add(b))
                        }
                    },
                    |a, b| Ok(a + b),
                )?,
                Op::Sub => self.binary_int_float(
                    |a, b| {
                        if overflow_checks {
                            a.checked_sub(b).ok_or(VmError::Overflow)
                        } else {
                            Ok(a.wrapping_sub(b))
                        }
                    },
                    |a, b| Ok(a - b),
                )?,
                Op::Mul => self.binary_int_float(
                    |a, b| {
                        if overflow_checks {
                            a.checked_mul(b).ok_or(VmError::Overflow)
                        } else {
                            Ok(a.wrapping_mul(b))
                        }
                    },
                    |a, b| Ok(a * b),
                )?,
                Op::Div => self.binary_int_float(
                    |a, b| {
                        if b == 0 {
                            Err(VmError::DivideByZero)
                        } else if overflow_checks {
                            a.checked_div(b).ok_or(VmError::Overflow)
                        } else {
                            Ok(a.wrapping_div(b))
                        }
                    },
                    |a, b| if b == 0.0 { Err(VmError::DivideByZero) } else { Ok(a / b) },
                )?,
                Op::Rem => self.binary_int_float(
                    |a, b| {
                        if b == 0 {
                            Err(VmError::DivideByZero)
                        } else if overflow_checks {
                            a.checked_rem(b).ok_or(VmError::Overflow)
                        } else {
                            Ok(a.wrapping_rem(b))
                        }
                    },
                    |a, b| if b == 0.0 { Err(VmError::DivideByZero) } else { Ok(a % b) },
                )?,
                Op::Neg => {
                    let v = self.pop()?;
                    let res = match v {
                        Value::Int(i) => Value::Int(if overflow_checks {
                            i.checked_neg().ok_or(VmError::Overflow)?
                        } else {
                            i.wrapping_neg()
                        }),
                        Value::Float(f) => Value::Float(-f),
                        _ => return Err(VmError::TypeMismatch { expected: "int or float", got: v.type_name().into() }),
                    };
                    self.push(res);
                }

                Op::Eq => self.compare_eq()?,
                Op::ValueEq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let res = self.value_eq(&a, &b);
                    self.push(Value::Bool(res));
                }
                Op::Hash => {
                    let v = self.pop()?;
                    let h = self.value_hash(&v);
                    self.push(Value::Int(h as i64));
                }
                Op::Lt => self.compare_int_float(|a, b| a < b, |a, b| a < b)?,
                Op::Le => self.compare_int_float(|a, b| a <= b, |a, b| a <= b)?,
                Op::Gt => self.compare_int_float(|a, b| a > b, |a, b| a > b)?,
                Op::Ge => self.compare_int_float(|a, b| a >= b, |a, b| a >= b)?,
                Op::Not => {
                    let v = self.pop()?;
                    self.push(Value::Bool(!v.is_truthy()));
                }
                Op::And => {
                    let b = self.pop()?.is_truthy();
                    let a = self.pop()?.is_truthy();
                    self.push(Value::Bool(a && b));
                }
                Op::Or => {
                    let b = self.pop()?.is_truthy();
                    let a = self.pop()?.is_truthy();
                    self.push(Value::Bool(a || b));
                }

                Op::Br(off) => self.current_frame().pc = off as usize,
                Op::BrFalse(off) => {
                    if !self.pop()?.is_truthy() {
                        self.current_frame().pc = off as usize;
                    }
                }
                Op::BrTrue(off) => {
                    if self.pop()?.is_truthy() {
                        self.current_frame().pc = off as usize;
                    }
                }
                Op::Call(id) => {
                    let (params_len, locals) = {
                        let m = self.resolve_method(id)?;
                        (m.params.len(), m.locals)
                    };
                    let args = self.pop_args(params_len)?;
                    self.call_stack.push(Frame::new(id, args, locals));
                    match self.run_frame()? {
                        FrameResult::Normal(v) => self.push(v),
                        FrameResult::Exception(e) => pending = Some(e),
                    }
                }
                Op::CallVirt(name) => {
                    // The compiler pushes arguments first, then the instance.
                    // Pop the instance, resolve the method, then pop the arguments.
                    let instance = self.pop()?;
                    let Value::Object(handle) = instance else {
                        return Err(VmError::TypeMismatch { expected: "object", got: instance.type_name().into() });
                    };
                    let class_id = self.heap.get(handle)?.instance_class_id()?;
                    let method_id = self.resolve_virtual(class_id, &name)?;
                    let params_len = self.resolve_method(method_id)?.params.len();
                    // params does NOT include the instance, so we pop params_len arguments
                    let mut args = self.pop_args(params_len)?;
                    // Insert the instance at the beginning (as the first parameter)
                    args.insert(0, Value::Object(handle));
                    match self.invoke_frame(method_id, args)? {
                        FrameResult::Normal(v) => self.push(v),
                        FrameResult::Exception(e) => pending = Some(e),
                    }
                }
                Op::CallSuper(id) => {
                    // Non-virtual call to a base class method from `super.Method()`.
                    // The compiler pushes arguments first, then `this`.
                    let instance = self.pop()?;
                    let Value::Object(handle) = instance else {
                        return Err(VmError::TypeMismatch { expected: "object", got: instance.type_name().into() });
                    };
                    let params_len = self.resolve_method(id)?.params.len();
                    let mut args = self.pop_args(params_len)?;
                    args.insert(0, Value::Object(handle));
                    match self.invoke_frame(id, args)? {
                        FrameResult::Normal(v) => self.push(v),
                        FrameResult::Exception(e) => pending = Some(e),
                    }
                }
                Op::NewObj(class_id, type_args) => {
                    let class = self
                        .module
                        .classes
                        .get(&class_id)
                        .ok_or(VmError::UnknownClass(class_id))?
                        .clone();
                    if class.is_abstract || class.is_interface {
                        return Err(VmError::Runtime(format!(
                            "cannot instantiate {} `{}`",
                            if class.is_interface { "interface" } else { "abstract class" },
                            class.name
                        )));
                    }
                    let field_count = class.fields.len();
                    let mut fields = Vec::with_capacity(field_count);
                    
                    for field in &class.fields {
                        // Substitute generic parameters in field types
                        let field_ty = substitute_type_desc(&field.ty, &class.generic_params, &type_args);
                        fields.push(default_value(&field_ty));
                    }
                    let handle = self
                        .heap
                        .allocate(AuraObject::Instance { class_id, fields });
                    self.push(Value::Object(handle));
                }
                Op::Ldfld(idx) => {
                    let instance = self.pop()?;
                    let Value::Object(handle) = instance else {
                        return Err(VmError::TypeMismatch { expected: "object", got: instance.type_name().into() });
                    };
                    let field = self.heap.get(handle)?.field(idx as usize)?.clone();
                    self.push(field);
                }
                Op::Stfld(idx) => {
                    let instance = self.pop()?;
                    let value = self.pop()?;
                    let Value::Object(handle) = instance else {
                        return Err(VmError::TypeMismatch { expected: "object", got: instance.type_name().into() });
                    };
                    *self.heap.get_mut(handle)?.field_mut(idx as usize)? = value;
                }
                Op::Ldsfld(class_id, idx) => {
                    let v = self.static_fields[&class_id][idx as usize].clone();
                    self.push(v);
                }
                Op::Stsfld(class_id, idx) => {
                    let v = self.pop()?;
                    self.static_fields.get_mut(&class_id).unwrap()[idx as usize] = v;
                }

                Op::NewEnum(enum_id, variant_idx) => {
                    let enum_def = self.module.enums.get(&enum_id)
                        .ok_or_else(|| VmError::Runtime(format!("unknown enum {:?}", enum_id)))?;
                    let variant = enum_def.variants.get(variant_idx as usize)
                        .ok_or_else(|| VmError::Runtime(format!("unknown variant index {}", variant_idx)))?;
                    let field_count = variant.fields.len();
                    let mut fields = Vec::with_capacity(field_count);
                    for _ in 0..field_count {
                        fields.push(self.pop()?);
                    }
                    fields.reverse();
                    self.push(Value::Enum(enum_id.0, variant_idx as u32, fields));
                }
                Op::EnumTag => {
                    let v = self.pop()?;
                    match v {
                        Value::Enum(_, variant_idx, _) => self.push(Value::Int(variant_idx as i64)),
                        _ => return Err(VmError::TypeMismatch {
                            expected: "enum",
                            got: v.type_name().into(),
                        }),
                    }
                }
                Op::EnumField(idx) => {
                    let v = self.pop()?;
                    match v {
                        Value::Enum(_, _, fields) => {
                            let field = fields.get(idx as usize)
                                .ok_or_else(|| VmError::Runtime(format!("enum field index {} out of range", idx)))?;
                            self.push(field.clone());
                        }
                        _ => return Err(VmError::TypeMismatch {
                            expected: "enum",
                            got: v.type_name().into(),
                        }),
                    }
                }

                Op::NewTuple(count) => {
                    let mut elements = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        elements.push(self.pop()?);
                    }
                    elements.reverse();
                    self.push(Value::Tuple(elements));
                }
                Op::TupleField(idx) => {
                    let v = self.pop()?;
                    match v {
                        Value::Tuple(elements) => {
                            let elem = elements.get(idx as usize)
                                .ok_or_else(|| VmError::Runtime(format!("tuple index {} out of range", idx)))?;
                            self.push(elem.clone());
                        }
                        _ => return Err(VmError::TypeMismatch {
                            expected: "tuple",
                            got: v.type_name().into(),
                        }),
                    }
                }

                Op::Print => {
                    let v = self.pop()?;
                    match &v {
                        Value::String(handle) => print!("{}", self.heap.get_string(*handle).unwrap_or("")),
                        _ => print!("{}", v),
                    }
                }
                Op::PrintLn => println!(),
                Op::StringConcat(count) => {
                    let mut parts = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        parts.push(self.pop()?);
                    }
                    parts.reverse();
                    
                    let mut result = String::new();
                    for part in parts {
                        match part {
                            Value::String(handle) => {
                                result.push_str(self.heap.get_string(handle).unwrap_or(""));
                            }
                            Value::Int(i) => result.push_str(&i.to_string()),
                            Value::Float(f) => result.push_str(&f.to_string()),
                            Value::Bool(b) => result.push_str(&b.to_string()),
                            Value::Char(c) => result.push_str(&c.to_string()),
                            Value::Null => result.push_str("null"),
                            Value::Unit => result.push_str("()"),
                            _ => result.push_str(&part.to_string()),
                        }
                    }
                    
                    let handle = self.heap.allocate(AuraObject::String(result));
                    self.push(Value::String(handle));
                }
                Op::Ret => unreachable!("Ret handled above match"),
                Op::Break | Op::Continue => unreachable!("Break/Continue should be resolved by emitter"),
                Op::Throw => {
                    let exc = self.pop()?;
                    self.attach_stack_trace(&exc);
                    after_finally = None;
                    pending = Some(exc);
                }
                Op::EndFinally => {
                    if let Some(exc) = after_finally.take() {
                        pending = Some(exc);
                    }
                }
            }
        }
    }

    fn current_frame(&mut self) -> &mut Frame {
        self.call_stack.last_mut().expect("call stack underflow")
    }

    fn push(&mut self, value: Value) {
        self.current_frame().push(value);
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.current_frame().pop()
    }

    fn pop_args(&mut self, count: usize) -> Result<Vec<Value>, VmError> {
        let mut args = Vec::with_capacity(count);
        for _ in 0..count {
            args.push(self.pop()?);
        }
        args.reverse();
        Ok(args)
    }

    fn local(&self, idx: u16) -> Result<&Value, VmError> {
        self.call_stack
            .last()
            .and_then(|f| f.locals.get(idx as usize))
            .ok_or_else(|| VmError::Runtime(format!("local {idx} out of range")))
    }

    fn local_mut(&mut self, idx: u16) -> Result<&mut Value, VmError> {
        self.call_stack
            .last_mut()
            .and_then(|f| f.locals.get_mut(idx as usize))
            .ok_or_else(|| VmError::Runtime(format!("local {idx} out of range")))
    }

    fn arg(&self, idx: u16) -> Result<&Value, VmError> {
        // Arguments are stored at the start of locals.
        self.local(idx)
    }

    fn resolve_method(&self, id: MethodId) -> Result<&MethodDef, VmError> {
        self.module.method(id).ok_or(VmError::UnknownMethod(id))
    }

    fn resolve_virtual(&self, class_id: ClassId, name: &str) -> Result<MethodId, VmError> {
        let mut visited = HashSet::new();
        self.resolve_virtual_from(class_id, name, &mut visited)
    }

    /// Search a class's own methods, then its super class chain, then its
    /// implemented/extended interfaces for a method by name.
    fn resolve_virtual_from(
        &self,
        class_id: ClassId,
        name: &str,
        visited: &mut HashSet<ClassId>,
    ) -> Result<MethodId, VmError> {
        if !visited.insert(class_id) {
            return Err(VmError::Runtime(format!("method {} not found", name)));
        }
        let class = self
            .module
            .classes
            .get(&class_id)
            .ok_or(VmError::UnknownClass(class_id))?;
        for (id, m) in &class.methods {
            if m.name == name {
                return Ok(*id);
            }
        }
        if let Some(super_id) = class.super_class {
            if let Ok(id) = self.resolve_virtual_from(super_id, name, visited) {
                return Ok(id);
            }
        }
        for iface_id in &class.interfaces {
            if let Ok(id) = self.resolve_virtual_from(*iface_id, name, visited) {
                return Ok(id);
            }
        }
        Err(VmError::Runtime(format!("method {} not found", name)))
    }

    /// Find the innermost exception handler covering `pc` that applies to the
    /// given exception value. Catch handlers take precedence over the `finally`
    /// handler of the same protected region.
    fn find_handler(&self, handlers: &[ExceptionHandler], pc: u32, exc: &Value) -> Option<ExceptionHandler> {
        let mut best: Option<(u32, usize, bool)> = None;
        for (i, h) in handlers.iter().enumerate() {
            if h.start <= pc && pc < h.end {
                let matches = match &h.catch_type {
                    Some(ty) => self.value_matches(ty, exc),
                    None => true,
                };
                if !matches {
                    continue;
                }
                let is_catch = h.catch_type.is_some();
                let better = match best {
                    None => true,
                    Some((b_start, _, b_is_catch)) => {
                        h.start > b_start || (h.start == b_start && is_catch && !b_is_catch)
                    }
                };
                if better {
                    best = Some((h.start, i, is_catch));
                }
            }
        }
        best.map(|(_, i, _)| handlers[i].clone())
    }

    /// True if the runtime type of `exc` is assignable to `ty`.
    fn value_matches(&self, ty: &TypeDesc, exc: &Value) -> bool {
        match (ty, exc) {
            (TypeDesc::String, Value::String(_)) => true,
            (TypeDesc::Int8
            | TypeDesc::Int16
            | TypeDesc::Int32
            | TypeDesc::Int64
            | TypeDesc::UInt8
            | TypeDesc::UInt16
            | TypeDesc::UInt32
            | TypeDesc::UInt64, Value::Int(_)) => true,
            (TypeDesc::Float32 | TypeDesc::Float64, Value::Float(_)) => true,
            (TypeDesc::Bool, Value::Bool(_)) => true,
            (TypeDesc::Null, Value::Null) => true,
            (TypeDesc::Tuple(_), Value::Tuple(_)) => true,
            (TypeDesc::Enum(eid), Value::Enum(eid2, _, _)) => *eid == EnumId(*eid2),
            (TypeDesc::GenericParam(_), _) => true,
            (TypeDesc::Class(target, _), Value::Object(handle)) => match self.heap.get(*handle) {
                Ok(AuraObject::Instance { class_id, .. }) => self.is_instance_of(*class_id, *target),
                _ => false,
            },
            _ => false,
        }
    }

    /// True if `sub` is the same as, a subclass of, or an implementer of `base`.
    fn is_instance_of(&self, sub: ClassId, base: ClassId) -> bool {
        let mut visited = HashSet::new();
        self.is_instance_of_inner(sub, base, &mut visited)
    }

    fn is_instance_of_inner(&self, sub: ClassId, base: ClassId, visited: &mut HashSet<ClassId>) -> bool {
        if sub == base {
            return true;
        }
        if !visited.insert(sub) {
            return false;
        }
        let Some(class) = self.module.classes.get(&sub) else {
            return false;
        };
        if let Some(sup) = class.super_class {
            if self.is_instance_of_inner(sup, base, visited) {
                return true;
            }
        }
        for iface in &class.interfaces {
            if self.is_instance_of_inner(*iface, base, visited) {
                return true;
            }
        }
        false
    }

    fn binary_int_float<FI, FF>(&mut self, int_op: FI, float_op: FF) -> Result<(), VmError>
    where
        FI: FnOnce(i64, i64) -> Result<i64, VmError>,
        FF: FnOnce(f64, f64) -> Result<f64, VmError>,
    {
        let b = self.pop()?;
        let a = self.pop()?;
        let res = match (a, b) {
            (Value::Int(a), Value::Int(b)) => Value::Int(int_op(a, b)?),
            (Value::Float(a), Value::Float(b)) => Value::Float(float_op(a, b)?),
            (Value::Int(a), Value::Float(b)) => Value::Float(float_op(a as f64, b)?),
            (Value::Float(a), Value::Int(b)) => Value::Float(float_op(a, b as f64)?),
            (a, b) => {
                return Err(VmError::TypeMismatch {
                    expected: "int or float",
                    got: format!("{} and {}", a.type_name(), b.type_name()),
                })
            }
        };
        self.push(res);
        Ok(())
    }

    fn compare_eq(&mut self) -> Result<(), VmError> {
        let b = self.pop()?;
        let a = self.pop()?;
        let res = match (a, b) {
            (Value::Int(a), Value::Int(b)) => Value::Bool(a == b),
            (Value::Float(a), Value::Float(b)) => Value::Bool(a == b),
            (Value::Bool(a), Value::Bool(b)) => Value::Bool(a == b),
            (Value::Char(a), Value::Char(b)) => Value::Bool(a == b),
            (Value::Null, Value::Null) => Value::Bool(true),
            (Value::Null, Value::Object(_)) | (Value::Object(_), Value::Null) => Value::Bool(false),
            (Value::Null, Value::String(_)) | (Value::String(_), Value::Null) => Value::Bool(false),
            (Value::Object(a), Value::Object(b)) => Value::Bool(a == b),
            (Value::Enum(eid_a, vid_a, ref fields_a), Value::Enum(eid_b, vid_b, ref fields_b)) => {
                Value::Bool(eid_a == eid_b && vid_a == vid_b && fields_a == fields_b)
            }
            (a, b) => {
                return Err(VmError::TypeMismatch {
                    expected: "comparable values",
                    got: format!("{} and {}", a.type_name(), b.type_name()),
                })
            }
        };
        self.push(res);
        Ok(())
    }

    /// Deep structural equality. Record instances compare field-by-field;
    /// non-record objects compare by identity; strings compare by content.
    fn value_eq(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Null, _) | (_, Value::Null) => false,
            (Value::String(a), Value::String(b)) => {
                self.heap.get_string(*a) == self.heap.get_string(*b)
            }
            (Value::Object(a), Value::Object(b)) => self.object_value_eq(*a, *b),
            (Value::Enum(e1, v1, f1), Value::Enum(e2, v2, f2)) => {
                e1 == e2
                    && v1 == v2
                    && f1.len() == f2.len()
                    && f1.iter().zip(f2.iter()).all(|(x, y)| self.value_eq(x, y))
            }
            (Value::Tuple(t1), Value::Tuple(t2)) => {
                t1.len() == t2.len()
                    && t1.iter().zip(t2.iter()).all(|(x, y)| self.value_eq(x, y))
            }
            _ => false,
        }
    }

    /// Structural equality of two object handles.
    fn object_value_eq(&self, a: GcRef, b: GcRef) -> bool {
        if a == b {
            return true;
        }
        match (self.heap.get(a), self.heap.get(b)) {
            (Ok(AuraObject::String(s1)), Ok(AuraObject::String(s2))) => s1 == s2,
            (
                Ok(AuraObject::Instance {
                    class_id: c1,
                    fields: f1,
                }),
                Ok(AuraObject::Instance {
                    class_id: c2,
                    fields: f2,
                }),
            ) => {
                if c1 != c2 {
                    return false;
                }
                let is_record = self
                    .module
                    .classes
                    .get(c1)
                    .map(|c| c.is_record)
                    .unwrap_or(false);
                if !is_record {
                    return false;
                }
                f1.len() == f2.len()
                    && f1.iter().zip(f2.iter()).all(|(x, y)| self.value_eq(x, y))
            }
            _ => false,
        }
    }

    /// Structural hash of a value. Record instances hash over their fields;
    /// strings hash by content; non-record objects hash by identity.
    fn value_hash(&self, v: &Value) -> i32 {
        match v {
            Value::Int(i) => *i as i32,
            Value::Float(f) => f.to_bits() as i32,
            Value::Bool(b) => *b as i32,
            Value::Char(c) => *c as i32,
            Value::Null | Value::Unit => 0,
            Value::String(s) => self.heap.get_string(*s).map(fnv_hash).unwrap_or(0),
            Value::Object(handle) => self.object_value_hash(*handle),
            Value::Enum(eid, vid, fields) => {
                let mut h = 17i32;
                h = h.wrapping_mul(31).wrapping_add(*eid as i32);
                h = h.wrapping_mul(31).wrapping_add(*vid as i32);
                for f in fields {
                    h = h.wrapping_mul(31).wrapping_add(self.value_hash(f));
                }
                h
            }
            Value::Tuple(elements) => {
                let mut h = 17i32;
                for e in elements {
                    h = h.wrapping_mul(31).wrapping_add(self.value_hash(e));
                }
                h
            }
        }
    }

    /// Structural hash of an object handle.
    fn object_value_hash(&self, handle: GcRef) -> i32 {
        match self.heap.get(handle) {
            Ok(AuraObject::String(s)) => fnv_hash(s),
            Ok(AuraObject::Instance { class_id, fields }) => {
                let is_record = self
                    .module
                    .classes
                    .get(class_id)
                    .map(|c| c.is_record)
                    .unwrap_or(false);
                if !is_record {
                    return handle.0 as i32;
                }
                let mut h = 17i32;
                for f in fields {
                    h = h.wrapping_mul(31).wrapping_add(self.value_hash(f));
                }
                h
            }
            _ => handle.0 as i32,
        }
    }

    fn compare_int_float<FI, FF>(&mut self, int_op: FI, float_op: FF) -> Result<(), VmError>
    where
        FI: FnOnce(i64, i64) -> bool,
        FF: FnOnce(f64, f64) -> bool,
    {
        let b = self.pop()?;
        let a = self.pop()?;
        let res = match (a, b) {
            (Value::Int(a), Value::Int(b)) => Value::Bool(int_op(a, b)),
            (Value::Float(a), Value::Float(b)) => Value::Bool(float_op(a, b)),
            (Value::Int(a), Value::Float(b)) => Value::Bool(float_op(a as f64, b)),
            (Value::Float(a), Value::Int(b)) => Value::Bool(float_op(a, b as f64)),
            (a, b) => {
                return Err(VmError::TypeMismatch {
                    expected: "int or float",
                    got: format!("{} and {}", a.type_name(), b.type_name()),
                })
            }
        };
        self.push(res);
        Ok(())
    }

    /// Inclusive integer range of a concrete integer TypeDesc, as i64 values.
    fn int_range(ty: &TypeDesc) -> (i64, i64) {
        match ty {
            TypeDesc::Int8 => (i8::MIN as i64, i8::MAX as i64),
            TypeDesc::Int16 => (i16::MIN as i64, i16::MAX as i64),
            TypeDesc::Int32 => (i32::MIN as i64, i32::MAX as i64),
            TypeDesc::Int64 => (i64::MIN, i64::MAX),
            TypeDesc::UInt8 => (0, u8::MAX as i64),
            TypeDesc::UInt16 => (0, u16::MAX as i64),
            TypeDesc::UInt32 => (0, u32::MAX as i64),
            TypeDesc::UInt64 => (0, i64::MAX),
            _ => (i64::MIN, i64::MAX),
        }
    }

    /// Convert a value to the given numeric type. Narrowing truncates; with
    /// overflow checking enabled, a value that does not fit the target width
    /// raises [`VmError::Overflow`] instead of wrapping.
    fn convert(&self, v: Value, ty: &TypeDesc) -> Result<Value, VmError> {
        if ty.is_float() {
            let f = match v {
                Value::Int(i) => i as f64,
                Value::Float(f) => f,
                _ => {
                    return Err(VmError::TypeMismatch {
                        expected: "int or float",
                        got: v.type_name().into(),
                    })
                }
            };
            return Ok(Value::Float(if *ty == TypeDesc::Float32 {
                f as f32 as f64
            } else {
                f
            }));
        }
        if ty.is_int() {
            let raw = match v {
                Value::Int(i) => i,
                Value::Float(f) => f.trunc() as i64,
                _ => {
                    return Err(VmError::TypeMismatch {
                        expected: "int or float",
                        got: v.type_name().into(),
                    })
                }
            };
            if self.overflow_checks {
                let (min, max) = Self::int_range(ty);
                if raw < min || raw > max {
                    return Err(VmError::Overflow);
                }
            }
            let narrowed = match ty {
                TypeDesc::Int8 => raw as i8 as i64,
                TypeDesc::Int16 => raw as i16 as i64,
                TypeDesc::Int32 => raw as i32 as i64,
                TypeDesc::Int64 => raw,
                TypeDesc::UInt8 => raw as u8 as i64,
                TypeDesc::UInt16 => raw as u16 as i64,
                TypeDesc::UInt32 => raw as u32 as i64,
                TypeDesc::UInt64 => raw as u64 as i64,
                _ => raw,
            };
            return Ok(Value::Int(narrowed));
        }
        Err(VmError::TypeMismatch {
            expected: "numeric type",
            got: v.type_name().into(),
        })
    }

    /// The id of the standard `Exception` class, if present in the module.
    fn exception_class_id(&self) -> Option<ClassId> {
        self.module
            .classes
            .iter()
            .find(|(_, c)| c.name == "Exception")
            .map(|(id, _)| *id)
    }

    /// Multi-line stack trace of the current call stack, innermost frame last.
    fn capture_stack_trace(&self) -> String {
        let mut lines = Vec::new();
        for frame in self.call_stack.iter().rev() {
            let label = self.frame_label(frame.method_id);
            lines.push(format!("  at {}", label));
        }
        if lines.is_empty() {
            lines.push("  at <entrypoint>".to_string());
        }
        lines.join("\n")
    }

    /// `ClassName.Method` label for a method id.
    fn frame_label(&self, method_id: MethodId) -> String {
        for (_, class) in &self.module.classes {
            if let Some(m) = class.methods.get(&method_id).or_else(|| class.static_methods.get(&method_id)) {
                return format!("{}.{}", class.name, m.name);
            }
        }
        format!("{:?}", method_id)
    }

    /// If `exc` is an instance of the standard `Exception` class, record the
    /// current stack trace into its `stackTrace` field.
    fn attach_stack_trace(&mut self, exc: &Value) {
        let Value::Object(handle) = exc else { return };
        let Ok(obj) = self.heap.get(*handle) else { return };
        let AuraObject::Instance { class_id, .. } = obj else { return };
        let Some(exception_id) = self.exception_class_id() else { return };
        if !self.is_instance_of(*class_id, exception_id) {
            return;
        }
        let Some(class) = self.module.classes.get(class_id) else { return };
        let Some(idx) = class.fields.iter().position(|f| f.name == "stackTrace") else {
            return;
        };
        let trace = self.capture_stack_trace();
        let trace_handle = self.heap.allocate(AuraObject::String(trace));
        if let Ok(AuraObject::Instance { fields, .. }) = self.heap.get_mut(*handle) {
            if let Some(slot) = fields.get_mut(idx) {
                *slot = Value::String(trace_handle);
            }
        }
    }

    /// Format a thrown value for the uncaught-exception error message. Exception
    /// instances render as `Class: message` followed by their stack trace.
    fn describe_exception(&self, e: &Value) -> String {
        let Value::Object(handle) = e else {
            return describe_value(self, e);
        };
        let Ok(obj) = self.heap.get(*handle) else {
            return describe_value(self, e);
        };
        let AuraObject::Instance { class_id, fields } = obj else {
            return describe_value(self, e);
        };
        let Some(exception_id) = self.exception_class_id() else {
            return describe_value(self, e);
        };
        if !self.is_instance_of(*class_id, exception_id) {
            return describe_value(self, e);
        }
        let Some(class) = self.module.classes.get(class_id) else {
            return describe_value(self, e);
        };
        let field = |name: &str| {
            class
                .fields
                .iter()
                .position(|f| f.name == name)
                .and_then(|idx| fields.get(idx))
                .map(|v| self.value_as_string(v))
                .unwrap_or_default()
        };
        let mut out = class.name.clone();
        let message = field("message");
        if !message.is_empty() {
            out.push_str(": ");
            out.push_str(&message);
        }
        let trace = field("stackTrace");
        if !trace.is_empty() {
            out.push('\n');
            out.push_str(&trace);
        }
        out
    }

    /// Render a value as text, reading strings out of the heap.
    fn value_as_string(&self, v: &Value) -> String {
        match v {
            Value::String(handle) => self.heap.get_string(*handle).unwrap_or("").to_string(),
            other => describe_value(self, other),
        }
    }
}

trait ObjectExt {
    fn instance_class_id(&self) -> Result<ClassId, VmError>;
    fn field(&self, idx: usize) -> Result<&Value, VmError>;
    fn field_mut(&mut self, idx: usize) -> Result<&mut Value, VmError>;
}

impl ObjectExt for AuraObject {
    fn instance_class_id(&self) -> Result<ClassId, VmError> {
        match self {
            AuraObject::Instance { class_id, .. } => Ok(*class_id),
            _ => Err(VmError::TypeMismatch {
                expected: "instance",
                got: "non-instance object".into(),
            }),
        }
    }

    fn field(&self, idx: usize) -> Result<&Value, VmError> {
        match self {
            AuraObject::Instance { fields, .. } => {
                fields.get(idx).ok_or_else(|| VmError::Runtime("field index out of range".into()))
            }
            _ => Err(VmError::TypeMismatch {
                expected: "instance",
                got: "non-instance object".into(),
            }),
        }
    }

    fn field_mut(&mut self, idx: usize) -> Result<&mut Value, VmError> {
        match self {
            AuraObject::Instance { fields, .. } => fields
                .get_mut(idx)
                .ok_or_else(|| VmError::Runtime("field index out of range".into())),
            _ => Err(VmError::TypeMismatch {
                expected: "instance",
                got: "non-instance object".into(),
            }),
        }
    }
}
