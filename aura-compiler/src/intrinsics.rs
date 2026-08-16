//! The stdlib intrinsic surface: `List<T>`, `Map<K, V>`, `Set<T>`, string
//! methods, `Console` and `File`.
//!
//! Intrinsic types have no Aura source and no bytecode bodies. The type
//! checker injects them as ordinary [`crate::typer::ClassInfo`] entries so
//! method lookup, generics and overload checking reuse the existing
//! machinery, and the emitter lowers calls to them into
//! [`aura_bytecode::Op::NativeCall`] ops carrying the [`NativeId`] listed
//! here. The VM implements each id in `aura-vm/src/native.rs`.
//!
//! This module is the compiler's single source of truth for the intrinsic
//! API: names, signatures and native ids live here and nowhere else.

use crate::ast::Type;
use aura_bytecode::natives::NativeId;

/// An intrinsic method signature (receiver excluded from `params`).
pub struct IntrinsicMethod {
    /// Method name as written in Aura source.
    pub name: &'static str,
    /// Parameter types (receiver excluded).
    pub params: Vec<Type>,
    /// Return type (`Type::Unit` for `void`).
    pub return_ty: Type,
    /// Native implementation id.
    pub native: NativeId,
    /// Method-level generic parameter names (inferred at call sites), e.g.
    /// `T` in `Tasks.all(List<Task<T>>) -> Task<List<T>>`.
    pub generic_params: &'static [&'static str],
}

/// A read-only intrinsic property (getter only).
pub struct IntrinsicProperty {
    /// Property name.
    pub name: &'static str,
    /// Property type.
    pub ty: Type,
    /// Native implementation id of the getter.
    pub native: NativeId,
}

/// An intrinsic class or static class.
pub struct IntrinsicClass {
    /// Class name.
    pub name: &'static str,
    /// Generic parameter names (e.g. `["K", "V"]`).
    pub generic_params: &'static [&'static str],
    /// Native id of the constructor, or `None` for static classes (which
    /// the typer marks abstract so `new` is rejected).
    pub constructor: Option<NativeId>,
    /// Constructor parameter types (empty for the collection classes).
    pub constructor_params: Vec<Type>,
    /// Instance methods.
    pub methods: Vec<IntrinsicMethod>,
    /// Static methods.
    pub static_methods: Vec<IntrinsicMethod>,
    /// Instance properties (read-only).
    pub properties: Vec<IntrinsicProperty>,
}

fn gp(name: &str) -> Type {
    Type::GenericParam(name.to_string())
}

fn list_of(elem: Type) -> Type {
    Type::Class("List".to_string(), vec![elem])
}

fn m(name: &'static str, params: Vec<Type>, return_ty: Type, native: NativeId) -> IntrinsicMethod {
    IntrinsicMethod { name, params, return_ty, native, generic_params: &[] }
}

/// An intrinsic method with method-level generic parameters.
fn mg(
    name: &'static str,
    generic_params: &'static [&'static str],
    params: Vec<Type>,
    return_ty: Type,
    native: NativeId,
) -> IntrinsicMethod {
    IntrinsicMethod { name, params, return_ty, native, generic_params }
}

