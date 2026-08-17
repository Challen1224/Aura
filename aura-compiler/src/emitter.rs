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
/// An entry in the emitter's enclosing-finally stack, used to run
/// `finally` bodies before abrupt jumps (`return`, `break`, `continue`)
/// leave their protected regions.
#[derive(Clone)]
enum FinallyCtx {
    /// A `finally` block's statements (re-emitted inline at each exit).
    Finally(Vec<Stmt>),
    /// A fixed op sequence (the `using` Dispose call, whose resource slot
    /// is already allocated).
    Ops(Vec<Op>),
    /// A loop boundary (with its label): `break`/`continue` drain
    /// finallys only down to their target loop.
    LoopBoundary(Option<String>),
}

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
    /// Source line of the statement currently being emitted (from the
    /// parser's `Stmt::Mark` markers), for error locations.
    current_line: usize,
    /// Debug line table under construction: `(first_op_index, line)`
    /// recorded whenever the current statement line changes.
    line_starts: Vec<(u32, u32)>,
    /// Debug local-name table: slot index -> source name (last lexical
    /// binding wins when scopes reuse a slot).
    debug_names: Vec<String>,
    /// Enclosing `finally` blocks and loop boundaries, innermost last.
    finally_stack: Vec<FinallyCtx>,
    /// Scratch scope for typing lambda bodies from `&self` contexts: name
    /// -> type pairs consulted (last wins) before `local_types`.
    type_overlay: std::cell::RefCell<Vec<(String, Type)>>,
    /// Shared allocator for lifted-lambda method ids.
    lambda_counter: &'a std::cell::RefCell<u32>,
    /// Lambdas lifted out of this body: synthesized static methods
    /// (captures as leading parameters) awaiting compilation.
    lifted: Vec<(MethodId, MethodDecl)>,
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
                                init: None,
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

        // Lambda lifting: lambdas encountered during body emission reserve
        // fresh method ids from this counter and queue their synthesized
        // static methods here; each class drains its queue after its
        // declared members (nested lambdas re-fill it).
        let lambda_counter = std::cell::RefCell::new(self.next_method_id);
        let lifted_cell: std::cell::RefCell<Vec<(MethodId, MethodDecl)>> =
            std::cell::RefCell::new(Vec::new());

        let mut entrypoint: Option<MethodId> = None;
        for decl in &program.program.decls {
            if let Decl::Class(class) = decl {
                let class_id = *class_ids.get(&class.name).unwrap();
                let info = program.classes.get(&class.name).unwrap();

                let class_generic_params: Vec<aura_bytecode::GenericParam> = class.generic_params.iter().map(|gp| {
                    aura_bytecode::GenericParam {
                        name: gp.name.clone(),
                        constraint: gp.bounds.first().map(|c| map_type(c, &class_ids, &enum_ids, &[])),
                        variance: match gp.variance {
                            crate::ast::Variance::Covariant => aura_bytecode::Variance::Covariant,
                            crate::ast::Variance::Contravariant => aura_bytecode::Variance::Contravariant,
                            crate::ast::Variance::Invariant => aura_bytecode::Variance::Invariant,
                        },
                    }
                }).collect();
                let fields: Vec<FieldDef> = field_layouts[&class.name]
                    .iter()
                    .map(|(name, ty)| FieldDef {
                        name: name.clone(),
                        ty: map_type(ty, &class_ids, &enum_ids, &class_generic_params),
                        init: None,
                    })
                    .collect();
                let static_fields: Vec<FieldDef> = info
                    .static_fields
                    .iter()
                    .map(|(name, ty)| FieldDef {
                        name: name.clone(),
                        ty: map_type(ty, &class_ids, &enum_ids, &class_generic_params),
                        init: static_field_init(class, name),
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
                                &lambda_counter, &lifted_cell,
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
                                        &lambda_counter, &lifted_cell,
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
                                        &lambda_counter, &lifted_cell,
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
                        &lambda_counter, &lifted_cell,
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
                        &lambda_counter, &lifted_cell,
                    )?;
                    methods.insert(default_id, method_def);
                }

                // Compile lambdas lifted out of this class's bodies (a
                // lifted body may itself contain lambdas, so loop).
                loop {
                    let pending: Vec<(MethodId, MethodDecl)> = {
                        let mut cell = lifted_cell.borrow_mut();
                        cell.drain(..).collect()
                    };
                    if pending.is_empty() {
                        break;
                    }
                    for (lid, decl) in pending {
                        let (lid, def) = self.build_method_def(
                            program, class, &decl, info, class_id, lid, &method_ids,
                            &class_ids, &enum_ids, &variant_lookup, &field_layouts,
                            &class_generic_params, &constructor_ids, &mut module,
                            &lambda_counter, &lifted_cell,
                        )?;
                        static_methods.insert(lid, def);
                    }
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
        lambda_counter: &std::cell::RefCell<u32>,
        lifted_out: &std::cell::RefCell<Vec<(MethodId, MethodDecl)>>,
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
            lambda_counter,
        );
        me.emit_body()?;
        lifted_out.borrow_mut().extend(me.lifted.drain(..));

        let mut method_def = MethodDef {
            name: m.name.clone(),
            return_ty: map_type(&m.return_ty, class_ids, enum_ids, class_generic_params),
            params: m.params.iter().map(|p| map_type(&p.ty, class_ids, enum_ids, class_generic_params)).collect(),
            generic_params: m.generic_params.iter().map(|gp| {
                aura_bytecode::GenericParam {
                    name: gp.name.clone(),
                    constraint: gp.bounds.first().map(|c| map_type(c, class_ids, enum_ids, class_generic_params)),
                    variance: aura_bytecode::Variance::Invariant,
                }
            }).collect(),
            is_instance: !m.is_static,
            is_async: m.is_async,
            body: me.ops,
            handlers: me.handlers,
            max_stack: 8,
            locals: me.max_locals as u16,
            line_starts: me.line_starts,
            local_names: me.debug_names,
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
        is_async: false,
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
        throws: Vec::new(),
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
        is_async: false,
        constructor_chain: None,
        generic_params: Vec::new(),
        return_ty: Type::Unit,
        name: class.name.clone(),
        params: class.record_params.clone(),
        throws: Vec::new(),
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
        lambda_counter: &'a std::cell::RefCell<u32>,
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
            debug_names: locals.clone(),
            locals,
            max_locals: init_locals,
            params,
            local_types,
            constants: Vec::new(),
            method_ids,
            constructor_ids,
            finally_stack: Vec::new(),
            type_overlay: std::cell::RefCell::new(Vec::new()),
            lambda_counter,
            lifted: Vec::new(),
            break_targets: Vec::new(),
            continue_targets: Vec::new(),
            handlers: Vec::new(),
            current_line: 0,
            line_starts: Vec::new(),
        }
    }

    fn emit_body(&mut self) -> Result<(), String> {
        self.emit_body_inner().map_err(|e| {
            if self.current_line > 0 && !e.starts_with("line ") {
                format!(
                    "line {}, in `{}.{}`: {}",
                    self.current_line, self.class_name, self.method.name, e
                )
            } else if !e.starts_with("line ") {
                format!("in `{}.{}`: {}", self.class_name, self.method.name, e)
            } else {
                e
            }
        })
    }

    fn emit_body_inner(&mut self) -> Result<(), String> {
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

    /// Emit the enclosing `finally` bodies an abrupt jump exits, innermost
    /// first. `stop_at_loop`: `None` drains everything (a `return`);
    /// `Some(label)` drains down to the matching loop boundary (`break`/
    /// `continue`; unlabeled stops at the innermost boundary). Each body is
    /// emitted with the stack truncated below it, so its own abrupt
    /// statements re-run only the finallys outside it.
    fn emit_pending_finallys(
        &mut self,
        stop_at_loop: Option<&Option<String>>,
    ) -> Result<(), String> {
        let snapshot = self.finally_stack.clone();
        let mut i = snapshot.len();
        while i > 0 {
            i -= 1;
            match &snapshot[i] {
                FinallyCtx::LoopBoundary(loop_label) => match stop_at_loop {
                    Some(target) => {
                        let matches = match target {
                            None => true,
                            Some(l) => loop_label.as_ref() == Some(l),
                        };
                        if matches {
                            break;
                        }
                    }
                    None => {}
                },
                FinallyCtx::Finally(body) => {
                    self.finally_stack = snapshot[..i].to_vec();
                    let body = body.clone();
                    for st in &body {
                        self.emit_stmt(st)?;
                    }
                }
                FinallyCtx::Ops(ops) => {
                    self.ops.extend(ops.iter().cloned());
                }
            }
        }
        self.finally_stack = snapshot;
        Ok(())
    }

    /// True when any enclosing finally would run before a `return`.
    fn has_pending_finally(&self) -> bool {
        self.finally_stack
            .iter()
            .any(|f| matches!(f, FinallyCtx::Finally(_) | FinallyCtx::Ops(_)))
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::VarDecl(ty, name, init) => {
                if let Some(init) = init {
                    let target = if matches!(ty, Type::Infer) { None } else { Some(ty) };
                    self.emit_expr_with_target(init, target)?;
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
                let target_ty: Option<Type> = match target {
                    AssignTarget::Local(name) => self.local_types.get(name).cloned(),
                    _ => None,
                };
                self.emit_expr_with_target(value, target_ty.as_ref())?;
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
            Stmt::Mark(line) => {
                self.current_line = *line;
                let here = (self.ops.len() as u32, *line as u32);
                match self.line_starts.last_mut() {
                    // Several marks can land before any op is emitted
                    // (nested blocks): keep the latest line for that pc.
                    Some(last) if last.0 == here.0 => *last = here,
                    Some(last) if last.1 == here.1 => {}
                    _ => self.line_starts.push(here),
                }
            }
            Stmt::Return(Some(e)) => {
                let ret = self.method.return_ty.clone();
                self.emit_expr_with_target(e, Some(&ret))?;
                if self.has_pending_finally() {
                    // The return value is computed before the finally
                    // bodies run (C# order); stash it across them.
                    let tmp = self.push_local("__ret_temp".to_string()) as u16;
                    self.ops.push(Op::Stloc(tmp));
                    self.emit_pending_finallys(None)?;
                    self.ops.push(Op::Ldloc(tmp));
                }
                self.ops.push(Op::Ret);
            }
            Stmt::Return(None) => {
                if self.has_pending_finally() {
                    self.emit_pending_finallys(None)?;
                }
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
        self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
                self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
                
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
                self.finally_stack.pop();
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
        self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
                self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
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
                self.finally_stack.pop();
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
                // Register the loop variable's type: expression typing
                // (constructor overload resolution, operator lowering)
                // consults `local_types`, not just the slot list.
                let loop_var_ty = if matches!(var_type, Type::Infer) {
                    self.expr_ty(start).unwrap_or(Type::Int32)
                } else {
                    var_type.clone()
                };
                self.local_types.insert(var_name.clone(), loop_var_ty);
                self.ops.push(Op::Stloc(var_idx));
                
                let loop_start = self.ops.len() as u32;
                self.break_targets.push((label.clone(), Vec::new()));
        self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
                self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
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
                self.finally_stack.pop();
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
        self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
                self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
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
                self.finally_stack.pop();
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
                self.emit_pending_finallys(Some(label))?;
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
                self.emit_pending_finallys(Some(label))?;
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
                // Abrupt jumps out of the try/catch bodies must run this
                // finally first (drained by return/break/continue).
                if let Some(fb) = finally_body {
                    self.finally_stack.push(FinallyCtx::Finally(fb.clone()));
                }
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
                if finally_body.is_some() {
                    self.finally_stack.pop();
                }
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

                self.finally_stack.push(FinallyCtx::Ops(vec![
                    Op::Ldloc(resource_local),
                    Op::CallVirt("Dispose".to_string()),
                    Op::Pop,
                ]));
                let try_start = self.ops.len() as u32;
                for s in body {
                    self.emit_stmt(s)?;
                }
                let try_end = self.ops.len() as u32;
                let normal_jump = self.ops.len();
                self.ops.push(Op::Br(0));
                self.finally_stack.pop();

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
            Expr::Hole => {
                return Err("type hole reached code emission (checker bug)".to_string());
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
                    // Instance methods carry `this` in slot 0; a lifted
                    // lambda that captured it carries it as a leading
                    // parameter (also slot 0 by capture ordering).
                    if let Some(idx) = self.locals.iter().rposition(|n| n == "this") {
                        self.ops.push(Op::Ldloc(idx as u16));
                    } else {
                        return Err("`this` in static method".to_string());
                    }
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
            Expr::CustomOp(sym, left, right) => {
                // Custom operators always lower to a method call on the left
                // operand (the typer guaranteed the overload exists); same
                // temp dance as overloaded built-ins for evaluation order.
                let mname = format!("operator{}", sym);
                let tmp = self.push_local("__op_lhs".to_string()) as u16;
                self.emit_expr(left)?;
                self.ops.push(Op::Stloc(tmp));
                self.emit_expr(right)?;
                self.ops.push(Op::Ldloc(tmp));
                self.ops.push(Op::CallVirt(mname));
            }
            Expr::Binary(op, left, right) => {
                // User-defined operator: `a + b` is a method call on `a`.
                // `CallVirt` wants arguments below the receiver, so the left
                // operand is stashed in a temp — which also keeps
                // left-to-right evaluation order. `!=` negates `operator==`.
                if let Some((mname, _)) = self.user_operator(*op, left) {
                    let tmp = self.push_local("__op_lhs".to_string()) as u16;
                    self.emit_expr(left)?;
                    self.ops.push(Op::Stloc(tmp));
                    self.emit_expr(right)?;
                    self.ops.push(Op::Ldloc(tmp));
                    self.ops.push(Op::CallVirt(mname));
                    if matches!(op, BinOp::Ne) {
                        self.ops.push(Op::Not);
                    }
                    return Ok(());
                }
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
            Expr::Lambda { params, body } => {
                self.emit_lambda(params, body, None)?;
            }
            Expr::Await(inner) => {
                self.emit_expr(inner)?;
                self.ops.push(Op::Await);
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
                        // `t.wait()` drives the scheduler: a dedicated op,
                        // not a native (it must be able to re-raise the
                        // task's exception through the catch machinery).
                        if call.method == "wait" && call.args.is_empty() {
                            if let Ok(Type::Class(n, _)) =
                                self.expr_ty(target).map(Self::strip_nullable)
                            {
                                if n == "Task" {
                                    self.emit_expr(target)?;
                                    self.ops.push(Op::TaskWait);
                                    return Ok(());
                                }
                            }
                        }
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
                                self.emit_call_args(call)?;
                                if self.callee_is_async(class_name, &call.method) {
                                    self.ops.push(Op::Spawn(method_id));
                                } else {
                                    self.ops.push(Op::Call(method_id));
                                }
                            } else {
                                if let Some(ext) = self.extension_call_owner(target, &call.method) {
                                    // Extension call: static `Ext.Method(receiver, args...)`
                                    // — the receiver is the leading `__self` parameter.
                                    self.emit_expr(target)?;
                                    for arg in &call.args {
                                        self.emit_expr(arg)?;
                                    }
                                    let method_id = *self
                                        .method_ids
                                        .get(&(ext.clone(), call.method.clone(), false))
                                        .ok_or_else(|| format!(
                                            "unknown extension method `{}.{}`",
                                            ext, call.method
                                        ))?;
                                    self.ops.push(Op::Call(method_id));
                                    return Ok(());
                                }
                                // Instance call on local/field variable.
                                // Push arguments first, then the instance.
                                self.emit_call_args(call)?;
                                self.emit_expr(target)?;
                                self.ops.push(Op::CallVirt(call.method.clone()));
                            }
                        } else {
                            if let Some(ext) = self.extension_call_owner(target, &call.method) {
                                // Extension call: static `Ext.Method(receiver, args...)`
                                // — the receiver is the leading `__self` parameter.
                                self.emit_expr(target)?;
                                for arg in &call.args {
                                    self.emit_expr(arg)?;
                                }
                                let method_id = *self
                                    .method_ids
                                    .get(&(ext.clone(), call.method.clone(), false))
                                    .ok_or_else(|| format!(
                                        "unknown extension method `{}.{}`",
                                        ext, call.method
                                    ))?;
                                self.ops.push(Op::Call(method_id));
                                return Ok(());
                            }
                            // Push arguments first, then the instance.
                            self.emit_call_args(call)?;
                            self.emit_expr(target)?;
                            self.ops.push(Op::CallVirt(call.method.clone()));
                        }
                    } else if matches!(
                        self.local_types.get(&call.class_or_target),
                        Some(Type::Func(..))
                    ) {
                        // Calling a function-typed local: `f(2)` — arguments,
                        // then the closure on top, then Invoke.
                        let Some(Type::Func(ps, _)) =
                            self.local_types.get(&call.class_or_target).cloned()
                        else {
                            unreachable!()
                        };
                        for (arg, pty) in call.args.iter().zip(&ps) {
                            self.emit_expr_with_target(arg, Some(pty))?;
                        }
                        let idx = self.local_index(&call.class_or_target)?;
                        self.ops.push(Op::Ldloc(idx));
                        self.ops.push(Op::Invoke(call.args.len() as u16));
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
                            self.emit_call_args(call)?;
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
                            if self.callee_is_async(&owner, &call.method) {
                                self.ops.push(Op::Spawn(method_id));
                            } else {
                                self.ops.push(Op::Call(method_id));
                            }
                        }
                    }
                }
            }
            Expr::New(class_name, type_args, args) => {
                // Intrinsic constructors (`new List<int>()`, `new
                // WeakRef(obj)`) lower to a native call; arguments (if the
                // intrinsic declares any) are pushed in order first.
                if let Some(ctor) = crate::intrinsics::constructor_native(class_name) {
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.ops.push(Op::NativeCall(ctor as u16));
                    return Ok(());
                }
                // Inferred instantiation (`new Box(42)`): resolve the same
                // type arguments the typer inferred so constructor overload
                // matching and the NewObj type descriptors line up with the
                // explicit form.
                let inferred_args;
                let type_args: &Vec<Type> = if type_args.is_empty() {
                    match self.infer_new_type_args(class_name, args) {
                        Some(v) => {
                            inferred_args = v;
                            &inferred_args
                        }
                        None => type_args,
                    }
                } else {
                    type_args
                };
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
                    // Clear the temp so it doesn't pin the object until the
                    // frame exits (observable through weak references).
                    self.ops.push(Op::LdNull);
                    self.ops.push(Op::Stloc(obj_local));
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
        let slot = self.locals.len();
        if self.debug_names.len() <= slot {
            self.debug_names.resize(slot + 1, String::new());
        }
        self.debug_names[slot] = name.clone();
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
        self.finally_stack.push(FinallyCtx::LoopBoundary(label.clone()));
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
                self.finally_stack.pop();
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
                    // A lifted lambda that captured `this` carries it as an
                    // ordinary typed parameter; instance methods answer with
                    // the enclosing class.
                    if let Some(Type::Class(c, _)) = self.local_types.get("this") {
                        Some(c.clone())
                    } else if self.method.is_static {
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
                let mut t = layout[idx].1.clone();
                // Substitute the receiver's instantiation for generic fields.
                if let Ok(Type::Class(rn, rargs)) = self.expr_ty(obj) {
                    if !rargs.is_empty() {
                        if let Some(rinfo) = self.program.classes.get(&rn) {
                            t = substitute_type(&t, &build_subst(&rinfo.generic_params, &rargs));
                        }
                    }
                }
                match Self::strip_nullable(t) {
                    Type::Class(c, _) => Some(c),
                    _ => None,
                }
            }
            // Anything else (operator-call results, chained calls, ...):
            // fall back to full expression typing.
            _ => match self.expr_ty(expr).map(Self::strip_nullable) {
                Ok(Type::Class(c, _)) => Some(c),
                _ => None,
            },
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

    /// Emit an expression that may be a target-typed lambda.
    fn emit_expr_with_target(&mut self, e: &Expr, target: Option<&Type>) -> Result<(), String> {
        if let Expr::Lambda { params, body } = e {
            return self.emit_lambda(params, body, target);
        }
        self.emit_expr(e)
    }

    /// Compile a lambda: lift it to a fresh static method whose leading
    /// parameters are the captured locals (`this` first), queue that method
    /// for emission, and emit `NewClosure` over the captured values here.
    fn emit_lambda(
        &mut self,
        params: &[(String, Option<Type>)],
        body: &[Stmt],
        target: Option<&Type>,
    ) -> Result<(), String> {
        let exp: Option<(Vec<Type>, Type)> = match target {
            Some(Type::Func(p, r)) => Some((p.clone(), (**r).clone())),
            Some(Type::Nullable(inner)) => match inner.as_ref() {
                Type::Func(p, r) => Some((p.clone(), (**r).clone())),
                _ => None,
            },
            _ => None,
        };
        let mut ptys: Vec<Type> = Vec::with_capacity(params.len());
        for (i, (n, ann)) in params.iter().enumerate() {
            let t = match (ann, &exp) {
                (Some(t), _) => t.clone(),
                (None, Some((eps, _))) => eps
                    .get(i)
                    .cloned()
                    .ok_or_else(|| format!("lambda parameter `{}` has no matching target", n))?,
                (None, None) => {
                    return Err(format!(
                        "cannot determine the type of lambda parameter `{}`",
                        n
                    ));
                }
            };
            ptys.push(t);
        }
        // An expected return type naming an unbound method type parameter
        // (two-pass inference left it open) is resolved from the body, like
        // the no-target case.
        let ret_open = |t: &Type| -> bool {
            matches!(t, Type::GenericParam(_))
                || matches!(t, Type::Class(n, a) if a.is_empty()
                    && !self.program.classes.contains_key(n)
                    && !self.program.enums.contains_key(n)
                    && !self.program.newtypes.contains_key(n))
        };
        let expected_ret = exp.as_ref().map(|(_, r)| r.clone());
        let ret = match expected_ret {
            Some(r) if !ret_open(&r) => r,
            _ => {
                let [Stmt::Return(Some(e))] = body else {
                    return Err("block-bodied lambda needs a target type".to_string());
                };
                let mut saved: Vec<(String, Option<Type>)> = Vec::new();
                for ((n, _), t) in params.iter().zip(&ptys) {
                    saved.push((n.clone(), self.local_types.insert(n.clone(), t.clone())));
                }
                let r = self.expr_ty(e).map(widen_literal).unwrap_or(Type::Unit);
                for (n, old) in saved {
                    match old {
                        Some(v) => {
                            self.local_types.insert(n, v);
                        }
                        None => {
                            self.local_types.remove(&n);
                        }
                    }
                }
                r
            }
        };
        // Captures: free variables of the body naming enclosing locals,
        // `this` first (so field-access shortcuts that assume slot 0 keep
        // working in the lifted body), then declaration order.
        let bound: std::collections::HashSet<String> =
            params.iter().map(|(n, _)| n.clone()).collect();
        let mut free: Vec<String> = Vec::new();
        collect_free_vars_stmts(body, &bound, &mut free);
        let mut captures: Vec<String> = Vec::new();
        if free.iter().any(|n| n == "this") && self.locals.iter().any(|l| l == "this") {
            captures.push("this".to_string());
        }
        for l in &self.locals {
            if l != "this" && free.iter().any(|f| f == l) && !captures.contains(l) {
                captures.push(l.clone());
            }
        }
        let id = {
            let mut c = self.lambda_counter.borrow_mut();
            let v = *c;
            *c += 1;
            MethodId(v)
        };
        let mut lifted_params: Vec<crate::ast::Param> = Vec::new();
        for c in &captures {
            let ty = self
                .local_types
                .get(c)
                .cloned()
                .unwrap_or_else(|| Type::Class(self.class_name.to_string(), Vec::new()));
            lifted_params.push(crate::ast::Param { name: c.clone(), ty });
        }
        for ((n, _), t) in params.iter().zip(&ptys) {
            lifted_params.push(crate::ast::Param { name: n.clone(), ty: t.clone() });
        }
        // `Action` with an expression body: the expression is a statement.
        let lifted_body: Vec<Stmt> = if ret == Type::Unit {
            if let [Stmt::Return(Some(e))] = body {
                vec![Stmt::Expr(e.clone())]
            } else {
                body.to_vec()
            }
        } else {
            body.to_vec()
        };
        self.lifted.push((
            id,
            MethodDecl {
                is_static: true,
                visibility: crate::ast::Visibility::Public,
                is_virtual: false,
                is_override: false,
                is_abstract: false,
                is_final: false,
                is_constructor: false,
                constructor_chain: None,
                generic_params: Vec::new(),
                return_ty: ret,
                name: format!("__lambda{}", id.0),
                params: lifted_params,
                throws: Vec::new(),
                is_async: false,
                body: lifted_body,
            },
        ));
        for c in &captures {
            self.emit_expr(&Expr::Var(c.clone()))?;
        }
        self.ops.push(Op::NewClosure(id, captures.len() as u16));
        Ok(())
    }

    /// Whether the named static method is `async` (its calls spawn tasks).
    fn callee_is_async(&self, owner: &str, method: &str) -> bool {
        self.program
            .classes
            .get(owner)
            .and_then(|c| c.static_methods.get(method))
            .map(|mi| mi.is_async)
            .unwrap_or(false)
    }

    /// A receiver typed as a generic parameter resolves through its
    /// bounds: the first bound whose class (or a base/extended interface)
    /// declares the method answers. Mirrors the typer's bounded
    /// polymorphism.
    fn bounded_receiver(&self, name: &str, method: &str) -> Option<(String, Vec<Type>)> {
        let gp = self
            .method
            .generic_params
            .iter()
            .chain(self.class_info.generic_params.iter())
            .find(|g| g.name == name)?;
        gp.bounds.iter().find_map(|b| {
            let Type::Class(cn, cargs) = b else { return None };
            let mut queue = vec![cn.clone()];
            let mut seen = std::collections::HashSet::new();
            while let Some(k) = queue.pop() {
                if !seen.insert(k.clone()) {
                    continue;
                }
                let Some(info) = self.program.classes.get(&k) else {
                    continue;
                };
                if info.methods.contains_key(method) {
                    return Some((cn.clone(), cargs.clone()));
                }
                if let Some(sup) = &info.super_class {
                    queue.push(sup.clone());
                }
                queue.extend(info.interfaces.iter().cloned());
            }
            None
        })
    }

    /// Parameter types of the method a call resolves to, with the
    /// receiver's class-level substitution applied — used to target-type
    /// lambda arguments. Mirrors `call_return_type`'s resolution.
    fn call_param_types(&self, call: &CallExpr) -> Option<Vec<Type>> {
        if !call.args.iter().any(|a| matches!(a, Expr::Lambda { .. })) {
            return None;
        }
        let (class_name, is_instance, receiver_type_args): (String, bool, Vec<Type>) =
            if let Some(t) = &call.target {
                if let Expr::Var(c) = t.as_ref() {
                    if self.class_ids.contains_key(c) {
                        (c.clone(), false, Vec::new())
                    } else {
                        match Self::strip_nullable(self.expr_ty(t).ok()?) {
                            Type::Class(c, args) => (c, true, args),
                            _ => return None,
                        }
                    }
                } else {
                    match Self::strip_nullable(self.expr_ty(t).ok()?) {
                        Type::Class(c, args) => (c, true, args),
                        _ => return None,
                    }
                }
            } else if self.class_ids.contains_key(&call.class_or_target) {
                (call.class_or_target.clone(), false, Vec::new())
            } else {
                (self.class_name.to_string(), false, Vec::new())
            };
        let subst = self
            .program
            .classes
            .get(&class_name)
            .filter(|_| !receiver_type_args.is_empty())
            .map(|info| build_subst(&info.generic_params, &receiver_type_args))
            .unwrap_or_default();
        let mut cur = Some(class_name);
        while let Some(c) = cur {
            let info = self.program.classes.get(&c)?;
            let table = if is_instance { &info.methods } else { &info.static_methods };
            if let Some(mi) = table.get(&call.method) {
                // Mirror the typer's two-pass inference: non-lambda
                // arguments bind the method's type parameters before the
                // lambda targets are computed.
                let mut subst = subst.clone();
                if !mi.generic_params.is_empty() {
                    let vars: std::collections::HashSet<String> =
                        mi.generic_params.iter().map(|g| g.name.clone()).collect();
                    let mut bindings = HashMap::new();
                    for (param, arg) in mi.params.iter().zip(call.args.iter()) {
                        if matches!(arg, Expr::Lambda { .. }) {
                            continue;
                        }
                        if let Ok(at) = self.expr_ty(arg) {
                            let p = substitute_type(param, &subst);
                            crate::typer::unify_generic_types(
                                &p,
                                &widen_literal(at),
                                &vars,
                                &mut bindings,
                                &mut |_, prev, _| Some(prev),
                            );
                        }
                    }
                    for (k, v) in bindings {
                        subst.insert(k, v);
                    }
                }
                return Some(
                    mi.params
                        .iter()
                        .map(|p| substitute_type(p, &subst))
                        .collect(),
                );
            }
            cur = info.super_class.clone();
        }
        None
    }

    /// Emit call arguments, target-typing any lambda arguments by the
    /// resolved parameter types.
    fn emit_call_args(&mut self, call: &CallExpr) -> Result<(), String> {
        let ptys = self.call_param_types(call);
        for (i, arg) in call.args.iter().enumerate() {
            let target = ptys.as_ref().and_then(|p| p.get(i));
            self.emit_expr_with_target(arg, target)?;
        }
        Ok(())
    }

    /// Mirror of the typer's construction-site inference (`new Box(42)` ->
    /// `Box<int>`): unify constructor parameters against argument types.
    /// The typer has already validated the call; first-binding-wins is
    /// enough here.
    fn infer_new_type_args(&self, class_name: &str, args: &[Expr]) -> Option<Vec<Type>> {
        let info = self.program.classes.get(class_name)?;
        if info.generic_params.is_empty() {
            return None;
        }
        let widen = |t: Type| match t {
            Type::IntLit(v) => {
                if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
                    Type::Int32
                } else {
                    Type::Int64
                }
            }
            Type::FloatLit(_) => Type::Float64,
            Type::StringLit(_) => Type::String,
            other => other,
        };
        let vars: std::collections::HashSet<String> =
            info.generic_params.iter().map(|g| g.name.clone()).collect();
        for ctor in &info.constructors {
            if ctor.params.len() != args.len() {
                continue;
            }
            let mut bindings = HashMap::new();
            for (param, arg) in ctor.params.iter().zip(args) {
                if let Ok(at) = self.expr_ty(arg) {
                    crate::typer::unify_generic_types(
                        param,
                        &widen(at),
                        &vars,
                        &mut bindings,
                        &mut |_, prev, _| Some(prev),
                    );
                }
            }
            if let Some(all) = info
                .generic_params
                .iter()
                .map(|gp| bindings.get(&gp.name).cloned())
                .collect::<Option<Vec<Type>>>()
            {
                return Some(all);
            }
        }
        None
    }

    /// The extension class defining `method` for the target `key`, if any.
    fn extension_class(&self, key: &str, method: &str) -> Option<String> {
        self.program.extensions.get(key)?.get(method).cloned()
    }

    /// Return type of the extension method resolving `key.method`, if any.
    fn extension_return_ty(&self, key: &str, method: &str) -> Option<Type> {
        let ext = self.extension_class(key, method)?;
        self.program
            .classes
            .get(&ext)
            .and_then(|c| c.static_methods.get(method))
            .map(|mi| mi.return_ty.clone())
    }

    /// Decide whether `target.method(...)` is an extension-method call: the
    /// receiver's static type has no real method of that name, and an
    /// extension on the receiver's class (or a superclass) or on `string`
    /// defines it. Returns the extension class to `Call` statically.
    fn extension_call_owner(&self, target: &Expr, method: &str) -> Option<String> {
        let ty = self.expr_ty(target).ok()?;
        match ty {
            Type::String | Type::StringLit(_) | Type::LiteralUnion(..) => {
                if crate::intrinsics::string_method(method).is_some() {
                    return None;
                }
                self.extension_class("string", method)
            }
            Type::Class(name, _) => {
                // A real (possibly inherited) instance method always wins.
                let mut cur = Some(name.clone());
                while let Some(k) = cur {
                    let info = self.program.classes.get(&k)?;
                    if info.methods.contains_key(method) {
                        return None;
                    }
                    cur = info.super_class.clone();
                }
                let mut cur = Some(name);
                while let Some(k) = cur {
                    if let Some(ext) = self.extension_class(&k, method) {
                        return Some(ext);
                    }
                    cur = self.program.classes.get(&k).and_then(|i| i.super_class.clone());
                }
                None
            }
            _ => None,
        }
    }

    /// User-defined operator on the left operand's class (or a base): the
    /// method name to `CallVirt` and its return type with the receiver's
    /// type arguments substituted. The typer has already validated the
    /// operand types; this only decides the lowering.
    fn user_operator(&self, op: BinOp, left: &Expr) -> Option<(String, Type)> {
        self.user_operator_named(crate::typer::operator_method_name(op)?, left)
    }

    /// [`Self::user_operator`] by explicit method name (custom operators).
    fn user_operator_named(&self, mname: &str, left: &Expr) -> Option<(String, Type)> {
        let Ok(Type::Class(name, type_args)) = self.expr_ty(left) else {
            return None;
        };
        let mut cur = name.clone();
        loop {
            let info = self.program.classes.get(&cur)?;
            if let Some(mi) = info.methods.get(mname) {
                let receiver = self.program.classes.get(&name)?;
                let subst = crate::typer::build_subst(&receiver.generic_params, &type_args);
                return Some((
                    mname.to_string(),
                    crate::typer::substitute_type(&mi.return_ty, &subst),
                ));
            }
            cur = info.super_class.clone()?;
        }
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
                if let Some((_, t)) = self
                    .type_overlay
                    .borrow()
                    .iter()
                    .rev()
                    .find(|(n, _)| n == name)
                {
                    t.clone()
                } else if name == "this" && !self.method.is_static {
                    Type::Class(self.class_name.to_string(), Vec::new())
                } else {
                    self.local_types
                        .get(name)
                        .cloned()
                        .ok_or_else(|| format!("unknown variable `{}`", name))?
                }
            }
            Expr::New(class_name, type_args, args) => {
                // Constructor calls without explicit type arguments carry
                // their inferred instantiation (`new Box(42)` -> Box<int>).
                if type_args.is_empty() {
                    if let Some(inferred) = self.infer_new_type_args(class_name, args) {
                        return Ok(Type::Class(class_name.clone(), inferred));
                    }
                }
                Type::Class(class_name.clone(), type_args.clone())
            }
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
                let raw = layout
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| format!("unknown field `{}` on `{}`", name, owner))?;
                // Substitute the receiver's instantiation: field `k` of a
                // `Pair<string, float>` is a string, not `K`.
                if let Ok(Type::Class(rn, rargs)) = self.expr_ty(obj) {
                    if !rargs.is_empty() {
                        if let Some(rinfo) = self.program.classes.get(&rn) {
                            let subst = build_subst(&rinfo.generic_params, &rargs);
                            return Ok(substitute_type(&raw, &subst));
                        }
                    }
                }
                raw
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
            Expr::CustomOp(sym, left, _) => {
                // Needed so chained custom operators (`a |> f |> g`) can
                // resolve the outer overload from the inner result type.
                match self.user_operator_named(&format!("operator{}", sym), left) {
                    Some((_, ret)) => ret,
                    None => return Err(format!("no `operator{}` on left operand", sym)),
                }
            }
            Expr::Binary(op, left, right) => {
                match op {
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                    | BinOp::And | BinOp::Or => Type::Bool,
                    _ => {
                        if let Some((_, ret)) = self.user_operator(*op, left) {
                            ret
                        } else {
                            let lt = self.expr_ty(left).unwrap_or(Type::Int32);
                            let rt = self.expr_ty(right).unwrap_or(Type::Int32);
                            match promote(&lt, &rt) {
                                Some(t) => t,
                                // Type parameters (`T + T` under a numeric
                                // union constraint) keep their own type: the
                                // runtime op is dynamic, and the result must
                                // not be re-narrowed with a `Conv`.
                                None if lt == rt => lt,
                                None => Type::Int32,
                            }
                        }
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
            Expr::Await(inner) => match self.expr_ty(inner)? {
                Type::Class(n, args) if n == "Task" && args.len() == 1 => args[0].clone(),
                other => {
                    return Err(format!("`await` needs a Task<T>, got {}", other.name()));
                }
            },
            Expr::Lambda { params, body } => {
                // Only self-sufficient lambdas can be typed contextlessly:
                // annotated parameters with an expression body. The
                // parameters become visible to the body through the type
                // overlay (expr_ty takes `&self`).
                let mut ptys = Vec::with_capacity(params.len());
                for (n, ann) in params {
                    match ann {
                        Some(t) => ptys.push(t.clone()),
                        None => {
                            return Err(format!(
                                "cannot infer the type of lambda parameter `{}` here",
                                n
                            ));
                        }
                    }
                }
                let [Stmt::Return(Some(e))] = &body[..] else {
                    return Err("block-bodied lambda needs a target type".to_string());
                };
                {
                    let mut overlay = self.type_overlay.borrow_mut();
                    for ((n, _), t) in params.iter().zip(&ptys) {
                        overlay.push((n.clone(), t.clone()));
                    }
                }
                let ret = self.expr_ty(e).map(widen_literal);
                {
                    let mut overlay = self.type_overlay.borrow_mut();
                    let keep = overlay.len() - params.len();
                    overlay.truncate(keep);
                }
                Type::Func(ptys, Box::new(ret?))
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
                        if let Some(m) = crate::intrinsics::string_method(&call.method) {
                            return Ok(m.return_ty);
                        }
                        if let Some(ret) = self.extension_return_ty("string", &call.method) {
                            return Ok(ret);
                        }
                        return Err(format!("unknown method `{}` on `string`", call.method));
                    }
                    let Type::Class(c, args) = obj_ty else {
                        return Err("cannot determine call target class".to_string());
                    };
                    if !self.program.classes.contains_key(&c) {
                        if let Some((bc, bargs)) = self.bounded_receiver(&c, &call.method) {
                            class_name = bc;
                            receiver_type_args = bargs;
                            is_instance = true;
                            method_name = call.method.clone();
                            return self.resolve_method_return(
                                class_name, method_name, is_instance, receiver_type_args, call,
                            );
                        }
                    }
                    class_name = c;
                    receiver_type_args = args;
                    is_instance = true;
                }
            } else {
                let obj_ty = Self::strip_nullable(self.expr_ty(target)?);
                if matches!(obj_ty, Type::String | Type::StringLit(_) | Type::LiteralUnion(..)) {
                    if let Some(m) = crate::intrinsics::string_method(&call.method) {
                        return Ok(m.return_ty);
                    }
                    if let Some(ret) = self.extension_return_ty("string", &call.method) {
                        return Ok(ret);
                    }
                    return Err(format!("unknown method `{}` on `string`", call.method));
                }
                let Type::Class(c, args) = obj_ty else {
                    return Err("cannot determine call target class".to_string());
                };
                if !self.program.classes.contains_key(&c) {
                    if let Some((bc, bargs)) = self.bounded_receiver(&c, &call.method) {
                        return self.resolve_method_return(
                            bc,
                            call.method.clone(),
                            true,
                            bargs,
                            call,
                        );
                    }
                }
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
            // A bare call may invoke a function-typed local: `f(2)`.
            if let Some(Type::Func(_, ret)) = self.local_types.get(&call.class_or_target) {
                return Ok((**ret).clone());
            }
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
        self.resolve_method_return(class_name, method_name, is_instance, receiver_type_args, call)
    }

    /// Tail of `call_return_type`: resolve `class_name.method_name` through
    /// the superclass chain and substitute the receiver's type arguments.
    fn resolve_method_return(
        &self,
        class_name: String,
        method_name: String,
        is_instance: bool,
        receiver_type_args: Vec<Type>,
        call: &CallExpr,
    ) -> Result<Type, String> {
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
        // Extension fallback for chained typing: receiver class, then its
        // superclass chain.
        if is_instance {
            let mut cur = Some(class_name.clone());
            while let Some(k) = cur {
                if let Some(ret) = self.extension_return_ty(&k, &method_name) {
                    return Ok(ret);
                }
                cur = self.program.classes.get(&k).and_then(|i| i.super_class.clone());
            }
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

/// The constant initializer of static field `name` on `class`, if declared.
/// The typer has already restricted initializers to constant literals.
fn static_field_init(class: &ClassDecl, name: &str) -> Option<aura_bytecode::ConstInit> {
    use aura_bytecode::ConstInit;
    for member in &class.members {
        if let Member::Field(f) = member {
            if f.is_static && f.name == name {
                return match f.init.as_ref()? {
                    Expr::IntLit(v, _) => Some(ConstInit::Int(*v)),
                    Expr::FloatLit(v, _) => Some(ConstInit::Float(*v)),
                    Expr::Bool(b) => Some(ConstInit::Bool(*b)),
                    Expr::Char(c) => Some(ConstInit::Char(*c)),
                    Expr::String(s) => Some(ConstInit::Str(s.clone())),
                    Expr::Unary(UnaryOp::Neg, inner) => match inner.as_ref() {
                        Expr::IntLit(v, _) => Some(ConstInit::Int(-*v)),
                        Expr::FloatLit(v, _) => Some(ConstInit::Float(-*v)),
                        _ => None,
                    },
                    _ => None,
                };
            }
        }
    }
    None
}


/// Widen transient literal marker types to their carrier types.
fn widen_literal(t: Type) -> Type {
    match t {
        Type::IntLit(v) => {
            if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
                Type::Int32
            } else {
                Type::Int64
            }
        }
        Type::FloatLit(_) => Type::Float64,
        Type::StringLit(_) => Type::String,
        other => other,
    }
}

fn push_unique(out: &mut Vec<String>, name: &str) {
    if !out.iter().any(|n| n == name) {
        out.push(name.to_string());
    }
}

/// Collect free variable references (including `this` and bare-call
/// targets) in statement order. `bound` names are not free; declarations
/// bind for the remainder of their scope. Over-approximation is safe: the
/// caller intersects with the enclosing method's locals.
fn collect_free_vars_stmts(
    stmts: &[Stmt],
    bound: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    let mut bound = bound.clone();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl(_, name, init) => {
                if let Some(e) = init {
                    collect_free_vars_expr(e, &bound, out);
                }
                bound.insert(name.clone());
            }
            Stmt::TupleDecl(names, e) => {
                collect_free_vars_expr(e, &bound, out);
                for n in names {
                    bound.insert(n.clone());
                }
            }
            Stmt::Expr(e) | Stmt::Throw(e) => collect_free_vars_expr(e, &bound, out),
            Stmt::Assign(target, e) => {
                match target {
                    AssignTarget::Local(n) => {
                        if !bound.contains(n) {
                            push_unique(out, n);
                        }
                    }
                    AssignTarget::Field(obj, _) => collect_free_vars_expr(obj, &bound, out),
                    AssignTarget::StaticField(..) | AssignTarget::SuperField(_) => {}
                }
                collect_free_vars_expr(e, &bound, out);
            }
            Stmt::Return(e) => {
                if let Some(e) = e {
                    collect_free_vars_expr(e, &bound, out);
                }
            }
            Stmt::If(c, a, b) => {
                collect_free_vars_expr(c, &bound, out);
                collect_free_vars_stmts(a, &bound, out);
                if let Some(b) = b {
                    collect_free_vars_stmts(b, &bound, out);
                }
            }
            Stmt::IfLet(pat, e, a, b) => {
                collect_free_vars_expr(e, &bound, out);
                let mut inner = bound.clone();
                collect_pattern_bindings(pat, &mut inner);
                let inner_stmts = a;
                {
                    let saved = inner;
                    collect_free_vars_stmts(inner_stmts, &saved, out);
                }
                if let Some(b) = b {
                    collect_free_vars_stmts(b, &bound, out);
                }
            }
            Stmt::While { condition, body, .. } => {
                collect_free_vars_expr(condition, &bound, out);
                collect_free_vars_stmts(body, &bound, out);
            }
            Stmt::DoWhile { body, condition, .. } => {
                collect_free_vars_stmts(body, &bound, out);
                collect_free_vars_expr(condition, &bound, out);
            }
            Stmt::For { init, condition, update, body, .. } => {
                // The init statement's binding scopes over the rest.
                let mut seq: Vec<Stmt> = Vec::with_capacity(3 + body.len());
                seq.push((**init).clone());
                seq.push(Stmt::Expr(condition.clone()));
                seq.extend(body.iter().cloned());
                seq.push((**update).clone());
                collect_free_vars_stmts(&seq, &bound, out);
            }
            Stmt::ForIn { var_name, iterable, body, .. } => {
                collect_free_vars_expr(iterable, &bound, out);
                let mut inner = bound.clone();
                inner.insert(var_name.clone());
                collect_free_vars_stmts(body, &inner, out);
            }
            Stmt::Block(b) => collect_free_vars_stmts(b, &bound, out),
            Stmt::Try { try_body, catches, finally_body } => {
                collect_free_vars_stmts(try_body, &bound, out);
                for c in catches {
                    let mut inner = bound.clone();
                    inner.insert(c.name.clone());
                    collect_free_vars_stmts(&c.body, &inner, out);
                }
                if let Some(f) = finally_body {
                    collect_free_vars_stmts(f, &bound, out);
                }
            }
            Stmt::Using { name, expr, body, .. } => {
                collect_free_vars_expr(expr, &bound, out);
                let mut inner = bound.clone();
                if let Some(n) = name {
                    inner.insert(n.clone());
                }
                collect_free_vars_stmts(body, &inner, out);
            }
            Stmt::Mark(_) | Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }
}

fn collect_pattern_bindings(pat: &Pattern, bound: &mut std::collections::HashSet<String>) {
    match pat {
        Pattern::Binding(n) => {
            bound.insert(n.clone());
        }
        Pattern::EnumVariant(_, _, subs) | Pattern::RecordClass(_, subs) => {
            for sp in subs {
                collect_pattern_bindings(sp, bound);
            }
        }
        _ => {}
    }
}

fn collect_free_vars_expr(
    e: &Expr,
    bound: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    match e {
        Expr::Var(n) => {
            if !bound.contains(n) {
                push_unique(out, n);
            }
        }
        Expr::Lambda { params, body } => {
            let mut inner = bound.clone();
            for (n, _) in params {
                inner.insert(n.clone());
            }
            collect_free_vars_stmts(body, &inner, out);
        }
        Expr::Call(c) | Expr::NullConditionalCall(c) => {
            if c.target.is_none() && !bound.contains(&c.class_or_target) {
                // A bare call may name a function-typed local.
                push_unique(out, &c.class_or_target);
            }
            if let Some(t) = &c.target {
                collect_free_vars_expr(t, bound, out);
            }
            for a in &c.args {
                collect_free_vars_expr(a, bound, out);
            }
        }
        Expr::Field(obj, _) | Expr::NullConditionalField(obj, _) => {
            collect_free_vars_expr(obj, bound, out)
        }
        Expr::Binary(_, a, b)
        | Expr::CustomOp(_, a, b)
        | Expr::NullCoalesce(a, b)
        | Expr::Range(a, b, _) => {
            collect_free_vars_expr(a, bound, out);
            collect_free_vars_expr(b, bound, out);
        }
        Expr::Unary(_, x)
        | Expr::NonNullAssert(x)
        | Expr::Cast(x, _)
        | Expr::TryUnwrap(x)
        | Expr::TupleIndex(x, _) => collect_free_vars_expr(x, bound, out),
        Expr::Is(x, _, _) => collect_free_vars_expr(x, bound, out),
        Expr::Await(x) => collect_free_vars_expr(x, bound, out),
        Expr::Ternary(c, a, b) => {
            collect_free_vars_expr(c, bound, out);
            collect_free_vars_expr(a, bound, out);
            collect_free_vars_expr(b, bound, out);
        }
        Expr::New(_, _, args) | Expr::EnumVariant(_, _, args) | Expr::SuperCall(_, args) => {
            for a in args {
                collect_free_vars_expr(a, bound, out);
            }
        }
        Expr::Tuple(xs) => {
            for x in xs {
                collect_free_vars_expr(x, bound, out);
            }
        }
        Expr::Match(subject, arms) => {
            collect_free_vars_expr(subject, bound, out);
            for arm in arms {
                let mut inner = bound.clone();
                for p in &arm.patterns {
                    collect_pattern_bindings(p, &mut inner);
                }
                if let Some(g) = &arm.guard {
                    collect_free_vars_expr(g, &inner, out);
                }
                collect_free_vars_expr(&arm.body, &inner, out);
            }
        }
        Expr::With(x, sets) => {
            collect_free_vars_expr(x, bound, out);
            for (_, v) in sets {
                collect_free_vars_expr(v, bound, out);
            }
        }
        Expr::Block(b) => collect_free_vars_stmts(b, bound, out),
        Expr::InterpolatedString(parts) => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    collect_free_vars_expr(x, bound, out);
                }
            }
        }
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Null
        | Expr::StaticField(..)
        | Expr::SuperField(_)
        | Expr::Hole => {}
    }
}

fn map_type(ty: &Type, class_ids: &HashMap<String, ClassId>, enum_ids: &HashMap<String, EnumId>, generic_params: &[aura_bytecode::GenericParam]) -> TypeDesc {
    match ty {
        Type::Nullable(inner) => TypeDesc::Nullable(Box::new(map_type(inner, class_ids, enum_ids, generic_params))),
        // Function signatures are erased: closures are opaque at runtime.
        Type::Func(..) => TypeDesc::Function,
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
