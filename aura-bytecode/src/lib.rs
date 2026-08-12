//! Aura bytecode ISA and runtime value model.
//!
//! This crate is intentionally dependency-light: it defines instructions, the
//! object/value layout, method/class metadata, and serialization so the VM
//! and compiler can share a common representation without pulling in
//! execution or parsing code.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

pub mod encode;

/// An Aura primitive value.
///
/// Reference types (`AuraObject`) live behind `GcRef` handles into the VM heap.
/// All integer types (int8..int64, uint8..uint64) are stored in the single
/// `Int(i64)` slot; width is carried by static types and conversions.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Unit / void result.
    Unit,
    /// Signed or unsigned integer value of any width.
    Int(i64),
    /// 64-bit floating point number (float32 values are rounded to f32
    /// precision on conversion but stored in the same slot).
    Float(f64),
    /// Boolean.
    Bool(bool),
    /// Immutable string handle.
    String(GcRef),
    /// Object instance handle.
    Object(GcRef),
    /// Enum value: (enum_id, variant_index, field_values).
    Enum(u32, u32, Vec<Value>),
    /// Tuple value: (element_values).
    Tuple(Vec<Value>),
    /// Null reference.
    Null,
}

impl Value {
    /// Returns true for null or unit where a reference is expected.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null | Value::Unit)
    }

    /// True if this value is truthy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Null | Value::Unit => false,
            _ => true,
        }
    }

    /// Type tag used for diagnostics and runtime type checks.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::String(_) => "string",
            Value::Object(_) => "object",
            Value::Enum(..) => "enum",
            Value::Tuple(_) => "tuple",
            Value::Null => "null",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "()"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(fl) => write!(f, "{fl}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::String(_) => write!(f, "<string>"),
            Value::Object(_) => write!(f, "<object>"),
            Value::Enum(enum_id, variant_idx, fields) => {
                write!(f, "enum({enum_id}:{variant_idx})")?;
                if !fields.is_empty() {
                    write!(f, "(")?;
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 { write!(f, ", ")?; }
                        write!(f, "{field}")?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Value::Tuple(elements) => {
                write!(f, "(")?;
                for (i, elem) in elements.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{elem}")?;
                }
                write!(f, ")")
            }
            Value::Null => write!(f, "null"),
        }
    }
}

/// A handle into the managed heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GcRef(pub usize);

/// An object on the managed heap.
#[derive(Debug, Clone, PartialEq)]
pub enum AuraObject {
    /// Immutable UTF-8 string.
    String(String),
    /// Class instance with field slots indexed by field metadata.
    Instance {
        /// Class metadata id.
        class_id: ClassId,
        /// Field values in declaration order.
        fields: Vec<Value>,
    },
    /// Array of values (for future array support).
    Array {
        /// Element values.
        elements: Vec<Value>,
    },
}

impl AuraObject {
    /// Returns true if this object contains any reference fields.
    pub fn has_references(&self) -> bool {
        match self {
            AuraObject::String(_) | AuraObject::Array { .. } | AuraObject::Instance { .. } => true,
        }
    }

    /// Iterate references held by this object (for GC tracing).
    pub fn references(&self) -> Vec<GcRef> {
        let mut refs = Vec::new();
        match self {
            AuraObject::String(_) => {}
            AuraObject::Instance { fields, .. } | AuraObject::Array { elements: fields } => {
                for v in fields {
                    match v {
                        Value::String(r) | Value::Object(r) => refs.push(*r),
                        _ => {}
                    }
                }
            }
        }
        refs
    }
}

/// Unique identifier for a class in a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClassId(pub u32);

/// Unique identifier for a method in a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MethodId(pub u32);

/// Unique identifier for an enum in a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

/// Generic type parameter descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericParam {
    /// Parameter name (e.g., "T", "K", "V").
    pub name: String,
    /// Optional constraint (interface/class that T must implement/extend).
    pub constraint: Option<TypeDesc>,
    /// Variance: covariant (+), contravariant (-), invariant (none).
    pub variance: Variance,
}

/// Variance annotation for generic parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variance {
    /// Covariant (out) - can be used as return type.
    Covariant,
    /// Contravariant (in) - can be used as parameter type.
    Contravariant,
    /// Invariant - no variance.
    Invariant,
}

/// Field descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    /// Field name.
    pub name: String,
    /// Type descriptor.
    pub ty: TypeDesc,
}

/// Method descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDef {
    /// Method name.
    pub name: String,
    /// Return type descriptor.
    pub return_ty: TypeDesc,
    /// Parameter type descriptors.
    pub params: Vec<TypeDesc>,
    /// Generic parameters for this method.
    pub generic_params: Vec<GenericParam>,
    /// Whether the method is an instance method.
    pub is_instance: bool,
    /// Bytecode body, empty for runtime/intrinsic methods.
    pub body: Vec<Op>,
    /// Exception handler table covering ranges of the bytecode body.
    pub handlers: Vec<ExceptionHandler>,
    /// Maximum stack depth required by this method.
    pub max_stack: u16,
    /// Number of local variables (parameters + locals).
    pub locals: u16,
}

