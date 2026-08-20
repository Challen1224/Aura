# 4. Functions and Lambdas

Previous: [The Type System](03-the-type-system.md) ·
[Guide index](../index.md#the-language-guide) · Next:
[Pattern Matching](05-pattern-matching.md)

## Function types

`Func<..., R>` is the type of a function whose last type argument is its
return type; `Action<...>` returns void. Function values are first-class:
store them, pass them, return them, call them with ordinary call syntax.

```aura
class Program {
    static int Apply(Func<int, int> f, int v) {
        return f(v);
    }

    static void Main() {
        Func<int, int> double = x => x * 2;
        print(double(21));
        println();
        print(Program.Apply(double, 5));
        println();
    }
}
```

## Lambdas

Three body forms — bare parameter, annotated parameter list, block body:

```aura
class Program {
    static void Main() {
        Func<int, int> inc = x => x + 1;             // target-typed
        var add = (int a, int b) => a + b;           // annotated: var works
        Action<string> shout = s => {
            print(s.ToUpper());
            println();
        };

        print(inc(41));
        println();
        print(add(20, 22));
        println();
        shout("done");
    }
}
```

A bare-parameter lambda needs a target type (`Func<...>` on the left, a
parameter's declared type, a return type); a fully annotated
expression-body lambda determines its own type, so `var` works.

### Captures

Lambdas capture enclosing locals and `this` **by value** — the lambda sees
the value at creation time, and assigning to a captured variable inside the
lambda is a compile error:

```aura
class Program {
    static void Main() {
        int offset = 100;
        Func<int, int> shift = x => x + offset;
        offset = 0;              // does not affect the lambda
        print(shift(1));         // 101
        println();
    }
}
```

Nested lambdas capture transitively — `a => (int b) => a + b` works; note
that a call result cannot be called in one expression (`curry(1)(2)`), so
bind the intermediate function to a local first.

### Lambdas and generic methods

A generic method can take lambdas whose types mention its type parameters;
inference resolves the non-lambda arguments first, then target-types each
lambda, and an expression-body lambda's *return* can even determine an
open type parameter:

```aura
class Util {
    static <T1, T2> T2 Transform(T1 value, Func<T1, T2> f) {
        return f(value);
    }
}

class Program {
    static void Main() {
        print(Util.Transform(21, x => x * 2));        // T1 = T2 = int
        println();
        print(Util.Transform(7, n => "n = {n}"));     // T2 = string, from the body
        println();
    }
}
```

## Operator overloading

A class can define `+ - * / %` and the comparisons `== < <= > >=` as
instance methods named `operator+`, `operator==`, and so on. The left
operand's class is consulted; the one parameter is the right operand (any
type):

```aura
class Vec2 {
    int x;
    int y;

    Vec2(int x, int y) {
        this.x = x;
        this.y = y;
    }

    Vec2 operator+(Vec2 o) {
        return new Vec2(this.x + o.x, this.y + o.y);
    }

    bool operator==(Vec2 o) {
        return this.x == o.x && this.y == o.y;
    }
}

class Program {
    static void Main() {
        Vec2 v = new Vec2(1, 2) + new Vec2(3, 4);
        print("({v.x}, {v.y})");
        println();
        print(v == new Vec2(4, 6));
        println();
    }
}
```

The rules, each enforced with its own compile error: overloads are public
instance methods, one parameter, real return type; comparison overloads
return `bool`; `!=` cannot be declared (it is always the negation of
`operator==`); `&&` and `||` are not overloadable (they must
short-circuit). Without an overload, `==` on class instances is reference
identity — and *ordering* a class without `operator<` is a compile error,
not a pointer comparison.

## Custom operators

New operator symbols can be minted, starting with `|`, `&`, or `^` and
continuing over operator characters: `|>`, `^^`, `&+`, `|||`. They declare
and resolve exactly like the built-in overloads:

```aura
class Pipeline {
    int value;

    Pipeline(int value) {
        this.value = value;
    }

    Pipeline operator|>(int add) {
        return new Pipeline(this.value + add);
    }
}

class Program {
    static void Main() {
        Pipeline p = new Pipeline(1) |> 10 |> 100;
        print(p.value);
        println();
    }
}
```

All custom operators share one precedence tier — tighter than ranges and
comparisons, looser than `+`/`-` — and associate left, so pipelines chain
naturally. Using an undeclared symbol is a clean compile error naming the
missing `operator<sym>` overload.

---

Next: [Pattern Matching](05-pattern-matching.md).
