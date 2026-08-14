//! Compact 16-byte value model used by JIT-generated code.
//!
//! The interpreter's [`Value`] is exactly 16 bytes: an 8-byte tag word at
//! offset 0 followed by an 8-byte payload word at offset 8. JIT code treats a
//! value slot as `[tag: u64][payload: u64]`. The constants below are verified
//! against real `Value` instances by [`verify_layout`].

use aura_bytecode::{GcRef, Value};

/// `Value::Unit` tag.
pub const TAG_UNIT: u64 = 0;
/// `Value::Int` tag.
pub const TAG_INT: u64 = 1;
/// `Value::Float` tag.
pub const TAG_FLOAT: u64 = 2;
/// `Value::Bool` tag (truth bit packed at bit 8).
pub const TAG_BOOL: u64 = 3;
/// `Value::Char` tag (char code packed at bit 32 to match Rust's layout).
pub const TAG_CHAR: u64 = 4;
/// `Value::String` tag.
pub const TAG_STRING: u64 = 5;
/// `Value::Object` tag.
pub const TAG_OBJECT: u64 = 6;
/// `Value::Enum` tag.
pub const TAG_ENUM: u64 = 7;
/// `Value::Tuple` tag.
pub const TAG_TUPLE: u64 = 8;
/// `Value::Null` tag.
pub const TAG_NULL: u64 = 9;

/// Byte offset of the tag word within a `Value` slot.
pub const TAG_OFF: usize = 0;
/// Byte offset of the payload word within a `Value` slot.
pub const PAYLOAD_OFF: usize = 8;

/// Read the tag word of a value slot.
///
/// # Safety
/// `ptr` must point to 16 readable bytes.
pub unsafe fn read_tag(ptr: *const Value) -> u64 {
    (ptr as *const u8 as *const u64).read()
}

/// Read the payload word of a value slot.
///
/// # Safety
/// `ptr` must point to 16 readable bytes.
pub unsafe fn read_payload(ptr: *const Value) -> u64 {
    (ptr as *const u8 as *const u64).add(1).read()
}

/// Write a 16-byte value slot from tag + payload.
///
/// # Safety
/// `ptr` must point to 16 writable bytes.
pub unsafe fn write_parts(ptr: *mut Value, tag: u64, payload: u64) {
    (ptr as *mut u8 as *mut u64).write(tag);
    (ptr as *mut u8 as *mut u64).add(1).write(payload);
}

/// Reconstruct a `Value` from its raw parts read out of a slot.
///
/// # Safety
/// `ptr` must point to 16 readable bytes.
pub unsafe fn read_value(ptr: *const Value) -> Value {
    let mut v = Value::Unit;
    std::ptr::copy_nonoverlapping(ptr as *const u8, &mut v as *mut Value as *mut u8, 16);
    v
}

/// Copy 16 bytes from `src` into the `Value` at `dst`.
///
/// # Safety
/// `src` and `dst` must point to 16 bytes; `dst` must be aligned for `Value`.
pub unsafe fn copy_slot(src: *const Value, dst: *mut Value) {
    std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, 16);
}

/// Decompose a `Value` into its canonical `(tag, payload)` slot parts.
///
/// This must be used whenever a Rust-constructed `Value` is handed to
/// generated code: a plain Rust assignment only writes the bytes the enum
/// layout needs (discriminant plus inline data) and leaves the rest of the
/// 16-byte slot as stale garbage, but generated code reads the tag and
/// payload as whole qwords and relies on the unused bits being zero.
pub fn to_parts(v: &Value) -> (u64, u64) {
    match v {
        Value::Unit => (TAG_UNIT, 0),
        Value::Int(i) => (TAG_INT, *i as u64),
        Value::Float(f) => (TAG_FLOAT, f.to_bits()),
        Value::Bool(b) => (TAG_BOOL | (*b as u64) << 8, 0),
        Value::Char(c) => (TAG_CHAR | (*c as u64) << 32, 0),
        Value::String(GcRef(r)) => (TAG_STRING, *r as u64),
        Value::Object(GcRef(r)) => (TAG_OBJECT, *r as u64),
        Value::Enum(GcRef(r)) => (TAG_ENUM, *r as u64),
        Value::Tuple(GcRef(r)) => (TAG_TUPLE, *r as u64),
        Value::Null => (TAG_NULL, 0),
    }
}

/// Rewrite the `Value` at `ptr` in canonical slot form (all 16 bytes
/// written, unused bits zeroed). See [`to_parts`].
///
/// # Safety
/// `ptr` must point to a valid, aligned `Value`.
pub unsafe fn canonicalize(ptr: *mut Value) {
    let (tag, payload) = to_parts(&*ptr);
    write_parts(ptr, tag, payload);
}

