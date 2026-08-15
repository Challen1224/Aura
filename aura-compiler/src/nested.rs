//! Nested classes: `class Outer { class Inner { } }`.
//!
//! Purely a compile-time desugar. Each nested class is hoisted to top level
//! under a mangled name (`Outer.Inner`), and every reference in the program
//! is resolved through the enclosing scope chain — types, `new`, static
//! member access, bases, extension targets, and match patterns. From inside
//! `Outer` (and anything nested in it) the class is just `Inner`; from
//! outside it is `Outer.Inner`. Nothing downstream changes: the typer,
//! emitter, VM and JIT see ordinary classes whose names contain a dot.

use crate::ast::*;
use std::collections::HashSet;

/// Hoist nested class declarations to top level and resolve references.
/// Runs after alias expansion, before type checking.
pub fn flatten_nested_classes(program: &Program) -> Result<Program, String> {
    if !program
        .decls
        .iter()
        .any(|d| matches!(d, Decl::Class(c) if !c.nested.is_empty()))
    {
        return Ok(program.clone());
    }

    // Phase 1: hoist. Each hoisted class carries the scope chain used to
    // resolve unqualified names from inside it: its own mangled name first,
    // then its enclosing classes, innermost first.
    let mut flat: Vec<(ClassDecl, Vec<String>)> = Vec::new();
    let mut rest: Vec<Decl> = Vec::new();
    for decl in &program.decls {
        match decl {
            Decl::Class(c) => hoist(c.clone(), &[], &mut flat)?,
            other => rest.push(other.clone()),
        }
    }

    let names: HashSet<String> = flat.iter().map(|(c, _)| c.name.clone()).collect();

    // Phase 2: resolve references in every class body.
    let mut decls = rest;
    for (mut class, scope) in flat {
        let ctx = Ctx { scope, names: &names };
        class.bases = class.bases.iter().map(|b| ctx.resolve(b)).collect();
        class.extension_on = class.extension_on.as_ref().map(|t| ctx.ty(t));
        class.record_params = class
            .record_params
            .iter()
            .map(|p| Param { ty: ctx.ty(&p.ty), name: p.name.clone() })
            .collect();
        class.members = class.members.iter().map(|m| ctx.member(m)).collect();
        decls.push(Decl::Class(class));
    }
    Ok(Program { decls })
}

fn hoist(
    mut class: ClassDecl,
    enclosing: &[String],
    out: &mut Vec<(ClassDecl, Vec<String>)>,
) -> Result<(), String> {
    let mangled = match enclosing.first() {
        None => class.name.clone(),
        Some(inner_most) => format!("{}.{}", inner_most, class.name),
    };
    // Constructors were parsed under the short name; they follow the class.
    for member in &mut class.members {
        if let Member::Method(m) = member {
            if m.is_constructor {
                m.name = mangled.clone();
            }
        }
    }
    class.name = mangled.clone();
    // Scope chain for this class: itself, then its enclosing classes.
    let mut scope = vec![mangled.clone()];
    scope.extend(enclosing.iter().cloned());
    let children = std::mem::take(&mut class.nested);
    out.push((class, scope.clone()));
    // Children resolve through this class first: enclosing list is the new
    // scope minus... exactly `scope` (this class innermost).
    for child in children {
        hoist(child, &scope, out)?;
    }
    Ok(())
}

struct Ctx<'a> {
    /// Mangled names to try as prefixes, innermost first.
    scope: Vec<String>,
    /// All class names after mangling.
    names: &'a HashSet<String>,
}

