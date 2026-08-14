//! Native stdlib implementations executed by `Op::NativeCall`.
//!
//! One shared entry point, [`exec_native`], is used by both the interpreter
//! and the JIT's helper dispatcher so the two tiers cannot diverge. Arguments
//! arrive in push order: receiver first for instance natives, then the
//! remaining arguments left to right. Every native returns exactly one value
//! (`Value::Unit` for `void` natives).
//!
//! Error conditions (index out of range, missing map key, malformed numbers,
//! I/O failures) surface as [`VmError::Runtime`] with a message naming the
//! native. They are VM errors, not catchable Aura exceptions, until the
//! stdlib grows exception types.

use crate::{Vm, VmError};
use aura_bytecode::natives::NativeId;
use aura_bytecode::{AuraObject, GcRef, Value};

/// Execute the native `id` with already-popped arguments.
pub(crate) fn exec_native(vm: &mut Vm, id: NativeId, args: &[Value]) -> Result<Value, VmError> {
    debug_assert_eq!(args.len(), id.arity(), "{} arity", id.name());
    match id {
        // ---------------- List<T> ----------------
        NativeId::ListNew => {
            let handle = vm.heap.allocate(AuraObject::Array { elements: Vec::new() });
            Ok(Value::Object(handle))
        }
        NativeId::ListAdd => {
            let h = list_handle(vm, id, &args[0])?;
            let item = args[1].clone();
            list_mut(vm, h)?.push(item);
            Ok(Value::Unit)
        }
        NativeId::ListGet => {
            let h = list_handle(vm, id, &args[0])?;
            let idx = index_arg(vm, id, &args[1])?;
            let elements = list_ref(vm, h)?;
            elements
                .get(idx)
                .cloned()
                .ok_or_else(|| oob(id, idx, elements.len()))
        }
        NativeId::ListSet => {
            let h = list_handle(vm, id, &args[0])?;
            let idx = index_arg(vm, id, &args[1])?;
            let item = args[2].clone();
            let elements = list_mut(vm, h)?;
            let len = elements.len();
            let slot = elements.get_mut(idx).ok_or_else(|| oob(id, idx, len))?;
            *slot = item;
            Ok(Value::Unit)
        }
        NativeId::ListInsert => {
            let h = list_handle(vm, id, &args[0])?;
            let idx = index_arg(vm, id, &args[1])?;
            let item = args[2].clone();
            let elements = list_mut(vm, h)?;
            if idx > elements.len() {
                return Err(oob(id, idx, elements.len()));
            }
            elements.insert(idx, item);
            Ok(Value::Unit)
        }
        NativeId::ListRemoveAt => {
            let h = list_handle(vm, id, &args[0])?;
            let idx = index_arg(vm, id, &args[1])?;
            let elements = list_mut(vm, h)?;
            if idx >= elements.len() {
                return Err(oob(id, idx, elements.len()));
            }
            Ok(elements.remove(idx))
        }
        NativeId::ListIndexOf => {
            let h = list_handle(vm, id, &args[0])?;
            let pos = list_ref(vm, h)?
                .iter()
                .position(|e| vm.value_eq(e, &args[1]));
            Ok(Value::Int(pos.map(|p| p as i64).unwrap_or(-1)))
        }
        NativeId::ListContains => {
            let h = list_handle(vm, id, &args[0])?;
            let found = list_ref(vm, h)?.iter().any(|e| vm.value_eq(e, &args[1]));
            Ok(Value::Bool(found))
        }
        NativeId::ListClear => {
            let h = list_handle(vm, id, &args[0])?;
            list_mut(vm, h)?.clear();
            Ok(Value::Unit)
        }
        NativeId::ListCount => {
            let h = list_handle(vm, id, &args[0])?;
            Ok(Value::Int(list_ref(vm, h)?.len() as i64))
        }

        // ---------------- Map<K, V> ----------------
        NativeId::MapNew => {
            let handle = vm.heap.allocate(AuraObject::Map { entries: Vec::new() });
            Ok(Value::Object(handle))
        }
        NativeId::MapPut => {
            let h = map_handle(vm, id, &args[0])?;
            let (key, value) = (args[1].clone(), args[2].clone());
            let existing = map_ref(vm, h)?
                .iter()
                .position(|(k, _)| vm.value_eq(k, &key));
            let entries = map_mut(vm, h)?;
            match existing {
                Some(i) => entries[i].1 = value,
                None => entries.push((key, value)),
            }
            Ok(Value::Unit)
        }
        NativeId::MapGet => {
            let h = map_handle(vm, id, &args[0])?;
            map_ref(vm, h)?
                .iter()
                .find(|(k, _)| vm.value_eq(k, &args[1]))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| {
                    VmError::Runtime(format!(
                        "{}: key {} not found",
                        id.name(),
                        crate::describe_value(vm, &args[1])
                    ))
                })
        }
        NativeId::MapContainsKey => {
            let h = map_handle(vm, id, &args[0])?;
            let found = map_ref(vm, h)?.iter().any(|(k, _)| vm.value_eq(k, &args[1]));
            Ok(Value::Bool(found))
        }
        NativeId::MapRemove => {
            let h = map_handle(vm, id, &args[0])?;
            let existing = map_ref(vm, h)?
                .iter()
                .position(|(k, _)| vm.value_eq(k, &args[1]));
            match existing {
                Some(i) => {
                    map_mut(vm, h)?.remove(i);
                    Ok(Value::Bool(true))
                }
                None => Ok(Value::Bool(false)),
            }
        }
        NativeId::MapKeys => {
            let h = map_handle(vm, id, &args[0])?;
            let keys: Vec<Value> = map_ref(vm, h)?.iter().map(|(k, _)| k.clone()).collect();
            let handle = vm.heap.allocate(AuraObject::Array { elements: keys });
            Ok(Value::Object(handle))
        }
        NativeId::MapValues => {
            let h = map_handle(vm, id, &args[0])?;
            let values: Vec<Value> = map_ref(vm, h)?.iter().map(|(_, v)| v.clone()).collect();
            let handle = vm.heap.allocate(AuraObject::Array { elements: values });
            Ok(Value::Object(handle))
        }
        NativeId::MapClear => {
            let h = map_handle(vm, id, &args[0])?;
            map_mut(vm, h)?.clear();
            Ok(Value::Unit)
        }
        NativeId::MapCount => {
            let h = map_handle(vm, id, &args[0])?;
            Ok(Value::Int(map_ref(vm, h)?.len() as i64))
        }

        // ---------------- Set<T> ----------------
        NativeId::SetNew => {
            let handle = vm.heap.allocate(AuraObject::Set { elements: Vec::new() });
            Ok(Value::Object(handle))
        }
        NativeId::SetAdd => {
            let h = set_handle(vm, id, &args[0])?;
            let item = args[1].clone();
            let present = set_ref(vm, h)?.iter().any(|e| vm.value_eq(e, &item));
            if present {
                Ok(Value::Bool(false))
            } else {
                set_mut(vm, h)?.push(item);
                Ok(Value::Bool(true))
            }
        }
        NativeId::SetContains => {
            let h = set_handle(vm, id, &args[0])?;
            let found = set_ref(vm, h)?.iter().any(|e| vm.value_eq(e, &args[1]));
            Ok(Value::Bool(found))
        }
        NativeId::SetRemove => {
            let h = set_handle(vm, id, &args[0])?;
            let existing = set_ref(vm, h)?.iter().position(|e| vm.value_eq(e, &args[1]));
            match existing {
                Some(i) => {
                    set_mut(vm, h)?.remove(i);
                    Ok(Value::Bool(true))
                }
                None => Ok(Value::Bool(false)),
            }
        }
        NativeId::SetToList => {
            let h = set_handle(vm, id, &args[0])?;
            let elements = set_ref(vm, h)?.to_vec();
            let handle = vm.heap.allocate(AuraObject::Array { elements });
            Ok(Value::Object(handle))
        }
        NativeId::SetClear => {
            let h = set_handle(vm, id, &args[0])?;
            set_mut(vm, h)?.clear();
            Ok(Value::Unit)
        }
        NativeId::SetCount => {
            let h = set_handle(vm, id, &args[0])?;
            Ok(Value::Int(set_ref(vm, h)?.len() as i64))
        }

        // ---------------- string ----------------
        NativeId::StrLength => {
            let s = str_arg(vm, id, &args[0])?;
            Ok(Value::Int(s.chars().count() as i64))
        }
        NativeId::StrSubstring => {
            let s = str_arg(vm, id, &args[0])?.to_string();
            let start = index_arg(vm, id, &args[1])?;
            let len = index_arg(vm, id, &args[2])?;
            let chars: Vec<char> = s.chars().collect();
            if start > chars.len() || start + len > chars.len() {
                return Err(VmError::Runtime(format!(
                    "{}: range {}..{} out of bounds for length {}",
                    id.name(),
                    start,
                    start + len,
                    chars.len()
                )));
            }
            let sub: String = chars[start..start + len].iter().collect();
            Ok(alloc_string(vm, sub))
        }
        NativeId::StrCharAt => {
            let s = str_arg(vm, id, &args[0])?;
            let idx = index_arg(vm, id, &args[1])?;
            let count = s.chars().count();
            s.chars()
                .nth(idx)
                .map(Value::Char)
                .ok_or_else(|| oob(id, idx, count))
        }
        NativeId::StrContains => {
            let s = str_arg(vm, id, &args[0])?;
            let t = str_arg(vm, id, &args[1])?;
            Ok(Value::Bool(s.contains(t)))
        }
        NativeId::StrStartsWith => {
            let s = str_arg(vm, id, &args[0])?;
            let t = str_arg(vm, id, &args[1])?;
            Ok(Value::Bool(s.starts_with(t)))
        }
        NativeId::StrEndsWith => {
            let s = str_arg(vm, id, &args[0])?;
            let t = str_arg(vm, id, &args[1])?;
            Ok(Value::Bool(s.ends_with(t)))
        }
        NativeId::StrIndexOf => {
            let s = str_arg(vm, id, &args[0])?;
            let t = str_arg(vm, id, &args[1])?;
            // Report a character index, consistent with CharAt/Substring.
            let idx = s
                .find(t)
                .map(|byte_idx| s[..byte_idx].chars().count() as i64)
                .unwrap_or(-1);
            Ok(Value::Int(idx))
        }
        NativeId::StrSplit => {
            let s = str_arg(vm, id, &args[0])?.to_string();
            let sep = str_arg(vm, id, &args[1])?.to_string();
            if sep.is_empty() {
                return Err(VmError::Runtime(format!("{}: empty separator", id.name())));
            }
            let parts: Vec<String> = s.split(&sep).map(str::to_string).collect();
            let elements: Vec<Value> = parts.into_iter().map(|p| alloc_string(vm, p)).collect();
            let handle = vm.heap.allocate(AuraObject::Array { elements });
            Ok(Value::Object(handle))
        }
        NativeId::StrTrim => {
            let s = str_arg(vm, id, &args[0])?.trim().to_string();
            Ok(alloc_string(vm, s))
        }
        NativeId::StrToUpper => {
            let s = str_arg(vm, id, &args[0])?.to_uppercase();
            Ok(alloc_string(vm, s))
        }
        NativeId::StrToLower => {
            let s = str_arg(vm, id, &args[0])?.to_lowercase();
            Ok(alloc_string(vm, s))
        }
        NativeId::StrReplace => {
            let s = str_arg(vm, id, &args[0])?.to_string();
            let from = str_arg(vm, id, &args[1])?.to_string();
            let to = str_arg(vm, id, &args[2])?.to_string();
            if from.is_empty() {
                return Err(VmError::Runtime(format!("{}: empty search string", id.name())));
            }
            Ok(alloc_string(vm, s.replace(&from, &to)))
        }
        NativeId::StrToInt => {
            let s = str_arg(vm, id, &args[0])?;
            // The declared return type is `int` (int32), so parse and
            // range-check rather than silently truncating.
            s.trim()
                .parse::<i64>()
                .ok()
                .filter(|v| *v >= i32::MIN as i64 && *v <= i32::MAX as i64)
                .map(Value::Int)
                .ok_or_else(|| {
                    VmError::Runtime(format!("{}: cannot parse `{}` as int", id.name(), s))
                })
        }
        NativeId::StrToFloat => {
            let s = str_arg(vm, id, &args[0])?;
            s.trim()
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| VmError::Runtime(format!("{}: cannot parse `{}` as float", id.name(), s)))
        }

        // ---------------- Console ----------------
        NativeId::ConsoleReadLine => {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) => Ok(Value::Null),
                Ok(_) => {
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    Ok(alloc_string(vm, line))
                }
                Err(e) => Err(VmError::Runtime(format!("{}: {}", id.name(), e))),
            }
        }

        // ---------------- File ----------------
        NativeId::FileReadAllText => {
            let path = str_arg(vm, id, &args[0])?.to_string();
            std::fs::read_to_string(&path)
                .map(|s| alloc_string(vm, s))
                .map_err(|e| io_err(id, &path, e))
        }
        NativeId::FileWriteAllText => {
            let path = str_arg(vm, id, &args[0])?.to_string();
            let contents = str_arg(vm, id, &args[1])?.to_string();
            std::fs::write(&path, contents)
                .map(|_| Value::Unit)
                .map_err(|e| io_err(id, &path, e))
        }
        NativeId::FileAppendAllText => {
            use std::io::Write;
            let path = str_arg(vm, id, &args[0])?.to_string();
            let contents = str_arg(vm, id, &args[1])?.to_string();
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(contents.as_bytes()))
                .map(|_| Value::Unit)
                .map_err(|e| io_err(id, &path, e))
        }
        NativeId::FileExists => {
            let path = str_arg(vm, id, &args[0])?;
            Ok(Value::Bool(std::path::Path::new(path).exists()))
        }
        NativeId::FileDelete => {
            let path = str_arg(vm, id, &args[0])?.to_string();
            std::fs::remove_file(&path)
                .map(|_| Value::Unit)
                .map_err(|e| io_err(id, &path, e))
        }
        NativeId::FileReadAllLines => {
            let path = str_arg(vm, id, &args[0])?.to_string();
            let text = std::fs::read_to_string(&path).map_err(|e| io_err(id, &path, e))?;
            let elements: Vec<Value> = text
                .lines()
                .map(|l| alloc_string(vm, l.to_string()))
                .collect();
            let handle = vm.heap.allocate(AuraObject::Array { elements });
            Ok(Value::Object(handle))
        }
    }
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn wrong_receiver(id: NativeId, got: &Value) -> VmError {
    VmError::Runtime(format!(
        "{}: expected {} receiver, got {}",
        id.name(),
        id.name().split('.').next().unwrap_or("native"),
        got.type_name()
    ))
}