/// True if the value slot at `ptr` is an integer.
///
/// # Safety
/// `ptr` must point to 16 readable bytes.
pub unsafe fn is_int(ptr: *const Value) -> bool {
    read_tag(ptr) == TAG_INT
}

/// True if the value slot at `ptr` is a float.
///
/// # Safety
/// `ptr` must point to 16 readable bytes.
pub unsafe fn is_float(ptr: *const Value) -> bool {
    read_tag(ptr) == TAG_FLOAT
}

/// True if the value slot at `ptr` is truthy (matches `Value::is_truthy`).
///
/// # Safety
/// `ptr` must point to 16 readable bytes.
pub unsafe fn is_truthy(ptr: *const Value) -> bool {
    let tag = read_tag(ptr);
    let payload = read_payload(ptr);
    match tag {
        TAG_BOOL => (tag >> 8) & 1 != 0,
        TAG_INT => payload != 0,
        // `f != 0.0`: 0.0 and -0.0 are falsy, NaN truthy. The payload bits
        // shifted left are 0 only for 0.0 and -0.0.
        TAG_FLOAT => payload << 1 != 0,
        TAG_UNIT | TAG_NULL => false,
        _ => true,
    }
}

/// `GcRef` payload of a reference value (string/object/enum/tuple).
pub fn gc_ref(tag: u64, payload: u64) -> GcRef {
    debug_assert!(matches!(tag, TAG_STRING | TAG_OBJECT | TAG_ENUM | TAG_TUPLE));
    GcRef(payload as usize)
}

/// Verify the boxed slot layout the JIT relies on: a 16-byte `Value` with an
/// 8-byte tag word at offset 0 and an 8-byte payload word at offset 8. The
/// canonical representation is what `write_parts` produces and `read_value`
/// reconstructs. (The in-memory layout of a stack `Value` enum is not
/// meaningful here: repr(Rust) padding and niche words are unspecified, so we
/// round-trip through a slot instead of inspecting raw enum bytes.) Called
/// from a unit test so the JIT's assumptions are checked on every build.
pub fn verify_layout() -> Result<(), String> {
    unsafe {
        let roundtrip = |name: &str, tag: u64, payload: u64, expected: Value| -> Result<(), String> {
            let mut slot = Value::Unit;
            write_parts(&mut slot, tag, payload);
            let t = read_tag(&slot);
            let pay = read_payload(&slot);
            if t != tag || pay != payload {
                return Err(format!(
                    "layout mismatch for {name}: expected tag {tag:#x} payload {payload:#x}, got tag {t:#x} payload {pay:#x}"
                ));
            }
            let v = read_value(&slot);
            if v != expected {
                return Err(format!(
                    "round-trip mismatch for {name}: wrote tag {tag:#x} payload {payload:#x}, read back {v:?}"
                ));
            }
            let (ct, cp) = to_parts(&expected);
            if (ct, cp) != (tag, payload) {
                return Err(format!(
                    "to_parts mismatch for {name}: expected ({tag:#x}, {payload:#x}), got ({ct:#x}, {cp:#x})"
                ));
            }
            Ok(())
        };
        roundtrip("Unit", TAG_UNIT, 0, Value::Unit)?;
        roundtrip("Int", TAG_INT, 0x1122334455667788, Value::Int(0x1122334455667788))?;
        roundtrip("Float", TAG_FLOAT, 3.5f64.to_bits(), Value::Float(3.5))?;
        roundtrip("Bool(true)", TAG_BOOL | (1 << 8), 0, Value::Bool(true))?;
        roundtrip("Bool(false)", TAG_BOOL, 0, Value::Bool(false))?;
        roundtrip("Char", TAG_CHAR | ('A' as u64) << 32, 0, Value::Char('A'))?;
        roundtrip("String", TAG_STRING, 7, Value::String(GcRef(7)))?;
        roundtrip("Object", TAG_OBJECT, 9, Value::Object(GcRef(9)))?;
        roundtrip("Null", TAG_NULL, 0, Value::Null)?;
    }
    if std::mem::size_of::<Value>() != 16 {
        return Err(format!("Value is {} bytes, expected 16", std::mem::size_of::<Value>()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_verified() {
        verify_layout().unwrap();
    }

    #[test]
    fn roundtrip_slot() {
        unsafe {
            let mut slot = Value::Unit;
            write_parts(&mut slot, TAG_FLOAT, 1.25f64.to_bits());
            assert!(is_float(&slot));
            assert!(!is_int(&slot));
            assert!(is_truthy(&slot));
            let v = read_value(&slot);
            assert_eq!(v, Value::Float(1.25));
            write_parts(&mut slot, TAG_BOOL, 0);
            assert!(!is_truthy(&slot));
        }
    }
}
