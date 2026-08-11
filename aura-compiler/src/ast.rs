//! Abstract syntax tree for Aura.

/// A complete source file.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// Top-level declarations.
    pub decls: Vec<Decl>,
}

/// Top-level declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    /// Class declaration.
    Class(ClassDecl),
    /// Enum declaration.
    Enum(EnumDecl),
}

/// Enum declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

/// A single variant in an enum declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<EnumVariantField>,
}

/// A field in an enum variant.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantField {
    pub ty: Type,
    pub name: String,
}

/// Class declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    /// Class name.
    pub name: String,
    /// Generic parameters (e.g., `<T>`, `<K, V>`).
    pub generic_params: Vec<GenericParam>,
    /// Names after `:` (base class and/or interfaces). The type checker splits
    /// these into a single super class and a list of implemented interfaces.
    pub bases: Vec<String>,
    /// Whether this is an interface declaration.
    pub is_interface: bool,
    /// Whether this is an abstract class (cannot be instantiated).
    pub is_abstract: bool,
    /// Whether this is a sealed class (cannot be subclassed).
    pub is_sealed: bool,
    /// Member declarations.
    pub members: Vec<Member>,
}

/// Member visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Accessible from anywhere (default).
    Public,
    /// Accessible from the declaring class and its subclasses.
    Protected,
    /// Accessible only from within the declaring class.
    Private,
}

/// Generic parameter declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    /// Parameter name (e.g., "T", "K", "V").
    pub name: String,
    /// Optional constraint (interface/class that the type must implement/extend).
    pub constraint: Option<Type>,
}

/// Class member.
#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    /// Field declaration.
    Field(FieldDecl),
    /// Method declaration.
    Method(MethodDecl),
}

/// Field declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    /// Whether the field is static.
    pub is_static: bool,
    /// Member visibility.
    pub visibility: Visibility,
    /// Field type.
    pub ty: Type,
    /// Field name.
    pub name: String,
}

/// Method declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    /// Whether the method is static.
    pub is_static: bool,
    /// Member visibility.
    pub visibility: Visibility,
    /// Whether this method can be overridden by a subclass.
    pub is_virtual: bool,
    /// Whether this method overrides a base class method.
    pub is_override: bool,
    /// Whether this method is abstract (declared without a body).
    pub is_abstract: bool,
    /// Whether this method is final (cannot be overridden or re-declared).
    pub is_final: bool,
    /// Generic parameters for this method.
    pub generic_params: Vec<GenericParam>,
    /// Return type.
    pub return_ty: Type,
    /// Method name.
    pub name: String,
    /// Parameters.
    pub params: Vec<Param>,
    /// Body statements.
    pub body: Vec<Stmt>,
}

/// Parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// Parameter type.
    pub ty: Type,
    /// Parameter name.
    pub name: String,
}

/// Type annotation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    /// Unit / void.
    Unit,
    /// 32-bit integer.
    Int,
    /// 64-bit float.
    Float,
    /// Boolean.
    Bool,
    /// String.
    String,
    /// Named class type with optional type arguments.
    Class(String, Vec<Type>),
    /// Named enum type.
    Enum(String),
    /// Generic type parameter reference (e.g., `T`).
    GenericParam(String),
    /// Tuple type (e.g., `(int, string)`).
    Tuple(Vec<Type>),
}

