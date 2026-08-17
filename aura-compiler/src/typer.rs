//! Type checker for Aura.

use crate::ast::*;
use std::collections::{HashMap, HashSet};

/// Type substitution map: generic parameter name -> concrete type
pub type TypeSubst = HashMap<String, Type>;

/// Structurally match a declared parameter type against an argument type,
/// binding the type variables named in `vars` (method-level generic
/// parameters appear both as `Type::GenericParam` and as bare
/// `Type::Class(name, [])` references). When a variable is already bound,
/// `join` decides the merged binding (returning `None` keeps the previous
/// one; the caller records the conflict). Positions without variables are
/// left to ordinary assignability checking afterwards.
pub fn unify_generic_types(
    param: &Type,
    arg: &Type,
    vars: &HashSet<String>,
    bindings: &mut HashMap<String, Type>,
    join: &mut dyn FnMut(&str, Type, Type) -> Option<Type>,
) {
    let bind = |name: &str, arg: &Type, bindings: &mut HashMap<String, Type>, join: &mut dyn FnMut(&str, Type, Type) -> Option<Type>| {
        match bindings.get(name).cloned() {
            None => {
                bindings.insert(name.to_string(), arg.clone());
            }
            Some(prev) => {
                if prev != *arg {
                    if let Some(merged) = join(name, prev, arg.clone()) {
                        bindings.insert(name.to_string(), merged);
                    }
                }
            }
        }
    };
    match (param, arg) {
        (Type::GenericParam(n), _) if vars.contains(n) => bind(n, arg, bindings, join),
        (Type::Class(n, a), _) if a.is_empty() && vars.contains(n) => bind(n, arg, bindings, join),
        (Type::Class(pn, pa), Type::Class(an, aa)) if pn == an && pa.len() == aa.len() => {
            for (p, a) in pa.iter().zip(aa.iter()) {
                unify_generic_types(p, a, vars, bindings, join);
            }
        }
        (Type::Nullable(p), Type::Nullable(a)) => unify_generic_types(p, a, vars, bindings, join),
        (Type::Func(pp, pr), Type::Func(ap, ar)) if pp.len() == ap.len() => {
            for (p, a) in pp.iter().zip(ap.iter()) {
                unify_generic_types(p, a, vars, bindings, join);
            }
            unify_generic_types(pr, ar, vars, bindings, join);
        }
        // `T?` accepts a non-null argument: bind against the inner type.
        (Type::Nullable(p), a) => unify_generic_types(p, a, vars, bindings, join),
        (Type::Tuple(ps), Type::Tuple(as_)) if ps.len() == as_.len() => {
            for (p, a) in ps.iter().zip(as_.iter()) {
                unify_generic_types(p, a, vars, bindings, join);
            }
        }
        _ => {}
    }
}

/// A single narrowing fact derived from a condition (see
/// `TypeChecker::condition_facts`).
#[derive(Debug, Clone)]
enum GuardFact {
    /// Local `name` (declared as the nullable `declared`) is known non-null
    /// and narrows to `inner`.
    NonNull { name: String, inner: Type, declared: Type },
    /// An `is`-test binding `name` holding the subject typed as `ty`.
    Binding { name: String, ty: Type },
}

/// True if class `class_name` structurally satisfies interface `iface_name`
/// (duck typing): every abstract method of the interface — and of the
/// interfaces it extends — exists on the class or its superclasses as a
/// public, non-generic instance method with exactly the same parameter and
/// return types. Rules, deliberately conservative for v1:
///
/// * Exact type matching only — no parameter/return variance.
/// * Non-generic interfaces only; generic interfaces still require a
///   declared `: IFace<...>`.
/// * Sources must be classes (possibly abstract); interface-to-interface
///   satisfaction stays declaration-based.
/// * Interface default methods (with bodies) are optional: the emitter
///   records structural implementations into `ClassDef.interfaces`, so the
///   runtime inherits default bodies through the same lookup that declared
///   implementations use.
/// * Intrinsic stdlib classes (`List` etc.) are excluded — their methods
///   have no bytecode bodies, so interface-typed dispatch could not reach
///   them at runtime.
///
/// Shared by the type checker (assignability) and the emitter (recording
/// implementations for runtime type tests) so the two can never disagree.
pub fn structurally_satisfies(
    classes: &HashMap<String, ClassInfo>,
    class_name: &str,
    iface_name: &str,
) -> bool {
    let Some(iface) = classes.get(iface_name) else {
        return false;
    };
    if !iface.is_interface || !iface.generic_params.is_empty() {
        return false;
    }
    let Some(class) = classes.get(class_name) else {
        return false;
    };
    if class.is_interface || crate::intrinsics::is_intrinsic_class(class_name) {
        return false;
    }

    // The full requirement set: the interface plus everything it extends.
    let mut required: Vec<&ClassInfo> = Vec::new();
    let mut queue = vec![iface_name.to_string()];
    let mut seen = HashSet::new();
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(info) = classes.get(&name) {
            if !info.generic_params.is_empty() {
                return false;
            }
            required.push(info);
            queue.extend(info.interfaces.iter().cloned());
        }
    }

    // Look a method up along the class's superclass chain (nearest wins,
    // mirroring find_method's shadowing behavior).
    let find_on_class = |method: &str| -> Option<&MethodInfo> {
        let mut cur = Some(class_name);
        while let Some(c) = cur {
            let info = classes.get(c)?;
            if let Some(mi) = info.methods.get(method) {
                return Some(mi);
            }
            cur = info.super_class.as_deref();
        }
        None
    };

    for info in required {
        for (name, im) in &info.methods {
            if !im.is_abstract {
                continue; // default methods are optional
            }
            if !im.generic_params.is_empty() {
                return false;
            }
            let Some(found) = find_on_class(name) else {
                return false;
            };
            // An abstract match is fine: any runtime instance is a concrete
            // subclass that was forced to override it.
            if !found.is_instance
                || found.visibility != Visibility::Public
                || !found.generic_params.is_empty()
                || found.params != im.params
                || found.return_ty != im.return_ty
            {
                return false;
            }
        }
    }
    true
}

/// Explain why `class_name` does not structurally satisfy `iface_name`:
/// the first missing or mismatched abstract method, with both signatures.
fn explain_structural_failure(
    classes: &HashMap<String, ClassInfo>,
    class_name: &str,
    iface_name: &str,
) -> Option<String> {
    let iface = classes.get(iface_name)?;
    if !iface.is_interface {
        return None;
    }
    let sig = |params: &[Type], ret: &Type| {
        format!(
            "({}) -> {}",
            params.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "),
            ret.name()
        )
    };
    let find_on_class = |method: &str| -> Option<&MethodInfo> {
        let mut cur = Some(class_name);
        while let Some(c) = cur {
            let info = classes.get(c)?;
            if let Some(mi) = info.methods.get(method) {
                return Some(mi);
            }
            cur = info.super_class.as_deref();
        }
        None
    };
    for (name, im) in &iface.methods {
        if !im.is_abstract {
            continue;
        }
        let required = sig(&im.params, &im.return_ty);
        let Some(found) = find_on_class(name) else {
            return Some(format!(
                "`{}` is missing method `{} {}` required by `{}`",
                class_name, name, required, iface_name
            ));
        };
        if !found.is_instance {
            return Some(format!(
                "`{}.{}` is static but `{}` requires an instance method",
                class_name, name, iface_name
            ));
        }
        if found.visibility != Visibility::Public {
            return Some(format!(
                "`{}.{}` is not public but `{}` requires it to be",
                class_name, name, iface_name
            ));
        }
        if found.params != im.params || found.return_ty != im.return_ty {
            return Some(format!(
                "`{}.{}` has signature {} but `{}` requires {}",
                class_name,
                name,
                sig(&found.params, &found.return_ty),
                iface_name,
                required
            ));
        }
    }
    None
}

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
pub fn substitute_type(ty: &Type, subst: &TypeSubst) -> Type {
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
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute_type(inner, subst))),
        Type::Func(params, ret) => Type::Func(
            params.iter().map(|t| substitute_type(t, subst)).collect(),
            Box::new(substitute_type(ret, subst)),
        ),
        _ => ty.clone(),
    }
}

/// Build a substitution map from generic parameter names and concrete type arguments
pub fn build_subst(generic_params: &[GenericParam], type_args: &[Type]) -> TypeSubst {
    let mut subst = TypeSubst::new();
    for (param, arg) in generic_params.iter().zip(type_args.iter()) {
        subst.insert(param.name.clone(), arg.clone());
    }
    subst
}

/// If `name` is an operator-overload method name (`operator+`, `operator==`,
/// ...), the operator symbol; `None` for ordinary method names, including
/// identifiers that merely start with "operator".
pub fn operator_symbol_of(name: &str) -> Option<&str> {
    let sym = name.strip_prefix("operator")?;
    matches!(sym, "+" | "-" | "*" | "/" | "%" | "==" | "<" | "<=" | ">" | ">=").then_some(sym)
}

/// The operator-overload method name a binary operator resolves through, if
/// the operator is overloadable. `!=` resolves through `operator==` (it is
/// always the negation); `&&`/`||` are never overloadable (short-circuit).
pub fn operator_method_name(op: BinOp) -> Option<&'static str> {
    Some(match op {
        BinOp::Add => "operator+",
        BinOp::Sub => "operator-",
        BinOp::Mul => "operator*",
        BinOp::Div => "operator/",
        BinOp::Rem => "operator%",
        BinOp::Eq | BinOp::Ne => "operator==",
        BinOp::Lt => "operator<",
        BinOp::Le => "operator<=",
        BinOp::Gt => "operator>",
        BinOp::Ge => "operator>=",
        BinOp::And | BinOp::Or => return None,
    })
}

/// Whether `name` occurs anywhere inside `ty` (as a class-position ident or
/// generic-param reference, at any nesting depth). Used by the conservative
/// variance position check.
pub fn type_mentions(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Class(n, args) => n == name || args.iter().any(|a| type_mentions(a, name)),
        Type::GenericParam(n) => n == name,
        Type::Nullable(t) | Type::Newtype(_, t) => type_mentions(t, name),
        Type::Tuple(ts) => ts.iter().any(|t| type_mentions(t, name)),
        Type::Func(params, ret) => {
            params.iter().any(|t| type_mentions(t, name)) || type_mentions(ret, name)
        }
        _ => false,
    }
}

/// Collect names declared anywhere inside these statements (variable
/// declarations, loop variables, tuple destructuring, `using` bindings,
/// catch bindings). Used by lambda capture checking; a flat set is a safe
/// over-approximation.
fn collect_declared_names(stmts: &[Stmt], out: &mut std::collections::HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl(_, name, _) => {
                out.insert(name.clone());
            }
            Stmt::TupleDecl(names, _) => out.extend(names.iter().cloned()),
            Stmt::If(_, a, b) => {
                collect_declared_names(a, out);
                if let Some(b) = b {
                    collect_declared_names(b, out);
                }
            }
            Stmt::IfLet(_, _, a, b) => {
                collect_declared_names(a, out);
                if let Some(b) = b {
                    collect_declared_names(b, out);
                }
            }
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. } | Stmt::Block(body) => {
                collect_declared_names(body, out);
            }
            Stmt::For { init, update, body, .. } => {
                collect_declared_names(std::slice::from_ref(init), out);
                collect_declared_names(std::slice::from_ref(update), out);
                collect_declared_names(body, out);
            }
            Stmt::ForIn { var_name, body, .. } => {
                out.insert(var_name.clone());
                collect_declared_names(body, out);
            }
            Stmt::Try { try_body, catches, finally_body } => {
                collect_declared_names(try_body, out);
                for c in catches {
                    out.insert(c.name.clone());
                    collect_declared_names(&c.body, out);
                }
                if let Some(f) = finally_body {
                    collect_declared_names(f, out);
                }
            }
            Stmt::Using { name, body, .. } => {
                if let Some(n) = name {
                    out.insert(n.clone());
                }
                collect_declared_names(body, out);
            }
            _ => {}
        }
    }
}

/// Collect local names assigned anywhere inside these statements.
fn collect_assigned_locals(stmts: &[Stmt], out: &mut std::collections::HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign(AssignTarget::Local(name), _) => {
                out.insert(name.clone());
            }
            Stmt::If(_, a, b) => {
                collect_assigned_locals(a, out);
                if let Some(b) = b {
                    collect_assigned_locals(b, out);
                }
            }
            Stmt::IfLet(_, _, a, b) => {
                collect_assigned_locals(a, out);
                if let Some(b) = b {
                    collect_assigned_locals(b, out);
                }
            }
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. } | Stmt::Block(body) => {
                collect_assigned_locals(body, out);
            }
            Stmt::For { init, update, body, .. } => {
                collect_assigned_locals(std::slice::from_ref(init), out);
                collect_assigned_locals(std::slice::from_ref(update), out);
                collect_assigned_locals(body, out);
            }
            Stmt::ForIn { body, .. } => collect_assigned_locals(body, out),
            Stmt::Try { try_body, catches, finally_body } => {
                collect_assigned_locals(try_body, out);
                for c in catches {
                    collect_assigned_locals(&c.body, out);
                }
                if let Some(f) = finally_body {
                    collect_assigned_locals(f, out);
                }
            }
            Stmt::Using { body, .. } => collect_assigned_locals(body, out),
            _ => {}
        }
    }
}

/// Source symbol of a binary operator, for error messages.
pub fn binop_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
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
    /// Newtype registry: name -> underlying primitive type. Used by the
    /// emitter to recognize `Name(expr)` constructors, which erase to the
    /// wrapped expression.
    pub newtypes: HashMap<String, Type>,
    /// Extension methods: target key (`string` or a class name) -> method
    /// name -> extension class. Call sites resolve `x.Foo()` to a static
    /// call on the extension class when `x`'s type has no real `Foo`.
    pub extensions: HashMap<String, HashMap<String, String>>,
}

