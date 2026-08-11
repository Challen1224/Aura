//! Minimal binary encoding for Aura modules.
//!
//! This is intentionally simple: length-prefixed strings and vectors, with
//! opcodes stored as a single byte plus fixed-size operands.

use crate::{ClassDef, ClassId, EnumDef, EnumId, FieldDef, GenericParam, MethodDef, MethodId, Module, Op, TypeDesc, VariantDef, Variance};
use std::collections::HashMap;
use std::io::{self, Read, Write};

/// Serialize a module to a byte vector.
pub fn encode(module: &Module) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    write_string(&mut buf, &module.name)?;
    write_u32(&mut buf, module.entrypoint.map(|m| m.0).unwrap_or(u32::MAX))?;

    write_vec(&mut buf, &module.constant_pool, |w, s| write_string(w, s))?;

    write_u32(&mut buf, module.classes.len() as u32)?;
    for (id, class) in &module.classes {
        write_u32(&mut buf, id.0)?;
        write_class(&mut buf, class)?;
    }

    write_u32(&mut buf, module.enums.len() as u32)?;
    for (id, en) in &module.enums {
        write_u32(&mut buf, id.0)?;
        write_enum(&mut buf, en)?;
    }

    Ok(buf)
}

/// Deserialize a module from a byte reader.
pub fn decode<R: Read>(mut reader: R) -> io::Result<Module> {
    let name = read_string(&mut reader)?;
    let ep_raw = read_u32(&mut reader)?;
    let entrypoint = if ep_raw == u32::MAX {
        None
    } else {
        Some(MethodId(ep_raw))
    };

    let constant_pool = read_vec(&mut reader, |r| read_string(r))?;

    let class_count = read_u32(&mut reader)? as usize;
    let mut classes = HashMap::with_capacity(class_count);
    for _ in 0..class_count {
        let id = ClassId(read_u32(&mut reader)?);
        let class = read_class(&mut reader)?;
        classes.insert(id, class);
    }

    let enum_count = read_u32(&mut reader)? as usize;
    let mut enums = HashMap::with_capacity(enum_count);
    for _ in 0..enum_count {
        let id = EnumId(read_u32(&mut reader)?);
        let en = read_enum(&mut reader)?;
        enums.insert(id, en);
    }

    Ok(Module {
        name,
        entrypoint,
        constant_pool,
        classes,
        enums,
    })
}

fn write_class<W: Write>(w: &mut W, class: &ClassDef) -> io::Result<()> {
    write_string(w, &class.name)?;
    write_vec(w, &class.generic_params, |w, gp| write_generic_param(w, gp))?;
    write_u32(w, class.super_class.map(|c| c.0).unwrap_or(u32::MAX))?;
    write_vec(w, &class.interfaces, |w, i| write_u32(w, i.0))?;
    write_u8(w, class.is_interface as u8)?;
    write_u8(w, class.is_abstract as u8)?;
    write_vec(w, &class.fields, |w, f| write_field(w, f))?;
    write_vec(w, &class.static_fields, |w, f| write_field(w, f))?;

    write_u32(w, (class.methods.len() + class.static_methods.len()) as u32)?;
    for (id, m) in &class.methods {
        write_u32(w, id.0)?;
        write_u8(w, 1)?;
        write_method(w, m)?;
    }
    for (id, m) in &class.static_methods {
        write_u32(w, id.0)?;
        write_u8(w, 0)?;
        write_method(w, m)?;
    }
    Ok(())
}

