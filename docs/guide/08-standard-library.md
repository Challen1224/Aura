# 8. The Standard Library

Previous: [Async and Tasks](07-async-and-tasks.md) ·
[Guide index](../index.md#the-language-guide)

The standard library is deliberately small and fully verified under both
execution tiers. This chapter is the complete tour — if it isn't here, it
isn't in the library yet (the honest gap list is at the bottom).

## Console I/O

`print(value)` writes any value without a newline; `println()` ends the
line. `Console.ReadLine()` reads a line from standard input and returns
`string?` — null at end of input, which the type system makes you handle:

```aura
class Program {
    static void Main() {
        string? line = Console.ReadLine();
        print("read: {line ?? "(no input)"}");
        println();
    }
}
```

## Strings

Strings are immutable and character-indexed (Unicode scalar values, not
bytes). `Length` is a property; the methods are:

| Method | Notes |
|---|---|
| `Substring(start, len)`, `CharAt(i)` | character-indexed |
| `Contains(s)`, `StartsWith(s)`, `EndsWith(s)`, `IndexOf(s)` | search |
| `Split(sep)` | returns `List<string>` |
| `Trim()`, `ToUpper()`, `ToLower()`, `Replace(from, to)` | transforms |
| `ToInt()`, `ToFloat()` | parsing |

```aura
class Program {
    static void Main() {
        string s = "  Aura Language  ";
        string t = s.Trim();
        print(t.Length);
        println();
        print(t.ToUpper());
        println();
        print(t.Replace("Language", "Lang"));
        println();

        List<string> parts = "a,b,c".Split(",");
        print(parts.Count);
        println();

        int n = "42".ToInt();
        print(n + 1);
        println();
    }
}
```

There is no string `+` — build strings by interpolation: `"{a}, {b}"`.

## Collections

Three generic collections, all hash-indexed where it matters:

**`List<T>`** — ordered, index-addressable:
`Add`, `Get(i)`, `Set(i, v)`, `Insert(i, v)`, `RemoveAt(i)`, `IndexOf(v)`,
`Contains(v)`, `Clear()`, and the `Count` property.

**`Map<K, V>`** — insertion-ordered, hash lookup:
`Put(k, v)`, `Get(k)`, `ContainsKey(k)`, `Remove(k)`, `Keys()`, `Values()`,
`Clear()`, `Count`.

**`Set<T>`** — unique values, hash membership:
`Add(v)`, `Contains(v)`, `Remove(v)`, `ToList()`, `Clear()`, `Count`.

```aura
class Program {
    static void Main() {
        Map<string, int> ages = new Map<string, int>();
        ages.Put("ada", 36);
        ages.Put("grace", 45);

        if (ages.ContainsKey("ada")) {
            print("ada is {ages.Get("ada")}");
            println();
        }

        Set<string> seen = new Set<string>();
        seen.Add("x");
        seen.Add("x");          // duplicate: ignored
        print(seen.Count);
        println();
    }
}
```

### Iteration

`for (T x in ...)` iterates a `List` or `Set` directly; iterate a `Map`
through `Keys()` or `Values()`:

```aura
class Program {
    static void Main() {
        List<int> xs = new List<int>();
        xs.Add(1);
        xs.Add(2);
        xs.Add(3);
        int sum = 0;
        for (int x in xs) {
            sum = sum + x;
        }
        print(sum);
        println();

        Map<string, int> m = new Map<string, int>();
        m.Put("a", 1);
        m.Put("b", 2);
        for (string k in m.Keys()) {
            print("{k}={m.Get(k)} ");
        }
        println();
    }
}
```

Mutation-during-iteration semantics are pinned, not accidental: the element
count is snapshotted at loop entry, so appending to a List inside its own
loop does not extend that loop; Set iteration walks a snapshot copy, so Set
mutations never affect an iteration in progress; removing List elements
mid-iteration can make a later access fail with an index error.

## Files

Text-file I/O lives on the static `File` class:

| Method | Effect |
|---|---|
| `File.ReadAllText(path)` | whole file as one string |
| `File.ReadAllLines(path)` | `List<string>` of lines |
| `File.WriteAllText(path, text)` | create/overwrite |
| `File.AppendAllText(path, text)` | append |
| `File.Exists(path)` | `bool` |
| `File.Delete(path)` | remove |

```aura
class Program {
    static void Main() {
        string path = "aura_doc_demo.txt";
        File.WriteAllText(path, "one\n");
        File.AppendAllText(path, "two\n");

        List<string> lines = File.ReadAllLines(path);
        print(lines.Count);
        println();

        File.Delete(path);
        print(File.Exists(path));
        println();
    }
}
```

One caveat to plan around: I/O failures (missing file, permission trouble)
are **runtime errors, not catchable Aura exceptions** — check
`File.Exists` first where absence is expected.

## Reference types and the GC

Three wrapper types control how the garbage collector treats a referenced
object; they matter for caches and object registries:

* **`WeakRef<T>`** — does not keep its target alive. `isAlive()` /
  `get() -> T?`.
* **`SoftRef<T>`** — keeps its target alive *until memory pressure*
  (a `--gc-max-heap` limit forcing a choice); cleared last before
  out-of-memory. With no heap limit configured, softs behave like strong
  references.
* **`PhantomRef<T>`** — never keeps its target alive and cannot recover it
  (no `get()`); `isReclaimed()` supports post-mortem detection.

```aura
class Payload {
    int n;
    Payload(int n) { this.n = n; }
}

class Program {
    static void Main() {
        Payload strong = new Payload(7);
        WeakRef<Payload> weak = new WeakRef(strong);
        print(weak.isAlive());
        println();
        Payload? back = weak.get();
        print(back?.n ?? -1);
        println();
    }
}
```

GC behavior itself is tunable per run — thresholds, nursery size, pause
targets, a concurrent mode — through `aura run` flags; see the
[Tooling Reference](../tooling.md#gc-flags).

## What is not here yet

No math functions, random numbers, regular expressions, string builder,
binary file I/O, directory operations, networking, JSON, or threads.
These are on the [roadmap](../../TODO.md); the design intent is that the
existing pieces are *complete and verified* rather than broad and flaky.

---

That's the guide. For the CLI, project manifests, and every flag:
[Tooling Reference](../tooling.md).
