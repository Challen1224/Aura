//! Recursive-descent parser for Aura.

use crate::ast::*;
use crate::lexer::Token;
use std::collections::HashMap;

/// Expand `type X = ...;` aliases throughout the program, replacing every
/// reference to an alias name with its target type. Alias declarations
/// themselves are removed from the result.
///
/// Because generic parameters are represented as `Type::Class(name, [])` in
/// this parser, an alias that shares its name with a generic parameter is not
/// expanded (the parameter reference wins). Pick non-colliding alias names.
pub fn expand_type_aliases(program: &Program) -> Result<Program, String> {
    let mut aliases: HashMap<String, TypeAliasDecl> = HashMap::new();
    for decl in &program.decls {
        if let Decl::TypeAlias(a) = decl {
            if aliases.contains_key(&a.name) {
                return Err(format!("duplicate type alias `{}`", a.name));
            }
            aliases.insert(a.name.clone(), a.clone());
        }
    }
    // Literal-union algebra: resolve union declarations into flat member
    // lists. Operands may name other literal unions (members merge in
    // declaration order; duplicates arising through composition dedup
    // silently). A sum-type candidate whose bare variants ALL name literal
    // unions is reinterpreted as a union composition — `type Direction =
    // Horizontal | Vertical;` — while mixing union names with ordinary
    // variant names is an error rather than a silently-wrong enum.
    let mut union_ops: HashMap<String, Vec<UnionOperand>> = HashMap::new();
    for decl in &program.decls {
        if let Decl::LiteralUnion(u) = decl {
            if aliases.contains_key(&u.name) || union_ops.contains_key(&u.name) {
                return Err(format!("duplicate type `{}`", u.name));
            }
            union_ops.insert(u.name.clone(), u.operands.clone());
        }
    }
    let mut enum_tentative: HashMap<String, Vec<UnionOperand>> = HashMap::new();
    for decl in &program.decls {
        if let Decl::Enum(e) = decl {
            if e.generic_params.is_empty()
                && !e.variants.is_empty()
                && e.variants.iter().all(|v| v.fields.is_empty())
            {
                enum_tentative.insert(
                    e.name.clone(),
                    e.variants.iter().map(|v| UnionOperand::Named(v.name.clone())).collect(),
                );
            }
        }
    }
    // `Ok(Some(members))` = resolved union; `Ok(None)` = not a union (only
    // legal for tentative sum-type candidates with no union operands).
    fn resolve_union(
        name: &str,
        definite: &HashMap<String, Vec<UnionOperand>>,
        tentative: &HashMap<String, Vec<UnionOperand>>,
        aliases: &HashMap<String, TypeAliasDecl>,
        stack: &mut Vec<String>,
    ) -> Result<Option<Vec<String>>, String> {
        if stack.iter().any(|n| n == name) {
            return Err(format!("circular literal type `{}`", name));
        }
        let (ops, is_definite) = match definite.get(name) {
            Some(ops) => (ops, true),
            None => match tentative.get(name) {
                Some(ops) => (ops, false),
                None => {
                    // A plain alias may rename a union: follow one hop.
                    if let Some(a) = aliases.get(name) {
                        if let Type::Class(target, args) = &a.target {
                            if args.is_empty() && a.generic_params.is_empty() {
                                stack.push(name.to_string());
                                let r = resolve_union(target, definite, tentative, aliases, stack);
                                stack.pop();
                                return r;
                            }
                        }
                    }
                    return Ok(None);
                }
            },
        };
        stack.push(name.to_string());
        let mut members: Vec<String> = Vec::new();
        let mut resolved_named = 0usize;
        let mut unresolved: Option<String> = None;
        for op in ops {
            match op {
                UnionOperand::Literal(v) => {
                    if !members.contains(v) {
                        members.push(v.clone());
                    }
                }
                UnionOperand::Named(n) => {
                    match resolve_union(n, definite, tentative, aliases, stack)? {
                        Some(sub) => {
                            resolved_named += 1;
                            for v in sub {
                                if !members.contains(&v) {
                                    members.push(v);
                                }
                            }
                        }
                        None => {
                            if is_definite {
                                stack.pop();
                                return Err(format!(
                                    "`{}` in literal type `{}` is not a literal type",
                                    n, name
                                ));
                            }
                            unresolved = Some(n.clone());
                        }
                    }
                }
            }
        }
        stack.pop();
        if !is_definite {
            if let Some(n) = unresolved {
                if resolved_named > 0 {
                    return Err(format!(
                        "type `{}` mixes literal-union types and enum variants (`{}` is not a literal type)",
                        name, n
                    ));
                }
                return Ok(None); // an ordinary bare-variant sum type
            }
        }
        Ok(Some(members))
    }
    let mut converted_enums: Vec<String> = Vec::new();
    let union_names: Vec<String> = union_ops.keys().cloned().collect();
    for name in union_names {
        let members = resolve_union(&name, &union_ops, &enum_tentative, &aliases, &mut Vec::new())?
            .expect("definite union declarations always resolve or error");
        aliases.insert(
            name.clone(),
            TypeAliasDecl {
                name: name.clone(),
                generic_params: Vec::new(),
                target: Type::LiteralUnion(name, members),
            },
        );
    }
    let tentative_names: Vec<String> = enum_tentative.keys().cloned().collect();
    for name in tentative_names {
        if let Some(members) =
            resolve_union(&name, &union_ops, &enum_tentative, &aliases, &mut Vec::new())?
        {
            if aliases.contains_key(&name) {
                return Err(format!("duplicate type `{}`", name));
            }
            converted_enums.push(name.clone());
            aliases.insert(
                name.clone(),
                TypeAliasDecl {
                    name: name.clone(),
                    generic_params: Vec::new(),
                    target: Type::LiteralUnion(name, members),
                },
            );
        }
    }

    // Newtypes ride the same machinery as synthetic aliases whose target is
    // the opaque `Type::Newtype` wrapper: every occurrence of the name in a
    // type position resolves to the wrapper, while the wrapper itself is
    // never expanded further (it is nominal, not transparent). The
    // underlying type may itself use aliases, but must resolve to a
    // primitive — in particular a newtype cannot wrap another newtype.
    for decl in &program.decls {
        if let Decl::Newtype(n) = decl {
            if aliases.contains_key(&n.name) {
                return Err(format!(
                    "newtype `{}` conflicts with a type alias of the same name",
                    n.name
                ));
            }
            let underlying = expand(&n.underlying, &aliases, &mut Vec::new())?;
            if !matches!(
                underlying,
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
                    | Type::Bool
                    | Type::Char
                    | Type::String
            ) {
                return Err(format!(
                    "newtype `{}` must wrap a primitive type, got {}",
                    n.name,
                    underlying.name()
                ));
            }
            aliases.insert(
                n.name.clone(),
                TypeAliasDecl {
                    name: n.name.clone(),
                    generic_params: Vec::new(),
                    target: Type::Newtype(n.name.clone(), Box::new(underlying)),
                },
            );
        }
    }
    if aliases.is_empty() {
        return Ok(program.clone());
    }

    let mut decls = Vec::new();
    for decl in &program.decls {
        let expanded = match decl {
            Decl::Class(c) => {
                let mut c = c.clone();
                c.generic_params = expand_generic_params(&c.generic_params, &aliases)?;
                c.record_params = c
                    .record_params
                    .iter()
                    .map(|p| Ok(Param { ty: expand_type(&p.ty, &aliases)?, name: p.name.clone() }))
                    .collect::<Result<Vec<Param>, String>>()?;
                c.members = c
                    .members
                    .iter()
                    .map(|m| expand_member(m, &aliases))
                    .collect::<Result<Vec<_>, _>>()?;
                c.extension_on = match &c.extension_on {
                    Some(t) => Some(expand_type(t, &aliases)?),
                    None => None,
                };
                Decl::Class(c)
            }
            Decl::Enum(e) => {
                // Reinterpreted as a literal-union composition: consumed.
                if converted_enums.iter().any(|n| n == &e.name) {
                    continue;
                }
                let mut e = e.clone();
                e.generic_params = expand_generic_params(&e.generic_params, &aliases)?;
                for variant in &mut e.variants {
                    for field in &mut variant.fields {
                        field.ty = expand_type(&field.ty, &aliases)?;
                    }
                }
                Decl::Enum(e)
            }
            Decl::TypeAlias(_) => continue,
            // Resolved into a synthetic alias above: consumed.
            Decl::LiteralUnion(_) => continue,
            // Kept so the type checker can register the constructor name.
            Decl::Newtype(n) => Decl::Newtype(n.clone()),
        };
        decls.push(expanded);
    }
    Ok(Program { decls })
}

fn expand(ty: &Type, aliases: &HashMap<String, TypeAliasDecl>, stack: &mut Vec<String>) -> Result<Type, String> {
    match ty {
        Type::Class(name, args) => {
            if let Some(alias) = aliases.get(name) {
                if stack.contains(name) {
                    return Err(format!("circular type alias `{}`", name));
                }
                let mut subst = HashMap::new();
                for (param, arg) in alias.generic_params.iter().zip(args.iter()) {
                    subst.insert(param.name.clone(), arg.clone());
                }
                stack.push(name.clone());
                let result = expand(&substitute(&alias.target, &subst), aliases, stack);
                stack.pop();
                result
            } else {
                let args = args
                    .iter()
                    .map(|a| expand(a, aliases, stack))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Type::Class(name.clone(), args))
            }
        }
        Type::Enum(name) => {
            if let Some(alias) = aliases.get(name) {
                if stack.contains(name) {
                    return Err(format!("circular type alias `{}`", name));
                }
                stack.push(name.clone());
                let result = expand(&alias.target, aliases, stack);
                stack.pop();
                result
            } else {
                Ok(ty.clone())
            }
        }
        Type::Tuple(types) => {
            let types = types
                .iter()
                .map(|t| expand(t, aliases, stack))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Type::Tuple(types))
        }
        Type::Nullable(inner) => {
            let inner = expand(inner, aliases, stack)?;
            // An alias of a nullable target stays singly nullable.
            Ok(match inner {
                Type::Nullable(_) => inner,
                t => Type::Nullable(Box::new(t)),
            })
        }
        Type::Func(params, ret) => {
            let params = params
                .iter()
                .map(|t| expand(t, aliases, stack))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Type::Func(params, Box::new(expand(ret, aliases, stack)?)))
        }
        _ => Ok(ty.clone()),
    }
}

fn substitute(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::GenericParam(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Class(name, args) => {
            if args.is_empty() && subst.contains_key(name) {
                return subst.get(name).cloned().unwrap();
            }
            let args = args.iter().map(|a| substitute(a, subst)).collect();
            Type::Class(name.clone(), args)
        }
        Type::Tuple(types) => Type::Tuple(types.iter().map(|t| substitute(t, subst)).collect()),
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute(inner, subst))),
        Type::Func(params, ret) => Type::Func(
            params.iter().map(|t| substitute(t, subst)).collect(),
            Box::new(substitute(ret, subst)),
        ),
        _ => ty.clone(),
    }
}

fn expand_type(ty: &Type, aliases: &HashMap<String, TypeAliasDecl>) -> Result<Type, String> {
    expand(ty, aliases, &mut Vec::new())
}

fn expand_member(member: &Member, aliases: &HashMap<String, TypeAliasDecl>) -> Result<Member, String> {
    let expand_type = |ty: &Type| expand(ty, aliases, &mut Vec::new());
    match member {
        Member::Field(f) => {
            let mut f = f.clone();
            f.ty = expand_type(&f.ty)?;
            Ok(Member::Field(f))
        }
        Member::Method(m) => {
            let mut m = m.clone();
            m.return_ty = expand_type(&m.return_ty)?;
            m.generic_params = expand_generic_params(&m.generic_params, aliases)?;
            for p in &mut m.params {
                p.ty = expand_type(&p.ty)?;
            }
            for stmt in &mut m.body {
                expand_stmt(stmt, aliases)?;
            }
            Ok(Member::Method(m))
        }
        Member::Property(p) => {
            let mut p = p.clone();
            p.ty = expand_type(&p.ty)?;
            if let Some(Accessor { kind: AccessorKind::Body(body), .. }) = &mut p.getter {
                for stmt in body {
                    expand_stmt(stmt, aliases)?;
                }
            }
            if let Some(Accessor { kind: AccessorKind::Body(body), .. }) = &mut p.setter {
                for stmt in body {
                    expand_stmt(stmt, aliases)?;
                }
            }
            Ok(Member::Property(p))
        }
    }
}

