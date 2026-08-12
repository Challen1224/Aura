//! Bytecode emitter.
//!
//! Translates the typed AST into an Aura [`Module`].

use crate::ast::*;
use crate::typer::{ClassInfo, TypedProgram};
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

        // First pass: assign ids to classes.
        let mut class_ids: HashMap<String, ClassId> = HashMap::new();
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
                                &class_ids, &enum_ids, &field_layouts,
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
                                        &field_layouts, &class_generic_params, &constructor_ids, &mut module,
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
                                        &field_layouts, &class_generic_params, &constructor_ids, &mut module,
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

                module.classes.insert(
                    class_id,
                    ClassDef {
                        name: class.name.clone(),
                        generic_params: class_generic_params,
                        super_class: info.super_class.as_ref().map(|n| *class_ids.get(n).unwrap()),
                        interfaces: info.interfaces.iter().map(|n| *class_ids.get(n).unwrap()).collect(),
                        is_interface: info.is_interface,
                        is_abstract: info.is_abstract,
                        fields,
                        static_fields,
                        methods,
                        static_methods,
                    },
                );
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
                self.push_local(name.clone());
                self.local_types.insert(name.clone(), ty.clone());
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
            Stmt::ForIn { label, var_type: _, var_name, iterable, body } => {
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
            Expr::Int(i) => self.ops.push(Op::LdInt(*i)),
            Expr::Float(f) => {
                let idx = self.constants.len() as u32;
                self.constants.push(f.to_string());
                self.ops.push(Op::LdFloat(idx));
            }
            Expr::Bool(b) => self.ops.push(Op::LdBool(*b)),
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
            Expr::Binary(op, left, right) => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
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
            }
            Expr::Unary(op, operand) => {
                self.emit_expr(operand)?;
                self.ops.push(match op {
                    UnaryOp::Neg => Op::Neg,
                    UnaryOp::Not => Op::Not,
                });
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
                        } else {
                            for arg in &call.args {
                                self.emit_expr(arg)?;
                            }
                            let method_id = *self.method_ids.get(&(
                                call.class_or_target.clone(),
                                call.method.clone(),
                                false,
                            )).ok_or_else(|| {
                                format!(
                                    "unknown static method `{}` on `{}`",
                                    call.method, call.class_or_target
                                )
                            })?;
                            self.ops.push(Op::Call(method_id));
                        }
                    }
                }
            }
            Expr::New(class_name, type_args, args) => {
                let class_id = *self.class_ids.get(class_name).ok_or_else(|| {
                    format!("unknown class `{}`", class_name)
                })?;
                let has_constructor = !self.program.classes.get(class_name)
                    .map(|ci| ci.constructors.is_empty())
                    .unwrap_or(true);
                let ctor_id = if has_constructor {
                    Some(self.resolve_constructor_id(class_name, args)?)
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
                        let end_jump = self.ops.len();
                        self.ops.push(Op::Br(0));
                        end_jumps.push(end_jump);
                        
                        self.ops[guard_fail] = Op::BrFalse(self.ops.len() as u32);
                    } else {
                        // Emit body
                        self.emit_expr(&arm.body)?;
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
            Expr::Block(stmts) => {
                let start_locals = self.locals.len();
                let mut iter = stmts.iter().peekable();
                while let Some(s) = iter.next() {
                    if iter.peek().is_none() {
                        // Last statement: an expression leaves its value on the
                        // stack (no trailing Pop like a normal expression stmt).
                        if let Stmt::Expr(e) = s {
                            self.emit_expr(e)?;
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
        }
        Ok(())
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
                self.ops.push(Op::LdInt(*i));
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
                self.ops.push(Op::Eq);
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
                    match self.local_types.get(name) {
                        Some(Type::Class(c, _)) => Some(c.clone()),
                        _ => None,
                    }
                }
            }
            Expr::New(class_name, _, _) => Some(class_name.clone()),
            Expr::Field(obj, name) => {
                let owner = self.expr_class(obj)?;
                let layout = self.field_layout.get(&owner)?;
                let idx = layout.iter().rposition(|(n, _)| n == name)?;
                match &layout[idx].1 {
                    Type::Class(c, _) => Some(c.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Resolve the constructor `MethodId` of `class_name` that matches the
    /// given argument expressions (by count and assignability), mirroring the
    /// type checker's overload resolution.
    fn resolve_constructor_id(&self, class_name: &str, args: &[Expr]) -> Result<MethodId, String> {
        let ids = self.constructor_ids.get(class_name).ok_or_else(|| {
            format!("class `{}` declares no constructors", class_name)
        })?;
        let infos = self.program.classes.get(class_name)
            .ok_or_else(|| format!("unknown class `{}`", class_name))?;
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
                        .all(|(expected, actual)| self.is_assignable(expected, actual))
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
                self.resolve_constructor_id(super_name, &chain.args)
            }
            ConstructorTarget::This => {
                self.resolve_constructor_id(self.class_name, &chain.args)
            }
        }
    }

    /// Lightweight expression type inference for constructor argument matching.
    /// The type checker has already validated the program, so this only needs
    /// to be accurate enough to disambiguate overloads.
    fn expr_ty(&self, expr: &Expr) -> Result<Type, String> {
        let ty = match expr {
            Expr::Int(_) => Type::Int,
            Expr::Float(_) => Type::Float,
            Expr::Bool(_) => Type::Bool,
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
            Expr::Field(obj, name) => {
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
            Expr::Tuple(elements) => {
                let tys = elements
                    .iter()
                    .map(|e| self.expr_ty(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Type::Tuple(tys)
            }
            Expr::Binary(op, _, _) => {
                match op {
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                    | BinOp::And | BinOp::Or => Type::Bool,
                    _ => Type::Int,
                }
            }
            Expr::Unary(op, _) => {
                match op {
                    UnaryOp::Not => Type::Bool,
                    UnaryOp::Neg => Type::Int,
                }
            }
            Expr::Call(call) => {
                self.call_return_type(call)?
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
        if let Some(target) = &call.target {
            if let Expr::Var(c) = target.as_ref() {
                if self.class_ids.contains_key(c) {
                    class_name = c.clone();
                    is_instance = false;
                } else {
                    let obj_ty = self.expr_ty(target)?;
                    let Type::Class(c, _) = obj_ty else {
                        return Err("cannot determine call target class".to_string());
                    };
                    class_name = c;
                    is_instance = true;
                }
            } else {
                let obj_ty = self.expr_ty(target)?;
                let Type::Class(c, _) = obj_ty else {
                    return Err("cannot determine call target class".to_string());
                };
                class_name = c;
                is_instance = true;
            }
            method_name = call.method.clone();
        } else if call.class_or_target == "__intrinsics" {
            return match call.method.as_str() {
                "int" => Ok(Type::Int),
                "float" => Ok(Type::Float),
                "string" => Ok(Type::String),
                _ => Ok(Type::Unit),
            };
        } else {
            class_name = call.class_or_target.clone();
            is_instance = false;
            method_name = call.method.clone();
        }
        let mut cur = Some(class_name.clone());
        while let Some(c) = cur {
            let info = self.program.classes.get(&c).ok_or_else(|| {
                format!("unknown class `{}`", c)
            })?;
            let table = if is_instance { &info.methods } else { &info.static_methods };
            if let Some(mi) = table.get(&method_name) {
                return Ok(mi.return_ty.clone());
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
        if matches!(target, Type::Int | Type::Float) && matches!(source, Type::Int | Type::Float) {
            return true;
        }
        match (target, source) {
            (Type::Class(name, _), Type::Class(source_name, _)) => {
                if source_name == "null" {
                    return true;
                }
                self.is_subclass_of(source_name, name)
            }
            (Type::String, Type::Class(source_name, _)) if source_name == "null" => true,
            (Type::Enum(a), Type::Enum(b)) => a == b,
            (Type::Class(name, _), Type::Enum(enum_name)) => name == enum_name,
            (Type::Enum(enum_name), Type::Class(name, _)) => name == enum_name,
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

fn map_type(ty: &Type, class_ids: &HashMap<String, ClassId>, enum_ids: &HashMap<String, EnumId>, generic_params: &[aura_bytecode::GenericParam]) -> TypeDesc {
    match ty {
        Type::Unit => TypeDesc::Unit,
        Type::Int => TypeDesc::Int,
        Type::Float => TypeDesc::Float,
        Type::Bool => TypeDesc::Bool,
        Type::String => TypeDesc::String,
        Type::Enum(name) => {
            let enum_id = *enum_ids.get(name).unwrap_or(&EnumId(0));
            TypeDesc::Enum(enum_id)
        }
        Type::Class(name, args) => {
            if args.is_empty() {
                if let Some(idx) = generic_params.iter().position(|gp| gp.name == *name) {
                    return TypeDesc::GenericParam(idx as u32);
                }
                if let Some(enum_id) = enum_ids.get(name) {
                    return TypeDesc::Enum(*enum_id);
                }
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