fn read_class<R: Read>(r: &mut R) -> io::Result<ClassDef> {
    let name = read_string(r)?;
    let generic_params = read_vec(r, |r| read_generic_param(r))?;
    let super_raw = read_u32(r)?;
    let super_class = if super_raw == u32::MAX {
        None
    } else {
        Some(ClassId(super_raw))
    };
    let interfaces = read_vec(r, |r| Ok(ClassId(read_u32(r)?)))?;
    let is_interface = read_u8(r)? != 0;
    let is_abstract = read_u8(r)? != 0;
    let fields = read_vec(r, |r| read_field(r))?;
    let static_fields = read_vec(r, |r| read_field(r))?;

    let method_count = read_u32(r)? as usize;
    let mut methods = HashMap::with_capacity(method_count);
    let mut static_methods = HashMap::with_capacity(method_count);
    for _ in 0..method_count {
        let id = MethodId(read_u32(r)?);
        let is_instance = read_u8(r)? != 0;
        let m = read_method(r)?;
        if is_instance {
            methods.insert(id, m);
        } else {
            static_methods.insert(id, m);
        }
    }
    Ok(ClassDef {
        name,
        generic_params,
        super_class,
        interfaces,
        is_interface,
        is_abstract,
        fields,
        static_fields,
        methods,
        static_methods,
    })
}

fn write_enum<W: Write>(w: &mut W, en: &EnumDef) -> io::Result<()> {
    write_string(w, &en.name)?;
    write_vec(w, &en.variants, |w, v| write_variant(w, v))
}

fn read_enum<R: Read>(r: &mut R) -> io::Result<EnumDef> {
    let name = read_string(r)?;
    let variants = read_vec(r, |r| read_variant(r))?;
    Ok(EnumDef { name, variants })
}

fn write_variant<W: Write>(w: &mut W, v: &VariantDef) -> io::Result<()> {
    write_string(w, &v.name)?;
    write_vec(w, &v.fields, |w, f| write_field(w, f))
}

fn read_variant<R: Read>(r: &mut R) -> io::Result<VariantDef> {
    let name = read_string(r)?;
    let fields = read_vec(r, |r| read_field(r))?;
    Ok(VariantDef { name, fields })
}

fn write_field<W: Write>(w: &mut W, f: &FieldDef) -> io::Result<()> {
    write_string(w, &f.name)?;
    write_type(w, &f.ty)
}

fn read_field<R: Read>(r: &mut R) -> io::Result<FieldDef> {
    let name = read_string(r)?;
    let ty = read_type(r)?;
    Ok(FieldDef { name, ty })
}

fn write_method<W: Write>(w: &mut W, m: &MethodDef) -> io::Result<()> {
    write_string(w, &m.name)?;
    write_type(w, &m.return_ty)?;
    write_vec(w, &m.params, |w, p| write_type(w, p))?;
    write_vec(w, &m.generic_params, |w, gp| write_generic_param(w, gp))?;
    write_u16(w, m.max_stack)?;
    write_u16(w, m.locals)?;
    write_vec(w, &m.body, |w, op| write_op(w, op))
}

fn read_method<R: Read>(r: &mut R) -> io::Result<MethodDef> {
    let name = read_string(r)?;
    let return_ty = read_type(r)?;
    let params = read_vec(r, |r| read_type(r))?;
    let generic_params = read_vec(r, |r| read_generic_param(r))?;
    let max_stack = read_u16(r)?;
    let locals = read_u16(r)?;
    let body = read_vec(r, |r| read_op(r))?;
    Ok(MethodDef {
        name,
        return_ty,
        params,
        generic_params,
        is_instance: false, // set by caller
        body,
        max_stack,
        locals,
    })
}

fn write_type<W: Write>(w: &mut W, t: &TypeDesc) -> io::Result<()> {
    let tag: u8 = match t {
        TypeDesc::Unit => 0,
        TypeDesc::Int => 1,
        TypeDesc::Float => 2,
        TypeDesc::Bool => 3,
        TypeDesc::String => 4,
        TypeDesc::Class(_, _) => 5,
        TypeDesc::Null => 6,
        TypeDesc::GenericParam(_) => 7,
        TypeDesc::Enum(_) => 8,
        TypeDesc::Tuple(_) => 9,
    };
    write_u8(w, tag)?;
    match t {
        TypeDesc::Class(ClassId(id), args) => {
            write_u32(w, *id)?;
            write_vec(w, args, |w, arg| write_type(w, arg))?;
        }
        TypeDesc::GenericParam(idx) => {
            write_u32(w, *idx)?;
        }
        TypeDesc::Enum(EnumId(id)) => {
            write_u32(w, *id)?;
        }
        TypeDesc::Tuple(types) => {
            write_vec(w, types, |w, ty| write_type(w, ty))?;
        }
        _ => {}
    }
    Ok(())
}

