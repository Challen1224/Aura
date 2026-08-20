# 5. Pattern Matching

Previous: [Functions and Lambdas](04-functions-and-lambdas.md) ·
[Guide index](../index.md#the-language-guide) · Next:
[Error Handling](06-error-handling.md)

## The `match` expression

`match` inspects a value against a series of arms and evaluates the body of
the first arm whose pattern fits. It is an *expression* — the arms' bodies
must agree on a type, and the result is a value:

```aura
class Program {
    static void Main() {
        int score = 87;
        string grade = match (score) {
            90..=100 => "A",
            80..=89 => "B",
            70..=79 => "C",
            * => "F"
        };
        print(grade);
        println();
    }
}
```

Arms are separated by commas or newlines. `*` is the wildcard — it matches
anything, and belongs last.

## Pattern kinds

| Pattern | Matches |
|---|---|
| `42`, `1.5`, `true`, `"text"` | that literal value |
| `1..=5`, `1..5` | a value in the range (inclusive / exclusive end) |
| `Color.Red` | that enum variant |
| `Shape.Circle(r)` | the variant, binding its fields |
| `Ok(v)`, `Err(e)` | sum-type variants, bare |
| `Point(0, y)` | a record, positionally |
| `name` | anything, bound to `name` |
| `*` | anything, bound to nothing |
| `null` | null |

Patterns nest — a variant's field position takes another full pattern, so
specific cases peel off before general ones:

```aura
enum Shape {
    Point(int x, int y),
    Circle(float radius),
    Rect(int w, int h)
}

class Program {
    static string Describe(Shape s) {
        return match (s) {
            Shape.Point(0, 0) => "origin",
            Shape.Point(x, 0) => "on the x-axis at {x}",
            Shape.Point(x, y) => "point ({x}, {y})",
            Shape.Circle(1.0) => "unit circle",
            Shape.Circle(r) => "circle of radius {r}",
            Shape.Rect(w, h) if w == h => "square of side {w}",
            Shape.Rect(w, h) => "rect {w}x{h}"
        };
    }

    static void Main() {
        print(Program.Describe(Shape.Point(3, 0)));
        println();
        print(Program.Describe(Shape.Rect(5, 5)));
        println();
    }
}
```

Two refinements appear above:

* **Guards** — `pattern if condition => body` runs the arm only when the
  condition (which may use the pattern's bindings) holds; otherwise
  matching continues to the next arm.
* **Alternatives** — one arm may take several patterns separated by `|`:
  `1 | 2 | 3 => "small"`.

## `match` as a statement

A match used for its effects works too — arm bodies can be block
expressions:

```aura
enum Signal {
    Go,
    Stop
}

class Program {
    static void Main() {
        Signal s = Signal.Go;
        match (s) {
            Signal.Go => {
                print("moving");
                println();
                0
            },
            Signal.Stop => {
                print("halting");
                println();
                0
            }
        };
    }
}
```

## `if let`

When only one shape matters, `if let` binds a pattern without the ceremony
of a full match; the optional `else` covers the miss:

```aura
enum Option {
    None,
    Some(int value)
}

class Program {
    static void Main() {
        Option found = Option.Some(42);

        if let Option.Some(v) = found {
            print("got {v}");
            println();
        } else {
            print("nothing");
            println();
        }
    }
}
```

Enum-variant, range, constant, and binding patterns all work in `if let`.

---

Next: [Error Handling](06-error-handling.md).
