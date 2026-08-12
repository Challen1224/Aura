//! Type checker for Aura.

use crate::ast::*;
use std::collections::{HashMap, HashSet};

/// Type substitution map: generic parameter name -> concrete type
pub type TypeSubst = HashMap<String, Type>;

/// Human-readable name for a visibility level, used in diagnostics.
fn visibility_name(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
        Visibility::Internal => "internal",
    }
}

/// Substitute generic parameters in a type with concrete types
fn substitute_type(ty: &Type, subst: &TypeSubst) -> Type {
    match ty {
        Type::GenericParam(name) => {
            subst.get(name).cloned().unwrap_or_else(|| ty.clone())
        }
        Type::Class(name, args) => {
            if args.is_empty() && subst.contains_key(name) {
                return subst.get(name).cloned().unwrap();
            }
            let substituted_args: Vec<Type> = args.iter()
                .map(|arg| substitute_type(arg, subst))
                .collect();
            Type::Class(name.clone(), substituted_args)
        }
        Type::Enum(_) => ty.clone(),
        Type::Tuple(types) => {
            let substituted_types: Vec<Type> = types.iter()
                .map(|t| substitute_type(t, subst))
                .collect();
            Type::Tuple(substituted_types)
        }
        _ => ty.clone(),
    }
}

/// Build a substitution map from generic parameter names and concrete type arguments
fn build_subst(generic_params: &[GenericParam], type_args: &[Type]) -> TypeSubst {    let mut subst = TypeSubst::new();
    for (param, arg) in generic_params.iter().zip(type_args.iter()) {
        subst.insert(param.name.clone(), arg.clone());
    }
    subst
}

/// Type checking error.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct TypeError(pub String);

/// A typed view of the program produced by the type checker.
/// For this minimal implementation the AST is unchanged, but the checker
/// validates it and enriches the compiler's symbol tables.
#[derive(Debug, Clone)]
pub struct TypedProgram {
    /// Original AST program.
    pub program: Program,
    /// Class descriptors keyed by name.
    pub classes: HashMap<String, ClassInfo>,
    /// Enum descriptors keyed by name.
    pub enums: HashMap<String, EnumInfo>,
}

/// Class metadata populated by the type checker.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    /// Class name.
    pub name: String,
    /// Generic parameters for this class.
    pub generic_params: Vec<GenericParam>,
    /// Whether this is an interface declaration.
    pub is_interface: bool,
    /// Whether this is an abstract class (cannot be instantiated).
    pub is_abstract: bool,
    /// Whether this is a sealed class (cannot be subclassed).
    pub is_sealed: bool,
    /// Whether this is a record class (value semantics, structural equality).
    pub is_record: bool,
    /// Super class name for single inheritance.
    pub super_class: Option<String>,
    /// Interfaces implemented (classes) or extended (interfaces).
    pub interfaces: Vec<String>,
    /// Instance field names and types in declaration order.
    pub instance_fields: Vec<(String, Type)>,
    /// Static field names and types in declaration order.
    pub static_fields: Vec<(String, Type)>,
    /// Names of protected instance fields.
    pub protected_fields: HashSet<String>,
    /// Names of protected static fields.
    pub protected_static_fields: HashSet<String>,
    /// Names of private instance fields.
    pub private_fields: HashSet<String>,
    /// Names of private static fields.
    pub private_static_fields: HashSet<String>,
    /// Instance methods keyed by name.
    pub methods: HashMap<String, MethodInfo>,
    /// Static methods keyed by name.
    pub static_methods: HashMap<String, MethodInfo>,
    /// Constructors in declaration order (used when type-checking `new`).
    pub constructors: Vec<ConstructorInfo>,
    /// Instance properties keyed by name.
    pub properties: HashMap<String, PropertyInfo>,
    /// Static properties keyed by name.
    pub static_properties: HashMap<String, PropertyInfo>,
}

/// Property metadata.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    /// Property name.
    pub name: String,
    /// Property type.
    pub ty: Type,
    /// Member visibility.
    pub visibility: Visibility,
    /// Effective visibility of the getter accessor.
    pub getter_visibility: Visibility,
    /// Effective visibility of the setter accessor.
    pub setter_visibility: Visibility,
    /// Whether a getter accessor is declared.
    pub has_getter: bool,
    /// Whether a setter accessor is declared.
    pub has_setter: bool,
    /// Whether this is a static property.
    pub is_static: bool,
}

/// Constructor metadata.
#[derive(Debug, Clone)]
pub struct ConstructorInfo {
    /// Parameter types in declaration order.
    pub params: Vec<Type>,
}

/// Method metadata.
#[derive(Debug, Clone)]
pub struct MethodInfo {
    /// Method name.
    pub name: String,
    /// Generic parameters for this method.
    pub generic_params: Vec<GenericParam>,
    /// Return type.
    pub return_ty: Type,
    /// Parameter types.
    pub params: Vec<Type>,
    /// Whether this is an instance method.
    pub is_instance: bool,
    /// Member visibility.
    pub visibility: Visibility,
    /// Whether this method can be overridden by a subclass.
    pub is_virtual: bool,
    /// Whether this method overrides a base class method.
    pub is_override: bool,
    /// Whether this method is abstract (no body).
    pub is_abstract: bool,
    /// Whether this method is final (cannot be overridden or re-declared).
    pub is_final: bool,
}

/// Enum metadata.
#[derive(Debug, Clone)]
pub struct EnumInfo {
    /// Enum name.
    pub name: String,
    /// Generic parameters (empty for plain enums).
    pub generic_params: Vec<GenericParam>,
    /// Variants in declaration order.
    pub variants: Vec<VariantInfo>,
}

/// Variant metadata.
#[derive(Debug, Clone)]
pub struct VariantInfo {
    /// Variant name.
    pub name: String,
    /// Field names and types.
    pub fields: Vec<(String, Type)>,
}

/// Type checker state.
pub struct TypeChecker {
    classes: HashMap<String, ClassInfo>,
    enums: HashMap<String, EnumInfo>,
}