fn read_type<R: Read>(r: &mut R) -> io::Result<TypeDesc> {
    let tag = read_u8(r)?;
    Ok(match tag {
        0 => TypeDesc::Unit,
        1 => TypeDesc::Int,
        2 => TypeDesc::Float,
        3 => TypeDesc::Bool,
        4 => TypeDesc::String,
        5 => {
            let id = ClassId(read_u32(r)?);
            let args = read_vec(r, |r| read_type(r))?;
            TypeDesc::Class(id, args)
        }
        6 => TypeDesc::Null,
        7 => TypeDesc::GenericParam(read_u32(r)?),
        8 => TypeDesc::Enum(EnumId(read_u32(r)?)),
        9 => TypeDesc::Tuple(read_vec(r, |r| read_type(r))?),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "bad type tag")),
    })
}

fn write_generic_param<W: Write>(w: &mut W, gp: &GenericParam) -> io::Result<()> {
    write_string(w, &gp.name)?;
    match &gp.constraint {
        Some(ty) => {
            write_u8(w, 1)?;
            write_type(w, ty)?;
        }
        None => write_u8(w, 0)?,
    }
    let variance: u8 = match gp.variance {
        Variance::Covariant => 0,
        Variance::Contravariant => 1,
        Variance::Invariant => 2,
    };
    write_u8(w, variance)
}

fn read_generic_param<R: Read>(r: &mut R) -> io::Result<GenericParam> {
    let name = read_string(r)?;
    let has_constraint = read_u8(r)? != 0;
    let constraint = if has_constraint {
        Some(read_type(r)?)
    } else {
        None
    };
    let variance = match read_u8(r)? {
        0 => Variance::Covariant,
        1 => Variance::Contravariant,
        2 => Variance::Invariant,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "bad variance")),
    };
    Ok(GenericParam {
        name,
        constraint,
        variance,
    })
}

