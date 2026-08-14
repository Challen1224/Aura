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
    /// Type alias declaration (e.g., `type UserId = int;`).
    TypeAlias(TypeAliasDecl),
    /// Newtype declaration.
    Newtype(NewtypeDecl),
}

/// Type alias declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasDecl {
    /// Alias name (e.g., `UserId` in `type UserId = int;`).
    pub name: String,
    /// Generic parameters (e.g., `<T>` in `type Result2<T> = Result<T, string>;`).
    pub generic_params: Vec<GenericParam>,
    /// The type this alias refers to.
    pub target: Type,
}

/// Newtype declaration: `newtype UserId = int;` — a distinct nominal type
/// over a primitive. Unlike a type alias it does not interconvert with its
/// underlying type; construction is `UserId(expr)` and unwrapping is
/// `value.Value`. Fully erased at runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct NewtypeDecl {
    /// Newtype name.
    pub name: String,
    /// The underlying primitive type.
    pub underlying: Type,
}

/// Enum declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    /// Generic parameters (e.g., `<T, E>`). Sum types declared with
    /// `type Result<T, E> = ...` carry their type parameters here.
    pub generic_params: Vec<GenericParam>,
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
    /// Whether this is a record class (value semantics, immutable data class).
    pub is_record: bool,
    /// Primary constructor parameters of a record (synthesized into read-only
    /// positional properties). Empty for regular classes.
    pub record_params: Vec<Param>,
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
    /// Accessible from anywhere in the same module.
    Internal,
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
    /// Property with optional getter/setter accessors.
    Property(PropertyDecl),
}

/// A property declaration: `int Count { get { ... } set { ... } }`.
///
/// An accessor is either `Auto` (written as `get;` / `set;`) which is backed
/// by a compiler-generated field, or `Body` with explicit statements. The
/// setter body refers to the assigned value as `value`.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyDecl {
    /// Whether the property is static.
    pub is_static: bool,
    /// Member visibility.
    pub visibility: Visibility,
    /// Property type.
    pub ty: Type,
    /// Property name.
    pub name: String,
    /// Getter accessor, if declared.
    pub getter: Option<Accessor>,
    /// Setter accessor, if declared.
    pub setter: Option<Accessor>,
}

/// A property accessor implementation.
#[derive(Debug, Clone, PartialEq)]
pub struct Accessor {
    /// Optional accessor visibility override (must be more restrictive than
    /// the property's own visibility, e.g. `int Age { get; private set; }`).
    pub visibility: Option<Visibility>,
    /// The accessor implementation.
    pub kind: AccessorKind,
}

/// Property accessor implementation.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessorKind {
    /// Auto accessor (`get;` / `set;`) backed by a generated field.
    Auto,
    /// Accessor with an explicit body.
    Body(Vec<Stmt>),
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
    /// Whether this is a constructor: `Counter(int start) { ... }`.
    pub is_constructor: bool,
    /// For constructors, an optional chaining call `: super(...)` / `: this(...)`.
    pub constructor_chain: Option<ConstructorChain>,
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

/// Constructor chaining target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructorTarget {
    /// Chain to the base class constructor: `: super(...)`.
    Base,
    /// Chain to another constructor of the same class: `: this(...)`.
    This,
}

/// Constructor chaining call: `: super(...)` or `: this(...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstructorChain {
    /// Whether the call targets the base class or the same class.
    pub target: ConstructorTarget,
    /// Arguments passed to the chained constructor.
    pub args: Vec<Expr>,
}

/// Parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// Parameter type.
    pub ty: Type,
    /// Parameter name.
    pub name: String,
}

/// Suffix applied to an integer literal, e.g. `42i8`, `5u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntSuffix {
    /// No suffix: the literal type is inferred from its value (int if it fits
    /// 32 bits, otherwise int64).
    None,
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
}

/// Suffix applied to a float literal, e.g. `1.5f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatSuffix {
    /// No suffix: `float64`.
    None,
    /// `f32`
    F32,
    /// `f64`
    F64,
}