/// A protected region of a method's bytecode.
///
/// The region spans instructions `[start, end)` (inclusive/exclusive). When an
/// exception is raised at a protected instruction, the innermost applicable
/// handler takes over: a matching `catch` clause, or a `finally` clause that
/// re-propagates the exception after running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionHandler {
    /// Start pc of the protected region (inclusive).
    pub start: u32,
    /// End pc of the protected region (exclusive).
    pub end: u32,
    /// Type of exception caught. `None` for a `finally` handler.
    pub catch_type: Option<TypeDesc>,
    /// Pc of the handler body.
    pub handler_pc: u32,
    /// Local slot that receives the caught exception (catch handlers only).
    pub catch_local: u16,
}

/// Class descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    /// Class name.
    pub name: String,
    /// Generic parameters for this class.
    pub generic_params: Vec<GenericParam>,
    /// Super class id, if any (single inheritance).
    pub super_class: Option<ClassId>,
    /// Interface ids implemented (directly) by this class, or extended by this interface.
    pub interfaces: Vec<ClassId>,
    /// Whether this declaration is an interface.
    pub is_interface: bool,
    /// Whether this class is abstract (cannot be instantiated).
    pub is_abstract: bool,
    /// Whether this is a record class (value semantics, structural equality).
    pub is_record: bool,
    /// Instance fields in layout order (base class fields first, then own).
    pub fields: Vec<FieldDef>,
    /// Static fields declared by this class (not inherited).
    pub static_fields: Vec<FieldDef>,
    /// Methods declared by this class, keyed by method id.
    pub methods: HashMap<MethodId, MethodDef>,
    /// Static methods.
    pub static_methods: HashMap<MethodId, MethodDef>,
}

/// Enum variant descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantDef {
    /// Variant name.
    pub name: String,
    /// Fields (empty for simple variants).
    pub fields: Vec<FieldDef>,
}

/// Enum descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDef {
    /// Enum name.
    pub name: String,
    /// Variants in declaration order.
    pub variants: Vec<VariantDef>,
}

/// Type descriptor used by bytecode metadata and the type checker.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeDesc {
    /// Unit / void.
    Unit,
    /// 8-bit signed integer.
    Int8,
    /// 16-bit signed integer.
    Int16,
    /// 32-bit signed integer.
    Int32,
    /// 64-bit signed integer.
    Int64,
    /// 8-bit unsigned integer.
    UInt8,
    /// 16-bit unsigned integer.
    UInt16,
    /// 32-bit unsigned integer.
    UInt32,
    /// 64-bit unsigned integer.
    UInt64,
    /// 32-bit float.
    Float32,
    /// 64-bit float.
    Float64,
    /// Boolean.
    Bool,
    /// String reference type.
    String,
    /// Class instance reference type with optional type arguments.
    Class(ClassId, Vec<TypeDesc>),
    /// Enum type.
    Enum(EnumId),
    /// Generic type parameter (by index into enclosing generic params).
    GenericParam(u32),
    /// Tuple type with element types.
    Tuple(Vec<TypeDesc>),
    /// Null type (bottom of reference hierarchy).
    Null,
}

impl TypeDesc {
    /// True if this is an integer type.
    pub fn is_int(&self) -> bool {
        matches!(
            self,
            TypeDesc::Int8
                | TypeDesc::Int16
                | TypeDesc::Int32
                | TypeDesc::Int64
                | TypeDesc::UInt8
                | TypeDesc::UInt16
                | TypeDesc::UInt32
                | TypeDesc::UInt64
        )
    }

    /// True if this is a floating point type.
    pub fn is_float(&self) -> bool {
        matches!(self, TypeDesc::Float32 | TypeDesc::Float64)
    }
}

impl fmt::Display for TypeDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeDesc::Unit => write!(f, "void"),
            TypeDesc::Int8 => write!(f, "int8"),
            TypeDesc::Int16 => write!(f, "int16"),
            TypeDesc::Int32 => write!(f, "int32"),
            TypeDesc::Int64 => write!(f, "int64"),
            TypeDesc::UInt8 => write!(f, "uint8"),
            TypeDesc::UInt16 => write!(f, "uint16"),
            TypeDesc::UInt32 => write!(f, "uint32"),
            TypeDesc::UInt64 => write!(f, "uint64"),
            TypeDesc::Float32 => write!(f, "float32"),
            TypeDesc::Float64 => write!(f, "float64"),
            TypeDesc::Bool => write!(f, "bool"),
            TypeDesc::String => write!(f, "string"),
            TypeDesc::Class(ClassId(id), args) => {
                if args.is_empty() {
                    write!(f, "class#{id}")
                } else {
                    write!(f, "class#{id}<")?;
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 { write!(f, ", ")?; }
                        write!(f, "{arg}")?;
                    }
                    write!(f, ">")
                }
            }
            TypeDesc::Enum(EnumId(id)) => write!(f, "enum#{id}"),
            TypeDesc::GenericParam(idx) => write!(f, "T{idx}"),
            TypeDesc::Tuple(types) => {
                write!(f, "(")?;
                for (i, ty) in types.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{ty}")?;
                }
                write!(f, ")")
            }
            TypeDesc::Null => write!(f, "null"),
        }
    }
}

