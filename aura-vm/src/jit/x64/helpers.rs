//! VM helper stubs called by JIT-compiled code.
//!
//! The JIT does not inline complex operations (heap allocation, calls, field
//! access, exceptions, dynamic dispatch). Instead, generated code marshals the
//! arguments of a [`Op`] into its native frame's argument area and calls the
//! single dispatcher below. The dispatcher rebuilds the original [`Op`] from a
//! per-method table stored in the compiled method and executes the exact same
//! semantics as the interpreter, so JIT and interpreter results agree.
//!
//! Return status from the dispatcher (in `rax`):
//! * `0` — success; result (if any) is `native_frame.result`.
//! * `1` — an exception was thrown; the value is `native_frame.result`.
//! * `2` — a VM error occurred; it is stored in `vm.jit_error`.

use crate::{default_value, describe_value, substitute_type_desc, ObjectExt, Vm, VmError};
use aura_bytecode::{AuraObject, EnumValue, GcRef, MethodId, Op, TupleValue, Value};
use std::slice;

/// Unwrap a VM error inside [`exec`], converting it into the
/// `Err(Err(e))` form. Anything with an `Into<VmError>` error (heap errors,
/// lookup errors) works here.
macro_rules! try_vm {
    ($e:expr) => {
        match $e.map_err(::std::convert::Into::into) {
            Ok(v) => v,
            Err(e) => return Err(Err(e)),
        }
    };
}

/// Signature of the helper dispatcher. Generated code calls this with `rdi` =
/// native frame, `rsi` = argument array, `rdx` = argument count.
pub type JitFunction = unsafe extern "C" fn(*mut NativeFrame, *const Value, usize) -> u64;

// Offsets into [`NativeFrame`]; generated code relies on these constants.
pub const NF_VM: i32 = 0x00;
pub const NF_OP: i32 = 0x08;
pub const NF_ARG_BASE: i32 = 0x10;
pub const NF_ARG_COUNT: i32 = 0x18;
pub const NF_RESULT: i32 = 0x20;
pub const NF_METHOD_ID: i32 = 0x30;
pub const NF_HELPERS: i32 = 0x38;
pub const NF_DISPATCHER: i32 = 0x40;
pub const NF_FRAME_BASE: i32 = 0x48;
pub const NF_FRAME_BYTES: i32 = 0x50;
pub const NF_SIZE: usize = 0x58;

/// Frame shared between the JIT runtime and generated code. A new instance is
/// set up on the native stack by [`crate::jit::CompiledMethod::run`] and passed
/// to compiled methods in `rdi`.
#[repr(C)]
pub struct NativeFrame {
    /// Pointer to the `Vm` executing this method.
    pub vm: *mut Vm,
    /// Index into the `helpers` table of the op being executed (set by
    /// generated code before each helper call).
    pub op_id: u64,
    /// Marshalled argument array for the current helper call.
    pub arg_base: *const Value,
    /// Number of marshalled arguments.
    pub arg_count: usize,
    /// Result slot: holds the return value or a thrown exception.
    pub result: Value,
    /// Id of the method being executed (for stack traces).
    pub method_id: MethodId,
    /// Pointer to the compiled method's `Vec<Op>` helper table.
    pub helpers: *const Vec<Op>,
    /// Address of the [`dispatcher`] stub, called by generated code.
    pub dispatcher: usize,
    /// Base of the generated code's stack frame (`r13`), stored by the
    /// prologue. Null until the prologue has run. The GC scans
    /// `frame_base..frame_base + frame_bytes` conservatively for roots.
    pub frame_base: *const u8,
    /// Size in bytes of the generated code's stack frame.
    pub frame_bytes: usize,
}

/// Return statuses from the dispatcher / compiled code.
pub const STATUS_OK: u64 = 0;
/// The method threw; the exception is in `NativeFrame.result`.
pub const STATUS_EXCEPTION: u64 = 1;
/// A VM error occurred; it is stored in `vm.jit_error`.
pub const STATUS_ERROR: u64 = 2;

/// Set a VM error and return the error status.
fn error(vm: &mut Vm, e: VmError) -> u64 {
    vm.jit_error = Some(e);
    STATUS_ERROR
}

/// Structural enum equality (mirrors `Vm::enum_value_eq`).
fn enum_value_eq(vm: &Vm, a: GcRef, b: GcRef) -> bool {
    if a == b {
        return true;
    }
    match (vm.heap.get_enum(a), vm.heap.get_enum(b)) {
        (Ok(e1), Ok(e2)) => {
            e1.enum_id == e2.enum_id
                && e1.variant_idx == e2.variant_idx
                && e1.fields.len() == e2.fields.len()
                && e1.fields.iter().zip(e2.fields.iter()).all(|(x, y)| vm.value_eq(x, y))
        }
        _ => false,
    }
}