fn oob(id: NativeId, idx: usize, len: usize) -> VmError {
    VmError::Runtime(format!(
        "{}: index {} out of range (count {})",
        id.name(),
        idx,
        len
    ))
}

fn io_err(id: NativeId, path: &str, e: std::io::Error) -> VmError {
    VmError::Runtime(format!("{}: {}: {}", id.name(), path, e))
}

fn index_arg(vm: &Vm, id: NativeId, v: &Value) -> Result<usize, VmError> {
    match v {
        Value::Int(i) if *i >= 0 => Ok(*i as usize),
        Value::Int(i) => Err(VmError::Runtime(format!(
            "{}: negative index {}",
            id.name(),
            i
        ))),
        other => Err(VmError::Runtime(format!(
            "{}: expected int index, got {}",
            id.name(),
            crate::describe_value(vm, other)
        ))),
    }
}

fn str_arg<'a>(vm: &'a Vm, id: NativeId, v: &Value) -> Result<&'a str, VmError> {
    match v {
        Value::String(handle) => vm
            .heap
            .get_string(*handle)
            .ok_or_else(|| VmError::Runtime(format!("{}: dangling string handle", id.name()))),
        other => Err(wrong_receiver(id, other)),
    }
}

fn alloc_string(vm: &mut Vm, s: String) -> Value {
    Value::String(vm.heap.allocate(AuraObject::String(s)))
}

