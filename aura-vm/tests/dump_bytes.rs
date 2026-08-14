use aura_bytecode::{GcRef, Value};

#[test]
fn dump_enum_bytes() {
    let cases: Vec<Value> = vec![
        Value::Unit,
        Value::Int(0x1122334455667788),
        Value::Null,
        Value::Object(GcRef(0xdeadbeef)),
        Value::Bool(true),
        Value::Bool(false),
        Value::Float(1.0),
    ];
    for v in cases {
        let p = &v as *const Value as *const u8;
        let bytes: Vec<u8> = unsafe { std::slice::from_raw_parts(p, 16).to_vec() };
        println!("{v:?}: {:02x?}", bytes);
    }
}