/// Structural tuple equality (mirrors `Vm::tuple_value_eq`).
fn tuple_value_eq(vm: &Vm, a: GcRef, b: GcRef) -> bool {
    if a == b {
        return true;
    }
    match (vm.heap.get_tuple(a), vm.heap.get_tuple(b)) {
        (Ok(t1), Ok(t2)) => {
            t1.elements.len() == t2.elements.len()
                && t1.elements.iter().zip(t2.elements.iter()).all(|(x, y)| vm.value_eq(x, y))
        }
        _ => false,
    }
}

/// Execute the given op with already-marshalled arguments.
fn exec(vm: &mut Vm, op: &Op, args: &[Value]) -> Result<Option<Value>, Result<Value, VmError>> {
    // Result is `Ok(Some(v))` for a produced value, `Ok(None)` for no value.
    // `Err(Ok(exc))` is a thrown exception, `Err(Err(e))` a VM error.
    match op {
        Op::Call(id) => {
            let params_len = try_vm!(vm.resolve_method(*id)).params.len();
            let mut argv = args.to_vec();
            argv.truncate(params_len);
            match try_vm!(vm.invoke_frame(*id, argv)) {
                crate::FrameResult::Normal(v) => Ok(Some(v)),
                crate::FrameResult::Exception(e) => Err(Ok(e)),
            }
        }
        Op::CallVirt(name) => {
            let instance = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            let Value::Object(handle) = instance else {
                return Err(Err(VmError::TypeMismatch {
                    expected: "object".into(),
                    got: instance.type_name().into(),
                }));
            };
            let class_id = try_vm!(try_vm!(vm.heap.get(handle)).instance_class_id());
            let method_id = try_vm!(vm.resolve_virtual(class_id, name));
            let mut argv = args[1..].to_vec();
            argv.insert(0, Value::Object(handle));
            match try_vm!(vm.invoke_frame(method_id, argv)) {
                crate::FrameResult::Normal(v) => Ok(Some(v)),
                crate::FrameResult::Exception(e) => Err(Ok(e)),
            }
        }
        Op::CallSuper(id) => {
            let instance = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            let Value::Object(handle) = instance else {
                return Err(Err(VmError::TypeMismatch {
                    expected: "object".into(),
                    got: instance.type_name().into(),
                }));
            };
            let mut argv = args[1..].to_vec();
            argv.insert(0, Value::Object(handle));
            match try_vm!(vm.invoke_frame(*id, argv)) {
                crate::FrameResult::Normal(v) => Ok(Some(v)),
                crate::FrameResult::Exception(e) => Err(Ok(e)),
            }
        }
        Op::NewObj(class_id, type_args) => {
            let class = vm
                .module
                .classes
                .get(class_id)
                .cloned()
                .ok_or(Err(VmError::UnknownClass(*class_id)))?;
            if class.is_abstract || class.is_interface {
                return Err(Err(VmError::Runtime(format!(
                    "cannot instantiate {} `{}`",
                    if class.is_interface { "interface" } else { "abstract class" },
                    class.name
                ))));
            }
            let fields = class
                .fields
                .iter()
                .map(|f| {
                    substitute_type_desc(&f.ty, &class.generic_params, type_args)
                })
                .map(|t| default_value(&t))
                .collect();
            let handle = vm.heap.allocate(AuraObject::Instance { class_id: *class_id, fields });
            Ok(Some(Value::Object(handle)))
        }
        Op::Ldfld(idx) => {
            let instance = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            let Value::Object(handle) = instance else {
                return Err(Err(VmError::TypeMismatch {
                    expected: "object".into(),
                    got: instance.type_name().into(),
                }));
            };
            let field = try_vm!(try_vm!(vm.heap.get(handle)).field(*idx as usize)).clone();
            Ok(Some(field))
        }
        Op::Stfld(idx) => {
            let instance = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            let value = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let Value::Object(handle) = instance else {
                return Err(Err(VmError::TypeMismatch {
                    expected: "object".into(),
                    got: instance.type_name().into(),
                }));
            };
            *try_vm!(try_vm!(vm.heap.get_mut(handle)).field_mut(*idx as usize)) = value;
            Ok(None)
        }
        Op::Ldsfld(class_id, idx) => {
            let v = vm.static_fields[class_id][*idx as usize].clone();
            Ok(Some(v))
        }
        Op::Stsfld(class_id, idx) => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            vm.static_fields.get_mut(class_id).unwrap()[*idx as usize] = v;
            Ok(None)
        }
        Op::NewEnum(enum_id, variant_idx) => {
            let fields = args.to_vec();
            let handle = vm.heap.allocate(AuraObject::Enum(EnumValue {
                enum_id: enum_id.0,
                variant_idx: *variant_idx as u32,
                fields,
            }));
            Ok(Some(Value::Enum(handle)))
        }
        Op::EnumTag => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            match v {
                Value::Enum(handle) => {
                    let e = try_vm!(vm.heap.get_enum(handle));
                    Ok(Some(Value::Int(e.variant_idx as i64)))
                }
                _ => Err(Err(VmError::TypeMismatch {
                    expected: "enum".into(),
                    got: v.type_name().into(),
                })),
            }
        }
        Op::EnumField(idx) => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            match v {
                Value::Enum(handle) => {
                    let e = try_vm!(vm.heap.get_enum(handle));
                    let field = e
                        .fields
                        .get(*idx as usize)
                        .cloned()
                        .ok_or(Err(VmError::Runtime(format!(
                            "enum field index {} out of range",
                            idx
                        ))))?;
                    Ok(Some(field))
                }
                _ => Err(Err(VmError::TypeMismatch {
                    expected: "enum".into(),
                    got: v.type_name().into(),
                })),
            }
        }
        Op::NewTuple(_) => {
            let elements = args.to_vec();
            let handle = vm.heap.allocate(AuraObject::Tuple(TupleValue { elements }));
            Ok(Some(Value::Tuple(handle)))
        }
        Op::TupleField(idx) => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            match v {
                Value::Tuple(handle) => {
                    let t = try_vm!(vm.heap.get_tuple(handle));
                    let elem = t
                        .elements
                        .get(*idx as usize)
                        .cloned()
                        .ok_or(Err(VmError::Runtime(format!("tuple index {} out of range", idx))))?;
                    Ok(Some(elem))
                }
                _ => Err(Err(VmError::TypeMismatch {
                    expected: "tuple".into(),
                    got: v.type_name().into(),
                })),
            }
        }
        Op::Print => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            match &v {
                Value::String(handle) => print!("{}", vm.heap.get_string(*handle).unwrap_or("")),
                _ => print!("{}", describe_value(vm, &v)),
            }
            Ok(None)
        }
        Op::PrintLn => {
            println!();
            Ok(None)
        }
        Op::StringConcat(count) => {
            let parts = args.get(..*count as usize).ok_or(Err(VmError::StackUnderflow))?;
            let mut result = String::new();
            for part in parts {
                match part {
                    Value::String(handle) => {
                        result.push_str(vm.heap.get_string(*handle).unwrap_or(""));
                    }
                    Value::Int(i) => result.push_str(&i.to_string()),
                    Value::Float(f) => result.push_str(&f.to_string()),
                    Value::Bool(b) => result.push_str(&b.to_string()),
                    Value::Char(c) => result.push_str(&c.to_string()),
                    Value::Null => result.push_str("null"),
                    Value::Unit => result.push_str("()"),
                    other => result.push_str(&describe_value(vm, other)),
                }
            }
            let handle = vm.heap.allocate(AuraObject::String(result));
            Ok(Some(Value::String(handle)))
        }
        Op::Throw => {
            let exc = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            vm.attach_stack_trace(&exc);
            Err(Ok(exc))
        }
        Op::Hash => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            let h = vm.value_hash(&v);
            Ok(Some(Value::Int(h as i64)))
        }
        Op::ValueEq => {
            let a = args.get(0).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let b = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            Ok(Some(Value::Bool(vm.value_eq(&a, &b))))
        }
        Op::And => {
            let a = args.get(0).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let b = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            Ok(Some(Value::Bool(a.is_truthy() && b.is_truthy())))
        }
        Op::Or => {
            let a = args.get(0).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let b = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            Ok(Some(Value::Bool(a.is_truthy() || b.is_truthy())))
        }
        Op::Neg => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            let res = match v {
                Value::Int(i) => Value::Int(if vm.overflow_checks {
                    i.checked_neg().ok_or(Err(VmError::Overflow))?
                } else {
                    i.wrapping_neg()
                }),
                Value::Float(f) => Value::Float(-f),
                _ => {
                    return Err(Err(VmError::TypeMismatch {
                        expected: "int or float".into(),
                        got: v.type_name().into(),
                    }))
                }
            };
            Ok(Some(res))
        }
        Op::Not => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            Ok(Some(Value::Bool(!v.is_truthy())))
        }
        Op::Conv(ty) => {
            let v = args.first().cloned().ok_or(Err(VmError::StackUnderflow))?;
            Ok(Some(try_vm!(vm.convert(v, ty))))
        }
        Op::LdStr(idx) => {
            let s = vm
                .module
                .constant_pool
                .get(*idx as usize)
                .cloned()
                .ok_or(Err(VmError::Runtime("string constant out of range".into())))?;
            let handle = vm.heap.allocate(AuraObject::String(s));
            Ok(Some(Value::String(handle)))
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem => {
            let a = args.get(0).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let b = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let res = match (a, b) {
                (Value::Int(a), Value::Int(b)) => {
                    let r = match op {
                        Op::Add => {
                            if vm.overflow_checks {
                                a.checked_add(b).ok_or(Err(VmError::Overflow))?
                            } else {
                                a.wrapping_add(b)
                            }
                        }
                        Op::Sub => {
                            if vm.overflow_checks {
                                a.checked_sub(b).ok_or(Err(VmError::Overflow))?
                            } else {
                                a.wrapping_sub(b)
                            }
                        }
                        Op::Mul => {
                            if vm.overflow_checks {
                                a.checked_mul(b).ok_or(Err(VmError::Overflow))?
                            } else {
                                a.wrapping_mul(b)
                            }
                        }
                        Op::Div => {
                            if b == 0 {
                                return Err(Err(VmError::DivideByZero));
                            } else if vm.overflow_checks {
                                a.checked_div(b).ok_or(Err(VmError::Overflow))?
                            } else {
                                a.wrapping_div(b)
                            }
                        }
                        Op::Rem => {
                            if b == 0 {
                                return Err(Err(VmError::DivideByZero));
                            } else if vm.overflow_checks {
                                a.checked_rem(b).ok_or(Err(VmError::Overflow))?
                            } else {
                                a.wrapping_rem(b)
                            }
                        }
                        _ => unreachable!(),
                    };
                    Value::Int(r)
                }
                (Value::Float(a), Value::Float(b)) => {
                    let fa = a;
                    let res = match op {
                        Op::Add => fa + b,
                        Op::Sub => fa - b,
                        Op::Mul => fa * b,
                        Op::Div => {
                            if b == 0.0 {
                                return Err(Err(VmError::DivideByZero));
                            }
                            fa / b
                        }
                        Op::Rem => {
                            if b == 0.0 {
                                return Err(Err(VmError::DivideByZero));
                            }
                            fa % b
                        }
                        _ => unreachable!(),
                    };
                    Value::Float(res)
                }
                (Value::Int(a), Value::Float(b)) => {
                    let fa = a as f64;
                    let res = match op {
                        Op::Add => fa + b,
                        Op::Sub => fa - b,
                        Op::Mul => fa * b,
                        Op::Div => {
                            if b == 0.0 {
                                return Err(Err(VmError::DivideByZero));
                            }
                            fa / b
                        }
                        Op::Rem => {
                            if b == 0.0 {
                                return Err(Err(VmError::DivideByZero));
                            }
                            fa % b
                        }
                        _ => unreachable!(),
                    };
                    Value::Float(res)
                }
                (Value::Float(a), Value::Int(b)) => {
                    let fb = b as f64;
                    let res = match op {
                        Op::Add => a + fb,
                        Op::Sub => a - fb,
                        Op::Mul => a * fb,
                        Op::Div => {
                            if fb == 0.0 {
                                return Err(Err(VmError::DivideByZero));
                            }
                            a / fb
                        }
                        Op::Rem => {
                            if fb == 0.0 {
                                return Err(Err(VmError::DivideByZero));
                            }
                            a % fb
                        }
                        _ => unreachable!(),
                    };
                    Value::Float(res)
                }
                (a, b) => {
                    return Err(Err(VmError::TypeMismatch {
                        expected: "int or float".into(),
                        got: format!("{} and {}", a.type_name(), b.type_name()),
                    }))
                }
            };
            Ok(Some(res))
        }
        Op::Eq => {
            let a = args.get(0).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let b = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let res = match (a, b) {
                (Value::Int(a), Value::Int(b)) => Value::Bool(a == b),
                (Value::Float(a), Value::Float(b)) => Value::Bool(a == b),
                (Value::Bool(a), Value::Bool(b)) => Value::Bool(a == b),
                (Value::Char(a), Value::Char(b)) => Value::Bool(a == b),
                (Value::Null, Value::Null) => Value::Bool(true),
                // Null equals nothing but null (mirrors the interpreter).
                (Value::Null, _) | (_, Value::Null) => Value::Bool(false),
                (Value::Object(a), Value::Object(b)) => Value::Bool(a == b),
                (Value::Enum(a), Value::Enum(b)) => Value::Bool(enum_value_eq(vm, a, b)),
                (Value::Tuple(a), Value::Tuple(b)) => Value::Bool(tuple_value_eq(vm, a, b)),
                (a, b) => {
                    return Err(Err(VmError::TypeMismatch {
                        expected: "comparable values".into(),
                        got: format!("{} and {}", a.type_name(), b.type_name()),
                    }))
                }
            };
            Ok(Some(res))
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let a = args.get(0).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let b = args.get(1).cloned().ok_or(Err(VmError::StackUnderflow))?;
            let pred: fn(f64, f64) -> bool = match op {
                Op::Lt => |a, b| a < b,
                Op::Le => |a, b| a <= b,
                Op::Gt => |a, b| a > b,
                Op::Ge => |a, b| a >= b,
                _ => unreachable!(),
            };
            let res = match (a, b) {
                (Value::Int(a), Value::Int(b)) => Value::Bool(pred(a as f64, b as f64)),
                (Value::Float(a), Value::Float(b)) => Value::Bool(pred(a, b)),
                (Value::Int(a), Value::Float(b)) => Value::Bool(pred(a as f64, b)),
                (Value::Float(a), Value::Int(b)) => Value::Bool(pred(a, b as f64)),
                (a, b) => {
                    return Err(Err(VmError::TypeMismatch {
                        expected: "int or float".into(),
                        got: format!("{} and {}", a.type_name(), b.type_name()),
                    }))
                }
            };
            Ok(Some(res))
        }
        Op::NativeCall(id) => {
            let native = aura_bytecode::natives::NativeId::from_u16(*id)
                .ok_or(Err(VmError::Runtime(format!("unknown native id {id}"))))?;
            let v = try_vm!(crate::native::exec_native(vm, native, args));
            Ok(Some(v))
        }
        Op::IsInst(class_id) => {
            let v = args.first().ok_or(Err(VmError::StackUnderflow))?;
            Ok(Some(Value::Bool(vm.value_is_instance(v, *class_id))))
        }
        _ => Err(Err(VmError::Runtime(format!(
            "jit: unhandled helper op {op:?}"
        )))),
    }
}