/// Class metadata populated by the type checker.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    /// Class name.
    pub name: String,
    /// Source module (file) the class was declared in; scopes `internal`.
    pub module: String,
    /// Whether this is a static class (no instances, all members static).
    pub is_static: bool,
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
    /// Declared type arguments for each directly implemented generic
    /// interface (`class CatBox : IProducer<Cat>` records `IProducer ->
    /// [Cat]`). Non-generic interfaces map to an empty list.
    pub interface_args: HashMap<String, Vec<Type>>,
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
    /// Names of internal (module-scoped) instance fields.
    pub internal_fields: HashSet<String>,
    /// Names of internal (module-scoped) static fields.
    pub internal_static_fields: HashSet<String>,
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
    /// Declared checked exceptions: callers must catch each (or a
    /// supertype) or re-declare it. Empty = unchecked (the default).
    pub throws: Vec<String>,
    /// `async` method: returns `Task<T>`, body may await, calls spawn.
    pub is_async: bool,
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
    /// Newtype registry: name -> underlying primitive type.
    newtypes: HashMap<String, Type>,
    /// Locals currently narrowed from `T?` to `T` by a null check, mapped to
    /// their declared (nullable) type. Assigning to a narrowed local widens
    /// it back to the declared type (see `Stmt::Assign`).
    narrowed: std::cell::RefCell<HashMap<String, Type>>,
    /// Source line of the statement currently being checked (from the
    /// parser's `Stmt::Mark` markers); 0 when outside statement context.
    current_line: std::cell::Cell<usize>,
    /// Errors collected across method bodies (multiple-error reporting):
    /// each entry is already located ("line N, in `C.m`: ...").
    errors: std::cell::RefCell<Vec<String>>,
    /// `Class.Method` currently being checked, for error prefixes.
    current_context: std::cell::RefCell<String>,
    /// Extension methods: target key -> method name -> extension class.
    extensions: HashMap<String, HashMap<String, String>>,
    /// `throws` list of the method body currently being checked.
    current_throws: std::cell::RefCell<Vec<String>>,
    /// Stack of enclosing try-block catch types (one frame per `try` the
    /// checker is currently inside), for the checked-exception contract.
    handled_exceptions: std::cell::RefCell<Vec<Vec<Type>>>,
    /// Whether the body being checked belongs to an `async` method
    /// (gates `await`).
    current_is_async: std::cell::Cell<bool>,
}

