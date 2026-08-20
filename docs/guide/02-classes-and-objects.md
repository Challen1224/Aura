# 2. Classes and Objects

Previous: [Language Basics](01-language-basics.md) ·
[Guide index](../index.md#the-language-guide) · Next:
[The Type System](03-the-type-system.md)

## Classes, fields, and methods

A class declares fields, methods, and constructors. A constructor is a
method named after the class; `this` refers to the current instance.

```aura
class Counter {
    private int count;

    Counter(int start) {
        this.count = start;
    }

    void Increment() {
        this.count = this.count + 1;
    }

    int Value() {
        return this.count;
    }
}

class Program {
    static void Main() {
        Counter c = new Counter(10);
        c.Increment();
        c.Increment();
        print(c.Value());
        println();
    }
}
```

Constructors overload by parameter list and can chain — `: this(...)`
delegates to another constructor of the same class, `: super(...)` to a
base-class constructor:

```aura
class Point {
    int x;
    int y;

    Point(int x, int y) {
        this.x = x;
        this.y = y;
    }

    Point() : this(0, 0) {}
}

class Program {
    static void Main() {
        Point origin = new Point();
        print("({origin.x}, {origin.y})");
        println();
    }
}
```

## Visibility

| Modifier | Accessible from |
|---|---|
| `public` (default) | anywhere |
| `protected` | the declaring class and its subclasses |
| `private` | the declaring class only |
| `internal` | classes declared in the same file (module) |

A *module* is a source file: when a program is compiled from several files,
`internal` members are visible only to classes in the same file. In a
single-file program, `internal` is effectively file-wide.

## Static members and static classes

Static fields and methods belong to the class, not an instance. A
`static class` is the namespace idiom — it cannot be instantiated,
inherited from, or used as a type, and every member must be static. Static
fields may take constant-literal initializers, applied at program start:

```aura
static class MathUtil {
    static float PI = 3.14159;

    static int Max(int a, int b) {
        if (a > b) { return a; }
        return b;
    }
}

class Program {
    static void Main() {
        print(MathUtil.Max(3, 7));
        print(" ");
        print(MathUtil.PI);
        println();
    }
}
```

Initialized statics stay mutable — the initializer is a starting value, not
a constant declaration. Instance fields do not take initializers; assign
them in a constructor.

## Properties

A property is field-like access backed by accessors. `get;`/`set;` are
*auto* accessors with a compiler-generated backing field; either accessor
can instead have an explicit body (the setter sees the assigned value as
`value`), and accessors can tighten visibility individually:

```aura
class Person {
    private int _age;

    string Name { get; set; }

    int Age {
        get { return this._age; }
        private set { this._age = value; }
    }

    Person(int age) {
        this.Age = age;    // fine: inside the class
    }
}

class Program {
    static void Main() {
        Person p = new Person(30);
        p.Name = "Ada";
        print("{p.Name}, {p.Age}");
        println();
    }
}
```

Static properties work the same way on the class itself.

## Inheritance

Aura has single inheritance. A method must be declared `virtual` to be
overridable, and the subclass must say `override`; `super` reaches the base
class implementation:

```aura
class Animal {
    virtual string Speak() {
        return "...";
    }

    string Greet() {
        string sound = this.Speak();
        return "the animal says {sound}";
    }
}

class Dog : Animal {
    override string Speak() {
        string quiet = super.Speak();
        return "Woof! (not {quiet})";
    }
}

class Program {
    static void Main() {
        Animal a = new Dog();
        print(a.Greet());    // dynamic dispatch reaches Dog.Speak
        println();
    }
}
```

Note that an `override` method is not itself virtual: Aura's virtual chain
is exactly one level deep — the base declares `virtual`, one subclass
overrides, and overriding the override is a compile error (`virtual
override` is likewise rejected). Design for extension deliberately.

Modifiers controlling the extension points:

* `abstract class` cannot be instantiated; an `abstract` method has no body
  and must be implemented by concrete subclasses.
* `sealed class` cannot be subclassed.
* `final` on a method forbids overriding *and* re-declaring it in
  subclasses; `final override` seals an override against re-declaration
  too.

## Interfaces

An interface declares method signatures; a class lists the interfaces it
implements after its base class. Interfaces may extend other interfaces,
and a method *with* a body in an interface is a default method — inherited
by implementors that don't provide their own:

```aura
interface Drawable {
    string Draw();

    string Label() {
        return "shape";      // default method
    }
}

class Square : Drawable {
    string Draw() {
        return "[square]";
    }
}

class Program {
    static void Main() {
        Drawable d = new Square();
        print("{d.Label()}: {d.Draw()}");
        println();
    }
}
```

Interface types are first-class: variables, fields, parameters, and return
types can all be interface-typed, and dispatch is dynamic.

**Structural satisfaction (duck typing).** A class whose public instance
methods exactly match an interface's signatures satisfies it *without*
declaring it — assignment, dispatch, and `is` tests all agree. This applies
to non-generic interfaces with exact signature matches; when in doubt,
declare the interface explicitly (it also reads better).

Generic interfaces and variance (`IProducer<out T>`) are covered with the
rest of generics in [chapter 3](03-the-type-system.md#generics).

## Records

A `record` is an immutable data class with value semantics: its positional
parameters become read-only properties, `==` compares by structure, and
`with` produces a modified copy:

```aura
record Point(int x, int y);

class Program {
    static void Main() {
        Point a = new Point(3, 4);
        Point b = new Point(3, 4);
        print(a == b);               // true: value equality
        println();

        Point moved = a with { x = 30 };
        print("({moved.x}, {moved.y})");
        println();
    }
}
```

Records also participate in pattern matching positionally —
`Point(0, y)` matches points on the x-axis
([chapter 5](05-pattern-matching.md)).

## Nested classes

Classes nest to any depth. Inside the enclosing class the inner name is
used bare (shadowing any top-level class of the same name); outside, it is
qualified. A nested class can read its enclosing class's private members;
the reverse is denied:

```aura
class Outer {
    class Inner {
        int Answer() {
            return 42;
        }
    }

    int Delegate() {
        Inner i = new Inner();
        return i.Answer();
    }
}

class Program {
    static void Main() {
        Outer.Inner direct = new Outer.Inner();
        print(direct.Answer());
        println();
    }
}
```

## Extension methods

An `extension` block adds callable methods to an existing type — `this` in
the body is the receiver. Resolution prefers real methods; extensions apply
to subclasses of their target too:

```aura
extension StringExtras on string {
    bool IsShout() {
        return this == this.ToUpper();
    }
}

class Program {
    static void Main() {
        print("HELLO".IsShout());
        print(" ");
        print("hello".IsShout());
        println();
    }
}
```

Targets can be `string` or a non-generic class/interface — not enums,
newtypes, primitives, or the generic collections.

---

Next: [The Type System](03-the-type-system.md) — nullability, sum types,
and generics.
