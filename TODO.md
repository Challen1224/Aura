# Aura Language TODO

> **Status:** Production Ready  
> **Last Updated:** 2026-08-10  
> **Current Version:** 1.0.0

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
- [x] Basic primitive types: `int`, `float`, `bool`, `string`, `void`
- [x] Null reference type
- [x] Type inference for local variables
- [x] Static type checking
- [x] Enum types with pattern matching
- [x] Tuple types with creation, access, and destructuring
- [x] String interpolation: `"Hello {name}"`

#### ⏳ Planned

**P0 - Critical**
- [ ] **Numeric type hierarchy**
  - [ ] `int8`, `int16`, `int32`, `int64` (signed integers)
  - [ ] `uint8`, `uint16`, `uint32`, `uint64` (unsigned integers)
  - [ ] `float32`, `float64` (explicit precision)
  - [ ] Type coercion rules and conversions
  - [ ] Overflow/underflow checking (configurable)

- [ ] **Character and string types**
  - [ ] `char` type (Unicode scalar value)
  - [ ] Raw string literals: `r"..."`
  - [ ] Multi-line strings: `"""..."""`

- [ ] **Union types / Sum types**
  ```aura
  type Result<T, E> = Ok(T) | Err(E);
  ```

**P1 - High Priority**
- [ ] **Structural typing**
  - [ ] Duck typing for interfaces
  - [ ] Type aliases: `type UserId = int;`
  - [ ] Newtype pattern support

- [ ] **Type annotations**
  - [ ] Nullable types: `int?`
  - [ ] Non-null assertions: `value!`
  - [ ] Type guards and type narrowing

- [ ] **Literal types**
  ```aura
  type Direction = "north" | "south" | "east" | "west";
  ```

**P2 - Medium Priority**
- [ ] **Advanced type features**
  - [ ] Dependent types (research)
  - [ ] Refinement types
  - [ ] Phantom types
  - [ ] Type-level computation

- [ ] **Type inference improvements**
  - [ ] Hindley-Milner style inference
  - [ ] Better error messages for type mismatches
  - [ ] Type hole suggestions

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
  - [ ] Constructor chaining (no constructor syntax yet)
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
  - [ ] `internal` (module-scoped)

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

- [ ] **Static classes / namespaces**
  ```aura
  static class Math {
      static float PI = 3.14159;
      static int max(int a, int b) { }
  }
  ```

- [ ] **Properties**
  ```aura
  class Person {
      string Name { get; set; }
      int Age { get; private set; }
  }
  ```

**P2 - Medium Priority**
- [ ] **Operator overloading**
  ```aura
  class Vector {
      operator+(Vector other) -> Vector { }
      operator==(Vector other) -> bool { }
  }
  ```

- [ ] **Extension methods**
  ```aura
  extension StringExtensions on string {
      bool isPalindrome() { }
  }
  ```

- [ ] **Nested classes**
  ```aura
  class Outer {
      class Inner { }
  }
  ```

**P3 - Nice to Have**
- [ ] **Mixin / Trait system** (alternative or complement to interfaces)
- [ ] **Partial classes** (split class definition across files)
- [ ] **Record classes** (immutable data classes with value semantics)

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

#### ✅ Completed
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
- [ ] **Pattern matching enhancements**
  - [x] Enum variant patterns
  - [x] Range patterns
  - [x] Pattern guards with complex expressions
  - [ ] Nested patterns

- [ ] **Null conditional**

**P2 - Medium Priority**
- [x] **Labeled break/continue**
  ```aura
  outer: for (int i = 0; i < 10; i = i + 1) {
      for (int j = 0; j < 10; j = j + 1) {
          if (condition) break outer;
      }
  }
  ```

- [ ] **Guard clauses**
  ```aura
  if let Some(value) = optional {
      // use value
  }
  ```

