//! `aura lint` — static checks beyond what compilation enforces.
//!
//! Three checks, all AST-based and per-method:
//!
//! * **unused locals** — a declared local that is never read (assignment
//!   alone does not count as a read). Names starting with `_` are exempt,
//!   as are method/lambda parameters and pattern bindings (which often
//!   exist to satisfy a shape). Shadowing via pattern bindings can mask an
//!   unused outer variable of the same name — the imprecision errs toward
//!   silence, never toward a false report.
//! * **unreachable code** — statements after a `return`/`throw`/`break`/
//!   `continue` in the same block.
//! * **empty catch** — a catch clause whose body is empty swallows the
//!   exception silently.
//!
//! Diagnostics carry the closest preceding source line marker.

use aura_compiler::ast::{
    AccessorKind, AssignTarget, CallExpr, ClassDecl, Decl, Expr, InterpPart, Member, Pattern,
    Program, Stmt,
};
use std::collections::HashMap;

/// One lint diagnostic.
pub struct Diagnostic {
    /// 1-based source line (0 when unknown).
    pub line: usize,
    /// Human-readable message.
    pub message: String,
}

/// Lint a parsed program; diagnostics are sorted by line.
pub fn lint_program(program: &Program) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for decl in &program.decls {
        if let Decl::Class(class) = decl {
            lint_class(class, &mut diags);
        }
    }
    diags.sort_by_key(|d| d.line);
    diags
}

fn lint_class(class: &ClassDecl, diags: &mut Vec<Diagnostic>) {
    for member in &class.members {
        match member {
            Member::Method(m) => {
                lint_body(&format!("{}.{}", class.name, m.name), &m.body, diags);
            }
            Member::Property(p) => {
                for acc in [&p.getter, &p.setter].into_iter().flatten() {
                    if let AccessorKind::Body(body) = &acc.kind {
                        lint_body(&format!("{}.{}", class.name, p.name), body, diags);
                    }
                }
            }
            Member::Field(_) => {}
        }
    }
    for nested in &class.nested {
        lint_class(nested, diags);
    }
}

/// Locals declared in a body, keyed by name: declaration line and read count.
struct Scope {
    declared: HashMap<String, (usize, bool)>,
}

fn lint_body(method: &str, body: &[Stmt], diags: &mut Vec<Diagnostic>) {
    let mut scope = Scope { declared: HashMap::new() };
    let mut line = 0usize;
    walk_stmts(body, &mut line, &mut scope, diags);
    let mut unused: Vec<(&String, usize)> = scope
        .declared
        .iter()
        .filter(|(name, (_, read))| !read && !name.starts_with('_'))
        .map(|(name, (decl_line, _))| (name, *decl_line))
        .collect();
    unused.sort_by_key(|(_, l)| *l);
    for (name, decl_line) in unused {
        diags.push(Diagnostic {
            line: decl_line,
            message: format!("unused local `{name}` in `{method}` (prefix with `_` to keep)"),
        });
    }
}

/// Walk statements: record declarations/reads, flag unreachable code and
/// empty catches. `line` tracks the closest preceding `Stmt::Mark`.
fn walk_stmts(stmts: &[Stmt], line: &mut usize, scope: &mut Scope, diags: &mut Vec<Diagnostic>) {
    let mut exited: Option<&'static str> = None;
    for stmt in stmts {
        if let Stmt::Mark(l) = stmt {
            *line = *l;
            continue;
        }
        if let Some(kind) = exited {
            diags.push(Diagnostic {
                line: *line,
                message: format!("unreachable code (after `{kind}`)"),
            });
            exited = None; // one report per block, not one per statement
        }
        match stmt {
            Stmt::Return(_) => exited = Some("return"),
            Stmt::Throw(_) => exited = Some("throw"),
            Stmt::Break(_) => exited = Some("break"),
            Stmt::Continue(_) => exited = Some("continue"),
            _ => {}
        }
        walk_stmt(stmt, line, scope, diags);
    }
}

fn walk_stmt(stmt: &Stmt, line: &mut usize, scope: &mut Scope, diags: &mut Vec<Diagnostic>) {
    match stmt {
        Stmt::VarDecl(_, name, init) => {
            if let Some(init) = init {
                walk_expr(init, line, scope, diags);
            }
            scope.declared.insert(name.clone(), (*line, false));
        }
        Stmt::TupleDecl(names, init) => {
            walk_expr(init, line, scope, diags);
            for name in names {
                scope.declared.insert(name.clone(), (*line, false));
            }
        }
        Stmt::Expr(e) | Stmt::Throw(e) => walk_expr(e, line, scope, diags),
        Stmt::Assign(target, value) => {
            // Assignment writes the target; only the value side reads.
            match target {
                AssignTarget::Field(recv, _) => walk_expr(recv, line, scope, diags),
                AssignTarget::Local(_)
                | AssignTarget::StaticField(_, _)
                | AssignTarget::SuperField(_) => {}
            }
            walk_expr(value, line, scope, diags);
        }
        Stmt::Return(Some(e)) => walk_expr(e, line, scope, diags),
        Stmt::Return(None) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Mark(_) => {}
        Stmt::If(cond, then_body, else_body) => {
            walk_expr(cond, line, scope, diags);
            walk_stmts(then_body, line, scope, diags);
            if let Some(else_body) = else_body {
                walk_stmts(else_body, line, scope, diags);
            }
        }
        Stmt::IfLet(pattern, subject, then_body, else_body) => {
            walk_expr(subject, line, scope, diags);
            declare_pattern(pattern, scope);
            walk_stmts(then_body, line, scope, diags);
            if let Some(else_body) = else_body {
                walk_stmts(else_body, line, scope, diags);
            }
        }
        Stmt::While { condition, body, .. } => {
            walk_expr(condition, line, scope, diags);
            walk_stmts(body, line, scope, diags);
        }
        Stmt::For { init, condition, update, body, .. } => {
            walk_stmt(init, line, scope, diags);
            walk_expr(condition, line, scope, diags);
            walk_stmt(update, line, scope, diags);
            walk_stmts(body, line, scope, diags);
        }
        Stmt::ForIn { var_name, iterable, body, .. } => {
            walk_expr(iterable, line, scope, diags);
            scope.declared.insert(var_name.clone(), (*line, false));
            walk_stmts(body, line, scope, diags);
        }
        Stmt::DoWhile { body, condition, .. } => {
            walk_stmts(body, line, scope, diags);
            walk_expr(condition, line, scope, diags);
        }
        Stmt::Block(body) => walk_stmts(body, line, scope, diags),
        Stmt::Try { try_body, catches, finally_body } => {
            walk_stmts(try_body, line, scope, diags);
            for catch in catches {
                if catch.body.iter().all(|s| matches!(s, Stmt::Mark(_))) {
                    diags.push(Diagnostic {
                        line: *line,
                        message: format!(
                            "empty catch block silently swallows `{}`",
                            type_name(&catch.ty)
                        ),
                    });
                }
                // The bound exception is a parameter-like binding: exempt.
                walk_stmts(&catch.body, line, scope, diags);
            }
            if let Some(finally_body) = finally_body {
                walk_stmts(finally_body, line, scope, diags);
            }
        }
        // A `using` binding exists for its Dispose side effect; a body that
        // never reads it is still meaningful, so the name is exempt.
        Stmt::Using { expr, body, .. } => {
            walk_expr(expr, line, scope, diags);
            walk_stmts(body, line, scope, diags);
        }
    }
}