/// All intrinsic classes.
pub fn classes() -> Vec<IntrinsicClass> {
    use NativeId::*;
    vec![
        IntrinsicClass {
            name: "Task",
            generic_params: &["T"],
            constructor: None,
            constructor_params: vec![],
            methods: vec![
                // `wait` compiles to the dedicated TaskWait op (the emitter
                // special-cases it before native dispatch); the native id
                // is a loud guard.
                m("wait", vec![], gp("T"), TaskWaitStub),
            ],
            static_methods: vec![],
            properties: vec![],
        },
        IntrinsicClass {
            name: "Tasks",
            generic_params: &[],
            constructor: None,
            constructor_params: vec![],
            methods: vec![],
            static_methods: vec![
                m(
                    "pause",
                    vec![],
                    Type::Class("Task".to_string(), vec![Type::Int32]),
                    TaskPause,
                ),
                // Cooperative parallelism: gather every result (in list
                // order; the first failure in list order wins), or take the
                // first task to complete.
                mg(
                    "all",
                    &["T"],
                    vec![list_of(Type::Class("Task".to_string(), vec![gp("T")]))],
                    Type::Class("Task".to_string(), vec![list_of(gp("T"))]),
                    TasksAll,
                ),
                mg(
                    "race",
                    &["T"],
                    vec![list_of(Type::Class("Task".to_string(), vec![gp("T")]))],
                    Type::Class("Task".to_string(), vec![gp("T")]),
                    TasksRace,
                ),
            ],
            properties: vec![],
        },
        IntrinsicClass {
            name: "WeakRef",
            generic_params: &["T"],
            constructor: Some(WeakNew),
            constructor_params: vec![gp("T")],
            methods: vec![
                m("isAlive", vec![], Type::Bool, WeakIsAlive),
                m(
                    "get",
                    vec![],
                    Type::Nullable(Box::new(gp("T"))),
                    WeakGet,
                ),
            ],
            static_methods: vec![],
            properties: vec![],
        },
        IntrinsicClass {
            name: "SoftRef",
            generic_params: &["T"],
            constructor: Some(SoftNew),
            constructor_params: vec![gp("T")],
            methods: vec![
                m("isAlive", vec![], Type::Bool, SoftIsAlive),
                m(
                    "get",
                    vec![],
                    Type::Nullable(Box::new(gp("T"))),
                    SoftGet,
                ),
            ],
            static_methods: vec![],
            properties: vec![],
        },
        IntrinsicClass {
            name: "PhantomRef",
            generic_params: &["T"],
            constructor: Some(PhantomNew),
            constructor_params: vec![gp("T")],
            methods: vec![m("isReclaimed", vec![], Type::Bool, PhantomIsReclaimed)],
            static_methods: vec![],
            properties: vec![],
        },
        IntrinsicClass {
            name: "List",
            generic_params: &["T"],
            constructor: Some(ListNew),
            constructor_params: vec![],
            methods: vec![
                m("Add", vec![gp("T")], Type::Unit, ListAdd),
                m("Get", vec![Type::Int32], gp("T"), ListGet),
                m("Set", vec![Type::Int32, gp("T")], Type::Unit, ListSet),
                m("Insert", vec![Type::Int32, gp("T")], Type::Unit, ListInsert),
                m("RemoveAt", vec![Type::Int32], gp("T"), ListRemoveAt),
                m("IndexOf", vec![gp("T")], Type::Int32, ListIndexOf),
                m("Contains", vec![gp("T")], Type::Bool, ListContains),
                m("Clear", vec![], Type::Unit, ListClear),
            ],
            static_methods: vec![],
            properties: vec![IntrinsicProperty { name: "Count", ty: Type::Int32, native: ListCount }],
        },
        IntrinsicClass {
            name: "Map",
            generic_params: &["K", "V"],
            constructor: Some(MapNew),
            constructor_params: vec![],
            methods: vec![
                m("Put", vec![gp("K"), gp("V")], Type::Unit, MapPut),
                m("Get", vec![gp("K")], gp("V"), MapGet),
                m("ContainsKey", vec![gp("K")], Type::Bool, MapContainsKey),
                m("Remove", vec![gp("K")], Type::Bool, MapRemove),
                m("Keys", vec![], list_of(gp("K")), MapKeys),
                m("Values", vec![], list_of(gp("V")), MapValues),
                m("Clear", vec![], Type::Unit, MapClear),
            ],
            static_methods: vec![],
            properties: vec![IntrinsicProperty { name: "Count", ty: Type::Int32, native: MapCount }],
        },
        IntrinsicClass {
            name: "Set",
            generic_params: &["T"],
            constructor: Some(SetNew),
            constructor_params: vec![],
            methods: vec![
                m("Add", vec![gp("T")], Type::Bool, SetAdd),
                m("Contains", vec![gp("T")], Type::Bool, SetContains),
                m("Remove", vec![gp("T")], Type::Bool, SetRemove),
                m("ToList", vec![], list_of(gp("T")), SetToList),
                m("Clear", vec![], Type::Unit, SetClear),
            ],
            static_methods: vec![],
            properties: vec![IntrinsicProperty { name: "Count", ty: Type::Int32, native: SetCount }],
        },
        IntrinsicClass {
            name: "Console",
            generic_params: &[],
            constructor: None,
            constructor_params: vec![],
            methods: vec![],
            static_methods: vec![m(
                "ReadLine",
                vec![],
                Type::Nullable(Box::new(Type::String)),
                ConsoleReadLine,
            )],
            properties: vec![],
        },
        IntrinsicClass {
            name: "File",
            generic_params: &[],
            constructor: None,
            constructor_params: vec![],
            methods: vec![],
            static_methods: vec![
                m("ReadAllText", vec![Type::String], Type::String, FileReadAllText),
                m("WriteAllText", vec![Type::String, Type::String], Type::Unit, FileWriteAllText),
                m("AppendAllText", vec![Type::String, Type::String], Type::Unit, FileAppendAllText),
                m("Exists", vec![Type::String], Type::Bool, FileExists),
                m("Delete", vec![Type::String], Type::Unit, FileDelete),
                m("ReadAllLines", vec![Type::String], list_of(Type::String), FileReadAllLines),
            ],
            properties: vec![],
        },
    ]
}