fn expand_generic_params(
    params: &[GenericParam],
    aliases: &HashMap<String, TypeAliasDecl>,
) -> Result<Vec<GenericParam>, String> {
    params
        .iter()
        .map(|gp| {
            Ok(GenericParam {
                variance: Variance::Invariant,
                union: Vec::new(),
                requires_new: false,
                name: gp.name.clone(),
                bounds: gp
                    .bounds
                    .iter()
                    .map(|c| expand(c, aliases, &mut Vec::new()))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        })
        .collect()
}

fn expand_stmt(stmt: &mut Stmt, aliases: &HashMap<String, TypeAliasDecl>) -> Result<(), String> {
    let expand_type = |ty: &Type| expand(ty, aliases, &mut Vec::new());
    match stmt {
        Stmt::VarDecl(ty, _, init) => {
            *ty = expand_type(ty)?;
            if let Some(e) = init {
                expand_expr(e, aliases)?;
            }
        }
        Stmt::TupleDecl(_, expr) => expand_expr(expr, aliases)?,
        Stmt::Expr(expr) => expand_expr(expr, aliases)?,
        Stmt::Assign(_, expr) => expand_expr(expr, aliases)?,
        Stmt::Return(Some(expr)) => expand_expr(expr, aliases)?,
        Stmt::Return(None) => {}
        Stmt::If(cond, then_stmts, else_stmts) => {
            expand_expr(cond, aliases)?;
            for s in then_stmts {
                expand_stmt(s, aliases)?;
            }
            if let Some(else_stmts) = else_stmts {
                for s in else_stmts {
                    expand_stmt(s, aliases)?;
                }
            }
        }
        Stmt::IfLet(pattern, expr, then_stmts, else_stmts) => {
            expand_pattern(pattern, aliases)?;
            expand_expr(expr, aliases)?;
            for s in then_stmts {
                expand_stmt(s, aliases)?;
            }
            if let Some(else_stmts) = else_stmts {
                for s in else_stmts {
                    expand_stmt(s, aliases)?;
                }
            }
        }
        Stmt::While { condition, body, .. } => {
            expand_expr(condition, aliases)?;
            for s in body {
                expand_stmt(s, aliases)?;
            }
        }
        Stmt::For { init, condition, update, body, .. } => {
            expand_stmt(init, aliases)?;
            expand_expr(condition, aliases)?;
            expand_stmt(update, aliases)?;
            for s in body {
                expand_stmt(s, aliases)?;
            }
        }
        Stmt::ForIn { var_type, iterable, body, .. } => {
            *var_type = expand_type(var_type)?;
            expand_expr(iterable, aliases)?;
            for s in body {
                expand_stmt(s, aliases)?;
            }
        }
        Stmt::DoWhile { body, condition, .. } => {
            for s in body {
                expand_stmt(s, aliases)?;
            }
            expand_expr(condition, aliases)?;
        }
        Stmt::Block(body) => {
            for s in body {
                expand_stmt(s, aliases)?;
            }
        }
        Stmt::Throw(expr) => expand_expr(expr, aliases)?,
        Stmt::Try { try_body, catches, finally_body } => {
            for s in try_body {
                expand_stmt(s, aliases)?;
            }
            for c in catches {
                c.ty = expand_type(&c.ty)?;
                for s in &mut c.body {
                    expand_stmt(s, aliases)?;
                }
            }
            if let Some(finally_body) = finally_body {
                for s in finally_body {
                    expand_stmt(s, aliases)?;
                }
            }
        }
        Stmt::Using { resource_ty, expr, body, .. } => {
            if let Some(ty) = resource_ty {
                *ty = expand_type(ty)?;
            }
            expand_expr(expr, aliases)?;
            for s in body {
                expand_stmt(s, aliases)?;
            }
        }
        Stmt::Mark(_) => {}
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
    Ok(())
}

fn expand_expr(expr: &mut Expr, aliases: &HashMap<String, TypeAliasDecl>) -> Result<(), String> {
    let expand_type = |ty: &Type| expand(ty, aliases, &mut Vec::new());
    match expr {
        Expr::Lambda { params, body } => {
            for (_, ty) in params.iter_mut() {
                if let Some(t) = ty {
                    *t = expand_type(t)?;
                }
            }
            for st in body.iter_mut() {
                expand_stmt(st, aliases)?;
            }
        }
        Expr::Await(inner) => expand_expr(inner, aliases)?,
        Expr::Cast(inner, ty) => {
            *ty = expand_type(ty)?;
            expand_expr(inner, aliases)?;
        }
        Expr::Is(subject, ty, _) => {
            *ty = expand_type(ty)?;
            expand_expr(subject, aliases)?;
        }
        Expr::Hole => {}
        Expr::NonNullAssert(inner) => {
            expand_expr(inner, aliases)?;
        }
        Expr::Binary(op, a, b) => {
            expand_expr(a, aliases)?;
            expand_expr(b, aliases)?;
            let _ = op;
        }
        Expr::CustomOp(_, a, b) => {
            expand_expr(a, aliases)?;
            expand_expr(b, aliases)?;
        }
        Expr::Unary(_, a) => expand_expr(a, aliases)?,
        Expr::Ternary(c, a, b) => {
            expand_expr(c, aliases)?;
            expand_expr(a, aliases)?;
            expand_expr(b, aliases)?;
        }
        Expr::Call(call) => {
            for ta in &mut call.type_args {
                *ta = expand_type(ta)?;
            }
            if let Some(t) = &mut call.target {
                expand_expr(t, aliases)?;
            }
            for a in &mut call.args {
                expand_expr(a, aliases)?;
            }
        }
        Expr::NullConditionalCall(call) => {
            for ta in &mut call.type_args {
                *ta = expand_type(ta)?;
            }
            if let Some(t) = &mut call.target {
                expand_expr(t, aliases)?;
            }
            for a in &mut call.args {
                expand_expr(a, aliases)?;
            }
        }
        Expr::Field(inner, _) => expand_expr(inner, aliases)?,
        Expr::New(_, type_args, args) => {
            for ta in type_args {
                *ta = expand_type(ta)?;
            }
            for a in args {
                expand_expr(a, aliases)?;
            }
        }
        Expr::Match(inner, arms) => {
            expand_expr(inner, aliases)?;
            for arm in arms {
                for p in &mut arm.patterns {
                    expand_pattern(p, aliases)?;
                }
                if let Some(guard) = &mut arm.guard {
                    expand_expr(guard, aliases)?;
                }
                expand_expr(&mut arm.body, aliases)?;
            }
        }
        Expr::EnumVariant(_, _, args) => {
            for a in args {
                expand_expr(a, aliases)?;
            }
        }
        Expr::Tuple(items) => {
            for item in items {
                expand_expr(item, aliases)?;
            }
        }
        Expr::TupleIndex(inner, _) => expand_expr(inner, aliases)?,
        Expr::Range(a, b, _) => {
            expand_expr(a, aliases)?;
            expand_expr(b, aliases)?;
        }
        Expr::NullCoalesce(a, b) => {
            expand_expr(a, aliases)?;
            expand_expr(b, aliases)?;
        }
        Expr::NullConditionalField(inner, _) => expand_expr(inner, aliases)?,
        Expr::SuperCall(_, args) => {
            for a in args {
                expand_expr(a, aliases)?;
            }
        }
        Expr::With(inner, updates) => {
            expand_expr(inner, aliases)?;
            for (_, e) in updates {
                expand_expr(e, aliases)?;
            }
        }
        Expr::Block(body) => {
            for s in body {
                expand_stmt(s, aliases)?;
            }
        }
        Expr::TryUnwrap(inner) => expand_expr(inner, aliases)?,
        Expr::InterpolatedString(parts) => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    expand_expr(e, aliases)?;
                }
            }
        }
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Null
        | Expr::Var(_)
        | Expr::StaticField(..)
        | Expr::SuperField(_) => {}
    }
    Ok(())
}

fn expand_pattern(pattern: &mut Pattern, aliases: &HashMap<String, TypeAliasDecl>) -> Result<(), String> {
    match pattern {
        Pattern::EnumVariant(_, _, sub) => {
            for p in sub {
                expand_pattern(p, aliases)?;
            }
        }
        Pattern::RecordClass(_, sub) => {
            for p in sub {
                expand_pattern(p, aliases)?;
            }
        }
        Pattern::Range(a, b, _) => {
            expand_expr(a, aliases)?;
            expand_expr(b, aliases)?;
        }
        Pattern::Int(_)
        | Pattern::Float(_)
        | Pattern::Bool(_)
        | Pattern::StringLit(_)
        | Pattern::Null
        | Pattern::Wildcard
        | Pattern::Binding(_) => {}
    }
    Ok(())
}

/// Map a numeric type name to its AST type, or None if not a numeric type name.
pub fn numeric_type_from_name(name: &str) -> Option<Type> {
    Some(match name {
        "int8" => Type::Int8,
        "int16" => Type::Int16,
        "int32" => Type::Int32,
        "int64" => Type::Int64,
        "uint8" => Type::UInt8,
        "uint16" => Type::UInt16,
        "uint32" => Type::UInt32,
        "uint64" => Type::UInt64,
        "float32" => Type::Float32,
        "float64" => Type::Float64,
        _ => return None,
    })
}

/// Parser state.
pub struct Parser<'a> {
    tokens: &'a [Token],
    /// 1-based source line of each token (parallel to `tokens`).
    lines: &'a [usize],
    pos: usize,
    /// Inside an extension body: `this` in expressions parses as the
    /// synthesized `__self` receiver parameter.
    substitute_this: bool,
}

impl<'a> Parser<'a> {
    /// Create a parser over a token slice.
    pub fn new(tokens: &'a [Token], lines: &'a [usize]) -> Self {
        Self { tokens, lines, pos: 0, substitute_this: false }
    }

