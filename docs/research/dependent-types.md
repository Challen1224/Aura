# Dependent Types for Aura — Research Note

**Status:** research complete, no implementation planned. Recommendation: park
full dependent types indefinitely; if type-level guarantees become a priority,
the incremental path is (1) enforce the generic constraints we already parse,
(2) extend the existing `GuardFact` flow analysis with integer interval facts,
and only then evaluate (3) const generics. Refinement types with an SMT
solver are explicitly rejected for this project.

*Written 2026-08-14 against commit `d50aded`. Code references are to that
tree.*

---

## 1. What "dependent types" would mean

A dependent type mentions a *value*: `Vector<n>` where `n` is a runtime
length, `int<0..100>`, "a list that is not empty." The design space is a
spectrum, and the useful engineering question is where a language stops:

| Tier | Exemplars | What it buys | What it costs |
|---|---|---|---|
| Full dependent types (Π/Σ) | Idris, Agda, Coq, Lean | Arbitrary propositions as types; proofs in the language | A proof assistant: unification-heavy checker, totality checking, proof terms in user code. A research career, not a feature. |
| Refinement types | Liquid Haskell, F*, Dafny | `{v:int | 0 <= v && v < len xs}` checked automatically | An SMT solver (Z3) in the toolchain, verification-condition generation, opaque failure modes when the solver times out |
| Indexed types / const generics | Rust `[T; N]`, C++ NTTP | Type-level integers with equality checking (`Matrix<3,4>`) | Type-level evaluation, monomorphization-vs-erasure decision, arithmetic on type indices quickly reintroduces the solver problem |
| Flow-sensitive value tracking | TypeScript literal types + narrowing | Value facts proven by control flow, erased at runtime | A per-feature analysis; no general proofs — exactly as strong as the fact vocabulary |

Aura already sits at tier 4 in miniature, which is what makes this note more
than theory (§3).

## 2. What it would be *for* in Aura

Honest motivating cases, ranked by how often they bite today:

1. **List bounds.** `List.Get` raises a runtime error on an out-of-range
   index (`aura-vm/src/native.rs`, "index N out of range"). This is the
   canonical refinement-type sales pitch: prove `0 <= i < xs.Count` and
   delete the check. It is also the *hardest* case short of a solver,
   because `Count` is mutable state — any `Add`/`RemoveAt`/alias can
   invalidate a proven bound, so soundness needs effect/alias reasoning,
   not just arithmetic.
2. **Integer ranges.** Ports, percentages, month numbers: `int<1..12>`.
   Static where literals flow, guard-checked otherwise.
3. **Non-empty collections.** `First()`/`Last()` without a null/error path.
4. **Shape agreement.** `Matrix<R,C>` multiplication — real, but not an
   Aura audience problem today.

Cases 2–3 are cheap-tier features. Case 1 is the expensive one, and it is
the only one that would meaningfully change how Aura programs are written.

## 3. Groundwork already in the tree

The last several features quietly built most of tier 4:

- **Literal types** (`type Direction = "north" | ...`): types indexed by
  values already exist. `Type::StringLit` is a transient value-carrying type
  that the checker reasons about (`typer.rs::is_assignable`), erased at
  runtime — precisely the architecture a range type `int<1..12>` needs, with
  `IntLit` (which also already exists and range-checks against target
  widths) playing the role `StringLit` plays.
- **`GuardFact` narrowing** (`typer.rs::condition_facts` /
  `with_facts`): a working flow-sensitive fact engine with polarity,
  `&&`/`||`/`!` composition, short-circuit-sound rhs typing, assignment
  invalidation, and terminating-branch propagation. Today its vocabulary is
  two facts (`NonNull`, `Binding`). An `InRange(name, lo, hi)` fact derived
  from `i >= 0 && i < 10`-shaped conditions drops into this machinery
  without structural change — the analysis exists; only the fact vocabulary
  and the comparison-to-facts extraction grow.
- **Newtypes**: the pragmatic answer available *today*. A validated
  constructor (`static Age? Of(int v)` returning null on bad input) plus an
  opaque `newtype` gives "values of this type satisfy the invariant" with
  zero checker changes. This is the idiom the docs should teach while
  anything grander stays parked.

One **verified gap** blocks any constraint-based extension: generic
constraints are parsed (`GenericParam.constraint`) but **not enforced** —
`class Box<T : Sized>` accepts `Box<Plain>` today (confirmed by running a
counterexample against `d50aded`). Enforcing declared constraints is
ordinary type-checking work, is worth doing on its own, and is a
prerequisite for any type-level programming story.

## 4. Options considered

**A. Refinement types with SMT (rejected).** Wiring Z3 into a
"dependency-light" four-crate workspace, generating verification
conditions from a bytecode-oriented compiler, and debugging solver
timeouts is a second project larger than the language. It also collides
with the JIT/interpreter parity discipline: checks the typer deletes must
be provably deletable in *both* tiers. Not while stdlib/tooling remain the
stated priorities.

**B. Const generics `Vector<const int N>` (defer).** Coherent and
erasable (indices checked for equality only, no arithmetic), but Aura has
no fixed-size array type to index — `List<T>` is dynamic, so the flagship
use case doesn't exist yet. Revisit only if fixed-size arrays land.

**C. Interval facts in the guard engine (the real candidate).** Add
`GuardFact::InRange` fed by integer comparisons, consumed first by
diagnostics (warn on provably-out-of-range literal indices), later by an
opt-in `int<lo..hi>` range type reusing the literal-type
erasure/coercion pattern. Bounded scope, no solver, no new runtime. The
honest caveat: without alias/effect reasoning it must stay *advisory* for
List bounds (mutation invalidates counts), so it cannot delete runtime
checks — it can only catch bugs earlier. That limits its payoff to tier-2/3
cases (§2), which is respectable but not transformative.

**D. Do nothing beyond documenting the newtype idiom (the default).**

## 5. Recommendation

Dependent types stay parked. Concretely:

1. Keep runtime checks as the soundness backstop; they are cheap, tested,
   and identical across tiers.
2. When type-system appetite returns, do in order: enforce declared
   generic constraints (a gap regardless of this note), then option C's
   interval facts as diagnostics. Each is a normal-sized, verifiable
   feature in the existing style.
3. Teach the newtype-with-validating-constructor idiom in examples/docs as
   the supported way to carry invariants today.
4. Do not adopt an SMT solver for this project.
