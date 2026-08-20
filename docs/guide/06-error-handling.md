# 6. Error Handling

Previous: [Pattern Matching](05-pattern-matching.md) ·
[Guide index](../index.md#the-language-guide) · Next:
[Async and Tasks](07-async-and-tasks.md)

Aura offers two complementary styles: **exceptions** for failures you
usually don't handle locally, and **`Result` sum types with the `?`
operator** for failures that are part of a function's contract. Both are
first-class; pick per situation.

## Exceptions

Every exception derives from the built-in `Exception` class, which carries
three fields: `message` (string), `stackTrace` (string, filled in when
thrown), and `cause` (`Exception?`, for chaining). Define your own by
subclassing:

```aura
class ParseError : Exception {
    ParseError(string what) {
        this.message = "cannot parse {what}";
    }
}

class Program {
    static int Parse(string s) {
        if (s != "42") {
            throw new ParseError(s);
        }
        return 42;
    }

    static void Main() {
        try {
            print(Program.Parse("42"));
            println();
            Program.Parse("banana");
        } catch (ParseError e) {
            print("recovered: {e.message}");
            println();
        } finally {
            print("cleanup ran");
            println();
        }
    }
}
```

`catch` clauses are tried in order; the first whose type matches (exactly
or as a base class — `catch (Exception e)` catches everything) wins.
`finally` runs on every exit path: normal completion, a caught exception,
or one still propagating.

An exception nobody catches terminates the program with the message and a
stack trace; `e.stackTrace` exposes the same trace to handlers.

## Exception chaining

Wrap-and-rethrow keeps the original failure attached through `cause`:

```aura
class DbError : Exception {
    DbError(string m) { this.message = m; }
}

class AppError : Exception {
    AppError(string m, Exception c) {
        this.message = m;
        this.cause = c;
    }
}

class Program {
    static void Main() {
        try {
            try {
                throw new DbError("connection refused");
            } catch (DbError e) {
                throw new AppError("startup failed", e);
            }
        } catch (AppError outer) {
            print(outer.message);
            println();
            print(outer.cause?.message ?? "no cause");
            println();
        }
    }
}
```

An *uncaught* chained exception prints `caused by:` lines for each link.
`cause` is an ordinary `Exception?`, so the nullable rules from
[chapter 3](03-the-type-system.md#nullable-types) apply when reading it.

## `using`: resource cleanup

`using (resource) { ... }` calls the resource's `Dispose()` on the way out
— on normal exit *and* when the body throws:

```aura
class TempFile {
    string name;
    TempFile(string name) { this.name = name; }
    void Dispose() {
        print("disposed {this.name}");
        println();
    }
}

class Program {
    static void Main() {
        using (new TempFile("scratch.txt")) {
            print("working");
            println();
        }
        print("after block");
        println();
    }
}
```

## Checked exceptions (opt-in)

Exceptions are unchecked by default — any method may `throw` freely. A
method that declares `throws E1, E2` opts into a checked contract that
binds its **callers**: each declared exception must be caught by an
enclosing `try` (of the type or a supertype) or re-declared in the caller's
own `throws` clause, passing the obligation up:

```aura
class IOError : Exception {
    IOError(string m) { this.message = m; }
}

class Program {
    static string ReadConfig(string path) throws IOError {
        if (path == "") {
            throw new IOError("empty path");
        }
        return "config from {path}";
    }

    static void Main() {
        try {
            print(Program.ReadConfig("app.toml"));
            println();
        } catch (IOError e) {
            print(e.message);
            println();
        }
    }
}
```

Calling a `throws` method without catching or re-declaring is a compile
error. Overrides and interface implementations may not *add* throws their
base declaration lacks, so the contract survives virtual dispatch.

## Result types and the `?` operator

For failures that callers should handle as values, return a `Result` sum
type ([chapter 3](03-the-type-system.md#enums-and-sum-types)). The `?`
suffix unwraps an `Ok` — or returns the `Err` from the enclosing function
immediately, eliminating the match ladder:

```aura
type Result<T, E> = Ok(T) | Err(E);

class Program {
    static Result<int, string> SafeDivide(int a, int b) {
        if (b == 0) {
            return Err("division by zero");
        }
        return Ok(a / b);
    }

    static Result<int, string> Halve(int a, int b) {
        int q = SafeDivide(a, b)?;    // Err short-circuits out
        return Ok(q * 2);
    }

    static void Main() {
        print(match (Program.Halve(10, 2)) {
            Ok(v) => "ok {v}",
            Err(m) => "err {m}"
        });
        println();
        print(match (Program.Halve(10, 0)) {
            Ok(v) => "ok {v}",
            Err(m) => "err {m}"
        });
        println();
    }
}
```

**Choosing:** exceptions shine when the failure is rare and the recovery
point is far away (I/O gone wrong, invariant violations); `Result` shines
when failure is an expected outcome the immediate caller must think about
(parsing, validation). Aura deliberately supports both without forcing a
translation layer.

---

Next: [Async and Tasks](07-async-and-tasks.md).