    /// Source line of the current token (or the last one at EOF).
    fn cur_line(&self) -> usize {
        self.lines
            .get(self.pos.min(self.lines.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    }

    /// Parse the token stream into a program AST.
    pub fn parse(mut self) -> Result<Program, String> {
        let mut decls = Vec::new();
        while !self.is_at_end() {
            self.skip_newlines();
            if self.is_at_end() {
                break;
            }
            if self.check(Token::Enum) {
                decls.push(Decl::Enum(self.parse_enum()?));
            } else if self.match_token(Token::Type) {
                decls.push(self.parse_type_decl()?);
            } else if self.match_token(Token::Newtype) {
                let name = self.consume_ident("expected newtype name")?;
                self.consume(Token::Assign, "expected `=` in newtype declaration")?;
                let underlying = self.parse_type()?;
                self.consume(Token::Semi, "expected `;` after newtype declaration")?;
                decls.push(Decl::Newtype(NewtypeDecl { name, underlying }));
            } else if self.match_token(Token::Interface) {
                decls.push(Decl::Class(self.parse_class(true, false, false)?));
            } else if self.match_token(Token::Abstract) {
                let is_sealed = self.match_token(Token::Sealed);
                self.consume(Token::Class, "expected `class` after `abstract`")?;
                decls.push(Decl::Class(self.parse_class(false, true, is_sealed)?));
            } else if self.match_token(Token::Sealed) {
                let is_abstract = self.match_token(Token::Abstract);
                self.consume(Token::Class, "expected `class` after `sealed`")?;
                decls.push(Decl::Class(self.parse_class(false, is_abstract, true)?));
            } else if self.match_token(Token::Record) {
                decls.push(Decl::Class(self.parse_record()?));
            } else if self.match_token(Token::Static) {
                // `static class Name { ... }`: the namespace idiom. No
                // instances, no inheritance, all members static.
                self.consume(Token::Class, "expected `class` after `static`")?;
                let mut c = self.parse_class(false, false, true)?;
                c.is_static = true;
                decls.push(Decl::Class(c));
            } else if matches!(self.peek(), Some(Token::Ident(k)) if k == "extension") {
                // `extension Name on Target { ... }` (contextual keyword).
                self.advance();
                decls.push(Decl::Class(self.parse_extension()?));
            } else {
                // Common newcomer mistakes get targeted hints before the
                // generic expectation message.
                let hint = match self.peek() {
                    Some(Token::Ident(word)) => match word.as_str() {
                        "fn" | "func" | "function" | "def" => Some((
                            word.clone(),
                            "free functions are not supported; declare a method inside a class \
(e.g. `class Program { static void Main() { ... } }`)",
                        )),
                        "struct" => {
                            Some((word.clone(), "Aura has no `struct`; use `class` or `record`"))
                        }
                        _ => None,
                    },
                    // `let`/`var` are keywords, so they need their own arms.
                    Some(Token::Let) => Some((
                        "let".to_string(),
                        "top-level variables are not supported; declare fields inside a class",
                    )),
                    Some(Token::Var) => Some((
                        "var".to_string(),
                        "top-level variables are not supported; declare fields inside a class",
                    )),
                    _ => None,
                };
                if let Some((word, hint)) = hint {
                    return Err(format!(
                        "line {}: unexpected `{}` at top level — {}",
                        self.cur_line(),
                        word,
                        hint
                    ));
                }
                self.consume(Token::Class, "expected `class`, `record`, or `interface`")?;
                decls.push(Decl::Class(self.parse_class(false, false, false)?));
            }
        }
        // Inject the standard `Exception` base class so it is always available
        // (fields `message` and `stackTrace`, populated by the VM on throw).
        decls.insert(0, Decl::Class(ClassDecl {
            name: "Exception".to_string(),
            line: 0,
            module: String::new(),
            is_static: false,
            generic_params: vec![],
            bases: vec![],
            is_interface: false,
            is_abstract: false,
            is_sealed: false,
            is_record: false,
            record_params: vec![],
            members: vec![
                Member::Field(FieldDecl {
                    is_static: false,
                    visibility: Visibility::Public,
                    ty: Type::String,
                    name: "message".to_string(),
                    init: None,
                }),
                Member::Field(FieldDecl {
                    is_static: false,
                    visibility: Visibility::Public,
                    ty: Type::String,
                    name: "stackTrace".to_string(),
                    init: None,
                }),
                // Exception chaining: the wrapped original, null when the
                // exception stands alone.
                Member::Field(FieldDecl {
                    is_static: false,
                    visibility: Visibility::Public,
                    ty: Type::Nullable(Box::new(Type::Class(
                        "Exception".to_string(),
                        Vec::new(),
                    ))),
                    name: "cause".to_string(),
                    init: None,
                }),
            ],
            extension_on: None,
            nested: Vec::new(),
        }));
        Ok(Program { decls })
    }

    /// Parse the body of a class/interface declaration, assuming the leading
    /// `class`/`interface` keyword has already been consumed.
    fn parse_class(
        &mut self,
        is_interface: bool,
        is_abstract: bool,
        is_sealed: bool,
    ) -> Result<ClassDecl, String> {
        self.parse_class_impl(is_interface, is_abstract, is_sealed, false)
    }

    /// Parse a `record` declaration. The leading `record` keyword has already
    /// been consumed.
    fn parse_record(&mut self) -> Result<ClassDecl, String> {
        self.parse_class_impl(false, false, false, true)
    }

    /// Parse `extension Name on Target { methods }` (the leading `extension`
    /// keyword has already been consumed) and desugar it to a static class:
    /// every method becomes static with a leading `__self` parameter of the
    /// target type, and `this` in bodies parses as `__self`.
    fn parse_extension(&mut self) -> Result<ClassDecl, String> {
        let decl_line = self.cur_line();
        let name = self.consume_ident("expected extension name")?;
        match self.peek() {
            Some(Token::Ident(k)) if k == "on" => {
                self.advance();
            }
            _ => return Err(format!("expected `on` after `extension {}`", name)),
        }
        let target = self.parse_type()?;
        self.consume(Token::LBrace, "expected `{`")?;
        self.substitute_this = true;
        let mut members = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            members.push(self.parse_member(false, &name)?);
        }
        self.substitute_this = false;
        self.consume(Token::RBrace, "expected `}`")?;
        for member in &mut members {
            let Member::Method(m) = member else {
                return Err(format!(
                    "extension `{}` can only contain methods",
                    name
                ));
            };
            if m.is_constructor {
                return Err(format!(
                    "extension `{}` cannot declare a constructor",
                    name
                ));
            }
            if crate::typer::operator_symbol_of(&m.name).is_some() {
                return Err(format!(
                    "extension `{}` cannot declare `{}`: operator overloads must be \
                     declared on the class itself",
                    name, m.name
                ));
            }
            if m.is_static || m.is_virtual || m.is_override || m.is_abstract || m.is_final {
                return Err(format!(
                    "extension method `{}.{}` cannot be {}: extension methods are \
                     written like instance methods",
                    name,
                    m.name,
                    if m.is_static {
                        "static"
                    } else if m.is_virtual {
                        "virtual"
                    } else if m.is_override {
                        "override"
                    } else if m.is_abstract {
                        "abstract"
                    } else {
                        "final"
                    }
                ));
            }
            if m.visibility != Visibility::Public {
                return Err(format!(
                    "extension method `{}.{}` must be public",
                    name, m.name
                ));
            }
            m.is_static = true;
            m.params.insert(
                0,
                Param {
                    ty: target.clone(),
                    name: "__self".to_string(),
                },
            );
        }
        Ok(ClassDecl {
            name,
            line: decl_line,
            module: String::new(),
            is_static: true,
            generic_params: Vec::new(),
            bases: Vec::new(),
            is_interface: false,
            is_abstract: false,
            is_sealed: true,
            is_record: false,
            record_params: Vec::new(),
            members,
            extension_on: Some(target),
            nested: Vec::new(),
        })
    }

    fn parse_class_impl(
        &mut self,
        is_interface: bool,
        is_abstract: bool,
        is_sealed: bool,
        is_record: bool,
    ) -> Result<ClassDecl, String> {
        let decl_line = self.cur_line();
        let name = self.consume_ident("expected class name")?;
        let mut generic_params = if self.check(Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };
        let record_params = if is_record {
            if self.check(Token::LParen) {
                self.parse_params()?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let bases = if self.match_token(Token::Colon) {
            let mut bases: Vec<(String, Vec<Type>)> = Vec::new();
            loop {
                let base = self.consume_ident("expected base type name after `:`")?;
                if base == name {
                    return Err(format!("`{}` cannot inherit from itself", name));
                }
                if bases.iter().any(|(b, _)| *b == base) {
                    return Err(format!("`{}` lists base type `{}` more than once", name, base));
                }
                // Generic interface instantiation: `: IProducer<Cat>`.
                let args = if self.check(Token::Lt) {
                    self.parse_type_args()?
                } else {
                    Vec::new()
                };
                bases.push((base, args));
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
            bases
        } else {
            Vec::new()
        };
        self.parse_where_clauses(&mut generic_params)?;
        let mut members = Vec::new();
        if is_record && self.match_token(Token::Semi) {
            // `record Point(int x, int y);` shorthand: no explicit members.
            return Ok(ClassDecl {
                name,
                line: decl_line,
            module: String::new(),
            is_static: false,
                generic_params,
                bases,
                is_interface,
                is_abstract,
                is_sealed,
                is_record,
                record_params,
                members,
                extension_on: None,
            nested: Vec::new(),
            });
        }
        self.consume(Token::LBrace, "expected `{`")?;
        let mut nested = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            // Nested class declarations: `class`, `record`, `interface`,
            // `static class`, `abstract class`, `sealed class`.
            let two = (self.peek(), self.tokens.get(self.pos + 1));
            let nested_decl = match two {
                (Some(Token::Class), _) => {
                    self.advance();
                    Some(self.parse_class(false, false, false)?)
                }
                (Some(Token::Record), _) => {
                    self.advance();
                    Some(self.parse_record()?)
                }
                (Some(Token::Interface), _) => {
                    self.advance();
                    Some(self.parse_class(true, false, false)?)
                }
                (Some(Token::Static), Some(Token::Class)) => {
                    self.advance();
                    self.advance();
                    let mut c = self.parse_class(false, false, true)?;
                    c.is_static = true;
                    Some(c)
                }
                (Some(Token::Abstract), Some(Token::Class)) => {
                    self.advance();
                    self.advance();
                    Some(self.parse_class(false, true, false)?)
                }
                (Some(Token::Sealed), Some(Token::Class)) => {
                    self.advance();
                    self.advance();
                    Some(self.parse_class(false, false, true)?)
                }
                _ => None,
            };
            if let Some(n) = nested_decl {
                if is_interface {
                    return Err(format!(
                        "interface `{}` cannot declare nested class `{}`",
                        name, n.name
                    ));
                }
                nested.push(n);
                continue;
            }
            members.push(self.parse_member(is_interface, &name)?);
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(ClassDecl {
            name,
            line: decl_line,
            module: String::new(),
            is_static: false,
            generic_params,
            bases,
            is_interface,
            is_abstract,
            is_sealed,
            is_record,
            record_params,
            members,
            extension_on: None,
            nested,
        })
    }

    fn parse_enum(&mut self) -> Result<EnumDecl, String> {
        self.consume(Token::Enum, "expected `enum`")?;
        let name = self.consume_ident("expected enum name")?;
        let generic_params = if self.check(Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };
        self.consume(Token::LBrace, "expected `{`")?;
        let mut variants = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            let variant_name = self.consume_ident("expected variant name")?;
            let fields = if self.check(Token::LParen) {
                self.consume(Token::LParen, "expected `(`")?;
                let mut fields = Vec::new();
                if !self.check(Token::RParen) {
                    loop {
                        let ty = self.parse_type()?;
                        let field_name = self.consume_ident("expected field name")?;
                        fields.push(EnumVariantField { ty, name: field_name });
                        if !self.match_token(Token::Comma) {
                            break;
                        }
                    }
                }
                self.consume(Token::RParen, "expected `)`")?;
                fields
            } else {
                Vec::new()
            };
            variants.push(EnumVariant { name: variant_name, fields });
            self.skip_newlines();
            if !self.check(Token::RBrace) {
                self.match_token(Token::Comma);
            }
            self.skip_newlines();
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(EnumDecl { name, generic_params, variants })
    }

    /// Parse a sum-type declaration: `type Name<T, U> = V1 | V2(int) | ...;`
    /// Desugars into an [`EnumDecl`] with positional (unnamed) variant fields.
    fn parse_type_decl(&mut self) -> Result<Decl, String> {
        let name = self.consume_ident("expected type name after `type`")?;
        let generic_params = if self.check(Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };
        self.consume(Token::Assign, "expected `=` in type declaration")?;
        // A string-literal union: `type Direction = "north" | "south";`.
        // Stored as an alias whose target is the opaque `Type::LiteralUnion`,
        // so the existing alias-expansion pass resolves every use of the name.
        // A pipe list containing at least one string literal is a
        // literal-union declaration; operands may also name other literal
        // unions, merged during alias expansion. (An all-ident pipe list
        // stays a sum-type candidate and is reinterpreted there when every
        // name resolves to a literal union.)
        let looks_like_literal_union = {
            let mut pos = self.pos;
            let mut saw_lit = false;
            loop {
                match self.tokens.get(pos) {
                    Some(Token::StringLit(_)) => {
                        saw_lit = true;
                        pos += 1;
                    }
                    Some(Token::Ident(_)) => pos += 1,
                    _ => break false,
                }
                match self.tokens.get(pos) {
                    Some(Token::Pipe) => pos += 1,
                    Some(Token::Semi) => break saw_lit,
                    _ => break false,
                }
            }
        };
        if looks_like_literal_union {
            if !generic_params.is_empty() {
                return Err(format!(
                    "literal type `{}` cannot have generic parameters",
                    name
                ));
            }
            let mut operands = Vec::new();
            loop {
                match self.peek().cloned() {
                    Some(Token::StringLit(v)) => {
                        self.advance();
                        // Explicit duplicate literals in one declaration are
                        // almost certainly typos; duplicates arising through
                        // composed unions dedup silently at resolution.
                        if operands.contains(&UnionOperand::Literal(v.clone())) {
                            return Err(format!(
                                "duplicate literal {:?} in literal type `{}`",
                                v, name
                            ));
                        }
                        operands.push(UnionOperand::Literal(v));
                    }
                    Some(Token::Ident(n)) => {
                        self.advance();
                        operands.push(UnionOperand::Named(n));
                    }
                    _ => {
                        return Err(format!(
                            "expected string literal or literal-type name in literal type `{}`",
                            name
                        ));
                    }
                }
                if !self.match_token(Token::Pipe) {
                    break;
                }
            }
            self.consume(Token::Semi, "expected `;` after type declaration")?;
            return Ok(Decl::LiteralUnion(LiteralUnionDecl { name, operands }));
        }
        // A `type` declaration is either a sum type (`type Result<T,E> = Ok(T) | Err(E);`)
        // or an alias to a single type (`type UserId = int;`). A sum type starts
        // with a variant name followed by `(` (fields) or `|` (bare variants).
        let is_sum = matches!(self.tokens.get(self.pos), Some(Token::Ident(_)))
            && matches!(self.tokens.get(self.pos + 1), Some(Token::LParen | Token::Pipe));
        if is_sum {
            let mut variants = Vec::new();
            loop {
                let variant_name = self.consume_ident("expected variant name")?;
                let fields = if self.check(Token::LParen) {
                    self.consume(Token::LParen, "expected `(`")?;
                    let mut fields = Vec::new();
                    if !self.check(Token::RParen) {
                        loop {
                            let ty = self.parse_type()?;
                            fields.push(EnumVariantField { ty, name: String::new() });
                            if !self.match_token(Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.consume(Token::RParen, "expected `)`")?;
                    fields
                } else {
                    Vec::new()
                };
                variants.push(EnumVariant { name: variant_name, fields });
                if !self.match_token(Token::Pipe) {
                    break;
                }
            }
            self.consume(Token::Semi, "expected `;` after type declaration")?;
            Ok(Decl::Enum(EnumDecl { name, generic_params, variants }))
        } else {
            let target = self.parse_type()?;
            self.consume(Token::Semi, "expected `;` after type declaration")?;
            Ok(Decl::TypeAlias(TypeAliasDecl { name, generic_params, target }))
        }
    }

    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>, String> {
        self.consume(Token::Lt, "expected `<`")?;
        let mut params = Vec::new();
        loop {
            // Variance annotation: `<out T>` (covariant) / `<in T>`
            // (contravariant). `out` is contextual — only a variance marker
            // when another identifier follows.
            let variance = if self.match_token(Token::In) {
                Variance::Contravariant
            } else if matches!(self.peek(), Some(Token::Ident(k)) if k == "out")
                && matches!(self.tokens.get(self.pos + 1), Some(Token::Ident(_)))
            {
                self.advance();
                Variance::Covariant
            } else {
                Variance::Invariant
            };
            let name = self.consume_ident("expected generic parameter name")?;
            let mut gp = GenericParam {
                name,
                variance,
                bounds: Vec::new(),
                union: Vec::new(),
                requires_new: false,
            };
            if self.match_token(Token::Colon) {
                self.parse_one_constraint(&mut gp)?;
            }
            params.push(gp);
            if !self.match_token(Token::Comma) {
                break;
            }
        }
        self.consume(Token::Gt, "expected `>`")?;
        Ok(params)
    }

    /// Parse one constraint atom onto `gp`: a bound type (`IComparable`), a
    /// union (`int | float`), or `new()`. Used by both the inline form
    /// (`<T : int | float>`) and `where` clauses, which may comma-combine a
    /// bound with `new()`.
    fn parse_one_constraint(&mut self, gp: &mut GenericParam) -> Result<(), String> {
        if self.match_token(Token::New) {
            self.consume(Token::LParen, "expected `(` after `new` in a constraint")?;
            self.consume(Token::RParen, "expected `)` after `new(` in a constraint")?;
            if gp.requires_new {
                return Err(format!("duplicate `new()` constraint on `{}`", gp.name));
            }
            if !gp.union.is_empty() {
                return Err(format!(
                    "`new()` cannot be combined with a union constraint on `{}`",
                    gp.name
                ));
            }
            gp.requires_new = true;
            return Ok(());
        }
        let first = self.parse_type()?;
        if self.check(Token::Pipe) {
            if !gp.union.is_empty() {
                return Err(format!(
                    "type parameter `{}` is already constrained",
                    gp.name
                ));
            }
            if gp.requires_new || !gp.bounds.is_empty() {
                return Err(format!(
                    "a union constraint on `{}` cannot be combined with other constraints",
                    gp.name
                ));
            }
            let mut alts = vec![first];
            while self.match_token(Token::Pipe) {
                alts.push(self.parse_type()?);
            }
            gp.union = alts;
        } else {
            if !gp.union.is_empty() {
                return Err(format!(
                    "a union constraint on `{}` cannot be combined with other constraints",
                    gp.name
                ));
            }
            // Multiple subtype bounds accumulate: `where T : Entity, ICloneable`.
            gp.bounds.push(first);
        }
        Ok(())
    }

    /// Parse zero or more `where T : constraint` clauses following a class
    /// header or method signature, attaching each to its declared type
    /// parameter.
    fn parse_where_clauses(&mut self, params: &mut [GenericParam]) -> Result<(), String> {
        while matches!(self.peek(), Some(Token::Ident(k)) if k == "where") {
            self.advance();
            let pname = self.consume_ident("expected type parameter name after `where`")?;
            self.consume(Token::Colon, "expected `:` in `where` clause")?;
            let Some(gp) = params.iter_mut().find(|g| g.name == pname) else {
                return Err(format!(
                    "`where {}`: no type parameter of that name is declared",
                    pname
                ));
            };
            loop {
                self.parse_one_constraint(gp)?;
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
        }
        Ok(())
    }

    fn parse_member(&mut self, in_interface: bool, class_name: &str) -> Result<Member, String> {
        let decl_line = self.cur_line();
        let mut is_static = false;
        let mut visibility = Visibility::Public;
        let mut is_virtual = false;
        let mut is_override = false;
        let mut is_abstract = false;
        let mut is_final = false;
        let mut is_async = false;
        loop {
            if self.match_token(Token::Static) {
                is_static = true;
            } else if matches!(self.peek(), Some(Token::Ident(k)) if k == "async") {
                self.advance();
                is_async = true;
            } else if self.match_token(Token::Protected) {
                visibility = Visibility::Protected;
            } else if self.match_token(Token::Private) {
                visibility = Visibility::Private;
            } else if self.match_token(Token::Internal) {
                visibility = Visibility::Internal;
            } else if self.match_token(Token::Virtual) {
                is_virtual = true;
            } else if self.match_token(Token::Override) {
                is_override = true;
            } else if self.match_token(Token::Abstract) {
                is_abstract = true;
            } else if self.match_token(Token::Final) {
                is_final = true;
            } else if self.match_token(Token::Sealed) {
                return Err("`sealed` can only be applied to a class declaration".to_string());
            } else {
                break;
            }
        }
        let mut generic_params = if self.check(Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        // A constructor is declared with no return type: `Counter(int start) { ... }`.
        if let Some(Token::Ident(first)) = self.peek() {
            if first == class_name && self.pos + 1 < self.tokens.len() {
                if matches!(self.tokens.get(self.pos + 1), Some(Token::LParen)) {
                    if in_interface {
                        return Err(format!(
                            "interface `{}` cannot declare a constructor",
                            class_name
                        ));
                    }
                    if is_async {
                        return Err(format!(
                            "constructor `{}` cannot be async",
                            class_name
                        ));
                    }
                    if is_static || is_virtual || is_override || is_abstract || is_final {
                        return Err(format!(
                            "constructor `{}` cannot be {}",
                            class_name,
                            if is_static {
                                "static"
                            } else if is_virtual {
                                "virtual"
                            } else if is_override {
                                "override"
                            } else if is_abstract {
                                "abstract"
                            } else {
                                "final"
                            }
                        ));
                    }
                    if !generic_params.is_empty() {
                        return Err(format!(
                            "constructor `{}` cannot have generic parameters",
                            class_name
                        ));
                    }
                    self.advance();
                    let params = self.parse_params()?;
                    let mut constructor_chain = None;
                    if self.match_token(Token::Colon) {
                        if self.match_token(Token::Super) {
                            let args = self.parse_args()?;
                            constructor_chain = Some(ConstructorChain {
                                target: ConstructorTarget::Base,
                                args,
                            });
                        } else if let Some(Token::Ident(t)) = self.peek() {
                            if t == "this" {
                                self.advance();
                                let args = self.parse_args()?;
                                constructor_chain = Some(ConstructorChain {
                                    target: ConstructorTarget::This,
                                    args,
                                });
                            } else {
                                return Err(format!(
                                    "expected `super` or `this` after `:` in constructor `{}`",
                                    class_name
                                ));
                            }
                        } else {
                            return Err(format!(
                                "expected `super` or `this` after `:` in constructor `{}`",
                                class_name
                            ));
                        }
                    }
                    self.consume(Token::LBrace, "expected `{`")?;
                    let body = self.parse_block()?;
                    return Ok(Member::Method(MethodDecl {
                        line: decl_line,
                        is_static: false,
                        visibility,
                        is_virtual: false,
                        is_override: false,
                        is_abstract: false,
                        is_final: false,
                        is_constructor: true,
                        constructor_chain,
                        generic_params: Vec::new(),
                        return_ty: Type::Unit,
                        name: class_name.to_string(),
                        params,
                        throws: Vec::new(),
                        body,
                        is_async: false,
                    }));
                }
            }
        }
        let ty = self.parse_type()?;
        let mut name = self.consume_ident("expected member name")?;

        // Operator overload: `Vector operator+(Vector other) { ... }`. The
        // declaration is an ordinary instance method named `operator+`;
        // `a + b` lowers to a call on the left operand. (`operator` alone
        // remains a valid member name.)
        if name == "operator" {
            let sym: Option<String> = if self.match_token(Token::Plus) {
                Some("+".to_string())
            } else if self.match_token(Token::Minus) {
                Some("-".to_string())
            } else if self.match_token(Token::Star) {
                Some("*".to_string())
            } else if self.match_token(Token::Slash) {
                Some("/".to_string())
            } else if self.match_token(Token::Percent) {
                Some("%".to_string())
            } else if self.match_token(Token::Eq) {
                Some("==".to_string())
            } else if self.match_token(Token::Lt) {
                Some("<".to_string())
            } else if self.match_token(Token::Le) {
                Some("<=".to_string())
            } else if self.match_token(Token::Gt) {
                Some(">".to_string())
            } else if self.match_token(Token::Ge) {
                Some(">=".to_string())
            } else if let Some(Token::CustomOp(sym)) = self.peek() {
                // Custom operator overload: `Vector operator|>(Vector other)`.
                let sym = sym.clone();
                self.advance();
                Some(sym)
            } else if self.check(Token::Ne) {
                return Err(
                    "`operator!=` cannot be declared: declare `operator==` and `!=` is \
                     derived as its negation"
                        .to_string(),
                );
            } else {
                None
            };
            if let Some(sym) = sym {
                if in_interface {
                    return Err(format!(
                        "interface `{}` cannot declare `operator{}`: operator overloads \
                         must be declared on a class",
                        class_name, sym
                    ));
                }
                if is_static || is_virtual || is_override || is_abstract || is_final {
                    return Err(format!(
                        "`operator{}` cannot be {}: operator overloads are plain instance \
                         methods",
                        sym,
                        if is_static {
                            "static"
                        } else if is_virtual {
                            "virtual"
                        } else if is_override {
                            "override"
                        } else if is_abstract {
                            "abstract"
                        } else {
                            "final"
                        }
                    ));
                }
                if visibility != Visibility::Public {
                    return Err(format!("`operator{}` must be public", sym));
                }
                if !generic_params.is_empty() {
                    return Err(format!("`operator{}` cannot have generic parameters", sym));
                }
                if !self.check(Token::LParen) {
                    return Err(format!("expected parameter list after `operator{}`", sym));
                }
                name = format!("operator{}", sym);
            }
        }

        if self.check(Token::LParen) {
            let params = self.parse_params()?;
            // Checked-exception contract: `throws IOException, ParseError`.
            let mut throws: Vec<String> = Vec::new();
            if matches!(self.peek(), Some(Token::Ident(k)) if k == "throws") {
                self.advance();
                loop {
                    let mut n = self.consume_ident("expected exception type after `throws`")?;
                    while matches!(self.peek(), Some(Token::Dot))
                        && matches!(self.tokens.get(self.pos + 1), Some(Token::Ident(_)))
                    {
                        self.advance();
                        let part = self.consume_ident("expected name after `.`")?;
                        n.push('.');
                        n.push_str(&part);
                    }
                    if throws.contains(&n) {
                        return Err(format!("`{}` is listed in `throws` more than once", n));
                    }
                    throws.push(n);
                    if !self.match_token(Token::Comma) {
                        break;
                    }
                }
            }
            self.parse_where_clauses(&mut generic_params)?;
            let body = if is_abstract || (in_interface && !self.check(Token::LBrace)) {
                if is_static || is_virtual {
                    return Err(format!("abstract method `{}` cannot be {}",
                        name, if is_static { "static" } else { "virtual" }));
                }
                self.consume(Token::Semi, "expected `;` after abstract method")?;
                Vec::new()
            } else {
                self.consume(Token::LBrace, "expected `{`")?;
                self.parse_block()?
            };
            Ok(Member::Method(MethodDecl {
                line: decl_line,
                is_static,
                visibility,
                is_virtual,
                is_override,
                is_abstract,
                is_final,
                is_constructor: false,
                constructor_chain: None,
                generic_params,
                return_ty: ty,
                name,
                params,
                throws,
                body,
                is_async,
            }))
        } else if self.check(Token::LBrace) {
            if is_virtual || is_override || is_abstract || is_final {
                return Err(format!("`{}` cannot be used on a property",
                    if is_abstract { "abstract" } else if is_virtual { "virtual" } else if is_override { "override" } else { "final" }));
            }
            if !generic_params.is_empty() {
                return Err(format!("property `{}` cannot have generic parameters", name));
            }
            self.consume(Token::LBrace, "expected `{`")?;
            let mut getter = None;
            let mut setter = None;
            loop {
                self.skip_newlines();
                if self.check(Token::RBrace) {
                    break;
                }
                let accessor_visibility = if self.match_token(Token::Private) {
                    Some(Visibility::Private)
                } else if self.match_token(Token::Protected) {
                    Some(Visibility::Protected)
                } else if self.match_token(Token::Internal) {
                    Some(Visibility::Internal)
                } else {
                    None
                };
                if let Some(Token::Ident(kw)) = self.peek() {
                    if kw == "get" || kw == "set" {
                        let kw = kw.clone();
                        self.advance();
                        let kind = if self.match_token(Token::Semi) {
                            AccessorKind::Auto
                        } else {
                            self.consume(Token::LBrace, "expected `;` or `{` after accessor")?;
                            let body = self.parse_block()?;
                            AccessorKind::Body(body)
                        };
                        let accessor = Accessor {
                            visibility: accessor_visibility,
                            kind,
                        };
                        if kw == "get" {
                            if getter.is_some() {
                                return Err(format!("property `{}` has more than one getter", name));
                            }
                            getter = Some(accessor);
                        } else {
                            if setter.is_some() {
                                return Err(format!("property `{}` has more than one setter", name));
                            }
                            setter = Some(accessor);
                        }
                        continue;
                    }
                }
                return Err(format!("expected `get` or `set` accessor in property `{}`", name));
            }
            self.consume(Token::RBrace, "expected `}`")?;
            if getter.is_none() && setter.is_none() {
                return Err(format!("property `{}` must declare a getter or setter", name));
            }
            Ok(Member::Property(PropertyDecl {
                is_static,
                visibility,
                ty,
                name,
                getter,
                setter,
            }))
        } else {
            if is_virtual || is_override || is_abstract || is_final {
                return Err(format!("`{}` cannot be used on a field",
                    if is_abstract { "abstract" } else if is_virtual { "virtual" } else if is_override { "override" } else { "final" }));
            }
            let init = if self.match_token(Token::Assign) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.consume(Token::Semi, "expected `;`")?;
            Ok(Member::Field(FieldDecl {
                is_static,
                visibility,
                ty,
                name,
                init,
            }))
        }
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, String> {
        self.consume(Token::LParen, "expected `(`")?;
        let mut params = Vec::new();
        if !self.check(Token::RParen) {
            loop {
                let ty = self.parse_type()?;
                let name = self.consume_ident("expected parameter name")?;
                params.push(Param { ty, name });
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
        }
        self.consume(Token::RParen, "expected `)`")?;
        Ok(params)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            stmts.push(Stmt::Mark(self.cur_line()));
            stmts.push(self.parse_stmt()?);
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        
        // Check for labeled statement: identifier followed by colon
        let label = if let Some(Token::Ident(name)) = self.peek() {
            let name = name.clone();
            // Look ahead to see if next token is colon
            if self.pos + 1 < self.tokens.len() {
                if matches!(self.tokens.get(self.pos + 1), Some(Token::Colon)) {
                    self.advance(); // consume identifier
                    self.advance(); // consume colon
                    self.skip_newlines(); // skip newlines after label
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        
        if self.match_token(Token::If) {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_if()
        } else if self.match_token(Token::While) {
            self.parse_while(label)
        } else if self.match_token(Token::For) {
            self.parse_for(label)
        } else if self.match_token(Token::Do) {
            self.parse_do_while(label)
        } else if self.match_token(Token::Break) {
            let break_label = if let Some(Token::Ident(name)) = self.peek() {
                let name = name.clone();
                self.advance();
                Some(name)
            } else {
                None
            };
            self.consume_semi()?;
            Ok(Stmt::Break(break_label))
        } else if self.match_token(Token::Continue) {
            let continue_label = if let Some(Token::Ident(name)) = self.peek() {
                let name = name.clone();
                self.advance();
                Some(name)
            } else {
                None
            };
            self.consume_semi()?;
            Ok(Stmt::Continue(continue_label))
        } else if self.match_token(Token::Return) {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_return()
        } else if self.match_token(Token::Throw) {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_throw()
        } else if self.match_token(Token::Try) {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_try()
        } else if self.match_token(Token::Using) {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_using()
        } else if self.check(Token::LParen) && self.peek_ahead_is_tuple_decl() {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_tuple_decl()
        } else if self.check_type() && self.peek_ahead_is_ident_after_type() {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.parse_var_decl()
        } else if self.check(Token::LBrace) {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            self.advance();
            Ok(Stmt::Block(self.parse_block()?))
        } else {
            if label.is_some() {
                return Err("labels can only be applied to loops".to_string());
            }
            let expr = self.parse_expr()?;
            if self.match_token(Token::Assign) {
                let target = expr_to_assign_target(expr)?;
                let value = self.parse_expr()?;
                self.consume_semi()?;
                Ok(Stmt::Assign(target, value))
            } else {
                self.consume_semi()?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn peek_ahead_is_tuple_decl(&self) -> bool {
        let mut pos = self.pos;
        if pos >= self.tokens.len() || !matches!(self.tokens.get(pos), Some(Token::LParen)) {
            return false;
        }
        pos += 1;
        
        // Find the matching closing paren, tracking paren depth
        let mut depth = 1;
        let start_pos = pos;
        while pos < self.tokens.len() && depth > 0 {
            match self.tokens.get(pos) {
                Some(Token::LParen) => depth += 1,
                Some(Token::RParen) => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                pos += 1;
            }
        }
        
        if depth != 0 {
            return false;
        }
        
        // pos is now at the closing paren
        let close_paren_pos = pos;
        pos += 1;
        
        if pos >= self.tokens.len() {
            return false;
        }
        
        // Check what follows the closing paren
        // Case 1: Tuple type declaration: (type, type) name = expr;
        // Case 2: Tuple destructuring: (type name, type name) = expr;
        
        let is_destructure = matches!(self.tokens.get(pos), Some(Token::Assign));
        let is_type_decl = matches!(self.tokens.get(pos), Some(Token::Ident(_)));
        
        if !is_destructure && !is_type_decl {
            return false;
        }
        
        if is_type_decl {
            pos += 1;
            if pos >= self.tokens.len() || !matches!(self.tokens.get(pos), Some(Token::Assign)) {
                return false;
            }
        }
        
        // Now verify the content inside parens looks like a tuple
        // It should have at least one comma at depth 0
        let mut has_comma = false;
        let mut inner_depth = 0;
        for i in start_pos..close_paren_pos {
            match self.tokens.get(i) {
                Some(Token::LParen) => inner_depth += 1,
                Some(Token::RParen) => inner_depth -= 1,
                Some(Token::Comma) if inner_depth == 0 => has_comma = true,
                _ => {}
            }
        }
        
        has_comma
    }

    fn parse_tuple_decl(&mut self) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(`")?;
        
        // Try to parse as tuple type declaration first: (type1, type2) name = ...
        // or tuple destructuring: (type1 name1, type2 name2) = ...
        
        let first_type = self.parse_type()?;
        
        // Check what comes after the first type
        if self.check(Token::Comma) || self.check(Token::RParen) {
            // This is tuple type declaration: (type1, type2) name = ...
            let mut types = vec![first_type];
            
            while self.match_token(Token::Comma) {
                types.push(self.parse_type()?);
            }
            
            self.consume(Token::RParen, "expected `)`")?;
            let name = self.consume_ident("expected variable name")?;
            self.consume(Token::Assign, "expected `=`")?;
            let expr = self.parse_expr()?;
            self.consume_semi()?;
            
            // Create a VarDecl with tuple type
            Ok(Stmt::VarDecl(Type::Tuple(types), name, Some(expr)))
        } else if self.check(Token::Ident(String::new())) {
            // This is tuple destructuring: (type name, ...)
            let mut names = Vec::new();
            let name = self.consume_ident("expected variable name")?;
            names.push(name);
            
            while self.match_token(Token::Comma) {
                let _ty = self.parse_type()?;
                let name = self.consume_ident("expected variable name")?;
                names.push(name);
            }
            
            self.consume(Token::RParen, "expected `)`")?;
            self.consume(Token::Assign, "expected `=`")?;
            let expr = self.parse_expr()?;
            self.consume_semi()?;
            Ok(Stmt::TupleDecl(names, expr))
        } else {
            Err("expected `,` or variable name in tuple declaration".to_string())
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        // Check for `if let` syntax
        if self.match_token(Token::Let) {
            return self.parse_if_let();
        }
        
        self.consume(Token::LParen, "expected `(` after `if`")?;
        let cond = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        let then_branch = self.parse_stmt_body()?;
        let else_branch = if self.match_token(Token::Else) {
            Some(self.parse_stmt_body()?)
        } else {
            None
        };
        Ok(Stmt::If(cond, then_branch, else_branch))
    }
    
    fn parse_if_let(&mut self) -> Result<Stmt, String> {
        // Parse: if let pattern = expr { ... } else { ... }
        let pattern = self.parse_pattern()?;
        self.consume(Token::Assign, "expected `=` after pattern in `if let`")?;
        let expr = self.parse_expr()?;
        let then_branch = self.parse_stmt_body()?;
        let else_branch = if self.match_token(Token::Else) {
            Some(self.parse_stmt_body()?)
        } else {
            None
        };
        Ok(Stmt::IfLet(pattern, expr, then_branch, else_branch))
    }

    fn parse_while(&mut self, label: Option<String>) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(` after `while`")?;
        let cond = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        let body = self.parse_stmt_body()?;
        Ok(Stmt::While {
            label,
            condition: cond,
            body,
        })
    }

    fn parse_for(&mut self, label: Option<String>) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(` after `for`")?;
        
        // Check for `for (Type var in expr)` syntax
        // We need to look ahead past the type and identifier to see if 'in' follows
        let is_for_in = self.peek_for_in_pattern();
        
        if is_for_in {
            // Parse for-in loop
            let ty = self.parse_type()?;
            let var_name = self.consume_ident("expected variable name")?;
            self.consume(Token::In, "expected `in`")?;
            let range_expr = self.parse_expr()?;
            self.consume(Token::RParen, "expected `)` after for-in range")?;
            let body = self.parse_stmt_body()?;
            return Ok(Stmt::ForIn {
                label,
                var_type: ty,
                var_name,
                iterable: range_expr,
                body,
            });
        }
        
        // Parse init statement (can be var decl, expr, or empty)
        let init = if self.check(Token::Semi) {
            self.advance(); // consume ;
            Stmt::Block(vec![]) // empty init
        } else if self.check_type() && self.peek_ahead_is_ident_after_type() {
            let stmt = self.parse_var_decl()?;
            stmt
        } else {
            let expr = self.parse_expr()?;
            if self.match_token(Token::Assign) {
                let target = expr_to_assign_target(expr)?;
                let value = self.parse_expr()?;
                self.consume(Token::Semi, "expected `;`")?;
                Stmt::Assign(target, value)
            } else {
                self.consume(Token::Semi, "expected `;`")?;
                Stmt::Expr(expr)
            }
        };
        
        // Parse condition (can be empty)
        let cond = if self.check(Token::Semi) {
            Expr::Bool(true) // empty condition = always true
        } else {
            self.parse_expr()?
        };
        self.consume(Token::Semi, "expected `;`")?;
        
        // Parse update (can be empty)
        let update = if self.check(Token::RParen) {
            Stmt::Block(vec![]) // empty update
        } else {
            let expr = self.parse_expr()?;
            if self.match_token(Token::Assign) {
                let target = expr_to_assign_target(expr)?;
                let value = self.parse_expr()?;
                Stmt::Assign(target, value)
            } else {
                Stmt::Expr(expr)
            }
        };
        
        self.consume(Token::RParen, "expected `)`")?;
        let body = self.parse_stmt_body()?;
        
        Ok(Stmt::For {
            label,
            init: Box::new(init),
            condition: cond,
            update: Box::new(update),
            body,
        })
    }
    
    /// Check if we're at the start of a `for (Type var in ...)` pattern
    fn peek_for_in_pattern(&self) -> bool {
        let mut pos = self.pos;
        
        // Check for type
        if pos >= self.tokens.len() || !self.check_type_token(&self.tokens[pos]) {
            return false;
        }
        pos += 1;
        
        // Skip generic type arguments if present
        if pos < self.tokens.len() && matches!(self.tokens.get(pos), Some(Token::Lt)) {
            let mut depth = 1;
            pos += 1;
            while pos < self.tokens.len() && depth > 0 {
                match self.tokens.get(pos) {
                    Some(Token::Lt) => depth += 1,
                    Some(Token::Gt) => depth -= 1,
                    None => return false,
                    _ => {}
                }
                pos += 1;
            }
        }
        
        // Skip a nullable marker (`T?`) if present
        if matches!(self.tokens.get(pos), Some(Token::Question)) {
            pos += 1;
        }

        // Check for identifier
        if pos >= self.tokens.len() || !matches!(self.tokens.get(pos), Some(Token::Ident(_))) {
            return false;
        }
        pos += 1;
        
        // Check for 'in'
        matches!(self.tokens.get(pos), Some(Token::In))
    }

    fn parse_do_while(&mut self, label: Option<String>) -> Result<Stmt, String> {
        let body = self.parse_stmt_body()?;
        self.consume(Token::While, "expected `while` after `do` body")?;
        self.consume(Token::LParen, "expected `(` after `while`")?;
        let cond = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        self.consume_semi()?;
        Ok(Stmt::DoWhile {
            label,
            body,
            condition: cond,
        })
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        let expr = if self.check(Token::Semi) || self.check(Token::Newline) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.consume_semi()?;
        Ok(Stmt::Return(expr))
    }

    fn parse_throw(&mut self) -> Result<Stmt, String> {
        let expr = self.parse_expr()?;
        self.consume_semi()?;
        Ok(Stmt::Throw(expr))
    }

    fn parse_try(&mut self) -> Result<Stmt, String> {
        let try_body = self.parse_stmt_body()?;
        let mut catches = Vec::new();
        loop {
            self.skip_newlines();
            if self.match_token(Token::Catch) {
                self.consume(Token::LParen, "expected `(` after `catch`")?;
                let ty = self.parse_type()?;
                let name = self.consume_ident("expected catch variable name")?;
                self.consume(Token::RParen, "expected `)` after catch variable")?;
                let body = self.parse_stmt_body()?;
                catches.push(CatchClause { ty, name, body });
            } else {
                break;
            }
        }
        self.skip_newlines();
        let finally_body = if self.match_token(Token::Finally) {
            Some(self.parse_stmt_body()?)
        } else {
            None
        };
        if catches.is_empty() && finally_body.is_none() {
            return Err("expected `catch` or `finally` after `try` block".to_string());
        }
        Ok(Stmt::Try {
            try_body,
            catches,
            finally_body,
        })
    }

    fn parse_using(&mut self) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(` after `using`")?;
        let (resource_ty, name, expr) = if self.check_type() && self.peek_ahead_is_ident_after_type() {
            let ty = self.parse_type()?;
            let n = self.consume_ident("expected resource variable name")?;
            self.consume(Token::Assign, "expected `=` in `using` declaration")?;
            let e = self.parse_expr()?;
            (Some(ty), Some(n), e)
        } else {
            let e = self.parse_expr()?;
            (None, None, e)
        };
        self.consume(Token::RParen, "expected `)` after `using` resource")?;
        let body = self.parse_stmt_body()?;
        Ok(Stmt::Using {
            resource_ty,
            name,
            expr: Box::new(expr),
            body,
        })
    }

    fn parse_var_decl(&mut self) -> Result<Stmt, String> {
        let ty = self.parse_type()?;
        let name = self.consume_ident("expected variable name")?;
        let init = if self.match_token(Token::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.consume_semi()?;
        Ok(Stmt::VarDecl(ty, name, init))
    }

    fn parse_stmt_body(&mut self) -> Result<Vec<Stmt>, String> {
        self.skip_newlines();
        if self.match_token(Token::LBrace) {
            self.parse_block()
        } else {
            let line = self.cur_line();
            Ok(vec![Stmt::Mark(line), self.parse_stmt()?])
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_binary(0)?;
        // Record copy-with: `p with { x = 5, y = 6 }`.
        while self.match_token(Token::With) {
            self.consume(Token::LBrace, "expected `{` after `with`")?;
            let mut updates = Vec::new();
            if !self.check(Token::RBrace) {
                loop {
                    let field = self.consume_ident("expected field name in `with` block")?;
                    self.consume(Token::Assign, "expected `=` after field name in `with` block")?;
                    let value = self.parse_expr()?;
                    updates.push((field, value));
                    if !self.match_token(Token::Comma) {
                        break;
                    }
                }
            }
            self.consume(Token::RBrace, "expected `}` after `with` block")?;
            expr = Expr::With(Box::new(expr), updates);
        }
        if self.match_token(Token::Question) {
            let then_expr = self.parse_expr()?;
            self.consume(Token::Colon, "expected `:` in ternary expression")?;
            let else_expr = self.parse_expr()?;
            Ok(Expr::Ternary(Box::new(expr), Box::new(then_expr), Box::new(else_expr)))
        } else {
            Ok(expr)
        }
    }

    /// Binary-expression parsing by precedence climbing over one table
    /// (replaces the old eight-layer recursive-descent chain). Tiers,
    /// loosest first: `??`, `||`, `&&`, equality, relational/`is`,
    /// ranges, custom operators (one shared tier), additive,
    /// multiplicative. All binary operators are left-associative; ranges
    /// do not chain (explicit error instead of a confusing downstream
    /// one).
    fn parse_binary(&mut self, min_bp: u8) -> Result<Expr, String> {
        const BP_COALESCE: u8 = 1;
        const BP_OR: u8 = 2;
        const BP_AND: u8 = 3;
        const BP_EQUALITY: u8 = 4;
        const BP_RELATIONAL: u8 = 5;
        const BP_RANGE: u8 = 6;
        const BP_CUSTOM: u8 = 7;
        const BP_ADDITIVE: u8 = 8;
        const BP_MULTIPLICATIVE: u8 = 9;

        let mut left = self.parse_unary()?;
        loop {
            enum Op {
                Simple(BinOp, u8),
                Coalesce,
                Is,
                Range(bool),
                Custom(String),
            }
            let op = match self.peek() {
                Some(Token::NullCoalesce) => Op::Coalesce,
                Some(Token::Or) => Op::Simple(BinOp::Or, BP_OR),
                Some(Token::And) => Op::Simple(BinOp::And, BP_AND),
                Some(Token::Eq) => Op::Simple(BinOp::Eq, BP_EQUALITY),
                Some(Token::Ne) => Op::Simple(BinOp::Ne, BP_EQUALITY),
                Some(Token::Lt) => Op::Simple(BinOp::Lt, BP_RELATIONAL),
                Some(Token::Le) => Op::Simple(BinOp::Le, BP_RELATIONAL),
                Some(Token::Gt) => Op::Simple(BinOp::Gt, BP_RELATIONAL),
                Some(Token::Ge) => Op::Simple(BinOp::Ge, BP_RELATIONAL),
                Some(Token::Is) => Op::Is,
                Some(Token::DotDot) => Op::Range(false),
                Some(Token::DotDotEq) => Op::Range(true),
                Some(Token::Plus) => Op::Simple(BinOp::Add, BP_ADDITIVE),
                Some(Token::Minus) => Op::Simple(BinOp::Sub, BP_ADDITIVE),
                Some(Token::Star) => Op::Simple(BinOp::Mul, BP_MULTIPLICATIVE),
                Some(Token::Slash) => Op::Simple(BinOp::Div, BP_MULTIPLICATIVE),
                Some(Token::Percent) => Op::Simple(BinOp::Rem, BP_MULTIPLICATIVE),
                Some(Token::CustomOp(sym)) => Op::Custom(sym.clone()),
                _ => break,
            };
            let bp = match &op {
                Op::Coalesce => BP_COALESCE,
                Op::Simple(_, bp) => *bp,
                Op::Is => BP_RELATIONAL,
                Op::Range(_) => BP_RANGE,
                Op::Custom(_) => BP_CUSTOM,
            };
            if bp < min_bp {
                break;
            }
            self.advance();
            match op {
                Op::Simple(binop, bp) => {
                    let right = self.parse_binary(bp + 1)?;
                    left = Expr::Binary(binop, Box::new(left), Box::new(right));
                }
                Op::Coalesce => {
                    let right = self.parse_binary(BP_COALESCE + 1)?;
                    left = Expr::NullCoalesce(Box::new(left), Box::new(right));
                }
                Op::Is => {
                    // `expr is Type` / `expr is Type name` (type test +
                    // binding); no right-hand expression.
                    let ty = self.parse_type()?;
                    let binding = if let Some(Token::Ident(_)) = self.peek() {
                        Some(self.consume_ident("expected binding name")?)
                    } else {
                        None
                    };
                    left = Expr::Is(Box::new(left), ty, binding);
                }
                Op::Range(inclusive) => {
                    if matches!(left, Expr::Range(..)) {
                        return Err(format!(
                            "line {}: range expressions cannot be chained",
                            self.cur_line()
                        ));
                    }
                    let right = self.parse_binary(BP_RANGE + 1)?;
                    left = Expr::Range(Box::new(left), Box::new(right), inclusive);
                }
                Op::Custom(sym) => {
                    let right = self.parse_binary(BP_CUSTOM + 1)?;
                    left = Expr::CustomOp(sym, Box::new(left), Box::new(right));
                }
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        // `await expr` (contextual keyword): only when another
        // expression-starting token follows, so `await` stays usable as an
        // ordinary identifier elsewhere.
        if matches!(self.peek(), Some(Token::Ident(k)) if k == "await")
            && matches!(
                self.tokens.get(self.pos + 1),
                Some(
                    Token::Ident(_)
                        | Token::IntLit(..)
                        | Token::FloatLit(..)
                        | Token::StringLit(_)
                        | Token::LParen
                        | Token::New
                        | Token::True
                        | Token::False
                )
            )
        {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(Expr::Await(Box::new(operand)));
        }
        if self.match_token(Token::Minus) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary(UnaryOp::Neg, Box::new(operand)))
        } else if self.match_token(Token::Bang) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary(UnaryOp::Not, Box::new(operand)))
        } else if self.looks_like_cast() {
            // C-style cast: (int8)expr
            self.consume(Token::LParen, "expected `(`")?;
            let ty = self.parse_type()?;
            self.consume(Token::RParen, "expected `)`")?;
            let operand = self.parse_unary()?;
            Ok(Expr::Cast(Box::new(operand), ty))
        } else {
            let mut expr = self.parse_call_or_field()?;
            // Rust-style cast: expr as int8
            while self.match_token(Token::As) {
                let ty = self.parse_type()?;
                expr = Expr::Cast(Box::new(expr), ty);
            }
            Ok(expr)
        }
    }

    /// True if the upcoming `(` starts a C-style cast, i.e. `(` type `)`
    /// followed by an expression-starting token. Without this lookahead a
    /// parenthesized expression would be ambiguous with a cast.
    fn looks_like_cast(&self) -> bool {
        if !self.check(Token::LParen) {
            return false;
        }
        let mut pos = self.pos + 1;
        while pos < self.tokens.len() && self.tokens[pos] == Token::Newline {
            pos += 1;
        }
        // A type is a keyword type or an identifier (class / numeric type name).
        let type_start = match self.tokens.get(pos) {
            Some(Token::Void | Token::Int | Token::Float | Token::Bool | Token::String | Token::Ident(_) | Token::Var) => true,
            _ => false,
        };
        if !type_start {
            return false;
        }
        pos += 1;
        // Skip generic type arguments if present.
        if pos < self.tokens.len() && self.tokens[pos] == Token::Lt {
            let mut depth = 1;
            pos += 1;
            while pos < self.tokens.len() && depth > 0 {
                match self.tokens.get(pos) {
                    Some(Token::Lt) => depth += 1,
                    Some(Token::Gt) => depth -= 1,
                    _ => {}
                }
                pos += 1;
            }
        }
        if self.tokens.get(pos) != Some(&Token::RParen) {
            return false;
        }
        pos += 1;
        // The token after `)` must start an expression, not a binary operator,
        // otherwise the parens are a grouping.
        matches!(
            self.tokens.get(pos),
            Some(
                Token::Ident(_)
                    | Token::IntLit(_, _)
                    | Token::FloatLit(_, _)
                    | Token::StringLit(_)
                    | Token::True
                    | Token::False
                    | Token::Null
                    | Token::LParen
                    | Token::Bang
                    | Token::Minus
                    | Token::New
                    | Token::Match
                    | Token::Super
                    | Token::LBrace
            )
        )
    }

    fn parse_call_or_field(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.match_token(Token::Dot) {
                // Check for tuple index access: .0, .1, etc.
                // Also handle the case where the lexer produced a float literal like 0.0
                let tuple_index = match self.peek() {
                    Some(Token::IntLit(idx, _)) => {
                        let idx = *idx as usize;
                        self.advance();
                        Some(idx)
                    }
                    Some(Token::FloatLit(f, _)) => {
                        // If we see a float like 0.0 after a dot, treat the integer part as the index
                        let idx = *f as usize;
                        self.advance();
                        Some(idx)
                    }
                    _ => None,
                };
                if let Some(idx) = tuple_index {
                    expr = Expr::TupleIndex(Box::new(expr), idx);
                    continue;
                }
                let name = self.consume_ident("expected member name after `.`")?;
                if let Expr::Var(ref target) = expr {
                    if target == "super" {
                        if self.check(Token::LParen) {
                            let args = self.parse_args()?;
                            expr = Expr::SuperCall(name, args);
                        } else {
                            expr = Expr::SuperField(name);
                        }
                        continue;
                    }
                }
                if let Expr::Var(ref class_name) = expr {
                    let is_type_name = class_name.chars().next().map_or(false, |c| c.is_ascii_uppercase());
                    if is_type_name {
                        // Explicit type arguments on a static generic call:
                        // `Util.Pick<int>(...)`. The lookahead keeps
                        // comparisons (`Config.Min < x`) as expressions.
                        let type_args = if self.check(Token::Lt) && self.lookahead_is_type_args() {
                            self.parse_type_args()?
                        } else {
                            Vec::new()
                        };
                        if self.check(Token::LParen) {
                            let args = self.parse_args()?;
                            expr = Expr::Call(CallExpr {
                                target: None,
                                class_or_target: class_name.clone(),
                                method: name,
                                type_args,
                                args,
                            });
                        } else {
                            expr = Expr::StaticField(class_name.clone(), name);
                        }
                        continue;
                    }
                }
                let type_args = if self.check(Token::Lt) {
                    if self.lookahead_is_type_args() {
                        self.parse_type_args()?
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                if self.check(Token::LParen) {
                    let args = self.parse_args()?;
                    expr = Expr::Call(CallExpr {
                        target: Some(Box::new(expr.clone())),
                        class_or_target: name.clone(),
                        method: name,
                        type_args,
                        args,
                    });
                } else {
                    expr = Expr::Field(Box::new(expr), name);
                }
            } else if self.match_token(Token::NullConditional) {
                // Null conditional access: ?.field or ?.method()
                let name = self.consume_ident("expected member name after `?.`")?;
                if self.check(Token::LParen) {
                    let args = self.parse_args()?;
                    expr = Expr::NullConditionalCall(CallExpr {
                        target: Some(Box::new(expr.clone())),
                        class_or_target: name.clone(),
                        method: name,
                        type_args: Vec::new(),
                        args,
                    });
                } else {
                    expr = Expr::NullConditionalField(Box::new(expr), name);
                }
            } else if self.check(Token::Bang) {
                // Non-null assertion: `value!`. `!=` lexes as a single token,
                // so a lone `!` after a postfix expression is unambiguous.
                self.advance();
                expr = Expr::NonNullAssert(Box::new(expr));
                continue;
            } else if self.check(Token::Question) {
                // `?` is ambiguous: it starts a ternary (`cond ? a : b`) or is
                // a postfix error-propagation operator (`expr?`). A ternary is
                // always followed by a then-expression and then `:`; consume
                // the `?`, try to parse that, and restore on success so the
                // enclosing expression can parse the ternary.
                let saved = self.pos;
                self.advance(); // tentatively consume `?`
                let is_ternary = self.try_parse_ternary_then();
                if is_ternary {
                    self.pos = saved;
                    break;
                }
                expr = Expr::TryUnwrap(Box::new(expr));
                continue;
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Attempt to parse the then-part of a ternary following an already
    /// consumed `?`. Returns true if a complete expression followed by `:` is
    /// found; the caller restores the token position on success.
    fn try_parse_ternary_then(&mut self) -> bool {
        self.parse_expr()
            .map(|_| self.check(Token::Colon))
            .unwrap_or(false)
    }

    fn lookahead_is_type_args(&mut self) -> bool {
        // `<` starts type arguments only when a balanced list of type-shaped
        // tokens closes with `>` immediately followed by `(` — a generic
        // method call is the only expression form type args can precede.
        // Anything else (`a.n < other.n`) is a comparison.
        if !self.check(Token::Lt) {
            return false;
        }
        let mut i = self.pos + 1;
        let mut depth = 1usize;
        while i < self.tokens.len() {
            match &self.tokens[i] {
                Token::Newline => {}
                Token::Lt => depth += 1,
                Token::Gt => {
                    depth -= 1;
                    if depth == 0 {
                        let mut j = i + 1;
                        while matches!(self.tokens.get(j), Some(Token::Newline)) {
                            j += 1;
                        }
                        return matches!(self.tokens.get(j), Some(Token::LParen));
                    }
                }
                Token::Ident(_)
                | Token::Comma
                | Token::Question
                | Token::Int
                | Token::Float
                | Token::Bool
                | Token::String
                | Token::Void => {}
                _ => return false,
            }
            i += 1;
        }
        false
    }

    fn is_class_name(&self, _name: &str) -> bool {
        false
    }

    fn finish_call(&mut self, target: Expr, method: String) -> Result<Expr, String> {
        let args = self.parse_args()?;
        Ok(Expr::Call(CallExpr {
            target: Some(Box::new(target)),
            class_or_target: method.clone(),
            method,
            type_args: Vec::new(),
            args,
        }))
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        if self.match_token(Token::Match) {
            return self.parse_match();
        }
        if self.match_token(Token::True) {
            return Ok(Expr::Bool(true));
        }
        if self.match_token(Token::False) {
            return Ok(Expr::Bool(false));
        }
        if self.match_token(Token::Null) {
            return Ok(Expr::Null);
        }
        if self.match_token(Token::Super) {
            return Ok(Expr::Var("super".to_string()));
        }
        if self.match_token(Token::New) {
            let mut name = self.consume_ident("expected class name after `new`")?;
            // Qualified nested-class name: `new Outer.Inner(...)`.
            while matches!(self.peek(), Some(Token::Dot))
                && matches!(self.tokens.get(self.pos + 1), Some(Token::Ident(_)))
            {
                self.advance();
                let part = self.consume_ident("expected name after `.`")?;
                name.push('.');
                name.push_str(&part);
            }
            let type_args = if self.check(Token::Lt) {
                self.parse_type_args()?
            } else {
                Vec::new()
            };
            let args = self.parse_args()?;
            return Ok(Expr::New(name, type_args, args));
        }
        if self.check(Token::LBrace) {
            self.advance();
            let stmts = self.parse_block()?;
            return Ok(Expr::Block(stmts));
        }

        match self.peek() {
            Some(Token::IntLit(v, suffix)) => {
                let v = *v;
                let suffix = *suffix;
                self.advance();
                Ok(Expr::IntLit(v, suffix))
            }
            Some(Token::FloatLit(x, suffix)) => {
                let v = *x;
                let suffix = *suffix;
                self.advance();
                Ok(Expr::FloatLit(v, suffix))
            }
            Some(Token::CharLit(c)) => {
                let v = *c;
                self.advance();
                Ok(Expr::Char(v))
            }
            Some(Token::StringLit(s)) => {
                let v = s.clone();
                self.advance();
                
                // Check if this is an interpolated string
                if self.check(Token::InterpStart) {
                    let mut parts = vec![InterpPart::Literal(v)];
                    
                    while self.match_token(Token::InterpStart) {
                        // Parse the expression inside the interpolation
                        let expr = self.parse_expr()?;
                        parts.push(InterpPart::Expr(expr));
                        
                        // Expect InterpEnd
                        if !self.match_token(Token::InterpEnd) {
                            return Err("expected `}` to close string interpolation".to_string());
                        }
                        
                        // Check if there's another string part
                        if let Some(Token::StringLit(s)) = self.peek() {
                            parts.push(InterpPart::Literal(s.clone()));
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    
                    Ok(Expr::InterpolatedString(parts))
                } else {
                    Ok(Expr::String(v))
                }
            }
            Some(Token::Ident(name)) => {
                let mut name = name.clone();
                self.advance();
                // Inside an extension body the receiver is the synthesized
                // `__self` parameter; `this` refers to it.
                if name == "this" && self.substitute_this {
                    name = "__self".to_string();
                }
                // Single-parameter lambda: `x => x + 1`.
                if self.check(Token::FatArrow) {
                    self.advance();
                    let body = self.parse_lambda_body()?;
                    return Ok(Expr::Lambda { params: vec![(name, None)], body });
                }
                // `_` in expression position is a typed hole: the checker
                // reports the expected type and in-scope candidates.
                if name == "_" && !self.check(Token::LParen) {
                    return Ok(Expr::Hole);
                }
                if self.check(Token::LParen) {
                    let args = self.parse_args()?;
                    if name == "print" || name == "println" {
                        Ok(Expr::Call(CallExpr {
                            target: None,
                            class_or_target: "__intrinsics".to_string(),
                            method: name,
                            type_args: Vec::new(),
                            args,
                        }))
                    } else {
                        Ok(Expr::Call(CallExpr {
                            target: None,
                            class_or_target: name.clone(),
                            method: name,
                            type_args: Vec::new(),
                            args,
                        }))
                    }
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Some(Token::LParen) => {
                // Parenthesized lambda: `() => ...`, `(a, b) => ...`,
                // `(int x) => ...` — decided by scanning for `) =>`.
                if self.lookahead_is_lambda() {
                    return self.parse_lambda_parenthesized();
                }
                self.advance();
                let expr = self.parse_expr()?;
                if self.match_token(Token::Comma) {
                    // This is a tuple literal
                    let mut elements = vec![expr];
                    if !self.check(Token::RParen) {
                        loop {
                            elements.push(self.parse_expr()?);
                            if !self.match_token(Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.consume(Token::RParen, "expected `)`")?;
                    Ok(Expr::Tuple(elements))
                } else {
                    // This is a parenthesized expression
                    self.consume(Token::RParen, "expected `)`")?;
                    Ok(expr)
                }
            }
            Some(t) => Err(format!("line {}: unexpected token {}", self.cur_line(), t)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    /// True when the upcoming `(`...`)` is a lambda parameter list — i.e.
    /// the matching close paren is immediately followed by `=>`.
    fn lookahead_is_lambda(&self) -> bool {
        let mut pos = self.pos;
        if !matches!(self.tokens.get(pos), Some(Token::LParen)) {
            return false;
        }
        pos += 1;
        let mut depth = 1usize;
        while pos < self.tokens.len() {
            match &self.tokens[pos] {
                Token::LParen => depth += 1,
                Token::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(pos + 1), Some(Token::FatArrow));
                    }
                }
                _ => {}
            }
            pos += 1;
        }
        false
    }

    /// Parse `(params) => body` with the cursor on `(`. A parameter is
    /// `name` or `Type name`.
    fn parse_lambda_parenthesized(&mut self) -> Result<Expr, String> {
        self.consume(Token::LParen, "expected `(`")?;
        let mut params: Vec<(String, Option<Type>)> = Vec::new();
        while !self.check(Token::RParen) {
            // `Type name` when two identifier-ish tokens are adjacent
            // (`int x`, `List<int> xs`, `Outer.Inner v`); bare `name`
            // otherwise.
            let typed = match (self.peek(), self.tokens.get(self.pos + 1)) {
                (Some(Token::Ident(_)), Some(Token::Ident(_))) => true,
                (Some(Token::Ident(_)), Some(Token::Lt | Token::Dot)) => true,
                (Some(t), _) if self.check_type_token(t) && !matches!(t, Token::Ident(_)) => true,
                _ => false,
            };
            if typed {
                let ty = self.parse_type()?;
                let name = self.consume_ident("expected lambda parameter name")?;
                params.push((name, Some(ty)));
            } else {
                let name = self.consume_ident("expected lambda parameter name")?;
                params.push((name, None));
            }
            if !self.match_token(Token::Comma) {
                break;
            }
        }
        self.consume(Token::RParen, "expected `)`")?;
        self.consume(Token::FatArrow, "expected `=>`")?;
        let body = self.parse_lambda_body()?;
        Ok(Expr::Lambda { params, body })
    }

    /// Lambda body after `=>`: a block, or an expression desugared to
    /// `return expr;`.
    fn parse_lambda_body(&mut self) -> Result<Vec<Stmt>, String> {
        if self.check(Token::LBrace) {
            self.advance();
            self.parse_block()
        } else {
            let e = self.parse_expr()?;
            Ok(vec![Stmt::Return(Some(e))])
        }
    }

    fn parse_match(&mut self) -> Result<Expr, String> {
        self.consume(Token::LParen, "expected `(` after `match`")?;
        let subject = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        self.consume(Token::LBrace, "expected `{`")?;
        
        let mut arms = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            arms.push(self.parse_match_arm()?);
            self.skip_newlines();
            self.match_token(Token::Comma);
            self.skip_newlines();
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(Expr::Match(Box::new(subject), arms))
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, String> {
        let mut patterns = vec![self.parse_pattern()?];
        while self.match_token(Token::Or) {
            patterns.push(self.parse_pattern()?);
        }
        
        let guard = if self.match_token(Token::If) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        
        self.consume(Token::FatArrow, "expected `=>`")?;
        let body = self.parse_expr()?;
        self.skip_newlines();
        
        Ok(MatchArm { patterns, guard, body })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        if self.match_token(Token::Star) {
            return Ok(Pattern::Wildcard);
        }
        if self.match_token(Token::Null) {
            return Ok(Pattern::Null);
        }
        if self.match_token(Token::True) {
            return Ok(Pattern::Bool(true));
        }
        if self.match_token(Token::False) {
            return Ok(Pattern::Bool(false));
        }
        if let Some(Token::IntLit(i, _)) = self.peek() {
            let v = *i;
            self.advance();
            // Check for range pattern: 1..5 or 1..=5
            if self.match_token(Token::DotDot) {
                let end = self.parse_pattern_expr()?;
                return Ok(Pattern::Range(Box::new(Expr::IntLit(v, crate::ast::IntSuffix::None)), Box::new(end), false));
            }
            if self.match_token(Token::DotDotEq) {
                let end = self.parse_pattern_expr()?;
                return Ok(Pattern::Range(Box::new(Expr::IntLit(v, crate::ast::IntSuffix::None)), Box::new(end), true));
            }
            return Ok(Pattern::Int(v));
        }
        if let Some(Token::FloatLit(x, _)) = self.peek() {
            let v = *x;
            self.advance();
            // Check for range pattern
            if self.match_token(Token::DotDot) {
                let end = self.parse_pattern_expr()?;
                return Ok(Pattern::Range(Box::new(Expr::FloatLit(v, crate::ast::FloatSuffix::None)), Box::new(end), false));
            }
            if self.match_token(Token::DotDotEq) {
                let end = self.parse_pattern_expr()?;
                return Ok(Pattern::Range(Box::new(Expr::FloatLit(v, crate::ast::FloatSuffix::None)), Box::new(end), true));
            }
            return Ok(Pattern::Float(v));
        }
        if let Some(Token::StringLit(s)) = self.peek() {
            let v = s.clone();
            self.advance();
            return Ok(Pattern::StringLit(v));
        }
        if let Some(Token::Ident(name)) = self.peek() {
            let name = name.clone();
            self.advance();
            if self.match_token(Token::Dot) {
                let variant = self.consume_ident("expected variant name")?;
                let args = if self.check(Token::LParen) {
                    self.consume(Token::LParen, "expected `(`")?;
                    let mut args = Vec::new();
                    if !self.check(Token::RParen) {
                        loop {
                            let arg = self.parse_pattern()?;
                            args.push(arg);
                            if !self.match_token(Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.consume(Token::RParen, "expected `)`")?;
                    args
                } else {
                    Vec::new()
                };
                return Ok(Pattern::EnumVariant(name, variant, args));
            }
            if self.check(Token::LParen) {
                // Record pattern: `Point(x, y)` with positional sub-patterns.
                self.consume(Token::LParen, "expected `(`")?;
                let mut args = Vec::new();
                if !self.check(Token::RParen) {
                    loop {
                        args.push(self.parse_pattern()?);
                        if !self.match_token(Token::Comma) {
                            break;
                        }
                    }
                }
                self.consume(Token::RParen, "expected `)`")?;
                return Ok(Pattern::RecordClass(name, args));
            }
            return Ok(Pattern::Binding(name));
        }
        Err(format!("expected pattern, found {}", self.peek_desc()))
    }

    /// Parse an expression in a pattern context (limited to literals and simple expressions)
    fn parse_pattern_expr(&mut self) -> Result<Expr, String> {
        if let Some(Token::IntLit(i, suffix)) = self.peek() {
            let v = *i;
            let suffix = *suffix;
            self.advance();
            return Ok(Expr::IntLit(v, suffix));
        }
        if let Some(Token::FloatLit(x, suffix)) = self.peek() {
            let v = *x;
            let suffix = *suffix;
            self.advance();
            return Ok(Expr::FloatLit(v, suffix));
        }
        if let Some(Token::Ident(name)) = self.peek() {
            let name = name.clone();
            self.advance();
            return Ok(Expr::Var(name));
        }
        Err(format!("expected literal or identifier in range pattern, found {}", self.peek_desc()))
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, String> {
        self.consume(Token::LParen, "expected `(`")?;
        let mut args = Vec::new();
        if !self.check(Token::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
        }
        self.consume(Token::RParen, "expected `)`")?;
        Ok(args)
    }

    fn parse_type(&mut self) -> Result<Type, String> {
        let base = self.parse_base_type()?;
        if self.match_token(Token::Question) {
            if self.check(Token::Question) {
                return Err("doubly-nullable type `T??` is not allowed".to_string());
            }
            return Ok(Type::Nullable(Box::new(base)));
        }
        Ok(base)
    }

    fn parse_base_type(&mut self) -> Result<Type, String> {
        match self.peek() {
            Some(Token::Void) => {
                self.advance();
                Ok(Type::Unit)
            }
            Some(Token::Var) => {
                self.advance();
                Ok(Type::Infer)
            }
            Some(Token::Int) => {
                self.advance();
                Ok(Type::Int32)
            }
            Some(Token::Float) => {
                self.advance();
                Ok(Type::Float64)
            }
            Some(Token::Bool) => {
                self.advance();
                Ok(Type::Bool)
            }
            Some(Token::String) => {
                self.advance();
                Ok(Type::String)
            }
            Some(Token::LParen) => {
                self.advance();
                let mut types = Vec::new();
                if !self.check(Token::RParen) {
                    loop {
                        types.push(self.parse_type()?);
                        if !self.match_token(Token::Comma) {
                            break;
                        }
                    }
                }
                self.consume(Token::RParen, "expected `)`")?;
                if types.len() == 1 {
                    Ok(types.into_iter().next().unwrap())
                } else {
                    Ok(Type::Tuple(types))
                }
            }
            Some(Token::Ident(n)) => {
                let mut n = n.clone();
                self.advance();
                if n == "char" {
                    return Ok(Type::Char);
                }
                if let Some(ty) = numeric_type_from_name(&n) {
                    return Ok(ty);
                }
                // Function types: `Func<int, string>` (last argument is the
                // return type), `Action<int>` / `Action` (void-returning).
                if n == "Func" && self.check(Token::Lt) {
                    let mut args = self.parse_type_args()?;
                    if args.is_empty() {
                        return Err("`Func` needs at least a return type".to_string());
                    }
                    let ret = args.pop().unwrap();
                    return Ok(Type::Func(args, Box::new(ret)));
                }
                if n == "Action" {
                    if self.check(Token::Lt) {
                        let args = self.parse_type_args()?;
                        return Ok(Type::Func(args, Box::new(Type::Unit)));
                    }
                    return Ok(Type::Func(Vec::new(), Box::new(Type::Unit)));
                }
                // Qualified nested-class name: `Outer.Inner`.
                while matches!(self.peek(), Some(Token::Dot))
                    && matches!(self.tokens.get(self.pos + 1), Some(Token::Ident(_)))
                {
                    self.advance();
                    let part = self.consume_ident("expected name after `.`")?;
                    n.push('.');
                    n.push_str(&part);
                }
                // Check for type arguments
                if self.check(Token::Lt) {
                    let args = self.parse_type_args()?;
                    Ok(Type::Class(n, args))
                } else {
                    // Check if it's a generic parameter reference (single uppercase letter)
                    // or a class name. For now, treat all as Class.
                    Ok(Type::Class(n, Vec::new()))
                }
            }
            Some(t) => Err(format!("expected type, found {}", t)),
            None => Err("expected type".to_string()),
        }
    }

    fn parse_type_args(&mut self) -> Result<Vec<Type>, String> {
        self.consume(Token::Lt, "expected `<`")?;
        let mut args = Vec::new();
        loop {
            args.push(self.parse_type()?);
            if !self.match_token(Token::Comma) {
                break;
            }
        }
        self.consume(Token::Gt, "expected `>`")?;
        Ok(args)
    }

    fn check_type(&self) -> bool {
        matches!(
            self.peek(),
            Some(Token::Void | Token::Int | Token::Float | Token::Bool | Token::String | Token::Ident(_) | Token::LParen | Token::Var)
        )
    }

    fn check_type_token(&self, tok: &Token) -> bool {
        matches!(
            tok,
            Token::Void | Token::Int | Token::Float | Token::Bool | Token::String | Token::Ident(_) | Token::LParen | Token::Var
        )
    }

    fn peek_ahead_is_ident_after_type(&self) -> bool {
        let mut pos = self.pos;
        if pos >= self.tokens.len() {
            return false;
        }
        if !self.check_type_token(&self.tokens[pos]) {
            return false;
        }
        // `await expr;` starts with an identifier but is a statement
        // expression, never a declaration type.
        if matches!(self.tokens.get(pos), Some(Token::Ident(k)) if k == "await") {
            return false;
        }
        let first_is_ident = matches!(self.tokens.get(pos), Some(Token::Ident(_)));
        pos += 1;
        // Skip a qualified nested-class name (`Outer.Inner v = ...`) — only
        // after an identifier head, and only when another identifier follows
        // the dot (so `a.b = 5` stays an assignment... both shapes skip the
        // dots; the decisive check is the trailing identifier below).
        if first_is_ident {
            while matches!(self.tokens.get(pos), Some(Token::Dot))
                && matches!(self.tokens.get(pos + 1), Some(Token::Ident(_)))
            {
                pos += 2;
            }
        }
        // Skip generic type arguments if present
        if pos < self.tokens.len() && matches!(self.tokens.get(pos), Some(Token::Lt)) {
            let mut depth = 1;
            pos += 1;
            while pos < self.tokens.len() && depth > 0 {
                match self.tokens.get(pos) {
                    Some(Token::Lt) => depth += 1,
                    Some(Token::Gt) => depth -= 1,
                    None => return false,
                    _ => {}
                }
                pos += 1;
            }
        }
        let mut nullable = false;
        if matches!(self.tokens.get(pos), Some(Token::Question)) {
            nullable = true;
            pos += 1;
        }
        if !matches!(self.tokens.get(pos), Some(Token::Ident(_))) {
            return false;
        }
        if nullable {
            // `x ? y : z` (a ternary statement) also matches
            // "type-question-ident", so a nullable var decl additionally
            // requires a declaration continuation after the name.
            pos += 1;
            return matches!(self.tokens.get(pos), Some(Token::Assign | Token::Semi));
        }
        true
    }

    fn consume(&mut self, expected: Token, msg: &str) -> Result<(), String> {
        if self.check_token(&expected) {
            self.advance();
            Ok(())
        } else {
            Err(format!("line {}: {} (found {})", self.cur_line(), msg, self.peek_desc()))
        }
    }

    fn consume_ident(&mut self, msg: &str) -> Result<String, String> {
        match self.peek() {
            Some(Token::Ident(s)) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(format!("line {}: {} (found {})", self.cur_line(), msg, self.peek_desc())),
        }
    }

    fn consume_semi(&mut self) -> Result<(), String> {
        self.skip_newlines();
        if self.check(Token::Semi) {
            self.advance();
        }
        Ok(())
    }

    fn match_token(&mut self, expected: Token) -> bool {
        if self.check_token(&expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, expected: Token) -> bool {
        self.check_token(&expected)
    }

    fn check_token(&self, expected: &Token) -> bool {
        match self.peek() {
            Some(t) => discriminant_eq(t, expected),
            None => false,
        }
    }

    fn advance(&mut self) -> Option<Token> {
        if !self.is_at_end() {
            self.pos += 1;
            Some(self.tokens[self.pos - 1].clone())
        } else {
            None
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_desc(&self) -> String {
        match self.peek() {
            Some(t) => t.to_string(),
            None => "EOF".to_string(),
        }
    }

    fn skip_newlines(&mut self) {
        while self.check(Token::Newline) {
            self.advance();
        }
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Some(Token::Eof) | None)
    }
}

fn discriminant_eq(a: &Token, b: &Token) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, String> {
    match expr {
        Expr::Var(name) => Ok(AssignTarget::Local(name)),
        Expr::Field(obj, name) => Ok(AssignTarget::Field(obj, name)),
        Expr::StaticField(class, name) => Ok(AssignTarget::StaticField(class, name)),
        Expr::SuperField(name) => Ok(AssignTarget::SuperField(name)),
        _ => Err("invalid assignment target".to_string()),
    }
}