impl TypeChecker {
    /// Create a new type checker.
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
            enums: HashMap::new(),
            newtypes: HashMap::new(),
            narrowed: std::cell::RefCell::new(HashMap::new()),
            current_line: std::cell::Cell::new(0),
            errors: std::cell::RefCell::new(Vec::new()),
            current_context: std::cell::RefCell::new(String::new()),
            extensions: HashMap::new(),
            current_throws: std::cell::RefCell::new(Vec::new()),
            handled_exceptions: std::cell::RefCell::new(Vec::new()),
            current_is_async: std::cell::Cell::new(false),
        }
    }

    /// Record a located error and keep checking (multiple-error
    /// reporting). Capped so a badly broken file stays readable.
    fn record_error(&self, msg: String) {
        let mut errors = self.errors.borrow_mut();
        if errors.len() >= 20 {
            return;
        }
        errors.push(Self::locate(
            self.current_line.get(),
            &self.current_context.borrow(),
            msg,
        ));
    }

    fn locate(line: usize, ctx: &str, msg: String) -> String {
        if line > 0 && !ctx.is_empty() {
            format!("line {}, in `{}`: {}", line, ctx, msg)
        } else if !ctx.is_empty() {
            format!("in `{}`: {}", ctx, msg)
        } else {
            msg
        }
    }

    /// ` — did you mean \`x\`?` when a candidate is a close misspelling
    /// of `target`, else empty. Distance cap scales with length so short
    /// names don't match everything.
    pub(crate) fn suggest<'n>(
        candidates: impl Iterator<Item = &'n str>,
        target: &str,
    ) -> String {
        let cap = 1 + target.len() / 4;
        let mut best: Option<(usize, &str)> = None;
        for c in candidates {
            if c == target {
                continue;
            }
            let d = Self::edit_distance(c, target, cap);
            if d <= cap && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, c));
            }
        }
        match best {
            Some((_, name)) => format!(" — did you mean `{}`?", name),
            None => String::new(),
        }
    }

    /// Levenshtein distance, early-exiting once it must exceed `cap`.
    fn edit_distance(a: &str, b: &str, cap: usize) -> usize {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        if a.len().abs_diff(b.len()) > cap {
            return cap + 1;
        }
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.iter().enumerate() {
            let mut row = vec![i + 1];
            let mut row_min = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let cost = if ca == cb { 0 } else { 1 };
                let v = (prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1);
                row_min = row_min.min(v);
                row.push(v);
            }
            if row_min > cap {
                return cap + 1;
            }
            prev = row;
        }
        prev[b.len()]
    }

    /// Type-check a program and return a typed view. Reports every
    /// collected diagnostic (newline-separated) when any method failed.
    pub fn check(self, program: &Program) -> Result<TypedProgram, TypeError> {
        let line = std::cell::Cell::new(0usize);
        let ctx = std::cell::RefCell::new(String::new());
        // The checker walks statements sequentially, updating
        // current_line/current_context as it goes; when an error surfaces,
        // those cells still hold the location of the failing statement.
        let result = {
            let mut me = self;
            let r = me.check_inner(program);
            line.set(me.current_line.get());
            *ctx.borrow_mut() = me.current_context.borrow().clone();
            (r, me.errors.take())
        };
        let (result, mut collected) = result;
        match result {
            Err(TypeError(msg)) => {
                collected.push(Self::locate(line.get(), &ctx.borrow(), msg));
                Err(TypeError(collected.join("\n")))
            }
            Ok(_) if !collected.is_empty() => Err(TypeError(collected.join("\n"))),
            Ok(v) => Ok(v),
        }
    }

    fn check_inner(&mut self, program: &Program) -> Result<TypedProgram, TypeError> {
        self.inject_intrinsics();
        self.gather_decls(program)?;
        for decl in &program.decls {
            match decl {
                // Record-and-continue: an error in one class does not hide
                // errors in the next (multiple-error reporting).
                Decl::Class(c) => {
                    if let Err(TypeError(msg)) = self.check_class(c) {
                        self.record_error(msg);
                    }
                }
                Decl::Enum(_) => {}
                Decl::TypeAlias(_) => {}
                Decl::Newtype(_) => {}
                // Consumed by alias expansion; never reaches the checker.
                Decl::LiteralUnion(_) => {}
            }
        }
        Ok(TypedProgram {
            program: program.clone(),
            classes: std::mem::take(&mut self.classes),
            enums: std::mem::take(&mut self.enums),
            newtypes: std::mem::take(&mut self.newtypes),
            extensions: std::mem::take(&mut self.extensions),
        })
    }

    /// Register the stdlib intrinsic classes (`List`, `Map`, `Set`, `Console`,
    /// `File`) as ordinary [`ClassInfo`] entries so method lookup, generic
    /// substitution and overload checking reuse the existing machinery. A user
    /// declaration reusing one of these names reports a duplicate-class error.
    fn inject_intrinsics(&mut self) {
        for ic in crate::intrinsics::classes() {
            let to_method_info = |m: &crate::intrinsics::IntrinsicMethod, is_instance: bool| MethodInfo {
                name: m.name.to_string(),
                generic_params: m
                    .generic_params
                    .iter()
                    .map(|n| GenericParam {
                        name: n.to_string(),
                        variance: Variance::Invariant,
                        bounds: Vec::new(),
                        union: Vec::new(),
                        requires_new: false,
                    })
                    .collect(),
                return_ty: m.return_ty.clone(),
                params: m.params.clone(),
                is_instance,
                visibility: Visibility::Public,
                is_virtual: false,
                is_override: false,
                is_abstract: false,
                is_final: true,
                throws: Vec::new(),
                is_async: false,
            };
            let info = ClassInfo {
                name: ic.name.to_string(),
                module: String::new(),
                is_static: false,
                generic_params: ic
                    .generic_params
                    .iter()
                    .map(|n| GenericParam {
                        name: n.to_string(),
                        variance: Variance::Invariant,
                        bounds: Vec::new(),
                        union: Vec::new(),
                        requires_new: false,
                    })
                    .collect(),
                is_interface: false,
                // Static classes (no constructor) are marked abstract so
                // `new Console()` is rejected with the usual message.
                is_abstract: ic.constructor.is_none(),
                is_sealed: true,
                is_record: false,
                super_class: None,
                interfaces: Vec::new(),
            interface_args: HashMap::new(),
                instance_fields: Vec::new(),
                static_fields: Vec::new(),
                protected_fields: HashSet::new(),
                internal_fields: HashSet::new(),
                internal_static_fields: HashSet::new(),
                protected_static_fields: HashSet::new(),
                private_fields: HashSet::new(),
                private_static_fields: HashSet::new(),
                methods: ic
                    .methods
                    .iter()
                    .map(|m| (m.name.to_string(), to_method_info(m, true)))
                    .collect(),
                static_methods: ic
                    .static_methods
                    .iter()
                    .map(|m| (m.name.to_string(), to_method_info(m, false)))
                    .collect(),
                constructors: if ic.constructor.is_some() {
                    vec![ConstructorInfo { params: ic.constructor_params.clone() }]
                } else {
                    Vec::new()
                },
                properties: ic
                    .properties
                    .iter()
                    .map(|p| {
                        (
                            p.name.to_string(),
                            PropertyInfo {
                                name: p.name.to_string(),
                                ty: p.ty.clone(),
                                visibility: Visibility::Public,
                                getter_visibility: Visibility::Public,
                                setter_visibility: Visibility::Public,
                                has_getter: true,
                                has_setter: false,
                                is_static: false,
                            },
                        )
                    })
                    .collect(),
                static_properties: HashMap::new(),
            };
            self.classes.insert(ic.name.to_string(), info);
        }
    }

    fn gather_decls(&mut self, program: &Program) -> Result<(), TypeError> {
        // Newtypes first, so class/enum name collisions are caught whichever
        // declaration order the source uses. The parser has already resolved
        // and validated the underlying primitive.
        for decl in &program.decls {
            if let Decl::Newtype(n) = decl {
                if self.newtypes.contains_key(&n.name) {
                    return Err(TypeError(format!("duplicate newtype `{}`", n.name)));
                }
                if self.classes.contains_key(&n.name) {
                    return Err(TypeError(format!(
                        "newtype `{}` conflicts with a class of the same name",
                        n.name
                    )));
                }
                if self.enums.contains_key(&n.name) {
                    return Err(TypeError(format!(
                        "newtype `{}` conflicts with an enum of the same name",
                        n.name
                    )));
                }
                self.newtypes.insert(n.name.clone(), n.underlying.clone());
            }
        }
        for decl in &program.decls {
            match decl {
                Decl::Class(c) => {
                    if self.classes.contains_key(&c.name) {
                        return Err(TypeError(format!("duplicate class `{}`", c.name)));
                    }
                    if self.newtypes.contains_key(&c.name) {
                        return Err(TypeError(format!(
                            "class `{}` conflicts with a newtype of the same name",
                            c.name
                        )));
                    }
                    if c.is_static {
                        if !c.bases.is_empty() {
                            return Err(TypeError(format!(
                                "static class `{}` cannot inherit or implement interfaces",
                                c.name
                            )));
                        }
                        for member in &c.members {
                            let (is_static, what, mname): (bool, &str, &str) = match member {
                                Member::Field(f) => (f.is_static, "field", &f.name),
                                Member::Method(m) if m.is_constructor => {
                                    return Err(TypeError(format!(
                                        "static class `{}` cannot declare a constructor",
                                        c.name
                                    )));
                                }
                                Member::Method(m) => (m.is_static, "method", &m.name),
                                Member::Property(p) => (p.is_static, "property", &p.name),
                            };
                            if !is_static {
                                return Err(TypeError(format!(
                                    "static class `{}` cannot declare instance {} `{}`; all members must be static",
                                    c.name, what, mname
                                )));
                            }
                        }
                    }
                    // Field initializers: static fields accept constant
                    // literals only (applied at VM startup); instance field
                    // initializers need constructor injection and are not
                    // supported yet.
                    for member in &c.members {
                        if let Member::Field(f) = member {
                            if let Some(init) = &f.init {
                                if !f.is_static {
                                    return Err(TypeError(format!(
                                        "instance field `{}.{}` cannot have an initializer yet; assign it in a constructor",
                                        c.name, f.name
                                    )));
                                }
                                let lit_ty = match init {
                                    Expr::IntLit(v, suffix) => self.suffixed_int_type(*suffix, *v)?,
                                    Expr::FloatLit(_, _) => Type::Float64,
                                    Expr::Bool(_) => Type::Bool,
                                    Expr::Char(_) => Type::Char,
                                    Expr::String(v) => Type::StringLit(v.clone()),
                                    Expr::Unary(UnaryOp::Neg, inner) => match inner.as_ref() {
                                        Expr::IntLit(v, suffix) => {
                                            self.suffixed_int_type(*suffix, -*v)?
                                        }
                                        Expr::FloatLit(_, _) => Type::Float64,
                                        _ => {
                                            return Err(TypeError(format!(
                                                "static field `{}.{}` initializer must be a constant literal",
                                                c.name, f.name
                                            )));
                                        }
                                    },
                                    _ => {
                                        return Err(TypeError(format!(
                                            "static field `{}.{}` initializer must be a constant literal",
                                            c.name, f.name
                                        )));
                                    }
                                };
                                if !self.is_assignable(&f.ty, &lit_ty) {
                                    return Err(self.mismatch_error(
                                        format!(
                                            "cannot initialize static field `{}.{}` of type {} with {}",
                                            c.name,
                                            f.name,
                                            f.ty.name(),
                                            lit_ty.name()
                                        ),
                                        &f.ty,
                                        &lit_ty,
                                    ));
                                }
                            }
                        }
                    }
                    if self.enums.contains_key(&c.name) {
                        return Err(TypeError(format!("`{}` is already defined as an enum", c.name)));
                    }
                    let mut info = ClassInfo {
                        name: c.name.clone(),
                        module: c.module.clone(),
                        is_static: c.is_static,
                        generic_params: c.generic_params.clone(),
                        is_interface: c.is_interface,
                        is_abstract: c.is_abstract,
                        is_sealed: c.is_sealed,
                        is_record: c.is_record,
                        super_class: None,
                        interfaces: Vec::new(),
                        interface_args: HashMap::new(),
                        instance_fields: Vec::new(),
                        static_fields: Vec::new(),
                        protected_fields: HashSet::new(),
                        internal_fields: HashSet::new(),
                        internal_static_fields: HashSet::new(),
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
                                        Visibility::Internal => {
                                            info.internal_static_fields.insert(f.name.clone());
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
                                        Visibility::Internal => {
                                            info.internal_fields.insert(f.name.clone());
                                        }
                                        Visibility::Public => {}
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
                                if m.is_async {
                                    if !m.is_static {
                                        return Err(TypeError(format!(
                                            "async method `{}.{}` must be static (instance/virtual async is not supported yet)",
                                            c.name, m.name
                                        )));
                                    }
                                    if c.is_interface {
                                        return Err(TypeError(format!(
                                            "interface `{}` cannot declare async method `{}`",
                                            c.name, m.name
                                        )));
                                    }
                                    if !m.throws.is_empty() {
                                        return Err(TypeError(format!(
                                            "async method `{}.{}` cannot declare `throws` (task exceptions surface at await/wait sites)",
                                            c.name, m.name
                                        )));
                                    }
                                    match &m.return_ty {
                                        Type::Class(n, args) if n == "Task" && args.len() == 1 => {}
                                        other => {
                                            return Err(TypeError(format!(
                                                "async method `{}.{}` must return Task<T>, not {}",
                                                c.name,
                                                m.name,
                                                other.name()
                                            )));
                                        }
                                    }
                                }
                                // Operator overloads (`operator+` etc.): one
                                // parameter (the right operand; `this` is the
                                // left), a real result, and comparisons must
                                // yield bool so `a < b` stays a condition.
                                if let Some(sym) = operator_symbol_of(&m.name) {
                                    if m.params.len() != 1 {
                                        return Err(TypeError(format!(
                                            "`operator{}` on class `{}` must take exactly one \
                                             parameter (the right operand)",
                                            sym, c.name
                                        )));
                                    }
                                    if m.return_ty == Type::Unit {
                                        return Err(TypeError(format!(
                                            "`operator{}` on class `{}` must return a value",
                                            sym, c.name
                                        )));
                                    }
                                    if matches!(sym, "==" | "<" | "<=" | ">" | ">=")
                                        && m.return_ty != Type::Bool
                                    {
                                        return Err(TypeError(format!(
                                            "`operator{}` on class `{}` must return bool",
                                            sym, c.name
                                        )));
                                    }
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
                                    throws: m.throws.clone(),
                                    is_async: m.is_async,
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
                Decl::TypeAlias(_) => {}
                Decl::Newtype(_) => {} // registered in the pre-pass above
                Decl::LiteralUnion(_) => {} // consumed by alias expansion
            }
        }
        self.split_bases(program)?;
        self.validate_hierarchy()?;
        // Checked-exception validation: `throws` names must be exception
        // classes, and an override or interface implementation may not add
        // throws its base declaration lacks (callers dispatch through the
        // base contract).
        for decl in &program.decls {
            let Decl::Class(c) = decl else { continue };
            for member in &c.members {
                let Member::Method(m) = member else { continue };
                for exc in &m.throws {
                    let ok = self
                        .classes
                        .get(exc)
                        .map(|i| !i.is_interface && !i.is_static)
                        .unwrap_or(false)
                        && (exc == "Exception" || self.is_subclass_of(exc, "Exception"));
                    if !ok {
                        return Err(TypeError(format!(
                            "`{}.{}` declares `throws {}`, which is not an exception class (must derive from `Exception`)",
                            c.name, m.name, exc
                        )));
                    }
                }
            }
        }
        for info in self.classes.values() {
            for (mname, mi) in &info.methods {
                let mut bases: Vec<(String, MethodInfo)> = Vec::new();
                if let Some(sup) = &info.super_class {
                    if let Some((d, base)) = self.find_method(sup, mname) {
                        bases.push((d, base));
                    }
                }
                let mut visited = HashSet::new();
                for iface in self.all_interfaces(&info.name, &mut visited) {
                    if let Some(im) = self.classes.get(&iface).and_then(|i| i.methods.get(mname)) {
                        bases.push((iface.clone(), im.clone()));
                    }
                }
                for (base_name, base_mi) in bases {
                    for exc in &mi.throws {
                        let covered = base_mi
                            .throws
                            .iter()
                            .any(|b| b == exc || self.is_subclass_of(exc, b));
                        if !covered {
                            return Err(TypeError(format!(
                                "`{}.{}` declares `throws {}`, but the declaration it overrides in `{}` does not: callers dispatching through `{}` could not know to handle it",
                                info.name, mname, exc, base_name, base_name
                            )));
                        }
                    }
                }
            }
        }
        // Variance validation: `in`/`out` annotations are interface-only,
        // never on method type parameters, and a variant parameter may only
        // appear on its own side of member signatures — conservatively, any
        // occurrence (however nested) on the wrong side is rejected.
        for decl in &program.decls {
            let Decl::Class(c) = decl else { continue };
            for member in &c.members {
                if let Member::Method(m) = member {
                    if let Some(gp) = m
                        .generic_params
                        .iter()
                        .find(|g| g.variance != Variance::Invariant)
                    {
                        return Err(TypeError(format!(
                            "variance annotation on `{}` in `{}.{}`: `in`/`out` are only \
                             allowed on interface type parameters",
                            gp.name, c.name, m.name
                        )));
                    }
                }
            }
            let variant: Vec<&GenericParam> = c
                .generic_params
                .iter()
                .filter(|g| g.variance != Variance::Invariant)
                .collect();
            if variant.is_empty() {
                continue;
            }
            if !c.is_interface {
                return Err(TypeError(format!(
                    "variance annotation on `{}` of `{}`: `in`/`out` are only allowed on \
                     interface type parameters",
                    variant[0].name, c.name
                )));
            }
            for gp in variant {
                for member in &c.members {
                    match member {
                        Member::Method(m) => match gp.variance {
                            Variance::Covariant => {
                                if let Some(p) =
                                    m.params.iter().find(|p| type_mentions(&p.ty, &gp.name))
                                {
                                    return Err(TypeError(format!(
                                        "covariant `out {}` cannot appear in parameter \
                                         `{}` of `{}.{}`: `out` parameters are \
                                         output-only (returns)",
                                        gp.name, p.name, c.name, m.name
                                    )));
                                }
                            }
                            Variance::Contravariant => {
                                if type_mentions(&m.return_ty, &gp.name) {
                                    return Err(TypeError(format!(
                                        "contravariant `in {}` cannot appear in the \
                                         return type of `{}.{}`: `in` parameters are \
                                         input-only (method parameters)",
                                        gp.name, c.name, m.name
                                    )));
                                }
                            }
                            Variance::Invariant => {}
                        },
                        Member::Property(p) => {
                            if type_mentions(&p.ty, &gp.name) {
                                let bad = match gp.variance {
                                    Variance::Covariant => p.setter.is_some(),
                                    Variance::Contravariant => p.getter.is_some(),
                                    Variance::Invariant => false,
                                };
                                if bad {
                                    return Err(TypeError(format!(
                                        "variant `{}` cannot appear in property `{}.{}`: \
                                         a {} makes it flow the wrong way",
                                        gp.name,
                                        c.name,
                                        p.name,
                                        if gp.variance == Variance::Covariant {
                                            "setter"
                                        } else {
                                            "getter"
                                        }
                                    )));
                                }
                            }
                        }
                        Member::Field(_) => {}
                    }
                }
            }
        }
        // Extension registration. The desugared static class was gathered
        // like any other above; this builds and validates the call-site
        // mapping (target -> method -> extension class). Runs after the main
        // loop so targets declared later in the file resolve.
        for decl in &program.decls {
            let Decl::Class(c) = decl else { continue };
            let Some(target) = &c.extension_on else { continue };
            let key = match target {
                Type::String => "string".to_string(),
                Type::Class(n, args) if !args.is_empty() => {
                    return Err(TypeError(format!(
                        "extension `{}` on `{}`: generic extension targets are not \
                         supported yet",
                        c.name, n
                    )));
                }
                Type::Class(n, _) => {
                    if self.enums.contains_key(n) {
                        return Err(TypeError(format!(
                            "extension `{}`: extensions on enum `{}` are not supported",
                            c.name, n
                        )));
                    }
                    if self.newtypes.contains_key(n) {
                        return Err(TypeError(format!(
                            "extension `{}`: extensions on newtype `{}` are not supported",
                            c.name, n
                        )));
                    }
                    if crate::intrinsics::is_intrinsic_class(n) {
                        return Err(TypeError(format!(
                            "extension `{}`: extensions on built-in class `{}` are \
                             not supported yet",
                            c.name, n
                        )));
                    }
                    let Some(info) = self.classes.get(n) else {
                        return Err(TypeError(format!(
                            "extension `{}` targets unknown type `{}`",
                            c.name, n
                        )));
                    };
                    if info.is_static {
                        return Err(TypeError(format!(
                            "extension `{}` cannot target static class `{}`: it has \
                             no instances",
                            c.name, n
                        )));
                    }
                    n.clone()
                }
                other => {
                    return Err(TypeError(format!(
                        "extension `{}` target must be `string` or a class, not {}",
                        c.name,
                        other.name()
                    )));
                }
            };
            for member in &c.members {
                let Member::Method(m) = member else { continue };
                // A real method on the target always wins at call sites, so
                // an extension method it shadows could never be called —
                // reject the dead declaration outright.
                if key == "string" {
                    if crate::intrinsics::string_method(&m.name).is_some() {
                        return Err(TypeError(format!(
                            "extension method `{}.{}` conflicts with a built-in \
                             string method",
                            c.name, m.name
                        )));
                    }
                } else if self.find_method(&key, &m.name).is_some() {
                    return Err(TypeError(format!(
                        "extension method `{}.{}` conflicts with an existing method \
                         on `{}`",
                        c.name, m.name, key
                    )));
                }
                let entry = self.extensions.entry(key.clone()).or_default();
                if let Some(prev) = entry.insert(m.name.clone(), c.name.clone()) {
                    return Err(TypeError(format!(
                        "duplicate extension method `{}` on `{}`: declared in both \
                         `{}` and `{}`",
                        m.name, key, prev, c.name
                    )));
                }
            }
        }
        Ok(())
    }

    /// Split the raw base-name list of each class into a super class (for
    /// classes) and a list of implemented/extended interfaces.
    fn split_bases(&mut self, program: &Program) -> Result<(), TypeError> {
        for decl in &program.decls {
            if let Decl::Class(c) = decl {
                let mut super_class = None;
                let mut interfaces = Vec::new();
                let mut interface_args: HashMap<String, Vec<Type>> = HashMap::new();
                for (base, _) in &c.bases {
                    if self.classes.get(base).map(|i| i.is_static).unwrap_or(false) {
                        return Err(TypeError(format!(
                            "cannot inherit from static class `{}`",
                            base
                        )));
                    }
                }
                for (base, base_args) in &c.bases {
                    let base_info = self
                        .classes
                        .get(base)
                        .ok_or_else(|| {
                            TypeError(format!("unknown base type `{}` for `{}`", base, c.name))
                        })?;
                    let is_interface = base_info.is_interface;
                    if is_interface {
                        // A generic interface must be implemented at an
                        // instantiation; the declared arguments drive
                        // conformance checking and variance-aware
                        // assignability.
                        if base_info.generic_params.len() != base_args.len() {
                            return Err(TypeError(format!(
                                "interface `{}` expects {} type argument(s), but `{}` \
                                 implements it with {}",
                                base,
                                base_info.generic_params.len(),
                                c.name,
                                base_args.len()
                            )));
                        }
                        for arg in base_args {
                            self.validate_type_with_generics(arg, &c.generic_params)?;
                        }
                        interfaces.push(base.clone());
                        interface_args.insert(base.clone(), base_args.clone());
                    } else if !base_args.is_empty() {
                        return Err(TypeError(format!(
                            "class `{}` extends `{}` with type arguments: generic base \
                             classes are not supported yet",
                            c.name, base
                        )));
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
                info.interface_args = interface_args;
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
                    } else if info.internal_fields.contains(field) {
                        Visibility::Internal
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
                    } else if info.internal_static_fields.contains(field) {
                        Visibility::Internal
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

    /// The type arguments with which `class_name` (instantiated at
    /// `class_args`) implements interface `iface`, if it does: the directly
    /// declared instantiation, or one inherited from a superclass or an
    /// extended interface. Only the top-level class's own parameters need
    /// substituting — generic base classes are unsupported, so deeper
    /// declarations are already concrete.
    fn declared_interface_args(
        &self,
        class_name: &str,
        class_args: &[Type],
        iface: &str,
    ) -> Option<Vec<Type>> {
        let top = self.classes.get(class_name)?;
        let subst = build_subst(&top.generic_params, class_args);
        let mut queue = vec![class_name.to_string()];
        let mut seen = HashSet::new();
        while let Some(c) = queue.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            let Some(info) = self.classes.get(&c) else {
                continue;
            };
            if let Some(args) = info.interface_args.get(iface) {
                return Some(args.iter().map(|a| substitute_type(a, &subst)).collect());
            }
            if let Some(sup) = &info.super_class {
                queue.push(sup.clone());
            }
            queue.extend(info.interfaces.iter().cloned());
        }
        None
    }

    /// Infer an expression's type with an optional expected type. Lambdas
    /// are the only target-typed expressions: their parameter and return
    /// types come from the expected `Func<...>` when not annotated.
    #[allow(clippy::too_many_arguments)]
    fn infer_expr_expecting(
        &self,
        e: &Expr,
        expected: Option<&Type>,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        if let Expr::Lambda { params, body } = e {
            return self.check_lambda(
                params, body, expected, class, locals, in_instance, generic_params,
            );
        }
        self.infer_expr(e, class, locals, in_instance, return_ty, generic_params)
    }

    /// Type-check a lambda. Parameter types come from annotations or the
    /// expected `Func<...>`; the body is checked against the expected
    /// return type. Without a target type the lambda must annotate every
    /// parameter and have an expression body (its return type is then
    /// inferred). Captured outer locals are read-only (by-value capture).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn check_lambda(
        &self,
        params: &[(String, Option<Type>)],
        body: &[Stmt],
        expected: Option<&Type>,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        self.check_lambda_open(
            params, body, expected, None, class, locals, in_instance, generic_params,
        )
    }

    /// `check_lambda` with an optional set of still-unbound method type
    /// parameters: an expected return type mentioning one is treated as
    /// open, and the lambda's body determines it (expression bodies only) —
    /// the C#-style flow where `transform(21, x => x * 2)` binds `T2` from
    /// the lambda's result.
    #[allow(clippy::too_many_arguments)]
    fn check_lambda_open(
        &self,
        params: &[(String, Option<Type>)],
        body: &[Stmt],
        expected: Option<&Type>,
        open_vars: Option<&HashSet<String>>,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        generic_params: &[GenericParam],
    ) -> Result<Type, TypeError> {
        // Unwrap the expectation down to a Func shape when there is one.
        let exp = match expected {
            Some(Type::Func(p, r)) => Some((p.clone(), (**r).clone())),
            Some(Type::Nullable(inner)) => match inner.as_ref() {
                Type::Func(p, r) => Some((p.clone(), (**r).clone())),
                _ => None,
            },
            _ => None,
        };
        if let Some((ep, _)) = &exp {
            if ep.len() != params.len() {
                return Err(TypeError(format!(
                    "lambda has {} parameter(s) but {} takes {}",
                    params.len(),
                    expected.unwrap().name(),
                    ep.len()
                )));
            }
        }
        let mut param_tys: Vec<Type> = Vec::with_capacity(params.len());
        for (i, (pname, ann)) in params.iter().enumerate() {
            let ty = match (ann, &exp) {
                (Some(t), _) => t.clone(),
                (None, Some((ep, _))) => ep[i].clone(),
                (None, None) => {
                    return Err(TypeError(format!(
                        "cannot infer the type of lambda parameter `{}`: annotate it \
                         (`(int {}) => ...`) or give the lambda a Func<...> target type",
                        pname, pname
                    )));
                }
            };
            self.validate_type_with_generics(&ty, generic_params)?;
            param_tys.push(ty);
        }
        // By-value capture: assigning to a captured outer local inside the
        // body would silently not affect the original — reject it.
        let mut declared: HashSet<String> = params.iter().map(|(n, _)| n.clone()).collect();
        collect_declared_names(body, &mut declared);
        let mut assigned: HashSet<String> = HashSet::new();
        collect_assigned_locals(body, &mut assigned);
        for name in &assigned {
            if !declared.contains(name) && locals.contains_key(name) {
                return Err(TypeError(format!(
                    "lambda captures `{}` by value; assigning to it inside the lambda \
                     would not affect the outer variable",
                    name
                )));
            }
        }
        // Body scope: outer locals (captures) plus parameters.
        let mut body_locals = locals.clone();
        for ((n, _), t) in params.iter().zip(&param_tys) {
            body_locals.insert(n.clone(), t.clone());
        }
        let ret_is_open = match (&exp, open_vars) {
            (Some((_, r)), Some(vars)) => vars.iter().any(|v| type_mentions(r, v)),
            _ => false,
        };
        let saved_narrow = self.narrowed.replace(HashMap::new());
        let saved_async = self.current_is_async.replace(false);
        let result = (|| -> Result<Type, TypeError> {
            match &exp {
                Some((_, exp_ret)) if !ret_is_open => {
                    if *exp_ret == Type::Unit {
                        // `Action` with an expression body: the expression
                        // is a statement; any result is discarded.
                        if let [Stmt::Return(Some(e))] = body {
                            let _ = self.infer_expr(
                                e, class, &body_locals, in_instance, exp_ret, generic_params,
                            )?;
                            return Ok(Type::Func(param_tys.clone(), Box::new(Type::Unit)));
                        }
                    }
                    let mut body_locals = body_locals.clone();
                    for stmt in body {
                        self.check_stmt(
                            stmt,
                            class,
                            &mut body_locals,
                            exp_ret,
                            in_instance,
                            generic_params,
                        )?;
                    }
                    Ok(Type::Func(param_tys.clone(), Box::new(exp_ret.clone())))
                }
                _ => {
                    // Self-inference (annotated parameters, or an open
                    // return the body determines): expression body only.
                    if let [Stmt::Return(Some(e))] = body {
                        let ret = self.infer_expr(
                            e, class, &body_locals, in_instance, &Type::Unit, generic_params,
                        )?;
                        Ok(Type::Func(
                            param_tys.clone(),
                            Box::new(self.resolve_literal(&ret)),
                        ))
                    } else {
                        Err(TypeError(
                            "a block-bodied lambda needs a target type: assign it to a \
                             Func<...>/Action<...> variable or pass it to a typed parameter"
                                .to_string(),
                        ))
                    }
                }
            }
        })();
        self.narrowed.replace(saved_narrow);
        self.current_is_async.set(saved_async);
        result
    }

    /// Infer a generic class's type arguments from a constructor call with
    /// none written (`new Box(42)` -> `Box<int>`): the first constructor of
    /// matching arity whose parameters unify against the argument types and
    /// bind every parameter wins. Bindings from several arguments join
    /// through assignability, exactly like generic-method inference.
    fn infer_new_type_args(
        &self,
        class_info: &ClassInfo,
        class_name: &str,
        arg_types: &[Type],
    ) -> Result<Vec<Type>, TypeError> {
        let vars: HashSet<String> = class_info
            .generic_params
            .iter()
            .map(|gp| gp.name.clone())
            .collect();
        let mut last_conflict: Option<(String, Type, Type)> = None;
        for ctor in &class_info.constructors {
            if ctor.params.len() != arg_types.len() {
                continue;
            }
            let mut bindings: HashMap<String, Type> = HashMap::new();
            let mut conflict: Option<(String, Type, Type)> = None;
            for (param, arg_ty) in ctor.params.iter().zip(arg_types) {
                unify_generic_types(
                    param,
                    &self.resolve_literal(arg_ty),
                    &vars,
                    &mut bindings,
                    &mut |name, prev, new| {
                        if self.is_assignable(&prev, &new) {
                            Some(prev)
                        } else if self.is_assignable(&new, &prev) {
                            Some(new)
                        } else {
                            if conflict.is_none() {
                                conflict = Some((name.to_string(), prev.clone(), new.clone()));
                            }
                            None
                        }
                    },
                );
            }
            if let Some(c) = conflict {
                last_conflict = Some(c);
                continue;
            }
            if let Some(all) = class_info
                .generic_params
                .iter()
                .map(|gp| bindings.get(&gp.name).cloned())
                .collect::<Option<Vec<Type>>>()
            {
                return Ok(all);
            }
        }
        if let Some((name, a, b)) = last_conflict {
            return Err(TypeError(format!(
                "cannot infer `{}` for `new {}(...)`: conflicting argument types {} and {}",
                name,
                class_name,
                a.name(),
                b.name()
            )));
        }
        Err(TypeError(format!(
            "cannot infer type arguments for `{}` from this constructor call; write \
             `new {}<...>(...)` explicitly",
            class_name, class_name
        )))
    }

    /// The declared type parameter a value of type `ty` refers to, if any.
    /// `T` appears as `Type::GenericParam("T")` or — since the parser emits
    /// plain idents as class types — `Type::Class("T", [])` when no class of
    /// that name exists.
    fn generic_param_of<'a>(
        &self,
        ty: &Type,
        generic_params: &'a [GenericParam],
    ) -> Option<&'a GenericParam> {
        let name = match ty {
            Type::GenericParam(n) => n,
            Type::Class(n, args) if args.is_empty() && !self.classes.contains_key(n) => n,
            _ => return None,
        };
        generic_params.iter().find(|gp| gp.name == *name)
    }

    /// True when the parameter carries a union constraint whose alternatives
    /// are all numeric (`where T : int | float`) — the license for
    /// arithmetic and ordering on `T` values.
    fn union_is_numeric(&self, gp: &GenericParam) -> bool {
        !gp.union.is_empty() && gp.union.iter().all(|t| self.is_numeric(t))
    }

    /// Enforce a `new()` constraint: the argument must be a concrete class
    /// with a parameterless (or no declared) constructor.
    fn require_default_constructible(
        &self,
        arg: &Type,
        param: &str,
        owner: &str,
    ) -> Result<(), TypeError> {
        let ok = match arg {
            Type::Class(n, _) => self.classes.get(n).is_some_and(|info| {
                !info.is_interface
                    && !info.is_abstract
                    && !info.is_static
                    && (info.constructors.is_empty()
                        || info.constructors.iter().any(|c| c.params.is_empty()))
            }),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(TypeError(format!(
                "type argument {} for parameter `{}` of `{}` does not satisfy `new()`: \
                 it must be a concrete class with a parameterless constructor",
                arg.name(),
                param,
                owner
            )))
        }
    }

    /// Enforce the checked-exception contract at a call site: each declared
    /// throw of the callee must be caught by an enclosing `try` (a catch of
    /// the type or a supertype) or re-declared by the calling method's own
    /// `throws` clause. Methods without a `throws` clause stay unchecked.
    fn require_throws_handled(&self, mi: &MethodInfo, callee: &str) -> Result<(), TypeError> {
        for exc in &mi.throws {
            let caught = self.handled_exceptions.borrow().iter().flatten().any(|c| {
                matches!(c, Type::Class(cn, _) if cn == exc || self.is_subclass_of(exc, cn))
            });
            let declared = self
                .current_throws
                .borrow()
                .iter()
                .any(|d| d == exc || self.is_subclass_of(exc, d));
            if !caught && !declared {
                return Err(TypeError(format!(
                    "call to `{}` may throw `{}`: catch it or declare `throws {}`",
                    callee, exc, exc
                )));
            }
        }
        Ok(())
    }

    /// Resolve a user-defined operator for a binary op whose left operand
    /// has type `lt`: the left class (or a base) declares the operator
    /// method. Returns the (parameter, return) types with the receiver's
    /// type arguments substituted.
    fn user_operator(&self, op: BinOp, lt: &Type) -> Option<(Type, Type)> {
        let mname = operator_method_name(op)?;
        let Type::Class(name, type_args) = lt else { return None };
        if name == "null" {
            return None;
        }
        let (_, mi) = self.find_method(name, mname)?;
        let info = self.classes.get(name)?;
        let subst = build_subst(&info.generic_params, type_args);
        Some((
            substitute_type(&mi.params[0], &subst),
            substitute_type(&mi.return_ty, &subst),
        ))
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
    /// Field names visible on `class_name` (own + inherited), for
    /// did-you-mean suggestions.
    fn field_candidates(&self, class_name: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = Some(class_name.to_string());
        while let Some(c) = cur {
            let Some(info) = self.classes.get(&c) else { break };
            out.extend(info.instance_fields.iter().map(|(n, _)| n.clone()));
            cur = info.super_class.clone();
        }
        out
    }

    /// Method names visible on `class_name` (own + inherited), for
    /// did-you-mean suggestions.
    fn method_candidates(&self, class_name: &str, is_static: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = Some(class_name.to_string());
        while let Some(c) = cur {
            let Some(info) = self.classes.get(&c) else { break };
            let map = if is_static { &info.static_methods } else { &info.methods };
            out.extend(map.keys().cloned());
            cur = info.super_class.clone();
        }
        out
    }

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
        // A nested class can access its enclosing class's private and
        // protected members (`Outer.Inner` reaches into `Outer`); the
        // reverse is not granted, as in C#.
        let nested_in = |inner: &str, outer: &str| {
            inner.len() > outer.len() + 1
                && inner.starts_with(outer)
                && inner.as_bytes()[outer.len()] == b'.'
        };
        match visibility {
            Visibility::Public => true,
            Visibility::Protected => {
                current == declared_in
                    || self.is_subclass_of(current, declared_in)
                    || nested_in(current, declared_in)
            }
            Visibility::Private => current == declared_in || nested_in(current, declared_in),
            // Module-scoped: accessible only from classes declared in the
            // same source module (file). Single-file programs put every
            // class in one module, so `internal` is file-wide there.
            Visibility::Internal => {
                let module_of = |name: &str| {
                    self.classes.get(name).map(|i| i.module.clone()).unwrap_or_default()
                };
                module_of(current) == module_of(declared_in)
            }
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
                        // Compare against the interface signature at the
                        // declared instantiation: `class CatBox :
                        // IProducer<Cat>` must implement `produce()` at
                        // `Cat`, not at the interface's own `T`.
                        let iargs = self
                            .declared_interface_args(&info.name, &[], &iface)
                            .unwrap_or_default();
                        let isubst = build_subst(&ii.generic_params, &iargs);
                        let want_ret = substitute_type(&mi.return_ty, &isubst);
                        let want_params: Vec<Type> =
                            mi.params.iter().map(|p| substitute_type(p, &isubst)).collect();
                        if found.return_ty != want_ret || found.params != want_params {
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
                self.current_line.set(0);
                *self.current_context.borrow_mut() = format!("{}.{}", class.name, m.name);
                *self.current_throws.borrow_mut() = m.throws.clone();
                self.handled_exceptions.borrow_mut().clear();
                self.current_is_async.set(m.is_async);
                let mut locals: HashMap<String, Type> = HashMap::new();
                let mut all_generic_params = class_generic_params.clone();
                all_generic_params.extend(m.generic_params.clone());
                
                for param in &m.params {
                    self.validate_type_with_generics(&param.ty, &all_generic_params)?;
                    locals.insert(param.name.clone(), param.ty.clone());
                }
                self.validate_type_with_generics(&m.return_ty, &all_generic_params)?;
                // Async bodies return the task's element type: `return 5;`
                // inside `async Task<int>`.
                let body_ret: Type = match (&m.is_async, &m.return_ty) {
                    (true, Type::Class(n, args)) if n == "Task" && args.len() == 1 => {
                        args[0].clone()
                    }
                    _ => m.return_ty.clone(),
                };
                for stmt in &m.body {
                    // Multiple-error reporting: a failed statement is
                    // recorded, the rest of this method is skipped (its
                    // statements would cascade), and the next method is
                    // checked normally.
                    if let Err(TypeError(msg)) = self.check_stmt(
                        stmt,
                        &info,
                        &mut locals,
                        &body_ret,
                        !m.is_static,
                        &all_generic_params,
                    ) {
                        self.record_error(msg);
                        break;
                    }
                }
                self.current_is_async.set(false);
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

    /// Recognize `x != null` / `x == null` (either operand order) on a local
    /// variable. Returns (name, is_not_null).
    fn null_comparison(cond: &Expr) -> Option<(String, bool)> {
        let (op, l, r) = match cond {
            Expr::Binary(op @ (BinOp::Ne | BinOp::Eq), l, r) => (op, l.as_ref(), r.as_ref()),
            _ => return None,
        };
        let name = match (l, r) {
            (Expr::Var(n), Expr::Null) | (Expr::Null, Expr::Var(n)) => n.clone(),
            _ => return None,
        };
        Some((name, matches!(op, BinOp::Ne)))
    }

    /// True if a branch always transfers control away (so code after the
    /// enclosing `if` only runs when the branch was not taken).
    fn branch_terminates(body: &[Stmt]) -> bool {
        matches!(
            body.last(),
            Some(Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break(_) | Stmt::Continue(_))
        )
    }

    /// Run `f` with local `name` narrowed from `T?` to `T`, restoring the
    /// declared type afterwards.
    fn with_narrowed<R>(
        &self,
        locals: &mut HashMap<String, Type>,
        name: &str,
        inner: Type,
        declared: Type,
        f: impl FnOnce(&Self, &mut HashMap<String, Type>) -> R,
    ) -> R {
        locals.insert(name.to_string(), inner);
        let prev = self.narrowed.borrow_mut().insert(name.to_string(), declared.clone());
        let result = f(self, locals);
        locals.insert(name.to_string(), declared);
        match prev {
            Some(p) => { self.narrowed.borrow_mut().insert(name.to_string(), p); }
            None => { self.narrowed.borrow_mut().remove(name); }
        }
        result
    }

    /// The (name, non-null inner, declared nullable) triple for a condition
    /// that null-tests a nullable local, if any.
    /// A one-line hint explaining WHY `source` does not flow into `target`,
    /// when the pair matches a known pattern with a known fix. Appended to
    /// assignment/argument/return mismatch errors.
    fn mismatch_hint(&self, target: &Type, source: &Type) -> Option<String> {
        match (target, source) {
            (t, Type::Nullable(inner)) if self.is_assignable(t, inner) => Some(
                "the value may be null: narrow with `if (x != null)`, or use `!` / `??`".to_string(),
            ),
            (Type::Newtype(n, u), s) if self.is_assignable(u, s) => {
                Some(format!("wrap the value: `{}(...)`", n))
            }
            (t, Type::Newtype(_, u)) if self.is_assignable(t, u) => {
                Some("unwrap the value with `.Value`".to_string())
            }
            (Type::LiteralUnion(n, members), Type::StringLit(v)) => {
                let shown: Vec<String> =
                    members.iter().take(6).map(|m| format!("{:?}", m)).collect();
                let more = if members.len() > 6 { " | ..." } else { "" };
                Some(format!(
                    "{:?} is not a member of `{}` (members: {}{})",
                    v,
                    n,
                    shown.join(" | "),
                    more
                ))
            }
            (Type::LiteralUnion(tn, tm), Type::LiteralUnion(sn, sm)) => {
                let extra: Vec<String> = sm
                    .iter()
                    .filter(|m| !tm.contains(*m))
                    .take(4)
                    .map(|m| format!("{:?}", m))
                    .collect();
                if extra.is_empty() {
                    None
                } else {
                    Some(format!(
                        "`{}` can hold {} which `{}` cannot",
                        sn,
                        extra.join(", "),
                        tn
                    ))
                }
            }
            (Type::Class(tn, ta), Type::Class(sn, _)) if ta.is_empty() => {
                let target_is_iface = self
                    .classes
                    .get(tn)
                    .map(|i| i.is_interface)
                    .unwrap_or(false);
                if target_is_iface {
                    explain_structural_failure(&self.classes, sn, tn)
                } else {
                    None
                }
            }
            (t, s) if self.is_numeric(t) && self.is_numeric(s) => Some(format!(
                "narrowing conversion from {} to {} can lose range; widen the target or convert explicitly",
                s.name(),
                t.name()
            )),
            _ => None,
        }
    }

    /// Build the error for a typed hole (`_`) where `expected` is known:
    /// name the type, list in-scope values that fit, and suggest a way to
    /// construct one when the type has an obvious constructor form.
    fn hole_error(&self, expected: &Type, locals: &HashMap<String, Type>) -> TypeError {
        let mut msg = format!(
            "type hole: an expression of type {} is needed here",
            expected.name()
        );
        let mut candidates: Vec<String> = locals
            .iter()
            .filter(|(name, ty)| *name != "this" && self.is_assignable(expected, ty))
            .map(|(name, ty)| format!("`{}` ({})", name, ty.name()))
            .collect();
        candidates.sort();
        if !candidates.is_empty() {
            let shown = candidates.len().min(6);
            let more = if candidates.len() > 6 { ", ..." } else { "" };
            msg.push_str(&format!(
                "; in scope: {}{}",
                candidates[..shown].join(", "),
                more
            ));
        }
        match expected {
            Type::LiteralUnion(_, members) => {
                let shown: Vec<String> =
                    members.iter().take(6).map(|m| format!("{:?}", m)).collect();
                msg.push_str(&format!("; members: {}", shown.join(" | ")));
            }
            Type::Newtype(n, _) => {
                msg.push_str(&format!("; or construct one: `{}(...)`", n));
            }
            Type::Bool => msg.push_str("; or `true` / `false`"),
            Type::Class(n, _) => {
                if let Some(info) = self.classes.get(n) {
                    if !info.is_interface && !info.is_abstract {
                        msg.push_str(&format!("; or construct one: `new {}(...)`", n));
                    }
                }
            }
            _ => {}
        }
        TypeError(msg)
    }

    /// Format a mismatch error with the hint appended when one applies.
    fn mismatch_error(&self, base: String, target: &Type, source: &Type) -> TypeError {
        match self.mismatch_hint(target, source) {
            Some(hint) => TypeError(format!("{base}; hint: {hint}")),
            None => TypeError(base),
        }
    }

    fn narrowable(&self, cond: &Expr, locals: &HashMap<String, Type>) -> Option<(String, bool, Type, Type)> {
        let (name, is_ne) = Self::null_comparison(cond)?;
        match locals.get(&name) {
            Some(Type::Nullable(inner)) => {
                Some((name.clone(), is_ne, (**inner).clone(), Type::Nullable(inner.clone())))
            }
            _ => None,
        }
    }

    /// Everything a condition proves about locals when it evaluates to
    /// `positive`. Facts compose through `&&` (both sides hold when true),
    /// `||` (both negations hold when false) and `!` (flips polarity):
    ///
    /// * `x != null` when true / `x == null` when false: `x` is non-null.
    /// * `x is T [name]` when true: the binding `name` holds the subject
    ///   typed as `T`, and a nullable subject variable is known non-null.
    ///
    /// `&&`/`||` deliberately contribute nothing in the other polarity — a
    /// false `a && b` doesn't say which side failed.
    fn condition_facts(
        &self,
        cond: &Expr,
        locals: &HashMap<String, Type>,
        positive: bool,
    ) -> Vec<GuardFact> {
        let mut facts = Vec::new();
        self.collect_facts(cond, locals, positive, &mut facts);
        facts
    }

    fn collect_facts(
        &self,
        cond: &Expr,
        locals: &HashMap<String, Type>,
        positive: bool,
        out: &mut Vec<GuardFact>,
    ) {
        if let Some((name, is_ne)) = Self::null_comparison(cond) {
            if is_ne == positive {
                if let Some(Type::Nullable(inner)) = locals.get(&name) {
                    out.push(GuardFact::NonNull {
                        name: name.clone(),
                        inner: (**inner).clone(),
                        declared: Type::Nullable(inner.clone()),
                    });
                }
            }
            return;
        }
        match cond {
            Expr::Is(subject, ty, binding) if positive => {
                if let Some(b) = binding {
                    out.push(GuardFact::Binding { name: b.clone(), ty: ty.clone() });
                }
                if let Expr::Var(name) = subject.as_ref() {
                    if let Some(Type::Nullable(inner)) = locals.get(name) {
                        out.push(GuardFact::NonNull {
                            name: name.clone(),
                            inner: (**inner).clone(),
                            declared: Type::Nullable(inner.clone()),
                        });
                    }
                }
            }
            Expr::Binary(BinOp::And, a, b) if positive => {
                self.collect_facts(a, locals, true, out);
                self.collect_facts(b, locals, true, out);
            }
            Expr::Binary(BinOp::Or, a, b) if !positive => {
                self.collect_facts(a, locals, false, out);
                self.collect_facts(b, locals, false, out);
            }
            Expr::Unary(UnaryOp::Not, a) => {
                self.collect_facts(a, locals, !positive, out);
            }
            _ => {}
        }
    }

    /// Run `f` with every fact applied to `locals` (narrowed types
    /// registered for assignment re-widening; `is`-bindings introduced),
    /// restoring everything afterwards.
    fn with_facts<R>(
        &self,
        locals: &mut HashMap<String, Type>,
        facts: &[GuardFact],
        f: impl FnOnce(&Self, &mut HashMap<String, Type>) -> Result<R, TypeError>,
    ) -> Result<R, TypeError> {
        let mut saved_locals: Vec<(String, Option<Type>)> = Vec::new();
        let mut saved_narrowed: Vec<(String, Option<Type>)> = Vec::new();
        for fact in facts {
            match fact {
                GuardFact::NonNull { name, inner, declared } => {
                    saved_locals.push((name.clone(), locals.insert(name.clone(), inner.clone())));
                    saved_narrowed.push((
                        name.clone(),
                        self.narrowed.borrow_mut().insert(name.clone(), declared.clone()),
                    ));
                }
                GuardFact::Binding { name, ty } => {
                    saved_locals.push((name.clone(), locals.insert(name.clone(), ty.clone())));
                }
            }
        }
        let result = f(self, locals);
        for (name, prev) in saved_locals.into_iter().rev() {
            match prev {
                Some(t) => { locals.insert(name, t); }
                None => { locals.remove(&name); }
            }
        }
        for (name, prev) in saved_narrowed.into_iter().rev() {
            match prev {
                Some(t) => { self.narrowed.borrow_mut().insert(name, t); }
                None => { self.narrowed.borrow_mut().remove(&name); }
            }
        }
        result
    }

    /// Apply facts' type effects to a scratch locals map (expression
    /// contexts: typing the rhs of `&&`/`||`, where no assignments occur).
    fn apply_fact_types(facts: &[GuardFact], locals: &mut HashMap<String, Type>) {
        for fact in facts {
            match fact {
                GuardFact::NonNull { name, inner, .. } => {
                    locals.insert(name.clone(), inner.clone());
                }
                GuardFact::Binding { name, ty } => {
                    locals.insert(name.clone(), ty.clone());
                }
            }
        }
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
                // `var` infers the declared type from the initializer;
                // literal markers resolve to their carrier types so
                // `var x = 5` is an int32, `var s = "hi"` a string.
                if matches!(ty, Type::Infer) {
                    let Some(init) = init else {
                        return Err(TypeError(format!(
                            "`var {}` requires an initializer to infer from",
                            name
                        )));
                    };
                    if matches!(init, Expr::Hole) {
                        return Err(TypeError(format!(
                            "`var {}` cannot infer from a hole; annotate the type",
                            name
                        )));
                    }
                    let init_ty = self.infer_expr(init, class, locals, in_instance, return_ty, generic_params)?;
                    let inferred = self.resolve_literal(&init_ty);
                    if matches!(&inferred, Type::Unit)
                        || matches!(&inferred, Type::Class(n, _) if n == "null")
                    {
                        return Err(TypeError(format!(
                            "cannot infer a type for `{}` from {}; annotate the type",
                            name,
                            inferred.name()
                        )));
                    }
                    locals.insert(name.clone(), inferred);
                    return Ok(());
                }
                self.validate_type_with_generics(ty, generic_params)?;
                if init.is_none()
                    && matches!(ty, Type::Class(_, _) | Type::String | Type::Enum(_) | Type::Tuple(_) | Type::GenericParam(_))
                {
                    return Err(TypeError(format!(
                        "variable `{}` of non-nullable type {} must be initialized (or declared as {}?)",
                        name,
                        ty.name(),
                        ty.name()
                    )));
                }
                if let Some(init) = init {
                    if matches!(init, Expr::Hole) {
                        return Err(self.hole_error(ty, locals));
                    }
                    let init_ty = self.infer_expr_expecting(init, Some(ty), class, locals, in_instance, return_ty, generic_params)?;
                    if !self.is_assignable(ty, &init_ty) {
                        return Err(self.mismatch_error(
                            format!(
                                "cannot assign {} to variable `{}` of type {}",
                                init_ty.name(),
                                name,
                                ty.name()
                            ),
                            ty,
                            &init_ty,
                        ));
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
                // Assigning to a narrowed local checks against its declared
                // (nullable) type and widens it back for later statements.
                if let AssignTarget::Local(name) = target {
                    let declared = self.narrowed.borrow().get(name).cloned();
                    if let Some(declared) = declared {
                        if matches!(value, Expr::Hole) {
                            return Err(self.hole_error(&declared, locals));
                        }
                        let value_ty = self.infer_expr_expecting(value, Some(&declared), class, locals, in_instance, return_ty, generic_params)?;
                        if !self.is_assignable(&declared, &value_ty) {
                            return Err(self.mismatch_error(
                                format!(
                                    "cannot assign {} to `{}` of type {}",
                                    value_ty.name(),
                                    name,
                                    declared.name()
                                ),
                                &declared,
                                &value_ty,
                            ));
                        }
                        locals.insert(name.clone(), declared);
                        self.narrowed.borrow_mut().remove(name);
                        return Ok(());
                    }
                }
                let target_ty = self.infer_assign_target(target, class, locals, in_instance, return_ty, generic_params)?;
                if matches!(value, Expr::Hole) {
                    return Err(self.hole_error(&target_ty, locals));
                }
                let value_ty = self.infer_expr_expecting(value, Some(&target_ty), class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_assignable(&target_ty, &value_ty) {
                    return Err(TypeError(format!(
                        "cannot assign {} to {}",
                        value_ty.name(),
                        target_ty.name()
                    )));
                }
            }
            Stmt::Return(Some(e)) => {
                if matches!(e, Expr::Hole) {
                    return Err(self.hole_error(return_ty, locals));
                }
                let ty = self.infer_expr_expecting(e, Some(return_ty), class, locals, in_instance, return_ty, generic_params)?;
                if !self.is_assignable(return_ty, &ty) {
                    return Err(self.mismatch_error(
                        format!(
                            "return type mismatch: expected {}, got {}",
                            return_ty.name(),
                            ty.name()
                        ),
                        return_ty,
                        &ty,
                    ));
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
                // Type guards: everything the condition proves when true
                // narrows the then-branch (null checks, `is` bindings, facts
                // composed through `&&`/`!`); the false-facts narrow the
                // else-branch, and remain in force after a terminating
                // then-branch (`if (x == null) { return; }`).
                let true_facts = self.condition_facts(cond, locals, true);
                let false_facts = self.condition_facts(cond, locals, false);
                self.with_facts(locals, &true_facts, |me, locals| {
                    for s in then_branch {
                        me.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                    Ok(())
                })?;
                if let Some(else_branch) = else_branch {
                    self.with_facts(locals, &false_facts, |me, locals| {
                        for s in else_branch {
                            me.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                        }
                        Ok(())
                    })?;
                }
                if else_branch.is_none() && Self::branch_terminates(then_branch) {
                    for fact in &false_facts {
                        if let GuardFact::NonNull { name, inner, declared } = fact {
                            locals.insert(name.clone(), inner.clone());
                            self.narrowed.borrow_mut().insert(name.clone(), declared.clone());
                        }
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
                // The condition's true-facts hold inside the body (null
                // checks, `is` bindings, `&&` compositions); assignments to
                // narrowed locals re-widen them (the condition re-narrows
                // each iteration).
                let body_facts = self.condition_facts(condition, locals, true);
                self.with_facts(locals, &body_facts, |me, locals| {
                    for s in body {
                        me.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                    Ok(())
                })?;
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
                let iter_ty = self.infer_expr(iterable, class, locals, in_instance, return_ty, generic_params)?;
                let effective_ty = match &iter_ty {
                    // Ranges iterate integers; `var` infers int.
                    Type::Class(name, _) if name == "Range" => {
                        if matches!(var_type, Type::Infer) {
                            Type::Int32
                        } else if var_type.is_int() {
                            var_type.clone()
                        } else {
                            return Err(TypeError(format!(
                                "for-in loop variable must be an integer, got {}",
                                var_type.name()
                            )));
                        }
                    }
                    // Collections iterate their element type; `var` infers it.
                    Type::Class(name, type_args) if name == "List" || name == "Set" => {
                        let elem_ty = type_args.first();
                        if matches!(var_type, Type::Infer) {
                            elem_ty.cloned().ok_or_else(|| {
                                TypeError(format!(
                                    "cannot infer the element type of this {}; annotate the loop variable",
                                    name
                                ))
                            })?
                        } else {
                            if let Some(elem_ty) = elem_ty {
                                if !self.is_assignable(var_type, elem_ty) {
                                    return Err(TypeError(format!(
                                        "for-in loop variable of type {} cannot hold {} elements of type {}",
                                        var_type.name(),
                                        name,
                                        elem_ty.name()
                                    )));
                                }
                            }
                            var_type.clone()
                        }
                    }
                    _ => {
                        return Err(TypeError(format!(
                            "for-in requires a range, List, or Set expression, got {}",
                            iter_ty.name()
                        )));
                    }
                };
                locals.insert(var_name.clone(), effective_ty);
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
            Stmt::Mark(line) => {
                self.current_line.set(*line);
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
                // The catch clauses cover checked-exception contracts for
                // calls inside the try body (only).
                self.handled_exceptions
                    .borrow_mut()
                    .push(catches.iter().map(|c| c.ty.clone()).collect());
                let try_result: Result<(), TypeError> = (|| {
                    for s in try_body {
                        self.check_stmt(s, class, locals, return_ty, in_instance, generic_params)?;
                    }
                    Ok(())
                })();
                self.handled_exceptions.borrow_mut().pop();
                try_result?;
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
            Expr::String(v) => Ok(Type::StringLit(v.clone())),
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
                        Err(TypeError(format!("unknown variable `{}`{}", name, Self::suggest(locals.keys().map(String::as_str), name))))
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
                    Err(TypeError(format!("unknown variable `{}`{}", name, Self::suggest(locals.keys().map(String::as_str), name))))
                }
            }
            Expr::Field(obj, name) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance, return_ty, generic_params)?;
                // Intrinsic string properties (`s.Length`). Literals and
                // literal-union values are strings at runtime.
                if matches!(obj_ty, Type::String | Type::StringLit(_) | Type::LiteralUnion(..)) {
                    if let Some(p) = crate::intrinsics::string_property(name) {
                        return Ok(p.ty);
                    }
                    return Err(TypeError(format!(
                        "unknown property `{}` on `string`",
                        name
                    )));
                }
                // Newtype unwrap: `id.Value` yields the underlying type.
                if let Type::Newtype(nt_name, underlying) = &obj_ty {
                    if name == "Value" {
                        return Ok((**underlying).clone());
                    }
                    return Err(TypeError(format!(
                        "unknown property `{}` on newtype `{}` (only `Value` is available)",
                        name, nt_name
                    )));
                }
                let (class_name, type_args) = if let Type::Class(name, args) = &obj_ty {
                    (name.clone(), args.clone())
                } else if in_instance && matches!(obj.as_ref(), Expr::Var(n) if n == "this") {
                    // For `this`, use the current class with empty type args
                    (class.name.clone(), vec![])
                } else {
                    if let Type::Nullable(_) = &obj_ty {
                        return Err(TypeError(format!(
                            "`{}` may be null (type {}): narrow with an `if` check, or use `?.` / `!`",
                            name,
                            obj_ty.name()
                        )));
                    }
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
                        TypeError(format!("unknown field `{}` on `{}`{}", name, class_name, Self::suggest(self.field_candidates(&class_name).iter().map(String::as_str), name)))
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
            Expr::Hole => Err(TypeError(
                "type hole: the expected type cannot be determined at this position; use `_` where a typed value is expected (an initializer, argument, or return value)"
                    .to_string(),
            )),
            Expr::Is(subject, ty, _binding) => {
                let subject_ty = self.infer_expr(subject, class, locals, in_instance, return_ty, generic_params)?;
                let subject_core = match &subject_ty {
                    Type::Nullable(inner) => (**inner).clone(),
                    t => t.clone(),
                };
                let Type::Class(tested_name, tested_args) = ty else {
                    return Err(TypeError(format!(
                        "`is` requires a class or interface type, got {}",
                        ty.name()
                    )));
                };
                if !tested_args.is_empty() {
                    return Err(TypeError(
                        "`is` cannot test generic type arguments (they are erased at runtime)".to_string(),
                    ));
                }
                if crate::intrinsics::is_intrinsic_class(tested_name) {
                    return Err(TypeError(format!(
                        "`is` cannot test stdlib type `{}`",
                        tested_name
                    )));
                }
                let Some(tested_info) = self.classes.get(tested_name) else {
                    return Err(TypeError(format!("unknown class `{}`", tested_name)));
                };
                let Type::Class(subj_name, _) = &subject_core else {
                    return Err(TypeError(format!(
                        "`is` requires a class-typed subject, got {}",
                        subject_ty.name()
                    )));
                };
                // Reject provably-impossible tests between unrelated concrete
                // classes (single inheritance makes them disjoint); interfaces
                // keep everything possible via structural satisfaction.
                let subj_is_interface = self
                    .classes
                    .get(subj_name)
                    .map(|i| i.is_interface)
                    .unwrap_or(false);
                if !tested_info.is_interface && !subj_is_interface {
                    let related = self.is_subclass_of(tested_name, subj_name)
                        || self.is_subclass_of(subj_name, tested_name);
                    if !related {
                        return Err(TypeError(format!(
                            "`is` test can never succeed: {} and {} are unrelated classes",
                            subject_ty.name(),
                            tested_name
                        )));
                    }
                }
                Ok(Type::Bool)
            }
            Expr::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                let lt = self.infer_expr(left, class, locals, in_instance, return_ty, generic_params)?;
                if lt != Type::Bool {
                    return Err(TypeError("logical operators require booleans".to_string()));
                }
                // The rhs is only reached when the lhs is true (`&&`) or
                // false (`||`), so its facts hold while typing the rhs:
                // `x != null && x.f > 0`, `x is Circle c && c.r > 0`.
                let facts = self.condition_facts(left, locals, matches!(op, BinOp::And));
                let mut rhs_locals = locals.clone();
                Self::apply_fact_types(&facts, &mut rhs_locals);
                let rt = self.infer_expr(right, class, &rhs_locals, in_instance, return_ty, generic_params)?;
                if rt != Type::Bool {
                    return Err(TypeError("logical operators require booleans".to_string()));
                }
                Ok(Type::Bool)
            }
            Expr::Binary(op, left, right) => {
                let lt = self.infer_expr(left, class, locals, in_instance, return_ty, generic_params)?;
                let rt = self.infer_expr(right, class, locals, in_instance, return_ty, generic_params)?;
                if op.is_comparison() {
                    let lt_is_null = matches!(&lt, Type::Class(n, _) if n == "null");
                    let rt_is_null = matches!(&rt, Type::Class(n, _) if n == "null");
                    if !lt_is_null && !rt_is_null {
                        // User-defined operators: the left class declares
                        // `operator==` / `operator<` etc.; `!=` is always the
                        // negation of `operator==`. A nullable left operand
                        // must be narrowed first — silently falling back to
                        // reference comparison would change meaning.
                        if let Some((param, _)) = self.user_operator(*op, &lt) {
                            if !self.is_assignable(&param, &rt) {
                                return Err(self.mismatch_error(
                                    format!(
                                        "cannot apply `{}`: right operand does not fit `{}`",
                                        binop_symbol(*op),
                                        lt.name()
                                    ),
                                    &param,
                                    &rt,
                                ));
                            }
                            return Ok(Type::Bool);
                        }
                        if let Type::Nullable(inner) = &lt {
                            if self.user_operator(*op, inner).is_some() {
                                return Err(TypeError(format!(
                                    "left operand of `{}` has type {} and may be null; \
                                     narrow it (`!= null`, `!`) before using the operator \
                                     overload",
                                    binop_symbol(*op),
                                    lt.name()
                                )));
                            }
                        }
                        // T and T? compare as their underlying type.
                        let lt_u = if let Type::Nullable(t) = &lt { (**t).clone() } else { lt.clone() };
                        let rt_u = if let Type::Nullable(t) = &rt { (**t).clone() } else { rt.clone() };
                        // Ordering a class requires an overload; only `==`/`!=`
                        // (reference identity) come built in. Type parameters
                        // order only under an all-numeric union constraint
                        // (`where T : int | float`), against another `T`.
                        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
                            if let Some(gp) = self.generic_param_of(&lt_u, generic_params) {
                                let rhs_same = self
                                    .generic_param_of(&rt_u, generic_params)
                                    .is_some_and(|r| r.name == gp.name);
                                if self.union_is_numeric(gp) && rhs_same {
                                    return Ok(Type::Bool);
                                }
                                return Err(TypeError(format!(
                                    "cannot order type parameter `{}` with `{}`: constrain \
                                     it with a numeric union (`where {} : int | float`) and \
                                     compare two `{}` values",
                                    gp.name,
                                    binop_symbol(*op),
                                    gp.name,
                                    gp.name
                                )));
                            }
                            if let Type::Class(n, _) = &lt_u {
                                if self.classes.contains_key(n) {
                                    return Err(TypeError(format!(
                                        "class `{}` does not define `operator{}`; ordering \
                                         comparisons on objects need an operator overload",
                                        n,
                                        binop_symbol(*op)
                                    )));
                                }
                            }
                        }
                        if !self.is_numeric(&lt_u) || !self.is_numeric(&rt_u) {
                            // String literals compare with strings, and with
                            // literal unions when the literal is a member.
                            let string_compat = |a: &Type, b: &Type| -> bool {
                                match (a, b) {
                                    (Type::String, Type::StringLit(_)) => true,
                                    (Type::StringLit(_), Type::StringLit(_)) => true,
                                    (Type::String, Type::LiteralUnion(..)) => true,
                                    (Type::LiteralUnion(_, m), Type::StringLit(v)) => m.contains(v),
                                    // Two unions compare when they can hold a
                                    // common member.
                                    (Type::LiteralUnion(_, a), Type::LiteralUnion(_, b)) => {
                                        a.iter().any(|m| b.contains(m))
                                    }
                                    _ => false,
                                }
                            };
                            if lt_u != rt_u
                                && !string_compat(&lt_u, &rt_u)
                                && !string_compat(&rt_u, &lt_u)
                            {
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
                    // User-defined arithmetic operators on the left class.
                    if let Some((param, ret)) = self.user_operator(*op, &lt) {
                        if !self.is_assignable(&param, &rt) {
                            return Err(self.mismatch_error(
                                format!(
                                    "cannot apply `{}`: right operand does not fit `{}`",
                                    binop_symbol(*op),
                                    lt.name()
                                ),
                                &param,
                                &rt,
                            ));
                        }
                        return Ok(ret);
                    }
                    if let Type::Nullable(inner) = &lt {
                        if self.user_operator(*op, inner).is_some() {
                            return Err(TypeError(format!(
                                "left operand of `{}` has type {} and may be null; narrow \
                                 it (`!= null`, `!`) before using the operator overload",
                                binop_symbol(*op),
                                lt.name()
                            )));
                        }
                    }
                    if let Type::Class(n, _) = &lt {
                        if n != "null" && self.classes.contains_key(n) {
                            return Err(TypeError(format!(
                                "class `{}` does not define `operator{}`",
                                n,
                                binop_symbol(*op)
                            )));
                        }
                    }
                    // Union-constrained type parameters: `where T : int |
                    // float` allows arithmetic between two `T` values (every
                    // alternative supports it; the runtime op is dynamic).
                    if let Some(gp) = self.generic_param_of(&lt, generic_params) {
                        let rhs_same = self
                            .generic_param_of(&rt, generic_params)
                            .is_some_and(|r| r.name == gp.name);
                        if self.union_is_numeric(gp) && rhs_same {
                            return Ok(lt.clone());
                        }
                        return Err(TypeError(format!(
                            "cannot apply `{}` to type parameter `{}`: constrain it with a \
                             numeric union (`where {} : int | float`) and use two `{}` \
                             operands",
                            binop_symbol(*op),
                            gp.name,
                            gp.name,
                            gp.name
                        )));
                    }
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
            Expr::Call(call) => self.check_call(call, class, locals, in_instance, return_ty, generic_params, false),
            Expr::Lambda { params, body } => self.check_lambda(
                params, body, None, class, locals, in_instance, generic_params,
            ),
            Expr::Await(inner) => {
                if !self.current_is_async.get() {
                    return Err(TypeError(
                        "`await` is only allowed inside `async` methods".to_string(),
                    ));
                }
                let t = self.infer_expr(inner, class, locals, in_instance, return_ty, generic_params)?;
                match &t {
                    Type::Class(n, args) if n == "Task" && args.len() == 1 => {
                        Ok(args[0].clone())
                    }
                    other => Err(TypeError(format!(
                        "`await` needs a Task<T>, got {}",
                        other.name()
                    ))),
                }
            }
            Expr::New(class_name, type_args, args) => {
                let Some(class_info) = self.classes.get(class_name) else {
                    if generic_params.iter().any(|gp| gp.name == *class_name) {
                        return Err(TypeError(format!(
                            "cannot construct type parameter `{}`: generics are erased at \
                             runtime, so `new {}()` is not supported yet (`new()` \
                             constraints are enforced at call sites)",
                            class_name, class_name
                        )));
                    }
                    return Err(TypeError(format!("unknown class `{}`", class_name)));
                };
                if class_info.is_static {
                    return Err(TypeError(format!(
                        "cannot instantiate static class `{}`",
                        class_name
                    )));
                }
                if class_info.is_abstract || class_info.is_interface {
                    return Err(TypeError(format!(
                        "cannot instantiate {} `{}`",
                        if class_info.is_interface { "interface" } else { "abstract class" },
                        class_name
                    )));
                }
                let arg_types: Vec<Type> = args
                    .iter()
                    .map(|a| self.infer_expr(a, class, locals, in_instance, return_ty, generic_params))
                    .collect::<Result<_, _>>()?;
                // Generic type inference at construction: `new Box(42)`
                // binds the class's type parameters by unifying constructor
                // parameters against the argument types, exactly like
                // generic-method call sites.
                let inferred: Vec<Type>;
                let type_args: &Vec<Type> =
                    if type_args.is_empty() && !class_info.generic_params.is_empty() {
                        inferred = self.infer_new_type_args(class_info, class_name, &arg_types)?;
                        &inferred
                    } else {
                        type_args
                    };
                self.validate_type_args(class_name, type_args, generic_params)?;
                for arg in type_args {
                    self.validate_type_with_generics(arg, generic_params)?;
                }
                let subst = build_subst(&class_info.generic_params, type_args);
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
                let then_ty = self.resolve_literal(&self.infer_expr(then_expr, class, locals, in_instance, return_ty, generic_params)?);
                let else_ty = self.resolve_literal(&self.infer_expr(else_expr, class, locals, in_instance, return_ty, generic_params)?);
                let then_null = matches!(&then_ty, Type::Class(n, _) if n == "null");
                let else_null = matches!(&else_ty, Type::Class(n, _) if n == "null");
                // A null branch makes the result nullable.
                if then_null || else_null {
                    let other = if then_null { &else_ty } else { &then_ty };
                    if then_null && else_null {
                        return Ok(then_ty);
                    }
                    return Ok(match other {
                        Type::Nullable(_) => other.clone(),
                        t => Type::Nullable(Box::new(t.clone())),
                    });
                }
                // Literal marker types widen to their carrier type so
                // branches with different literals unify.
                let then_ty = self.resolve_literal(&then_ty);
                let else_ty = self.resolve_literal(&else_ty);
                if !self.is_assignable(&then_ty, &else_ty) && !self.is_assignable(&else_ty, &then_ty) {
                    return Err(TypeError(format!(
                        "ternary branches must have compatible types, got {} and {}",
                        then_ty.name(),
                        else_ty.name()
                    )));
                }
                // T / T? branches join to T?.
                if matches!(&else_ty, Type::Nullable(_)) && !matches!(&then_ty, Type::Nullable(_)) {
                    return Ok(else_ty);
                }
                Ok(then_ty)
            }
            Expr::NonNullAssert(inner) => {
                let inner_ty = self.infer_expr(inner, class, locals, in_instance, return_ty, generic_params)?;
                Ok(match inner_ty {
                    Type::Nullable(t) => *t,
                    // Redundant assertions are allowed and are no-ops.
                    t => t,
                })
            }
            Expr::NullCoalesce(left, right) => {
                let left_ty = self.infer_expr(left, class, locals, in_instance, return_ty, generic_params)?;
                let right_ty = self.infer_expr(right, class, locals, in_instance, return_ty, generic_params)?;
                // `a ?? b`: a must be nullable; the result is a's underlying
                // type, nullable again only if b is nullable too.
                let inner = match &left_ty {
                    Type::Nullable(t) => (**t).clone(),
                    Type::Class(n, _) if n == "null" => right_ty.clone(),
                    _ => {
                        return Err(TypeError(format!(
                            "`??` requires a nullable left operand, got {}",
                            left_ty.name()
                        )));
                    }
                };
                if !self.is_assignable(&Type::Nullable(Box::new(inner.clone())), &right_ty) {
                    return Err(TypeError(format!(
                        "null coalescing operands must have compatible types, got {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    )));
                }
                if matches!(&right_ty, Type::Nullable(_)) {
                    Ok(Type::Nullable(Box::new(inner)))
                } else {
                    Ok(inner)
                }
            }
            Expr::NullConditionalField(obj, field_name) => {
                let obj_ty = self.infer_expr(obj, class, locals, in_instance, return_ty, generic_params)?;
                let obj_ty = match obj_ty {
                    Type::Nullable(inner) => *inner,
                    t => t,
                };
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
                        TypeError(format!("unknown field `{}` on `{}`{}", field_name, class_name, Self::suggest(self.field_candidates(&class_name).iter().map(String::as_str), field_name)))
                    })?;
                if !self.can_access(&class.name, &declared_in, visibility) {
                    return Err(TypeError(format!(
                        "field `{}` on `{}` is {}",
                        field_name, declared_in, visibility_name(visibility)
                    )));
                }
                // Result is the nullable version of the field type.
                Ok(match ty {
                    Type::Nullable(_) => ty,
                    t => Type::Nullable(Box::new(t)),
                })
            }
            Expr::NullConditionalCall(call) => {
                // Like a regular call, but the receiver may be nullable and
                // the result is nullable (null propagates through).
                let ret = self.check_call(call, class, locals, in_instance, return_ty, generic_params, true)?;
                Ok(match ret {
                    Type::Unit => Type::Unit,
                    Type::Nullable(_) => ret,
                    t => Type::Nullable(Box::new(t)),
                })
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
                    
                    // Type check body. Literal marker types (string/int/
                    // float literals) widen to their carrier type so arms
                    // with different literals unify.
                    let body_ty = {
                        let t = self.infer_expr(&arm.body, class, &arm_locals, in_instance, return_ty, generic_params)?;
                        self.resolve_literal(&t)
                    };
                    
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
            Expr::TryUnwrap(inner) => {
                let inner_ty = self.infer_expr(inner, class, locals, in_instance, return_ty, generic_params)?;
                let (enum_name, type_args) = match &inner_ty {
                    Type::Class(name, args) if self.enums.contains_key(name) => (name.clone(), args.clone()),
                    Type::Enum(name) => (name.clone(), Vec::new()),
                    _ => {
                        return Err(TypeError(format!(
                            "`?` requires a Result-like sum type, got `{}`",
                            inner_ty.name()
                        )));
                    }
                };
                // The enclosing function must return the same sum type so the
                // error variant can be propagated as-is.
                let return_name = match return_ty {
                    Type::Class(name, _) if self.enums.contains_key(name) => name.clone(),
                    Type::Enum(name) => name.clone(),
                    _ => {
                        return Err(TypeError(format!(
                            "`?` can only be used in a function returning a Result-like sum type, but it returns `{}`",
                            return_ty.name()
                        )));
                    }
                };
                if return_name != enum_name {
                    return Err(TypeError(format!(
                        "`?` propagates `{}` but the enclosing function returns `{}`",
                        enum_name, return_name
                    )));
                }
                // The first variant is the success case; unwrap its single field.
                let enum_info = self.enums.get(&enum_name).unwrap();
                let success = enum_info.variants.first().ok_or_else(|| {
                    TypeError(format!("sum type `{}` has no variants", enum_name))
                })?;
                if success.fields.len() != 1 {
                    return Err(TypeError(format!(
                        "cannot `?` unwrap `{}`: success variant `{}` must carry exactly one value",
                        enum_name, success.name
                    )));
                }
                let subst = build_subst(&enum_info.generic_params, &type_args);
                Ok(substitute_type(&success.fields[0].1, &subst))
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
            AssignTarget::Local(name) => locals.get(name).cloned().ok_or_else(|| {
                TypeError(format!(
                    "unknown variable `{}`{}",
                    name,
                    Self::suggest(locals.keys().map(String::as_str), name)
                ))
            }),
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
                        TypeError(format!("unknown field `{}` on `{}`{}", name, class_name, Self::suggest(self.field_candidates(&class_name).iter().map(String::as_str), name)))
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

    /// Try to resolve `call` as an extension method on the target key
    /// (`string` or a class name): checks the call's arguments against the
    /// extension method's parameters minus the leading `__self` receiver.
    /// `Ok(None)` when no extension defines the method.
    #[allow(clippy::too_many_arguments)]
    fn check_extension_call(
        &self,
        call: &CallExpr,
        key: &str,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
    ) -> Result<Option<Type>, TypeError> {
        let Some(ext_class) = self.extensions.get(key).and_then(|m| m.get(&call.method)) else {
            return Ok(None);
        };
        let info = self
            .classes
            .get(ext_class)
            .and_then(|c| c.static_methods.get(&call.method))
            .ok_or_else(|| {
                TypeError(format!(
                    "extension class `{}` lost method `{}`",
                    ext_class, call.method
                ))
            })?;
        let trimmed = MethodInfo {
            params: info.params[1..].to_vec(),
            ..info.clone()
        };
        self.require_throws_handled(&trimmed, &format!("{}.{}", ext_class, call.method))?;
        self.check_call_args(call, &trimmed, class, locals, in_instance, return_ty, generic_params)
            .map(Some)
    }

    fn check_call(
        &self,
        call: &CallExpr,
        class: &ClassInfo,
        locals: &HashMap<String, Type>,
        in_instance: bool,
        return_ty: &Type,
        generic_params: &[GenericParam],
        nullable_receiver: bool,
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
                    let resolved = self.find_method(&class.name, &call.method);
                    let (declared_in, method_info) = match resolved {
                        Some(r) => r,
                        None => {
                            let mut cur = Some(class.name.clone());
                            while let Some(k) = cur {
                                if let Some(ret) = self.check_extension_call(
                                    call, &k, class, locals, in_instance, return_ty,
                                    generic_params,
                                )? {
                                    return Ok(ret);
                                }
                                cur = self.classes.get(&k).and_then(|i| i.super_class.clone());
                            }
                            return Err(TypeError(format!(
                                "unknown method `{}` on `{}`{}",
                                call.method,
                                class.name,
                                Self::suggest(
                                    self.method_candidates(&class.name, false)
                                        .iter()
                                        .map(String::as_str),
                                    &call.method
                                )
                            )));
                        }
                    };
                    if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                        return Err(TypeError(format!(
                            "method `{}` on `{}` is {}",
                            call.method, declared_in, visibility_name(method_info.visibility)
                        )));
                    }
                    self.require_throws_handled(
                        &method_info,
                        &format!("{}.{}", declared_in, call.method),
                    )?;
                    return self.check_call_args(call, &method_info, class, locals, in_instance, return_ty, generic_params);
                }
                if self.classes.contains_key(class_name) {
                    let (declared_in, method_info) = self
                        .find_static_method(class_name, &call.method)
                        .ok_or_else(|| {
                            TypeError(format!(
                                "unknown static method `{}` on `{}`{}",
                                call.method,
                                class_name,
                                Self::suggest(
                                    self.method_candidates(class_name, true)
                                        .iter()
                                        .map(String::as_str),
                                    &call.method
                                )
                            ))
                        })?;
                    if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                        return Err(TypeError(format!(
                            "static method `{}` on `{}` is {}",
                            call.method, declared_in, visibility_name(method_info.visibility)
                        )));
                    }
                    self.require_throws_handled(
                        &method_info,
                        &format!("{}.{}", declared_in, call.method),
                    )?;
                    return self.check_call_args(call, &method_info, class, locals, in_instance, return_ty, generic_params);
                }
            }
            let target_ty = self.infer_expr(target, class, locals, in_instance, return_ty, generic_params)?;
            let target_ty = match target_ty {
                Type::Nullable(inner) if nullable_receiver => *inner,
                t => t,
            };
            // Intrinsic string methods (`s.Substring(...)`, `s.Split(...)`).
            // Literals and literal-union values are strings at runtime.
            if matches!(target_ty, Type::String | Type::StringLit(_) | Type::LiteralUnion(..)) {
                let Some(im) = crate::intrinsics::string_method(&call.method) else {
                    if let Some(ret) = self.check_extension_call(
                        call, "string", class, locals, in_instance, return_ty, generic_params,
                    )? {
                        return Ok(ret);
                    }
                    return Err(TypeError(format!(
                        "unknown method `{}` on `string`",
                        call.method
                    )));
                };
                let info = MethodInfo {
                    name: im.name.to_string(),
                    generic_params: Vec::new(),
                    return_ty: im.return_ty.clone(),
                    params: im.params.clone(),
                    is_instance: true,
                    visibility: Visibility::Public,
                    is_virtual: false,
                    is_override: false,
                    is_abstract: false,
                    is_final: true,
                    throws: Vec::new(),
                    is_async: false,
                };
                return self.check_call_args(call, &info, class, locals, in_instance, return_ty, generic_params);
            }
            let (name, type_args) = if let Type::Class(name, args) = &target_ty {
                // Bounded polymorphism: a receiver typed as a generic
                // parameter exposes its constraint's members
                // (`static <T : Sized> ... x.Size()`); unconstrained
                // parameters have no callable members.
                if args.is_empty() {
                    if let Some(gp) = generic_params.iter().find(|gp| gp.name == *name) {
                        // With several bounds, the first one declaring the
                        // method answers the call.
                        let hit = gp.bounds.iter().find_map(|b| match b {
                            Type::Class(cn, cargs)
                                if self.find_method(cn, &call.method).is_some() =>
                            {
                                Some((cn.clone(), cargs.clone()))
                            }
                            _ => None,
                        });
                        match hit {
                            Some(h) => h,
                            None => {
                                return Err(TypeError(format!(
                                    "cannot call `{}` on type parameter `{}`: no bound declares it (bounds: {})",
                                    call.method,
                                    name,
                                    if gp.bounds.is_empty() {
                                        "none".to_string()
                                    } else {
                                        gp.bounds
                                            .iter()
                                            .map(|b| b.name())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    }
                                )));
                            }
                        }
                    } else {
                        (name.clone(), args.clone())
                    }
                } else {
                    (name.clone(), args.clone())
                }
            } else if in_instance && matches!(target.as_ref(), Expr::Var(n) if n == "this") {
                // For `this`, use the current class with empty type args
                // (we're inside the generic class definition)
                (class.name.clone(), vec![])
            } else {
                if let Type::Nullable(_) = &target_ty {
                    return Err(TypeError(format!(
                        "receiver of `{}` may be null (type {}): narrow with an `if` check, or use `?.` / `!`",
                        call.method,
                        target_ty.name()
                    )));
                }
                return Err(TypeError(format!(
                    "cannot call method on non-class type {}",
                    target_ty.name()
                )));
            };
            if !self.classes.contains_key(&name) {
                return Err(TypeError(format!("unknown class `{}`", name)));
            }
            let resolved = self.find_method(&name, &call.method);
            let (declared_in, method_info) = match resolved {
                Some(r) => r,
                None => {
                    // Extension fallback: the receiver's class, then its
                    // superclass chain (a real method always wins — checked
                    // above by `find_method`).
                    let mut cur = Some(name.clone());
                    while let Some(k) = cur {
                        if let Some(ret) = self.check_extension_call(
                            call, &k, class, locals, in_instance, return_ty, generic_params,
                        )? {
                            return Ok(ret);
                        }
                        cur = self.classes.get(&k).and_then(|i| i.super_class.clone());
                    }
                    return Err(TypeError(format!(
                        "unknown method `{}` on `{}`{}",
                        call.method,
                        name,
                        Self::suggest(
                            self.method_candidates(&name, false)
                                .iter()
                                .map(String::as_str),
                            &call.method
                        )
                    )));
                }
            };
            if !self.can_access(&class.name, &declared_in, method_info.visibility) {
                return Err(TypeError(format!(
                    "method `{}` on `{}` is {}",
                    call.method, declared_in, visibility_name(method_info.visibility)
                )));
            }

            // Build substitution map from class generic params and type args
            let target_class = self.classes.get(&name).unwrap();
            let subst = build_subst(&target_class.generic_params, &type_args);
            
            self.require_throws_handled(
                &method_info,
                &format!("{}.{}", declared_in, call.method),
            )?;
            // Check arguments with substituted types
            return self.check_call_args_with_subst(call, &method_info, &subst, class, locals, in_instance, return_ty, generic_params);
        } else {
            // static call: ClassName.Method(args) or EnumName.Variant(args)
            let class_name = call.class_or_target.clone();

            // Calling a function-typed local: `f(2)`. Locals shadow class
            // and method names.
            match locals.get(&class_name) {
                Some(Type::Func(params, ret)) => {
                    let (params, ret) = (params.clone(), (**ret).clone());
                    if call.args.len() != params.len() {
                        return Err(TypeError(format!(
                            "`{}` takes {} argument(s), got {}",
                            class_name,
                            params.len(),
                            call.args.len()
                        )));
                    }
                    for (arg, pty) in call.args.iter().zip(&params) {
                        if matches!(arg, Expr::Hole) {
                            return Err(self.hole_error(pty, locals));
                        }
                        let at = self.infer_expr_expecting(
                            arg, Some(pty), class, locals, in_instance, return_ty,
                            generic_params,
                        )?;
                        if !self.is_assignable(pty, &at) {
                            return Err(self.mismatch_error(
                                format!(
                                    "argument of type {} does not fit parameter of type {} \
                                     in call to `{}`",
                                    at.name(),
                                    pty.name(),
                                    class_name
                                ),
                                pty,
                                &at,
                            ));
                        }
                    }
                    return Ok(ret);
                }
                Some(Type::Nullable(inner)) if matches!(inner.as_ref(), Type::Func(..)) => {
                    return Err(TypeError(format!(
                        "`{}` may be null (type {}): narrow it before calling",
                        class_name,
                        locals.get(&class_name).unwrap().name()
                    )));
                }
                _ => {}
            }

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
                // Newtype constructor: `UserId(expr)` wraps a value of the
                // underlying type. Purely a compile-time conversion.
                if let Some(underlying) = self.newtypes.get(&call.method).cloned() {
                    if call.args.len() != 1 {
                        return Err(TypeError(format!(
                            "newtype constructor `{}` expects 1 argument, got {}",
                            call.method,
                            call.args.len()
                        )));
                    }
                    let arg_ty = self.infer_expr(&call.args[0], class, locals, in_instance, return_ty, generic_params)?;
                    if !self.is_assignable(&underlying, &arg_ty) {
                        return Err(TypeError(format!(
                            "cannot construct {}({}): expected {}",
                            call.method,
                            arg_ty.name(),
                            underlying.name()
                        )));
                    }
                    return Ok(Type::Newtype(call.method.clone(), Box::new(underlying)));
                }
                // Bare static method call: `safeDivide(a, b)` resolves to a
                // static method of the current class.
                if let Some((declared_in, method_info)) = self.find_static_method(&class.name, &call.method) {
                    if self.can_access(&class.name, &declared_in, method_info.visibility) {
                        self.require_throws_handled(
                            &method_info,
                            &format!("{}.{}", declared_in, call.method),
                        )?;
                        return self.check_call_args(call, &method_info, class, locals, in_instance, return_ty, generic_params);
                    }
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
            self.require_throws_handled(
                &method_info,
                &format!("{}.{}", declared_in, call.method),
            )?;
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
        self.check_call_args_with_subst(
            call,
            method_info,
            &TypeSubst::new(),
            class,
            locals,
            in_instance,
            return_ty,
            generic_params,
        )
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
        if call.args.len() != method_info.params.len() {
            return Err(TypeError(format!(
                "method `{}` expects {} arguments, got {}",
                call.method,
                method_info.params.len(),
                call.args.len()
            )));
        }
        for (arg, param) in call.args.iter().zip(method_info.params.iter()) {
            if matches!(arg, Expr::Hole) {
                return Err(self.hole_error(&substitute_type(param, subst), locals));
            }
        }
        // Two-pass argument typing: non-lambda arguments first, whose types
        // partially bind the method's type parameters; lambda arguments are
        // then target-typed by the partially substituted parameter — their
        // own parameter types must be resolved, while an open return type is
        // determined by the body (`transform(21, x => x * 2)` binds T1 from
        // 21, then T2 from the body).
        let method_vars: HashSet<String> = method_info
            .generic_params
            .iter()
            .map(|gp| gp.name.clone())
            .collect();
        let mut arg_slots: Vec<Option<Type>> = Vec::with_capacity(call.args.len());
        for arg in &call.args {
            if matches!(arg, Expr::Lambda { .. }) {
                arg_slots.push(None);
            } else {
                arg_slots.push(Some(self.infer_expr(
                    arg, class, locals, in_instance, return_ty, generic_params,
                )?));
            }
        }
        let mut pre = subst.clone();
        if !method_vars.is_empty() && arg_slots.iter().any(|s| s.is_none()) {
            let mut pre_bindings: HashMap<String, Type> = HashMap::new();
            for (param, aty) in method_info.params.iter().zip(&arg_slots) {
                if let Some(aty) = aty {
                    let p = substitute_type(param, subst);
                    unify_generic_types(
                        &p,
                        &self.resolve_literal(aty),
                        &method_vars,
                        &mut pre_bindings,
                        &mut |_, prev, _| Some(prev),
                    );
                }
            }
            for (k, v) in pre_bindings {
                pre.insert(k, v);
            }
        }
        for (i, arg) in call.args.iter().enumerate() {
            if arg_slots[i].is_some() {
                continue;
            }
            let Expr::Lambda { params: lparams, body: lbody } = arg else {
                unreachable!()
            };
            let pty = method_info.params.get(i).map(|p| substitute_type(p, &pre));
            let params_resolved = matches!(&pty, Some(Type::Func(fp, _))
                if !fp.iter().any(|t| method_vars.iter().any(|v| type_mentions(t, v))));
            let ty = if params_resolved {
                self.check_lambda_open(
                    lparams,
                    lbody,
                    pty.as_ref(),
                    Some(&method_vars),
                    class,
                    locals,
                    in_instance,
                    generic_params,
                )?
            } else {
                self.infer_expr_expecting(
                    arg, None, class, locals, in_instance, return_ty, generic_params,
                )?
            };
            arg_slots[i] = Some(ty);
        }
        let arg_tys: Vec<Type> = arg_slots.into_iter().map(Option::unwrap).collect();

        // Method-level generics are inferred from the arguments by unifying
        // each declared parameter type (after the receiver's class-level
        // substitution) against the argument's type. When one variable is
        // bound by several arguments, the bindings join through
        // assignability — the more general type wins — and irreconcilable
        // bindings are an error.
        let mut subst = subst.clone();
        if !method_info.generic_params.is_empty() {
            let vars: HashSet<String> = method_info
                .generic_params
                .iter()
                .map(|gp| gp.name.clone())
                .collect();
            let mut bindings: HashMap<String, Type> = HashMap::new();
            let mut conflict: Option<(String, Type, Type)> = None;
            for (param, arg_ty) in method_info.params.iter().zip(arg_tys.iter()) {
                let p = substitute_type(param, &subst);
                unify_generic_types(&p, &self.resolve_literal(arg_ty), &vars, &mut bindings, &mut |name, prev, new| {
                    if self.is_assignable(&prev, &new) {
                        Some(prev)
                    } else if self.is_assignable(&new, &prev) {
                        Some(new)
                    } else {
                        if conflict.is_none() {
                            conflict = Some((name.to_string(), prev.clone(), new.clone()));
                        }
                        None
                    }
                });
            }
            if let Some((name, a, b)) = conflict {
                return Err(TypeError(format!(
                    "cannot infer `{}` in `{}`: conflicting argument types {} and {}",
                    name,
                    call.method,
                    a.name(),
                    b.name()
                )));
            }
            for gp in &method_info.generic_params {
                let Some(bound) = bindings.get(&gp.name) else {
                    return Err(TypeError(format!(
                        "cannot infer `{}` in `{}` from the arguments",
                        gp.name, call.method
                    )));
                };
                for constraint in &gp.bounds {
                    if !self.is_assignable(constraint, bound) {
                        return Err(TypeError(format!(
                            "inferred type {} for `{}` in `{}` does not satisfy constraint {}",
                            bound.name(),
                            gp.name,
                            call.method,
                            constraint.name()
                        )));
                    }
                }
                if !gp.union.is_empty()
                    && !gp.union.iter().any(|alt| self.is_assignable(alt, bound))
                {
                    return Err(TypeError(format!(
                        "inferred type {} for `{}` in `{}` must be one of {}",
                        bound.name(),
                        gp.name,
                        call.method,
                        gp.union
                            .iter()
                            .map(|t| t.name())
                            .collect::<Vec<_>>()
                            .join(" | ")
                    )));
                }
                if gp.requires_new {
                    self.require_default_constructible(bound, &gp.name, &call.method)?;
                }
                subst.insert(gp.name.clone(), bound.clone());
            }
        }

        let substituted_params: Vec<Type> = method_info
            .params
            .iter()
            .map(|p| substitute_type(p, &subst))
            .collect();
        for (arg_ty, expected) in arg_tys.iter().zip(substituted_params.iter()) {
            if !self.is_assignable(expected, arg_ty) {
                return Err(self.mismatch_error(
                    format!(
                        "argument type mismatch in `{}`: expected {}, got {}",
                        call.method,
                        expected.name(),
                        arg_ty.name()
                    ),
                    expected,
                    arg_ty,
                ));
            }
        }
        Ok(substitute_type(&method_info.return_ty, &subst))
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
            Type::StringLit(_) => Type::String,
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
        // Nullability (strict): `null` and `T?` are only assignable to
        // nullable targets; `T` widens into `T?`.
        match (target, source) {
            (Type::Nullable(t), Type::Nullable(s)) => return self.is_assignable(t, s),
            (Type::Nullable(_), Type::Class(sn, _)) if sn == "null" => return true,
            (Type::Nullable(t), s) => return self.is_assignable(t, s),
            // Unbound generic parameters stay permissive (unified later).
            (Type::GenericParam(_), Type::Nullable(_)) => return true,
            (_, Type::Nullable(_)) => return false,
            _ => {}
        }
        // Untyped literals coerce to any numeric type whose range fits.
        match (target, source) {
            (target, Type::IntLit(v)) => {
                return self.int_target_fits(target, *v);
            }
            (Type::Float32 | Type::Float64, Type::FloatLit(_)) => return true,
            // A string literal is a `string`, and satisfies a literal union
            // exactly when it is one of the declared members.
            (Type::String, Type::StringLit(_)) => return true,
            (Type::LiteralUnion(_, members), Type::StringLit(v)) => {
                return members.contains(v);
            }
            // Widening: a literal-union value is always a valid string.
            (Type::String, Type::LiteralUnion(..)) => return true,
            // Subset widening: a union flows into another union exactly when
            // every member it can hold is a member of the target (the rule
            // that makes composed unions like `type Extended = Direction |
            // "up";` accept Direction values).
            (Type::LiteralUnion(_, target_members), Type::LiteralUnion(_, source_members)) => {
                return source_members.iter().all(|m| target_members.contains(m));
            }
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
                // inferred `Result<int, T>`. Variance annotations steer the
                // per-argument direction: `out` compares target←source
                // (covariant, also the pre-existing permissive default for
                // unannotated parameters), `in` compares source←target.
                if target_name == source_name
                    && !target_args.is_empty()
                    && !source_args.is_empty()
                {
                    if target_args.len() != source_args.len() {
                        return false;
                    }
                    let variances: Vec<Variance> = self
                        .classes
                        .get(target_name)
                        .map(|i| i.generic_params.iter().map(|g| g.variance).collect())
                        .unwrap_or_default();
                    return target_args.iter().zip(source_args).enumerate().all(
                        |(i, (t, s))| match variances.get(i).copied() {
                            Some(Variance::Contravariant) => self.is_assignable(s, t),
                            _ => self.is_assignable(t, s),
                        },
                    );
                }
                // Generic interface target: the source must implement it at
                // a declared instantiation, compared variance-aware — `out`
                // widens (IProducer<Cat> into IProducer<Animal>), `in`
                // narrows (IComparer<Animal> into IComparer<Cat>),
                // unannotated parameters must match exactly. The name-only
                // subclass walk below would ignore the arguments.
                if !target_args.is_empty()
                    && self
                        .classes
                        .get(target_name)
                        .map(|i| i.is_interface && !i.generic_params.is_empty())
                        .unwrap_or(false)
                {
                    let Some(sargs) =
                        self.declared_interface_args(source_name, source_args, target_name)
                    else {
                        return false;
                    };
                    if sargs.len() != target_args.len() {
                        return false;
                    }
                    let variances: Vec<Variance> = self
                        .classes
                        .get(target_name)
                        .map(|i| i.generic_params.iter().map(|g| g.variance).collect())
                        .unwrap_or_default();
                    return target_args.iter().zip(&sargs).enumerate().all(
                        |(i, (t, s))| match variances.get(i).copied() {
                            Some(Variance::Covariant) => self.is_assignable(t, s),
                            Some(Variance::Contravariant) => self.is_assignable(s, t),
                            _ => t == s,
                        },
                    );
                }
                // Strict nullability: null is NOT assignable to a plain class
                // reference — only to `T?` (handled above).
                // Upcast: a subclass instance is assignable to its base type.
                self.is_subclass_of(source_name, target_name)
                    || structurally_satisfies(&self.classes, source_name, target_name)
            }
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
                Err(TypeError(format!(
                    "unknown type `{}`{}",
                    name,
                    Self::suggest(self.type_candidates(), name)
                )))
            }
            _ => Ok(()),
        }
    }

    /// Every nameable type, for did-you-mean suggestions.
    fn type_candidates(&self) -> impl Iterator<Item = &str> {
        self.classes
            .keys()
            .map(String::as_str)
            .chain(self.enums.keys().map(String::as_str))
            .chain(self.newtypes.keys().map(String::as_str))
    }

    /// Check a generic class reference's type arguments: exact arity (raw
    /// references to generic classes are rejected — required for phantom
    /// type parameters to be un-dodgeable) and declared constraints
    /// (`<T : IFace>` accepts only arguments assignable to the constraint;
    /// arguments that are themselves type parameters are accepted without
    /// constraint propagation — a documented v1 limit).
    fn validate_type_args(
        &self,
        name: &str,
        args: &[Type],
        generic_params: &[GenericParam],
    ) -> Result<(), TypeError> {
        let Some(info) = self.classes.get(name) else {
            return Ok(());
        };
        if info.generic_params.len() != args.len() {
            return Err(TypeError(format!(
                "class `{}` expects {} type argument(s), got {}",
                name,
                info.generic_params.len(),
                args.len()
            )));
        }
        for (param, arg) in info.generic_params.iter().zip(args.iter()) {
            // An argument that is itself an enclosing type parameter cannot
            // be checked here; its own use sites are.
            let is_param = matches!(arg, Type::GenericParam(_))
                || matches!(arg, Type::Class(n, a) if a.is_empty()
                    && generic_params.iter().any(|gp| gp.name == *n));
            if is_param {
                continue;
            }
            for constraint in &param.bounds {
                if !self.is_assignable(constraint, arg) {
                    return Err(TypeError(format!(
                        "type argument {} for parameter `{}` of `{}` does not satisfy constraint {}",
                        arg.name(),
                        param.name,
                        name,
                        constraint.name()
                    )));
                }
            }
            if !param.union.is_empty()
                && !param.union.iter().any(|alt| self.is_assignable(alt, arg))
            {
                return Err(TypeError(format!(
                    "type argument {} for parameter `{}` of `{}` must be one of {}",
                    arg.name(),
                    param.name,
                    name,
                    param
                        .union
                        .iter()
                        .map(|t| t.name())
                        .collect::<Vec<_>>()
                        .join(" | ")
                )));
            }
            if param.requires_new {
                self.require_default_constructible(arg, &param.name, name)?;
            }
        }
        Ok(())
    }

    fn validate_type_with_generics(&self, ty: &Type, generic_params: &[GenericParam]) -> Result<(), TypeError> {
        match ty {
            Type::Class(name, args) => {
                if generic_params.iter().any(|gp| gp.name == *name) {
                    return Ok(());
                }
                if !self.classes.contains_key(name) && !self.enums.contains_key(name) {
                    return Err(TypeError(format!(
                        "unknown type `{}`{}",
                        name,
                        Self::suggest(self.type_candidates(), name)
                    )));
                }
                if self.classes.get(name).map(|i| i.is_static).unwrap_or(false) {
                    return Err(TypeError(format!(
                        "static class `{}` cannot be used as a type",
                        name
                    )));
                }
                self.validate_type_args(name, args, generic_params)?;
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
            Type::Func(params, ret) => {
                for p in params {
                    self.validate_type_with_generics(p, generic_params)?;
                }
                self.validate_type_with_generics(ret, generic_params)
            }
            Type::Nullable(inner) | Type::Newtype(_, inner) => {
                self.validate_type_with_generics(inner, generic_params)
            }
            Type::Tuple(ts) => {
                for t in ts {
                    self.validate_type_with_generics(t, generic_params)?;
                }
                Ok(())
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
