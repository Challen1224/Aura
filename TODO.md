# Aura Language TODO

> **Status:** Core language and compiler: usable. VM with baseline x86-64
> JIT: implemented; static, virtual, and super calls all tier up. GC now
> actually runs: threshold-triggered, safepoint-based mark-and-sweep,
> collecting under both tiers (JIT frames scanned conservatively — see
> §2.1; generational/weak-ref work unstarted). Stdlib: collections
> (`List`/`Map`/`Set`), string methods, `Console.ReadLine`, and text `File`
> I/O are implemented and verified under both interpreter and JIT, including
> mid-run tier transitions; still missing: networking, async, reflection,
> binary I/O, string builder/formatting (see §4). Tooling: early.  
> **Last Updated:** 2026-08-13  
> **Current Version:** 0.27.0

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
- [ ] **Task parallelism**
  ```aura
  Task<int> computeAsync(int x) {
      return x * x;
  }
  
  let result = await computeAsync(42);
  ```

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
- [ ] **Generational GC**
  - [ ] Young generation (nursery)
  - [ ] Old generation (tenured)
  - [ ] Minor GC (young gen only)
  - [ ] Major GC (full heap)
  - [ ] Write barriers

- [ ] **GC tuning**
  - [ ] Configurable heap size
  - [ ] GC pause time targets
  - [ ] Throughput vs latency tradeoffs
  - [ ] GC statistics and metrics

- [ ] **Weak references**
  ```aura
  WeakRef<Object> weak = new WeakRef(obj);
  if (weak.isAlive()) {
      let obj = weak.get();
  }
  ```

- [ ] **Soft/phantom references**

**P1 - High Priority**
- [ ] **Concurrent GC**
  - [ ] Concurrent marking
  - [ ] Concurrent sweeping
  - [ ] Stop-the-world minimization
  - [ ] Read/write barriers

- [ ] **Compaction**
  - [ ] Mark-compact algorithm
  - [ ] Reduce fragmentation
  - [ ] Improve cache locality

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
- [ ] **Source-level debugging**
  - [ ] Debug information (DWARF or custom format)
  - [ ] Line number mapping
  - [ ] Variable inspection
  - [x] Stack traces

- [ ] **REPL / Interactive mode**
  ```bash
  $ aura repl
  Aura 0.1.0
  >>> let x = 42;
  >>> x + 10
  52
  ```

**P1 - High Priority**
- [ ] **Debugger integration**
  - [ ] GDB/LLDB support
  - [ ] VS Code debugger adapter
  - [ ] Breakpoints (source and bytecode)
  - [ ] Step execution (step into, over, out)
  - [ ] Watch expressions

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
- [ ] **Error messages**
  - [ ] Better syntax error messages
  - [ ] Error spans and source context
  - [ ] Suggestion for common mistakes
  - [ ] Multiple error reporting

- [ ] **Operator precedence parsing**
  - [ ] Precedence climbing or Pratt parser
  - [ ] Custom operator definitions

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

#### ⏳ Planned

**P0 - Critical**
- [ ] **Additional commands**
  ```bash
  aura build          # Build project
  aura test           # Run tests
  aura repl           # Start REPL
  aura fmt <file>     # Format code
  aura lint <file>    # Lint code
  aura doc            # Generate documentation
  ```

- [ ] **Build configuration**
  - [ ] `aura.toml` or `aura.json` project file
  - [ ] Dependencies
  - [ ] Build targets
  - [ ] Compiler options

**P1 - High Priority**
- [ ] **Watch mode**
  ```bash
  aura run --watch    # Re-run on file changes
  ```

- [ ] **Verbose output**
  ```bash
  aura run --verbose  # Show compilation details
  aura run --stats    # Show performance stats
  ```

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
  ```bash
  aura init           # Initialize new project
  aura add <package>  # Add dependency
  aura remove <pkg>   # Remove dependency
  aura update         # Update dependencies
  ```

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
  - [ ] Test runner
  - [ ] Assertions library
  - [ ] Test discovery
  - [ ] Test reporting

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
  - [ ] Windows support
  - [ ] macOS support
  - [ ] Linux support (primary)
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