/// A complete compiled Aura module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Module {
    /// Module name.
    pub name: String,
    /// Class definitions by id.
    pub classes: HashMap<ClassId, ClassDef>,
    /// Enum definitions by id.
    pub enums: HashMap<EnumId, EnumDef>,
    /// Entrypoint method id (typically `Program::Main`).
    pub entrypoint: Option<MethodId>,
    /// String constant pool.
    pub constant_pool: Vec<String>,
}

impl Module {
    /// Resolve a class by id.
    pub fn class(&self, id: ClassId) -> Option<&ClassDef> {
        self.classes.get(&id)
    }

    /// Resolve a method by id (searching all classes).
    pub fn method(&self, id: MethodId) -> Option<&MethodDef> {
        for class in self.classes.values() {
            if let Some(m) = class.methods.get(&id) {
                return Some(m);
            }
            if let Some(m) = class.static_methods.get(&id) {
                return Some(m);
            }
        }
        None
    }
}

/// Aura bytecode instruction.
///
/// Operands are stored inline. `u32` operands reference constants, locals, or
/// module ids. `i32` operands encode small literal integers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// No operation.
    Nop,

    // Stack manipulation
    /// Push a constant 32-bit integer.
    LdInt(i32),
    /// Push a constant 64-bit integer (used for literals outside the int32 range).
    LdInt64(i64),
    /// Push a constant float (index into constant pool).
    LdFloat(u32),
    /// Convert the value on top of the stack to the given numeric type,
    /// applying width truncation and float32 precision rounding.
    Conv(TypeDesc),
    /// Push a constant boolean.
    LdBool(bool),
    /// Push a string constant (index into constant pool).
    LdStr(u32),
    /// Push null.
    LdNull,
    /// Load local at index.
    Ldloc(u16),
    /// Store to local at index.
    Stloc(u16),
    /// Load argument at index.
    Ldarg(u16),
    /// Duplicate top of stack.
    Dup,
    /// Pop top of stack.
    Pop,

    // Arithmetic / logic
    /// Add top two values.
    Add,
    /// Subtract top two values.
    Sub,
    /// Multiply top two values.
    Mul,
    /// Divide top two values.
    Div,
    /// Remainder top two values.
    Rem,
    /// Negate top value.
    Neg,
    /// Bitwise/comparison equals.
    Eq,
    /// Deep structural value equality (used for record classes).
    ValueEq,
    /// Hash the top value (record-aware structural hash).
    Hash,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
    /// Logical not.
    Not,
    /// Logical and.
    And,
    /// Logical or.
    Or,

    // Control flow
    /// Branch unconditionally.
    Br(u32),
    /// Branch if top of stack is false / zero.
    BrFalse(u32),
    /// Branch if top of stack is true / non-zero.
    BrTrue(u32),
    /// Call method by id.
    Call(MethodId),
    /// Call virtual method by name (dispatch via runtime type).
    CallVirt(String),
    /// Call a base class method non-virtually (from `super.Method()`).
    CallSuper(MethodId),
    /// Return from current method.
    Ret,
    /// Break out of the innermost loop (resolved to Br by emitter).
    Break,
    /// Continue to the next iteration of the innermost loop (resolved to Br by emitter).
    Continue,
    /// Throw the value on top of the stack as an exception.
    Throw,
    /// End of a `finally` block: re-propagate a pending exception, if any.
    EndFinally,

    // Object model
    /// Create a new instance of class id with optional type arguments.
    NewObj(ClassId, Vec<TypeDesc>),
    /// Load field by index.
    Ldfld(u16),
    /// Store field by index.
    Stfld(u16),
    /// Load static field by class id and field index.
    Ldsfld(ClassId, u16),
    /// Store static field by class id and field index.
    Stsfld(ClassId, u16),

    // Enum model
    /// Create an enum value: pops field values from stack, pushes enum value with given enum id and variant index.
    NewEnum(EnumId, u16),
    /// Push the variant index of the enum value on top of stack.
    EnumTag,
    /// Load enum field by index from the enum value on top of stack.
    EnumField(u16),

    // Tuple model
    /// Create a tuple from the top N values on the stack.
    NewTuple(u16),
    /// Load tuple field by index from the tuple value on top of stack.
    TupleField(u16),

    // Intrinsics / IO
    /// Print top of stack.
    Print,
    /// Print newline.
    PrintLn,
    /// Concatenate N strings from the stack into a single string.
    StringConcat(u16),
}

/// A shared immutable handle to a module (used by VM and compiler).
pub type SharedModule = Arc<Module>;