fn walk_expr(expr: &Expr, line: &mut usize, scope: &mut Scope, diags: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Var(name) => mark_read(name, scope),
        Expr::Binary(_, a, b)
        | Expr::CustomOp(_, a, b)
        | Expr::NullCoalesce(a, b)
        | Expr::Range(a, b, _) => {
            walk_expr(a, line, scope, diags);
            walk_expr(b, line, scope, diags);
        }
        Expr::Unary(_, e)
        | Expr::Cast(e, _)
        | Expr::NonNullAssert(e)
        | Expr::TupleIndex(e, _)
        | Expr::Field(e, _)
        | Expr::NullConditionalField(e, _)
        | Expr::TryUnwrap(e)
        | Expr::Await(e)
        | Expr::Is(e, _, _) => walk_expr(e, line, scope, diags),
        Expr::Ternary(c, t, f) => {
            walk_expr(c, line, scope, diags);
            walk_expr(t, line, scope, diags);
            walk_expr(f, line, scope, diags);
        }
        Expr::Call(call) | Expr::NullConditionalCall(call) => {
            walk_call(call, line, scope, diags);
        }
        Expr::New(_, _, args) | Expr::EnumVariant(_, _, args) | Expr::SuperCall(_, args) => {
            for arg in args {
                walk_expr(arg, line, scope, diags);
            }
        }
        Expr::Tuple(items) => {
            for item in items {
                walk_expr(item, line, scope, diags);
            }
        }
        Expr::InterpolatedString(parts) => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    walk_expr(e, line, scope, diags);
                }
            }
        }
        Expr::Match(subject, arms) => {
            walk_expr(subject, line, scope, diags);
            for arm in arms {
                for pattern in &arm.patterns {
                    declare_pattern(pattern, scope);
                }
                if let Some(guard) = &arm.guard {
                    walk_expr(guard, line, scope, diags);
                }
                walk_expr(&arm.body, line, scope, diags);
            }
        }
        Expr::With(base, fields) => {
            walk_expr(base, line, scope, diags);
            for (_, value) in fields {
                walk_expr(value, line, scope, diags);
            }
        }
        Expr::Block(stmts) => walk_stmts(stmts, line, scope, diags),
        Expr::Lambda { body, .. } => walk_stmts(body, line, scope, diags),
        Expr::IntLit(_, _)
        | Expr::FloatLit(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Null
        | Expr::StaticField(_, _)
        | Expr::SuperField(_)
        | Expr::Hole => {}
    }
}

fn walk_call(call: &CallExpr, line: &mut usize, scope: &mut Scope, diags: &mut Vec<Diagnostic>) {
    match &call.target {
        Some(target) => walk_expr(target, line, scope, diags),
        // `x.Method()` parses with the receiver name in `class_or_target`
        // when it is a bare identifier — it may be a local or a class name;
        // conservatively count it as a read.
        None => mark_read(&call.class_or_target, scope),
    }
    for arg in &call.args {
        walk_expr(arg, line, scope, diags);
    }
}

/// Reads mark a declared local as used; unknown names (fields, classes,
/// pattern bindings) are ignored.
fn mark_read(name: &str, scope: &mut Scope) {
    if let Some((_, read)) = scope.declared.get_mut(name) {
        *read = true;
    }
}

/// Pattern bindings shadow declared locals for the arm's body; register them
/// as already-read so the shadowed outer name is not falsely reported and
/// the binding itself is never flagged.
fn declare_pattern(pattern: &Pattern, scope: &mut Scope) {
    match pattern {
        Pattern::Binding(name) => {
            scope.declared.insert(name.clone(), (0, true));
        }
        Pattern::EnumVariant(_, _, subs) | Pattern::RecordClass(_, subs) => {
            for sub in subs {
                declare_pattern(sub, scope);
            }
        }
        _ => {}
    }
}

/// Short display name for a type in diagnostics.
fn type_name(ty: &aura_compiler::ast::Type) -> String {
    ty.name()
}