fn write_op<W: Write>(w: &mut W, op: &Op) -> io::Result<()> {
    let (code, extra): (u8, Vec<u8>) = match op {
        Op::Nop => (0, vec![]),
        Op::LdInt(i) => (1, i.to_le_bytes().to_vec()),
        Op::LdFloat(idx) => (2, idx.to_le_bytes().to_vec()),
        Op::LdBool(b) => (3, vec![*b as u8]),
        Op::LdStr(idx) => (4, idx.to_le_bytes().to_vec()),
        Op::LdNull => (5, vec![]),
        Op::Ldloc(idx) => (6, idx.to_le_bytes().to_vec()),
        Op::Stloc(idx) => (7, idx.to_le_bytes().to_vec()),
        Op::Ldarg(idx) => (8, idx.to_le_bytes().to_vec()),
        Op::Dup => (9, vec![]),
        Op::Pop => (10, vec![]),
        Op::Add => (20, vec![]),
        Op::Sub => (21, vec![]),
        Op::Mul => (22, vec![]),
        Op::Div => (23, vec![]),
        Op::Rem => (24, vec![]),
        Op::Neg => (25, vec![]),
        Op::Eq => (30, vec![]),
        Op::Lt => (31, vec![]),
        Op::Le => (32, vec![]),
        Op::Gt => (33, vec![]),
        Op::Ge => (34, vec![]),
        Op::Not => (35, vec![]),
        Op::And => (36, vec![]),
        Op::Or => (37, vec![]),
        Op::Br(off) => (40, off.to_le_bytes().to_vec()),
        Op::BrFalse(off) => (41, off.to_le_bytes().to_vec()),
        Op::BrTrue(off) => (42, off.to_le_bytes().to_vec()),
        Op::Call(MethodId(id)) => (50, id.to_le_bytes().to_vec()),
        Op::CallVirt(name) => (51, encode_string_bytes(name)),
        Op::CallSuper(MethodId(id)) => (52, id.to_le_bytes().to_vec()),
        Op::Ret => (60, vec![]),
        Op::Break => (61, vec![]),
        Op::Continue => (62, vec![]),
        Op::NewObj(ClassId(id), type_args) => {
            let mut v = id.to_le_bytes().to_vec();
            // Encode type arguments
            let args_bytes = encode_type_args(type_args);
            v.extend(args_bytes);
            (70, v)
        }
        Op::Ldfld(idx) => (71, idx.to_le_bytes().to_vec()),
        Op::Stfld(idx) => (72, idx.to_le_bytes().to_vec()),
        Op::Ldsfld(ClassId(cid), idx) => {
            let mut v = cid.to_le_bytes().to_vec();
            v.extend(idx.to_le_bytes());
            (73, v)
        }
        Op::Stsfld(ClassId(cid), idx) => {
            let mut v = cid.to_le_bytes().to_vec();
            v.extend(idx.to_le_bytes());
            (74, v)
        }
        Op::Print => (80, vec![]),
        Op::PrintLn => (81, vec![]),
        Op::NewEnum(EnumId(id), variant_idx) => {
            let mut v = id.to_le_bytes().to_vec();
            v.extend(variant_idx.to_le_bytes());
            (90, v)
        }
        Op::EnumTag => (91, vec![]),
        Op::EnumField(idx) => (92, idx.to_le_bytes().to_vec()),
        Op::NewTuple(count) => (100, count.to_le_bytes().to_vec()),
        Op::TupleField(idx) => (101, idx.to_le_bytes().to_vec()),
        Op::StringConcat(count) => (102, count.to_le_bytes().to_vec()),
    };
    write_u8(w, code)?;
    w.write_all(&extra)
}

fn read_op<R: Read>(r: &mut R) -> io::Result<Op> {
    let code = read_u8(r)?;
    Ok(match code {
        0 => Op::Nop,
        1 => Op::LdInt(read_i32(r)?),
        2 => Op::LdFloat(read_u32(r)?),
        3 => Op::LdBool(read_u8(r)? != 0),
        4 => Op::LdStr(read_u32(r)?),
        5 => Op::LdNull,
        6 => Op::Ldloc(read_u16(r)?),
        7 => Op::Stloc(read_u16(r)?),
        8 => Op::Ldarg(read_u16(r)?),
        9 => Op::Dup,
        10 => Op::Pop,
        20 => Op::Add,
        21 => Op::Sub,
        22 => Op::Mul,
        23 => Op::Div,
        24 => Op::Rem,
        25 => Op::Neg,
        30 => Op::Eq,
        31 => Op::Lt,
        32 => Op::Le,
        33 => Op::Gt,
        34 => Op::Ge,
        35 => Op::Not,
        36 => Op::And,
        37 => Op::Or,
        40 => Op::Br(read_u32(r)?),
        41 => Op::BrFalse(read_u32(r)?),
        42 => Op::BrTrue(read_u32(r)?),
        50 => Op::Call(MethodId(read_u32(r)?)),
        51 => Op::CallVirt(read_string(r)?),
        52 => Op::CallSuper(MethodId(read_u32(r)?)),
        60 => Op::Ret,
        61 => Op::Break,
        62 => Op::Continue,
        70 => {
            let id = ClassId(read_u32(r)?);
            let type_args = decode_type_args(r)?;
            Op::NewObj(id, type_args)
        }
        71 => Op::Ldfld(read_u16(r)?),
        72 => Op::Stfld(read_u16(r)?),
        73 => Op::Ldsfld(ClassId(read_u32(r)?), read_u16(r)?),
        74 => Op::Stsfld(ClassId(read_u32(r)?), read_u16(r)?),
        80 => Op::Print,
        81 => Op::PrintLn,
        90 => {
            let id = EnumId(read_u32(r)?);
            let variant_idx = read_u16(r)?;
            Op::NewEnum(id, variant_idx)
        }
        91 => Op::EnumTag,
        92 => Op::EnumField(read_u16(r)?),
        100 => Op::NewTuple(read_u16(r)?),
        101 => Op::TupleField(read_u16(r)?),
        102 => Op::StringConcat(read_u16(r)?),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "bad opcode")),
    })
}