impl Ctx<'_> {
    /// Resolve a (possibly dotted) class name through the scope chain:
    /// `Inner` inside `Outer` becomes `Outer.Inner` when that class exists;
    /// otherwise the name is returned unchanged.
    fn resolve(&self, name: &str) -> String {
        for prefix in &self.scope {
            let cand = format!("{}.{}", prefix, name);
            if self.names.contains(&cand) {
                return cand;
            }
        }
        name.to_string()
    }

    /// Interpret an expression tree as a class path (`Outer.Inner.Deep`),
    /// as produced by the expression parser: `Var`, then `StaticField` /
    /// `Field` links. `None` when any link is not a class.
    fn class_path(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Var(n) => {
                let r = self.resolve(n);
                self.names.contains(&r).then_some(r)
            }
            Expr::StaticField(a, b) => {
                let ra = self.resolve(a);
                let cand = format!("{}.{}", ra, b);
                self.names.contains(&cand).then_some(cand)
            }
            Expr::Field(inner, b) => {
                let p = self.class_path(inner)?;
                let cand = format!("{}.{}", p, b);
                self.names.contains(&cand).then_some(cand)
            }
            _ => None,
        }
    }

    fn ty(&self, t: &Type) -> Type {
        match t {
            Type::Class(n, args) => {
                Type::Class(self.resolve(n), args.iter().map(|a| self.ty(a)).collect())
            }
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|a| self.ty(a)).collect()),
            Type::Nullable(inner) => Type::Nullable(Box::new(self.ty(inner))),
            Type::Newtype(n, inner) => Type::Newtype(n.clone(), Box::new(self.ty(inner))),
            other => other.clone(),
        }
    }

    fn member(&self, m: &Member) -> Member {
        match m {
            Member::Field(f) => Member::Field(FieldDecl {
                ty: self.ty(&f.ty),
                init: f.init.as_ref().map(|e| self.expr(e)),
                ..f.clone()
            }),
            Member::Method(md) => Member::Method(MethodDecl {
                return_ty: self.ty(&md.return_ty),
                params: md
                    .params
                    .iter()
                    .map(|p| Param { ty: self.ty(&p.ty), name: p.name.clone() })
                    .collect(),
                constructor_chain: md.constructor_chain.as_ref().map(|cc| ConstructorChain {
                    target: cc.target,
                    args: cc.args.iter().map(|a| self.expr(a)).collect(),
                }),
                generic_params: md
                    .generic_params
                    .iter()
                    .map(|gp| GenericParam {
                        name: gp.name.clone(),
                        constraint: gp.constraint.as_ref().map(|c| self.ty(c)),
                        union: gp.union.iter().map(|t| self.ty(t)).collect(),
                        requires_new: gp.requires_new,
                    })
                    .collect(),
                body: md.body.iter().map(|s| self.stmt(s)).collect(),
                ..md.clone()
            }),
            Member::Property(p) => Member::Property(PropertyDecl {
                ty: self.ty(&p.ty),
                getter: p.getter.as_ref().map(|a| self.accessor(a)),
                setter: p.setter.as_ref().map(|a| self.accessor(a)),
                ..p.clone()
            }),
        }
    }

    fn accessor(&self, a: &Accessor) -> Accessor {
        Accessor {
            visibility: a.visibility,
            kind: match &a.kind {
                AccessorKind::Auto => AccessorKind::Auto,
                AccessorKind::Body(b) => {
                    AccessorKind::Body(b.iter().map(|s| self.stmt(s)).collect())
                }
            },
        }
    }

    fn stmts(&self, ss: &[Stmt]) -> Vec<Stmt> {
        ss.iter().map(|s| self.stmt(s)).collect()
    }

    fn stmt(&self, s: &Stmt) -> Stmt {
        match s {
            Stmt::VarDecl(ty, name, init) => {
                Stmt::VarDecl(self.ty(ty), name.clone(), init.as_ref().map(|e| self.expr(e)))
            }
            Stmt::TupleDecl(names, e) => Stmt::TupleDecl(names.clone(), self.expr(e)),
            Stmt::Expr(e) => Stmt::Expr(self.expr(e)),
            Stmt::Assign(target, e) => Stmt::Assign(self.assign_target(target), self.expr(e)),
            Stmt::Return(e) => Stmt::Return(e.as_ref().map(|e| self.expr(e))),
            Stmt::If(c, t, f) => Stmt::If(
                self.expr(c),
                self.stmts(t),
                f.as_ref().map(|b| self.stmts(b)),
            ),
            Stmt::IfLet(p, e, t, f) => Stmt::IfLet(
                self.pattern(p),
                self.expr(e),
                self.stmts(t),
                f.as_ref().map(|b| self.stmts(b)),
            ),
            Stmt::While { label, condition, body } => Stmt::While {
                label: label.clone(),
                condition: self.expr(condition),
                body: self.stmts(body),
            },
            Stmt::For { label, init, condition, update, body } => Stmt::For {
                label: label.clone(),
                init: Box::new(self.stmt(init)),
                condition: self.expr(condition),
                update: Box::new(self.stmt(update)),
                body: self.stmts(body),
            },
            Stmt::ForIn { label, var_type, var_name, iterable, body } => Stmt::ForIn {
                label: label.clone(),
                var_type: self.ty(var_type),
                var_name: var_name.clone(),
                iterable: self.expr(iterable),
                body: self.stmts(body),
            },
            Stmt::DoWhile { label, body, condition } => Stmt::DoWhile {
                label: label.clone(),
                body: self.stmts(body),
                condition: self.expr(condition),
            },
            Stmt::Block(b) => Stmt::Block(self.stmts(b)),
            Stmt::Throw(e) => Stmt::Throw(self.expr(e)),
            Stmt::Try { try_body, catches, finally_body } => Stmt::Try {
                try_body: self.stmts(try_body),
                catches: catches
                    .iter()
                    .map(|c| CatchClause {
                        ty: self.ty(&c.ty),
                        name: c.name.clone(),
                        body: self.stmts(&c.body),
                    })
                    .collect(),
                finally_body: finally_body.as_ref().map(|b| self.stmts(b)),
            },
            Stmt::Using { resource_ty, name, expr, body } => Stmt::Using {
                resource_ty: resource_ty.as_ref().map(|t| self.ty(t)),
                name: name.clone(),
                expr: Box::new(self.expr(expr)),
                body: self.stmts(body),
            },
            Stmt::Mark(_) | Stmt::Break(_) | Stmt::Continue(_) => s.clone(),
        }
    }

    fn assign_target(&self, t: &AssignTarget) -> AssignTarget {
        match t {
            AssignTarget::Local(n) => AssignTarget::Local(n.clone()),
            AssignTarget::Field(obj, name) => {
                // `Outer.Inner.count = 5` parses as a field store on a class
                // path; it is really a static store on the nested class.
                if let Some(path) = self.class_path(obj) {
                    AssignTarget::StaticField(path, name.clone())
                } else {
                    AssignTarget::Field(Box::new(self.expr(obj)), name.clone())
                }
            }
            AssignTarget::StaticField(cls, name) => {
                AssignTarget::StaticField(self.resolve(cls), name.clone())
            }
            AssignTarget::SuperField(n) => AssignTarget::SuperField(n.clone()),
        }
    }

    fn pattern(&self, p: &Pattern) -> Pattern {
        match p {
            Pattern::RecordClass(name, subs) => Pattern::RecordClass(
                self.resolve(name),
                subs.iter().map(|sp| self.pattern(sp)).collect(),
            ),
            Pattern::EnumVariant(enum_name, variant, subs) => {
                // `Outer.P(x, y)` parses as an enum-variant pattern; when
                // the dotted name is a nested record class, it is really a
                // record pattern.
                let joined = format!("{}.{}", self.resolve(enum_name), variant);
                let subs: Vec<Pattern> = subs.iter().map(|sp| self.pattern(sp)).collect();
                if self.names.contains(&joined) {
                    Pattern::RecordClass(joined, subs)
                } else {
                    Pattern::EnumVariant(enum_name.clone(), variant.clone(), subs)
                }
            }
            Pattern::Range(a, b, incl) => Pattern::Range(
                Box::new(self.expr(a)),
                Box::new(self.expr(b)),
                *incl,
            ),
            other => other.clone(),
        }
    }

    fn call(&self, c: &CallExpr) -> CallExpr {
        let target = match &c.target {
            // `Outer.Inner.Make(...)` parses as a call on a class path:
            // rewrite the path to a single class name so the static-call
            // machinery sees it.
            Some(t) => match self.class_path(t) {
                Some(path) => Some(Box::new(Expr::Var(path))),
                None => Some(Box::new(self.expr(t))),
            },
            // Bare static call `Inner.Make(...)` on a nested class in scope.
            None => None,
        };
        let class_or_target = if c.target.is_none() {
            self.resolve(&c.class_or_target)
        } else {
            c.class_or_target.clone()
        };
        CallExpr {
            target,
            class_or_target,
            method: c.method.clone(),
            type_args: c.type_args.iter().map(|t| self.ty(t)).collect(),
            args: c.args.iter().map(|a| self.expr(a)).collect(),
        }
    }

    fn expr(&self, e: &Expr) -> Expr {
        match e {
            Expr::New(name, targs, args) => Expr::New(
                self.resolve(name),
                targs.iter().map(|t| self.ty(t)).collect(),
                args.iter().map(|a| self.expr(a)).collect(),
            ),
            Expr::Call(c) => Expr::Call(self.call(c)),
            Expr::NullConditionalCall(c) => Expr::NullConditionalCall(self.call(c)),
            Expr::Field(obj, name) => {
                // A field read on a class path is a static-field read on the
                // nested class (`Outer.Inner.count`).
                if let Some(path) = self.class_path(obj) {
                    Expr::StaticField(path, name.clone())
                } else {
                    Expr::Field(Box::new(self.expr(obj)), name.clone())
                }
            }
            Expr::StaticField(cls, name) => {
                // `Inner.count` inside `Outer`: the class part resolves
                // through the scope chain.
                Expr::StaticField(self.resolve(cls), name.clone())
            }
            Expr::NullConditionalField(obj, name) => {
                Expr::NullConditionalField(Box::new(self.expr(obj)), name.clone())
            }
            Expr::Binary(op, l, r) => {
                Expr::Binary(*op, Box::new(self.expr(l)), Box::new(self.expr(r)))
            }
            Expr::Unary(op, x) => Expr::Unary(*op, Box::new(self.expr(x))),
            Expr::Ternary(c, t, f) => Expr::Ternary(
                Box::new(self.expr(c)),
                Box::new(self.expr(t)),
                Box::new(self.expr(f)),
            ),
            Expr::Cast(x, t) => Expr::Cast(Box::new(self.expr(x)), self.ty(t)),
            Expr::Match(subject, arms) => Expr::Match(
                Box::new(self.expr(subject)),
                arms.iter()
                    .map(|a| MatchArm {
                        patterns: a.patterns.iter().map(|p| self.pattern(p)).collect(),
                        guard: a.guard.as_ref().map(|g| self.expr(g)),
                        body: self.expr(&a.body),
                    })
                    .collect(),
            ),
            Expr::EnumVariant(en, v, args) => Expr::EnumVariant(
                en.clone(),
                v.clone(),
                args.iter().map(|a| self.expr(a)).collect(),
            ),
            Expr::Tuple(xs) => Expr::Tuple(xs.iter().map(|x| self.expr(x)).collect()),
            Expr::TupleIndex(x, i) => Expr::TupleIndex(Box::new(self.expr(x)), *i),
            Expr::Range(a, b, incl) => {
                Expr::Range(Box::new(self.expr(a)), Box::new(self.expr(b)), *incl)
            }
            Expr::NullCoalesce(a, b) => {
                Expr::NullCoalesce(Box::new(self.expr(a)), Box::new(self.expr(b)))
            }
            Expr::NonNullAssert(x) => Expr::NonNullAssert(Box::new(self.expr(x))),
            Expr::Is(x, t, binding) => {
                Expr::Is(Box::new(self.expr(x)), self.ty(t), binding.clone())
            }
            Expr::SuperCall(name, args) => Expr::SuperCall(
                name.clone(),
                args.iter().map(|a| self.expr(a)).collect(),
            ),
            Expr::With(x, sets) => Expr::With(
                Box::new(self.expr(x)),
                sets.iter().map(|(n, v)| (n.clone(), self.expr(v))).collect(),
            ),
            Expr::Block(b) => Expr::Block(self.stmts(b)),
            Expr::TryUnwrap(x) => Expr::TryUnwrap(Box::new(self.expr(x))),
            Expr::InterpolatedString(parts) => Expr::InterpolatedString(
                parts
                    .iter()
                    .map(|p| match p {
                        InterpPart::Literal(s) => InterpPart::Literal(s.clone()),
                        InterpPart::Expr(x) => InterpPart::Expr(self.expr(x)),
                    })
                    .collect(),
            ),
            Expr::IntLit(..)
            | Expr::FloatLit(..)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Null
            | Expr::Var(_)
            | Expr::Hole
            | Expr::SuperField(_) => e.clone(),
        }
    }
}
