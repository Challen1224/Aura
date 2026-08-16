# Heap Compaction for Aura — Research Note

**Status:** research complete, no implementation planned. Recommendation:
park mark-compact indefinitely. Aura's heap has no memory region of its
own to compact — the system allocator owns placement — and the
never-moving, never-reused handle design is now the load-bearing
invariant under weak/soft/phantom references, snapshot-based concurrent
marking, and the JIT's conservative frame scan. The classical benefits of
compaction (defragmenting a managed region, bump allocation, locality of
a compacted survivor space) either do not apply to this heap shape or
cost a production-VM-scale rewrite to obtain honestly.

*Written 2026-08-16 against commit `86d592b` (uncommitted tree including
weak/soft/phantom references and the concurrent collector). Code
references are to that tree.*

---

## 1. What compaction means, and what Aura's heap actually is

Mark-compact collectors assume the runtime owns a linear address range:
after marking, live objects slide toward one end, free space becomes one
contiguous run, allocation becomes a bump pointer, and survivors end up
adjacent in memory (locality). All three sub-goals of the TODO item —
the algorithm, fragmentation, locality — presuppose that memory model.

Aura's heap is not that model. `Heap` stores objects in a
`HashMap<GcRef, HeapObject>` (`aura-vm/src/heap.rs`); each `AuraObject`'s
real payload — `String` buffers, `Vec<Value>` element storage, map
entries — is a separate Rust/system allocation the VM never sees the
address of. The VM manages *object lifetimes*, not *object placement*.
Three consequences:

1. **There is no VM-owned region to defragment.** Fragmentation, to the
   extent it exists, lives inside the system allocator's arenas, and
   coalescing/reuse there is the allocator's job (and modern allocators
   are good at it). A "compaction" pass in the VM has nothing to slide.
2. **Allocation is already not the bottleneck compaction fixes.** The
   classical payoff — bump-pointer allocation in a compacted nursery —
   does not apply: every allocation is a `HashMap` insert plus malloc,
   and would remain so after any header-level compaction.
3. **Locality is set by malloc and hash order, not by the collector.**
   Marking and sweeping traverse the `HashMap` in hashed order; object
   payloads are wherever malloc put them. Moving `HeapObject` headers
   around cannot make `List<string>` element buffers adjacent.

## 2. Never-moving is now a load-bearing invariant

Since the generational work, four shipped features lean directly on the
fact that handles are allocated monotonically, never reused, and objects
never move:

- **Weak/soft/phantom liveness is heap membership.** `WeakRef.isAlive()`
  is exactly `heap.contains(handle)` — sound *because* a dead target's
  handle can never be recycled into a live object
  (`aura-vm/tests/weak_refs.rs` doc header states this contract).
- **SATB concurrent marking.** The concurrent collector's soundness
  argument is "unreachable at snapshot time is unreachable forever" —
  which holds because object identity is a never-reused handle. A
  compacting collector that recycled slots would need epoch tagging or
  forwarding to keep both the weak-ref contract and the snapshot
  argument intact.
- **The JIT's conservative frame scan** reads tagged handle values out
  of raw slot memory (`aura-vm/src/lib.rs`, `maybe_collect`'s
  `jit_frames` loop). Handles-not-addresses is what makes conservative
  scanning safe; any design where a scan result must be *updated*
  (moving collectors rewrite references) requires precise stack maps the
  JIT does not have.
- **The write-barrier remembered set** logs handles; a moving collector
  would additionally need to fix up every stored reference at move time.

None of these makes moving *impossible* — a handle-indirection table
(handles stay stable, storage moves underneath) preserves every contract
above. It is what the honest cost analysis below assumes.

## 3. What a real implementation would take, and what it would buy

**Design sketch (handle-table arena):** replace the map with
`table: Vec<Option<u32>>` (handle → arena offset) plus a bump arena of
object records. Mark-compact slides records, rewrites only the table.
Handles never change, so weak refs, SATB, the JIT scan, and the
remembered set all survive unmodified.

**What it buys at header level: almost nothing.** `AuraObject` records
are a few pointers each; compacting them tightens one array while every
payload buffer stays in malloc. Traversal locality improves marginally
over hashed order; fragmentation and cache behavior of actual data are
untouched. This is the "cosmetic mark-compact" outcome: the algorithm
box gets checked, the benefits do not appear.

**What full compaction requires:** inlining variable-size payloads into
the arena — custom object layouts (`unsafe`), variable-size sliding,
interior pointers eliminated, `&mut Vec<Value>` borrows re-plumbed
through arena offsets, growth of a `List` becoming an arena reallocation
with forwarding, and the concurrent collector's snapshot becoming an
arena copy with offset translation. That is the memory subsystem of a
production VM. For a portfolio-scale language whose heap ceiling is
megabytes, the payoff cannot justify destabilizing the invariant stack
that weak refs and concurrent marking were verified against.

## 4. The adjacent win that is real (if footprint ever matters)

The VM does hold memory it never returns: `HashMap`/`HashSet` capacities
(object table, nursery set, remembered set) and per-object `Vec`
capacities only grow. A post-major `shrink_to_fit` pass — "footprint
compaction" — would return real bytes to the allocator at a measurable,
bounded cost, with zero movement and zero invariant risk. It was
considered and deliberately not implemented now (no workload demands it);
it is the first thing to reach for if RSS ever becomes a complaint, and
it should be labeled footprint trimming, not mark-compact.

## 5. Conclusion

Park compaction indefinitely. The three sub-goals resolve honestly as:
mark-compact — architecture mismatch, cosmetic at header level, rewrite
at payload level; fragmentation — owned by the system allocator, not the
VM; cache locality — determined by payload placement the VM does not
control. If GC work continues, the valuable directions are the ones the
current design rewards: smarter minor/major scheduling, allocation-rate
feedback, and (if ever needed) footprint trimming — not moving objects.
