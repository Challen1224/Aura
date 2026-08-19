# Aura Language TODO

> **Status:** Core language and compiler: usable. VM with baseline x86-64
> JIT: implemented; static, virtual, and super calls all tier up. GC:
> generational (logical nursery, write barrier, minor/major split) with
> tuning flags, stats, weak/soft/phantom refs, and an opt-in concurrent
> collector (background marking, chunked sweeps), collecting under both tiers (JIT
> frames scanned conservatively — see §2.1). Debugging: source-level
> debugger (`aura debug`: breakpoints incl. bytecode-level, step
> into/over/out, locals, watches) plus a DAP adapter (`aura dap`) for VS
> Code/nvim-dap, on the interpreter tier; line-mapped error traces.
> Language: custom operator symbols (`|>`-style, one precedence tier)
> atop the existing overload set; multi-error compiles with did-you-mean
> suggestions and source-context rendering.
> Stdlib: collections
> (`List`/`Map`/`Set`), string methods, `Console.ReadLine`, and text `File`
> I/O are implemented and verified under both interpreter and JIT, including
> mid-run tier transitions; still missing: networking, async, reflection,
> binary I/O, string builder/formatting (see §4).
> Tooling: `aura.toml` projects (`init`/`build`/`test`, `[run]`
> defaults), a REPL, `fmt` (token-safe), `lint`, `doc` (from `///`
> comments), `run --watch`/`--stats`, plus the debugger and DAP adapter.
> Platforms: Linux (primary) and Windows x64 — the JIT's executable-memory
> layer has a `kernel32` path, JIT boundary calls are pinned to the System V
> ABI via `extern "sysv64"`, and a Windows installer
> (`packaging/windows/build-msi.sh`, cross-built from Linux with wixl)
> installs `aura.exe` onto `PATH`. Linux ships as a per-architecture
> `.deb` (`packaging/debian/build-deb.sh`; arm64 verified end-to-end).  
> **Last Updated:** 2026-08-19  
> **Current Version:** 1.1.0

---

## Table of Contents

