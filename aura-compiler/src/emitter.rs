//! Bytecode emitter.
//!
//! Translates the typed AST into an Aura [`Module`].

use crate::ast::*;
use crate::typer::{build_subst, promote, substitute_type, ClassInfo, TypedProgram};
use aura_bytecode::{ClassDef, ClassId, EnumDef, EnumId, ExceptionHandler, FieldDef, MethodDef, MethodId, Module, Op, TypeDesc, VariantDef};
use std::collections::HashMap;

/// Emitter state.
pub struct Emitter {
    module_name: String,
    next_class_id: u32,
    next_method_id: u32,
    next_enum_id: u32,
}

/// Full instance field layout for a class (base fields first, then own).
type FieldLayout = HashMap<String, Vec<(String, Type)>>;

/// Local builder for a method body.
struct MethodEmitter<'a> {
    class_id: ClassId,
    class_name: &'a str,
    method: &'a MethodDecl,
    class_info: &'a ClassInfo,
    program: &'a TypedProgram,
    class_ids: &'a HashMap<String, ClassId>,
    enum_ids: &'a HashMap<String, EnumId>,
    variant_lookup: &'a HashMap<String, (EnumId, u16)>,
    field_layout: &'a HashMap<String, Vec<(String, Type)>>,
    ops: Vec<Op>,
    locals: Vec<String>,
    max_locals: usize,
    params: Vec<String>,
    local_types: HashMap<String, Type>,
    constants: Vec<String>,
    method_ids: &'a HashMap<(String, String, bool), MethodId>,
    constructor_ids: &'a HashMap<String, Vec<MethodId>>,
    break_targets: Vec<(Option<String>, Vec<usize>)>,
    continue_targets: Vec<(Option<String>, Vec<usize>)>,
    handlers: Vec<ExceptionHandler>,
}

impl Emitter {
    /// Create an emitter for the given module name.
    pub fn new(module_name: &str) -> Self {
        Self {
            module_name: module_name.to_string(),
            next_class_id: 0,
            next_method_id: 0,
            next_enum_id: 0,
        }
    }

    /// Emit a module from a typed program.
    pub fn emit(mut self, program: &TypedProgram) -> Result<Module, String> {
        let mut module = Module {
            name: self.module_name.clone(),
            classes: HashMap::new(),
            enums: HashMap::new(),
            entrypoint: None,
            constant_pool: Vec::new(),
        };

        // First pass: assign ids to classes. The stdlib intrinsic classes get
        // ids and metadata-only `ClassDef`s (no methods, no fields) so type
        // descriptors naming them resolve; calls and construction lower to
        // `Op::NativeCall`, never `NewObj`/`CallVirt`, so the empty defs are
        // never dispatched through.
        let mut class_ids: HashMap<String, ClassId> = HashMap::new();
        for ic in crate::intrinsics::classes() {
            let id = ClassId(self.next_class_id);
            self.next_class_id += 1;
            class_ids.insert(ic.name.to_string(), id);
            module.classes.insert(
                id,
                ClassDef {
                    name: ic.name.to_string(),
                    generic_params: ic
                        .generic_params
                        .iter()
                        .map(|n| aura_bytecode::GenericParam {
                            name: n.to_string(),
                            constraint: None,
                            variance: aura_bytecode::Variance::Invariant,
                        })
                        .collect(),
                    super_class: None,
                    interfaces: Vec::new(),
                    is_interface: false,
                    is_abstract: ic.constructor.is_none(),
                    is_record: false,
                    fields: Vec::new(),
                    static_fields: Vec::new(),
                    methods: HashMap::new(),
                    static_methods: HashMap::new(),
                },
            );
        }
        for decl in &program.program.decls {
            if let Decl::Class(c) = decl {
                let id = ClassId(self.next_class_id);
                self.next_class_id += 1;
                class_ids.insert(c.name.clone(), id);
            }
        }

        // Assign ids to enums.
        let mut enum_ids: HashMap<String, EnumId> = HashMap::new();
        for decl in &program.program.decls {
            if let Decl::Enum(e) = decl {
                let id = EnumId(self.next_enum_id);
                self.next_enum_id += 1;
                enum_ids.insert(e.name.clone(), id);
            }
        }

        // Map each sum-type/enum variant name to its enum id and index so bare
        // constructors (`Ok(5)`) and bare patterns (`Ok(v)`) can be emitted.
        let mut variant_lookup: HashMap<String, (EnumId, u16)> = HashMap::new();
        for decl in &program.program.decls {
            if let Decl::Enum(e) = decl {
                let id = enum_ids[&e.name];
                for (idx, v) in e.variants.iter().enumerate() {
                    variant_lookup.insert(v.name.clone(), (id, idx as u16));
                }
            }
        }

        // Second pass: assign ids to methods.
        let mut method_ids: HashMap<(String, String, bool), MethodId> = HashMap::new();
        let mut constructor_ids: HashMap<String, Vec<MethodId>> = HashMap::new();
        for decl in &program.program.decls {
            if let Decl::Class(class) = decl {
                for member in &class.members {
                    match member {
                        Member::Method(m) => {
                            let id = MethodId(self.next_method_id);
                            self.next_method_id += 1;
                            method_ids.insert((class.name.clone(), m.name.clone(), !m.is_static), id);
                            if m.is_constructor {
                                constructor_ids
                                    .entry(class.name.clone())
                                    .or_default()
                                    .push(id);
                            }
                        }
                        Member::Property(p) => {
                            if p.getter.is_some() {
                                let id = MethodId(self.next_method_id);
                                self.next_method_id += 1;
                                method_ids.insert(
                                    (class.name.clone(), format!("get_{}", p.name), !p.is_static),
                                    id,
                                );
                            }
                            if p.setter.is_some() {
                                let id = MethodId(self.next_method_id);
                                self.next_method_id += 1;
                                method_ids.insert(
                                    (class.name.clone(), format!("set_{}", p.name), !p.is_static),
                                    id,
                                );
                            }
                        }
                        Member::Field(_) => {}
                    }
                }
                // The record primary constructor is synthesized by the type
                // checker (last entry in `info.constructors`); give it an id so
                // `new` and `: this(...)` resolution line up with the checker.
                if class.is_record {
                    let id = MethodId(self.next_method_id);
                    self.next_method_id += 1;
                    constructor_ids.entry(class.name.clone()).or_default().push(id);
                }
                // Plain classes that declare no constructors get an implicit
                // zero-parameter default constructor synthesized by the type
                // checker; give it an id as well.
                if !class.is_record
                    && !class.members.iter().any(|m| matches!(m, Member::Method(m) if m.is_constructor))
                {
                    let id = MethodId(self.next_method_id);
                    self.next_method_id += 1;
                    constructor_ids.entry(class.name.clone()).or_default().push(id);
                }
            }
        }

        // Emit enum definitions.
        for decl in &program.program.decls {
            if let Decl::Enum(e) = decl {
                let enum_id = *enum_ids.get(&e.name).unwrap();
                let variants: Vec<VariantDef> = e.variants.iter().map(|v| {
                    VariantDef {
                        name: v.name.clone(),
                        fields: v.fields.iter().map(|f| {
                            FieldDef {
                                name: f.name.clone(),
                                ty: map_type(&f.ty, &class_ids, &enum_ids, &[]),
                            }
                        }).collect(),
                    }
                }).collect();
                module.enums.insert(enum_id, EnumDef {
                    name: e.name.clone(),
                    variants,
                });
            }
        }

        // Third pass: emit class defs and methods.
        let mut field_layouts: FieldLayout = HashMap::new();
        for decl in &program.program.decls {
            if let Decl::Class(c) = decl {
                build_field_layout(program, &c.name, &mut field_layouts);
            }
        }

        let mut entrypoint: Option<MethodId> = None;
        for decl in &program.program.decls {
            if let Decl::Class(class) = decl {
                let class_id = *class_ids.get(&class.name).unwrap();
                let info = program.classes.get(&class.name).unwrap();

                let class_generic_params: Vec<aura_bytecode::GenericParam> = class.generic_params.iter().map(|gp| {
                    aura_bytecode::GenericParam {
                        name: gp.name.clone(),
                        constraint: gp.constraint.as_ref().map(|c| map_type(c, &class_ids, &enum_ids, &[])),
                        variance: aura_bytecode::Variance::Invariant,
                    }
                }).collect();
                let fields: Vec<FieldDef> = field_layouts[&class.name]
                    .iter()
                    .map(|(name, ty)| FieldDef {
                        name: name.clone(),
                        ty: map_type(ty, &class_ids, &enum_ids, &class_generic_params),
                    })
                    .collect();
                let static_fields: Vec<FieldDef> = info
                    .static_fields
                    .iter()
                    .map(|(name, ty)| FieldDef {
                        name: name.clone(),
                        ty: map_type(ty, &class_ids, &enum_ids, &class_generic_params),
                    })
                    .collect();

                let mut methods = HashMap::new();
                let mut static_methods = HashMap::new();
                let mut ctor_cursor = 0usize;

                for member in &class.members {
                    match member {
                        Member::Method(m) => {
                            let method_id = if m.is_constructor {
                                let id = constructor_ids
                                    .get(&class.name)
                                    .and_then(|ids| ids.get(ctor_cursor))
                                    .copied()
                                    .ok_or_else(|| {
                                        format!("missing constructor id for `{}`", class.name)
                                    })?;
                                ctor_cursor += 1;
                                id
                            } else {
                                *method_ids
                                    .get(&(class.name.clone(), m.name.clone(), !m.is_static))
                                    .unwrap()
                            };

                            if class.name == "Program" && m.name == "Main" && m.is_static {
                                entrypoint = Some(method_id);
                            }

                            let (method_id, method_def) = self.build_method_def(
                                program, class, m, info, class_id, method_id, &method_ids,
                                &class_ids, &enum_ids, &variant_lookup, &field_layouts,
                                &class_generic_params, &constructor_ids, &mut module,
                            )?;

                            if m.is_static {
                                static_methods.insert(method_id, method_def);
                            } else {
                                methods.insert(method_id, method_def);
                            }
                        }
                        Member::Property(p) => {
                            let mut emitted: Vec<(bool, (MethodId, MethodDef))> = Vec::new();
                            if p.getter.is_some() {
                                let accessor_id = *method_ids
                                    .get(&(class.name.clone(), format!("get_{}", p.name), !p.is_static))
                                    .unwrap();
                                emitted.push((
                                    p.is_static,
                                    self.build_method_def(
                                        program, class, &property_accessor_decl(class.name.clone(), p, true),
                                        info, class_id, accessor_id, &method_ids, &class_ids, &enum_ids,
                                        &variant_lookup, &field_layouts, &class_generic_params, &constructor_ids, &mut module,
                                    )?,
                                ));
                            }
                            if p.setter.is_some() {
                                let accessor_id = *method_ids
                                    .get(&(class.name.clone(), format!("set_{}", p.name), !p.is_static))
                                    .unwrap();
                                emitted.push((
                                    p.is_static,
                                    self.build_method_def(
                                        program, class, &property_accessor_decl(class.name.clone(), p, false),
                                        info, class_id, accessor_id, &method_ids, &class_ids, &enum_ids,
                                        &variant_lookup, &field_layouts, &class_generic_params, &constructor_ids, &mut module,
                                    )?,
                                ));
                            }
                            for (is_static, (method_id, method_def)) in emitted {
                                if is_static {
                                    static_methods.insert(method_id, method_def);
                                } else {
                                    methods.insert(method_id, method_def);
                                }
                            }
                        }
                        Member::Field(_) => {}
                    }
                }

                // Emit the synthesized record primary constructor: it assigns
                // each record parameter to its backing instance field.
                if class.is_record {
                    let primary = record_primary_constructor(class);
                    let primary_id = constructor_ids
                        .get(&class.name)
                        .and_then(|ids| ids.get(info.constructors.len() - 1))
                        .copied()
                        .ok_or_else(|| {
                            format!("missing primary constructor id for record `{}`", class.name)
                        })?;
                    let (_, method_def) = self.build_method_def(
                        program, class, &primary, info, class_id, primary_id, &method_ids,
                        &class_ids, &enum_ids, &variant_lookup, &field_layouts,
                        &class_generic_params, &constructor_ids, &mut module,
                    )?;
                    methods.insert(primary_id, method_def);
                }
                // Emit the synthesized implicit default constructor for classes
                // that declare none: an empty body (which still chains to the
                // base class's zero-parameter constructor when present).
                if !class.is_record
                    && !class.members.iter().any(|m| matches!(m, Member::Method(m) if m.is_constructor))
                {
                    let default = record_primary_constructor(class);
                    let default_id = constructor_ids
                        .get(&class.name)
                        .and_then(|ids| ids.first())
                        .copied()
                        .ok_or_else(|| {
                            format!("missing default constructor id for `{}`", class.name)
                        })?;
                    let (_, method_def) = self.build_method_def(
                        program, class, &default, info, class_id, default_id, &method_ids,
                        &class_ids, &enum_ids, &variant_lookup, &field_layouts,
                        &class_generic_params, &constructor_ids, &mut module,
                    )?;
                    methods.insert(default_id, method_def);
                }

                module.classes.insert(
                    class_id,
                    ClassDef {
                        name: class.name.clone(),
                        generic_params: class_generic_params,
                        super_class: info.super_class.as_ref().map(|n| *class_ids.get(n).unwrap()),
                        interfaces: info.interfaces.iter().map(|n| *class_ids.get(n).unwrap()).collect(),
                        is_interface: info.is_interface,
                        is_abstract: info.is_abstract,
                        is_record: info.is_record,
                        fields,
                        static_fields,
                        methods,
                        static_methods,
                    },
                );
            }
        }

        // Record structural (duck-typed) interface implementations into each
        // class's ClassDef.interfaces so runtime type tests (catch matching,
        // instance-of walks) and interface default-method resolution agree
        // with the type checker's structural assignability.
        for decl in &program.program.decls {
            let Decl::Class(c) = decl else { continue };
            let Some(info) = program.classes.get(&c.name) else { continue };
            if info.is_interface {
                continue;
            }
            let class_id = class_ids[&c.name];
            for other in &program.program.decls {
                let Decl::Class(i) = other else { continue };
                let Some(iface_info) = program.classes.get(&i.name) else { continue };
                if !iface_info.is_interface {
                    continue;
                }
                let iface_id = class_ids[&i.name];
                let def = module.classes.get_mut(&class_id).unwrap();
                if !def.interfaces.contains(&iface_id)
                    && crate::typer::structurally_satisfies(&program.classes, &c.name, &i.name)
                {
                    def.interfaces.push(iface_id);
                }
            }
        }

        module.entrypoint = entrypoint;
        Ok(module)
    }