/// Type annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Unit / void.
    Unit,
    /// 8-bit signed integer.
    Int8,
    /// 16-bit signed integer.
    Int16,
    /// 32-bit signed integer.
    Int32,
    /// 64-bit signed integer.
    Int64,
    /// 8-bit unsigned integer.
    UInt8,
    /// 16-bit unsigned integer.
    UInt16,
    /// 32-bit unsigned integer.
    UInt32,
    /// 64-bit unsigned integer.
    UInt64,
    /// 32-bit floating point number.
    Float32,
    /// 64-bit floating point number.
    Float64,
    /// Boolean.
    Bool,
    /// Unicode scalar value.
    Char,
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
    /// Nullable type: `T?`. Only nullable types may hold `null`.
    Nullable(Box<Type>),
    /// A declared newtype: distinct nominal wrapper carrying its name and
    /// underlying primitive type. Erased to the underlying type at runtime.
    Newtype(String, Box<Type>),
    /// Untyped integer literal carrying its value. Only ever inferred; never
    /// written in source. Used to allow literals to coerce to any integer type
    /// whose range fits the value.
    IntLit(i64),
    /// Untyped float literal carrying its value. Only ever inferred.
    FloatLit(f64),
}

impl Type {
    /// Textual representation used in diagnostics.
    pub fn name(&self) -> String {
        match self {
            Type::Unit => "void".to_string(),
            Type::Int8 => "int8".to_string(),
            Type::Int16 => "int16".to_string(),
            Type::Int32 => "int32".to_string(),
            Type::Int64 => "int64".to_string(),
            Type::UInt8 => "uint8".to_string(),
            Type::UInt16 => "uint16".to_string(),
            Type::UInt32 => "uint32".to_string(),
            Type::UInt64 => "uint64".to_string(),
            Type::Float32 => "float32".to_string(),
            Type::Float64 => "float64".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Char => "char".to_string(),
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
            Type::Nullable(inner) => format!("{}?", inner.name()),
            Type::Newtype(name, _) => name.clone(),
            Type::IntLit(v) => format!("int literal {}", v),
            Type::FloatLit(_) => "float literal".to_string(),
        }
    }

    /// True if this is a signed integer type (not a literal marker).
    pub fn is_signed_int(&self) -> bool {
        matches!(
            self,
            Type::Int8 | Type::Int16 | Type::Int32 | Type::Int64
        )
    }

    /// True if this is an unsigned integer type (not a literal marker).
    pub fn is_unsigned_int(&self) -> bool {
        matches!(
            self,
            Type::UInt8 | Type::UInt16 | Type::UInt32 | Type::UInt64
        )
    }

    /// True if this is any integer type (not a literal marker).
    pub fn is_int(&self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    /// True if this is a floating point type (not a literal marker).
    pub fn is_float(&self) -> bool {
        matches!(self, Type::Float32 | Type::Float64)
    }

    /// True if this is any numeric type (not a literal marker).
    pub fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
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
    /// If-let statement: pattern match with optional else.
    /// `if let pattern = expr { ... } else { ... }`
    IfLet(Pattern, Expr, Vec<Stmt>, Option<Vec<Stmt>>),
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
    /// Integer literal with optional type suffix (e.g. `42`, `5u8`, `10i64`).
    IntLit(i64, IntSuffix),
    /// Float literal with optional type suffix (e.g. `1.5`, `1.5f32`).
    FloatLit(f64, FloatSuffix),
    /// Explicit numeric cast: `(int8)x` or `x as int8`.
    Cast(Box<Expr>, Type),
    /// Boolean literal.
    Bool(bool),
    /// Character literal (Unicode scalar value).
    Char(char),
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
    /// `new ClassName()` with optional type arguments and constructor arguments.
    New(String, Vec<Type>, Vec<Expr>),
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
    /// Non-null assertion: `value!`. Asserts a nullable value is non-null,
    /// raising a runtime error if it is null.
    NonNullAssert(Box<Expr>),
    /// Null conditional field access: `a?.field` returns null if `a` is null.
    NullConditionalField(Box<Expr>, String),
    /// Null conditional method call: `a?.method()` returns null if `a` is null.
    NullConditionalCall(CallExpr),
    /// Base class method call from a subclass method: `super.Method(args)`.
    SuperCall(String, Vec<Expr>),
    /// Base class field access from a subclass method: `super.field`.
    SuperField(String),
    /// Record copy-with expression: `p with { x = 5, y = 6 }` produces a copy
    /// of a record with the listed fields replaced.
    With(Box<Expr>, Vec<(String, Expr)>),
    /// Block expression: `{ stmts }` evaluates to the last expression's value.
    Block(Vec<Stmt>),
    /// Error propagation: `expr?` unwraps a `Result`-like sum type, returning
    /// the error variant from the enclosing function when it does not match.
    TryUnwrap(Box<Expr>),
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
    Int(i64),
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
    /// Enum variant pattern with nested sub-patterns: `Some(Point(1, 2))`.
    EnumVariant(String, String, Vec<Pattern>),
    /// Record pattern with positional sub-patterns: `Point(1, y)`.
    RecordClass(String, Vec<Pattern>),
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