fn write_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    let bytes = encode_string_bytes(s);
    w.write_all(&bytes)
}

fn encode_string_bytes(s: &str) -> Vec<u8> {
    let raw = s.as_bytes();
    let mut v = Vec::with_capacity(4 + raw.len());
    v.extend((raw.len() as u32).to_le_bytes());
    v.extend(raw);
    v
}

fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
    let len = read_u32(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn write_vec<W: Write, T, F>(w: &mut W, items: &[T], mut write_item: F) -> io::Result<()>
where
    F: FnMut(&mut W, &T) -> io::Result<()>,
{
    write_u32(w, items.len() as u32)?;
    for item in items {
        write_item(w, item)?;
    }
    Ok(())
}

fn read_vec<R: Read, T, F>(r: &mut R, mut read_item: F) -> io::Result<Vec<T>>
where
    F: FnMut(&mut R) -> io::Result<T>,
{
    let len = read_u32(r)? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(read_item(r)?);
    }
    Ok(out)
}

fn write_u8<W: Write>(w: &mut W, v: u8) -> io::Result<()> {
    w.write_all(&[v])
}
fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn write_u16<W: Write>(w: &mut W, v: u16) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn read_u16<R: Read>(r: &mut R) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn write_u32<W: Write>(w: &mut W, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_le_bytes(b))
}

fn encode_type_args(type_args: &[TypeDesc]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend((type_args.len() as u32).to_le_bytes());
    for arg in type_args {
        let tag: u8 = match arg {
            TypeDesc::Unit => 0,
            TypeDesc::Int => 1,
            TypeDesc::Float => 2,
            TypeDesc::Bool => 3,
            TypeDesc::String => 4,
            TypeDesc::Class(id, _) => {
                v.extend(id.0.to_le_bytes());
                5
            }
            TypeDesc::GenericParam(idx) => {
                v.extend(idx.to_le_bytes());
                7
            }
            TypeDesc::Null => 6,
            TypeDesc::Enum(id) => {
                v.extend(id.0.to_le_bytes());
                8
            }
            TypeDesc::Tuple(_) => {
                // For now, we don't support nested tuples in type args
                // This could be extended in the future
                9
            }
        };
        v.push(tag);
    }
    v
}

fn decode_type_args<R: Read>(r: &mut R) -> io::Result<Vec<TypeDesc>> {
    let count = read_u32(r)? as usize;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_u8(r)?;
        let arg = match tag {
            0 => TypeDesc::Unit,
            1 => TypeDesc::Int,
            2 => TypeDesc::Float,
            3 => TypeDesc::Bool,
            4 => TypeDesc::String,
            5 => {
                let id = ClassId(read_u32(r)?);
                TypeDesc::Class(id, vec![])
            }
            6 => TypeDesc::Null,
            7 => TypeDesc::GenericParam(read_u32(r)?),
            8 => TypeDesc::Enum(EnumId(read_u32(r)?)),
            9 => TypeDesc::Tuple(vec![]), // Simplified: empty tuple for now
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "bad type tag")),
        };
        args.push(arg);
    }
    Ok(args)
}