impl Type {
    /// Textual representation used in diagnostics.
    pub fn name(&self) -> String {
        match self {
            Type::Unit => "void".to_string(),
            Type::Int => "int".to_string(),
            Type::Float => "float".to_string(),
            Type::Bool => "bool".to_string(),
            Type::String => "string".to_string(),
            Type::Class(name, args) => {
                if args.is_empty() {
                    name.clone()
                } else {
                    format!("{}<{}>", name, args.iter().map(|t| t.name()).collect::<Vec<_>>().join(", "))
                }
            }
            Type::Enum(name) => name.clone(),
            Type::GenericParam(name) => name.clone(),
            Type::Tuple(types) => {
                format!("({})", types.iter().map(|t| t.name()).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// Statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Variable declaration with optional initializer.
    VarDecl(Type, String, Option<Expr>),
    /// Tuple destructuring declaration: `let (x, y) = point;`
    TupleDecl(Vec<String>, Expr),
    /// Expression statement.
    Expr(Expr),
    /// Assignment to a variable or field.
    Assign(AssignTarget, Expr),
    /// Return statement.
    Return(Option<Expr>),
    /// If statement with optional else.
    If(Expr, Vec<Stmt>, Option<Vec<Stmt>>),
    /// While loop with optional label.
    While {
        label: Option<String>,
        condition: Expr,
        body: Vec<Stmt>,
    },
    /// For loop: for (init; cond; update) { body } with optional label.
    For {
        label: Option<String>,
        init: Box<Stmt>,
        condition: Expr,
        update: Box<Stmt>,
        body: Vec<Stmt>,
    },
    /// For-in loop: for (Type var in range) { body } with optional label.
    ForIn {
        label: Option<String>,
        var_type: Type,
        var_name: String,
        iterable: Expr,
        body: Vec<Stmt>,
    },
    /// Do-while loop: do { body } while (cond); with optional label.
    DoWhile {
        label: Option<String>,
        body: Vec<Stmt>,
        condition: Expr,
    },
    /// Break statement with optional label.
    Break(Option<String>),
    /// Continue statement with optional label.
    Continue(Option<String>),
    /// Block of statements.
    Block(Vec<Stmt>),
    /// Throw an exception: `throw expr;`.
    Throw(Expr),
    /// Try/catch/finally statement.
    Try {
        /// Statements protected by the exception handlers.
        try_body: Vec<Stmt>,
        /// Catch clauses in source order.
        catches: Vec<CatchClause>,
        /// Optional finally body.
        finally_body: Option<Vec<Stmt>>,
    },
    /// Resource acquisition statement: `using (Type name = expr) { body }` or
    /// `using (expr) { body }`. Disposes the resource (calls `Dispose`) on both
    /// normal exit and when an exception propagates.
    Using {
        /// Declared resource type, if `using (Type name = expr)`.
        resource_ty: Option<Type>,
        /// Resource variable name, if `using (Type name = expr)`.
        name: Option<String>,
        /// Expression producing the resource.
        expr: Box<Expr>,
        /// Body guarded by the resource.
        body: Vec<Stmt>,
    },
}

/// A single `catch (Type name) { body }` clause.
#[derive(Debug, Clone, PartialEq)]
pub struct CatchClause {
    /// Type of exception this clause catches.
    pub ty: Type,
    /// Variable bound to the caught exception.
    pub name: String,
    /// Handler body.
    pub body: Vec<Stmt>,
}

/// Assignment target.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    /// Local variable.
    Local(String),
    /// Field access on an expression.
    Field(Box<Expr>, String),
    /// Static field access.
    StaticField(String, String),
    /// Base class field access from a subclass method.
    SuperField(String),
}

/// Expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Integer literal.
    Int(i32),
    /// Float literal.
    Float(f64),
    /// Boolean literal.
    Bool(bool),
    /// String literal.
    String(String),
    /// Interpolated string: `"Hello {name}!"`.
    InterpolatedString(Vec<InterpPart>),
    /// Null literal.
    Null,
    /// Local variable.
    Var(String),
    /// Binary operation.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Unary operation.
    Unary(UnaryOp, Box<Expr>),
    /// Ternary conditional: cond ? then_expr : else_expr
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Method call.
    Call(CallExpr),
    /// Field access.
    Field(Box<Expr>, String),
    /// Static field access.
    StaticField(String, String),
    /// `new ClassName()` with optional type arguments.
    New(String, Vec<Type>),
    /// Match expression.
    Match(Box<Expr>, Vec<MatchArm>),
    /// Enum variant access or construction: `EnumName.VariantName` or `EnumName.VariantName(args)`.
    EnumVariant(String, String, Vec<Expr>),
    /// Tuple literal: `(1, 2, 3)`.
    Tuple(Vec<Expr>),
    /// Tuple index access: `tuple.0`, `tuple.1`.
    TupleIndex(Box<Expr>, usize),
    /// Range expression: `start..end` (exclusive) or `start..=end` (inclusive).
    Range(Box<Expr>, Box<Expr>, bool),
    /// Null coalescing: `a ?? b` returns `a` if not null, else `b`.
    NullCoalesce(Box<Expr>, Box<Expr>),
    /// Null conditional field access: `a?.field` returns null if `a` is null.
    NullConditionalField(Box<Expr>, String),
    /// Null conditional method call: `a?.method()` returns null if `a` is null.
    NullConditionalCall(CallExpr),
    /// Base class method call from a subclass method: `super.Method(args)`.
    SuperCall(String, Vec<Expr>),
    /// Base class field access from a subclass method: `super.field`.
    SuperField(String),
}

/// A part of an interpolated string.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    /// Literal string part.
    Literal(String),
    /// Expression to be interpolated.
    Expr(Expr),
}

/// A single arm in a match expression.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// Patterns to match against.
    pub patterns: Vec<Pattern>,
    /// Optional guard condition.
    pub guard: Option<Expr>,
    /// Body expression to evaluate if matched.
    pub body: Expr,
}

/// Pattern in a match arm.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Integer literal pattern.
    Int(i32),
    /// Float literal pattern.
    Float(f64),
    /// Boolean literal pattern.
    Bool(bool),
    /// String literal pattern.
    StringLit(String),
    /// Null pattern.
    Null,
    /// Wildcard pattern (matches anything).
    Wildcard,
    /// Enum variant pattern: `Color.Red` or `Some(value)`.
    EnumVariant(String, String, Vec<String>),
    /// Binding pattern: bind matched value to a name.
    Binding(String),
    /// Range pattern: `1..=5` or `1..5`.
    Range(Box<Expr>, Box<Expr>, bool),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    /// True if this operator produces a boolean result.
    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Call expression.
#[derive(Debug, Clone, PartialEq)]
pub struct CallExpr {
    /// Optional target expression for instance calls; None for static calls.
    pub target: Option<Box<Expr>>,
    /// Class or instance target name (for static calls this is the class).
    pub class_or_target: String,
    /// Method name.
    pub method: String,
    /// Type arguments for generic methods.
    pub type_args: Vec<Type>,
    /// Arguments.
    pub args: Vec<Expr>,
}