/// Methods callable on `string` receivers.
pub fn string_methods() -> Vec<IntrinsicMethod> {
    use NativeId::*;
    vec![
        m("Substring", vec![Type::Int32, Type::Int32], Type::String, StrSubstring),
        m("CharAt", vec![Type::Int32], Type::Char, StrCharAt),
        m("Contains", vec![Type::String], Type::Bool, StrContains),
        m("StartsWith", vec![Type::String], Type::Bool, StrStartsWith),
        m("EndsWith", vec![Type::String], Type::Bool, StrEndsWith),
        m("IndexOf", vec![Type::String], Type::Int32, StrIndexOf),
        m("Split", vec![Type::String], list_of(Type::String), StrSplit),
        m("Trim", vec![], Type::String, StrTrim),
        m("ToUpper", vec![], Type::String, StrToUpper),
        m("ToLower", vec![], Type::String, StrToLower),
        m("Replace", vec![Type::String, Type::String], Type::String, StrReplace),
        m("ToInt", vec![], Type::Int32, StrToInt),
        m("ToFloat", vec![], Type::Float64, StrToFloat),
    ]
}

/// Properties readable on `string` receivers.
pub fn string_properties() -> Vec<IntrinsicProperty> {
    vec![IntrinsicProperty { name: "Length", ty: Type::Int32, native: NativeId::StrLength }]
}

// ---------------------------------------------------------------------------
// Lookup helpers
// ---------------------------------------------------------------------------

/// The intrinsic class named `name`, if any.
pub fn find_class(name: &str) -> Option<IntrinsicClass> {
    classes().into_iter().find(|c| c.name == name)
}

/// True if `name` is an intrinsic class name.
pub fn is_intrinsic_class(name: &str) -> bool {
    matches!(name, "List" | "Map" | "Set" | "Console" | "File")
}

/// The zero-argument constructor native of intrinsic class `name`.
pub fn constructor_native(name: &str) -> Option<NativeId> {
    find_class(name)?.constructor
}

/// An instance method of intrinsic class `class`.
pub fn method(class: &str, name: &str) -> Option<IntrinsicMethod> {
    find_class(class)?.methods.into_iter().find(|m| m.name == name)
}

/// A static method of intrinsic class `class`.
pub fn static_method(class: &str, name: &str) -> Option<IntrinsicMethod> {
    find_class(class)?.static_methods.into_iter().find(|m| m.name == name)
}

/// A property of intrinsic class `class`.
pub fn property(class: &str, name: &str) -> Option<IntrinsicProperty> {
    find_class(class)?.properties.into_iter().find(|p| p.name == name)
}

/// A method callable on `string` receivers.
pub fn string_method(name: &str) -> Option<IntrinsicMethod> {
    string_methods().into_iter().find(|m| m.name == name)
}

/// A property readable on `string` receivers.
pub fn string_property(name: &str) -> Option<IntrinsicProperty> {
    string_properties().into_iter().find(|p| p.name == name)
}
