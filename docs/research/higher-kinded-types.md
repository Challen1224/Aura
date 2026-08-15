# Higher-Kinded Types for Aura — Research Note

**Status:** research complete, no implementation planned. Recommendation:
park HKT indefinitely — its prerequisites don't exist yet and its payoff in
Aura's stdlib is three instantiations wide. The genuinely useful path the
TODO sketch points at is (1) first-class function types and lambdas
(`Func<A, B>` appears in the sketch and is unrepresentable today), then
(2) concrete `map`/`filter`/`fold` on `List`/`Set`/`Option` — the
LINQ move, which captures most of the Functor pitch with zero type-system
work. Revisit HKT only if a real library need appears; if it does, the
interface-witness encoding over Aura's erased generics fits, and the
unification extension required is the tractable (Miller-pattern) kind.

*Written 2026-08-15 against commit `3bb65d2`. Code references are to that
tree.*

---

## 1. What "higher-kinded types" would mean

Every generic Aura has today abstracts over *types*: `Box<T>` takes a
complete type like `int` or `List<string>`. A higher-kinded parameter
abstracts over *type constructors* — things like `List` or `Option` that
still need an argument. In kind notation, today's parameters have kind `*`;
`F<_>` in the TODO sketch has kind `* -> *`:

```aura
trait Functor<F<_>> {
    F<B> map<A, B>(F<A> fa, Func<A, B> f);
}
```

One `map` contract, instantiable at `F = List`, `F = Option`, `F = Set`.
What other languages did with this:

| Language | Position | Consequence |
|---|---|---|
| Haskell | Full HKT + type classes | `Functor`/`Monad` hierarchies; the reference design |
| Scala | Full HKT (`F[_]`) + implicits | Same power; famous for abstraction-heavy libraries |
| C# | Rejected | LINQ methods are written per-container; nobody can abstract `SelectMany` over the container. The ecosystem thrived anyway. |
| Rust | Rejected (GATs as partial answer) | `Iterator` and friends are concrete traits; the gap is felt mainly by category-theory-shaped libraries |
| Kotlin/Java | Rejected; Arrow emulates via `Kind<F, A>` brands | Works, but the encoding leaks into every signature |
| Swift | Deferred indefinitely | Protocols with associated types cover most uses |

The pattern: mainstream C-family languages all declined, and their
collection libraries took the concrete route instead.

## 2. What it would be *for* in Aura

The honest inventory of things `Functor`-style abstraction would serve, at
today's surface: `List`, `Set`, `Map`, and user sum types like
`Option`/`Result`. That is **three or four instantiations** — each of which
could carry a hand-written `map` for less total code than the abstraction
machinery. HKT earns its keep in ecosystems with dozens of container-shaped
libraries; Aura's charter (small language, real programs) is years from
that being the bottleneck.

## 3. The two missing prerequisites

The sketch cannot be expressed today for reasons *below* the kind system:

1. **No function types.** `Func<A, B>` names nothing: the AST, `Type` enum,
   bytecode, and VM have no function type, no lambda expression, no closure
   representation, and no method references (`grep Func|Lambda|Closure`
   over `ast.rs` / `aura-bytecode` returns nothing). A `Functor` without
   the ability to pass `f` is inert. First-class functions are a real,
   self-contained feature: a `Type::Func(params, ret)`, a closure value
   carrying captured locals (GC-traced), a bytecode construction op, an
   invoke path in both tiers, and capture analysis in the typer. That is a
   P1-sized feature with immediate payoff far beyond HKT.
2. **No `trait` / no implicit resolution.** Aura has interfaces —
   recently upgraded with variance and real generic implementation
   (`class CatFactory : IProducer<Cat>`) — but no mechanism that finds "the
   Functor instance for List" at a call site. Haskell/Scala resolve
   instances implicitly; in Aura every consumer would take the witness
   object as an ordinary parameter. Workable (it is what dictionary-passing
   compiles to anyway), but boilerplate the sketch quietly assumes away.

## 4. What the tree already has, and what HKT would add

Groundwork that would carry over:

- **Erased generics dispatch fine.** `CallVirt` is name-based; a
  `ListFunctor : Functor<List>` witness works at runtime with zero VM/JIT
  changes — the entire cost is in the checker, which is the right shape for
  this codebase (nested classes, extensions, and operators all landed as
  compile-time-only features).
- **Constraint machinery generalizes.** `GenericParam` now carries bounds,
  unions, `new()`, and variance; a kind field (`arity: usize`, default 0)
  is the same pattern. `validate_type_args` already arity-checks type
  argument lists at every use site — kind checking is that same check
  , applied one level up.
- **Unification is close.** `unify_generic_types` unifies first-order
  types. `F<A>` against `List<int>` needs constructor variables
  (`F ↦ List`, `A ↦ int`) — in the sketch's shape the arguments to `F` are
  always distinct bound variables, which is exactly the Miller-pattern
  fragment where higher-order unification is decidable and simple. No
  general higher-order unification needed.

What would still be new: kinded parameter syntax (`F<_>`), a constructor
variable in the `Type` representation (today `Type::Class(name, args)`
assumes a saturated application — partial application `F` alone is
unrepresentable), kind inference or annotation on every generic parameter,
and error messages that explain kind mismatches to a user who has never
heard the word "kind."

## 5. Options considered

1. **Full HKT** (kinded params + constructor unification + interface
   witnesses). Feasible on erased generics; several weeks of checker work;
   payoff gated on §3's prerequisites and on a library ecosystem that does
   not exist yet. **Rejected for now.**
2. **Defunctionalized brands** (Arrow-style `Kind<F, A>`). All of the
   boilerplate, none of the checking; without lambdas it abstracts nothing.
   **Rejected.**
3. **Concrete combinators** — after lambdas land, put `map`/`filter`/
   `fold`/`Select`-style methods directly on `List`/`Set`/`Map` (native
   intrinsics) and on user sum types (extension methods). This is C#'s
   LINQ answer: no new type theory, immediately useful in every program,
   and it removes ~all of the demand HKT would have served.
   **Recommended, sequenced after first-class functions.**

## 6. Recommendation

Park HKT indefinitely. Promote **first-class function types + lambdas** to
the feature queue (it is the load-bearing prerequisite the TODO sketch
assumes, and independently the biggest expressiveness gap in the language),
then build concrete collection combinators on top. Re-open this note only
if Aura grows a library ecosystem whose authors are writing the same
container-generic code three times — the witness-interface design in §4 is
the path, and nothing landing in the meantime (variance, constraints,
inference) makes it harder; most of it makes it easier.