- [ ] **Expression blocks**
  ```aura
  let result = {
      let x = computeX();
      let y = computeY();
      x + y
  };
  ```

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
- [ ] **Generic constraints**
  ```aura
  class Comparable<T> where T : IComparable<T> { }
  class Numeric<T> where T : int | float { }
  class DefaultConstructible<T> where T : new() { }
  ```

- [ ] **Variance annotations**
  ```aura
  interface IEnumerable<out T> { }  // Covariant
  interface IComparer<in T> { }     // Contravariant
  ```

- [ ] **Generic type inference**
  ```aura
  let box = new Box(42);  // Infer Box<int>
  ```

**P1 - High Priority**
- [ ] **Higher-kinded types** (research)
  ```aura
  trait Functor<F<_>> {
      F<B> map<A, B>(F<A> fa, Func<A, B> f);
  }
  ```

- [ ] **Generic methods with multiple type parameters**
  ```aura
  T2 transform<T1, T2>(T1 value, Func<T1, T2> converter) { }
  ```

- [ ] **Generic constraints with multiple bounds**
  ```aura
  class Repository<T> where T : Entity, ICloneable { }
  ```

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
- [ ] **Checked exceptions** (optional)
  ```aura
  void readFile(string path) throws IOException { }
  ```

- [ ] **Result type** (alternative to exceptions)
  ```aura
  Result<int, string> divide(int a, int b) {
      if (b == 0) return Err("Division by zero");
      return Ok(a / b);
  }
  ```

**P1 - High Priority**
- [ ] **Exception chaining**

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
- [ ] **Async/await**
  ```aura
  async Task<string> fetchData(string url) {
      let response = await http.get(url);
      return response.body;
  }
  ```

- [ ] **Fibers / green threads**
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
- [x] Mark-and-sweep garbage collector
- [x] Object allocation
- [x] Reference tracking
- [x] Basic GC triggering (threshold-based)

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
- [ ] **JIT compilation**
  - [ ] Tiered compilation (interpreter → baseline JIT → optimizing JIT)
  - [ ] Method inlining
  - [ ] Constant folding
  - [ ] Dead code elimination
  - [ ] Loop optimizations

- [ ] **Bytecode optimizations**
  - [ ] Peephole optimizations
  - [ ] Constant propagation
  - [ ] Common subexpression elimination
  - [ ] Strength reduction

- [ ] **Stack machine optimizations**
  - [ ] Convert to register-based IR
  - [ ] Register allocation
  - [ ] SSA form

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
  - [ ] Stack traces

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
- [x] Type inference for local variables
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
  - [ ] Immutable string class
  - [ ] String builder (mutable)
  - [ ] String formatting
  - [ ] Regular expressions
  - [ ] Unicode support

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

#### ⏳ Planned

**P0 - Critical**
- [ ] **List**
  ```aura
  List<int> list = new List<int>();
  list.add(1);
  list.add(2);
  let first = list.get(0);
  ```
  - [ ] ArrayList (dynamic array)
  - [ ] LinkedList
  - [ ] Common operations (add, remove, get, set, size)

- [ ] **Map**
  ```aura
  Map<string, int> map = new Map<string, int>();
  map.put("key", 42);
  let value = map.get("key");
  ```
  - [ ] HashMap
  - [ ] TreeMap (sorted)
  - [ ] LinkedHashMap (insertion order)

- [ ] **Set**
  ```aura
  Set<int> set = new Set<int>();
  set.add(1);
  set.add(2);
  ```
  - [ ] HashSet
  - [ ] TreeSet (sorted)

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
  string name = readLine();
  print("Hello, " + name + "!");
  ```
  - [ ] `Console.read()`, `Console.readLine()`
  - [ ] `Console.write()`, `Console.writeLine()`
  - [ ] Formatted output

- [ ] **File I/O**
  ```aura
  string content = File.readAllText("data.txt");
  File.writeAllText("output.txt", content);
  ```
  - [ ] File reading (text and binary)
  - [ ] File writing (text and binary)
  - [ ] File existence, deletion, renaming
  - [ ] Directory operations

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