impl TypeChecker {
    /// Create a new type checker.
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
            enums: HashMap::new(),
        }
    }

    /// Type-check a program and return a typed view.
    pub fn check(mut self, program: &Program) -> Result<TypedProgram, TypeError> {
        self.gather_decls(program)?;
        for decl in &program.decls {
            match decl {
                Decl::Class(c) => self.check_class(c)?,
                Decl::Enum(_) => {}
            }
        }
        Ok(TypedProgram {
            program: program.clone(),
            classes: self.classes,
            enums: self.enums,
        })
    }

    fn gather_decls(&mut self, program: &Program) -> Result<(), TypeError> {
        for decl in &program.decls {
            match decl {
                Decl::Class(c) => {
                    if self.classes.contains_key(&c.name) {
                        return Err(TypeError(format!("duplicate class `{}`", c.name)));
                    }
                    if self.enums.contains_key(&c.name) {
                        return Err(TypeError(format!("`{}` is already defined as an enum", c.name)));
                    }
                    let mut info = ClassInfo {
                        name: c.name.clone(),
                        generic_params: c.generic_params.clone(),
                        is_interface: c.is_interface,
                        is_abstract: c.is_abstract,
                        is_sealed: c.is_sealed,
                        is_record: c.is_record,
                        super_class: None,
                        interfaces: Vec::new(),
                        instance_fields: Vec::new(),
                        static_fields: Vec::new(),
                        protected_fields: HashSet::new(),
                        protected_static_fields: HashSet::new(),
                        private_fields: HashSet::new(),
                        private_static_fields: HashSet::new(),
                        methods: HashMap::new(),
                        static_methods: HashMap::new(),
                        constructors: Vec::new(),
                        properties: HashMap::new(),
                        static_properties: HashMap::new(),
                    };
                    if c.is_abstract && c.is_interface {
                        return Err(TypeError(format!(
                            "`{}` cannot be both abstract and an interface",
                            c.name
                        )));
                    }
                    if c.is_abstract && c.is_sealed {
                        return Err(TypeError(format!(
                            "`{}` cannot be both abstract and sealed",
                            c.name
                        )));
                    }
                    for member in &c.members {
                        match member {
                            Member::Field(f) => {
                                if c.is_interface {
                                    return Err(TypeError(format!(
                                        "interface `{}` cannot declare field `{}`",
                                        c.name, f.name
                                    )));
                                }
                                if info.properties.contains_key(&f.name)
                                    || info.static_properties.contains_key(&f.name)
                                {
                                    return Err(TypeError(format!(
                                        "`{}` already has a property named `{}`",
                                        c.name, f.name
                                    )));
                                }
                                if f.is_static {
                                    match f.visibility {
                                        Visibility::Protected => {
                                            info.protected_static_fields.insert(f.name.clone());
                                        }
                                        Visibility::Private => {
                                            info.private_static_fields.insert(f.name.clone());
                                        }
                                        Visibility::Public | Visibility::Internal => {}
                                    }
                                    info.static_fields.push((f.name.clone(), f.ty.clone()));
                                } else {
                                    match f.visibility {
                                        Visibility::Protected => {
                                            info.protected_fields.insert(f.name.clone());
                                        }
                                        Visibility::Private => {
                                            info.private_fields.insert(f.name.clone());
                                        }
                                        Visibility::Public | Visibility::Internal => {}
                                    }
                                    info.instance_fields.push((f.name.clone(), f.ty.clone()));
                                }
                            }
                            Member::Method(m) => {
                                if m.is_constructor {
                                    if c.is_interface {
                                        return Err(TypeError(format!(
                                            "interface `{}` cannot declare a constructor",
                                            c.name
                                        )));
                                    }
                                    if m.name != c.name {
                                        return Err(TypeError(format!(
                                            "constructor name `{}` must match class name `{}`",
                                            m.name, c.name
                                        )));
                                    }
                                    if m.return_ty != Type::Unit {
                                        return Err(TypeError(format!(
                                            "constructor `{}` cannot have a return type",
                                            c.name
                                        )));
                                    }
                                    let duplicate = info.constructors.iter().any(|ctor| {
                                        ctor.params.len() == m.params.len()
                                            && ctor
                                                .params
                                                .iter()
                                                .zip(m.params.iter())
                                                .all(|(a, b)| a == &b.ty)
                                    });
                                    if duplicate {
                                        return Err(TypeError(format!(
                                            "duplicate constructor for class `{}`",
                                            c.name
                                        )));
                                    }
                                    info.constructors.push(ConstructorInfo {
                                        params: m.params.iter().map(|p| p.ty.clone()).collect(),
                                    });
                                    continue;
                                }
                                if c.is_interface && m.is_static {
                                    return Err(TypeError(format!(
                                        "interface `{}` cannot declare static method `{}`",
                                        c.name, m.name
                                    )));
                                }
                                if c.is_interface && m.visibility == Visibility::Protected {
                                    return Err(TypeError(format!(
                                        "interface `{}` method `{}` cannot be protected",
                                        c.name, m.name
                                    )));
                                }
                                if c.is_interface && m.visibility == Visibility::Private {
                                    return Err(TypeError(format!(
                                        "interface `{}` method `{}` cannot be private",
                                        c.name, m.name
                                    )));
                                }
                                if c.is_interface && m.is_final {
                                    return Err(TypeError(format!(
                                        "interface `{}` method `{}` cannot be final",
                                        c.name, m.name
                                    )));
                                }
                                if m.is_abstract && !c.is_interface && !c.is_abstract {
                                    return Err(TypeError(format!(
                                        "abstract method `{}.{}` must be declared in an abstract class or interface",
                                        c.name, m.name
                                    )));
                                }
                                if m.is_abstract && m.is_static {
                                    return Err(TypeError(format!(
                                        "abstract method `{}.{}` cannot be static",
                                        c.name, m.name
                                    )));
                                }
                                if m.is_final && m.is_abstract {
                                    return Err(TypeError(format!(
                                        "method `{}.{}` cannot be both final and abstract",
                                        c.name, m.name
                                    )));
                                }
                                if m.is_final && m.is_virtual {
                                    return Err(TypeError(format!(
                                        "method `{}.{}` cannot be both final and virtual",
                                        c.name, m.name
                                    )));
                                }
                                let is_abstract =
                                    m.is_abstract || (c.is_interface && m.body.is_empty());
                                let is_virtual = !m.is_final
                                    && (m.is_virtual
                                        || is_abstract
                                        || (c.is_interface && !m.body.is_empty()));
                                let mi = MethodInfo {
                                    name: m.name.clone(),
                                    generic_params: m.generic_params.clone(),
                                    return_ty: m.return_ty.clone(),
                                    params: m.params.iter().map(|p| p.ty.clone()).collect(),
                                    is_instance: !m.is_static,
                                    visibility: m.visibility,
                                    is_virtual,
                                    is_override: m.is_override,
                                    is_abstract,
                                    is_final: m.is_final,
                                };
                                if m.is_static {
                                    info.static_methods.insert(m.name.clone(), mi);
                                } else {
                                    info.methods.insert(m.name.clone(), mi);
                                }
                            }
                            Member::Property(p) => {
                                if c.is_interface {
                                    return Err(TypeError(format!(
                                        "interface `{}` cannot declare property `{}`",
                                        c.name, p.name
                                    )));
                                }
                                let name_conflict = info.instance_fields.iter().any(|(n, _)| *n == p.name)
                                    || info.static_fields.iter().any(|(n, _)| *n == p.name)
                                    || info.methods.contains_key(&p.name)
                                    || info.static_methods.contains_key(&p.name)
                                    || info.properties.contains_key(&p.name)
                                    || info.static_properties.contains_key(&p.name);
                                if name_conflict {
                                    return Err(TypeError(format!(
                                        "`{}` already has a member named `{}`",
                                        c.name, p.name
                                    )));
                                }
                                let getter_visibility =
                                    self.effective_accessor_visibility(p.visibility, &p.getter)?;
                                let setter_visibility =
                                    self.effective_accessor_visibility(p.visibility, &p.setter)?;
                                let pi = PropertyInfo {
                                    name: p.name.clone(),
                                    ty: p.ty.clone(),
                                    visibility: p.visibility,
                                    getter_visibility,
                                    setter_visibility,
                                    has_getter: p.getter.is_some(),
                                    has_setter: p.setter.is_some(),
                                    is_static: p.is_static,
                                };
                                if p.is_static {
                                    info.static_properties.insert(p.name.clone(), pi);
                                } else {
                                    info.properties.insert(p.name.clone(), pi);
                                }
                                // Auto accessors are backed by a generated field.
                                if matches!(p.getter.as_ref().map(|a| &a.kind), Some(AccessorKind::Auto))
                                    || matches!(p.setter.as_ref().map(|a| &a.kind), Some(AccessorKind::Auto))
                                {
                                    let backing = format!("__prop_{}", p.name);
                                    if p.is_static {
                                        info.static_fields.push((backing, p.ty.clone()));
                                    } else {
                                        info.instance_fields.push((backing, p.ty.clone()));
                                    }
                                }
                            }
                        }
                    }
                    if c.is_record {
                        let existing_names: HashSet<String> = info
                            .instance_fields
                            .iter()
                            .map(|(n, _)| n.clone())
                            .chain(info.static_fields.iter().map(|(n, _)| n.clone()))
                            .chain(info.properties.keys().cloned())
                            .chain(info.static_properties.keys().cloned())
                            .chain(info.methods.keys().cloned())
                            .chain(info.static_methods.keys().cloned())
                            .collect();
                        for param in &c.record_params {
                            if existing_names.contains(&param.name) {
                                return Err(TypeError(format!(
                                    "record `{}` already has a member named `{}`",
                                    c.name, param.name
                                )));
                            }
                            info.instance_fields.push((param.name.clone(), param.ty.clone()));
                        }
                        // The synthesized primary constructor is the default
                        // overload used by `new` when no explicit constructor
                        // is selected.
                        let primary_params: Vec<Type> = c
                            .record_params
                            .iter()
                            .map(|p| p.ty.clone())
                            .collect();
                        info.constructors.push(ConstructorInfo { params: primary_params });
                    }
                    // Classes that declare no constructors get an implicit
                    // zero-parameter default constructor.
                    if !c.is_interface && info.constructors.is_empty() {
                        info.constructors.push(ConstructorInfo { params: Vec::new() });
                    }
                    self.classes.insert(c.name.clone(), info);
                }
                Decl::Enum(e) => {
                    if self.enums.contains_key(&e.name) {
                        return Err(TypeError(format!("duplicate enum `{}`", e.name)));
                    }
                    if self.classes.contains_key(&e.name) {
                        return Err(TypeError(format!("`{}` is already defined as a class", e.name)));
                    }
                    let mut variants = Vec::new();
                    for v in &e.variants {
                        let fields = v.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
                        variants.push(VariantInfo {
                            name: v.name.clone(),
                            fields,
                        });
                        for f in &v.fields {
                            self.validate_type_with_generics(&f.ty, &e.generic_params)?;
                        }
                    }
                    self.enums.insert(e.name.clone(), EnumInfo {
                        name: e.name.clone(),
                        generic_params: e.generic_params.clone(),
                        variants,
                    });
                }
            }
        }
        self.split_bases(program)?;
        self.validate_hierarchy()?;
        Ok(())
    }

    /// Split the raw base-name list of each class into a super class (for
    /// classes) and a list of implemented/extended interfaces.
    fn split_bases(&mut self, program: &Program) -> Result<(), TypeError> {
        for decl in &program.decls {
            if let Decl::Class(c) = decl {
                let mut super_class = None;
                let mut interfaces = Vec::new();
                for base in &c.bases {
                    let base_info = self
                        .classes
                        .get(base)
                        .ok_or_else(|| {
                            TypeError(format!("unknown base type `{}` for `{}`", base, c.name))
                        })?;
                    let is_interface = base_info.is_interface;
                    if is_interface {
                        interfaces.push(base.clone());
                    } else if c.is_record {
                        return Err(TypeError(format!(
                            "record `{}` cannot inherit from class `{}`",
                            c.name, base
                        )));
                    } else if c.is_interface {
                        return Err(TypeError(format!(
                            "interface `{}` cannot extend class `{}`",
                            c.name, base
                        )));
                    } else if base_info.is_sealed {
                        return Err(TypeError(format!(
                            "class `{}` cannot extend sealed class `{}`",
                            c.name, base
                        )));
                    } else if super_class.is_some() {
                        return Err(TypeError(format!(
                            "class `{}` can have only one super class, but found `{}` and `{}`",
                            c.name,
                            super_class.as_ref().unwrap(),
                            base
                        )));
                    } else {
                        super_class = Some(base.clone());
                    }
                }
                let info = self.classes.get_mut(&c.name).unwrap();
                info.super_class = super_class;
                info.interfaces = interfaces;
            }
        }
        Ok(())
    }

    /// Validate class hierarchy: super classes and interfaces exist and no cycles.
    fn validate_hierarchy(&self) -> Result<(), TypeError> {
        for info in self.classes.values() {
            if let Some(super_name) = &info.super_class {
                // Check for cycles by walking the chain.
                let mut cur = Some(super_name.as_str());
                while let Some(c) = cur {
                    if c == info.name {
                        return Err(TypeError(format!(
                            "circular inheritance involving `{}`",
                            info.name
                        )));
                    }
                    cur = self.classes.get(c).and_then(|ci| ci.super_class.as_deref());
                }
            }
        }
        for info in self.classes.values() {
            let mut visited = HashSet::new();
            visited.insert(info.name.clone());
            self.validate_interface_chain(&info.name, &mut visited)?;
        }
        Ok(())
    }

    fn validate_interface_chain(&self, name: &str, visited: &mut HashSet<String>) -> Result<(), TypeError> {
        let info = self.classes.get(name).unwrap();
        for iface in &info.interfaces {
            if !visited.insert(iface.clone()) {
                return Err(TypeError(format!(
                    "circular inheritance involving `{}`",
                    name
                )));
            }
            self.validate_interface_chain(iface, visited)?;
            visited.remove(iface);
        }
        Ok(())
    }

    /// True if `sub` is the same as, a descendant of, or an implementer of
    /// `base`. Interfaces are treated uniformly: a class "inherits" the
    /// interfaces it implements and the interfaces those interfaces extend.
    fn is_subclass_of(&self, sub: &str, base: &str) -> bool {
        if sub == base {
            return true;
        }
        let mut visited = HashSet::new();
        self.is_subclass_of_inner(sub, base, &mut visited)
    }

    fn is_subclass_of_inner(&self, sub: &str, base: &str, visited: &mut HashSet<String>) -> bool {
        if sub == base {
            return true;
        }
        if !visited.insert(sub.to_string()) {
            return false;
        }
        if let Some(info) = self.classes.get(sub) {
            if let Some(sup) = &info.super_class {
                if self.is_subclass_of_inner(sup, base, visited) {
                    return true;
                }
            }
            for iface in &info.interfaces {
                if self.is_subclass_of_inner(iface, base, visited) {
                    return true;
                }
            }
        }
        false
    }

    /// Look up an instance field starting at `class_name`, walking the super chain.
    /// Returns (declaring class, type, visibility).
    fn find_instance_field(&self, class_name: &str, field: &str) -> Option<(String, Type, Visibility)> {
        let mut cur = Some(class_name);
        while let Some(c) = cur {
            if let Some(info) = self.classes.get(c) {
                if let Some((_, ty)) = info.instance_fields.iter().find(|(n, _)| n == field) {
                    let visibility = if info.private_fields.contains(field) {
                        Visibility::Private
                    } else if info.protected_fields.contains(field) {
                        Visibility::Protected
                    } else {
                        Visibility::Public
                    };
                    return Some((c.to_string(), ty.clone(), visibility));
                }
                cur = info.super_class.as_deref();
            } else {
                break;
            }
        }
        None
    }

    /// Look up an instance property starting at `class_name`, walking the super chain.
    /// Returns (declaring class, property info).
    fn find_instance_property(&self, class_name: &str, name: &str) -> Option<(String, PropertyInfo)> {
        let mut cur = Some(class_name.to_string());
        while let Some(c) = cur {
            if let Some(info) = self.classes.get(&c) {
                if let Some(pi) = info.properties.get(name) {
                    return Some((c.clone(), pi.clone()));
                }
                cur = info.super_class.clone();
            } else {
                break;
            }
        }
        None
    }

    /// Look up a static property starting at `class_name`, walking the super chain.
    /// Returns (declaring class, property info).
    fn find_static_property(&self, class_name: &str, name: &str) -> Option<(String, PropertyInfo)> {
        let mut cur = Some(class_name.to_string());
        while let Some(c) = cur {
            if let Some(info) = self.classes.get(&c) {
                if let Some(pi) = info.static_properties.get(name) {
                    return Some((c.clone(), pi.clone()));
                }
                cur = info.super_class.clone();
            } else {
                break;
            }
        }
        None
    }

    /// Look up a static field starting at `class_name`, walking the super chain.
    /// Returns (declaring class, type, visibility).
    fn find_static_field(&self, class_name: &str, field: &str) -> Option<(String, Type, Visibility)> {
        let mut cur = Some(class_name);
        while let Some(c) = cur {
            if let Some(info) = self.classes.get(c) {
                if let Some((_, ty)) = info.static_fields.iter().find(|(n, _)| n == field) {
                    let visibility = if info.private_static_fields.contains(field) {
                        Visibility::Private
                    } else if info.protected_static_fields.contains(field) {
                        Visibility::Protected
                    } else {
                        Visibility::Public
                    };
                    return Some((c.to_string(), ty.clone(), visibility));
                }
                cur = info.super_class.as_deref();
            } else {
                break;
            }
        }
        None
    }

    /// Look up an instance method starting at `class_name`, walking the super chain.
    fn find_method(&self, class_name: &str, method: &str) -> Option<(String, MethodInfo)> {
        let mut visited = HashSet::new();
        self.find_method_inner(class_name, method, &mut visited)
    }

    fn find_method_inner(
        &self,
        class_name: &str,
        method: &str,
        visited: &mut HashSet<String>,
    ) -> Option<(String, MethodInfo)> {
        if !visited.insert(class_name.to_string()) {
            return None;
        }
        let info = self.classes.get(class_name)?;
        if let Some(mi) = info.methods.get(method) {
            return Some((class_name.to_string(), mi.clone()));
        }
        if let Some(sup) = &info.super_class {
            if let Some(r) = self.find_method_inner(sup, method, visited) {
                return Some(r);
            }
        }
        for iface in &info.interfaces {
            if let Some(r) = self.find_method_inner(iface, method, visited) {
                return Some(r);
            }
        }
        None
    }

    /// Require that a value of `ty` is disposable: its class (or a base class /
    /// implemented interface) declares an instance `Dispose()` method returning void.
    fn require_disposable(&self, ty: &Type) -> Result<(), TypeError> {
        let class_name = match ty {
            Type::Class(name, _) => name.clone(),
            _ => {
                return Err(TypeError(format!(
                    "`using` requires a class resource, got {}",
                    ty.name()
                )))
            }
        };
        let (_, method) = self.find_method(&class_name, "Dispose").ok_or_else(|| {
            TypeError(format!(
                "`using` resource type `{}` has no `Dispose()` method",
                class_name
            ))
        })?;
        if !method.params.is_empty() || method.return_ty != Type::Unit {
            return Err(TypeError(format!(
                "`Dispose()` on `{}` must take no parameters and return void",
                class_name
            )));
        }
        Ok(())
    }

    /// Look up a static method starting at `class_name`, walking the super chain.
    fn find_static_method(&self, class_name: &str, method: &str) -> Option<(String, MethodInfo)> {
        let mut cur = Some(class_name);
        while let Some(c) = cur {
            if let Some(info) = self.classes.get(c) {
                if let Some(mi) = info.static_methods.get(method) {
                    return Some((c.to_string(), mi.clone()));
                }
                cur = info.super_class.as_deref();
            } else {
                break;
            }
        }
        None
    }

    /// Whether members declared in `declared_in` with `visibility` are
    /// accessible from code in class `current`.
    fn can_access(&self, current: &str, declared_in: &str, visibility: Visibility) -> bool {
        match visibility {
            Visibility::Public => true,
            Visibility::Protected => {
                current == declared_in || self.is_subclass_of(current, declared_in)
            }
            Visibility::Private => current == declared_in,
            Visibility::Internal => true,
        }
    }

    /// Effective visibility of an accessor. An explicit accessor modifier must
    /// be more restrictive than the property's own visibility (as in C#); when
    /// absent the accessor inherits the property visibility.
    fn effective_accessor_visibility(
        &self,
        property_visibility: Visibility,
        accessor: &Option<Accessor>,
    ) -> Result<Visibility, TypeError> {
        let Some(vis) = accessor.as_ref().and_then(|a| a.visibility) else {
            return Ok(property_visibility);
        };
        let rank = |v: Visibility| match v {
            Visibility::Private => 0,
            Visibility::Protected => 1,
            Visibility::Internal => 2,
            Visibility::Public => 3,
        };
        if rank(vis) >= rank(property_visibility) {
            return Err(TypeError(format!(
                "accessor visibility {} is not more restrictive than the property visibility {}",
                visibility_name(vis),
                visibility_name(property_visibility)
            )));
        }
        Ok(vis)
    }

    /// Validate a constructor's chaining call and, for `: this(...)`, return
    /// the index of the target constructor within `ctor_members`.
    fn check_constructor_chain(
        &self,
        class: &ClassInfo,
        ctor: &MethodDecl,
        ctor_members: &[&MethodDecl],
        idx: usize,
    ) -> Result<Option<usize>, TypeError> {
        let mut locals: HashMap<String, Type> = HashMap::new();
        for p in &ctor.params {
            locals.insert(p.name.clone(), p.ty.clone());
        }
        let Some(chain) = &ctor.constructor_chain else {
            // Derived constructors must chain explicitly when the base class
            // declares no zero-parameter constructor.
            if let Some(super_name) = &class.super_class {
                if let Some(super_info) = self.classes.get(super_name) {
                    if !super_info.constructors.is_empty()
                        && !super_info.constructors.iter().any(|c| c.params.is_empty())
                    {
                        return Err(TypeError(format!(
                            "constructor `{}(...)` must explicitly call `: super(...)` because super class `{}` has no zero-parameter constructor",
                            class.name, super_name
                        )));
                    }
                }
            }
            return Ok(None);
        };
        let arg_types: Vec<Type> = chain
            .args
            .iter()
            .map(|a| self.infer_expr(a, class, &locals, true, &Type::Unit, &[]))
            .collect::<Result<_, _>>()?;
        let args_desc = |ts: &[Type]| -> String {
            ts.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ")
        };
        match chain.target {
            ConstructorTarget::Base => {
                let Some(super_name) = &class.super_class else {
                    return Err(TypeError(format!(
                        "class `{}` has no super class to chain to in `: super(...)`",
                        class.name
                    )));
                };
                let super_info = self.classes.get(super_name).unwrap();
                if super_info.is_interface {
                    return Err(TypeError(format!(
                        "cannot chain `: super(...)` to interface `{}`",
                        super_name
                    )));
                }
                if super_info.constructors.is_empty() {
                    if !chain.args.is_empty() {
                        return Err(TypeError(format!(
                            "super class `{}` declares no constructor; `: super(...)` must be called with no arguments",
                            super_name
                        )));
                    }
                    return Ok(None);
                }
                let matched = super_info.constructors.iter().any(|c| {
                    c.params.len() == arg_types.len()
                        && c.params
                            .iter()
                            .zip(arg_types.iter())
                            .all(|(e, a)| self.is_assignable(e, a))
                });
                if !matched {
                    return Err(TypeError(format!(
                        "no constructor of super class `{}` matches arguments ({})",
                        super_name,
                        args_desc(&arg_types)
                    )));
                }
                Ok(None)
            }
            ConstructorTarget::This => {
                for (j, other) in ctor_members.iter().enumerate() {
                    if j == idx {
                        continue;
                    }
                    let matches = other.params.len() == arg_types.len()
                        && other
                            .params
                            .iter()
                            .zip(arg_types.iter())
                            .all(|(p, a)| self.is_assignable(&p.ty, a));
                    if matches {
                        return Ok(Some(j));
                    }
                }
                Err(TypeError(format!(
                    "no `: this(...)` constructor of `{}` matches arguments ({})",
                    class.name,
                    args_desc(&arg_types)
                )))
            }
        }
    }

    fn check_class(&mut self, class: &ClassDecl) -> Result<(), TypeError> {
        let info = self.classes.get(&class.name).unwrap().clone();
        let class_generic_params = &class.generic_params;

        // Validate virtual / override / abstract / final modifiers.
        for member in &class.members {
            if let Member::Method(m) = member {
                if m.is_virtual && m.is_override {
                    return Err(TypeError(format!(
                        "method `{}.{}` cannot be both virtual and override",
                        class.name, m.name
                    )));
                }
                if m.is_static && (m.is_virtual || m.is_override) {
                    return Err(TypeError(format!(
                        "static method `{}.{}` cannot be virtual or override",
                        class.name, m.name
                    )));
                }
                if m.is_override {
                    let Some(super_name) = &info.super_class else {
                        return Err(TypeError(format!(
                            "method `{}.{}` is marked override but `{}` has no super class",
                            class.name, m.name, class.name
                        )));
                    };
                    let (base_class, base_method) = self.find_method(super_name, &m.name).ok_or_else(|| {
                        TypeError(format!(
                            "method `{}.{}` is marked override but no such method exists in a base class",
                            class.name, m.name
                        ))
                    })?;
                    if base_method.is_final {
                        return Err(TypeError(format!(
                            "method `{}.{}` cannot override final method `{}.{}`",
                            class.name, m.name, base_class, m.name
                        )));
                    }
                    if !base_method.is_virtual {
                        return Err(TypeError(format!(
                            "method `{}.{}` overrides `{}.{}` which is not virtual",
                            class.name, m.name, base_class, m.name
                        )));
                    }
                    if base_method.return_ty != m.return_ty || base_method.params != m.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>() {
                        return Err(TypeError(format!(
                            "method `{}.{}` override signature must match `{}.{}`",
                            class.name, m.name, base_class, m.name
                        )));
                    }
                }
            }
        }

        // A final method cannot be re-declared anywhere in the hierarchy,
        // even without the `override` keyword.
        for member in &class.members {
            if let Member::Method(m) = member {
                if m.is_override {
                    continue;
                }
                let mut cur = info.super_class.clone();
                while let Some(c) = cur {
                    let ci = self.classes.get(&c).unwrap();
                    if let Some(mi) = ci.methods.get(&m.name) {
                        if mi.is_final {
                            return Err(TypeError(format!(
                                "method `{}.{}` cannot re-declare final method `{}.{}`",
                                class.name, m.name, c, m.name
                            )));
                        }
                        break;
                    }
                    cur = ci.super_class.clone();
                }
            }
        }

        // A concrete class must provide implementations for every abstract
        // method inherited from its super classes and implemented interfaces.
        if !info.is_abstract && !info.is_interface {
            // Inherited abstract class methods.
            let mut cur = info.super_class.clone();
            while let Some(c) = cur {
                let ci = self.classes.get(&c).unwrap();
                for (m_name, mi) in &ci.methods {
                    if mi.is_abstract {
                        let (_, found) = self.find_method(&info.name, m_name).ok_or_else(|| {
                            TypeError(format!(
                                "class `{}` must implement abstract method `{}`",
                                info.name, m_name
                            ))
                        })?;
                        if found.is_abstract {
                            return Err(TypeError(format!(
                                "class `{}` must override abstract method `{}` from `{}`",
                                info.name, m_name, c
                            )));
                        }
                    }
                }
                cur = ci.super_class.clone();
            }
            // Abstract methods from implemented interfaces.
            let mut visited = HashSet::new();
            for iface in self.all_interfaces(&info.name, &mut visited) {
                let ii = self.classes.get(&iface).unwrap();
                for (m_name, mi) in &ii.methods {
                    if mi.is_abstract {
                        let Some((declared_in, found)) = self.find_method(&info.name, m_name) else {
                            return Err(TypeError(format!(
                                "class `{}` must implement method `{}` from interface `{}`",
                                info.name, m_name, iface
                            )));
                        };
                        if found.is_abstract {
                            return Err(TypeError(format!(
                                "class `{}` must override abstract method `{}` from `{}`",
                                info.name, m_name, declared_in
                            )));
                        }
                        if found.return_ty != mi.return_ty || found.params != mi.params {
                            return Err(TypeError(format!(
                                "method `{}.{}` has a different signature than interface method `{}.{}`",
                                info.name, m_name, iface, m_name
                            )));
                        }
                    }
                }
            }
        }

        let ctor_members: Vec<&MethodDecl> = class
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Method(m) if m.is_constructor => Some(m),
                _ => None,
            })
            .collect();
        let mut this_chain_targets: HashMap<usize, usize> = HashMap::new();
        for (i, ctor) in ctor_members.iter().enumerate() {
            if let Some(target_idx) =
                self.check_constructor_chain(&info, ctor, &ctor_members, i)?
            {
                this_chain_targets.insert(i, target_idx);
            }
        }
        for start in 0..ctor_members.len() {
            let mut path: HashSet<usize> = HashSet::new();
            let mut cur = start;
            loop {
                if !path.insert(cur) {
                    return Err(TypeError(format!(
                        "constructors of `{}` form a `: this(...)` chain cycle",
                        class.name
                    )));
                }
                match this_chain_targets.get(&cur) {
                    Some(&next) => cur = next,
                    None => break,
                }
            }
        }

        for member in &class.members {
            if let Member::Method(m) = member {
                let mut locals: HashMap<String, Type> = HashMap::new();
                let mut all_generic_params = class_generic_params.clone();
                all_generic_params.extend(m.generic_params.clone());
                
                for param in &m.params {
                    self.validate_type_with_generics(&param.ty, &all_generic_params)?;
                    locals.insert(param.name.clone(), param.ty.clone());
                }
                self.validate_type_with_generics(&m.return_ty, &all_generic_params)?;
                for stmt in &m.body {
                    self.check_stmt(stmt, &info, &mut locals, &m.return_ty, !m.is_static, &all_generic_params)?;
                }
            } else if let Member::Field(f) = member {
                self.validate_type_with_generics(&f.ty, class_generic_params)?;
            } else if let Member::Property(p) = member {
                self.validate_type_with_generics(&p.ty, class_generic_params)?;
                if let Some(Accessor { kind: AccessorKind::Body(body), .. }) = &p.getter {
                    let mut locals: HashMap<String, Type> = HashMap::new();
                    for stmt in body {
                        self.check_stmt(stmt, &info, &mut locals, &p.ty, !p.is_static, class_generic_params)?;
                    }
                }
                if let Some(Accessor { kind: AccessorKind::Body(body), .. }) = &p.setter {
                    let mut locals: HashMap<String, Type> = HashMap::new();
                    locals.insert("value".to_string(), p.ty.clone());
                    for stmt in body {
                        self.check_stmt(stmt, &info, &mut locals, &Type::Unit, !p.is_static, class_generic_params)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// All interfaces implemented/extended by `class_name`, including those
    /// inherited through super classes and super-interfaces (transitive closure).
    fn all_interfaces(&self, class_name: &str, visited: &mut HashSet<String>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(info) = self.classes.get(class_name) {
            for iface in &info.interfaces {
                if visited.insert(iface.clone()) {
                    out.push(iface.clone());
                    out.extend(self.all_interfaces(iface, visited));
                }
            }
            if let Some(sup) = &info.super_class {
                out.extend(self.all_interfaces(sup, visited));
            }
        }
        out
    }

    fn check_stmt(
        &self,
        stmt: &Stmt,
        class: &ClassInfo,
        locals: &mut HashMap<String, Type>,
        return_ty: &Type,
        in_instance: bool,
        generic_params: &[GenericParam],
    ) -> Result<(), TypeError> {
        match stmt {
            Stmt::VarDecl(ty, name, init) => {
                self.validate_type_with_generics(ty, generic_params)?;
                if let Some(init) = init {
                    let init_ty = self.infer_expr(init, class, locals, in_instance, return_ty, generic_params)?;
                    if !self.is_assignable(ty, &init_ty) {
                        return Err(TypeError(format!(
                            "cannot assign {} to variable `{}` of type {}",
                            init_ty.name(),
                            name,
                            ty.name()
                        )));
                    }
                }
                locals.insert(name.clone(), ty.clone());
            }
            Stmt::TupleDecl(names, expr) => {
                let expr_ty = self.infer_expr(expr, class, locals, in_instance, return_ty, generic_params)?;
                if let Type::Tuple(tuple_types) = &expr_ty {
                    if names.len() != tuple_types.len() {
                        return Err(TypeError(format!(
                            "tuple destructuring expects {} variables but got {}",
                            tuple_types.len(),
                            names.len()
                        )));
                    }
                    for (name, ty) in names.iter().zip(tuple_types.iter()) {
                        locals.insert(name.clone(), ty.clone());
                    }
                } else {
                    return Err(TypeError(format!(
                        "cannot destructure non-tuple type {}",
                        expr_ty.name()
                    )));
                }
            }
            Stmt::Expr(e) => {
                let _ = self.infer_expr(e, class, locals, in_instance, return_ty, generic_params)?;
            }
            Stmt::Assign(target, value) => {
                let target_ty = self.infer_assign_target(target, class, locals, in_instance, return_ty, generic_params)?;
                let value_ty = self.infer_expr(value, class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_assignable(&target_ty, &value_ty) {
                    return Err(TypeError(format!(
                        "cannot assign {} to {}",
                        value_ty.name(),
                        target_ty.name()
                    )));
                }
            }
            Stmt::Return(Some(e)) => {
                let ty = self.infer_expr(e, class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_assignable(return_ty, &ty) {
                    return Err(TypeError(format!(
                        "return type mismatch: expected {}, got {}",
                        return_ty.name(),
                        ty.name()
                    )));
                }
            }
            Stmt::Return(None) => {
                if *return_ty != Type::Unit {
                    return Err(TypeError(format!(
                        "expected return value of type {}",
                        return_ty.name()
                    )));
                }
            }
            Stmt::If(cond, then_branch, else_branch) => {
                let cond_ty = self.infer_expr(cond, class, locals, in_instance, return_ty, generic_params)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "if condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
                for s in then_branch {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
                if let Some(else_branch) = else_branch {
                    for s in else_branch {
                        self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                }
            }
            Stmt::IfLet(pattern, expr, then_branch, else_branch) => {
                let subject_ty = self.infer_expr(expr, class, locals, in_instance, return_ty, generic_params)?;

                // Validate the pattern against the subject type and collect bindings.
                let bindings = self.check_pattern(pattern, &subject_ty, class, locals, in_instance, return_ty, generic_params)?;

                // Bind the pattern names for the then branch only.
                let mut saved: Vec<(String, Option<Type>)> = Vec::new();
                for (name, _) in &bindings {
                    saved.push((name.clone(), locals.get(name).cloned()));
                }
                for (name, ty) in &bindings {
                    locals.insert(name.clone(), ty.clone());
                }
                for s in then_branch {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
                for (name, prev) in saved {
                    match prev {
                        Some(ty) => { locals.insert(name, ty); }
                        None => { locals.remove(&name); }
                    }
                }

                if let Some(else_branch) = else_branch {
                    for s in else_branch {
                        self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                }
            }
            Stmt::While { label: _, condition, body } => {
                let cond_ty = self.infer_expr(condition, class, locals, in_instance, return_ty, generic_params)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "while condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
            }
            Stmt::For { label: _, init, condition, update, body } => {
                self.check_stmt(init, class, locals, return_ty, in_instance, generic_params)?;
                let cond_ty = self.infer_expr(condition, class, locals, in_instance, return_ty, generic_params)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "for condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
                self.check_stmt(update, class, locals, return_ty, in_instance, generic_params)?;
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
            }
            Stmt::ForIn { label: _, var_type, var_name, iterable, body } => {
                self.validate_type_with_generics(var_type, generic_params)?;
                let range_ty = self.infer_expr(iterable, class, locals, in_instance, return_ty, generic_params)?;
                // For now, only support Range type
                if range_ty != Type::Class("Range".to_string(), vec![]) {
                    return Err(TypeError(format!(
                        "for-in requires a range expression, got {}",
                        range_ty.name()
                    )));
                }
                // The loop variable must be an integer type.
                if !var_type.is_int() {
                    return Err(TypeError(format!(
                        "for-in loop variable must be an integer, got {}",
                        var_type.name()
                    )));
                }
                locals.insert(var_name.clone(), var_type.clone());
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
            }
            Stmt::DoWhile { label: _, body, condition } => {
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
                let cond_ty = self.infer_expr(condition, class, locals, in_instance, return_ty, generic_params)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "do-while condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
            }
            Stmt::Break(_) | Stmt::Continue(_) => {
                // These are valid inside loops, checked by the emitter
            }
            Stmt::Throw(e) => {
                let _ = self.infer_expr(e, class, locals, in_instance, return_ty, generic_params)?;
            }
            Stmt::Try {
                try_body,
                catches,
                finally_body,
            } => {
                for s in try_body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
                for catch in catches {
                    self.validate_type_with_generics(&catch.ty, generic_params)?;
                    locals.insert(catch.name.clone(), catch.ty.clone());
                    for s in &catch.body {
                        self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                }
                if let Some(finally_body) = finally_body {
                    for s in finally_body {
                        self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                }
            }
            Stmt::Using {
                resource_ty,
                name,
                expr,
                body,
            } => {
                let expr_ty = self.infer_expr(expr, class, locals, in_instance, return_ty, generic_params)?;
                match (resource_ty, name) {
                    (Some(decl_ty), Some(n)) => {
                        self.validate_type_with_generics(decl_ty, generic_params)?;
                        if !self.is_assignable(decl_ty, &expr_ty) {
                            return Err(TypeError(format!(
                                "cannot use value of type {} as `using` resource of type {}",
                                expr_ty.name(),
                                decl_ty.name()
                            )));
                        }
                        locals.insert(n.clone(), decl_ty.clone());
                    }
                    (None, Some(n)) => {
                        locals.insert(n.clone(), expr_ty.clone());
                    }
                    (None, None) => {}
                    (Some(_), None) => unreachable!("parser always pairs resource type with name"),
                }
                self.require_disposable(&expr_ty)?;
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
            }
            Stmt::Block(stmts) => {
                for s in stmts {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
            }
        }
        Ok(())
    }

    /// Recursively validate a pattern against an expected type and collect the
    /// names it binds (each with the type of the value it will hold).
    fn check_pattern(
        &self,
        pattern: &Pattern,
        expected_ty: &Type,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Vec<(String, Type)>, TypeError> {
        let mut bindings = Vec::new();
        match pattern {
            Pattern::Binding(name) => bindings.push((name.clone(), expected_ty.clone())),
            Pattern::EnumVariant(enum_name, variant_name, sub_patterns) => {
                let enum_info = self.enums.get(enum_name).ok_or_else(|| {
                    TypeError(format!("unknown enum `{}`", enum_name))
                })?;
                let variant = enum_info.variants.iter().find(|v| v.name == *variant_name)
                    .ok_or_else(|| TypeError(format!(
                        "unknown variant `{}.{}`", enum_name, variant_name
                    )))?;
                if sub_patterns.len() != variant.fields.len() {
                    return Err(TypeError(format!(
                        "variant `{}.{}` has {} fields but pattern has {} sub-patterns",
                        enum_name, variant_name, variant.fields.len(), sub_patterns.len()
                    )));
                }
                let enum_args = match expected_ty {
                    Type::Enum(sn) if sn == enum_name => Vec::new(),
                    Type::Class(name, args) if name == enum_name => args.clone(),
                    _ => {
                        return Err(TypeError(format!(
                            "cannot match enum `{}` against value of type `{}`",
                            enum_name, expected_ty.name()
                        )));
                    }
                };
                let subst = build_subst(&enum_info.generic_params, &enum_args);
                for (sub, (_, field_ty)) in sub_patterns.iter().zip(variant.fields.iter()) {
                    let sub_ty = substitute_type(field_ty, &subst);
                    bindings.extend(self.check_pattern(
                        sub, &sub_ty, class, locals, in_instance, return_ty, generic_params,
                    )?);
                }
            }
            Pattern::RecordClass(class_name, sub_patterns) => {
                match self.classes.get(class_name) {
                    Some(info) => {
                        if !info.is_record {
                            return Err(TypeError(format!(
                                "`{}` is not a record class",
                                class_name
                            )));
                        }
                        let Type::Class(expected_name, _) = expected_ty else {
                            return Err(TypeError(format!(
                                "cannot match record `{}` against value of type `{}`",
                                class_name,
                                expected_ty.name()
                            )));
                        };
                        if expected_name != class_name {
                            return Err(TypeError(format!(
                                "record pattern `{}` cannot match value of type `{}`",
                                class_name,
                                expected_ty.name()
                            )));
                        }
                        if sub_patterns.len() != info.instance_fields.len() {
                            return Err(TypeError(format!(
                                "record `{}` has {} fields but pattern has {} sub-patterns",
                                class_name,
                                info.instance_fields.len(),
                                sub_patterns.len()
                            )));
                        }
                        for (sub, (_, field_ty)) in sub_patterns.iter().zip(info.instance_fields.iter()) {
                            bindings.extend(self.check_pattern(
                                sub, field_ty, class, locals, in_instance, return_ty, generic_params,
                            )?);
                        }
                    }
                    None => {
                        // Bare sum-type variant pattern: `Ok(v)` with no type
                        // prefix. Resolve the variant to its unique sum type.
                        let (enum_name, enum_info) = self.resolve_bare_variant(class_name)?;
                        let variant = enum_info.variants.iter().find(|v| v.name == *class_name).unwrap();
                        if sub_patterns.len() != variant.fields.len() {
                            return Err(TypeError(format!(
                                "variant `{}` has {} fields but pattern has {} sub-patterns",
                                class_name,
                                variant.fields.len(),
                                sub_patterns.len()
                            )));
                        }
                        let enum_args = match expected_ty {
                            Type::Enum(sn) if *sn == *enum_name => Vec::new(),
                            Type::Class(name, args) if *name == *enum_name => args.clone(),
                            _ => {
                                return Err(TypeError(format!(
                                    "cannot match variant `{}` against value of type `{}`",
                                    class_name,
                                    expected_ty.name()
                                )));
                            }
                        };
                        let subst = build_subst(&enum_info.generic_params, &enum_args);
                        for (sub, (_, field_ty)) in sub_patterns.iter().zip(variant.fields.iter()) {
                            let sub_ty = substitute_type(field_ty, &subst);
                            bindings.extend(self.check_pattern(
                                sub, &sub_ty, class, locals, in_instance, return_ty, generic_params,
                            )?);
                        }
                    }
                }
            }
            Pattern::Range(start, end, _) => {
                if !expected_ty.is_int() && !expected_ty.is_float() {
                    return Err(TypeError(format!(
                        "range pattern requires int or float subject, got {}",
                        expected_ty.name()
                    )));
                }
                let start_ty = self.infer_expr(start, class, locals, in_instance, return_ty, generic_params)?;
                let end_ty = self.infer_expr(end, class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_assignable(expected_ty, &start_ty) || !self.is_assignable(expected_ty, &end_ty) {
                    return Err(TypeError(format!(
                        "range pattern bounds must match subject type {}, got {} and {}",
                        expected_ty.name(), start_ty.name(), end_ty.name()
                    )));
                }
            }
            Pattern::Int(v) if !expected_ty.is_int() => {
                return Err(TypeError(format!(
                    "integer pattern cannot match subject of type {}",
                    expected_ty.name()
                )));
            }
            Pattern::Int(v) if expected_ty.is_int() && !self.int_target_fits(expected_ty, *v) => {
                return Err(TypeError(format!(
                    "integer pattern {} does not fit subject type {}",
                    v,
                    expected_ty.name()
                )));
            }
            Pattern::Float(_) if !expected_ty.is_float() => {
                return Err(TypeError(format!(
                    "float pattern cannot match subject of type {}",
                    expected_ty.name()
                )));
            }
            Pattern::Bool(_) if expected_ty != &Type::Bool => {
                return Err(TypeError(format!(
                    "bool pattern cannot match subject of type {}",
                    expected_ty.name()
                )));
            }
            Pattern::StringLit(_) if expected_ty != &Type::String => {
                return Err(TypeError(format!(
                    "string pattern cannot match subject of type {}",
                    expected_ty.name()
                )));
            }
            Pattern::Int(_) | Pattern::Float(_) | Pattern::Bool(_) | Pattern::StringLit(_) | Pattern::Null | Pattern::Wildcard => {}
        }
        Ok(bindings)
    }

    fn infer_expr(
        &self,
        expr: &Expr,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        match expr {
            Expr::IntLit(v, suffix) => match suffix {
                IntSuffix::None => Ok(Type::IntLit(*v)),
                _ => self.suffixed_int_type(*suffix, *v),
            },
            Expr::FloatLit(v, suffix) => Ok(match suffix {
                FloatSuffix::None => Type::FloatLit(*v),
                FloatSuffix::F32 => Type::Float32,
                FloatSuffix::F64 => Type::Float64,
            }),
            Expr::Cast(inner, ty) => {
                let inner_ty = self.infer_expr(inner, class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_numeric(ty) {
                    return Err(TypeError(format!(
                        "cannot cast to non-numeric type {}",
                        ty.name()
                    )));
                }
                if !self.is_numeric(&inner_ty) {
                    return Err(TypeError(format!(
                        "cannot cast {} to {}",
                        inner_ty.name(),
                        ty.name()
                    )));
                }
                Ok(ty.clone())
            }
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::Char(_) => Ok(Type::Char),
            Expr::String(_) => Ok(Type::String),
            Expr::InterpolatedString(parts) => {
                // Type check all expressions in the interpolation
                for part in parts {
                    if let InterpPart::Expr(expr) = part {
                        self.infer_expr(expr, class, locals, in_instance, return_ty, generic_params)?;
                    }
                }
                Ok(Type::String)
            }
            Expr::Null => Ok(Type::Class("null".to_string(), Vec::new())), // null reference type
            Expr::Var(name) => {
                if name == "this" {
                    if in_instance {
                        Ok(Type::Class(class.name.clone(), Vec::new()))
                    } else {
                        Err(TypeError("`this` in static method".to_string()))
                    }
                } else if name == "super" {
                    Err(TypeError("`super` must be used as `super.member`".to_string()))
                } else if let Some(ty) = locals.get(name) {
                    Ok(ty.clone())
                } else if in_instance {
                    if let Some((declared_in, ty, visibility)) = self.find_instance_field(&class.name, name) {
                        if !self.can_access(&class.name, &declared_in, visibility) {
                            return Err(TypeError(format!(
                                "field `{}` on `{}` is {}",
                                name, declared_in, visibility_name(visibility)
                            )));
                        }
                        Ok(ty)
                    } else if let Some((declared_in, ty, visibility)) = self.find_static_field(&class.name, name) {
                        if !self.can_access(&class.name, &declared_in, visibility) {
                            return Err(TypeError(format!(
                                "static field `{}` on `{}` is {}",
                                name, declared_in, visibility_name(visibility)
                            )));
                        }
                        Ok(ty)
                    } else {
                        Err(TypeError(format!("unknown variable `{}`", name)))
                    }
                } else if let Some((declared_in, ty, visibility)) = self.find_static_field(&class.name, name) {
                    if !self.can_access(&class.name, &declared_in, visibility) {
                        return Err(TypeError(format!(
                            "static field `{}` on `{}` is {}",
                            name, declared_in, visibility_name(visibility)
                        )));
                    }
                    Ok(ty)
                } else {
                    Err(TypeError(format!("unknown variable `{}`", name)))
                }
            }
            Expr::Field(obj, name) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance, return_ty, generic_params)?;
                let (class_name, type_args) = if let Type::Class(name, args) = &obj_ty {
                    (name.clone(), args.clone())
                } else if in_instance && matches!(obj.as_ref(), Expr::Var(n) if n == "this") {
                    // For `this`, use the current class with empty type args
                    (class.name.clone(), vec![])
                } else {
                    return Err(TypeError(format!(
                        "cannot access field `{}` on non-class type {}",
                        name,
                        obj_ty.name()
                    )));
                };
                if !self.classes.contains_key(&class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                }
                if let Some((declared_in, pi)) = self.find_instance_property(&class_name, name) {
                    if !pi.has_getter {
                        return Err(TypeError(format!(
                            "property `{}` on `{}` has no getter",
                            name, declared_in
                        )));
                    }
                    if !self.can_access(&class.name, &declared_in, pi.getter_visibility) {
                        return Err(TypeError(format!(
                            "property `{}` on `{}` is {}",
                            name, declared_in, visibility_name(pi.getter_visibility)
                        )));
                    }
                    let target_class = self.classes.get(&class_name).unwrap();
                    let subst = build_subst(&target_class.generic_params, &type_args);
                    return Ok(substitute_type(&pi.ty, &subst));
                }
                let (declared_in, field_type, visibility) =
                    self.find_instance_field(&class_name, name).ok_or_else(|| {
                        TypeError(format!("unknown field `{}` on `{}`", name, class_name))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "field `{}` on `{}` is {}",
                        name, declared_in, visibility_name(visibility)
                    )));
                }

                // Apply substitution for generic type parameters
                let target_class = self.classes.get(&class_name).unwrap();
                let subst = build_subst(&target_class.generic_params, &type_args);
                Ok(substitute_type(&field_type, &subst))
            }
            Expr::StaticField(class_name, name) => {
                if self.enums.contains_key(class_name) {
                    return self.infer_enum_construction(
                        class_name, name, &[], class, locals, in_instance, return_ty, generic_params,
                    );
                }
                if !self.classes.contains_key(class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                }
                if let Some((declared_in, pi)) = self.find_static_property(class_name, name) {
                    if !pi.has_getter {
                        return Err(TypeError(format!(
                            "static property `{}` on `{}` has no getter",
                            name, declared_in
                        )));
                    }
                    if !self.can_access(&class.name, &declared_in, pi.getter_visibility) {
                        return Err(TypeError(format!(
                            "static property `{}` on `{}` is {}",
                            name, declared_in, visibility_name(pi.getter_visibility)
                        )));
                    }
                    return Ok(pi.ty);
                }
                let (declared_in, ty, visibility) =
                    self.find_static_field(class_name, name).ok_or_else(|| {
                        TypeError(format!("unknown static field `{}` on `{}`", name, class_name))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "static field `{}` on `{}` is {}",
                        name, declared_in, visibility_name(visibility)
                    )));
                }
                Ok(ty)
            }
            Expr::Binary(op, left, right) => {
                let lt = self.infer_expr(left, class, locals, in_instance, return_ty, generic_params)?;
                let rt = self.infer_expr(right, class, locals, in_instance, return_ty, generic_params)?;
                if op.is_comparison() {
                    let lt_is_null = matches!(&lt, Type::Class(n, _) if n == "null");
                    let rt_is_null = matches!(&rt, Type::Class(n, _) if n == "null");
                    if !lt_is_null && !rt_is_null {
                        if !self.is_numeric(&lt) || !self.is_numeric(&rt) {
                            if lt != rt {
                                return Err(TypeError(format!(
                                    "cannot compare {} and {}",
                                    lt.name(),
                                    rt.name()
                                )));
                            }
                        }
                    }
                    Ok(Type::Bool)
                } else if matches!(op, BinOp::And | BinOp::Or) {
                    if lt != Type::Bool || rt != Type::Bool {
                        return Err(TypeError("logical operators require booleans".to_string()));
                    }
                    Ok(Type::Bool)
                } else {
                    self.arithmetic_type(&lt, &rt)
                }
            }
            Expr::Unary(op, operand) => {
                match op {
                    UnaryOp::Neg => {
                        // Fold negation into literals so `-128` can coerce to
                        // int8 and negative suffixed literals are range-checked
                        // against their folded value.
                        if let Expr::IntLit(v, suffix) = operand.as_ref() {
                            let neg = v.checked_neg().ok_or_else(|| {
                                TypeError(format!("integer literal {} is too small to negate", v))
                            })?;
                            return self.suffixed_int_type(*suffix, neg);
                        }
                        if let Expr::FloatLit(f, suffix) = operand.as_ref() {
                            return Ok(match suffix {
                                FloatSuffix::None => Type::FloatLit(-f),
                                FloatSuffix::F32 => Type::Float32,
                                FloatSuffix::F64 => Type::Float64,
                            });
                        }
                        let ty = self.infer_expr(operand, class, locals, in_instance, return_ty, generic_params)?;
                        if !self.is_numeric(&ty) {
                            return Err(TypeError(format!("cannot negate {}", ty.name())));
                        }
                        Ok(ty)
                    }
                    UnaryOp::Not => {
                        let ty = self.infer_expr(operand, class, locals, in_instance, return_ty, generic_params)?;
                        if ty != Type::Bool {
                            return Err(TypeError("`!` requires bool".to_string()));
                        }
                        Ok(Type::Bool)
                    }
                }
            }
            Expr::Call(call) => self.check_call(call, class, locals, in_instance, return_ty, generic_params),
            Expr::New(class_name, type_args, args) => {
                let Some(class_info) = self.classes.get(class_name) else {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                };
                if class_info.is_abstract || class_info.is_interface {
                    return Err(TypeError(format!(
                        "cannot instantiate {} `{}`",
                        if class_info.is_interface { "interface" } else { "abstract class" },
                        class_name
                    )));
                }
                let subst = build_subst(&class_info.generic_params, type_args);
                let arg_types: Vec<Type> = args
                    .iter()
                    .map(|a| self.infer_expr(a, class, locals, in_instance, return_ty, generic_params))
                    .collect::<Result<_, _>>()?;
                let selected = class_info
                    .constructors
                    .iter()
                    .find(|ctor| {
                        ctor.params.len() == arg_types.len()
                            && ctor
                                .params
                                .iter()
                                .zip(arg_types.iter())
                                .all(|(expected, actual)| {
                                    self.is_assignable(&substitute_type(expected, &subst), actual)
                                })
                    });
                let Some(ctor) = selected else {
                    let sigs: Vec<String> = class_info
                        .constructors
                        .iter()
                        .map(|c| {
                            format!(
                                "({})",
                                c.params
                                    .iter()
                                    .map(|p| substitute_type(p, &subst).name())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        })
                        .collect();
                    return Err(TypeError(format!(
                        "no constructor of `{}` matches arguments ({}){}",
                        class_name,
                        arg_types.iter().map(|t| t.name()).collect::<Vec<_>>().join(", "),
                        if sigs.is_empty() {
                            String::new()
                        } else {
                            format!("; available: {}", sigs.join(", "))
                        }
                    )));
                };
                let param_types: Vec<Type> = ctor
                    .params
                    .iter()
                    .map(|p| substitute_type(p, &subst))
                    .collect();
                for (arg_ty, expected) in arg_types.iter().zip(param_types.iter()) {
                    if !self.is_assignable(expected, arg_ty) {
                        return Err(TypeError(format!(
                            "argument of type {} is not assignable to constructor parameter of type {}",
                            arg_ty.name(),
                            expected.name()
                        )));
                    }
                }
                Ok(Type::Class(class_name.clone(), type_args.clone()))
            }
            Expr::Ternary(cond, then_expr, else_expr) => {
                let cond_ty = self.infer_expr(cond, class, locals, in_instance, return_ty, generic_params)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "ternary condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
                let then_ty = self.infer_expr(then_expr, class, locals, in_instance, return_ty, generic_params)?;
                let else_ty = self.infer_expr(else_expr, class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_assignable(&then_ty, &else_ty) {
                    return Err(TypeError(format!(
                        "ternary branches must have compatible types, got {} and {}",
                        then_ty.name(),
                        else_ty.name()
                    )));
                }
                Ok(then_ty)
            }
            Expr::NullCoalesce(left, right) => {
                let left_ty = self.infer_expr(left, class, locals, in_instance, return_ty, generic_params)?;
                let right_ty = self.infer_expr(right, class, locals, in_instance, return_ty, generic_params)?;
                // The result type is the left type (which should be nullable)
                // For now, we just check that the types are compatible
                if !self.is_assignable(&left_ty, &right_ty) && !self.is_assignable(&right_ty, &left_ty) {
                    return Err(TypeError(format!(
                        "null coalescing operands must have compatible types, got {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    )));
                }
                Ok(left_ty)
            }
            Expr::NullConditionalField(obj, field_name) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance, return_ty, generic_params)?;
                let class_name = if let Type::Class(name, _) = &obj_ty {
                    name.clone()
                } else {
                    return Err(TypeError(format!(
                        "null conditional field access requires class type, got {}",
                        obj_ty.name()
                    )));
                };
                if !self.classes.contains_key(&class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                }
                let (declared_in, ty, visibility) =
                    self.find_instance_field(&class_name, field_name).ok_or_else(|| {
                        TypeError(format!("unknown field `{}` on `{}`", field_name, class_name))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "field `{}` on `{}` is {}",
                        field_name, declared_in, visibility_name(visibility)
                    )));
                }
                // Result is nullable version of the field type
                Ok(ty)
            }
            Expr::NullConditionalCall(call) => {
                // Similar to regular call, but the result is nullable
                self.check_call(call, class, locals, in_instance, return_ty, generic_params)
            }
            Expr::Match(subject, arms) => {
                let subject_ty = self.infer_expr(subject, class, locals, in_instance, return_ty, generic_params)?;
                
                let mut result_ty: Option<Type> = None;
                
                for arm in arms {
                    let mut arm_bindings: Vec<(String, Type)> = Vec::new();
                    for pattern in &arm.patterns {
                        arm_bindings.extend(self.check_pattern(
                            pattern, &subject_ty, class, locals, in_instance, return_ty, generic_params,
                        )?);
                    }
                    
                    // Bind the pattern names for the arm body and guard.
                    let mut arm_locals = locals.clone();
                    for (name, ty) in &arm_bindings {
                        arm_locals.insert(name.clone(), ty.clone());
                    }
                    
                    // Type check guard
                    if let Some(guard) = &arm.guard {
                        let guard_ty = self.infer_expr(guard, class, &arm_locals, in_instance, return_ty, generic_params)?;
                        if guard_ty != Type::Bool {
                            return Err(TypeError(format!(
                                "match guard must be bool, got {}",
                                guard_ty.name()
                            )));
                        }
                    }
                    
                    // Type check body
                    let body_ty = self.infer_expr(&arm.body, class, &arm_locals, in_instance, return_ty, generic_params)?;
                    
                    // Check that all arms have compatible types
                    if let Some(ref expected) = result_ty {
                        if !self.is_assignable(expected, &body_ty) {
                            return Err(TypeError(format!(
                                "match arms must have compatible types, expected {} but got {}",
                                expected.name(),
                                body_ty.name()
                            )));
                        }
                    } else {
                        result_ty = Some(body_ty);
                    }
                }
                
                Ok(result_ty.unwrap_or(Type::Unit))
            }
            Expr::EnumVariant(enum_name, variant_name, args) => {
                return self.infer_enum_construction(
                    enum_name, variant_name, args, class, locals, in_instance, return_ty, generic_params,
                );
            }
            Expr::Tuple(elements) => {
                let mut types = Vec::new();
                for elem in elements {
                    types.push(self.infer_expr(elem, class, locals, in_instance, return_ty, generic_params)?);
                }
                Ok(Type::Tuple(types))
            }
            Expr::TupleIndex(tuple, idx) => {
                let tuple_ty = self.infer_expr(tuple, class, locals, in_instance, return_ty, generic_params)?;
                if let Type::Tuple(types) = &tuple_ty {
                    if *idx >= types.len() {
                        return Err(TypeError(format!(
                            "tuple index {} out of bounds for tuple of size {}",
                            idx,
                            types.len()
                        )));
                    }
                    Ok(types[*idx].clone())
                } else {
                    return Err(TypeError(format!(
                        "cannot index non-tuple type {}",
                        tuple_ty.name()
                    )));
                }
            }
            Expr::Range(start, end, _inclusive) => {
                let start_ty = self.infer_expr(start, class, locals, in_instance, return_ty, generic_params)?;
                let end_ty = self.infer_expr(end, class, locals, in_instance, return_ty, generic_params)?;
                if !self.resolve_literal(&start_ty).is_int() {
                    return Err(TypeError(format!(
                        "range start must be an integer, got {}",
                        start_ty.name()
                    )));
                }
                if !self.resolve_literal(&end_ty).is_int() {
                    return Err(TypeError(format!(
                        "range end must be an integer, got {}",
                        end_ty.name()
                    )));
                }
                // Return a special Range type for now
                Ok(Type::Class("Range".to_string(), vec![]))
            }
            Expr::SuperCall(method_name, args) => {
                if !in_instance {
                    return Err(TypeError("`super` in static method".to_string()));
                }
                let super_name = self.super_class_of(class)?;
                let (_, method_info) = self.find_method(super_name, method_name).ok_or_else(|| {
                    TypeError(format!(
                        "unknown method `{}` on super class `{}`",
                        method_name, super_name
                    ))
                })?;
                if !method_info.is_instance {
                    return Err(TypeError(format!(
                        "`super.{method_name}` is a static method; use `{super_name}.{method_name}`"
                    )));
                }
                if method_info.is_abstract {
                    return Err(TypeError(format!(
                        "cannot call abstract method `{super_name}.{method_name}` via `super`"
                    )));
                }
                if args.len() != method_info.params.len() {
                    return Err(TypeError(format!(
                        "method `{}` expects {} arguments, got {}",
                        method_name,
                        method_info.params.len(),
                        args.len()
                    )));
                }
                for (arg, expected) in args.iter().zip(method_info.params.iter()) {
                    let arg_ty = self.infer_expr(arg, class, locals, in_instance, return_ty, generic_params)?;
                    if !self.is_assignable(expected, &arg_ty) {
                        return Err(TypeError(format!(
                            "argument type mismatch in `{}`: expected {}, got {}",
                            method_name,
                            expected.name(),
                            arg_ty.name()
                        )));
                    }
                }
                Ok(method_info.return_ty.clone())
            }
            Expr::SuperField(field_name) => {
                if !in_instance {
                    return Err(TypeError("`super` in static method".to_string()));
                }
                let super_name = self.super_class_of(class)?;
                if let Some((declared_in, pi)) = self.find_instance_property(super_name, field_name) {
                    if !pi.has_getter {
                        return Err(TypeError(format!(
                            "property `{}` on super class `{}` has no getter",
                            field_name, super_name
                        )));
                    }
                    if !self.can_access(&class.name, &declared_in, pi.getter_visibility) {
                        return Err(TypeError(format!(
                            "property `{}` on `{}` is {}",
                            field_name, declared_in, visibility_name(pi.getter_visibility)
                        )));
                    }
                    return Ok(pi.ty);
                }
                let (declared_in, ty, visibility) =
                    self.find_instance_field(super_name, field_name).ok_or_else(|| {
                        TypeError(format!(
                            "unknown field `{}` on super class `{}`",
                            field_name, super_name
                        ))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "field `{}` on `{}` is {}",
                        field_name, declared_in, visibility_name(visibility)
                    )));
                }
                Ok(ty)
            }
            Expr::With(obj, updates) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance, return_ty, generic_params)?;
                let Type::Class(class_name, _) = &obj_ty else {
                    return Err(TypeError(format!(
                        "`with` requires a record value, got {}",
                        obj_ty.name()
                    )));
                };
                let info = self.classes.get(class_name).ok_or_else(|| {
                    TypeError(format!("unknown class `{}`", class_name))
                })?;
                if !info.is_record {
                    return Err(TypeError(format!(
                        "`with` requires a record value, got {}",
                        obj_ty.name()
                    )));
                }
                for (field, value) in updates {
                    let field_ty = info
                        .instance_fields
                        .iter()
                        .find(|(n, _)| n == field)
                        .map(|(_, t)| t.clone())
                        .ok_or_else(|| {
                            TypeError(format!("record `{}` has no field `{}`", class_name, field))
                        })?;
                    let value_ty = self.infer_expr(value, class, locals, in_instance, return_ty, generic_params)?;
                    if !self.is_assignable(&field_ty, &value_ty) {
                        return Err(TypeError(format!(
                            "cannot assign {} to record field `{}.{}` of type {}",
                            value_ty.name(),
                            class_name,
                            field,
                            field_ty.name()
                        )));
                    }
                }
                Ok(obj_ty)
            }
            Expr::Block(stmts) => {
                // Block locals are scoped to the block and do not leak out.
                let mut block_locals = locals.clone();
                let mut result_ty = Type::Unit;
                for (i, s) in stmts.iter().enumerate() {
                    let is_last = i + 1 == stmts.len();
                    if is_last {
                        if let Stmt::Expr(e) = s {
                            result_ty = self.infer_expr(e, class, &block_locals, in_instance, return_ty, generic_params)?;
                        } else {
                            self.check_stmt(s, class, &mut block_locals, return_ty, in_instance, generic_params)?;
                        }
                    } else {
                        self.check_stmt(s, class, &mut block_locals, return_ty, in_instance, generic_params)?;
                    }
                }
                Ok(result_ty)
            }
        }
    }

    /// Resolve the super class name for the current class, or error if none.
    fn super_class_of<'a>(&self, class: &'a ClassInfo) -> Result<&'a str, TypeError> {
        class.super_class.as_deref().ok_or_else(|| {
            TypeError(format!("`{}` has no super class", class.name))
        })
    }

    fn infer_assign_target(
        &self,
        target: &AssignTarget,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        match target {
            AssignTarget::Local(name) => locals
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError(format!("unknown variable `{}`", name))),
            AssignTarget::Field(obj, name) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance, return_ty, generic_params)?;
                let (class_name, type_args) = if let Type::Class(cname, args) = &obj_ty {
                    (cname.clone(), args.clone())
                } else if in_instance && matches!(obj.as_ref(), Expr::Var(n) if n == "this") {
                    (class.name.clone(), vec![])
                } else {
                    return Err(TypeError(format!(
                        "cannot assign field `{}` on non-class type {}",
                        name,
                        obj_ty.name()
                    )));
                };
                if !self.classes.contains_key(&class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                }
                let target_class = self.classes.get(&class_name).unwrap();
                let subst = build_subst(&target_class.generic_params, &type_args);
                if let Some((declared_in, pi)) = self.find_instance_property(&class_name, name) {
                    if !pi.has_setter {
                        return Err(TypeError(format!(
                            "property `{}` on `{}` has no setter",
                            name, declared_in
                        )));
                    }
                    if !self.can_access(&class.name, &declared_in, pi.setter_visibility) {
                        return Err(TypeError(format!(
                            "property `{}` on `{}` is {}",
                            name, declared_in, visibility_name(pi.setter_visibility)
                        )));
                    }
                    return Ok(substitute_type(&pi.ty, &subst));
                }
                let (declared_in, ty, visibility) =
                    self.find_instance_field(&class_name, name).ok_or_else(|| {
                        TypeError(format!("unknown field `{}` on `{}`", name, class_name))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "field `{}` on `{}` is {}",
                        name, declared_in, visibility_name(visibility)
                    )));
                }
                Ok(substitute_type(&ty, &subst))
            }
            AssignTarget::StaticField(class_name, name) => {
                if !self.classes.contains_key(class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                }
                if let Some((declared_in, pi)) = self.find_static_property(class_name, name) {
                    if !pi.has_setter {
                        return Err(TypeError(format!(
                            "static property `{}` on `{}` has no setter",
                            name, declared_in
                        )));
                    }
                    if !self.can_access(&class.name, &declared_in, pi.setter_visibility) {
                        return Err(TypeError(format!(
                            "static property `{}` on `{}` is {}",
                            name, declared_in, visibility_name(pi.setter_visibility)
                        )));
                    }
                    return Ok(pi.ty);
                }
                let (declared_in, ty, visibility) =
                    self.find_static_field(class_name, name).ok_or_else(|| {
                        TypeError(format!("unknown static field `{}` on `{}`", name, class_name))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "static field `{}` on `{}` is {}",
                        name, declared_in, visibility_name(visibility)
                    )));
                }
                Ok(ty)
            }
            AssignTarget::SuperField(name) => {
                if !in_instance {
                    return Err(TypeError("`super` in static method".to_string()));
                }
                let super_name = self.super_class_of(class)?;
                if let Some((declared_in, pi)) = self.find_instance_property(super_name, name) {
                    if !pi.has_setter {
                        return Err(TypeError(format!(
                            "property `{}` on super class `{}` has no setter",
                            name, super_name
                        )));
                    }
                    if !self.can_access(&class.name, &declared_in, pi.setter_visibility) {
                        return Err(TypeError(format!(
                            "property `{}` on `{}` is {}",
                            name, declared_in, visibility_name(pi.setter_visibility)
                        )));
                    }
                    return Ok(pi.ty);
                }
                let (declared_in, ty, visibility) =
                    self.find_instance_field(super_name, name).ok_or_else(|| {
                        TypeError(format!(
                            "unknown field `{}` on super class `{}`",
                            name, super_name
                        ))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "field `{}` on `{}` is {}",
                        name, declared_in, visibility_name(visibility)
                    )));
                }
                Ok(ty)
            }
        }
    }

    /// Whether a type is a bare reference to the given type parameter (either
    /// the parser's `Class(name, [])` form or an explicit `GenericParam`).
    fn is_param_ref(ty: &Type, param: &str) -> bool {
        matches!(ty, Type::Class(n, args) if n == param && args.is_empty())
            || matches!(ty, Type::GenericParam(n) if n == param)
    }

    /// Resolve a bare variant name to the single sum type that declares it.
    /// Returns an error if no enum declares it, or if several do (ambiguous).
    fn resolve_bare_variant(&self, variant_name: &str) -> Result<(String, EnumInfo), TypeError> {
        let mut found = None;
        for (name, info) in &self.enums {
            if info.variants.iter().any(|v| v.name == variant_name) {
                if found.is_some() {
                    return Err(TypeError(format!(
                        "variant `{}` is ambiguous: declared by multiple sum types; use `Type.{}`",
                        variant_name, variant_name
                    )));
                }
                found = Some((name.clone(), info.clone()));
            }
        }
        found.ok_or_else(|| TypeError(format!("unknown variant `{}`", variant_name)))
    }

    /// Type-check construction of an enum variant, e.g. `Result.Ok(5)` or a
    /// bare `Ok(5)`. `enum_name` is the resolved sum-type/enum name. Generic
    /// type parameters that appear in the selected variant's fields are
    /// inferred from the argument expressions; parameters not used by this
    /// variant are left as `Type::GenericParam` and later unified against an
    /// expected type by `is_assignable`.
    fn infer_enum_construction(
        &self,
        enum_name: &str,
        variant_name: &str,
        args: &[Expr],
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        let enum_info = self.enums.get(enum_name).ok_or_else(|| {
            TypeError(format!("unknown enum `{}`", enum_name))
        })?;
        let variant = enum_info.variants.iter().find(|v| v.name == *variant_name)
            .ok_or_else(|| TypeError(format!(
                "unknown variant `{}.{}`", enum_name, variant_name
            )))?;
        if args.len() != variant.fields.len() {
            return Err(TypeError(format!(
                "variant `{}.{}` expects {} arguments, got {}",
                enum_name, variant_name, variant.fields.len(), args.len()
            )));
        }

        let mut arg_tys = Vec::with_capacity(args.len());
        for arg in args {
            arg_tys.push(self.infer_expr(arg, class, locals, in_instance, return_ty, generic_params)?);
        }

        // Infer type parameters from arguments that occupy whole field slots.
        let mut subst: TypeSubst = HashMap::new();
        for ((_, field_ty), arg_ty) in variant.fields.iter().zip(arg_tys.iter()) {
            if let Some(p) = enum_info.generic_params.iter().find(|gp| Self::is_param_ref(field_ty, &gp.name)) {
                subst.insert(p.name.clone(), arg_ty.clone());
            }
        }

        // Validate each argument against its substituted field type; an
        // unbound parameter is satisfied by the argument that fills its slot.
        for ((_, field_ty), arg_ty) in variant.fields.iter().zip(arg_tys.iter()) {
            let expected = substitute_type(field_ty, &subst);
            let unbound = enum_info.generic_params.iter()
                .find(|gp| Self::is_param_ref(&expected, &gp.name));
            if let Some(p) = unbound {
                subst.insert(p.name.clone(), arg_ty.clone());
            } else if !self.is_assignable(&expected, arg_ty) {
                return Err(TypeError(format!(
                    "argument type mismatch in `{}.{}`: expected {}, got {}",
                    enum_name, variant_name, expected.name(), arg_ty.name()
                )));
            }
        }

        if enum_info.generic_params.is_empty() {
            Ok(Type::Enum(enum_name.to_string()))
        } else {
            let type_args = enum_info.generic_params.iter().map(|gp| {
                subst.get(&gp.name).cloned()
                    .unwrap_or_else(|| Type::GenericParam(gp.name.clone()))
            }).collect();
            Ok(Type::Class(enum_name.to_string(), type_args))
        }
    }

    fn check_call(
        &self,
        call: &CallExpr,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        if call.class_or_target == "__intrinsics" {
            if call.method == "print" {
                if call.args.len() != 1 {
                    return Err(TypeError("print expects 1 argument".to_string()));
                }
                let _ = self.infer_expr(&call.args[0], class, locals, in_instance, return_ty, generic_params)?;
                return Ok(Type::Unit);
            }
            if call.method == "println" {
                if !call.args.is_empty() {
                    return Err(TypeError("println expects 0 arguments".to_string()));
                }
                return Ok(Type::Unit);
            }
        }

        if let Some(target) = &call.target {
            // instance call: target.Method(args)
            // Special case: ClassName.Method syntax was parsed as target=Var(ClassName).
            if let Expr::Var(class_name) = target.as_ref() {
                if class_name == "super" {
                    return Err(TypeError("`super` in static method".to_string()));
                }
                if class_name == "this" {
                    if !in_instance {
                        return Err(TypeError("`this` in static method".to_string()));
                    }
                    let (declared_in, method_info) = self
                        .find_method(&class.name, &call.method)
                        .ok_or_else(|| {
                            TypeError(format!(
                                "unknown method `{}` on `{}`",
                                call.method, class.name
                            ))
                        })?;
                    if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                        return Err(TypeError(format!(
                            "method `{}` on `{}` is {}",
                            call.method, declared_in, visibility_name(method_info.visibility)
                        )));
                    }
                    return self.check_call_args(call, &method_info, class, locals, in_instance, return_ty, generic_params);
                }
                if self.classes.contains_key(class_name) {
                    let (declared_in, method_info) = self
                        .find_static_method(class_name, &call.method)
                        .ok_or_else(|| {
                            TypeError(format!(
                                "unknown static method `{}` on `{}`",
                                call.method, class_name
                            ))
                        })?;
                    if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                        return Err(TypeError(format!(
                            "static method `{}` on `{}` is {}",
                            call.method, declared_in, visibility_name(method_info.visibility)
                        )));
                    }
                    return self.check_call_args(call, &method_info, class, locals, in_instance, return_ty, generic_params);
                }
            }
            let target_ty = self.infer_expr(target, class, locals, in_instance, return_ty, generic_params)?;
            let (name, type_args) = if let Type::Class(name, args) = &target_ty {
                (name.clone(), args.clone())
            } else if in_instance && matches!(target.as_ref(), Expr::Var(n) if n == "this") {
                // For `this`, use the current class with empty type args
                // (we're inside the generic class definition)
                (class.name.clone(), vec![])
            } else {
                return Err(TypeError(format!(
                    "cannot call method on non-class type {}",
                    target_ty.name()
                )));
            };
            if !self.classes.contains_key(&name) {
                return Err(TypeError(format!("unknown class `{}`", name)));
            }
            let (declared_in, method_info) = self.find_method(&name, &call.method).ok_or_else(|| {
                TypeError(format!("unknown method `{}` on `{}`", call.method, name))
            })?;
            if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                return Err(TypeError(format!(
                    "method `{}` on `{}` is {}",
                    call.method, declared_in, visibility_name(method_info.visibility)
                )));
            }

            // Build substitution map from class generic params and type args
            let target_class = self.classes.get(&name).unwrap();
            let subst = build_subst(&target_class.generic_params, &type_args);
            
            // Check arguments with substituted types
            return self.check_call_args_with_subst(call, &method_info, &subst, class, locals, in_instance, return_ty, generic_params);
        } else {
            // static call: ClassName.Method(args) or EnumName.Variant(args)
            let class_name = call.class_or_target.clone();

            if self.enums.contains_key(&class_name) {
                return self.infer_enum_construction(
                    &class_name, &call.method, &call.args, class, locals, in_instance, return_ty, generic_params,
                );
            }

            if !self.classes.contains_key(&class_name) {
                // Bare variant construction: `Ok(5)` where `Ok` is a sum-type
                // variant. Only attempt this when a matching variant exists, so
                // unknown-class errors keep their usual message.
                if self.enums.values().any(|info| info.variants.iter().any(|v| v.name == call.method)) {
                    let (enum_name, _) = self.resolve_bare_variant(&call.method)?;
                    return self.infer_enum_construction(
                        &enum_name, &call.method, &call.args, class, locals, in_instance, return_ty, generic_params,
                    );
                }
                return Err(TypeError(format!("unknown class `{}`", class_name)));
            }
            let (declared_in, method_info) = self
                .find_static_method(&class_name, &call.method)
                .ok_or_else(|| {
                    TypeError(format!(
                        "unknown static method `{}` on `{}`",
                        call.method, class_name
                    ))
                })?;
            if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                return Err(TypeError(format!(
                    "static method `{}` on `{}` is {}",
                    call.method, declared_in, visibility_name(method_info.visibility)
                )));
            }
            return self.check_call_args(call, &method_info, class, locals, in_instance, return_ty, generic_params);
        }
    }

    fn check_call_args(
        &self,
        call: &CallExpr,
        method_info: &MethodInfo,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        if call.args.len() != method_info.params.len() {
            return Err(TypeError(format!(
                "method `{}` expects {} arguments, got {}",
                call.method,
                method_info.params.len(),
                call.args.len()
            )));
        }
        for (arg, expected) in call.args.iter().zip(method_info.params.iter()) {
            let arg_ty = self.infer_expr(arg, class, locals, in_instance, return_ty, generic_params)?;
            if !self.is_assignable(expected, &arg_ty) {
                return Err(TypeError(format!(
                    "argument type mismatch in `{}`: expected {}, got {}",
                    call.method,
                    expected.name(),
                    arg_ty.name()
                )));
            }
        }
        Ok(method_info.return_ty.clone())
    }

    fn check_call_args_with_subst(
        &self,
        call: &CallExpr,
        method_info: &MethodInfo,
        subst: &TypeSubst,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        // Substitute generic parameters in method signature
        let substituted_params: Vec<Type> = method_info.params.iter()
            .map(|p| substitute_type(p, subst))
            .collect();
        let substituted_return = substitute_type(&method_info.return_ty, subst);
        
        if call.args.len() != substituted_params.len() {
            return Err(TypeError(format!(
                "method `{}` expects {} arguments, got {}",
                call.method,
                substituted_params.len(),
                call.args.len()
            )));
        }
        for (arg, expected) in call.args.iter().zip(substituted_params.iter()) {
            let arg_ty = self.infer_expr(arg, class, locals, in_instance, return_ty, generic_params)?;
            if !self.is_assignable(expected, &arg_ty) {
                return Err(TypeError(format!(
                    "argument type mismatch in `{}`: expected {}, got {}",
                    call.method,
                    expected.name(),
                    arg_ty.name()
                )));
            }
        }
        Ok(substituted_return)
    }

    fn arithmetic_type(&self, a: &Type, b: &Type) -> Result<Type, TypeError> {
        let a = self.resolve_literal(a);
        let b = self.resolve_literal(b);
        if !a.is_numeric() || !b.is_numeric() {
            return Err(TypeError(format!(
                "cannot operate on {} and {}",
                a.name(),
                b.name()
            )));
        }
        Ok(promote(&a, &b).ok_or_else(|| {
            TypeError(format!(
                "cannot promote {} and {} to a common type",
                a.name(),
                b.name()
            ))
        })?)
    }

    /// True if this is any numeric type, including untyped literal markers.
    fn is_numeric(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Int8
                | Type::Int16
                | Type::Int32
                | Type::Int64
                | Type::UInt8
                | Type::UInt16
                | Type::UInt32
                | Type::UInt64
                | Type::Float32
                | Type::Float64
                | Type::IntLit(_)
                | Type::FloatLit(_)
        )
    }

    /// Map an untyped literal marker to its natural concrete type.
    fn resolve_literal(&self, ty: &Type) -> Type {
        match ty {
            Type::IntLit(v) => {
                if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                    Type::Int32
                } else {
                    Type::Int64
                }
            }
            Type::FloatLit(_) => Type::Float64,
            _ => ty.clone(),
        }
    }

    /// The concrete type of a suffixed integer literal, checking the value fits.
    fn suffixed_int_type(&self, suffix: IntSuffix, value: i64) -> Result<Type, TypeError> {
        let (ty, min, max) = match suffix {
            IntSuffix::None => return Ok(Type::IntLit(value)),
            IntSuffix::I8 => (Type::Int8, i8::MIN as i64, i8::MAX as i64),
            IntSuffix::I16 => (Type::Int16, i16::MIN as i64, i16::MAX as i64),
            IntSuffix::I32 => (Type::Int32, i32::MIN as i64, i32::MAX as i64),
            IntSuffix::I64 => (Type::Int64, i64::MIN, i64::MAX),
            IntSuffix::U8 => (Type::UInt8, 0, u8::MAX as i64),
            IntSuffix::U16 => (Type::UInt16, 0, u16::MAX as i64),
            IntSuffix::U32 => (Type::UInt32, 0, u32::MAX as i64),
            IntSuffix::U64 => (Type::UInt64, 0, i64::MAX),
        };
        if value < min || value > max {
            return Err(TypeError(format!(
                "integer literal {} does not fit in {}",
                value,
                ty.name()
            )));
        }
        Ok(ty)
    }

    /// True if `source` can be implicitly converted to `target`. Numeric
    /// conversions are widening-only; narrowing and float-to-int require an
    /// explicit cast.
    fn is_assignable(&self, target: &Type, source: &Type) -> bool {
        if target == source {
            return true;
        }
        // Untyped literals coerce to any numeric type whose range fits.
        match (target, source) {
            (target, Type::IntLit(v)) => {
                return self.int_target_fits(target, *v);
            }
            (Type::Float32 | Type::Float64, Type::FloatLit(_)) => return true,
            _ => {}
        }
        if self.is_numeric(target) && self.is_numeric(source) {
            return self.numeric_widening(target, source);
        }
        match (target, source) {
            // Unbound type parameters (from sum-type construction) accept any
            // type; they are unified with an expected type later.
            (Type::GenericParam(_), _) | (_, Type::GenericParam(_)) => true,
            (Type::Class(target_name, target_args), Type::Class(source_name, source_args)) => {
                // Same-name reference: compare type arguments elementwise so a
                // sum type like `Result<int, string>` is checked against an
                // inferred `Result<int, T>`.
                if target_name == source_name
                    && !target_args.is_empty()
                    && !source_args.is_empty()
                {
                    return target_args.len() == source_args.len()
                        && target_args
                            .iter()
                            .zip(source_args)
                            .all(|(t, s)| self.is_assignable(t, s));
                }
                // Null is assignable to any class reference.
                if source_name == "null" {
                    return true;
                }
                // Upcast: a subclass instance is assignable to its base type.
                self.is_subclass_of(source_name, target_name)
            }
            // Null is assignable to string (reference type).
            (Type::String, Type::Class(source_name, _)) if source_name == "null" => true,
            (Type::Enum(a), Type::Enum(b)) => a == b,
            (Type::Class(name, _), Type::Enum(enum_name)) => name == enum_name,
            (Type::Enum(enum_name), Type::Class(name, _)) => name == enum_name,
            (Type::Tuple(t_target), Type::Tuple(t_source)) => {
                t_target.len() == t_source.len()
                    && t_target
                        .iter()
                        .zip(t_source)
                        .all(|(t, s)| self.is_assignable(t, s))
            }
            _ => false,
        }
    }

    /// Whether an integer literal value fits in the target numeric type.
    fn int_target_fits(&self, target: &Type, v: i64) -> bool {
        match target {
            Type::Int8 => v >= i8::MIN as i64 && v <= i8::MAX as i64,
            Type::Int16 => v >= i16::MIN as i64 && v <= i16::MAX as i64,
            Type::Int32 => v >= i32::MIN as i64 && v <= i32::MAX as i64,
            Type::Int64 => true,
            Type::UInt8 => v >= 0 && v <= u8::MAX as i64,
            Type::UInt16 => v >= 0 && v <= u16::MAX as i64,
            Type::UInt32 => v >= 0 && v <= u32::MAX as i64,
            Type::UInt64 => v >= 0,
            Type::Float32 | Type::Float64 => true,
            _ => false,
        }
    }

    /// Implicit widening rules for two concrete numeric types.
    fn numeric_widening(&self, target: &Type, source: &Type) -> bool {
        use Type::*;
        let bits = |t: &Type| match t {
            Int8 | UInt8 => 8,
            Int16 | UInt16 => 16,
            Int32 | UInt32 | Float32 => 32,
            Int64 | UInt64 | Float64 => 64,
            _ => 0,
        };
        match (target, source) {
            // int -> int / uint -> uint: widening only.
            (Int8 | Int16 | Int32 | Int64, Int8 | Int16 | Int32 | Int64)
            | (UInt8 | UInt16 | UInt32 | UInt64, UInt8 | UInt16 | UInt32 | UInt64) => {
                bits(target) >= bits(source)
            }
            // uint -> int: allowed only when the target is strictly wider.
            (Int8 | Int16 | Int32 | Int64, UInt8 | UInt16 | UInt32 | UInt64) => {
                bits(target) > bits(source)
            }
            // int -> uint: never implicit (a signed value can be negative).
            (UInt8 | UInt16 | UInt32 | UInt64, Int8 | Int16 | Int32 | Int64) => false,
            // int -> float.
            (Float32 | Float64, Int8 | Int16 | Int32 | Int64) => true,
            (Float32 | Float64, UInt8 | UInt16 | UInt32 | UInt64) => true,
            // float32 -> float64 widening; float64 -> float32 needs a cast.
            (Float64, Float32) => true,
            _ => false,
        }
    }

    fn validate_type(&self, ty: &Type) -> Result<(), TypeError> {
        match ty {
            Type::Class(name, _) if !self.classes.contains_key(name) => {
                Err(TypeError(format!("unknown type `{}`", name)))
            }
            _ => Ok(()),
        }
    }

    fn validate_type_with_generics(&self, ty: &Type, generic_params: &[GenericParam]) -> Result<(), TypeError> {
        match ty {
            Type::Class(name, args) => {
                if generic_params.iter().any(|gp| gp.name == *name) {
                    return Ok(());
                }
                if !self.classes.contains_key(name) && !self.enums.contains_key(name) {
                    return Err(TypeError(format!("unknown type `{}`", name)));
                }
                for arg in args {
                    self.validate_type_with_generics(arg, generic_params)?;
                }
                Ok(())
            }
            Type::Enum(name) => {
                if !self.enums.contains_key(name) {
                    return Err(TypeError(format!("unknown enum type `{}`", name)));
                }
                Ok(())
            }
            Type::GenericParam(name) => {
                if generic_params.iter().any(|gp| gp.name == *name) {
                    Ok(())
                } else {
                    Err(TypeError(format!("unknown type parameter `{}`", name)))
                }
            }
            _ => Ok(()),
        }
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Bit width of a concrete integer type.
pub(crate) fn int_bits(ty: &Type) -> u8 {
    match ty {
        Type::Int8 | Type::UInt8 => 8,
        Type::Int16 | Type::UInt16 => 16,
        Type::Int32 | Type::UInt32 => 32,
        Type::Int64 | Type::UInt64 => 64,
        _ => 0,
    }
}

/// Common type for a binary numeric operation.
///
/// Rules:
/// - same sign family: the wider type wins;
/// - equal bits but mixed signedness: promote to the next wider signed type
///   (int8+uint8 -> int16, int32+uint32 -> int64); int64+uint64 cannot be
///   promoted;
/// - different bits, mixed signedness: a signed wider operand wins when it is
///   strictly wider than the unsigned one; otherwise promote to a signed type
///   wider than the unsigned operand;
/// - any float operand wins, choosing the wider float.
pub(crate) fn promote(a: &Type, b: &Type) -> Option<Type> {
    if a.is_float() && b.is_float() {
        return Some(if *a == Type::Float32 && *b == Type::Float32 {
            Type::Float32
        } else {
            Type::Float64
        });
    }
    if a.is_float() || b.is_float() {
        return Some(if *a == Type::Float32 || *b == Type::Float32 {
            Type::Float32
        } else {
            Type::Float64
        });
    }
    let a_signed = a.is_signed_int();
    let b_signed = b.is_signed_int();
    let a_bits = int_bits(a);
    let b_bits = int_bits(b);
    if a_signed == b_signed {
        return Some(if a_bits >= b_bits { a.clone() } else { b.clone() });
    }
    // Mixed signedness, equal bits.
    if a_bits == b_bits {
        return match a_bits {
            64 => None,
            32 => Some(Type::Int64),
            16 => Some(Type::Int32),
            _ => Some(Type::Int16),
        };
    }
    // Mixed signedness, different bits.
    let (wide, wide_signed) = if a_bits > b_bits { (a, a_signed) } else { (b, b_signed) };
    if wide_signed {
        Some(wide.clone())
    } else {
        // The wider operand is unsigned and cannot hold negatives; promote to
        // a signed type strictly wider than it.
        match int_bits(wide) {
            64 => None,
            32 => Some(Type::Int64),
            16 => Some(Type::Int32),
            _ => Some(Type::Int16),
        }
    }
}