    /// Emit a single method into a `MethodDef`, adding any string/float constants
    /// to the module constant pool.
    #[allow(clippy::too_many_arguments)]
    fn build_method_def(
        &self,
        program: &TypedProgram,
        class: &ClassDecl,
        m: &MethodDecl,
        info: &ClassInfo,
        class_id: ClassId,
        method_id: MethodId,
        method_ids: &HashMap<(String, String, bool), MethodId>,
        class_ids: &HashMap<String, ClassId>,
        enum_ids: &HashMap<String, EnumId>,
        variant_lookup: &HashMap<String, (EnumId, u16)>,
        field_layouts: &FieldLayout,
        class_generic_params: &[aura_bytecode::GenericParam],
        constructor_ids: &HashMap<String, Vec<MethodId>>,
        module: &mut Module,
    ) -> Result<(MethodId, MethodDef), String> {
        let mut me = MethodEmitter::new(
            class_id,
            &class.name,
            m,
            info,
            program,
            class_ids,
            enum_ids,
            variant_lookup,
            field_layouts,
            method_ids,
            constructor_ids,
        );
        me.emit_body()?;

        let mut method_def = MethodDef {
            name: m.name.clone(),
            return_ty: map_type(&m.return_ty, class_ids, enum_ids, class_generic_params),
            params: m.params.iter().map(|p| map_type(&p.ty, class_ids, enum_ids, class_generic_params)).collect(),
            generic_params: m.generic_params.iter().map(|gp| {
                aura_bytecode::GenericParam {
                    name: gp.name.clone(),
                    constraint: gp.constraint.as_ref().map(|c| map_type(c, class_ids, enum_ids, class_generic_params)),
                    variance: aura_bytecode::Variance::Invariant,
                }
            }).collect(),
            is_instance: !m.is_static,
            body: me.ops,
            handlers: me.handlers,
            max_stack: 8,
            locals: me.max_locals as u16,
        };

        let constant_offset = module.constant_pool.len() as u32;
        if !me.constants.is_empty() {
            module.constant_pool.extend(me.constants);
            for op in &mut method_def.body {
                if let Op::LdStr(idx) | Op::LdFloat(idx) = op {
                    *idx += constant_offset;
                }
            }
        }
        Ok((method_id, method_def))
    }
}

/// Build the synthetic `MethodDecl` for a property accessor.
///
/// Auto accessors read/write the synthetic backing field `__prop_{name}`;
/// explicit accessor bodies are emitted as-is.
fn property_accessor_decl(class_name: String, p: &PropertyDecl, is_getter: bool) -> MethodDecl {
    let accessor = if is_getter { &p.getter } else { &p.setter };
    let body = match accessor {
        None => Vec::new(),
        Some(Accessor { kind: AccessorKind::Body(body), .. }) => body.clone(),
        Some(Accessor { kind: AccessorKind::Auto, .. }) => {
            let backing = format!("__prop_{}", p.name);
            if is_getter {
                let expr = if p.is_static {
                    Expr::StaticField(class_name.clone(), backing)
                } else {
                    Expr::Field(Box::new(Expr::Var("this".to_string())), backing)
                };
                vec![Stmt::Return(Some(expr))]
            } else {
                let target = if p.is_static {
                    AssignTarget::StaticField(class_name.clone(), backing)
                } else {
                    AssignTarget::Field(Box::new(Expr::Var("this".to_string())), backing)
                };
                vec![Stmt::Assign(target, Expr::Var("value".to_string()))]
            }
        }
    };
    MethodDecl {
        is_static: p.is_static,
        visibility: p.visibility,
        is_virtual: false,
        is_override: false,
        is_abstract: false,
        is_final: false,
        is_constructor: false,
        constructor_chain: None,
        generic_params: Vec::new(),
        return_ty: if is_getter { p.ty.clone() } else { Type::Unit },
        name: format!("{}_{}", if is_getter { "get" } else { "set" }, p.name),
        params: if is_getter {
            Vec::new()
        } else {
            vec![Param { ty: p.ty.clone(), name: "value".to_string() }]
        },
        body,
    }
}

/// Build the synthetic `MethodDecl` for a record's primary constructor: it
/// assigns each record parameter to the instance field with the same name.
fn record_primary_constructor(class: &ClassDecl) -> MethodDecl {
    let body: Vec<Stmt> = class
        .record_params
        .iter()
        .map(|p| {
            Stmt::Assign(
                AssignTarget::Field(Box::new(Expr::Var("this".to_string())), p.name.clone()),
                Expr::Var(p.name.clone()),
            )
        })
        .collect();
    MethodDecl {
        is_static: false,
        visibility: Visibility::Public,
        is_virtual: false,
        is_override: false,
        is_abstract: false,
        is_final: false,
        is_constructor: true,
        constructor_chain: None,
        generic_params: Vec::new(),
        return_ty: Type::Unit,
        name: class.name.clone(),
        params: class.record_params.clone(),
        body,
    }
}

/// Build the full instance field layout for a class: base fields first, then own.
fn build_field_layout(
    program: &TypedProgram,
    class_name: &str,
    cache: &mut FieldLayout,
) -> Vec<(String, Type)> {
    if let Some(layout) = cache.get(class_name) {
        return layout.clone();
    }
    let info = program.classes.get(class_name).unwrap();
    let mut layout = Vec::new();
    if let Some(super_name) = &info.super_class {
        layout.extend(build_field_layout(program, super_name, cache));
    }
    layout.extend(info.instance_fields.clone());
    cache.insert(class_name.to_string(), layout.clone());
    layout
}

impl<'a> MethodEmitter<'a> {
    fn new(
        class_id: ClassId,
        class_name: &'a str,
        method: &'a MethodDecl,
        class_info: &'a ClassInfo,
        program: &'a TypedProgram,
        class_ids: &'a HashMap<String, ClassId>,
        enum_ids: &'a HashMap<String, EnumId>,
        variant_lookup: &'a HashMap<String, (EnumId, u16)>,
        field_layout: &'a HashMap<String, Vec<(String, Type)>>,
        method_ids: &'a HashMap<(String, String, bool), MethodId>,
        constructor_ids: &'a HashMap<String, Vec<MethodId>>,
    ) -> Self {
        let params: Vec<String> = method.params.iter().map(|p| p.name.clone()).collect();
        let mut locals = Vec::new();
        let mut local_types = HashMap::new();
        if !method.is_static {
            locals.push("this".to_string());
            local_types.insert(
                "this".to_string(),
                Type::Class(class_name.to_string(), Vec::new()),
            );
        }
        for p in &method.params {
            locals.push(p.name.clone());
            local_types.insert(p.name.clone(), p.ty.clone());
        }
        let init_locals = locals.len();
        Self {
            class_id,
            class_name,
            method,
            class_info,
            program,
            class_ids,
            enum_ids,
            variant_lookup,
            field_layout,
            ops: Vec::new(),
            locals,
            max_locals: init_locals,
            params,
            local_types,
            constants: Vec::new(),
            method_ids,
            constructor_ids,
            break_targets: Vec::new(),
            continue_targets: Vec::new(),
            handlers: Vec::new(),
        }
    }

