# 7. Async and Tasks

Previous: [Error Handling](06-error-handling.md) ·
[Guide index](../index.md#the-language-guide) · Next:
[The Standard Library](08-standard-library.md)

Aura's concurrency is **cooperative and deterministic**: everything runs on
one thread, tasks interleave only at `await` points, and scheduling is
FIFO — so a concurrent program interleaves the same way on every run. This
makes concurrent code testable to the byte.

## `async` methods and tasks

An `async` method must be static and return `Task<T>`; its body returns a
`T` and may `await` other tasks. *Calling* an async method spawns a task —
the call returns immediately with a `Task<T>` handle, and the task makes
progress whenever the scheduler runs: at any `await`, or when synchronous
code calls `t.wait()`:

```aura
class Program {
    static async Task<string> Brew(string what, int rounds) {
        int i = 0;
        while (i < rounds) {
            print("  {what}: step {i + 1}/{rounds}");
            println();
            await Tasks.pause();
            i = i + 1;
        }
        return "{what} ready";
    }

    static async Task<int> Kitchen() {
        var tea = Program.Brew("tea", 2);       // both tasks are now live
        var coffee = Program.Brew("coffee", 3);
        print(await tea);
        println();
        print(await coffee);
        println();
        return 0;
    }

    static void Main() {
        Program.Kitchen().wait();    // sync code drives the scheduler
    }
}
```

`Tasks.pause()` yields for one scheduler round, letting other ready tasks
run — that is what makes tea and coffee interleave step by step above.
Awaiting an already-finished task is immediate, and multiple awaiters of
one task all wake when it completes.

## Failures and deadlocks

An exception inside a task doesn't vanish: it surfaces, catchably, at every
place the task is awaited or waited on (and again on repeated waits):

```aura
class Program {
    static async Task<int> Doomed() {
        await Tasks.pause();
        throw new Exception();
    }

    static void Main() {
        var t = Program.Doomed();
        try {
            t.wait();
        } catch (Exception e) {
            print("task failed, caught in sync code");
            println();
        }
    }
}
```

Await cycles — two tasks awaiting each other, or a task awaiting itself —
are *detected* and reported as deadlocks rather than hanging the program.

## Combinators: `Tasks.all` and `Tasks.race`

`Tasks.all(list)` completes when every part has, with results **in list
order** (not completion order); the first failure in list order fails the
whole, catchably. `Tasks.race(list)` completes with the first task to
finish; the losers keep running and stay awaitable:

```aura
class Program {
    static async Task<int> Work(int id, int rounds) {
        int i = 0;
        while (i < rounds) {
            await Tasks.pause();
            i = i + 1;
        }
        return id * 10;
    }

    static async Task<int> Gather() {
        List<Task<int>> parts = new List<Task<int>>();
        parts.Add(Program.Work(1, 3));
        parts.Add(Program.Work(2, 1));
        parts.Add(Program.Work(3, 2));

        var all = await Tasks.all(parts);     // [10, 20, 30] — list order
        int total = 0;
        for (int v in all) {
            total = total + v;
        }
        return total;
    }

    static void Main() {
        print(Program.Gather().wait());
        println();
    }
}
```

## The rules, and the honest limits

Each rule has its own compile error:

* `async` methods must be **static** and return `Task<T>`.
* `await` is legal only inside `async` methods (not in lambdas).
* `await` needs a `Task<T>`-typed operand.
* `throws` clauses are not allowed on async methods.

And the current limits, stated plainly: there are no async lambdas, no
instance/virtual async methods, and no real async I/O yet — the task
system is the substrate those would plug into. "Parallel" here means
deterministically interleaved on one thread, not multi-core execution.
Methods containing async operations run on the interpreter tier; the JIT
compiles the hot synchronous code around them as usual.

---

Next: [The Standard Library](08-standard-library.md).