- [1. Language Features](#1-language-features)
  - [1.1 Type System](#11-type-system)
  - [1.2 Object-Oriented Programming](#12-object-oriented-programming)
  - [1.3 Control Flow & Expressions](#13-control-flow--expressions)
  - [1.4 Generics & Polymorphism](#14-generics--polymorphism)
  - [1.5 Error Handling](#15-error-handling)
  - [1.6 Concurrency & Async](#16-concurrency--async)
- [2. Runtime & VM](#2-runtime--vm)
  - [2.1 Memory Management](#21-memory-management)
  - [2.2 Performance](#22-performance)
  - [2.3 Debugging & Profiling](#23-debugging--profiling)
- [3. Compiler Frontend](#3-compiler-frontend)
  - [3.1 Parser & Lexer](#31-parser--lexer)
  - [3.2 Type Checking](#32-type-checking)
  - [3.3 Code Generation](#33-code-generation)
- [4. Standard Library](#4-standard-library)
  - [4.1 Core Types](#41-core-types)
  - [4.2 Collections](#42-collections)
  - [4.3 I/O & Filesystem](#43-io--filesystem)
  - [4.4 Concurrency Primitives](#44-concurrency-primitives)
- [5. Tooling & Ecosystem](#5-tooling--ecosystem)
  - [5.1 CLI](#51-cli)
  - [5.2 Package Management](#52-package-management)
  - [5.3 IDE Support](#53-ide-support)
- [6. Testing & Quality](#6-testing--quality)
- [7. Documentation](#7-documentation)
- [8. Future Considerations](#8-future-considerations)

---

## 1. Language Features

### 1.1 Type System

#### ✅ Completed
- [x] Primitive types: `int8`–`int64`, `uint8`–`uint64`, `float32`/`float64`,
      `bool`, `char`, `string`, `void`
- [x] Null reference type
- [x] Type inference for local variables (`var`) — NOTE: this box was
      checked long before it was true; `var` did not exist until
      2026-08-14. It now infers from initializers (literals resolve to
      carrier types; nullables narrow like annotated locals; works in
      `for (var x in ...)`); `var` without an initializer or from
      null/void is a compile error.
- [x] Static type checking
- [x] Enum types with pattern matching
- [x] Tuple types with creation, access, and destructuring
- [x] String interpolation: `"Hello {name}"`

#### ⏳ Planned

**P0 - Critical**
- [x] **Numeric type hierarchy**
  - [x] `int8`, `int16`, `int32`, `int64` (signed integers)
  - [x] `uint8`, `uint16`, `uint32`, `uint64` (unsigned integers)
  - [x] `float32`, `float64` (explicit precision)
  - [x] Type coercion rules and conversions
  - [x] Overflow/underflow checking (configurable)

- [x] **Character and string types**
  - [x] `char` type (Unicode scalar value)
  - [x] Raw string literals: `r"..."`
  - [x] Multi-line strings: `"""..."""`

- [x] **Union types / Sum types**
  ```aura
  type Result<T, E> = Ok(T) | Err(E);
  ```
  - [x] Generic sum types via `type Name<...> = V1(...) | V2(...)`
  - [x] Bare variant construction: `Ok(5)`
  - [x] Bare variant patterns: `match (r) { Ok(v) => ..., Err(m) => ... }`

**P1 - High Priority**
- [ ] **Structural typing**
  - [x] Duck typing for interfaces — a class whose public, non-generic
        instance methods exactly match an interface's abstract methods
        (including its extends-closure) satisfies it without declaring
        `: IFace`. Default methods are optional and inherited; the emitter
        records structural implementations into runtime metadata, so
        catch-by-interface and dispatch agree with the typer. v1 limits,
        by design: exact signature match (no variance), non-generic
        interfaces only, classes only as sources (interface-to-interface
        stays declaration-based), stdlib intrinsics excluded, and a
        default-method-only interface is satisfied by every class.
        Verified under both tiers (aura-vm/tests/duck_typing.rs).
  - [x] Type aliases: `type UserId = int;` (parser-level expansion; also
        composes with `?`)
  - [x] Newtype pattern support — `newtype UserId = int;` declares a
        distinct nominal wrapper over a primitive (int widths, float, bool,
        char, string; a newtype cannot wrap a class or another newtype).
        No implicit conversion in either direction, and distinct newtypes
        over the same primitive don't interconvert. Construct with
        `UserId(expr)`, unwrap with `.Value`; `==`/`!=` work between the
        same newtype; arithmetic and the underlying type's methods require
        unwrapping. Fully erased at runtime (zero cost — works as
        collection keys, composes with `T?` narrowing and `!`). Verified
        under both tiers (aura-vm/tests/newtype.rs).

- [x] **Nullable types (strict)** — `T?` for reference and value types;
      `null` is only assignable to `T?`, and member access on `T?` is a
      compile error until the value is narrowed or asserted. Narrowing
      covers locals/params via `if (x != null)` (then-branch),
      `if (x == null)` (else-branch and after an always-exiting branch),
      and `while (x != null)`; assigning to a narrowed variable widens it
      back. `value!` asserts non-null (runtime error if null, via an
      AssertNonNull native under both tiers); `??` unwraps to the non-null
      type; `?.` produces `T?`. `Console.ReadLine()` now returns `string?`.
      Known holes (deliberate, documented): fields are not
      definite-assignment checked (an unassigned non-nullable reference
      field reads as null and fails at use with a runtime error), field
      expressions are never narrowed (copy to a local or use `!`), and
      compound conditions (`x != null && ...`) don't narrow yet.
      Verified: aura-vm/tests/nullable.rs (narrowing/assert/??/?.
      semantics under interpreter and JIT, plus compile-fail cases pinning
      each strictness rule).
  - [x] Type guards beyond null checks — `expr is Type` / `expr is Type
        name` runtime tests (new `IsInst` op reusing the catch-matching
        instance walk, so duck-typed interfaces test correctly; null and
        provably-impossible tests between unrelated concrete classes are
        compile errors; generic type arguments are not testable — erased).
        The binding is flow-scoped: visible in the then-branch, loop body,
        and the rhs of the same `&&` chain, never the else-branch. Facts
        compose through conditions: `&&` carries left-side facts into the
        right side and the branch, `||` narrows by the negation, `!` flips
        — so `x != null && x.f > 0` and `m == null || m.R() < 0` type and
        run safely. Prerequisite fix that this soundness depends on:
        `&&`/`||` now short-circuit (they previously evaluated both
        operands eagerly), pinned by a side-effect-counting test. Verified
        under both tiers (aura-vm/tests/type_guards.rs).

- [x] **Literal types** (string-literal unions)
  ```aura
  type Direction = "north" | "south" | "east" | "west";
  ```
  A literal is assignable to the union exactly when it is a declared
  member; plain strings and other unions (even with overlapping members)
  never flow in, and comparing against a non-member literal is a compile
  error. Widening is free: a union value is a `string` at runtime (fully
  erased — string methods, hashed Map keys, `foreach`, and `T?` narrowing
  all work). Bare string literals now infer a transient literal type that
  widens to `string` everywhere else (mirroring the existing int-literal
  machinery). Int-literal unions are not implemented. Subset-widening
  between unions was originally excluded but is now part of the union
  algebra (see Type-level computation below). Verified under both tiers
  (aura-vm/tests/literal_types.rs).

**P2 - Medium Priority**
- [ ] **Advanced type features**
  - [x] Dependent types (research) — study written:
        docs/research/dependent-types.md. Conclusion: park indefinitely;
        no implementation planned. If type-level guarantees become a
        priority, the incremental path is (1) enforce generic constraints
        (see gap below), (2) integer interval facts in the existing
        GuardFact narrowing engine as diagnostics, and only then evaluate
        const generics. SMT-based refinement types are rejected for this
        project. Near-term idiom for invariants: newtype + validating
        constructor.
  - [ ] Refinement types (see research note above: rejected in SMT form;
        interval-fact diagnostics are the plausible subset)
  - [x] Phantom types — supported as a checked idiom: a generic parameter
        used only as a compile-time tag (`FileHandle<Open>` vs
        `FileHandle<Closed>`) is un-dodgeable. Type-argument arity is now
        exact (raw references to generic classes like `FileHandle` or
        `new FileHandle(1)` are compile errors, as are arguments on
        non-generic classes), cross-tag assignment/argument passing is
        rejected by the existing invariant checking, and tags are fully
        erased at runtime. Verified under both tiers
        (aura-vm/tests/phantom_types.rs, examples/phantom_types.aura).
  - [x] Type-level computation (v1: literal-union algebra) — union
        declarations compose other unions (`type Direction = Horizontal |
        Vertical;`, mixed literal/name operands allowed); members merge in
        declaration order with silent dedup across operands (explicit
        duplicate literals in one declaration still error). Subset
        widening follows the algebra: a union value flows into any union
        containing all its members; the reverse stays rejected. An
        all-bare-variant `type` declaration is reinterpreted as a union
        only when every name resolves to a literal union; mixing union
        names with enum variants is a hard error (previously it silently
        parsed as a wrong enum), and cycles/unknown names are errors. Sum
        types are untouched. Generic type aliases (already shipped) are
        the type-level-function baseline. Not in scope: conditional/mapped
        types, int-literal unions, comptime-style evaluation. Prerequisite
        fix that landed with this: match-arm and ternary joins now widen
        int/float literal markers like string literals (int-literal match
        expressions previously never unified). Verified under both tiers
        (aura-vm/tests/union_algebra.rs).

- [x] **Generic constraint enforcement** — `<T : IFace>` (and class
      constraints) are now enforced at every type reference and `new`
      instantiation via assignability, so structural (duck-typed)
      satisfaction counts. v1 limit, documented: a type argument that is
      itself a generic parameter is accepted without constraint
      propagation. Was: parsed but silently ignored (found during the
      dependent-types research).

- [ ] **Type inference improvements**
  - [x] Hindley-Milner style inference (scoped: local + call-site
        unification) — full HM is deliberately out of scope (it does not
        coexist with subtyping; that trade-off is why C#/Kotlin/Java do
        local inference). What shipped: `var` locals (see 1.1), and
        generic-method call-site type-argument inference by structural
        unification of parameter types against arguments — nested shapes
        (`List<T>`, `T?`, tuples) participate, bindings from several
        arguments join through assignability to the more general type,
        and conflicts / uninferable variables / unsatisfied constraints
        are precise errors. Constrained method generics (`<T : Sized>`)
        also gained bounded polymorphism: a constrained parameter's
        members are callable in the body via the constraint. Explicit
        call-site type arguments (`Pick<int>(...)`) are not implemented —
        inference only. Verified under both tiers
        (aura-vm/tests/inference.rs).
  - [x] Better error messages for type mismatches — two halves. (1)
        Locations: type/emit errors now carry `line N, in
        `Class.Method`:` (per-token lexer lines -> parser-injected
        `Stmt::Mark` markers -> one central prefix wrapper; parse errors
        report their line via the consume helpers). Signature-level errors
        outside statement context still lack lines. (2) Hints: known
        mismatch patterns append a one-line fix — nullable (narrow or
        `!`/`??`), newtype (wrap with `Name(...)` / unwrap with
        `.Value`), literal unions (non-member literal lists the members;
        union-into-subset names the members that don't fit), structural
        interface near-misses (the exact missing/mismatched method with
        both signatures), and numeric narrowing. Hints fire on
        assignments, arguments, and returns. Markers are proven
        behavior-neutral by the full suite (aura-vm/tests/diagnostics.rs).
  - [x] Type hole suggestions — `_` in expression position is a typed
        hole: always a compile error, but one that names the expected type
        and lists the fits. Positions with a known expected type
        (declaration initializers, assignments, return values, call
        arguments) report it exactly, list in-scope locals/params whose
        types fit (never non-matching ones), and add a construction
        suggestion when the type has an obvious one: literal-union
        members, `Name(...)` for newtypes, `new X(...)` for concrete
        classes, `true`/`false` for bool. Untyped positions (`1 + _`) say
        honestly that the expected type cannot be determined; `var x = _`
        points at annotating. Declaring `_` still parses (write-only
        discard); reading it is a hole. Pattern wildcards are unaffected
        (separate `Pattern::Wildcard`). Verified:
        aura-vm/tests/type_holes.rs.

**P3 - Nice to Have**
- [ ] **Experimental features**
  - [ ] Linear types (resource management)
  - [ ] Affine types
  - [ ] Session types for protocol verification

---

### 1.2 Object-Oriented Programming

#### ✅ Completed
- [x] Class declarations
- [x] Instance and static methods
- [x] Instance and static fields
- [x] Constructor support (via `new`)
- [x] Virtual method dispatch (runtime polymorphism)
- [x] `this` reference

#### ⏳ Planned

**P0 - Critical**
- [x] **Inheritance**
  ```aura
  class Animal {
      virtual void speak() { }
  }
  
  class Dog : Animal {
      override void speak() {
          print("Woof!");
      }
  }
  ```
  - [x] Single inheritance
  - [x] `super` keyword for base class access
  - [x] Constructor chaining (`: super(...)` / `: this(...)`), constructor overloads, and implicit base constructor invocation
  - [x] Protected visibility modifier

- [ ] **Interfaces**
  ```aura
  interface Drawable {
      void draw();
  }
  
  interface Resizable {
      void resize(int width, int height);
  }
  
  class Window : Drawable, Resizable {
      void draw() { /* ... */ }
      void resize(int w, int h) { /* ... */ }
  }
  ```
  - [x] Interface declarations
  - [x] Multiple interface implementation
  - [x] Default interface methods
  - [x] Interface inheritance
  - [x] Interface-typed variables, fields, parameters, and return types
  - [x] Interface validation (no fields, no static/protected methods, cannot extend classes)

- [x] **Visibility modifiers**
  - [x] `public` (default)
  - [x] `private`
  - [x] `protected`
  - [x] `internal` (module-scoped) — previously parsed but vacuously
        public (`can_access` returned true unconditionally, and internal
        FIELDS weren't even tracked). Now enforced with a minimal module
        model: **module = source file**. The CLI accepts multiple files
        (`aura run main.aura lib.aura`); declarations share one flat
        namespace (no imports), and `internal` members (methods, static
        methods, fields, static fields) are only accessible from classes
        declared in the same file. Single-file programs put everything in
        one module, so `internal` stays file-wide there — no behavior
        change. All language features resolve across files (inheritance,
        duck typing, literal unions, newtypes, generics); the builtin
        `Exception` is deduplicated across parses and belongs to no
        module. Verified under both tiers (aura-vm/tests/modules.rs,
        examples/multifile/).

**P1 - High Priority**
- [x] **Abstract classes and methods**
  ```aura
  abstract class Shape {
      abstract int area();
  }
  ```
  - [x] `abstract` class modifier (cannot be instantiated)
  - [x] `abstract` methods (must be implemented by concrete subclasses)
  - [x] Abstract classes cannot be both abstract and sealed

- [x] **Sealed classes and final methods**
  ```aura
  sealed class FinalClass { }
  ```
  - [x] `sealed` class modifier (cannot be subclassed)
  - [x] `final` method modifier (cannot be overridden or re-declared)
  - [x] `final override` (override a virtual method and seal it)
  - [x] Cannot mix `final` with `virtual`/`abstract`, `sealed` with `abstract`

- [x] **Static classes / namespaces**
  ```aura
  static class Math {
      static float PI = 3.14159;
      static int Max(int a, int b) { ... }
  }
  ```
  `static class` is the namespace idiom with C# rules: cannot be
  instantiated, inherited from, or used as a type; no constructors; every
  member must be static — each rule its own compile error. This also
  landed **static field initializers** (on any class, not just static
  ones): constant literals only (int/float/bool/char/string, optionally
  negated), type-checked against the field, stored as `ConstInit` in the
  bytecode `FieldDef`, and applied at VM startup (string constants
  allocate on the heap and are GC roots via static fields). Initialized
  statics remain mutable — initializers are starting values, not consts.
  Not included, documented: instance field initializers (need constructor
  injection; a dedicated error points at assigning in a constructor) and
  arbitrary initializer expressions (need static constructors). Verified
  under both tiers (aura-vm/tests/static_classes.rs,
  examples/static_classes.aura).

- [x] **Properties**
  ```aura
  class Person {
      string Name { get; set; }
      int Age { get; private set; }
  }
  ```
  - [x] Auto accessors with backing fields
  - [x] Explicit getter/setter bodies
  - [x] Visibility modifiers per accessor
  - [x] Static properties

**P2 - Medium Priority**
- [x] **Operator overloading**
  ```aura
  class Vector {
      Vector operator+(Vector other) { ... }
      bool operator==(Vector other) { ... }
  }
  ```
  Overloadable: `+ - * / %` and `== < <= > >=`. An overload is an
  instance method named `operator+` etc. — `a + b` lowers to a call on
  the left operand (JIT-transparent: plain `CallVirt`, left-to-right
  evaluation preserved via a temp), so overloads are inherited and
  generic receivers substitute their type arguments. One parameter, the
  right operand — any type, so `vector * 2.0` works. Rules, each its own
  error: public, non-static/virtual/abstract, one parameter, real return
  type; comparisons must return bool; `operator!=` cannot be declared
  (`!=` is always the negation of `operator==`); `&&`/`||` are not
  overloadable (short-circuit); a nullable left operand must be narrowed
  first. `==`/`!=` without an overload stay reference equality (records
  keep structural equality); ordering a class *without* `operator<` is
  now a compile error (it silently pointer-compared before). Also fixed
  two latent parser bugs the feature exposed: `a.f < b.n` was mis-parsed
  as generic type args (lookahead now requires a balanced `<...>` then
  `(`), and explicit type args on static generic calls
  (`Util.Pick<int>(...)`) never parsed at all. Not included: unary
  operator overloads and `[]` indexing. Verified under both tiers
  (aura-vm/tests/operator_overloading.rs,
  examples/operator_overloading.aura).

- [x] **Extension methods**
  ```aura
  extension StringExtensions on string {
      bool isPalindrome() { ... }
  }
  ```
  `extension Name on Target { methods }` adds callable methods to an
  existing type; `this` in bodies is the receiver. Desugars to a static
  class whose methods take the receiver as a leading parameter, so
  `"noon".isPalindrome()` and `StringExtensions.isPalindrome("noon")` are
  the same static call (JIT-transparent, zero VM changes). Resolution: a
  real (or inherited) method on the receiver always wins; otherwise the
  receiver's class then its superclass chain is searched for an
  extension. Extensions on a base class apply to subclass receivers;
  an extension shadowed by an existing target method is rejected at
  declaration (dead code), while a *subclass* may still declare the name.
  Targets: `string` or a non-generic user class/interface. Extensions
  live in ordinary modules (multi-file tested). Rules, each its own
  error: methods only, public, no constructors/operators/modifiers;
  targets can't be enums, newtypes, static classes, built-in collection
  classes, primitives, or generic instantiations; duplicate extension
  methods on one target collide; nullable receivers must be narrowed.
  Note: the receiver evaluates before the arguments (static-call order).
  Not included: extensions on `List`/`Map`/`Set` (needs generic targets),
  extension properties/operators, `?.` dispatch to extensions. Verified
  under both tiers (aura-vm/tests/extension_methods.rs,
  examples/extension_methods.aura).

- [x] **Nested classes**
  ```aura
  class Outer {
      class Inner { }
  }
  ```
  A compile-time desugar (`aura-compiler/src/nested.rs`): nested classes
  hoist to top level under mangled names (`Outer.Inner`) and every
  reference resolves through the enclosing scope chain — unqualified
  `Inner` inside `Outer` (shadowing any top-level `Inner`), qualified
  `Outer.Inner` outside, in types, `new`, static member reads AND
  writes, `is` checks, and match patterns (`Outer.P(x, _)`). Classes,
  records, interfaces, static classes, abstract/sealed classes, and
  generic classes all nest, to arbitrary depth; sibling references work.
  A nested class can read its enclosing class's private/protected
  members from any depth; the reverse is denied, as in C#. Interfaces
  cannot declare nested classes. Zero typer/emitter/VM/JIT changes for
  the feature itself — but its tests exposed two pre-existing bugs, both
  fixed: (1) the x86-64 JIT panicked (regalloc index OOB) on any method
  containing dead bytecode blocks, e.g. a match whose last arm is
  irrefutable — `lower_blocks` skipped unreachable blocks while ids and
  successor lists kept sparse numbering; placeholders now hold the slots
  (regression: aura-vm/tests/jit_dead_blocks.rs). (2) Range for-in loop
  variables were never registered in the emitter's type map, so
  `new C(i)` inside `for (var i in 1..=n)` failed to compile. Verified
  under both tiers (aura-vm/tests/nested_classes.rs,
  examples/nested_classes.aura).

**P3 - Nice to Have**
- [ ] **Mixin / Trait system** (alternative or complement to interfaces)
- [ ] **Partial classes** (split class definition across files)
- [x] **Record classes** (immutable data classes with value semantics)

---

### 1.3 Control Flow & Expressions

#### ✅ Completed
- [x] `if` / `else` statements
- [x] `while` loops
- [x] `for` loops with break/continue
- [x] `do-while` loops
- [x] Ternary operator (`?:`)
- [x] Match expressions with pattern matching
- [x] Arithmetic operators: `+`, `-`, `*`, `/`, `%`
- [x] Comparison operators: `==`, `!=`, `<`, `<=`, `>`, `>=`
- [x] Logical operators: `&&`, `||`, `!`
- [x] Variable declarations with type annotations
- [x] Assignment expressions
- [x] Variable shadowing (proper scoping)

- [x] **Range expressions**
  ```aura
  for (int i in 1..10) { }      // exclusive: 1 to 9
  for (int i in 1..=10) { }     // inclusive: 1 to 10
  for (int i in 2+1..2*3) { }   // expressions allowed
  ```

- [x] **Range patterns in match expressions**
  ```aura
  match (value) {
      1..=5 => "low"
      6..=10 => "medium"
      * => "other"
  }
  ```

- [x] **Null coalescing operator**
  ```aura
  string result = nullableValue ?? "default";
  ```

- [x] **Null conditional operator**
  ```aura
  string name = person?.name;
  string result = obj?.method();
  ```

#### ⏳ Planned

**P1 - High Priority**
- [x] **Pattern matching enhancements**
  - [x] Enum variant patterns
  - [x] Range patterns
  - [x] Pattern guards with complex expressions
  - [x] Nested patterns

- [x] **Null conditional**

**P2 - Medium Priority**
- [x] **Labeled break/continue**
  ```aura
  outer: for (int i = 0; i < 10; i = i + 1) {
      for (int j = 0; j < 10; j = j + 1) {
          if (condition) break outer;
      }
  }
  ```

- [x] **Guard clauses**
  ```aura
  if let Some(value) = optional {
      // use value
  }
  ```
  Supports enum variant, range, constant, and binding patterns with optional else.

- [x] **Expression blocks**
  ```aura
  int result = {
      int x = computeX();
      int y = computeY();
      x + y
  };
  ```
  A block evaluates to its last expression's value; block locals are scoped to the block.

**P3 - Nice to Have**
- [ ] **Coroutines / generators**
  ```aura
  generator<int> fibonacci() {
      yield 0;
      yield 1;
      // ...
  }
  ```

- [ ] **Comprehensions**
  ```aura
  let squares = [x * x for x in 1..10];
  ```

---

### 1.4 Generics & Polymorphism

#### ✅ Completed
- [x] Generic classes: `class Box<T> { }`
- [x] Generic methods: `T identity<T>(T value) { }`
- [x] Type arguments: `Box<int>`, `new Box<string>()`
- [x] Reified generics (runtime type information)
- [x] Generic type substitution
- [x] Multiple instantiations of generic types
- [x] Generic constraints (basic)

#### ⏳ Planned

**P0 - Critical**
- [x] **Generic constraints**
  ```aura
  class Comparable<T> where T : IComparable { }
  class Numeric<T> where T : int | float { }
  class DefaultConstructible<T> where T : new() { }
  ```
  `where` clauses on classes and generic methods (`static <T> T Max(...)
  where T : int | float`), plus the same constraints inline
  (`<T : int | float>`). Three forms: a subtype bound (member access on
  `T` via the existing bounded polymorphism), a union — the type
  argument must fit one alternative, and an all-numeric union
  additionally licenses arithmetic and ordering between two `T` values
  in bodies (the runtime ops are dynamic, so int and float
  instantiations share one compiled body) — and `new()`, requiring a
  concrete class with a parameterless (or no declared) constructor. A
  bound may comma-combine with `new()` (`where T : Base, new()`); a
  union may not. Enforced at class instantiation and at generic-method
  call sites against inferred arguments, each violation with its own
  error. Also tightened: ordering (`<`) on a type parameter without a
  numeric union constraint is now a compile error (it previously
  type-checked and died or pointer-compared at runtime), and `T op T`
  requires both operands to be the same `T` — `T + 1` is rejected since
  a float instantiation would mix types at runtime. Not included,
  documented: `new T()` (needs reified generics — targeted error points
  at `new()` being a call-site contract), self-referential bounds
  (`IComparable<T>`), and constraint checking when the argument is
  itself an enclosing type parameter (skipped, as before). Verified
  under both tiers (aura-vm/tests/generic_constraints.rs,
  examples/generic_constraints.aura).

- [x] **Variance annotations**
  ```aura
  interface IProducer<out T> { T produce(); }      // Covariant
  interface IComparer<in T> { int compare(T a, T b); }  // Contravariant
  ```
  `out T` (covariant): `IProducer<Cat>` flows into `IProducer<Animal>`.
  `in T` (contravariant): `IComparer<Animal>` flows into
  `IComparer<Cat>`. Unannotated parameters are invariant across
  instantiations. Soundness checked conservatively: `out T` may not
  occur anywhere in a method parameter type (nested included, e.g.
  `List<T>`), `in T` may not occur in a return type, properties follow
  their accessors, and annotations are interface-only (classes and
  method type parameters reject them). This feature also made
  **generic-interface implementation real**: base lists now take type
  arguments (`class CatFactory : IProducer<Cat>` — previously
  unparseable), the declared instantiation is recorded, conformance is
  checked against the substituted interface signatures (generic
  implementors like `class Repeat<E> : IProducer<E>` included), a
  generic interface implemented without arguments is an error, and
  assignability into a generic-interface type resolves the source's
  declared instantiation through its superclass/interface chain and
  compares variance-aware. Not included, documented: generic *base
  classes* with type arguments (explicit error), interface-extends
  argument substitution beyond direct declarations, and tightening the
  pre-existing permissive covariance of same-name non-interface
  generics (`List<Cat>` into `List<Animal>` still passes; separate
  cleanup). Verified under both tiers (aura-vm/tests/variance.rs,
  examples/variance.aura).

- [x] **Generic type inference**
  ```aura
  var box = new Box(42);  // Infer Box<int>
  ```
  Constructor calls on generic classes without explicit type arguments
  infer them by unifying constructor parameters against the argument
  types — the same machinery as generic-method call sites. Bindings
  from several arguments join through assignability (`new Pair(derived,
  base)` binds the wider type); conflicting arguments are an error;
  constructors that never mention a parameter cannot determine it and
  say so (phantom-tagged classes keep their guarantee — that test's
  pinned message changed accordingly); constraints (bounds, unions,
  `new()`) apply to inferred arguments exactly as to explicit ones.
  Works with `var` and explicitly-typed targets, records, multi-param
  classes, and nested inference (`new Box(new Box(7))`). Also fixed a
  pre-existing bug it exposed: field access on a generic instantiation
  (`pair.k.Length`) failed to compile even with explicit type arguments
  — the emitter never substituted the receiver's instantiation into
  field types (methods substituted; fields did not). Verified under
  both tiers (aura-vm/tests/construction_inference.rs,
  examples/construction_inference.aura).

**P1 - High Priority**
- [x] **Higher-kinded types** (research) — study written:
      docs/research/higher-kinded-types.md. Conclusion: park indefinitely.
      The sketch's own `Func<A, B>` is the real finding — Aura has no
      function types, lambdas, or closures, and that prerequisite is both
      the blocker and the independently-valuable next feature. After
      lambdas land, concrete `map`/`filter`/`fold` on the collections (the
      LINQ move) captures most of the Functor payoff with zero type-system
      work. If HKT ever earns its keep (a library ecosystem writing the
      same container-generic code repeatedly), the path is interface
      witnesses over the existing erased generics plus Miller-pattern
      constructor unification — every recently landed feature (variance,
      constraints, inference) makes that easier, not harder.

- [x] **First-class function types and lambdas**
  ```aura
  Func<int, int> double = x => x * 2;      // last type arg = return type
  var add = (int a, int b) => a + b;       // annotated lambdas self-infer
  Action<string> log = s => { ... };       // void-returning
  ```
  Lambdas (`x => expr`, parenthesized/typed parameter lists, block
  bodies) capture enclosing locals and `this` **by value** — assigning
  to a captured variable is a compile error with a pointed message.
  Target-typed from `Func<...>`/`Action<...>` contexts (declarations,
  assignments, returns, call arguments); fully annotated
  expression-body lambdas self-infer, so `var` works. Compilation lifts
  each lambda to a synthesized static method whose leading parameters
  are its captures (`this` first), so lambda bodies tier up in the JIT
  like any method; a closure is a GC-traced heap object pairing the
  lifted method with its captured values (`Op::NewClosure`), and calls
  go through `Op::Invoke` — implemented natively in the interpreter and
  via the existing helper mechanism in JIT code, so **both tiers run
  closures**. Nested lambdas (currying: `a => (int b) => a + b`)
  capture outer-lambda parameters transitively. Function values are
  first-class: reassignable, passable, returnable, storable. Not
  included, documented: calling a call result directly
  (`curry(1)(2)` — bind it to a local first), method references
  (`Program.Max` as a value), lambdas as arguments binding *generic*
  method type parameters (annotate the lambda instead), and capture by
  reference. Verified under both tiers (aura-vm/tests/lambdas.rs,
  examples/lambdas.aura).

- [x] **Generic methods with multiple type parameters**
  ```aura
  static <T1, T2> T2 transform(T1 value, Func<T1, T2> converter) { ... }
  Util.transform(21, x => x * 2);   // T1 = int from 21, T2 = int from the body
  ```
  Multiple type parameters already declared/inferred/explicitly-applied
  fine; this item landed the piece the sketch actually needs —
  **two-pass call-site inference over lambda arguments** (the exclusion
  documented when lambdas shipped). Non-lambda arguments bind what they
  can first; each lambda argument is then target-typed by the partially
  substituted parameter: its parameter types must be resolved by that
  point, while a return type still naming an open type parameter is
  determined by the lambda's body and flows into the bindings
  (`transform(7, n => "n={n}")` makes T2 = string). Annotated lambdas
  and explicit type arguments work as before; a lambda nothing
  constrains still asks for annotations, and conflicts with inferred
  bindings are errors. Mirrored in the emitter (same unification pass)
  so lifted signatures match. Open-return inference needs expression
  bodies; block-bodied lambdas want a concrete target. Verified under
  both tiers (aura-vm/tests/multi_param_generics.rs,
  examples/multi_param_generics.aura).

- [x] **Generic constraints with multiple bounds**
  ```aura
  class Repository<T> where T : Entity, ICloneable { }
  ```
  `GenericParam` now carries a bound *list* rather than a single
  constraint: a type argument must satisfy every bound (class and any
  number of interfaces), each violation naming the failing bound, at
  instantiations and generic-method call sites alike. Bounded member
  access searches all bounds — the first bound declaring the method
  answers, and a method no bound declares gets an error listing the
  bounds. The emitter learned bounded receivers too (previously chained
  calls on a bounded `T`, like `v.name().Length`, could not be typed at
  emission — latent single-bound gap). Bounds combine with `new()`;
  union constraints stand alone and reject any combination (two
  existing tests pinned the old wordings — `already constrained`,
  `without a constraint` — and were updated to the new messages;
  rejections unchanged). Verified under both tiers
  (aura-vm/tests/multiple_bounds.rs, examples/multiple_bounds.aura).

**P2 - Medium Priority**
- [ ] **Variadic generics**
  ```aura
  class Tuple<T...> { }
  ```

- [ ] **Generic type aliases**
  ```aura
  type Dictionary<K, V> = Map<K, V>;
  ```

**P3 - Nice to Have**
- [ ] **Dependent generics** (types that depend on values)
- [ ] **Generic associated types**
- [ ] **Type-level computation with generics**

---

### 1.5 Error Handling

#### ✅ Completed
- [x] **Exception system**
  ```aura
  try {
      readFile("data.txt");
  } catch (FileNotFoundException e) {
      print("File not found: " + e.message);
  } catch (IOException e) {
      print("IO error: " + e.message);
  } finally {
      cleanup();
  }
  ```
- [x] **Custom exception classes**
  ```aura
  class Exception {
      string message;
      string stackTrace;
  }
  
  class MyError : Exception { }
  ```
- [x] **Stack traces**
  ```aura
  try {
      throw new MyError();
  } catch (MyError e) {
      print(e.stackTrace);  // "at Program.Main\n..."
  }
  ```
- [x] **Resource management with `using`**
  ```aura
  using (resource) {
      resource.Use();
  }  // resource.Dispose() called automatically
  ```

#### ⏳ Planned

**P0 - Critical**
- [x] **Checked exceptions** (optional)
  ```aura
  static string readFile(string path) throws IOException { ... }
  ```
  Opt-in design ("optional" taken seriously): exceptions stay unchecked
  by default — `throw` freely, no clause needed. A method declaring
  `throws IOException, ParseError` binds its **callers**: each declared
  exception must be caught by an enclosing `try` (a catch of the type
  or a supertype — `Exception` covers everything; a catch body is not
  covered by its own try) or re-declared in the caller's own `throws`
  clause, passing the obligation up. `throws` names must be classes
  deriving from `Exception`; overrides and interface implementations
  may not add throws their base declaration lacks (the contract
  survives virtual dispatch); duplicates in one clause are rejected.
  Purely compile-time — zero bytecode/VM/JIT changes for the feature.
  Its tests exposed a serious pre-existing x86-64 JIT bug, fixed:
  `throw` is a block terminator but the terminator lowering only knew
  branches and returns, so compiled methods silently **dropped throw
  ops and fell through**, returning normal values instead of raising
  (verified broken at the previous HEAD; regression pinned in
  aura-vm/tests/jit_dead_blocks.rs::compiled_throw_raises). Not
  included, documented: throw-site checking inside unchecked methods
  (by design), `throws` on constructors/lambdas/`Func` types. Verified
  under both tiers (aura-vm/tests/checked_exceptions.rs,
  examples/checked_exceptions.aura).

- [ ] **Result type** (alternative to exceptions)
  ```aura
  Result<int, string> divide(int a, int b) {
      if (b == 0) return Err("Division by zero");
      return Ok(a / b);
  }
  ```

**P1 - High Priority**
- [x] **Exception chaining**
  ```aura
  catch (DbError e) { throw new AppError("boot failed", e); }
  // AppError's ctor: this.cause = e;   ... later: e.cause?.message
  ```
  The builtin `Exception` gained `cause` (`Exception?`, null by
  default) — subclasses store the wrapped original through ordinary
  constructors and field assignment, and reading `cause` follows the
  standard nullable rules (narrow before use, `is` refines the concrete
  type). The uncaught-exception report walks the chain and prints
  `caused by:` lines per link, depth-capped at 8 against cyclic chains
  (`... (chain truncated)`), and null fields no longer render as a
  stray "null" line. Zero compiler changes beyond the injected field —
  chaining is plain object graph, so it survives catches, rethrows, GC,
  and the JIT tier boundary unchanged. Verified under both tiers
  (aura-vm/tests/exception_chaining.rs,
  examples/exception_chaining.aura).

**P2 - Medium Priority**
- [ ] **Typed throws**
  ```aura
  throws<IOException, ParseException> void parse(string input) { }
  ```

- [ ] **Error propagation operator**
  ```aura
  let value = mightFail()?;  // Propagates error
  ```

---

### 1.6 Concurrency & Async

#### ⏳ Planned

**P1 - High Priority**
- [x] **Async/await**
  ```aura
  static async Task<string> brew(string what) {
      await Tasks.pause();          // cooperative yield
      return "{what} ready";
  }
  var t = Program.brew("tea");      // spawns a hot task (lazy progress)
  print(t.wait());                  // sync code drives the scheduler
  ```
  Cooperative coroutines on interpreter frames — real, deterministic,
  single-threaded concurrency (the sketch's `http.get` needs async I/O
  natives that don't exist yet; this is the substrate they'd plug
  into). An `async` method must be static and return `Task<T>`; its
  body returns the element type and may `await` any `Task<T>`. Calls
  spawn hot tasks (queued immediately) that progress only while the
  scheduler runs — at any `await` or at `t.wait()` from sync code.
  `await` suspends the current task's frame (frames are heap data:
  locals/stack/pc lift out of the call stack and park on the awaited
  task); FIFO scheduling makes interleaving deterministic and
  assertable. `Tasks.pause()` yields one scheduler round. Task
  exceptions surface catchably at every await/wait site (and keep
  failing on repeated waits); await cycles are detected as deadlocks,
  including self-await (a `waiting` state keeps cycles from spinning
  the ready queue). Suspended frames and results are GC roots. The
  three new ops (`Spawn`/`Await`/`TaskWait`) are interpreter-only —
  methods touching them fall back from the JIT while everything else
  tiers up (asserted). Rules, each its own error: `await` only in
  async methods (and not in lambdas), `Task<T>` return shape, no
  instance/interface async, no `throws` on async, `await` needs a
  `Task<T>`. Not included, documented: async lambdas, instance/virtual
  async, `Task.whenAll`-style combinators, real async I/O. Verified
  under both tiers (aura-vm/tests/async_await.rs,
  examples/async_await.aura).

- [x] **Fibers / green threads** (research) — study written:
      docs/research/fibers.md. Conclusion: park indefinitely. The
      async/await tasks are functionally a green-thread system already;
      the only remaining delta is yield-at-any-call-depth, and that is
      precisely an interpreter re-architecture (the op loop executes
      nested calls by Rust recursion, so mid-chain suspension would have
      to capture native frames) plus an unsolvable-here JIT wall (compiled
      frames can never suspend without deopt machinery). Function coloring
      is the price that keeps Aura's implementation small and sound. If
      concurrency work continues, the valuable next steps are task
      combinators (`Tasks.all`/`race`), async lambdas, and async I/O
      natives — not fibers.
- [x] **Task parallelism**
  ```aura
  var parts = await Tasks.all(taskList);   // every result, in list order
  var first = await Tasks.race(taskList);  // first completion wins
  ```
  Cooperative combinators over the task scheduler — the follow-up the
  fibers note sequenced. `Tasks.all(List<Task<T>>) -> Task<List<T>>`
  completes when every part has, results in **list order** (not
  completion order); the first failure in list order fails the whole,
  catchably; an empty list completes immediately with an empty List.
  `Tasks.race(List<Task<T>>) -> Task<T>` completes with the first part
  to finish (list order breaks same-round ties); a failing winner
  propagates (JS `Promise.race` semantics); losers keep running and
  stay awaitable; an empty race is a hard error. Combinators are tasks
  themselves — no frame, evaluated on resume, parked as waiters of
  their incomplete parts — so they nest, compose, and obey the same
  deadlock detection. This also landed method-level generic parameters
  on intrinsic statics (`all<T>`/`race<T>` infer `T` from the
  argument). "Parallel" means deterministically interleaved on one
  thread; the sketch's `await computeAsync(42)` form worked already.
  Verified under both tiers (aura-vm/tests/task_parallelism.rs,
  examples/task_parallelism.aura).

**P2 - Medium Priority**
- [ ] **Threads**
  ```aura
  Thread t = new Thread(() => {
      // background work
  });
  t.start();
  t.join();
  ```

- [ ] **Mutexes and locks**
  ```aura
  Mutex mutex = new Mutex();
  mutex.lock();
  // critical section
  mutex.unlock();
  ```

- [ ] **Channels**
  ```aura
  Channel<int> channel = new Channel();
  channel.send(42);
  let value = channel.receive();
  ```

**P3 - Nice to Have**
- [ ] **Actors**
- [ ] **Software transactional memory**
- [ ] **Data parallelism**
  ```aura
  parallel for (int i = 0; i < 1000; i++) {
      // executed in parallel
  }
  ```

---

## 2. Runtime & VM

### 2.1 Memory Management

#### ✅ Completed
- [x] Managed heap with `GcRef` handles
- [x] Mark-and-sweep collector implementation (`Heap::collect`)
- [x] Object allocation
- [x] Reference tracking
- [x] **GC triggering (safepoint-based)** — crossing the allocation
      threshold sets a pending flag; collection runs at safepoints (top of
      the interpreter op loop, and in the JIT helper dispatcher after each
      helper), never inside `allocate` itself, because natives hold
      unrooted handles in Rust locals mid-operation. Roots: frame locals,
      eval stacks, deferred `finally` exceptions, static fields, and JIT
      native frames. JIT frames are scanned **conservatively**: reference
      values are always `KType::Unknown` and therefore always live in
      canonical 16-byte frame slots (never registers), so any slot with a
      reference tag and a live handle payload is a root. This can
      over-retain (a stale slot pins a dead object until the frame exits)
      but never frees a live object. Verified: collections run mid-program
      under both tiers, including while JIT frames are on the native stack
      (aura-vm/tests/gc.rs).

#### ⏳ Planned

**P0 - Critical**
- [x] **Generational GC**
  - [x] Young generation (nursery)
  - [x] Old generation (tenured)
  - [x] Minor GC (young gen only)
  - [x] Major GC (full heap)
  - [x] Write barriers

  A *logical* nursery over the handle-stable heap: objects never move,
  so a generation is set membership, not a memory region. New objects
  are young; `heap.get_mut` — provably the single mutation gateway —
  doubles as an object-granularity write barrier logging mutated old
  objects into the remembered set (the only old objects that can point
  into the nursery, since unmutated survivors' children were all
  promoted with them). Minor collections trace only the nursery, seeded
  by the roots plus the remembered set's children, with old objects
  terminal; survivors promote in place and the nursery empties. Full
  pressure escalates to the existing major mark-and-sweep. The nursery
  trigger is capped (≤64KB) independently of the adaptive full-heap
  threshold, so a large stable old generation keeps minors frequent and
  cheap while majors stay rare — the pinned-pressure benchmark
  (aura-vm/tests/gc_bench.rs, `--ignored`) went from 18 full-heap scans
  to 350 minors + 1 major on an 8000-object retained heap with 150k
  transient allocations, ~6% faster overall and with far smaller
  per-pause work. `gc_minor_collections()`/`gc_major_collections()`
  exposed alongside the combined counter. Verified: the write barrier
  keeps young objects alive when reachable only through mutated old
  ones (641-link old→young chain under churn); statics, closures, and
  suspended task frames stay rooted through generational passes
  (aura-vm/tests/gc_nursery.rs), and the entire existing GC-churn suite
  runs through the new collector.

- [x] **GC tuning**
  - [x] Configurable heap size
  - [x] GC pause time targets
  - [x] Throughput vs latency tradeoffs
  - [x] GC statistics and metrics

  Every knob is real and CLI-exposed on `aura run`: `--gc-threshold`
  (full-heap trigger, adaptive growth), `--gc-nursery` (explicit
  nursery size), `--gc-max-heap` (hard limit — exceeding it after a
  full collection is a runtime error, surfaced through both tiers),
  `--gc-mode throughput|balanced|latency` (presets trading pause count
  for pause size: throughput = 256KB nursery + 3x growth, latency =
  16KB nursery; verified by strictly-ordered collection counts on the
  same workload), and `--gc-pause-target-ms` — a best-effort soft
  target for *minor* pauses via a feedback loop that halves the nursery
  when minors run over target and doubles it when far under (clamped
  8KB–1MB); majors remain stop-the-world and unbounded, stated plainly.
  `--gc-stats` prints a summary to stderr; programmatically,
  `vm.gc_stats()` snapshots counts (minor/major), live objects/bytes,
  total allocations, bytes freed, thresholds, and pause totals/maxima
  per generation. Verified: aura-vm/tests/gc_tuning.rs (5 tests:
  nursery-size ordering, mode ordering, hard-limit error + generous
  limit no-op, pause-target adaptation to the floor, stats
  consistency), full battery green under both tiers.

- [x] **Weak references**
  ```aura
  WeakRef<Object> weak = new WeakRef(obj);
  if (weak.isAlive()) {
      let obj = weak.get();
  }
  ```
  `WeakRef<T>` is an intrinsic class: `new WeakRef(obj)` infers `T` from
  the argument (the first intrinsic constructor to take one — the
  constructor path now type-checks and emits arguments), `isAlive() ->
  bool`, `get() -> T?`. The target handle is stored untraced; because
  GcRef handles are allocated monotonically and never reused, liveness is
  exactly heap membership — no sweep-phase clearing in either generation,
  and a dead target can never be confused with a recycled handle.
  Promptness caveat (documented, tested around): the JIT roots frames by
  conservative slot scan, so a target can outlive its last strong
  reference while a compiled frame that ever handled it is still on the
  stack; the shared guarantee is that a weak ref alone never keeps its
  target alive once such frames exit. Fixed along the way: the emitter's
  `__new_temp` constructor temp pinned the last-constructed object until
  frame exit (invisible before weak refs existed) — now cleared after
  use. Verified: aura-vm/tests/weak_refs.rs (5 tests: weak-vs-strong
  liveness ×3 trials, promotion across generations then major-collection
  reclaim, construction-site inference, WeakRefs inside collections,
  non-object target runtime error), both tiers, qemu x86-64 ×3.

- [x] **Soft/phantom references**
  `SoftRef<T>` traces its target (keeps it alive, unlike weak) until
  memory pressure clears it. Pressure means a hard heap limit: when a
  full collection still leaves the heap over `--gc-max-heap`, every soft
  reference is cleared and the collection re-runs before the heap-limit
  error is considered — softly-held memory is the last to go before
  out-of-memory. With no limit configured there is no pressure signal,
  so softs behave like strong references (stated, not implied away).
  `PhantomRef<T>` is untraced like weak but the target is unrecoverable
  by construction: no `get()` exists (compile error, pinned by a test),
  only `isReclaimed()` — poll-based post-mortem detection, since the
  language has no finalizers or reference queues. The weak-ref JIT
  promptness caveat applies to phantoms identically. Fixed along the
  way: the `--gc-max-heap` limit was only checked when a collection
  happened to trigger, so small programs could exceed it invisibly —
  allocation now flags a safepoint collection whenever the limit is
  crossed. Verified: aura-vm/tests/soft_phantom_refs.rs (6 tests:
  soft-survives-churn-without-pressure ×3 trials, pressure clears soft
  while strong data completes, strong-only overflow still errors,
  phantom reclamation ×3 trials, no-get compile error, inference +
  non-object target errors), both tiers, qemu x86-64 ×3.

**P1 - High Priority**
- [x] **Concurrent GC** (opt-in: `--gc-concurrent` / `Vm::set_gc_concurrent`)
  - [x] Concurrent marking — snapshot-at-the-beginning: the
    stop-the-world cost of a major shrinks to a deep clone of the object
    map (no tracing); the entire mark runs on a background thread over
    the snapshot. Sound because handles are monotonic and never reused:
    unreachable-at-snapshot is unreachable forever, and anything
    allocated after the snapshot is implicitly live.
  - [x] Concurrent sweeping — honestly *incremental*, not off-thread:
    the dead set is applied in bounded chunks (512 objects per safepoint
    slice); deletion must touch the mutator-owned map. Snapshot
    deallocation does happen off-thread.
  - [x] Stop-the-world minimization — measured, not asserted: on the
    churn demo, STW mode ran 56 majors with a 5.05ms max pause;
    concurrent mode's largest STW slice (snapshot + sweep chunks) was
    1.31ms with 1.76ms of marking moved off-thread. Minors stay STW
    (already pause-target-bounded) and keep the nursery collected while
    a cycle runs. Backpressure: at 2x threshold the mutator stalls on
    the marker so the heap cannot balloon; `--gc-max-heap` pressure
    joins the cycle synchronously so the limit stays exact and soft-ref
    clearing keeps its guarantee.
  - [x] Read/write barriers — the write barrier is the existing
    `heap.get_mut` remembered-set gateway (generational); concurrent
    marking needs no additional barrier because the literal snapshot
    *is* the SATB invariant. A read barrier is unnecessary by
    construction: the collector never moves objects. Stated rather than
    implied: reclamation timing under concurrent mode depends on marker
    speed (true of every concurrent collector), which is why the
    deterministic stop-the-world collector remains the default.
  Verified: aura-vm/tests/gc_concurrent.rs (4 tests: STW-vs-concurrent
  result parity with cycle/minor assertions ×3 trials, eventual weak
  reclamation via allocating spin-wait on both tiers, max-heap exactness
  + soft clearing + strong-overflow error under concurrent, stats
  consistency incl. off-thread mark time), suite run ×5 on host for
  flake-shaking, examples crash-sweep 72/72 under `--jit
  --gc-concurrent`, gc_concurrent example byte-identical across all four
  mode combos on host and under qemu, full battery green, qemu x86-64
  ×3.

- [x] **Compaction** (research) — study written:
      docs/research/compaction.md. Conclusion: park indefinitely. The
      heap has no VM-owned memory region to compact — objects live in a
      handle-keyed map and every payload (string/list/map storage) is a
      separate system allocation, so fragmentation and cache placement
      belong to the allocator, not the collector. Never-moving,
      never-reused handles are now the load-bearing invariant under
      weak/soft/phantom liveness, SATB concurrent-marking soundness, and
      the JIT's conservative frame scan; a header-only mark-compact
      behind a handle table would preserve them but buy nearly nothing,
      and true payload compaction is a production-VM memory-subsystem
      rewrite. The real adjacent win, if footprint ever matters, is a
      post-major `shrink_to_fit` pass ("footprint trimming", not
      mark-compact) — documented in the note, deliberately not built
      now.

- [ ] **Finalizers**
  ```aura
  class Resource {
      ~Resource() {
          // cleanup
      }
  }
  ```

**P2 - Medium Priority**
- [ ] **Region-based memory management**
- [ ] **Stack allocation for small objects**
- [ ] **Escape analysis**
- [ ] **Object pooling**

**P3 - Nice to Have**
- [ ] **Incremental GC**
- [ ] **Reference counting (hybrid approach)**
- [ ] **Manual memory management escape hatch**

---

### 2.2 Performance

#### ⏳ Planned

**P0 - Critical**
- [x] **JIT compilation**
  - [x] Tiered compilation (interpreter → baseline JIT; optimizing JIT pending)
  - [x] Constant folding
  - [x] Dead code elimination
  - [ ] Method inlining
  - [ ] Loop optimizations

- [ ] **Bytecode optimizations**
  - [ ] Peephole optimizations
  - [ ] Constant propagation
  - [ ] Common subexpression elimination
  - [ ] Strength reduction

- [x] **Stack machine optimizations**
  - [x] Convert to register-based IR
  - [x] Register allocation
  - [x] SSA form

**P1 - High Priority**
- [ ] **Inline caching**
  - [ ] Monomorphic inline cache
  - [ ] Polymorphic inline cache
  - [ ] Megamorphic fallback

- [ ] **Speculative optimization**
  - [ ] Type profiling
  - [ ] Guarded devirtualization
  - [ ] On-stack replacement (OSR)

- [ ] **Memory optimizations**
  - [ ] Object layout optimization
  - [ ] Field access optimization
  - [ ] Array bounds check elimination

**P2 - Medium Priority**
- [ ] **AOT compilation**
  - [ ] Ahead-of-time compilation to native code
  - [ ] Cross-compilation support
  - [ ] Link-time optimization

- [ ] **Profiling-guided optimization**
  - [ ] Collect runtime profiles
  - [ ] Optimize hot paths
  - [ ] Deoptimization support

**P3 - Nice to Have**
- [ ] **SIMD support**
- [ ] **GPU compute support**
- [ ] **WebAssembly backend**

---

### 2.3 Debugging & Profiling

#### ⏳ Planned

**P0 - Critical**
- [x] **Source-level debugging**
  - [x] Debug information (custom format, not DWARF — DWARF targets
    native codegen; a bytecode VM carries its own tables, as JVM/CPython
    do): each `MethodDef` now holds a sorted `(first_op_index, line)`
    table built from the parser's line marks and a slot-indexed
    local-name table (params first, `this` included, compiler temps
    `__`-prefixed; on slot reuse across scopes the last binding's name
    wins — documented tradeoff). Both tables round-trip through the
    binary module format.
  - [x] Line number mapping — powers three things: debugger stops,
    `aura debug` breakpoints by source line, and line-enriched stack
    traces on runtime errors (`aura run` now prints `at Class.Method
    (line N)` frames on VmError; the program-visible
    `Exception.stackTrace` format is deliberately unchanged to preserve
    tier output parity, since JIT frames have no line mapping).
  - [x] Variable inspection — a real interactive debugger: `aura debug
    <file> [--break N]` with breakpoints (`b`/`d`), `c`/`s`/`n`
    (continue / step into / step over), `locals`, `p <name>`, `bt`, `q`;
    embedders get the same via the `Debugger` trait +
    `Vm::set_debugger`. Debugger runs the interpreter tier only —
    installing one suppresses JIT tier-up (compiled frames cannot stop);
    stated, not implied away. Breakpoints are line-only (no per-file
    disambiguation in multi-file programs yet).
  - [x] Stack traces
  Verified: aura-vm/tests/debugger.rs (6 tests: emitted tables sorted +
  named, breakpoint stop with correct locals, step-into/step-over path
  with depths and backtraces, quit sentinel, exact three-frame error
  trace lines, debugger-suppresses-JIT with jit_compiled_count == 0),
  interactive CLI smoke via piped stdin on host and under qemu, full
  battery green (warnings baseline moved 115 -> 114: `is_jit` was dead
  code on non-x86 hosts until `stack_trace()` began reading it), qemu
  x86-64 ×3.

- [x] **REPL / Interactive mode** — `aura repl` (see §5.1 for the
      design: accumulate-and-replay with output-delta printing, exact
      because the VM is deterministic). Declarations, statements, and
      bare expressions all work; `--jit` runs sessions under the JIT.

**P1 - High Priority**
- [x] **Debugger integration** (see docs/debugging.md)
  - [x] GDB/LLDB support — GDB is Aura-aware via the CPython
    `libpython.py` model: tools/gdb/aura_gdb.py adds `aura-bt`,
    `aura-locals`, `aura-line`, reading the VM's own data structures to
    reconstruct the Aura-level stack, source lines, and named locals at
    any stop — the triage tool for when the VM itself crashes or hangs
    (DAP covers healthy-VM debugging). VM cooperation: no-mangle
    `AURA_CURRENT_VM` registered per run + `Vm.gdb_index`, a flat
    HashMap-free method index; debuginfo comes from the opt-in `gdb`
    cargo profile (`cargo build --profile gdb -p aura-cli`, split
    `.dwo`) — keeping it in default release overflowed this
    environment's disk across the test-binary set, and full-workspace
    debuginfo OOMs the compiler crate. Verified live on host gdb
    and against the x86-64 build via `qemu -g` + gdb-multiarch. Parked
    with rationale in docs/debugging.md: LLDB (different Python API,
    port when needed) and DWARF-for-JIT via the GDB JIT interface
    (large, covers only JIT frames — interpreter frames are heap data no
    unwinder can walk).
  - [x] VS Code debugger adapter — `aura dap` serves the Debug Adapter
    Protocol on stdio (works with VS Code, nvim-dap, any DAP client):
    launch, line breakpoints (before start and while stopped), stops
    with reasons, stackTrace with real lines, scopes/variables, evaluate
    (inspection paths, watch panel/hovers), program output as `output`
    events (protocol owns stdout, so `print` is redirected via
    `Vm::set_output`), exited/terminated. Architecture: protocol loop on
    the main thread, VM on a worker thread, channel-backed `Debugger`
    serving queries from the paused VM. Limits stated in the module doc:
    single thread, line-only breakpoints shared across files, no
    `pause` while running (no async interrupt), running-state breakpoint
    edits apply at the next stop.
  - [x] Breakpoints (source and bytecode) — source lines plus exact
    (method, op-index) bytecode breakpoints that stop mid-line
    (`bb Class.Method <op>` in the CLI, `Vm::add_bytecode_breakpoint`),
    with a `dis` disassembly view marking the current pc.
  - [x] Step execution (step into, over, out) — `Out` added to the
    resume set: run the frame to completion, stop at the caller's next
    line (CLI `o`/`finish`, DAP stepOut).
  - [x] Watch expressions — honest scope: local-rooted inspection paths
    (`p.x`, `xs[0]`, `a.b[2].c`) evaluated live against the paused VM
    via `DebugView::eval_path` — persistent watches re-reported at every
    stop (CLI `w`/`unw`, `Vm::add_watch`) and one-shot evaluation (CLI
    `p`, DAP evaluate). Not an expression evaluator — no calls, no
    arithmetic; stated, not implied. The `Debugger` trait now receives a
    `DebugView` (live read-only window: frames, per-frame locals, path
    evaluation, disassembly) alongside the snapshot.
  Verified: aura-vm/tests/debugger.rs grew to 9 tests (step-out path,
  bytecode breakpoint at exact pc incl. unknown-label rejection, watches
  + path evaluation incl. three error shapes); aura-cli/tests/dap.rs
  drives a complete DAP session against the real binary over stdio
  (breakpoint stop, stack/scopes/variables, evaluate, step, output
  events, clean exit); CLI and DAP sessions also driven by hand on host
  and under qemu x86-64; full battery green (warnings 114 = baseline),
  qemu ×3.

- [ ] **Profiler**
  - [ ] CPU profiling (sampling and instrumentation)
  - [ ] Memory profiling (allocation tracking)
  - [ ] GC profiling (pause times, collection stats)
  - [ ] Flame graphs
  - [ ] Integration with perf/profilers

**P2 - Medium Priority**
- [ ] **Runtime introspection**
  ```aura
  let methods = obj.getClass().getMethods();
  let fields = obj.getClass().getFields();
  ```

- [ ] **Logging framework**
- [ ] **Tracing support**
- [ ] **Crash dumps and analysis**

---

## 3. Compiler Frontend

### 3.1 Parser & Lexer

#### ✅ Completed
- [x] Lexer with token recognition
- [x] Recursive descent parser
- [x] AST generation
- [x] Basic error recovery
- [x] String literals with escape sequences
- [x] Comments (line and block)

#### ⏳ Planned

**P0 - Critical**
- [x] **Error messages**
  - [x] Better syntax error messages — parse errors already carried
    `line N: expected ... (found <token>)`; added targeted top-level
    hints for the classic newcomer mistakes: `fn`/`func`/`def` ("free
    functions are not supported; declare a method inside a class"),
    `struct` ("use `class` or `record`"), `let`/`var` at top level.
  - [x] Error spans and source context — `CompileError::render(files)`
    produces rustc-style output: the diagnostic, then the offending
    source line in a numbered gutter with an underline; the CLI routes
    every compile (run/debug/compile) through it. Spans are
    line-granularity (the typer tracks statements via line marks, not
    columns) so the underline covers the statement — stated, not
    implied. Multi-file attribution: parse/lex errors carry their file
    name; type errors attach context only for single-file programs.
    `Display` is unchanged, so every existing error-needle test still
    pins the same text.
  - [x] Suggestion for common mistakes — did-you-mean via
    length-capped Levenshtein (cap = 1 + len/4, early-exit) for unknown
    types (classes ∪ enums ∪ newtypes), instance/static methods
    (inherited included), fields (inherited included), and variables
    (locals in scope) — `unknown type \`Pont\` — did you mean
    \`Point\`?`. Distant names get no suggestion (pinned by a test).
  - [x] Multiple error reporting — the typer records one error per
    broken method body (the rest of that method is skipped: its
    statements would cascade) and keeps checking the remaining methods
    and classes, capped at 20 diagnostics; single-error programs keep
    the exact old one-line format. Parser and lexer remain first-error
    (statement-level parse recovery is future work — stated).
  Verified: aura-compiler/tests/diagnostics.rs (7 tests: multi-error
  count/locations, single-error format stability, four suggestion kinds
  and no-suggestion-for-distant-names, parser hints, rendered source
  context for type and parse errors, 20-error cap, error-kind
  preservation), full battery green with zero error-needle regressions
  (206 aura-vm / 216 workspace), examples 72/72, warnings 114 =
  baseline, qemu x86-64 ×3.

- [x] **Operator precedence parsing**
  - [x] Precedence climbing or Pratt parser — the eight-layer
    recursive-descent chain (`??` → `||` → `&&` → equality → relational
    → ranges → additive → multiplicative) is now one precedence-climbing
    loop over a table (`parse_binary(min_bp)`), preserving every
    grouping exactly (pinned by a hand-computed precedence-matrix test
    on both tiers plus the full battery: the rewrite touches every
    expression in every program). One behavior improvement: `1..2..3`
    now errors with "range expressions cannot be chained" instead of a
    confusing downstream parse error.
  - [x] Custom operator definitions — new operator symbols starting
    with `|`, `&`, or `^` (the characters no built-in expression
    operator begins with — `<`/`>` belong to generics, so nested-generic
    `>>` is untouched), continuing over the operator charset: `|>`,
    `^^`, `&+`, `|||`, ... Declared exactly like the built-in overloads
    (`float operator|>(Vec2 o)`) and resolved through the same
    machinery (instance method on the left operand's class, same
    public/instance/no-generics rules). All custom operators share one
    documented precedence tier — looser than arithmetic, tighter than
    ranges and comparisons — and are left-associative, so `a + b |> c`
    is `(a + b) |> c` and pipelines chain. Undeclared symbols are a
    clean type error naming the missing `operator<sym>` overload.
    `&&`/`||` stay reserved; single `|` (literal unions) is untouched
    (pinned by a no-whitespace lexing test).
  Verified: aura-vm/tests/custom_operators.rs (4 tests: precedence
  matrix parity, end-to-end custom ops incl. chaining + precedence
  interaction with `+` and `==`, four diagnostics incl. the range-chain
  error and static-operator rejection, lexer compatibility), example
  examples/custom_operators.aura byte-identical both tiers, full battery
  green (210 aura-vm / 220 workspace, examples 73/73, warnings 114 =
  baseline), qemu x86-64 213/0 ×3.

**P1 - High Priority**
- [ ] **Incremental parsing**
  - [ ] Parse only changed portions
  - [ ] Support for IDE integration

- [ ] **Macro system**
  ```aura
  macro assert(condition) {
      if (!condition) {
          throw new AssertionError("Assertion failed");
      }
  }
  ```

**P2 - Medium Priority**
- [ ] **Parser combinators**
- [ ] **Grammar definition language**
- [ ] **Syntax extensions**

---

### 3.2 Type Checking

#### ✅ Completed
- [x] Type inference for local variables (`var`) — NOTE: this box was
      checked long before it was true; `var` did not exist until
      2026-08-14. It now infers from initializers (literals resolve to
      carrier types; nullables narrow like annotated locals; works in
      `for (var x in ...)`); `var` without an initializer or from
      null/void is a compile error.
- [x] Generic type checking
- [x] Type substitution
- [x] Basic type compatibility checks

#### ⏳ Planned

**P0 - Critical**
- [ ] **Type error messages**
  - [ ] Clear, actionable error messages
  - [ ] Type mismatch explanations
  - [ ] Suggested fixes

- [ ] **Flow-sensitive typing**
  - [ ] Null checking
  - [ ] Definite assignment analysis
  - [ ] Unreachable code detection

**P1 - High Priority**
- [ ] **Advanced type inference**
  - [ ] Bidirectional type checking
  - [ ] Local type inference improvements
  - [ ] Better handling of overloading

- [ ] **Type checking for patterns**
- [ ] **Exhaustiveness checking** (for match expressions)

**P2 - Medium Priority**
- [ ] **Type checking optimizations**
  - [ ] Incremental type checking
  - [ ] Caching type information
  - [ ] Parallel type checking

---

### 3.3 Code Generation

#### ✅ Completed
- [x] Bytecode emission
- [x] Constant pool management
- [x] Method and class metadata
- [x] Generic instantiation

#### ⏳ Planned

**P0 - Critical**
- [ ] **Bytecode verification**
  - [ ] Stack depth validation
  - [ ] Type safety verification
  - [ ] Control flow validation

- [ ] **Optimization passes**
  - [ ] Dead code elimination
  - [ ] Constant folding
  - [ ] Peephole optimizations

**P1 - High Priority**
- [ ] **Debug information**
  - [ ] Source line mapping
  - [ ] Local variable information
  - [ ] Type information

- [ ] **Metadata optimization**
  - [ ] Compress metadata
  - [ ] Lazy loading
  - [ ] Deduplication

**P2 - Medium Priority**
- [ ] **Multiple backend support**
  - [ ] LLVM IR backend
  - [ ] WebAssembly backend
  - [ ] JavaScript backend

---

## 4. Standard Library

### 4.1 Core Types

#### ⏳ Planned

**P0 - Critical**
- [ ] **String**
  - [x] String methods on the primitive `string` type (native-backed):
        `Length`, `Substring`, `CharAt`, `Contains`, `StartsWith`,
        `EndsWith`, `IndexOf`, `Split`, `Trim`, `ToUpper`, `ToLower`,
        `Replace`, `ToInt`, `ToFloat` — char-indexed (Unicode scalar values)
  - [ ] String builder (mutable)
  - [ ] String formatting
  - [ ] Regular expressions

- [ ] **Math**
  - [ ] Basic math functions (sin, cos, sqrt, etc.)
  - [ ] Random number generation
  - [ ] Big integer / decimal
  - [ ] Complex numbers

- [ ] **Object**
  - [ ] Base `Object` class
  - [ ] `toString()`, `equals()`, `hashCode()`
  - [ ] `getClass()`, reflection basics

**P1 - High Priority**
- [ ] **Comparable & sorting**
  - [ ] `Comparable<T>` interface
  - [ ] `Comparator<T>` interface
  - [ ] Sorting algorithms

- [ ] **Iterable & iterators**
  ```aura
  interface Iterable<T> {
      Iterator<T> iterator();
  }
  
  interface Iterator<T> {
      bool hasNext();
      T next();
  }
  ```

**P2 - Medium Priority**
- [ ] **Reflection**
  - [ ] Runtime type information
  - [ ] Dynamic method invocation
  - [ ] Attribute/annotation system

---

### 4.2 Collections

#### ✅ Completed (v1, native-backed intrinsics)

Implemented as VM natives behind a generic `NativeCall` opcode; typed
generically by the compiler (`List<T>`, `Map<K, V>`, `Set<T>`). `Map`/`Set`
lookups are hash-indexed (structural `value_hash` buckets over the
insertion-ordered vector, `value_eq` on collision): measured ~10x faster at
4k entries with linear-in-N scaling, verified under interpreter and JIT
(aura-vm/tests/map_perf.rs, stdlib_hash.rs). Removal is still O(n) (index
fixup). Out-of-range/missing-key errors remain VM runtime errors, not
catchable Aura exceptions.

- [x] **List** — `Add`, `Get`, `Set`, `Insert`, `RemoveAt`, `IndexOf`,
      `Contains`, `Clear`, `Count`
  ```aura
  List<int> list = new List<int>();
  list.Add(1);
  int first = list.Get(0);
  ```
- [x] **Map** — `Put`, `Get`, `ContainsKey`, `Remove`, `Keys`, `Values`,
      `Clear`, `Count` (insertion-ordered)
- [x] **Set** — `Add`, `Contains`, `Remove`, `ToList`, `Clear`, `Count`

#### ⏳ Planned

**P0 - Critical**
- [x] Hash-based `Map`/`Set` lookup — `value_hash` bucket index with
      `value_eq` collision resolution; insertion order preserved; float
      `-0.0`/`0.0` hash/eq mismatch fixed as a prerequisite. Measured:
      4k-entry build+lookup dropped from 0.313s to 0.031s (interpreter,
      host), scaling ratio for 8x workload fell from ~33x to ~7-8x.
- [x] Iteration support — `for (T x in list)` / `for (T x in set)` reuse
      the range for-in syntax; the emitter desugars to the proven
      Count/Get native-call loop (Sets iterate a `ToList()` snapshot
      copy), so the JIT path is the one already verified for native calls
      in hot loops. Map iterates via `Keys()`/`Values()`.
      **Mutation during iteration:** the element count is snapshotted at
      loop entry — elements appended to a List during its own iteration
      are not visited, and removing List elements during iteration may
      fail a later `Get` with an index-out-of-range runtime error; Set
      mutations never affect an iteration in progress (snapshot copy).
      Each of these behaviors is pinned by aura-vm/tests/foreach.rs.
- [ ] Sorted variants (TreeMap/TreeSet), LinkedList

**P1 - High Priority**
- [ ] **Queue & Stack**
  - [ ] Queue (FIFO)
  - [ ] Deque (double-ended queue)
  - [ ] Stack (LIFO)
  - [ ] PriorityQueue

- [ ] **Utility classes**
  - [ ] Collections (static utility methods)
  - [ ] Arrays (array utilities)

**P2 - Medium Priority**
- [ ] **Immutable collections**
  - [ ] ImmutableList
  - [ ] ImmutableMap
  - [ ] ImmutableSet

- [ ] **Concurrent collections**
  - [ ] ConcurrentHashMap
  - [ ] ConcurrentQueue
  - [ ] Thread-safe collections

---

### 4.3 I/O & Filesystem

#### ⏳ Planned

**P0 - Critical**
- [ ] **Console I/O**
  ```aura
  print("Enter your name: ");
  string name = Console.ReadLine();
  ```
  - [x] `Console.ReadLine()` (returns `null` at EOF)
  - [ ] `Console.Read()`, formatted output
        (`print`/`println` remain the write path for now)

- [ ] **File I/O**
  ```aura
  string content = File.ReadAllText("data.txt");
  File.WriteAllText("output.txt", content);
  ```
  - [x] Text file reading: `ReadAllText`, `ReadAllLines`
  - [x] Text file writing: `WriteAllText`, `AppendAllText`
  - [x] `Exists`, `Delete` (I/O failures are VM runtime errors, not
        catchable Aura exceptions)
  - [ ] Binary I/O, renaming, directory operations

**P1 - High Priority**
- [ ] **Streams**
  ```aura
  using (var stream = File.openRead("data.bin")) {
      byte[] buffer = new byte[1024];
      int bytesRead = stream.read(buffer);
  }
  ```
  - [ ] InputStream / OutputStream
  - [ ] Buffered streams
  - [ ] Compression streams (gzip, zip)

- [ ] **Path manipulation**
  ```aura
  Path path = new Path("/home/user/file.txt");
  string name = path.getFileName();
  Path parent = path.getParent();
  ```

**P2 - Medium Priority**
- [ ] **Networking**
  - [ ] TCP sockets
  - [ ] UDP sockets
  - [ ] HTTP client
  - [ ] HTTP server (basic)

- [ ] **Serialization**
  - [ ] JSON
  - [ ] XML
  - [ ] Binary serialization

---

### 4.4 Concurrency Primitives

#### ⏳ Planned

**P1 - High Priority**
- [ ] **Thread**
  ```aura
  Thread t = new Thread(() => {
      print("Running in background");
  });
  t.start();
  t.join();
  ```

- [ ] **Synchronization**
  - [ ] Mutex / Lock
  - [ ] Semaphore
  - [ ] Countdown latch
  - [ ] Barrier

- [ ] **Atomic operations**
  ```aura
  AtomicInt counter = new AtomicInt(0);
  counter.incrementAndGet();
  ```

**P2 - Medium Priority**
- [ ] **Concurrent collections** (see 4.2)
- [ ] **Thread pools**
- [ ] **Futures / Promises**

---

## 5. Tooling & Ecosystem

### 5.1 CLI

#### ✅ Completed
- [x] `aura compile <file>` - Compile source to bytecode
- [x] `aura run <file>` - Compile and run source
- [x] Basic error reporting
- [x] Debian package — `packaging/debian/build-deb.sh` builds
      `aura_<version>_<arch>.deb` natively for the host architecture
      (arm64 verified end-to-end: installed via dpkg, `aura` on PATH,
      examples run from `/usr/share/aura/examples`, clean removal).
      Depends are computed from the binary with `dpkg-shlibdeps`;
      docs/examples install under `/usr/share/aura`, copyright and
      changelog under `/usr/share/doc/aura`.
- [x] Windows installer — `packaging/windows/build-msi.sh` builds
      `Aura-<version>-x64.msi` from Linux (mingw cross-compile + msitools
      wixl; no Windows machine or WiX toolset needed). Per-machine
      install to `Program Files\Aura`, appends `bin` to the system PATH
      (removed on uninstall), bundles examples/docs/README/LICENSE,
      permanent UpgradeCode so future versions upgrade in place.

#### ⏳ Planned

**P0 - Critical**
- [x] **Additional commands** — all six, each with real semantics:
  - [x] `aura build` — compiles the project to a binary bytecode module
        (`build/<name>.aurac`, via the existing encode format);
        `aura run <file>.aurac` runs one directly.
  - [x] `aura test` — project test runner: each `tests/*.aura` file is
        its own program (it provides its own `Program.Main`), compiled
        with every source file except the entry; a test passes when Main
        exits without a runtime error, so failures are ordinary throws.
        `aura test <substring>` filters by file name; failing runs print
        their error and fail the command.
  - [x] `aura repl` — real interactive session despite the language
        having no top-level statements: declarations accumulate at top
        level, statements accumulate in a synthesized `Program.Main`,
        and each input recompiles and reruns the session with output
        captured — only the delta beyond the last successful run is
        shown, which is exact because the VM is deterministic. Bare
        expressions wrap in `print(...)`; unbalanced braces continue on
        the next line; failed inputs (compile or runtime) are reported
        and discarded without poisoning the session. `:show` prints the
        synthesized program. Stated limitation: `Console.ReadLine`
        shares stdin with the REPL and reruns replay, so interactive
        input doesn't belong in sessions.
  - [x] `aura fmt` — whitespace normalizer, deliberately scoped to what
        cannot change meaning: leading indentation (4 spaces per bracket
        depth, +1 for continuation lines), trailing whitespace, final
        newline. Interiors of block comments, `"""` strings, and
        multi-line raw strings are verbatim. Safety net: the result is
        re-lexed and its token stream compared to the original — any
        difference aborts without writing (this net caught a real
        multi-line-raw-string bug during development). `--check` mode
        for CI. Verified: formatting all 75 example files is idempotent
        and leaves every program's output byte-identical.
  - [x] `aura lint` — AST-based checks: unused locals (assignment alone
        is not a use; `_`-prefix exempts; parameters, pattern bindings,
        and `using` bindings exempt by design), unreachable code after
        `return`/`throw`/`break`/`continue`, and empty catch blocks.
        Exits non-zero on findings. Shadowing via pattern bindings can
        mask an unused outer local — imprecision errs toward silence,
        never false reports.
  - [x] `aura doc` — Markdown API docs from the AST (public/protected
        members, signatures rendered with modifiers/generics/`throws`)
        plus `///` doc comments, attached via new `line` fields on
        class/method declarations (0 for synthesized ones). Extension
        methods hide their `__self` desugar parameter.

- [ ] **Build configuration**
  - [x] `aura.toml` project file — `[package]` name/version, `[build]`
        entry/sources/output, `[run]` jit/gc defaults (flags override).
        Discovered by walking up from the cwd like Cargo; unknown keys
        are parse errors (serde `deny_unknown_fields`), so typos can't
        silently do nothing. `aura init [name]` scaffolds a project
        (manifest, `src/main.aura`, sample test, `.gitignore`).
  - [ ] Dependencies (needs a package registry — see 5.2)
  - [ ] Build targets
  - [x] Compiler options — `[run]` covers the VM/JIT surface; the
        compiler itself has no configurable options yet.

**P1 - High Priority**
- [x] **Watch mode** — `aura run --watch`: re-compiles and re-runs on
      any source mtime change (1s polling, dependency-free); compile
      and runtime failures are reported and watched through rather than
      fatal. Requires source files (a compiled module has no sources).

- [x] **Verbose output** — `aura run --stats` prints compile time, run
      time, and JIT-compiled method count to stderr (on top of the
      existing `--gc-stats`). A separate `--verbose` was folded into
      `--stats` rather than duplicated.

**P2 - Medium Priority**
- [ ] **Cross-compilation**
  ```bash
  aura build --target wasm
  aura build --target native
  ```

---

### 5.2 Package Management

#### ⏳ Planned

**P1 - High Priority**
- [ ] **Package manager**
  - [x] `aura init` — scaffolds a project (aura.toml, src, tests; see
        §5.1 Build configuration)
  - [ ] `aura add <package>` / `aura remove <pkg>` / `aura update`
        (need a registry and a dependency model first)

- [ ] **Package registry**
  - [ ] Central package repository
  - [ ] Package versioning (semver)
  - [ ] Dependency resolution
  - [ ] Package publishing

**P2 - Medium Priority**
- [ ] **Workspaces**
  - [ ] Multi-package projects
  - [ ] Shared dependencies
  - [ ] Local package references

---

### 5.3 IDE Support

#### ⏳ Planned

**P1 - High Priority**
- [ ] **Language Server Protocol (LSP)**
  - [ ] Diagnostics (errors, warnings)
  - [ ] Code completion
  - [ ] Go to definition
  - [ ] Find references
  - [ ] Rename refactoring
  - [ ] Hover information
  - [ ] Signature help

- [ ] **VS Code extension**
  - [ ] Syntax highlighting
  - [ ] LSP integration
  - [ ] Debug adapter
  - [ ] Snippets

**P2 - Medium Priority**
- [ ] **Other IDE support**
  - [ ] IntelliJ IDEA plugin
  - [ ] Neovim integration
  - [ ] Emacs mode

- [ ] **Code formatting**
  - [ ] `aura fmt` command
  - [ ] Configurable style rules
  - [ ] Auto-format on save

---

## 6. Testing & Quality

#### ⏳ Planned

**P0 - Critical**
- [ ] **Unit test framework**
  ```aura
  @Test
  void testAddition() {
      assert(2 + 2 == 4);
  }
  
  @Test
  void testException() {
      assertThrows<DivideByZeroException>(() => {
          divide(1, 0);
      });
  }
  ```
  - [x] Test runner — `aura test` (file-level: each `tests/*.aura` is a
        program that fails by throwing; see §5.1). The annotation-based
        method-level framework sketched above (`@Test`, assertions,
        `assertThrows`) needs an annotation system and stays open.
  - [ ] Assertions library
  - [x] Test discovery — `tests/` directory scan with name filtering
  - [x] Test reporting — per-test PASS/FAIL with the failure's error,
        summary line, non-zero exit on failure

- [ ] **Test coverage**
  - [ ] Line coverage
  - [ ] Branch coverage
  - [ ] Coverage reports

**P1 - High Priority**
- [ ] **Integration tests**
  - [ ] End-to-end tests
  - [ ] Test fixtures
  - [ ] Mock/stub framework

- [ ] **Benchmarking**
  ```aura
  @Benchmark
  void benchmarkSort() {
      // benchmark code
  }
  ```
  - [ ] Benchmark runner
  - [ ] Performance regression detection
  - [ ] Statistical analysis

**P2 - Medium Priority**
- [ ] **Property-based testing**
  ```aura
  @Property
  void testCommutative(int a, int b) {
      assert(a + b == b + a);
  }
  ```

- [ ] **Fuzzing**
  - [ ] Input fuzzing
  - [ ] Mutation testing

---

## 7. Documentation

#### ⏳ Planned

**P0 - Critical**
- [ ] **Language specification**
  - [ ] Formal grammar
  - [ ] Type system rules
  - [ ] Semantics
  - [ ] Memory model

- [ ] **Tutorial**
  - [ ] Getting started guide
  - [ ] Language basics
  - [ ] Advanced topics
  - [ ] Examples and exercises

**P1 - High Priority**
- [ ] **API documentation**
  - [ ] Standard library docs
  - [ ] Code examples
  - [ ] Search functionality

- [ ] **Style guide**
  - [ ] Naming conventions
  - [ ] Code formatting
  - [ ] Best practices

**P2 - Medium Priority**
- [ ] **Migration guides**
  - [ ] From other languages (C#, Java, Rust)
  - [ ] Version migration

- [ ] **Design documents**
  - [ ] Architecture decisions
  - [ ] RFCs for new features
  - [ ] Performance analysis

---

## 8. Future Considerations

### Experimental Features

**P3 - Research / Experimental**
- [ ] **Effect system** (algebraic effects)
- [ ] **Gradual typing** (mix static and dynamic)
- [ ] **Logic programming** (Prolog-like features)
- [ ] **Functional reactive programming**
- [ ] **Quantum computing primitives** (research)

### Platform Support

**P2 - Medium Priority**
- [ ] **Cross-platform**
  - [x] Windows support (x64) — full toolchain including the JIT. The
        executable-memory layer (`aura-vm/src/jit/x64/mem.rs`) gained a
        `VirtualAlloc`/`VirtualProtect`/`VirtualFree` path declared
        directly against `kernel32` (keeping the crate's zero-dependency
        property, the Windows analogue of the raw-syscall Linux path);
        the three JIT ABI boundaries (entry pointer, helper dispatcher,
        overflow raiser) are declared `extern "sysv64"`, so generated
        code speaks System V on every OS and Rust marshals the
        difference — zero per-OS codegen. Prologue frames larger than a
        page are now allocated page-by-page with probe stores, because
        Windows faults if `sub rsp` skips its stack guard page (emitted
        on all OSes to keep codegen identical; harmless on Linux). On
        any other OS, executable-memory allocation fails cleanly and
        tier-up falls back to the interpreter permanently. Shipped as an
        MSI installer: `packaging/windows/build-msi.sh`
        cross-compiles `x86_64-pc-windows-gnu` and builds
        `Aura-<version>-x64.msi` with msitools' wixl entirely from
        Linux — per-machine install, appends `bin` to the system PATH,
        bundles examples/docs/LICENSE, stable UpgradeCode with major
        upgrades configured (details in §5.1). Honest limits: the Windows exe is
        cross-built and its MSI tables verified with msitools
        (file/upgrade/environment tables inspected, layout extracted
        and checked); the JIT ABI/probe changes are verified by the
        full battery under qemu x86-64 on Linux, but the binary has not
        been executed on a real Windows machine from this environment.
  - [ ] macOS support (the JIT would need an `mmap`-via-libc path and
        `MAP_JIT`/`pthread_jit_write_protect` handling; interpreter
        likely works today, unverified)
  - [x] Linux support (primary) — packaged as a `.deb`
        (`packaging/debian/build-deb.sh`, per-architecture; arm64
        verified installed-and-running, amd64 buildable on any x86-64
        machine the same way)
  - [ ] BSD support

- [ ] **WebAssembly**
  - [ ] Compile to WASM
  - [ ] Run in browsers
  - [ ] Node.js support

**P3 - Nice to Have**
- [ ] **Embedded systems**
  - [ ] Bare-metal support
  - [ ] RTOS support
  - [ ] Microcontroller targets

- [ ] **Mobile platforms**
  - [ ] iOS support
  - [ ] Android support

### Interoperability

**P1 - High Priority**
- [ ] **FFI (Foreign Function Interface)**
  ```aura
  @extern("c")
  int printf(string format, ...);
  ```
  - [ ] Call C functions
  - [ ] Call Rust functions
  - [ ] Expose Aura functions to C/Rust

**P2 - Medium Priority**
- [ ] **JVM interop**
  - [ ] Call Java/Kotlin code
  - [ ] Use Java libraries

- [ ] **.NET interop**
  - [ ] Call C# code
  - [ ] Use .NET libraries

- [ ] **JavaScript interop**
  - [ ] Call JS from Aura
  - [ ] Call Aura from JS
  - [ ] WebAssembly bridge

---

## Priority Legend

- **P0 - Critical**: Must have for basic usability. Blocks adoption.
- **P1 - High Priority**: Important for production use. High impact.
- **P2 - Medium Priority**: Nice to have. Improves experience.
- **P3 - Nice to Have**: Experimental or future considerations.

## Status Legend

- ✅ **Completed**: Feature is implemented and working
- 🔄 **In Progress**: Currently being implemented
- ⏳ **Planned**: Not started yet, but planned
- ❌ **Blocked**: Cannot proceed due to dependencies
- 🔬 **Research**: Needs investigation or prototyping

---

## Contributing

See `CONTRIBUTING.md` for guidelines on how to contribute to Aura.

## License

See `LICENSE` for licensing information.
