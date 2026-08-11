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
                                if f.is_static {
                                    match f.visibility {
                                        Visibility::Protected => {
                                            info.protected_static_fields.insert(f.name.clone());
                                        }
                                        Visibility::Private => {
                                            info.private_static_fields.insert(f.name.clone());
                                        }
                                        Visibility::Public => {}
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
                                        Visibility::Public => {}
                                    }
                                    info.instance_fields.push((f.name.clone(), f.ty.clone()));
                                }
                            }
                            Member::Method(m) => {
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
                        }
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
                    }
                    self.enums.insert(e.name.clone(), EnumInfo {
                        name: e.name.clone(),
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
                    let init_ty = self.infer_expr(init, class, locals, in_instance)?;
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
                let expr_ty = self.infer_expr(expr, class, locals, in_instance)?;
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
                let _ = self.infer_expr(e, class, locals, in_instance)?;
            }
            Stmt::Assign(target, value) => {
                let target_ty = self.infer_assign_target(target, class, locals, in_instance)?;
                let value_ty = self.infer_expr(value, class, locals, in_instance)?;
                if !self.is_assignable(&target_ty, &value_ty) {
                    return Err(TypeError(format!(
                        "cannot assign {} to {}",
                        value_ty.name(),
                        target_ty.name()
                    )));
                }
            }
            Stmt::Return(Some(e)) => {
                let ty = self.infer_expr(e, class, locals, in_instance)?;
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
                let cond_ty = self.infer_expr(cond, class, locals, in_instance)?;
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
            Stmt::While(cond, body) => {
                let cond_ty = self.infer_expr(cond, class, locals, in_instance)?;
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
            Stmt::For(init, cond, update, body) => {
                self.check_stmt(init, class, locals, return_ty, in_instance, generic_params)?;
                let cond_ty = self.infer_expr(cond, class, locals, in_instance)?;
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
            Stmt::ForIn(ty, var_name, range_expr, body) => {
                self.validate_type_with_generics(ty, generic_params)?;
                let range_ty = self.infer_expr(range_expr, class, locals, in_instance)?;
                // For now, only support Range type
                if range_ty != Type::Class("Range".to_string(), vec![]) {
                    return Err(TypeError(format!(
                        "for-in requires a range expression, got {}",
                        range_ty.name()
                    )));
                }
                // The loop variable must match the range element type (int for now)
                if *ty != Type::Int {
                    return Err(TypeError(format!(
                        "for-in loop variable must be int, got {}",
                        ty.name()
                    )));
                }
                locals.insert(var_name.clone(), ty.clone());
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
            }
            Stmt::DoWhile(body, cond) => {
                for s in body {
                    self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                }
                let cond_ty = self.infer_expr(cond, class, locals, in_instance)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "do-while condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
            }
            Stmt::Break | Stmt::Continue => {
                // These are valid inside loops, checked by the emitter
            }
            Stmt::Throw(e) => {
                let _ = self.infer_expr(e, class, locals, in_instance)?;
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
                let expr_ty = self.infer_expr(expr, class, locals, in_instance)?;
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

    fn infer_expr(
        &self,
        expr: &Expr,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
    ) -> Result<Type, TypeError> {
        match expr {
            Expr::Int(_) => Ok(Type::Int),
            Expr::Float(_) => Ok(Type::Float),
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::String(_) => Ok(Type::String),
            Expr::InterpolatedString(parts) => {
                // Type check all expressions in the interpolation
                for part in parts {
                    if let InterpPart::Expr(expr) = part {
                        self.infer_expr(expr, class, locals, in_instance)?;
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
                let obj_ty = self.infer_expr(obj, class, locals, in_instance)?;
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
                if let Some(enum_info) = self.enums.get(class_name) {
                    let variant = enum_info.variants.iter().find(|v| v.name == *name);
                    match variant {
                        Some(v) if v.fields.is_empty() => {
                            return Ok(Type::Enum(class_name.clone()));
                        }
                        Some(_) => {
                            return Err(TypeError(format!(
                                "enum variant `{}.{}` requires arguments",
                                class_name, name
                            )));
                        }
                        None => {
                            return Err(TypeError(format!(
                                "unknown variant `{}.{}`",
                                class_name, name
                            )));
                        }
                    }
                }
                if !self.classes.contains_key(class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
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
                let lt = self.infer_expr(left, class, locals, in_instance)?;
                let rt = self.infer_expr(right, class, locals, in_instance)?;
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
                let ty = self.infer_expr(operand, class, locals, in_instance)?;
                match op {
                    UnaryOp::Neg => {
                        if !self.is_numeric(&ty) {
                            return Err(TypeError(format!("cannot negate {}", ty.name())));
                        }
                        Ok(ty)
                    }
                    UnaryOp::Not => {
                        if ty != Type::Bool {
                            return Err(TypeError("`!` requires bool".to_string()));
                        }
                        Ok(Type::Bool)
                    }
                }
            }
            Expr::Call(call) => self.check_call(call, class, locals, in_instance),
            Expr::New(class_name, type_args) => {
                if self.classes.contains_key(class_name) {
                    let class_info = self.classes.get(class_name).unwrap();
                    if class_info.is_abstract || class_info.is_interface {
                        return Err(TypeError(format!(
                            "cannot instantiate {} `{}`",
                            if class_info.is_interface { "interface" } else { "abstract class" },
                            class_name
                        )));
                    }
                    // For now, just return the class type without validating type args
                    // In a full implementation, we'd check that type_args match the class's generic params
                    Ok(Type::Class(class_name.clone(), type_args.clone()))
                } else {
                    Err(TypeError(format!("unknown class `{}`", class_name)))
                }
            }
            Expr::Ternary(cond, then_expr, else_expr) => {
                let cond_ty = self.infer_expr(cond, class, locals, in_instance)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError(format!(
                        "ternary condition must be bool, got {}",
                        cond_ty.name()
                    )));
                }
                let then_ty = self.infer_expr(then_expr, class, locals, in_instance)?;
                let else_ty = self.infer_expr(else_expr, class, locals, in_instance)?;
                if !self.is_assignable(&then_ty, &else_ty) {
                    return Err(TypeError(format!(
                        "ternary branches must have compatible types, got {} and {}",
                        then_ty.name(),
                        else_ty.name()
                    )));
                }
                Ok(then_ty)
            }
            Expr::Match(subject, arms) => {
                let subject_ty = self.infer_expr(subject, class, locals, in_instance)?;
                
                let mut result_ty: Option<Type> = None;
                
                for arm in arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            Pattern::EnumVariant(enum_name, variant_name, bindings) => {
                                let enum_info = self.enums.get(enum_name).ok_or_else(|| {
                                    TypeError(format!("unknown enum `{}`", enum_name))
                                })?;
                                let variant = enum_info.variants.iter().find(|v| v.name == *variant_name)
                                    .ok_or_else(|| TypeError(format!(
                                        "unknown variant `{}.{}`", enum_name, variant_name
                                    )))?;
                                if bindings.len() != variant.fields.len() {
                                    return Err(TypeError(format!(
                                        "variant `{}.{}` has {} fields but pattern has {} bindings",
                                        enum_name, variant_name, variant.fields.len(), bindings.len()
                                    )));
                                }
                                if let Type::Enum(ref sn) = subject_ty {
                                    if sn != enum_name {
                                        return Err(TypeError(format!(
                                            "cannot match enum `{}` against value of type `{}`",
                                            enum_name, subject_ty.name()
                                        )));
                                    }
                                }
                            }
                            Pattern::Range(start, end, _inclusive) => {
                                // Range patterns only work with numeric types
                                if subject_ty != Type::Int && subject_ty != Type::Float {
                                    return Err(TypeError(format!(
                                        "range pattern requires int or float subject, got {}",
                                        subject_ty.name()
                                    )));
                                }
                                let start_ty = self.infer_expr(start, class, locals, in_instance)?;
                                let end_ty = self.infer_expr(end, class, locals, in_instance)?;
                                if start_ty != subject_ty {
                                    return Err(TypeError(format!(
                                        "range start type {} doesn't match subject type {}",
                                        start_ty.name(), subject_ty.name()
                                    )));
                                }
                                if end_ty != subject_ty {
                                    return Err(TypeError(format!(
                                        "range end type {} doesn't match subject type {}",
                                        end_ty.name(), subject_ty.name()
                                    )));
                                }
                            }
                            _ => {}
                        }
                    }
                    
                    // Type check guard
                    if let Some(guard) = &arm.guard {
                        let guard_ty = self.infer_expr(guard, class, locals, in_instance)?;
                        if guard_ty != Type::Bool {
                            return Err(TypeError(format!(
                                "match guard must be bool, got {}",
                                guard_ty.name()
                            )));
                        }
                    }
                    
                    // Type check body
                    let body_ty = self.infer_expr(&arm.body, class, locals, in_instance)?;
                    
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
                for (arg, (_, expected_ty)) in args.iter().zip(variant.fields.iter()) {
                    let arg_ty = self.infer_expr(arg, class, locals, in_instance)?;
                    if !self.is_assignable(expected_ty, &arg_ty) {
                        return Err(TypeError(format!(
                            "argument type mismatch in `{}.{}`: expected {}, got {}",
                            enum_name, variant_name, expected_ty.name(), arg_ty.name()
                        )));
                    }
                }
                Ok(Type::Enum(enum_name.clone()))
            }
            Expr::Tuple(elements) => {
                let mut types = Vec::new();
                for elem in elements {
                    types.push(self.infer_expr(elem, class, locals, in_instance)?);
                }
                Ok(Type::Tuple(types))
            }
            Expr::TupleIndex(tuple, idx) => {
                let tuple_ty = self.infer_expr(tuple, class, locals, in_instance)?;
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
                let start_ty = self.infer_expr(start, class, locals, in_instance)?;
                let end_ty = self.infer_expr(end, class, locals, in_instance)?;
                if start_ty != Type::Int {
                    return Err(TypeError(format!(
                        "range start must be an integer, got {}",
                        start_ty.name()
                    )));
                }
                if end_ty != Type::Int {
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
                    let arg_ty = self.infer_expr(arg, class, locals, in_instance)?;
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
    ) -> Result<Type, TypeError> {
        match target {
            AssignTarget::Local(name) => locals
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError(format!("unknown variable `{}`", name))),
            AssignTarget::Field(obj, name) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance)?;
                let class_name = if let Type::Class(name, _) = &obj_ty {
                    name.clone()
                } else if in_instance && matches!(obj.as_ref(), Expr::Var(n) if n == "this") {
                    class.name.clone()
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
                Ok(ty)
            }
            AssignTarget::StaticField(class_name, name) => {
                if !self.classes.contains_key(class_name) {
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
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

    fn check_call(
        &self,
        call: &CallExpr,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
    ) -> Result<Type, TypeError> {
        if call.class_or_target == "__intrinsics" {
            if call.method == "print" {
                if call.args.len() != 1 {
                    return Err(TypeError("print expects 1 argument".to_string()));
                }
                let _ = self.infer_expr(&call.args[0], class, locals, in_instance)?;
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
                    return self.check_call_args(call, &method_info, class, locals, in_instance);
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
                    return self.check_call_args(call, &method_info, class, locals, in_instance);
                }
            }
            let target_ty = self.infer_expr(target, class, locals, in_instance)?;
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
            return self.check_call_args_with_subst(call, &method_info, &subst, class, locals, in_instance);
        } else {
            // static call: ClassName.Method(args) or EnumName.Variant(args)
            let class_name = call.class_or_target.clone();
            
            if let Some(enum_info) = self.enums.get(&class_name) {
                let variant = enum_info.variants.iter().find(|v| v.name == call.method);
                match variant {
                    Some(v) => {
                        if call.args.len() != v.fields.len() {
                            return Err(TypeError(format!(
                                "variant `{}.{}` expects {} arguments, got {}",
                                class_name, call.method, v.fields.len(), call.args.len()
                            )));
                        }
                        for (arg, (_, expected_ty)) in call.args.iter().zip(v.fields.iter()) {
                            let arg_ty = self.infer_expr(arg, class, locals, in_instance)?;
                            if !self.is_assignable(expected_ty, &arg_ty) {
                                return Err(TypeError(format!(
                                    "argument type mismatch in `{}.{}`: expected {}, got {}",
                                    class_name, call.method, expected_ty.name(), arg_ty.name()
                                )));
                            }
                        }
                        return Ok(Type::Enum(class_name));
                    }
                    None => {
                        return Err(TypeError(format!(
                            "unknown variant `{}.{}`",
                            class_name, call.method
                        )));
                    }
                }
            }
            
            if !self.classes.contains_key(&class_name) {
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
            return self.check_call_args(call, &method_info, class, locals, in_instance);
        }
    }

    fn check_call_args(
        &self,
        call: &CallExpr,
        method_info: &MethodInfo,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
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
            let arg_ty = self.infer_expr(arg, class, locals, in_instance)?;
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
            let arg_ty = self.infer_expr(arg, class, locals, in_instance)?;
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
        match (a, b) {
            (Type::Int, Type::Int) => Ok(Type::Int),
            (Type::Float, Type::Float) => Ok(Type::Float),
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
            _ => Err(TypeError(format!(
                "cannot operate on {} and {}",
                a.name(),
                b.name()
            ))),
        }
    }

    fn is_numeric(&self, ty: &Type) -> bool {
        matches!(ty, Type::Int | Type::Float)
    }

    fn is_assignable(&self, target: &Type, source: &Type) -> bool {
        if target == source {
            return true;
        }
        if self.is_numeric(target) && self.is_numeric(source) {
            return true;
        }
        match (target, source) {
            (Type::Class(target_name, _), Type::Class(source_name, _)) => {
                // Null is assignable to any class reference.
                if source_name == "null" {
                    return true;
                }
                // Upcast: a subclass instance is assignable to its base type.
                self.is_subclass_of(source_name, target_name)
            }
            (Type::Enum(a), Type::Enum(b)) => a == b,
            (Type::Class(name, _), Type::Enum(enum_name)) => name == enum_name,
            (Type::Enum(enum_name), Type::Class(name, _)) => name == enum_name,
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