    fn emit_body(&mut self) -> Result<(), String> {
        if self.method.is_constructor {
            match &self.method.constructor_chain {
                Some(chain) => {
                    let target_id = self.resolve_constructor_id_for_chain(chain)?;
                    for arg in &chain.args {
                        self.emit_expr(arg)?;
                    }
                    self.ops.push(Op::Ldloc(0));
                    self.ops.push(Op::CallSuper(target_id));
                }
                None => {
                    // Implicitly invoke the base class's zero-parameter
                    // constructor when the constructor has no explicit chain.
                    if let Some(super_name) = &self.class_info.super_class {
                        if let (Some(ids), Some(info)) = (
                            self.constructor_ids.get(super_name),
                            self.program.classes.get(super_name),
                        ) {
                            if let Some(idx) = info.constructors.iter().position(|c| c.params.is_empty()) {
                                let target_id = ids[idx];
                                self.ops.push(Op::Ldloc(0));
                                self.ops.push(Op::CallSuper(target_id));
                            }
                        }
                    }
                }
            }
        }
        for stmt in &self.method.body {
            self.emit_stmt(stmt)?;
        }
        if self.ops.last() != Some(&Op::Ret) {
            self.ops.push(Op::LdNull);
            self.ops.push(Op::Ret);
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::VarDecl(ty, name, init) => {
                if let Some(init) = init {
                    self.emit_expr(init)?;
                } else {
                    self.ops.push(Op::LdNull);
                }
                // `var` declarations record the initializer's inferred type
                // (the typer has already validated it exists and resolves).
                let ty = if matches!(ty, Type::Infer) {
                    let init = init.as_ref().ok_or("`var` requires an initializer")?;
                    self.expr_ty(init)?
                } else {
                    ty.clone()
                };
                self.push_local(name.clone());
                self.local_types.insert(name.clone(), ty);
                self.ops.push(Op::Stloc((self.locals.len() - 1) as u16));
            }
            Stmt::TupleDecl(names, expr) => {
                self.emit_expr(expr)?;
                let tuple_local = self.locals.len() as u16;
                self.push_local("__tuple_temp".to_string());
                self.ops.push(Op::Stloc(tuple_local));
                for (i, name) in names.iter().enumerate() {
                    self.ops.push(Op::Ldloc(tuple_local));
                    self.ops.push(Op::TupleField(i as u16));
                    self.push_local(name.clone());
                    self.ops.push(Op::Stloc((self.locals.len() - 1) as u16));
                }
            }
            Stmt::Expr(e) => {
                self.emit_expr(e)?;
                // Intrinsics print/println leave nothing on the stack; avoid double pop.
                let needs_pop = !matches!(
                    e,
                    Expr::Call(crate::ast::CallExpr {
                        class_or_target,
                        ..
                    }) if class_or_target == "__intrinsics"
                );
                if needs_pop {
                    self.ops.push(Op::Pop);
                }
            }
            Stmt::Assign(target, value) => {
                self.emit_expr(value)?;
                match target {
                    AssignTarget::Local(name) => {
                        let idx = self.local_index(name)?;
                        self.ops.push(Op::Stloc(idx));
                    }
            AssignTarget::Field(obj, name) => {
                let obj_class = self.expr_class(obj);
                if let Some(obj_class) = &obj_class {
                    if let Some((_declaring, _)) = self.instance_property_opt(obj_class, name) {
                        if let Expr::Var(n) = obj.as_ref() {
                            if n == "this" {
                                self.ops.push(Op::Ldloc(0));
                            } else {
                                self.emit_expr(obj)?;
                            }
                        } else {
                            self.emit_expr(obj)?;
                        }
                        self.ops.push(Op::CallVirt(format!("set_{}", name)));
                        return Ok(());
                    }
                }
                if let Expr::Var(n) = obj.as_ref() {
                    if n == "this" {
                        self.ops.push(Op::Ldloc(0));
                    } else {
                        self.emit_expr(obj)?;
                    }
                } else {
                    self.emit_expr(obj)?;
                }
                let obj_class = self.expr_class(obj).ok_or_else(|| {
                    format!("cannot determine type of field target `{}`", name)
                })?;
                let idx = self.field_index_for(&obj_class, name)?;
                self.ops.push(Op::Stfld(idx));
            }
                    AssignTarget::StaticField(class_name, name) => {
                        if let Some((declaring, _)) = self.static_property_opt(class_name, name) {
                            let method_id = *self.method_ids.get(&(
                                declaring.clone(),
                                format!("set_{}", name),
                                false,
                            )).ok_or_else(|| {
                                format!("unknown setter for property `{}` on `{}`", name, declaring)
                            })?;
                            self.ops.push(Op::Call(method_id));
                        } else {
                            let (declaring, idx) = self.static_field_index(class_name, name)?;
                            let class_id = *self.class_ids.get(&declaring).unwrap();
                            self.ops.push(Op::Stsfld(class_id, idx));
                        }
                    }
                    AssignTarget::SuperField(name) => {
                        if let Some((declaring, _)) = self.super_instance_property(name) {
                            let method_id = *self.method_ids.get(&(
                                declaring.clone(),
                                format!("set_{}", name),
                                true,
                            )).ok_or_else(|| {
                                format!("unknown setter for property `{}` on `{}`", name, declaring)
                            })?;
                            self.ops.push(Op::Ldloc(0));
                            self.ops.push(Op::CallSuper(method_id));
                        } else {
                            self.ops.push(Op::Ldloc(0));
                            let idx = self.super_field_index(name)?;
                            self.ops.push(Op::Stfld(idx));
                        }
                    }
                }
            }
            Stmt::Return(Some(e)) => {
                self.emit_expr(e)?;
                self.ops.push(Op::Ret);
            }
            Stmt::Return(None) => {
                self.ops.push(Op::LdNull);
                self.ops.push(Op::Ret);
            }
            Stmt::If(cond, then_branch, else_branch) => {
                self.emit_expr(cond)?;
                let false_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));

                for s in then_branch {
                    self.emit_stmt(s)?;
                }

                if let Some(else_branch) = else_branch {
                    let end_jump = self.ops.len();
                    self.ops.push(Op::Br(0));
                    let else_start = self.ops.len() as u32;
                    for s in else_branch {
                        self.emit_stmt(s)?;
                    }
                    let end = self.ops.len() as u32;
                    self.ops[false_jump] = Op::BrFalse(else_start);
                    self.ops[end_jump] = Op::Br(end);
                } else {
                    let end = self.ops.len() as u32;
                    self.ops[false_jump] = Op::BrFalse(end);
                }
            }
            Stmt::IfLet(pattern, expr, then_branch, else_branch) => {
                // Evaluate the subject into a temporary local.
                self.emit_expr(expr)?;
                let subject_local = self.locals.len() as u16;
                self.push_local("__iflet_subject".to_string());
                self.ops.push(Op::Stloc(subject_local));

                // Emit pattern test; failures jump over the then-branch.
                let mut fail_jumps: Vec<usize> = Vec::new();
                self.emit_pattern(pattern, subject_local, &mut fail_jumps)?;

                if fail_jumps.is_empty() {
                    // Pattern always matches (wildcard or binding).
                    for s in then_branch {
                        self.emit_stmt(s)?;
                    }
                } else {
                    // Emit then-branch (executed when the pattern matches).
                    for s in then_branch {
                        self.emit_stmt(s)?;
                    }

                    if let Some(else_branch) = else_branch {
                        let end_jump = self.ops.len();
                        self.ops.push(Op::Br(0));
                        let else_start = self.ops.len() as u32;
                        for s in else_branch {
                            self.emit_stmt(s)?;
                        }
                        for jump in &fail_jumps {
                            self.ops[*jump] = Op::BrFalse(else_start);
                        }
                        let end = self.ops.len() as u32;
                        self.ops[end_jump] = Op::Br(end);
                    } else {
                        let end = self.ops.len() as u32;
                        for jump in &fail_jumps {
                            self.ops[*jump] = Op::BrFalse(end);
                        }
                    }
                }
            }
            Stmt::While { label, condition, body } => {
                let loop_start = self.ops.len() as u32;
                self.continue_targets.push((label.clone(), Vec::new()));
                self.break_targets.push((label.clone(), Vec::new()));
                
                self.emit_expr(condition)?;
                let exit_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                for s in body {
                    self.emit_stmt(s)?;
                }
                self.ops.push(Op::Br(loop_start));
                let end = self.ops.len() as u32;
                self.ops[exit_jump] = Op::BrFalse(end);
                
                // Patch break jumps
                let (_, breaks) = self.break_targets.pop().unwrap();
                for jump in breaks {
                    self.ops[jump] = Op::Br(end);
                }
                // Patch continue jumps
                let (_, continues) = self.continue_targets.pop().unwrap();
                for jump in continues {
                    self.ops[jump] = Op::Br(loop_start);
                }
            }
            Stmt::For { label, init, condition, update, body } => {
                // Emit init
                self.emit_stmt(init)?;
                
                let loop_start = self.ops.len() as u32;
                self.break_targets.push((label.clone(), Vec::new()));
                self.continue_targets.push((label.clone(), Vec::new()));
                
                // Emit condition
                self.emit_expr(condition)?;
                let exit_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                
                // Emit body
                for s in body {
                    self.emit_stmt(s)?;
                }
                
                // Continue target is the update statement
                let update_start = self.ops.len() as u32;
                
                // Emit update
                self.emit_stmt(update)?;
                self.ops.push(Op::Br(loop_start));
                
                let end = self.ops.len() as u32;
                self.ops[exit_jump] = Op::BrFalse(end);
                
                // Patch break jumps
                let (_, breaks) = self.break_targets.pop().unwrap();
                for jump in breaks {
                    self.ops[jump] = Op::Br(end);
                }
                // Patch continue jumps
                let (_, continues) = self.continue_targets.pop().unwrap();
                for jump in continues {
                    self.ops[jump] = Op::Br(update_start);
                }
            }
            Stmt::ForIn { label, var_type, var_name, iterable, body } => {
                // Collection iteration desugars to the index loop below;
                // range iteration keeps its original lowering further down.
                if !matches!(iterable, Expr::Range(..)) {
                    return self.emit_for_in_collection(label, var_type, var_name, iterable, body);
                }
                // Desugar `for (int i in start..end)` to:
                // int i = start;
                // while (i < end) { body; i = i + 1; }
                // or for inclusive: while (i <= end)

                // Extract start and end from the range expression
                let (start, end, inclusive) = match iterable {
                    Expr::Range(start, end, inclusive) => (start, end, *inclusive),
                    _ => return Err("for-in requires a range expression".to_string()),
                };
                
                // Emit: int var_name = start;
                self.emit_expr(start)?;
                self.push_local(var_name.clone());
                let var_idx = (self.locals.len() - 1) as u16;
                self.ops.push(Op::Stloc(var_idx));
                
                let loop_start = self.ops.len() as u32;
                self.break_targets.push((label.clone(), Vec::new()));
                self.continue_targets.push((label.clone(), Vec::new()));
                
                // Emit: var_name < end (or <= for inclusive)
                self.ops.push(Op::Ldloc(var_idx));
                self.emit_expr(end)?;
                if inclusive {
                    self.ops.push(Op::Le);
                } else {
                    self.ops.push(Op::Lt);
                }
                let exit_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                
                // Emit body
                for s in body {
                    self.emit_stmt(s)?;
                }
                
                // Continue target is the increment
                let update_start = self.ops.len() as u32;
                
                // Emit: var_name = var_name + 1
                self.ops.push(Op::Ldloc(var_idx));
                self.ops.push(Op::LdInt(1));
                self.ops.push(Op::Add);
                self.ops.push(Op::Stloc(var_idx));
                
                self.ops.push(Op::Br(loop_start));
                
                let end_pos = self.ops.len() as u32;
                self.ops[exit_jump] = Op::BrFalse(end_pos);
                
                // Patch break jumps
                let (_, breaks) = self.break_targets.pop().unwrap();
                for jump in breaks {
                    self.ops[jump] = Op::Br(end_pos);
                }
                // Patch continue jumps
                let (_, continues) = self.continue_targets.pop().unwrap();
                for jump in continues {
                    self.ops[jump] = Op::Br(update_start);
                }
            }
            Stmt::DoWhile { label, body, condition } => {
                let loop_start = self.ops.len() as u32;
                self.break_targets.push((label.clone(), Vec::new()));
                self.continue_targets.push((label.clone(), Vec::new()));
                
                for s in body {
                    self.emit_stmt(s)?;
                }
                
                let continue_target = self.ops.len() as u32;
                
                self.emit_expr(condition)?;
                self.ops.push(Op::BrTrue(loop_start));
                
                let end = self.ops.len() as u32;
                
                // Patch break jumps
                let (_, breaks) = self.break_targets.pop().unwrap();
                for jump in breaks {
                    self.ops[jump] = Op::Br(end);
                }
                // Patch continue jumps
                let (_, continues) = self.continue_targets.pop().unwrap();
                for jump in continues {
                    self.ops[jump] = Op::Br(continue_target);
                }
            }
            Stmt::Break(label) => {
                if self.break_targets.is_empty() {
                    return Err("break outside of loop".to_string());
                }
                let jump = self.ops.len();
                self.ops.push(Op::Br(0)); // placeholder
                
                if let Some(target_label) = label {
                    // Find the loop with matching label
                    let mut found = false;
                    for (loop_label, jumps) in self.break_targets.iter_mut().rev() {
                        if loop_label.as_ref() == Some(&target_label) {
                            jumps.push(jump);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Err(format!("no loop with label `{}` found", target_label));
                    }
                } else {
                    // Unlabeled break - use innermost loop
                    self.break_targets.last_mut().unwrap().1.push(jump);
                }
            }
            Stmt::Continue(label) => {
                if self.continue_targets.is_empty() {
                    return Err("continue outside of loop".to_string());
                }
                let jump = self.ops.len();
                self.ops.push(Op::Br(0)); // placeholder
                
                if let Some(target_label) = label {
                    // Find the loop with matching label
                    let mut found = false;
                    for (loop_label, jumps) in self.continue_targets.iter_mut().rev() {
                        if loop_label.as_ref() == Some(&target_label) {
                            jumps.push(jump);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Err(format!("no loop with label `{}` found", target_label));
                    }
                } else {
                    // Unlabeled continue - use innermost loop
                    self.continue_targets.last_mut().unwrap().1.push(jump);
                }
            }
            Stmt::Block(stmts) => {
                for s in stmts {
                    self.emit_stmt(s)?;
                }
            }
            Stmt::Throw(e) => {
                self.emit_expr(e)?;
                self.ops.push(Op::Throw);
            }
            Stmt::Try {
                try_body,
                catches,
                finally_body,
            } => {
                let try_start = self.ops.len() as u32;
                for s in try_body {
                    self.emit_stmt(s)?;
                }
                let try_end = self.ops.len() as u32;
                let normal_jump = self.ops.len();
                self.ops.push(Op::Br(0));

                let mut catch_end_jumps = Vec::new();
                let mut handler_entries = Vec::new();
                for catch in catches {
                    let handler_pc = self.ops.len() as u32;
                    self.push_local(catch.name.clone());
                    let catch_local = (self.locals.len() - 1) as u16;
                    self.local_types
                        .insert(catch.name.clone(), catch.ty.clone());
                    handler_entries.push(ExceptionHandler {
                        start: try_start,
                        end: try_end,
                        catch_type: Some(map_type(
                            &catch.ty,
                            self.class_ids,
                            self.enum_ids,
                            &[],
                        )),
                        handler_pc,
                        catch_local,
                    });
                    for s in &catch.body {
                        self.emit_stmt(s)?;
                    }
                    let jump = self.ops.len();
                    self.ops.push(Op::Br(0));
                    catch_end_jumps.push(jump);
                }

                let after_catches = self.ops.len() as u32;
                let finally_entry = if let Some(finally_body) = finally_body {
                    handler_entries.push(ExceptionHandler {
                        start: try_start,
                        end: after_catches,
                        catch_type: None,
                        handler_pc: after_catches,
                        catch_local: 0,
                    });
                    for s in finally_body {
                        self.emit_stmt(s)?;
                    }
                    self.ops.push(Op::EndFinally);
                    after_catches
                } else {
                    after_catches
                };

                self.ops[normal_jump] = Op::Br(finally_entry);
                for jump in catch_end_jumps {
                    self.ops[jump] = Op::Br(finally_entry);
                }
                self.handlers.extend(handler_entries);
            }
            Stmt::Using {
                resource_ty: _,
                name,
                expr,
                body,
            } => {
                // Lower `using (expr) { body }` to `try { body } finally { resource.Dispose(); }`
                self.emit_expr(expr)?;
                self.push_local(name.clone().unwrap_or_else(|| "__using_temp".to_string()));
                let resource_local = (self.locals.len() - 1) as u16;
                self.ops.push(Op::Stloc(resource_local));

                let try_start = self.ops.len() as u32;
                for s in body {
                    self.emit_stmt(s)?;
                }
                let try_end = self.ops.len() as u32;
                let normal_jump = self.ops.len();
                self.ops.push(Op::Br(0));

                let finally_entry = self.ops.len() as u32;
                self.ops.push(Op::Ldloc(resource_local));
                self.ops.push(Op::CallVirt("Dispose".to_string()));
                self.ops.push(Op::Pop);
                self.ops.push(Op::EndFinally);

                self.ops[normal_jump] = Op::Br(finally_entry);
                self.handlers.push(ExceptionHandler {
                    start: try_start,
                    end: try_end,
                    catch_type: None,
                    handler_pc: finally_entry,
                    catch_local: 0,
                });
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::NonNullAssert(inner) => {
                self.emit_expr(inner)?;
                self.ops.push(Op::NativeCall(aura_bytecode::natives::NativeId::AssertNonNull as u16));
            }
            Expr::Is(subject, ty, binding) => {
                let Type::Class(name, _) = ty else {
                    return Err(format!("`is` requires a class type, got {}", ty.name()));
                };
                let class_id = *self
                    .class_ids
                    .get(name)
                    .ok_or_else(|| format!("unknown class `{}`", name))?;
                self.emit_expr(subject)?;
                if let Some(b) = binding {
                    // Store a copy of the subject under the binding name; the
                    // typer only exposes it where the test is known true.
                    self.ops.push(Op::Dup);
                    self.push_local(b.clone());
                    let idx = (self.locals.len() - 1) as u16;
                    self.local_types.insert(b.clone(), ty.clone());
                    self.ops.push(Op::Stloc(idx));
                }
                self.ops.push(Op::IsInst(class_id));
            }
            Expr::IntLit(i, _) => self.emit_int_const(*i),
            Expr::FloatLit(f, suffix) => {
                let idx = self.constants.len() as u32;
                self.constants.push(f.to_string());
                self.ops.push(Op::LdFloat(idx));
                if *suffix == FloatSuffix::F32 {
                    self.ops.push(Op::Conv(TypeDesc::Float32));
                }
            }
            Expr::Cast(inner, ty) => {
                self.emit_expr(inner)?;
                self.ops.push(Op::Conv(map_type(ty, self.class_ids, self.enum_ids, &[])));
            }
            Expr::Bool(b) => self.ops.push(Op::LdBool(*b)),
            Expr::Char(c) => self.ops.push(Op::LdChar(*c as u32)),
            Expr::String(s) => {
                let idx = self.constants.len() as u32;
                self.constants.push(s.clone());
                self.ops.push(Op::LdStr(idx));
            }
            Expr::InterpolatedString(parts) => {
                // Emit code for each part
                for part in parts {
                    match part {
                        InterpPart::Literal(s) => {
                            let idx = self.constants.len() as u32;
                            self.constants.push(s.clone());
                            self.ops.push(Op::LdStr(idx));
                        }
                        InterpPart::Expr(expr) => {
                            self.emit_expr(expr)?;
                        }
                    }
                }
                // Concatenate all parts
                self.ops.push(Op::StringConcat(parts.len() as u16));
            }
            Expr::Null => self.ops.push(Op::LdNull),
            Expr::Var(name) => {
                if name == "this" {
                    if self.method.is_static {
                        return Err("`this` in static method".to_string());
                    }
                    self.ops.push(Op::Ldloc(0));
                } else if name == "super" {
                    return Err("`super` must be used as `super.member`".to_string());
                } else if let Some(idx) = self.locals.iter().rposition(|n| n == name) {
                    self.ops.push(Op::Ldloc(idx as u16));
                } else if let Some(idx) = self.field_index(name).ok() {
                    self.ops.push(Op::Ldloc(0)); // this
                    self.ops.push(Op::Ldfld(idx));
                } else if let Some((declaring, idx)) = self.static_field_index_opt(self.class_name, name) {
                    let class_id = *self.class_ids.get(&declaring).unwrap();
                    self.ops.push(Op::Ldsfld(class_id, idx));
                } else if self.class_info.name == *name {
                    return Err(format!("class name `{}` cannot be used as a value", name));
                } else {
                    return Err(format!("unknown variable `{}`", name));
                }
            }
            Expr::Field(obj, name) => {
                // Newtype unwrap (`id.Value`) is fully erased. Strip the
                // nullable wrapper: a narrowed `UserId?` local still types
                // as nullable here, but the typer has already proven the
                // access safe.
                if name == "Value" {
                    if let Ok(ty) = self.expr_ty(obj) {
                        if matches!(Self::strip_nullable(ty), Type::Newtype(..)) {
                            self.emit_expr(obj)?;
                            return Ok(());
                        }
                    }
                }
                // Intrinsic properties (`list.Count`, `s.Length`).
                if let Some(native) = self.intrinsic_property_native(obj, name)? {
                    self.ops.push(Op::NativeCall(native as u16));
                    return Ok(());
                }
                let obj_class = self.expr_class(obj);
                if let Some(obj_class) = &obj_class {
                    if let Some((_declaring, _)) = self.instance_property_opt(obj_class, name) {
                        if let Expr::Var(n) = obj.as_ref() {
                            if n == "this" {
                                self.ops.push(Op::Ldloc(0));
                            } else {
                                self.emit_expr(obj)?;
                            }
                        } else {
                            self.emit_expr(obj)?;
                        }
                        self.ops.push(Op::CallVirt(format!("get_{}", name)));
                        return Ok(());
                    }
                }
                if let Expr::Var(n) = obj.as_ref() {
                    if n == "this" {
                        self.ops.push(Op::Ldloc(0));
                    } else {
                        self.emit_expr(obj)?;
                    }
                } else {
                    self.emit_expr(obj)?;
                }
                let obj_class = self.expr_class(obj).ok_or_else(|| {
                    format!("cannot determine type of field target `{}`", name)
                })?;
                let idx = self.field_index_for(&obj_class, name)?;
                self.ops.push(Op::Ldfld(idx));
            }
            Expr::StaticField(class_name, name) => {
                if let Some(enum_id) = self.enum_ids.get(class_name) {
                    let enum_def = self.program.enums.get(class_name).ok_or_else(|| {
                        format!("unknown enum `{}`", class_name)
                    })?;
                    let variant_idx = enum_def.variants.iter().position(|v| v.name == *name)
                        .ok_or_else(|| format!("unknown variant `{}.{}`", class_name, name))?;
                    self.ops.push(Op::NewEnum(*enum_id, variant_idx as u16));
                } else if let Some((declaring, _)) = self.static_property_opt(class_name, name) {
                    let method_id = *self.method_ids.get(&(
                        declaring.clone(),
                        format!("get_{}", name),
                        false,
                    )).ok_or_else(|| {
                        format!("unknown getter for property `{}` on `{}`", name, declaring)
                    })?;
                    self.ops.push(Op::Call(method_id));
                } else {
                    let (declaring, idx) = self.static_field_index(class_name, name)?;
                    let class_id = *self.class_ids.get(&declaring).unwrap();
                    self.ops.push(Op::Ldsfld(class_id, idx));
                }
            }
            Expr::SuperCall(method_name, args) => {
                let method_id = self.super_method_id(method_name)?;
                for arg in args {
                    self.emit_expr(arg)?;
                }
                self.ops.push(Op::Ldloc(0)); // this
                self.ops.push(Op::CallSuper(method_id));
            }
            Expr::SuperField(name) => {
                if let Some((declaring, _)) = self.super_instance_property(name) {
                    let method_id = *self.method_ids.get(&(
                        declaring.clone(),
                        format!("get_{}", name),
                        true,
                    )).ok_or_else(|| {
                        format!("unknown getter for property `{}` on `{}`", name, declaring)
                    })?;
                    self.ops.push(Op::Ldloc(0)); // this
                    self.ops.push(Op::CallSuper(method_id));
                } else {
                    self.ops.push(Op::Ldloc(0)); // this
                    let idx = self.super_field_index(name)?;
                    self.ops.push(Op::Ldfld(idx));
                }
            }
            Expr::EnumVariant(enum_name, variant_name, args) => {
                let enum_id = *self.enum_ids.get(enum_name).ok_or_else(|| {
                    format!("unknown enum `{}`", enum_name)
                })?;
                let enum_info = self.program.enums.get(enum_name).ok_or_else(|| {
                    format!("unknown enum `{}`", enum_name)
                })?;
                let variant_idx = enum_info.variants.iter().position(|v| v.name == *variant_name)
                    .ok_or_else(|| format!("unknown variant `{}.{}`", enum_name, variant_name))?;
                for arg in args {
                    self.emit_expr(arg)?;
                }
                self.ops.push(Op::NewEnum(enum_id, variant_idx as u16));
            }
            Expr::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                // Short-circuit: the right operand must not be evaluated when
                // the left already decides the result — type guards like
                // `x != null && x.f > 0` depend on it for soundness.
                self.emit_expr(left)?;
                let short_jump = self.ops.len();
                if matches!(op, BinOp::And) {
                    self.ops.push(Op::BrFalse(0));
                } else {
                    self.ops.push(Op::BrTrue(0));
                }
                self.emit_expr(right)?;
                let end_jump = self.ops.len();
                self.ops.push(Op::Br(0));
                let short_target = self.ops.len() as u32;
                self.ops.push(Op::LdBool(matches!(op, BinOp::Or)));
                let end_target = self.ops.len() as u32;
                if matches!(op, BinOp::And) {
                    self.ops[short_jump] = Op::BrFalse(short_target);
                } else {
                    self.ops[short_jump] = Op::BrTrue(short_target);
                }
                self.ops[end_jump] = Op::Br(end_target);
            }
            Expr::Binary(op, left, right) => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                if matches!(op, BinOp::Eq | BinOp::Ne) {
                    // Records compare by value (deep structural equality); strings
                    // compare by content (the identity-based `Eq` cannot handle
                    // string vs string).
                    let stringish = |t: &Type| {
                        matches!(t, Type::String | Type::StringLit(_) | Type::LiteralUnion(..))
                    };
                    let value_eq = matches!((self.expr_ty(left), self.expr_ty(right)),
                        (Ok(l), Ok(r))
                            if self.is_record_type(&l) || self.is_record_type(&r)
                                || stringish(&l) || stringish(&r));
                    if value_eq {
                        self.ops.push(Op::ValueEq);
                        if matches!(op, BinOp::Ne) {
                            self.ops.push(Op::Not);
                        }
                        return Ok(());
                    }
                }
                let emit_op = match op {
                    BinOp::Add => Op::Add,
                    BinOp::Sub => Op::Sub,
                    BinOp::Mul => Op::Mul,
                    BinOp::Div => Op::Div,
                    BinOp::Rem => Op::Rem,
                    BinOp::Eq => Op::Eq,
                    BinOp::Ne => {
                        self.ops.push(Op::Eq);
                        Op::Not
                    }
                    BinOp::Lt => Op::Lt,
                    BinOp::Le => Op::Le,
                    BinOp::Gt => Op::Gt,
                    BinOp::Ge => Op::Ge,
                    BinOp::And => Op::And,
                    BinOp::Or => Op::Or,
                };
                self.ops.push(emit_op);
                // Arithmetic must re-narrow to the static result width: float32
                // is re-rounded to single precision, and narrow ints wrap to
                // their bit width (and are range-checked when overflow checking
                // is enabled).
                if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem) {
                    if let Ok(ty) = self.expr_ty(expr) {
                        if let Some(td) = arith_conv(&ty) {
                            self.ops.push(Op::Conv(td));
                        }
                    }
                }
            }
            Expr::Unary(op, operand) => {
                self.emit_expr(operand)?;
                self.ops.push(match op {
                    UnaryOp::Neg => Op::Neg,
                    UnaryOp::Not => Op::Not,
                });
                if matches!(op, UnaryOp::Neg) {
                    if let Ok(ty) = self.expr_ty(expr) {
                        if let Some(td) = arith_conv(&ty) {
                            self.ops.push(Op::Conv(td));
                        }
                    }
                }
            }
            Expr::Call(call) => {
                if call.class_or_target == "__intrinsics" {
                    if call.method == "print" {
                        self.emit_expr(&call.args[0])?;
                        self.ops.push(Op::Print);
                    } else if call.method == "println" {
                        self.ops.push(Op::PrintLn);
                    }
                } else {
                    if let Some(target) = &call.target {
                        // Intrinsic dispatch: static calls on `Console`/`File`
                        // and instance calls on `List`/`Map`/`Set`/`string`
                        // lower to `NativeCall` (receiver pushed first, then
                        // arguments — unlike `CallVirt`, which takes the
                        // receiver on top).
                        if let Some(native) = self.intrinsic_call_native(call, target)? {
                            self.ops.push(Op::NativeCall(native as u16));
                            return Ok(());
                        }
                        if let Expr::Var(class_name) = target.as_ref() {
                            if self.class_ids.contains_key(class_name) {
                                // Static method call: target is a class name.
                                let method_id = *self.method_ids.get(&(
                                    class_name.clone(),
                                    call.method.clone(),
                                    false,
                                )).ok_or_else(|| {
                                    format!(
                                        "unknown static method `{}` on `{}`",
                                        call.method, class_name
                                    )
                                })?;
                                for arg in &call.args {
                                    self.emit_expr(arg)?;
                                }
                                self.ops.push(Op::Call(method_id));
                            } else {
                                // Instance call on local/field variable.
                                // Push arguments first, then the instance.
                                for arg in &call.args {
                                    self.emit_expr(arg)?;
                                }
                                self.emit_expr(target)?;
                                self.ops.push(Op::CallVirt(call.method.clone()));
                            }
                        } else {
                            // Push arguments first, then the instance.
                            for arg in &call.args {
                                self.emit_expr(arg)?;
                            }
                            self.emit_expr(target)?;
                            self.ops.push(Op::CallVirt(call.method.clone()));
                        }
                    } else if self.program.newtypes.contains_key(&call.method) {
                        // Newtype constructor: fully erased — the wrapped
                        // expression is the value.
                        self.emit_expr(&call.args[0])?;
                    } else if let Some(im) =
                        crate::intrinsics::static_method(&call.class_or_target, &call.method)
                    {
                        // Static intrinsic call parsed without a target
                        // expression: `File.WriteAllText(...)`.
                        for arg in &call.args {
                            self.emit_expr(arg)?;
                        }
                        self.ops.push(Op::NativeCall(im.native as u16));
                    } else {
                        if let Some(enum_id) = self.enum_ids.get(&call.class_or_target) {
                            let enum_def = self.program.enums.get(&call.class_or_target).ok_or_else(|| {
                                format!("unknown enum `{}`", call.class_or_target)
                            })?;
                            let variant_idx = enum_def.variants.iter().position(|v| v.name == call.method)
                                .ok_or_else(|| format!("unknown variant `{}.{}`", call.class_or_target, call.method))?;
                            for arg in &call.args {
                                self.emit_expr(arg)?;
                            }
                            self.ops.push(Op::NewEnum(*enum_id, variant_idx as u16));
                        } else if let Some((enum_id, variant_idx)) = self.variant_lookup.get(&call.method).copied() {
                            // Bare sum-type constructor: `Ok(5)`.
                            for arg in &call.args {
                                self.emit_expr(arg)?;
                            }
                            self.ops.push(Op::NewEnum(enum_id, variant_idx));
                        } else {
                            for arg in &call.args {
                                self.emit_expr(arg)?;
                            }
                            // Bare static method call: `foo(args)` without a
                            // class qualifier resolves to the current class.
                            let owner = if self.class_ids.contains_key(&call.class_or_target) {
                                call.class_or_target.clone()
                            } else {
                                self.class_name.to_string()
                            };
                            let method_id = *self.method_ids.get(&(
                                owner.clone(),
                                call.method.clone(),
                                false,
                            )).ok_or_else(|| {
                                format!(
                                    "unknown static method `{}` on `{}`",
                                    call.method, owner
                                )
                            })?;
                            self.ops.push(Op::Call(method_id));
                        }
                    }
                }
            }
            Expr::New(class_name, type_args, args) => {
                // Intrinsic constructors (`new List<int>()`) lower to a native
                // call; the typer guarantees they take no arguments.
                if let Some(ctor) = crate::intrinsics::constructor_native(class_name) {
                    self.ops.push(Op::NativeCall(ctor as u16));
                    return Ok(());
                }
                let class_id = *self.class_ids.get(class_name).ok_or_else(|| {
                    format!("unknown class `{}`", class_name)
                })?;
                let has_constructor = !self.program.classes.get(class_name)
                    .map(|ci| ci.constructors.is_empty())
                    .unwrap_or(true);
                let ctor_id = if has_constructor {
                    Some(self.resolve_constructor_id(class_name, type_args, args)?)
                } else {
                    None
                };
                for arg in args {
                    self.emit_expr(arg)?;
                }
                let mapped_type_args: Vec<TypeDesc> = type_args.iter()
                    .map(|arg| map_type(arg, self.class_ids, self.enum_ids, &[]))
                    .collect();
                self.ops.push(Op::NewObj(class_id, mapped_type_args));
                if let Some(ctor_id) = ctor_id {
                    // Stash the new object, run its constructor, then restore it
                    // so the `new` expression evaluates to the object.
                    let obj_local = self.push_local("__new_temp".to_string()) as u16;
                    self.ops.push(Op::Stloc(obj_local));
                    self.ops.push(Op::Ldloc(obj_local));
                    self.ops.push(Op::CallSuper(ctor_id));
                    self.ops.push(Op::Pop);
                    self.ops.push(Op::Ldloc(obj_local));
                }
            }
            Expr::Tuple(elements) => {
                for elem in elements {
                    self.emit_expr(elem)?;
                }
                self.ops.push(Op::NewTuple(elements.len() as u16));
            }
            Expr::TupleIndex(tuple, idx) => {
                self.emit_expr(tuple)?;
                self.ops.push(Op::TupleField(*idx as u16));
            }
            Expr::Range(start, end, _inclusive) => {
                // For now, ranges are only used in for-in loops, which are desugared.
                // If used as a standalone expression, we emit start and end as a tuple.
                // This is a placeholder; proper range objects would need a Range class.
                self.emit_expr(start)?;
                self.emit_expr(end)?;
                self.ops.push(Op::NewTuple(2));
            }
            Expr::Ternary(cond, then_expr, else_expr) => {
                self.emit_expr(cond)?;
                let false_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                self.emit_expr(then_expr)?;
                let end_jump = self.ops.len();
                self.ops.push(Op::Br(0));
                let else_start = self.ops.len() as u32;
                self.emit_expr(else_expr)?;
                let end = self.ops.len() as u32;
                self.ops[false_jump] = Op::BrFalse(else_start);
                self.ops[end_jump] = Op::Br(end);
            }
            Expr::NullCoalesce(left, right) => {
                // Evaluate left operand
                self.emit_expr(left)?;
                // Duplicate it for the null check
                let temp_local = self.locals.len() as u16;
                self.push_local("__null_coalesce_temp".to_string());
                self.ops.push(Op::Stloc(temp_local));
                self.ops.push(Op::Ldloc(temp_local));
                // Check if null
                self.ops.push(Op::LdNull);
                self.ops.push(Op::Eq);
                let not_null_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                // If null, evaluate right operand
                self.emit_expr(right)?;
                let end_jump = self.ops.len();
                self.ops.push(Op::Br(0));
                // If not null, use left operand
                let left_value = self.ops.len() as u32;
                self.ops.push(Op::Ldloc(temp_local));
                let end = self.ops.len() as u32;
                self.ops[not_null_jump] = Op::BrFalse(left_value);
                self.ops[end_jump] = Op::Br(end);
            }
            Expr::NullConditionalField(obj, field_name) => {
                // Evaluate object
                self.emit_expr(obj)?;
                // Store in temp and duplicate for null check
                let temp_local = self.locals.len() as u16;
                self.push_local("__null_cond_temp".to_string());
                self.ops.push(Op::Stloc(temp_local));
                self.ops.push(Op::Ldloc(temp_local));
                // Check if null
                self.ops.push(Op::LdNull);
                self.ops.push(Op::Eq);
                let not_null_jump = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                // If null, leave null on stack
                self.ops.push(Op::LdNull);
                let end_jump = self.ops.len();
                self.ops.push(Op::Br(0));
                // If not null, access field
                let field_access = self.ops.len() as u32;
                self.ops.push(Op::Ldloc(temp_local));
                // Get field index
                let obj_class = self.expr_class(obj).ok_or_else(|| {
                    format!("cannot determine type of field target `{}`", field_name)
                })?;
                let idx = self.field_index_for(&obj_class, field_name)?;
                self.ops.push(Op::Ldfld(idx));
                let end = self.ops.len() as u32;
                self.ops[not_null_jump] = Op::BrFalse(field_access);
                self.ops[end_jump] = Op::Br(end);
            }
            Expr::NullConditionalCall(call) => {
                // Similar to regular call but with null check
                if let Some(target) = &call.target {
                    // Evaluate target
                    self.emit_expr(target)?;
                    // Store in temp and duplicate for null check
                    let temp_local = self.locals.len() as u16;
                    self.push_local("__null_call_temp".to_string());
                    self.ops.push(Op::Stloc(temp_local));
                    self.ops.push(Op::Ldloc(temp_local));
                    // Check if null
                    self.ops.push(Op::LdNull);
                    self.ops.push(Op::Eq);
                    let not_null_jump = self.ops.len();
                    self.ops.push(Op::BrFalse(0));
                    // If null, leave null on stack
                    self.ops.push(Op::LdNull);
                    let end_jump = self.ops.len();
                    self.ops.push(Op::Br(0));
                    // If not null, call method
                    let call_start = self.ops.len() as u32;
                    self.ops.push(Op::Ldloc(temp_local));
                    // Emit arguments
                    for arg in &call.args {
                        self.emit_expr(arg)?;
                    }
                    // Call method
                    self.ops.push(Op::CallVirt(call.method.clone()));
                    let end = self.ops.len() as u32;
                    self.ops[not_null_jump] = Op::BrFalse(call_start);
                    self.ops[end_jump] = Op::Br(end);
                } else {
                    return Err("null conditional call requires target".to_string());
                }
            }
            Expr::Match(subject, arms) => {
                // Emit subject
                self.emit_expr(subject)?;
                // Store subject in a temporary local
                let subject_local = self.locals.len() as u16;
                self.push_local("__match_subject".to_string());
                self.ops.push(Op::Stloc(subject_local));
                
                let mut end_jumps = Vec::new();
                let mut arm_fail_jumps = Vec::new();
                
                for arm in arms {
                    // For multiple patterns in one arm, we need to try each one
                    let mut pattern_success_jumps = Vec::new();
                    
                    for pattern in &arm.patterns {
                        let mut fail_jumps = Vec::new();
                        self.emit_pattern(pattern, subject_local, &mut fail_jumps)?;
                        // On success (no fail jump taken), jump to the body.
                        pattern_success_jumps.push(self.ops.len());
                        self.ops.push(Op::Br(0)); // placeholder
                        // Fail jumps skip this success jump and try the next pattern.
                        let after = self.ops.len() as u32;
                        for jump in &fail_jumps {
                            self.ops[*jump] = Op::BrFalse(after);
                        }
                        // A wildcard or binding always matches.
                        if matches!(pattern, Pattern::Wildcard | Pattern::Binding(_)) {
                            break; // No need to check more patterns
                        }
                    }
                    
                    // If none of the patterns matched, skip to next arm
                    let arm_fail_jump = self.ops.len();
                    self.ops.push(Op::Br(0)); // placeholder - will be patched to skip body
                    arm_fail_jumps.push(arm_fail_jump);
                    
                    // Patch pattern success jumps to here (body start)
                    let body_start = self.ops.len() as u32;
                    for jump in pattern_success_jumps {
                        match self.ops[jump] {
                            Op::Br(_) => self.ops[jump] = Op::Br(body_start),
                            Op::BrTrue(_) => self.ops[jump] = Op::BrTrue(body_start),
                            _ => self.ops[jump] = Op::BrTrue(body_start),
                        }
                    }
                    
                    // Check guard if present
                    if let Some(guard) = &arm.guard {
                        self.emit_expr(guard)?;
                        let guard_fail = self.ops.len();
                        self.ops.push(Op::BrFalse(0));
                        
                        // Emit body
                        self.emit_expr(&arm.body)?;
                        // A void body (e.g. an intrinsic or void method call)
                        // leaves nothing on the stack; push null so the match
                        // always yields exactly one value.
                        if self.expr_ty(&arm.body).map(|t| t == Type::Unit).unwrap_or(false) {
                            self.ops.push(Op::LdNull);
                        }
                        let end_jump = self.ops.len();
                        self.ops.push(Op::Br(0));
                        end_jumps.push(end_jump);
                        
                        self.ops[guard_fail] = Op::BrFalse(self.ops.len() as u32);
                    } else {
                        // Emit body
                        self.emit_expr(&arm.body)?;
                        if self.expr_ty(&arm.body).map(|t| t == Type::Unit).unwrap_or(false) {
                            self.ops.push(Op::LdNull);
                        }
                        let end_jump = self.ops.len();
                        self.ops.push(Op::Br(0));
                        end_jumps.push(end_jump);
                    }
                    
                    // Patch arm fail jump to here (after body)
                    self.ops[arm_fail_jump] = Op::Br(self.ops.len() as u32);
                }
                
                // If no arm matched, push null
                self.ops.push(Op::LdNull);
                
                // Patch all end jumps
                let end = self.ops.len() as u32;
                for jump in end_jumps {
                    self.ops[jump] = Op::Br(end);
                }
                
                // Pop the temporary local
                self.locals.pop();
            }
            Expr::With(obj, updates) => {
                let obj_class = self.expr_class(obj).or_else(|| match self.expr_ty(obj) {
                    Ok(Type::Class(c, _)) => Some(c),
                    _ => None,
                }).ok_or_else(|| "cannot determine record type of `with` operand".to_string())?;
                let class_id = *self.class_ids.get(&obj_class)
                    .ok_or_else(|| format!("unknown class `{}`", obj_class))?;
                let layout = self.field_layout.get(&obj_class).cloned()
                    .ok_or_else(|| format!("unknown field layout for `{}`", obj_class))?;

                // Copy the source record into a fresh instance, then override
                // the listed fields.
                self.emit_expr(obj)?;
                let src_local = self.push_local("__with_src".to_string()) as u16;
                self.ops.push(Op::Stloc(src_local));
                self.ops.push(Op::NewObj(class_id, vec![]));
                let new_local = self.push_local("__with_new".to_string()) as u16;
                self.ops.push(Op::Stloc(new_local));
                for (i, _) in layout.iter().enumerate() {
                    self.ops.push(Op::Ldloc(src_local));
                    self.ops.push(Op::Ldfld(i as u16));
                    self.ops.push(Op::Ldloc(new_local));
                    self.ops.push(Op::Stfld(i as u16));
                }
                for (field, value) in updates {
                    let idx = self.field_index_for(&obj_class, field)?;
                    self.emit_expr(value)?;
                    self.ops.push(Op::Ldloc(new_local));
                    self.ops.push(Op::Stfld(idx));
                }
                self.ops.push(Op::Ldloc(new_local));
            }
            Expr::Block(stmts) => {
                let start_locals = self.locals.len();
                let mut iter = stmts.iter().peekable();
                while let Some(s) = iter.next() {
                    if iter.peek().is_none() {
                        // Last statement: an expression leaves its value on the
                        // stack (no trailing Pop like a normal expression stmt).
                        if let Stmt::Expr(e) = s {
                            self.emit_expr(e)?;
                            // A void trailing expression leaves nothing on the
                            // stack; push null so the block still yields a value.
                            if self.expr_ty(e).map(|t| t == Type::Unit).unwrap_or(false) {
                                self.ops.push(Op::LdNull);
                            }
                            break;
                        }
                    }
                    self.emit_stmt(s)?;
                }
                // Non-expression-ending (or empty) blocks evaluate to void.
                if !matches!(stmts.last(), Some(Stmt::Expr(_))) {
                    self.ops.push(Op::LdNull);
                }
                // Block locals are scoped to the block.
                while self.locals.len() > start_locals {
                    self.locals.pop();
                }
            }
            Expr::TryUnwrap(inner) => {
                // `expr?` — if the subject is the success variant (variant 0),
                // push its payload; otherwise return the error value as-is
                // from the enclosing function.
                let inner_ty = self.expr_ty(inner)?;
                let _enum_name = match &inner_ty {
                    Type::Class(name, _) | Type::Enum(name) => name.clone(),
                    _ => {
                        return Err(format!(
                            "`?` requires a Result-like sum type, got `{}`",
                            inner_ty.name()
                        ));
                    }
                };
                self.emit_expr(inner)?;
                let temp = self.push_local("__try_unwrap".to_string()) as u16;
                self.ops.push(Op::Stloc(temp));
                // success variant index is 0
                self.ops.push(Op::Ldloc(temp));
                self.ops.push(Op::EnumTag);
                self.ops.push(Op::LdInt(0));
                self.ops.push(Op::Eq);
                let err_label = self.ops.len();
                self.ops.push(Op::BrFalse(0));
                self.ops.push(Op::Ldloc(temp));
                self.ops.push(Op::EnumField(0));
                let end_label = self.ops.len();
                self.ops.push(Op::Br(0));
                self.ops[err_label] = Op::BrFalse(self.ops.len() as u32);
                self.ops.push(Op::Ldloc(temp));
                self.ops.push(Op::Ret);
                self.ops[end_label] = Op::Br(self.ops.len() as u32);
                self.locals.pop();
            }
        }
        Ok(())
    }

    /// Push an integer constant, choosing the compact `LdInt` form for values
    /// that fit in 32 bits and `LdInt64` otherwise.
    fn emit_int_const(&mut self, v: i64) {
        if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
            self.ops.push(Op::LdInt(v as i32));
        } else {
            self.ops.push(Op::LdInt64(v));
        }
    }

    fn local_index(&self, name: &str) -> Result<u16, String> {
        self.locals
            .iter()
            .rposition(|n| n == name)
            .map(|i| i as u16)
            .ok_or_else(|| format!("unknown local `{}`", name))
    }

    /// Allocate a new local slot, tracking the peak local count so the frame
    /// stays sized correctly even when scoped locals are popped later.
    /// Returns the index of the newly allocated local.
    fn push_local(&mut self, name: String) -> usize {
        self.locals.push(name);
        let len = self.locals.len();
        if len > self.max_locals {
            self.max_locals = len;
        }
        len - 1
    }

    /// Emit a pattern test against the value in `subject_local`. On success,
    /// pattern bindings have been created. On failure, a `BrFalse` jump is
    /// recorded in `fail_jumps` (to be patched by the caller).
    fn emit_pattern(
        &mut self,
        pattern: &Pattern,
        subject_local: u16,
        fail_jumps: &mut Vec<usize>,
    ) -> Result<(), String> {
        match pattern {
            Pattern::Wildcard => {}
            Pattern::Int(i) => {
                self.ops.push(Op::Ldloc(subject_local));
                self.emit_int_const(*i);
                self.ops.push(Op::Eq);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));
            }
            Pattern::Float(f) => {
                let idx = self.constants.len() as u32;
                self.constants.push(f.to_string());
                self.ops.push(Op::Ldloc(subject_local));
                self.ops.push(Op::LdFloat(idx));
                self.ops.push(Op::Eq);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));
            }
            Pattern::Bool(b) => {
                self.ops.push(Op::Ldloc(subject_local));
                self.ops.push(Op::LdBool(*b));
                self.ops.push(Op::Eq);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));
            }
            Pattern::StringLit(s) => {
                let idx = self.constants.len() as u32;
                self.constants.push(s.clone());
                self.ops.push(Op::Ldloc(subject_local));
                self.ops.push(Op::LdStr(idx));
                self.ops.push(Op::ValueEq);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));
            }
            Pattern::Null => {
                self.ops.push(Op::Ldloc(subject_local));
                self.ops.push(Op::LdNull);
                self.ops.push(Op::Eq);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));
            }
            Pattern::Binding(name) => {
                // Binding always matches: copy the subject into a new local.
                let local_idx = self.push_local(name.clone()) as u16;
                self.ops.push(Op::Ldloc(subject_local));
                self.ops.push(Op::Stloc(local_idx));
            }
            Pattern::EnumVariant(enum_name, variant_name, sub_patterns) => {
                let enum_def = self.program.enums.get(enum_name).ok_or_else(|| {
                    format!("unknown enum `{}`", enum_name)
                })?;
                let variant_idx = enum_def.variants.iter().position(|v| v.name == *variant_name)
                    .ok_or_else(|| format!("unknown variant `{}.{}`", enum_name, variant_name))?;

                self.ops.push(Op::Ldloc(subject_local));
                self.ops.push(Op::EnumTag);
                self.ops.push(Op::LdInt(variant_idx as i32));
                self.ops.push(Op::Eq);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));

                for (i, sub) in sub_patterns.iter().enumerate() {
                    // Load field i into a temp local, then test the sub-pattern.
                    let field_local = self.push_local("__pattern_field".to_string()) as u16;
                    self.ops.push(Op::Ldloc(subject_local));
                    self.ops.push(Op::EnumField(i as u16));
                    self.ops.push(Op::Stloc(field_local));
                    self.emit_pattern(sub, field_local, fail_jumps)?;
                }
            }
            Pattern::RecordClass(class_name, sub_patterns) => {
                match self.program.classes.get(class_name) {
                    Some(info) => {
                        if !info.is_record {
                            return Err(format!("`{}` is not a record class", class_name));
                        }
                        for (i, sub) in sub_patterns.iter().enumerate() {
                            let field_local = self.push_local("__pattern_field".to_string()) as u16;
                            self.ops.push(Op::Ldloc(subject_local));
                            self.ops.push(Op::Ldfld(i as u16));
                            self.ops.push(Op::Stloc(field_local));
                            self.emit_pattern(sub, field_local, fail_jumps)?;
                        }
                    }
                    None => {
                        // Bare sum-type variant pattern: `Ok(v)`.
                        let (_enum_id, variant_idx) = *self.variant_lookup.get(class_name).ok_or_else(|| {
                            format!("unknown class `{}`", class_name)
                        })?;
                        self.ops.push(Op::Ldloc(subject_local));
                        self.ops.push(Op::EnumTag);
                        self.ops.push(Op::LdInt(variant_idx as i32));
                        self.ops.push(Op::Eq);
                        fail_jumps.push(self.ops.len());
                        self.ops.push(Op::BrFalse(0));

                        for (i, sub) in sub_patterns.iter().enumerate() {
                            let field_local = self.push_local("__pattern_field".to_string()) as u16;
                            self.ops.push(Op::Ldloc(subject_local));
                            self.ops.push(Op::EnumField(i as u16));
                            self.ops.push(Op::Stloc(field_local));
                            self.emit_pattern(sub, field_local, fail_jumps)?;
                        }
                    }
                }
            }
            Pattern::Range(start, end, inclusive) => {
                // subject >= start (or > for exclusive)
                self.ops.push(Op::Ldloc(subject_local));
                self.emit_expr(start)?;
                if *inclusive {
                    self.ops.push(Op::Ge);
                } else {
                    self.ops.push(Op::Gt);
                }
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));

                // subject <= end (or < for exclusive)
                self.ops.push(Op::Ldloc(subject_local));
                self.emit_expr(end)?;
                self.ops.push(Op::Le);
                fail_jumps.push(self.ops.len());
                self.ops.push(Op::BrFalse(0));
            }
        }
        Ok(())
    }

    /// Index of an instance field in the full (base + own) layout of the
    /// current class. Subclass declarations shadow base fields, so search from the end.
    fn field_index(&self, name: &str) -> Result<u16, String> {
        self.field_index_for(self.class_name, name)
    }

    fn field_index_for(&self, obj_class: &str, name: &str) -> Result<u16, String> {
        self.field_layout
            .get(obj_class)
            .and_then(|layout| layout.iter().rposition(|(n, _)| n == name))
            .map(|i| i as u16)
            .ok_or_else(|| format!("unknown field `{}` on `{}`", name, obj_class))
    }

    /// Static class name of an object expression, used to resolve field indices.
    /// Desugar `for (T x in <list-or-set>)` into an index loop over the
    /// existing List natives:
    ///
    /// ```text
    /// __it = <iterable>          // Set: <iterable>.ToList() snapshot copy
    /// __n  = __it.Count          // count snapshot, taken once
    /// __i  = 0
    /// while (__i < __n) { T x = __it.Get(__i); body; __i = __i + 1 }
    /// ```
    ///
    /// Mutation-during-iteration semantics (documented in TODO.md §4.2):
    /// the element count is snapshotted when the loop starts, so elements
    /// appended to a List during iteration are not visited; removing List
    /// elements during iteration may make a later `Get` fail with an
    /// index-out-of-range runtime error. Iterating a Set walks a `ToList()`
    /// snapshot copy, so Set mutations never affect an iteration in
    /// progress.
    fn emit_for_in_collection(
        &mut self,
        label: &Option<String>,
        var_type: &Type,
        var_name: &str,
        iterable: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        use aura_bytecode::natives::NativeId;
        let iter_ty = Self::strip_nullable(self.expr_ty(iterable)?);
        let is_set = matches!(&iter_ty, Type::Class(n, _) if n == "Set");
        if !matches!(&iter_ty, Type::Class(n, _) if n == "List" || n == "Set") {
            return Err(format!(
                "for-in requires a range, List, or Set expression, got {}",
                iter_ty.name()
            ));
        }

        // __it = iterable (Sets snapshot into a fresh List)
        self.emit_expr(iterable)?;
        if is_set {
            self.ops.push(Op::NativeCall(NativeId::SetToList as u16));
        }
        self.push_local("__foreach_it".to_string());
        let it_idx = (self.locals.len() - 1) as u16;
        self.ops.push(Op::Stloc(it_idx));

        // __n = __it.Count
        self.ops.push(Op::Ldloc(it_idx));
        self.ops.push(Op::NativeCall(NativeId::ListCount as u16));
        self.push_local("__foreach_n".to_string());
        let n_idx = (self.locals.len() - 1) as u16;
        self.ops.push(Op::Stloc(n_idx));

        // __i = 0
        self.ops.push(Op::LdInt(0));
        self.push_local("__foreach_i".to_string());
        let i_idx = (self.locals.len() - 1) as u16;
        self.ops.push(Op::Stloc(i_idx));

        let loop_start = self.ops.len() as u32;
        self.break_targets.push((label.clone(), Vec::new()));
        self.continue_targets.push((label.clone(), Vec::new()));

        // while (__i < __n)
        self.ops.push(Op::Ldloc(i_idx));
        self.ops.push(Op::Ldloc(n_idx));
        self.ops.push(Op::Lt);
        let exit_jump = self.ops.len();
        self.ops.push(Op::BrFalse(0));

        // T x = __it.Get(__i)
        self.ops.push(Op::Ldloc(it_idx));
        self.ops.push(Op::Ldloc(i_idx));
        self.ops.push(Op::NativeCall(NativeId::ListGet as u16));
        self.push_local(var_name.to_string());
        let var_idx = (self.locals.len() - 1) as u16;
        // `var` loop variables take the collection's element type.
        let var_type = if matches!(var_type, Type::Infer) {
            match &iter_ty {
                Type::Class(_, args) if !args.is_empty() => args[0].clone(),
                _ => return Err("cannot infer foreach element type".to_string()),
            }
        } else {
            var_type.clone()
        };
        self.local_types.insert(var_name.to_string(), var_type);
        self.ops.push(Op::Stloc(var_idx));

        for s in body {
            self.emit_stmt(s)?;
        }

        // Continue target: the increment.
        let update_start = self.ops.len() as u32;
        self.ops.push(Op::Ldloc(i_idx));
        self.ops.push(Op::LdInt(1));
        self.ops.push(Op::Add);
        self.ops.push(Op::Stloc(i_idx));
        self.ops.push(Op::Br(loop_start));

        let end_pos = self.ops.len() as u32;
        self.ops[exit_jump] = Op::BrFalse(end_pos);
        let (_, breaks) = self.break_targets.pop().unwrap();
        for jump in breaks {
            self.ops[jump] = Op::Br(end_pos);
        }
        let (_, continues) = self.continue_targets.pop().unwrap();
        for jump in continues {
            self.ops[jump] = Op::Br(update_start);
        }
        Ok(())
    }

    /// If `call` targets an intrinsic (static `Console`/`File` method, or an
    /// instance method of `List`/`Map`/`Set`/`string`), emit its receiver and
    /// arguments (receiver first, then arguments left to right) and return the
    /// native id to invoke. Returns `Ok(None)` for non-intrinsic calls with
    /// nothing emitted.
    fn intrinsic_call_native(&mut self, call: &CallExpr, target: &Expr) -> Result<Option<aura_bytecode::natives::NativeId>, String> {
        // Static intrinsic class: `Console.ReadLine()`, `File.Exists(p)`.
        if let Expr::Var(name) = target {
            if let Some(im) = crate::intrinsics::static_method(name, &call.method) {
                for arg in &call.args {
                    self.emit_expr(arg)?;
                }
                return Ok(Some(im.native));
            }
            // A name that is an intrinsic class but not one of its static
            // methods should not fall through to instance dispatch.
            if crate::intrinsics::is_intrinsic_class(name) && !self.local_types.contains_key(name) {
                return Err(format!(
                    "unknown static method `{}` on `{}`",
                    call.method, name
                ));
            }
        }
        // Instance dispatch by receiver type.
        let Ok(target_ty) = self.expr_ty(target) else {
            return Ok(None);
        };
        let native = match &Self::strip_nullable(target_ty) {
            Type::String | Type::StringLit(_) | Type::LiteralUnion(..) => crate::intrinsics::string_method(&call.method).map(|m| m.native),
            Type::Class(name, _) => crate::intrinsics::method(name, &call.method).map(|m| m.native),
            _ => None,
        };
        match native {
            Some(native) => {
                self.emit_expr(target)?;
                for arg in &call.args {
                    self.emit_expr(arg)?;
                }
                Ok(Some(native))
            }
            None => Ok(None),
        }
    }

    /// If `obj.name` reads an intrinsic property (`list.Count`, `s.Length`),
    /// emit the receiver and return the native id of the getter.
    fn intrinsic_property_native(&mut self, obj: &Expr, name: &str) -> Result<Option<aura_bytecode::natives::NativeId>, String> {
        let Ok(obj_ty) = self.expr_ty(obj) else {
            return Ok(None);
        };
        let native = match &Self::strip_nullable(obj_ty) {
            Type::String | Type::StringLit(_) | Type::LiteralUnion(..) => crate::intrinsics::string_property(name).map(|p| p.native),
            Type::Class(cname, _) => crate::intrinsics::property(cname, name).map(|p| p.native),
            _ => None,
        };
        match native {
            Some(native) => {
                self.emit_expr(obj)?;
                Ok(Some(native))
            }
            None => Ok(None),
        }
    }

    /// Erase nullability for dispatch: the typer has already enforced
    /// null-safety, and values of `T?` and `T` are identical at runtime.
    fn strip_nullable(ty: Type) -> Type {
        match ty {
            Type::Nullable(inner) => *inner,
            t => t,
        }
    }

    fn expr_class(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Var(name) => {
                if name == "this" {
                    if self.method.is_static {
                        None
                    } else {
                        Some(self.class_name.to_string())
                    }
                } else {
                    match self.local_types.get(name).cloned().map(Self::strip_nullable) {
                        Some(Type::Class(c, _)) => Some(c),
                        _ => None,
                    }
                }
            }
            Expr::New(class_name, _, _) => Some(class_name.clone()),
            Expr::NonNullAssert(inner) => self.expr_class(inner),
            Expr::Field(obj, name) => {
                let owner = self.expr_class(obj)?;
                let layout = self.field_layout.get(&owner)?;
                let idx = layout.iter().rposition(|(n, _)| n == name)?;
                match Self::strip_nullable(layout[idx].1.clone()) {
                    Type::Class(c, _) => Some(c),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Resolve the constructor `MethodId` of `class_name` that matches the
    /// given argument expressions (by count and assignability), mirroring the
    /// type checker's overload resolution. `type_args` are substituted into
    /// generic constructor parameters before matching.
    fn resolve_constructor_id(&self, class_name: &str, type_args: &[Type], args: &[Expr]) -> Result<MethodId, String> {
        let ids = self.constructor_ids.get(class_name).ok_or_else(|| {
            format!("class `{}` declares no constructors", class_name)
        })?;
        let infos = self.program.classes.get(class_name)
            .ok_or_else(|| format!("unknown class `{}`", class_name))?;
        let subst = build_subst(&infos.generic_params, type_args);
        let arg_types: Vec<Type> = args
            .iter()
            .map(|a| self.expr_ty(a))
            .collect::<Result<_, _>>()?;
        let idx = infos
            .constructors
            .iter()
            .position(|ctor| {
                ctor.params.len() == arg_types.len()
                    && ctor
                        .params
                        .iter()
                        .zip(arg_types.iter())
                        .all(|(expected, actual)| self.is_assignable(&substitute_type(expected, &subst), actual))
            })
            .ok_or_else(|| {
                format!(
                    "no constructor of `{}` matches arguments ({})",
                    class_name,
                    arg_types
                        .iter()
                        .map(|t| t.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        ids.get(idx).copied().ok_or_else(|| {
            format!("missing constructor id for `{}`", class_name)
        })
    }

    /// Resolve the target `MethodId` for a constructor chaining call
    /// (`: super(...)` or `: this(...)`).
    fn resolve_constructor_id_for_chain(&self, chain: &ConstructorChain) -> Result<MethodId, String> {
        match chain.target {
            ConstructorTarget::Base => {
                let super_name = self.class_info.super_class.as_ref().ok_or_else(|| {
                    format!("class `{}` has no super class to chain to", self.class_name)
                })?;
                self.resolve_constructor_id(super_name, &[], &chain.args)
            }
            ConstructorTarget::This => {
                self.resolve_constructor_id(self.class_name, &[], &chain.args)
            }
        }
    }

    /// Whether a type is a record class.
    fn is_record_type(&self, ty: &Type) -> bool {
        matches!(ty, Type::Class(name, _)
            if self.program.classes.get(name).map(|c| c.is_record).unwrap_or(false))
    }

    /// Lightweight expression type inference for constructor argument matching.
    /// The type checker has already validated the program, so this only needs
    /// to be accurate enough to disambiguate overloads.
    fn expr_ty(&self, expr: &Expr) -> Result<Type, String> {
        let ty = match expr {
            Expr::IntLit(v, suffix) => match suffix {
                IntSuffix::None => {
                    if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                        Type::Int32
                    } else {
                        Type::Int64
                    }
                }
                IntSuffix::I8 => Type::Int8,
                IntSuffix::I16 => Type::Int16,
                IntSuffix::I32 => Type::Int32,
                IntSuffix::I64 => Type::Int64,
                IntSuffix::U8 => Type::UInt8,
                IntSuffix::U16 => Type::UInt16,
                IntSuffix::U32 => Type::UInt32,
                IntSuffix::U64 => Type::UInt64,
            },
            Expr::FloatLit(_, suffix) => match suffix {
                FloatSuffix::None => Type::Float64,
                FloatSuffix::F32 => Type::Float32,
                FloatSuffix::F64 => Type::Float64,
            },
            Expr::Cast(_, ty) => ty.clone(),
            Expr::Bool(_) => Type::Bool,
            Expr::Char(_) => Type::Char,
            Expr::String(_) | Expr::InterpolatedString(_) => Type::String,
            Expr::Null => Type::Class("null".to_string(), Vec::new()),
            Expr::Var(name) => {
                if name == "this" && !self.method.is_static {
                    Type::Class(self.class_name.to_string(), Vec::new())
                } else {
                    self.local_types
                        .get(name)
                        .cloned()
                        .ok_or_else(|| format!("unknown variable `{}`", name))?
                }
            }
            Expr::New(class_name, type_args, _) => Type::Class(class_name.clone(), type_args.clone()),
            Expr::With(obj, _) => self.expr_ty(obj)?,
            Expr::Field(obj, name) => {
                // Intrinsic properties (`list.Count`, `s.Length`) and
                // newtype unwrap (`id.Value`).
                if let Ok(obj_ty) = self.expr_ty(obj) {
                    let stripped = Self::strip_nullable(obj_ty);
                    if let Type::Newtype(_, underlying) = &stripped {
                        if name == "Value" {
                            return Ok((**underlying).clone());
                        }
                    }
                    let prop = match &stripped {
                        Type::String | Type::StringLit(_) | Type::LiteralUnion(..) => crate::intrinsics::string_property(name),
                        Type::Class(cname, _) => crate::intrinsics::property(cname, name),
                        _ => None,
                    };
                    if let Some(p) = prop {
                        return Ok(p.ty);
                    }
                }
                let owner = self.expr_class(obj).ok_or_else(|| {
                    format!("cannot determine type of field target `{}`", name)
                })?;
                let layout = self.field_layout.get(&owner).ok_or_else(|| {
                    format!("unknown field layout for `{}`", owner)
                })?;
                layout
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| format!("unknown field `{}` on `{}`", name, owner))?
            }
            Expr::StaticField(class_name, name) => {
                let info = self.program.classes.get(class_name).ok_or_else(|| {
                    format!("unknown class `{}`", class_name)
                })?;
                info.static_fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| format!("unknown static field `{}.{}`", class_name, name))?
            }
            Expr::EnumVariant(enum_name, _, _) => Type::Enum(enum_name.clone()),
            Expr::TryUnwrap(inner) => {
                let inner_ty = self.expr_ty(inner)?;
                let enum_name = match &inner_ty {
                    Type::Class(name, _) | Type::Enum(name) => Some(name.clone()),
                    _ => None,
                };
                let name = enum_name.ok_or_else(|| "cannot determine unwrapped type of `?`".to_string())?;
                self.program.enums.get(&name)
                    .and_then(|e| e.variants.first())
                    .and_then(|v| v.fields.first())
                    .map(|f| f.1.clone())
                    .ok_or_else(|| format!("cannot determine unwrapped type of `?`"))?
            }
            Expr::Tuple(elements) => {
                let tys = elements
                    .iter()
                    .map(|e| self.expr_ty(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Type::Tuple(tys)
            }
            Expr::Binary(op, left, right) => {
                match op {
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                    | BinOp::And | BinOp::Or => Type::Bool,
                    _ => {
                        let lt = self.expr_ty(left).unwrap_or(Type::Int32);
                        let rt = self.expr_ty(right).unwrap_or(Type::Int32);
                        promote(&lt, &rt).unwrap_or(Type::Int32)
                    }
                }
            }
            Expr::Unary(op, operand) => {
                match op {
                    UnaryOp::Not => Type::Bool,
                    UnaryOp::Neg => self.expr_ty(operand).unwrap_or(Type::Int32),
                }
            }
            Expr::Call(call) => {
                self.call_return_type(call)?
            }
            Expr::NonNullAssert(inner) => {
                match self.expr_ty(inner)? {
                    Type::Nullable(t) => *t,
                    t => t,
                }
            }
            Expr::Is(..) => Type::Bool,
            Expr::NullCoalesce(left, right) => {
                match self.expr_ty(left)? {
                    Type::Nullable(t) => {
                        if matches!(self.expr_ty(right), Ok(Type::Nullable(_))) {
                            Type::Nullable(t)
                        } else {
                            *t
                        }
                    }
                    _ => self.expr_ty(right)?,
                }
            }
            Expr::Ternary(_, then_expr, else_expr) => {
                let t = self.expr_ty(then_expr)?;
                if matches!(&t, Type::Class(n, _) if n == "null") {
                    self.expr_ty(else_expr)?
                } else {
                    t
                }
            }
            _ => Type::Class("null".to_string(), Vec::new()),
        };
        Ok(ty)
    }

    /// Return type of a call expression, used by `expr_ty`.
    fn call_return_type(&self, call: &CallExpr) -> Result<Type, String> {
        let class_name;
        let method_name;
        let is_instance;
        // Type arguments of the receiver, used to substitute the return type
        // of generic instance methods (`Map<string, int>.Get` -> `int`).
        let mut receiver_type_args: Vec<Type> = Vec::new();
        if let Some(target) = &call.target {
            if let Expr::Var(c) = target.as_ref() {
                if self.class_ids.contains_key(c) {
                    class_name = c.clone();
                    is_instance = false;
                } else {
                    let obj_ty = Self::strip_nullable(self.expr_ty(target)?);
                    if matches!(obj_ty, Type::String | Type::StringLit(_) | Type::LiteralUnion(..)) {
                        return crate::intrinsics::string_method(&call.method)
                            .map(|m| m.return_ty)
                            .ok_or_else(|| format!("unknown method `{}` on `string`", call.method));
                    }
                    let Type::Class(c, args) = obj_ty else {
                        return Err("cannot determine call target class".to_string());
                    };
                    class_name = c;
                    receiver_type_args = args;
                    is_instance = true;
                }
            } else {
                let obj_ty = Self::strip_nullable(self.expr_ty(target)?);
                if matches!(obj_ty, Type::String | Type::StringLit(_) | Type::LiteralUnion(..)) {
                    return crate::intrinsics::string_method(&call.method)
                        .map(|m| m.return_ty)
                        .ok_or_else(|| format!("unknown method `{}` on `string`", call.method));
                }
                let Type::Class(c, args) = obj_ty else {
                    return Err("cannot determine call target class".to_string());
                };
                class_name = c;
                receiver_type_args = args;
                is_instance = true;
            }
            method_name = call.method.clone();
        } else if call.class_or_target == "__intrinsics" {
            return match call.method.as_str() {
                "int" => Ok(Type::Int32),
                "float" => Ok(Type::Float64),
                "string" => Ok(Type::String),
                _ => Ok(Type::Unit),
            };
        } else {
            // Newtype constructors type as the newtype itself.
            if let Some(underlying) = self.program.newtypes.get(&call.method) {
                return Ok(Type::Newtype(call.method.clone(), Box::new(underlying.clone())));
            }
            // Bare call: `foo(args)` without a class qualifier. If the
            // class_or_target is not a known class, it resolves to a static
            // method of the current class.
            class_name = if self.class_ids.contains_key(&call.class_or_target) {
                call.class_or_target.clone()
            } else {
                self.class_name.to_string()
            };
            is_instance = false;
            method_name = call.method.clone();
        }
        // Substitute the receiver's type arguments into the return type so
        // chained calls on generic receivers keep concrete types.
        let subst = self
            .program
            .classes
            .get(&class_name)
            .filter(|_| !receiver_type_args.is_empty())
            .map(|info| build_subst(&info.generic_params, &receiver_type_args))
            .unwrap_or_default();
        let mut cur = Some(class_name.clone());
        while let Some(c) = cur {
            let info = self.program.classes.get(&c).ok_or_else(|| {
                format!("unknown class `{}`", c)
            })?;
            let table = if is_instance { &info.methods } else { &info.static_methods };
            if let Some(mi) = table.get(&method_name) {
                // Mirror the typer's method-generic inference so dispatch on
                // inferred return types works (`Util.Pick(a, b).Foo()`).
                // First-binding-wins is enough here — the typer has already
                // validated and joined the bindings.
                let mut subst = subst.clone();
                if !mi.generic_params.is_empty() {
                    let vars: std::collections::HashSet<String> =
                        mi.generic_params.iter().map(|gp| gp.name.clone()).collect();
                    let mut bindings = HashMap::new();
                    for (param, arg) in mi.params.iter().zip(call.args.iter()) {
                        if let Ok(arg_ty) = self.expr_ty(arg) {
                            let p = substitute_type(param, &subst);
                            crate::typer::unify_generic_types(
                                &p,
                                &arg_ty,
                                &vars,
                                &mut bindings,
                                &mut |_, prev, _| Some(prev),
                            );
                        }
                    }
                    for (name, ty) in bindings {
                        subst.insert(name, ty);
                    }
                }
                return Ok(substitute_type(&mi.return_ty, &subst));
            }
            cur = info.super_class.clone();
        }
        Err(format!(
            "cannot determine return type of `{}.{}`",
            class_name, method_name
        ))
    }

    /// Mirror of the type checker's assignability check, used to match
    /// constructor overloads.
    fn is_assignable(&self, target: &Type, source: &Type) -> bool {
        if target == source {
            return true;
        }
        match (target, source) {
            (Type::Nullable(t), Type::Nullable(s)) => return self.is_assignable(t, s),
            (Type::Nullable(_), Type::Class(sn, _)) if sn == "null" => return true,
            (Type::Nullable(t), s) => return self.is_assignable(t, s),
            (_, Type::Nullable(_)) => return false,
            _ => {}
        }
        if target.is_numeric() && source.is_numeric() {
            return self.numeric_widening(target, source);
        }
        match (target, source) {
            (Type::Class(name, _), Type::Class(source_name, _)) => {
                if source_name == "null" {
                    return true;
                }
                self.is_subclass_of(source_name, name)
                    || crate::typer::structurally_satisfies(&self.program.classes, source_name, name)
            }
            (Type::LiteralUnion(_, target_members), Type::LiteralUnion(_, source_members)) => {
                source_members.iter().all(|m| target_members.contains(m))
            }
            (Type::String, Type::Class(source_name, _)) if source_name == "null" => true,
            (Type::Enum(a), Type::Enum(b)) => a == b,
            (Type::Class(name, _), Type::Enum(enum_name)) => name == enum_name,
            (Type::Enum(enum_name), Type::Class(name, _)) => name == enum_name,
            _ => false,
        }
    }

    /// Mirror of the type checker's implicit numeric widening rules.
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
            (Int8 | Int16 | Int32 | Int64, Int8 | Int16 | Int32 | Int64)
            | (UInt8 | UInt16 | UInt32 | UInt64, UInt8 | UInt16 | UInt32 | UInt64) => {
                bits(target) >= bits(source)
            }
            (Int8 | Int16 | Int32 | Int64, UInt8 | UInt16 | UInt32 | UInt64) => {
                bits(target) > bits(source)
            }
            (UInt8 | UInt16 | UInt32 | UInt64, Int8 | Int16 | Int32 | Int64) => false,
            (Float32 | Float64, Int8 | Int16 | Int32 | Int64)
            | (Float32 | Float64, UInt8 | UInt16 | UInt32 | UInt64) => true,
            (Float64, Float32) => true,
            _ => false,
        }
    }

    fn is_subclass_of(&self, sub: &str, sup: &str) -> bool {
        if sub == sup {
            return true;
        }
        let mut cur = sub.to_string();
        loop {
            let Some(parent) = self.program.classes.get(&cur).and_then(|c| c.super_class.clone()) else {
                return false;
            };
            if parent == sup {
                return true;
            }
            cur = parent;
        }
    }

    /// Index of a base class field; base declarations win over subclass shadows.
    fn super_field_index(&self, name: &str) -> Result<u16, String> {
        self.field_layout
            .get(self.class_name)
            .and_then(|layout| layout.iter().position(|(n, _)| n == name))
            .map(|i| i as u16)
            .ok_or_else(|| format!("unknown super field `{}`", name))
    }

    /// Find an instance property on `obj_class` or its super chain.
    fn instance_property_opt(&self, obj_class: &str, name: &str) -> Option<(String, ())> {
        let mut cur = Some(obj_class.to_string());
        while let Some(c) = cur {
            let info = self.program.classes.get(&c)?;
            if info.properties.contains_key(name) {
                return Some((c, ()));
            }
            cur = info.super_class.clone();
        }
        None
    }

    /// Find a static property on `class_name` or its super chain.
    fn static_property_opt(&self, class_name: &str, name: &str) -> Option<(String, ())> {
        let mut cur = Some(class_name.to_string());
        while let Some(c) = cur {
            let info = self.program.classes.get(&c)?;
            if info.static_properties.contains_key(name) {
                return Some((c, ()));
            }
            cur = info.super_class.clone();
        }
        None
    }

    /// Find an instance property on the current class's super chain.
    fn super_instance_property(&self, name: &str) -> Option<(String, ())> {
        let super_name = self
            .program
            .classes
            .get(self.class_name)
            .and_then(|ci| ci.super_class.clone())?;
        self.instance_property_opt(&super_name, name)
    }

    /// Resolve a static field's declaring class and index, walking the super chain.
    fn static_field_index(&self, class_name: &str, name: &str) -> Result<(String, u16), String> {
        self.static_field_index_opt(class_name, name)
            .ok_or_else(|| format!("unknown static field `{}` on `{}`", name, class_name))
    }

    fn static_field_index_opt(&self, class_name: &str, name: &str) -> Option<(String, u16)> {
        let mut cur = Some(class_name.to_string());
        while let Some(c) = cur {
            let info = self.program.classes.get(&c)?;
            if let Some(i) = info.static_fields.iter().position(|(n, _)| n == name) {
                return Some((c, i as u16));
            }
            cur = info.super_class.clone();
        }
        None
    }

    /// Look up the method id for `super.Method()`, resolving the declaring class
    /// in the super chain (non-virtual dispatch).
    fn super_method_id(&self, method: &str) -> Result<MethodId, String> {
        let super_name = self
            .program
            .classes
            .get(self.class_name)
            .and_then(|ci| ci.super_class.clone())
            .ok_or_else(|| format!("`{}` has no super class", self.class_name))?;
        let mut cur = Some(super_name.clone());
        while let Some(c) = cur {
            let info = self.program.classes.get(&c).unwrap();
            if info.methods.contains_key(method) {
                return Ok(*self
                    .method_ids
                    .get(&(c.clone(), method.to_string(), true))
                    .ok_or_else(|| format!("unknown method `{}` on `{}`", method, c))?);
            }
            cur = info.super_class.clone();
        }
        Err(format!(
            "unknown method `{}` on super class `{}`",
            method, super_name
        ))
    }
}

/// The `Conv` target needed to re-narrow an arithmetic result to its static
/// type. Native-width types (int64, uint64, float64) need no narrowing.
fn arith_conv(ty: &Type) -> Option<TypeDesc> {
    match ty {
        Type::Int8 => Some(TypeDesc::Int8),
        Type::Int16 => Some(TypeDesc::Int16),
        Type::Int32 => Some(TypeDesc::Int32),
        Type::UInt8 => Some(TypeDesc::UInt8),
        Type::UInt16 => Some(TypeDesc::UInt16),
        Type::UInt32 => Some(TypeDesc::UInt32),
        Type::Float32 => Some(TypeDesc::Float32),
        _ => None,
    }
}

fn map_type(ty: &Type, class_ids: &HashMap<String, ClassId>, enum_ids: &HashMap<String, EnumId>, generic_params: &[aura_bytecode::GenericParam]) -> TypeDesc {
    match ty {
        Type::Nullable(inner) => TypeDesc::Nullable(Box::new(map_type(inner, class_ids, enum_ids, generic_params))),
        // Newtypes are fully erased: at runtime a value of `Newtype` IS its
        // underlying primitive.
        Type::Newtype(_, inner) => map_type(inner, class_ids, enum_ids, generic_params),
        // Literal unions (and transient literal markers) are strings at
        // runtime.
        Type::LiteralUnion(..) | Type::StringLit(_) => TypeDesc::String,
        // `var` markers are replaced with inferred types before any TypeDesc
        // mapping; treat a stray one as a boxed reference slot.
        Type::Infer => TypeDesc::Null,
        Type::Unit => TypeDesc::Unit,
        // Literal marker types only exist transiently during type checking and
        // never reach code emission.
        Type::IntLit(_) | Type::FloatLit(_) => TypeDesc::Int32,
        Type::Int8 => TypeDesc::Int8,
        Type::Int16 => TypeDesc::Int16,
        Type::Int32 => TypeDesc::Int32,
        Type::Int64 => TypeDesc::Int64,
        Type::UInt8 => TypeDesc::UInt8,
        Type::UInt16 => TypeDesc::UInt16,
        Type::UInt32 => TypeDesc::UInt32,
        Type::UInt64 => TypeDesc::UInt64,
        Type::Float32 => TypeDesc::Float32,
        Type::Float64 => TypeDesc::Float64,
        Type::Bool => TypeDesc::Bool,
        Type::Char => TypeDesc::Char,
        Type::String => TypeDesc::String,
        Type::Enum(name) => {
            let enum_id = *enum_ids.get(name).unwrap_or(&EnumId(0));
            TypeDesc::Enum(enum_id)
        }
        Type::Class(name, args) => {
            if let Some(idx) = generic_params.iter().position(|gp| gp.name == *name) {
                return TypeDesc::GenericParam(idx as u32);
            }
            if let Some(enum_id) = enum_ids.get(name) {
                // Enum (or sum-type) reference; runtime type arguments are not
                // needed since enum values carry only their variant and fields.
                return TypeDesc::Enum(*enum_id);
            }
            let class_id = *class_ids.get(name).unwrap_or(&ClassId(0));
            let mapped_args: Vec<TypeDesc> = args.iter().map(|a| map_type(a, class_ids, enum_ids, generic_params)).collect();
            TypeDesc::Class(class_id, mapped_args)
        }
        Type::GenericParam(name) => {
            if let Some(idx) = generic_params.iter().position(|gp| gp.name == *name) {
                TypeDesc::GenericParam(idx as u32)
            } else {
                TypeDesc::GenericParam(0)
            }
        }
        Type::Tuple(types) => {
            let mapped_types: Vec<TypeDesc> = types.iter().map(|t| map_type(t, class_ids, enum_ids, generic_params)).collect();
            TypeDesc::Tuple(mapped_types)
        }
    }
}