fn list_handle(vm: &Vm, id: NativeId, v: &Value) -> Result<GcRef, VmError> {
    match v {
        Value::Object(h) if matches!(vm.heap.get(*h), Ok(AuraObject::Array { .. })) => Ok(*h),
        other => Err(wrong_receiver(id, other)),
    }
}

fn list_ref<'a>(vm: &'a Vm, h: GcRef) -> Result<&'a Vec<Value>, VmError> {
    match vm.heap.get(h)? {
        AuraObject::Array { elements } => Ok(elements),
        _ => unreachable!("checked by list_handle"),
    }
}

fn list_mut<'a>(vm: &'a mut Vm, h: GcRef) -> Result<&'a mut Vec<Value>, VmError> {
    match vm.heap.get_mut(h)? {
        AuraObject::Array { elements } => Ok(elements),
        _ => unreachable!("checked by list_handle"),
    }
}

fn map_handle(vm: &Vm, id: NativeId, v: &Value) -> Result<GcRef, VmError> {
    match v {
        Value::Object(h) if matches!(vm.heap.get(*h), Ok(AuraObject::Map { .. })) => Ok(*h),
        other => Err(wrong_receiver(id, other)),
    }
}

fn map_ref<'a>(vm: &'a Vm, h: GcRef) -> Result<&'a Vec<(Value, Value)>, VmError> {
    match vm.heap.get(h)? {
        AuraObject::Map { entries } => Ok(entries),
        _ => unreachable!("checked by map_handle"),
    }
}

fn map_mut<'a>(vm: &'a mut Vm, h: GcRef) -> Result<&'a mut Vec<(Value, Value)>, VmError> {
    match vm.heap.get_mut(h)? {
        AuraObject::Map { entries } => Ok(entries),
        _ => unreachable!("checked by map_handle"),
    }
}

fn set_handle(vm: &Vm, id: NativeId, v: &Value) -> Result<GcRef, VmError> {
    match v {
        Value::Object(h) if matches!(vm.heap.get(*h), Ok(AuraObject::Set { .. })) => Ok(*h),
        other => Err(wrong_receiver(id, other)),
    }
}

fn set_ref<'a>(vm: &'a Vm, h: GcRef) -> Result<&'a Vec<Value>, VmError> {
    match vm.heap.get(h)? {
        AuraObject::Set { elements } => Ok(elements),
        _ => unreachable!("checked by set_handle"),
    }
}

fn set_mut<'a>(vm: &'a mut Vm, h: GcRef) -> Result<&'a mut Vec<Value>, VmError> {
    match vm.heap.get_mut(h)? {
        AuraObject::Set { elements } => Ok(elements),
        _ => unreachable!("checked by set_handle"),
    }
}
