# 1. Language Basics

[Guide index](../index.md#the-language-guide) · Next:
[Classes and Objects](02-classes-and-objects.md)

## Program structure

An Aura program is a set of top-level declarations — classes, interfaces,
enums, records, and type declarations. There are no free functions and no
top-level statements; execution starts at the static `Main` method of a
class named `Program`:

```aura
class Program {
    static void Main() {
        print("running");
        println();
    }
}
```

Statements end with `;`. Blocks are brace-delimited and introduce scope.
Comments come in three forms:

```aura
class Program {
    // Line comment.
    /* Block
       comment. */
    /// Doc comment: attaches to the following declaration and is
    /// rendered by `aura doc`.
    static void Main() {}
}
```

Within a class, a static method can call a sibling by bare name
(`Helper(x)`) or qualified (`Program.Helper(x)`); from another class the
qualified form is required.

## Types

Aura is statically typed: every expression has a type known at compile
time, and errors are reported before anything runs.

| Category | Types |
|---|---|
| Signed integers | `int8`, `int16`, `int32` (alias `int`), `int64` |
| Unsigned integers | `uint8`, `uint16`, `uint32`, `uint64` |
| Floating point | `float32`, `float64` (alias `float`) |
| Other primitives | `bool`, `char` (a Unicode scalar), `string` |
| No value | `void` (return type only) |

Integer literals coerce implicitly to any integer type whose range fits the
value, and integer types widen implicitly when no information can be lost:

```aura
class Program {
    static void Main() {
        int8 small = 5;            // literal fits: fine
        int64 big = 3000000000;    // wider than int32: fine as int64
        int16 w = 300;
        int32 widened = w;         // implicit widening
        print(widened);
        println();
    }
}
```

Narrowing never happens implicitly. Two explicit cast forms exist, C-style
and `as`, and both **wrap** on overflow:

```aura
class Program {
    static void Main() {
        int8 a = (int8)(300);   // wraps to 44
        int8 b = 300 as int8;   // same
        print(a);
        print(" ");
        print(b);
        println();
    }
}
```

## Variables

Declare with an explicit type, or with `var` to infer the type from the
initializer:

```aura
class Program {
    static void Main() {
        int count = 3;
        var name = "ada";      // string
        var ratio = 2.5;       // float64
        print("{name} x{count} @ {ratio}");
        println();
    }
}
```

`var` requires an initializer, and the initializer must have a real type —
`var x = null;` is a compile error. Inner scopes may shadow outer
variables; each declaration is a distinct variable with proper scoping.

A variable named `_` (or prefixed with `_`) signals "intentionally unused"
— `aura lint` will not flag it.

## Strings and characters

String literals interpolate any expression inside braces:

```aura
class Program {
    static void Main() {
        int n = 6;
        print("{n} * 7 = {n * 7}");
        println();
    }
}
```

Three literal forms cover the awkward cases — escapes, verbatim text, and
multi-line text:

```aura
class Program {
    static void Main() {
        string escaped = "line one\nline two";
        string raw = r"C:\new\dir";        // backslashes verbatim
        string multi = """first
second""";                                  // spans lines
        print(escaped); println();
        print(raw); println();
        print(multi); println();
    }
}
```

`char` literals use single quotes and support escapes and Unicode scalars:
`'a'`, `'\n'`, `'\''`, `'\u{2764}'`. Strings are indexed by character (not
byte); the built-in string methods are covered in
[chapter 8](08-standard-library.md#strings).

## Operators

In order from loosest to tightest binding:

| Operators | Meaning |
|---|---|
| `??` | null coalescing ([chapter 3](03-the-type-system.md#nullable-types)) |
| `\|\|`, `&&` | logical or / and — both short-circuit |
| `==`, `!=` | equality |
| `<`, `<=`, `>`, `>=`, `is` | comparison, type test |
| `..`, `..=` | ranges |
| custom operators (`\|>`, `^^`, ...) | one shared tier — [chapter 4](04-functions-and-lambdas.md#custom-operators) |
| `+`, `-` | additive |
| `*`, `/`, `%` | multiplicative |

All binary operators are left-associative, and ranges do not chain
(`1..2..3` is a compile error).
Unary operators are `!` (logical not) and `-` (negation); the ternary
conditional `cond ? a : b` is also available:

```aura
class Program {
    static void Main() {
        int x = 17;
        string parity = x % 2 == 0 ? "even" : "odd";
        print(parity);
        println();
    }
}
```

Equality on class instances is reference identity unless the class overloads
`operator==`; `record` types compare by value ([chapter
2](02-classes-and-objects.md#records)).

## Control flow

`if`/`else`, `while`, `do`/`while`, and the three-part `for` are as in C:

```aura
class Program {
    static void Main() {
        for (int i = 1; i <= 5; i = i + 1) {
            if (i % 2 == 0) {
                print("{i} even ");
            } else {
                print("{i} odd ");
            }
        }
        println();

        int n = 3;
        while (n > 0) {
            print(n);
            n = n - 1;
        }
        println();
    }
}
```

### Ranges and `for`-`in`

`for (T x in a..b)` iterates a range — `..` excludes the end, `..=` includes
it — and the same syntax iterates collections
([chapter 8](08-standard-library.md#iteration)):

```aura
class Program {
    static void Main() {
        for (int i in 1..4) { print(i); }    // 123
        println();
        for (int i in 1..=4) { print(i); }   // 1234
        println();
        for (var i in 2 + 1..2 * 3) { print(i); }  // 345 — bounds are expressions
        println();
    }
}
```

### `break`, `continue`, and labels

Loops may be labeled; `break label` and `continue label` act on the labeled
loop rather than the innermost one:

```aura
class Program {
    static void Main() {
        outer: for (int i = 0; i < 5; i = i + 1) {
            for (int j = 0; j < 5; j = j + 1) {
                if (i * j > 6) {
                    break outer;
                }
                print("{i}{j} ");
            }
        }
        println();
    }
}
```

## Expression blocks

A block in expression position evaluates to its final expression — a tidy
way to scope intermediate work:

```aura
class Program {
    static void Main() {
        int hypotenuseSquared = {
            int a = 3;
            int b = 4;
            a * a + b * b
        };
        print(hypotenuseSquared);
        println();
    }
}
```

Note the final expression has no trailing semicolon — that is what makes it
the block's value.

---

Next: [Classes and Objects](02-classes-and-objects.md), where the rest of a
program's structure lives.