/// The dispatcher entry point. `status` in `rax`: 0 ok / 1 exception /
/// 2 vm error. The result or exception is written to `native_frame.result`.
unsafe extern "C" fn dispatcher(nf: *mut NativeFrame, args: *const Value, argc: usize) -> u64 {
    let nf = &mut *nf;
    let vm = &mut *nf.vm;
    let op = (&*nf.helpers)[nf.op_id as usize].clone();
    let argv = if argc == 0 {
        Vec::new()
    } else {
        slice::from_raw_parts(args, argc).to_vec()
    };
    match exec(vm, &op, &argv) {
        Ok(Some(v)) => {
            nf.result = v;
            // Rust's enum assignment leaves stale bytes in the unused bits of
            // the 16-byte slot; generated code reads the tag/payload as whole
            // qwords, so the slot must be rewritten in canonical form.
            crate::jit::value::canonicalize(&mut nf.result);
            // Safepoint: the result is rooted via nf.result and all other
            // live references sit in frame slots, so a deferred collection
            // is safe before returning to generated code.
            vm.maybe_collect();
            STATUS_OK
        }
        Ok(None) => {
            vm.maybe_collect();
            STATUS_OK
        }
        Err(Ok(exc)) => {
            nf.result = exc;
            crate::jit::value::canonicalize(&mut nf.result);
            vm.maybe_collect();
            STATUS_EXCEPTION
        }
        Err(Err(e)) => error(vm, e),
    }
}

/// Address of the dispatcher, loaded by generated code.
pub fn dispatcher_addr() -> usize {
    dispatcher as usize
}

/// Set `vm.jit_error` to an overflow error (called by inlined arithmetic when
/// overflow checking is enabled and the overflow flag trips).
unsafe extern "C" fn raise_overflow(vm: *mut Vm) {
    (*vm).jit_error = Some(VmError::Overflow);
}

/// Address of the overflow raiser.
pub fn raise_overflow_addr() -> usize {
    raise_overflow as usize
}
