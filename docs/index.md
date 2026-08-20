# The Aura Documentation

Aura is a statically-typed, object-oriented programming language in the C# /
Java tradition, with a modern type system — non-nullable references, pattern
matching, sum types, generics with constraints and variance, and first-class
functions — running on a bytecode VM with a generational garbage collector
and a tiered x86-64 JIT. It ships as a single `aura` binary for Linux and
Windows.

```aura
class Program {
    static void Main() {
        List<int> primes = new List<int>();
        primes.Add(2);
        primes.Add(3);
        primes.Add(5);
        for (int p in primes) {
            print("prime: {p}");
            println();
        }
    }
}
```

## Where to start

| I want to... | Read |
|---|---|
| Install Aura and run my first program | [Getting Started](getting-started.md) |
| Learn the language from the ground up | [Language Guide](#the-language-guide) |
| Look up a CLI command or `aura.toml` key | [Tooling Reference](tooling.md) |
| Debug a program (CLI or VS Code) | [Debugging](debugging.md) |

## The Language Guide

The guide is a course: each chapter builds on the previous one, and every
complete program in it is verified against the current compiler and VM by
`tools/check-doc-examples.sh`, so what you read is what runs.

1. **[Language Basics](guide/01-language-basics.md)** — program structure,
   types and variables, operators, strings and interpolation, control flow,
   ranges, expression blocks.
2. **[Classes and Objects](guide/02-classes-and-objects.md)** — classes,
   constructors, visibility, inheritance, interfaces, properties, records,
   static classes, nested classes, extension methods.
3. **[The Type System](guide/03-the-type-system.md)** — nullable types and
   narrowing, type guards, tuples, enums and sum types, aliases, newtypes,
   literal unions, generics with constraints and variance.
4. **[Functions and Lambdas](guide/04-functions-and-lambdas.md)** —
   `Func`/`Action` types, lambdas and captures, operator overloading,
   custom operators.
5. **[Pattern Matching](guide/05-pattern-matching.md)** — `match`
   expressions, pattern kinds, guards, `if let`, nested patterns.
6. **[Error Handling](guide/06-error-handling.md)** — exceptions,
   `try`/`catch`/`finally`, `using`, exception chaining, checked
   exceptions, `Result` types and the `?` operator.
7. **[Async and Tasks](guide/07-async-and-tasks.md)** — `async`/`await`,
   the cooperative task model, `Tasks.all` and `Tasks.race`.
8. **[The Standard Library](guide/08-standard-library.md)** — console I/O,
   strings, collections, files, reference types for interacting with the GC.

## How Aura runs your code

```
source (.aura)  →  lexer → parser → type checker → emitter  →  bytecode module
                                                                    │
                                              VM: interpreter ──────┤
                                                  x86-64 JIT (hot methods)
```

Programs start at `class Program { static void Main() }`. Methods run on the
bytecode interpreter first; on x86-64 hosts, methods that get hot are
compiled to native code (`--jit`). Memory is managed by a generational
garbage collector with tunable behavior — see the
[Tooling Reference](tooling.md#gc-flags).

## Design positions worth knowing up front

* **Null is opt-in.** A `string` can never be null; a `string?` can, and the
  compiler makes you deal with it before you touch its members.
* **Everything lives in a class.** There are no free functions or top-level
  statements; `static class` is the namespace idiom.
* **Exceptions are unchecked by default.** A method may opt in to checked
  exceptions with a `throws` clause, which binds its *callers*.
* **Value semantics are explicit.** Classes compare by reference; `record`
  types compare by value.
* **Determinism is a feature.** The VM is single-threaded; async is
  cooperative with FIFO scheduling, so concurrent programs interleave the
  same way every run.
