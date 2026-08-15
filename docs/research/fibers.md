# Fibers / Green Threads for Aura — Research Note

**Status:** research complete, no implementation planned. Recommendation:
park stackful fibers indefinitely. The async/await tasks that just landed
cover Aura's cooperative-concurrency use cases; the *delta* between them
and true fibers is exactly the part that requires re-architecting the
interpreter (and cannot be fixed at all for JIT frames). If concurrency
grows again, the cheap, valuable next steps are task combinators
(`Tasks.all`, `Tasks.race`) and async lambdas — not fibers.

*Written 2026-08-15 against commit `8519b6b`. Code references are to that
tree.*

---

## 1. What "fibers" would add over the tasks Aura just shipped

Aura now has cooperative tasks: `async Task<T>` methods spawn hot tasks,
`await` suspends the current task's frame, a FIFO scheduler interleaves
ready tasks deterministically, and `t.wait()` drives everything from sync
code. That is, functionally, a green-thread system — with one restriction
that defines the entire remaining gap:

**A task can only suspend at the top of its own frame.** `await` is legal
only in the `async` method's own body, not in anything it calls. Classic
fibers remove that restriction: `Fiber.yield()` works at *any* call depth,
so ordinary sync helper functions can block cooperatively without their
callers knowing (no "function coloring").

## 2. Why yield-at-any-depth is an architecture change, not a feature

The interpreter executes nested calls by **Rust recursion**: `Op::Call`
invokes `invoke_frame`, which pushes an Aura frame and re-enters the op
loop as a nested Rust call (`aura-vm/src/lib.rs`, the `Op::Call` arm).
The Aura call stack and the Rust native stack are interleaved. A
suspension three Aura frames deep would have to capture and later resume
three *Rust* stack frames — which Rust does not permit. Async/await works
precisely because it never needs to: `await` sits in the task's top frame,
which is pure heap data (`Frame { pc, locals, stack, after_finally }`) and
lifts out cleanly.

Two ways to remove the restriction, both expensive:

1. **Non-recursive interpreter.** Rewrite the op loop as an explicit
   call-stack machine: `Op::Call` pushes an Aura frame and *continues the
   same Rust loop* instead of recursing. Then any interpreted frame chain
   is heap data and a fiber can suspend anywhere. Cost: the single most
   invasive VM change possible — every call form (static, virtual, super,
   closure invoke, constructor chains), the exception unwinder, the
   `finally` machinery, and every native/JIT helper that re-enters the VM
   (`exec` in `jit/x64/helpers.rs` calls `invoke_frame` reentrantly)
   would need restructuring around resumable state. Weeks of work with a
   large regression surface for a VM whose test suite currently proves
   parity across two tiers.
2. **Stackful coroutines (native stack switching).** Give each fiber a
   real side stack (corosensei/context-switch style). Sidesteps the
   interpreter rewrite but imports unsafe stack juggling, platform
   specifics, and makes GC root scanning (currently: walk `call_stack`,
   task frames, JIT frames conservatively) scan foreign stacks.

And one wall that neither approach moves: **JIT frames can never
suspend.** A compiled method's locals live in native registers and stack
slots; a yield underneath it would have to capture a native frame
mid-flight. Every green-thread VM with a JIT (Go, Erlang/BEAM JIT, Loom)
solves this with safepoint-based deoptimization or by compiling
yield-points into the generated code — machinery far beyond Aura's
tiering model, where task ops simply keep a method in the interpreter.

## 3. What the tasks already cover

The honest use-case inventory for fibers in a single-threaded VM with no
blocking I/O:

- *Interleaving independent jobs* — covered: hot tasks + `Tasks.pause()`
  (`examples/async_await.aura` interleaves two workers step-by-step).
- *Producer/consumer pipelines* — expressible with tasks awaiting each
  other; ergonomic gaps are combinator-shaped, not fiber-shaped.
- *Yield deep inside sync helpers* — the one thing tasks cannot do. In
  practice the workaround is making the helper `async` (accepting the
  coloring), which is C#'s answer too.

Coloring is a real ergonomic cost, but it buys the property that makes
Aura's implementation small and sound: suspension points are visible in
the type system and only ever live in scheduler-run frames.

## 4. Recommendation

Park fibers. If concurrency work continues, sequence it as:

1. **Task combinators** — `Tasks.all(List<Task<T>>) -> Task<List<T>>`,
   `Tasks.race(...)`: pure library/native work over the existing table,
   no new suspension semantics.
2. **Async lambdas** — lets callbacks await; compiler-side only (lift to
   an async-flagged method).
3. **Async I/O natives** — the thing that would make any of this pay for
   real programs (the async note's `http.get` gap).

Re-open this note only if a concrete program needs yield-at-depth badly
enough to fund the non-recursive interpreter — and if that day comes, do
the rewrite for its own sake first (it also simplifies deep-recursion
limits and stack traces), then fibers fall out of it almost for free.
