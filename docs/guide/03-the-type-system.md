# 3. The Type System

Previous: [Classes and Objects](02-classes-and-objects.md) ·
[Guide index](../index.md#the-language-guide) · Next:
[Functions and Lambdas](04-functions-and-lambdas.md)

## Nullable types

Null is opt-in. A plain `T` never holds null; only `T?` can — and the
compiler will not let you touch a `T?`'s members until you have dealt with
the null case:

```aura
class Program {
    static void Main() {
        string s = null;    // ERROR: null needs string?
    }
}
```

The tools for dealing with it:

**Narrowing.** Inside `if (x != null)`, `x` *is* the non-nullable type. The
same applies after an `if (x == null)` branch that exits, and inside
`while (x != null)`. Facts flow through conditions: `&&` carries what its
left side proved into the right side, so `x != null && x.Length > 0` is
both well-typed and safe (the operators short-circuit).

**`??`** takes the left value unless it is null, in which case it takes the
right — and the result is non-nullable. **`?.`** accesses a member if the
receiver is non-null and produces null otherwise (typed `T?`). **`!`**
asserts non-null, with a runtime error if you were wrong.

```aura
class Account {
    string owner;
    Account(string owner) { this.owner = owner; }
    string Owner() { return this.owner; }
}

class Program {
    static string Describe(string? nickname) {
        return nickname ?? "(anonymous)";
    }

    static void Main() {
        string? given = "Ada";
        if (given != null) {
            print(given.ToUpper());        // narrowed: plain string here
            println();
        }

        string? missing = null;
        print(Program.Describe(missing));  // ?? picks the fallback
        println();

        Account? none = null;
        print(none?.Owner() ?? "(nobody)");  // ?. yields null, ?? recovers
        println();
    }
}
```

`?.` applies to class-typed receivers (fields and methods); on primitives,
narrow with `if` or supply a default with `??` instead.

Two honest limits, by design: *fields* are never narrowed (copy a nullable
field to a local first, or use `!`), and assigning to a narrowed variable
widens it back to `T?`.

## Type guards: `is`

`expr is Type` tests the runtime type; `expr is Type name` also binds
`name` with the tested type wherever the test is known true — the
then-branch, a loop body, or the right side of the same `&&` chain:

```aura
class Animal {
    virtual string Name() { return "animal"; }
}

class Dog : Animal {
    override string Name() { return "dog"; }
    string Fetch() { return "ball"; }
}

class Program {
    static string Inspect(Animal a) {
        if (a is Dog d && d.Fetch() == "ball") {
            return "a dog that fetches";     // d: Dog in here
        }
        string name = a.Name();
        return "just {name}";
    }

    static void Main() {
        print(Program.Inspect(new Dog()));
        println();
        print(Program.Inspect(new Animal()));
        println();
    }
}
```

Tests that could never succeed (between unrelated concrete classes) are
compile errors rather than always-false checks, and generic type arguments
are not testable at runtime (they are erased).

## Tuples

Tuple types write `(T1, T2, ...)`; access by position with `.0`, `.1`, or
destructure:

```aura
class Program {
    static (int, int) MinMax(int a, int b) {
        if (a < b) { return (a, b); }
        return (b, a);
    }

    static void Main() {
        (int, int) pair = Program.MinMax(9, 4);
        print(pair.0);
        print("..");
        print(pair.1);
        println();

        (int lo, int hi) = Program.MinMax(3, 1);
        print("{lo}..{hi}");
        println();
    }
}
```

## Enums and sum types

An `enum` is a closed set of variants; variants may carry fields, and
`match` takes them apart ([chapter 5](05-pattern-matching.md)):

```aura
enum Shape {
    Point,
    Circle(int radius),
    Rect(int w, int h)
}

class Program {
    static void Main() {
        Shape s = Shape.Rect(3, 4);
        int area = match (s) {
            Shape.Point => 0,
            Shape.Circle(r) => 3 * r * r,
            Shape.Rect(w, h) => w * h
        };
        print(area);
        println();
    }
}
```

A *sum type* is a generic enum declared in type-alias form, and its
variants construct and match **bare** — no qualification needed. This is
how `Result` works:

```aura
type Result<T, E> = Ok(T) | Err(E);

class Program {
    static Result<int, string> Parse(string s) {
        if (s == "42") { return Ok(42); }
        return Err("not the answer");
    }

    static void Main() {
        string msg = match (Program.Parse("42")) {
            Ok(v) => "got {v}",
            Err(e) => "failed: {e}"
        };
        print(msg);
        println();
    }
}
```

The `?` operator propagates the error variant automatically — see
[chapter 6](06-error-handling.md#result-types-and-the--operator).

## Type aliases and newtypes

A `type` alias is a new *name* for the same type — the two interconvert
freely. A `newtype` is a new *type* over a primitive — no implicit
conversion in either direction, which turns "I passed the wrong int"
into a compile error:

```aura
type Meters = int;
newtype UserId = int;

class Program {
    static void Main() {
        Meters m = 5;
        int plain = m;               // alias: same type
        print(plain);
        println();

        UserId id = UserId(1001);    // construct explicitly
        int raw = id.Value;          // unwrap explicitly
        print(raw);
        println();
    }
}
```

Newtypes are fully erased at runtime (zero cost), compare with `==`/`!=`
against the same newtype, and work as collection keys. Distinct newtypes
over the same primitive do not interconvert.

## Literal unions

A string-literal union is a type admitting exactly the listed values;
anything else — including a plain `string` — is a compile error at the
boundary, while the value itself *is* a string at runtime:

```aura
type Direction = "north" | "south" | "east" | "west";

class Program {
    static string Arrow(Direction d) {
        if (d == "north") { return "^"; }
        if (d == "south") { return "v"; }
        return "<>";
    }

    static void Main() {
        print(Program.Arrow("north"));    // "up" would be a compile error
        println();
    }
}
```

Compare union values with `==` against member literals; comparing against a
*non-member* literal is a compile error, not a silent `false`. (String
patterns in `match` apply to `string` subjects, not union-typed ones —
use `==` chains for unions.)

Unions compose: `type Move = Horizontal | Vertical;` merges other unions'
members, and a union value flows into any union containing all its members
(subset widening).

## Generics

Classes and methods take type parameters; generics are checked at
compile time and reified enough for distinct instantiations to coexist:

```aura
class Box<T> {
    private T item;
    Box(T item) { this.item = item; }
    T Get() { return this.item; }
}

class Program {
    static void Main() {
        var a = new Box(42);         // inferred: Box<int>
        Box<string> b = new Box("hi");
        print(a.Get());
        print(" ");
        print(b.Get());
        println();
    }
}
```

Type arguments are inferred at construction and at generic-method call
sites by unifying parameter types against arguments — nested shapes
(`List<T>`, `T?`, tuples) participate, and conflicts are precise errors.

### Constraints

`where` clauses (or inline bounds) restrict what a type argument may be —
and in return, the body may *use* the constraint:

```aura
interface Sized {
    int Size();
}

class Crate {
    int Size() { return 8; }
}

class Program {
    static <T : Sized> int Total(T a, T b) {
        return a.Size() + b.Size();    // callable via the bound
    }

    static void Main() {
        print(Program.Total(new Crate(), new Crate()));
        println();
    }
}
```

Three constraint forms:

* **Bounds** — `where T : Entity, ICloneable`: the argument must satisfy
  every listed class/interface (structural satisfaction counts). Members of
  each bound become callable on `T`.
* **Numeric unions** — `where T : int | float`: the argument must be one of
  the alternatives, and an all-numeric union licenses arithmetic and
  ordering between `T` values in the body.
* **`new()`** — the argument must be a concrete class with a parameterless
  constructor. (Combinable with bounds; `new T()` itself is not yet
  supported.)

### Variance

Interface type parameters may declare variance: `out T` (covariant — a
producer of `Cat` is a producer of `Animal`) and `in T` (contravariant — a
comparer of `Animal` can compare `Cat`s):

```aura
class Animal {
    virtual string Name() { return "animal"; }
}

class Cat : Animal {
    override string Name() { return "cat"; }
}

interface IProducer<out T> {
    T Produce();
}

class CatFactory : IProducer<Cat> {
    Cat Produce() { return new Cat(); }
}

class Program {
    static void Main() {
        IProducer<Animal> p = new CatFactory();   // covariance
        print(p.Produce().Name());
        println();
    }
}
```

The compiler checks soundness conservatively: an `out` parameter may not
appear in any method-parameter position, an `in` parameter may not appear
in a return type. Unannotated parameters are invariant.

### Phantom types

A type parameter used only as a compile-time tag is enforced like any
other — `FileHandle<Open>` and `FileHandle<Closed>` do not interconvert,
which lets an API make invalid state transitions unrepresentable at zero
runtime cost.

---

Next: [Functions and Lambdas](04-functions-and-lambdas.md).
