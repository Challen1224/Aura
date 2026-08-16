//! Managed heap and mark-and-sweep garbage collector.
//!
//! The heap stores [`AuraObject`] values behind opaque [`GcRef`] handles. It
//! keeps an object map indexed by generation counter so handles stay stable
//! across collections. A GC is triggered when allocated bytes exceed a
//! configurable threshold.

use aura_bytecode::{AuraObject, EnumValue, GcRef, TupleValue, Value};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Objects deleted per stop-the-world slice when applying a concurrent
/// cycle's dead set (bounds sweep pauses).
const SWEEP_CHUNK: usize = 512;

/// Default heap size threshold that triggers a collection.
const DEFAULT_GC_THRESHOLD: usize = 64 * 1024;

/// Errors that can occur during heap operations.
#[derive(Debug, thiserror::Error)]
pub enum HeapError {
    /// Tried to access an invalid or collected handle.
    #[error("invalid heap reference {0:?}")]
    InvalidRef(GcRef),
}

/// A managed heap.
#[derive(Debug, Default)]
pub struct Heap {
    /// Generation counter used to produce unique handles.
    next_gen: usize,
    /// Live objects keyed by handle.
    objects: HashMap<GcRef, HeapObject>,
    /// Approximate allocated bytes since last collection.
    allocated: usize,
    /// Threshold that triggers the next collection.
    threshold: usize,
    /// Set when `allocated` crosses `threshold`; the VM collects at its next
    /// safepoint. Allocation never collects inline, because callers routinely
    /// hold freshly-allocated handles in Rust locals the collector cannot see.
    gc_pending: bool,
    /// Number of collections run (for tests and diagnostics).
    collections: u64,
    /// Minor (nursery-only) collections run.
    minor_collections: u64,
    /// Major (full-heap) collections run.
    major_collections: u64,
    /// Total objects ever allocated (monotonic; for tests and diagnostics).
    total_allocations: u64,
    /// The nursery: handles allocated since the last collection. Objects
    /// never move (handles are stable), so a generation is a set
    /// membership, not a memory region.
    young: HashSet<GcRef>,
    /// Approximate bytes held by nursery objects.
    young_bytes: usize,
    /// Write barrier log: old objects mutated since the last collection.
    /// `get_mut` is the single mutation gateway, so logging there is a
    /// sound (object-granularity, over-approximate) remembered set — the
    /// only old objects that can point into the nursery.
    remembered: HashSet<GcRef>,
    /// Explicit nursery size, overriding the derived default. Also the
    /// lever the pause-target feedback loop adjusts.
    nursery_override: Option<usize>,
    /// Hard heap limit: exceeding it after a full collection is an
    /// out-of-memory error surfaced by the VM.
    max_heap: Option<usize>,
    /// Full-heap threshold growth factor after a major collection
    /// (throughput mode grows faster: fewer, bigger collections).
    growth_factor: usize,
    /// Best-effort soft target for *minor* pause times: each minor pause
    /// over target halves the nursery (less work per pause), well under
    /// target doubles it. Majors are stop-the-world and unbounded.
    pause_target: Option<Duration>,
    /// Cumulative pause time in minor collections.
    minor_pause_total: Duration,
    /// Cumulative pause time in major collections.
    major_pause_total: Duration,
    /// Longest single minor pause.
    max_minor_pause: Duration,
    /// Longest single major pause.
    max_major_pause: Duration,
    /// Approximate bytes reclaimed across all collections.
    bytes_freed: u64,
    /// Concurrent collection enabled (opt-in): majors become
    /// snapshot-at-the-beginning cycles marked on a background thread.
    concurrent: bool,
    /// The in-flight concurrent cycle, if any.
    cycle: Option<ConcurrentCycle>,
    /// Dead handles from a finished cycle, awaiting chunked deletion.
    pending_dead: Vec<GcRef>,
    /// Completed concurrent cycles.
    concurrent_cycles: u64,
    /// Cumulative background marking time (runs off-thread: informational,
    /// not a pause).
    concurrent_mark_total: Duration,
}

/// An in-flight concurrent marking cycle: the background thread owns a
/// deep snapshot of the heap taken at a safepoint and sends back the set
/// of handles that were unreachable *in that snapshot*. Handles are
/// allocated monotonically and never reused, so snapshot-time
/// unreachability is permanent (classic snapshot-at-the-beginning): the
/// dead set can be applied at any later safepoint, and anything
/// allocated after the snapshot is implicitly live.
#[derive(Debug)]
struct ConcurrentCycle {
    rx: mpsc::Receiver<MarkResult>,
    handle: std::thread::JoinHandle<()>,
}

#[derive(Debug)]
struct MarkResult {
    dead: Vec<GcRef>,
    mark_time: Duration,
}

/// Collector disposition presets: how to trade pause size against
/// collection frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    /// Larger nursery, faster threshold growth: fewer, bigger pauses.
    Throughput,
    /// The defaults.
    Balanced,
    /// Small nursery: frequent, small minor pauses.
    Latency,
}

/// A point-in-time snapshot of collector counters and configuration.
#[derive(Debug, Clone)]
pub struct GcStats {
    /// Total collections (minor + major).
    pub collections: u64,
    /// Minor (nursery-only) collections.
    pub minor_collections: u64,
    /// Major (full-heap) collections.
    pub major_collections: u64,
    /// Live objects right now.
    pub live_objects: usize,
    /// Approximate live bytes right now.
    pub live_bytes: usize,
    /// Objects ever allocated.
    pub total_allocations: u64,
    /// Approximate bytes reclaimed across all collections.
    pub bytes_freed: u64,
    /// Current full-heap threshold.
    pub threshold: usize,
    /// Current effective nursery size.
    pub nursery_size: usize,
    /// Cumulative minor pause time.
    pub minor_pause_total: Duration,
    /// Cumulative major pause time.
    pub major_pause_total: Duration,
    /// Longest single minor pause.
    pub max_minor_pause: Duration,
    /// Longest single major pause.
    pub max_major_pause: Duration,
    /// Completed concurrent marking cycles.
    pub concurrent_cycles: u64,
    /// Cumulative background marking time (off-thread, not a pause).
    pub concurrent_mark_total: Duration,
}

#[derive(Debug, Clone)]
struct HeapObject {
    object: AuraObject,
    marked: bool,
}

impl Heap {
    /// Create a new heap with the default collection threshold.
    pub fn new() -> Self {
        Self::with_threshold(DEFAULT_GC_THRESHOLD)
    }

    /// Create a new heap with a custom collection threshold.
    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            next_gen: 1,
            objects: HashMap::new(),
            allocated: 0,
            threshold,
            gc_pending: false,
            collections: 0,
            minor_collections: 0,
            major_collections: 0,
            total_allocations: 0,
            young: HashSet::new(),
            young_bytes: 0,
            remembered: HashSet::new(),
            nursery_override: None,
            max_heap: None,
            growth_factor: 2,
            pause_target: None,
            minor_pause_total: Duration::ZERO,
            major_pause_total: Duration::ZERO,
            max_minor_pause: Duration::ZERO,
            max_major_pause: Duration::ZERO,
            bytes_freed: 0,
            concurrent: false,
            cycle: None,
            pending_dead: Vec::new(),
            concurrent_cycles: 0,
            concurrent_mark_total: Duration::ZERO,
        }
    }

    /// Enable or disable the concurrent collector (majors become
    /// background marking cycles; minors stay stop-the-world).
    pub fn set_concurrent(&mut self, enabled: bool) {
        self.concurrent = enabled;
    }

    /// Set an explicit nursery size (None restores the derived default).
    pub fn set_nursery_size(&mut self, bytes: Option<usize>) {
        self.nursery_override = bytes;
    }

    /// Set a hard heap limit (checked after full collections).
    pub fn set_max_heap(&mut self, bytes: Option<usize>) {
        self.max_heap = bytes;
    }

    /// Live bytes exceeding the configured limit after the last full
    /// collection, as (live, limit).
    pub fn over_max_heap(&self) -> Option<(usize, usize)> {
        match self.max_heap {
            Some(limit) if self.allocated > limit => Some((self.allocated, limit)),
            _ => None,
        }
    }

    /// Best-effort soft target for minor pause times (None disables the
    /// feedback loop).
    pub fn set_pause_target(&mut self, target: Option<Duration>) {
        self.pause_target = target;
    }

    /// Apply a disposition preset.
    pub fn set_mode(&mut self, mode: GcMode) {
        match mode {
            GcMode::Throughput => {
                self.nursery_override = Some(256 * 1024);
                self.growth_factor = 3;
            }
            GcMode::Balanced => {
                self.nursery_override = None;
                self.growth_factor = 2;
            }
            GcMode::Latency => {
                self.nursery_override = Some(16 * 1024);
                self.growth_factor = 2;
            }
        }
    }

    /// Snapshot the collector's counters and configuration.
    pub fn stats(&self) -> GcStats {
        GcStats {
            collections: self.collections,
            minor_collections: self.minor_collections,
            major_collections: self.major_collections,
            live_objects: self.objects.len(),
            live_bytes: self.allocated,
            total_allocations: self.total_allocations,
            bytes_freed: self.bytes_freed,
            threshold: self.threshold,
            nursery_size: self.minor_threshold(),
            minor_pause_total: self.minor_pause_total,
            major_pause_total: self.major_pause_total,
            max_minor_pause: self.max_minor_pause,
            max_major_pause: self.max_major_pause,
            concurrent_cycles: self.concurrent_cycles,
            concurrent_mark_total: self.concurrent_mark_total,
        }
    }

    /// Replace the collection threshold (also re-arms the pending check).
    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
        if self.allocated >= self.threshold || self.young_bytes >= self.minor_threshold() {
            self.gc_pending = true;
        }
    }

    /// True if a collection should run at the next safepoint.
    pub fn needs_collect(&self) -> bool {
        self.gc_pending
    }

    /// Second-stage safepoint gate: whether this safepoint actually needs
    /// the (expensive) root scan and a `collect` call. In stop-the-world
    /// mode: always. In concurrent mode, a safepoint that lands while the
    /// background marker is still running — with no finished results, no
    /// nursery pressure, and no limit or backpressure concern — has
    /// nothing to do; this returns false and disarms the pending flag
    /// (the next allocation over threshold re-arms it).
    pub fn should_pause(&mut self) -> bool {
        if !self.concurrent {
            return true;
        }
        self.poll_cycle();
        if !self.pending_dead.is_empty()
            || self.cycle.is_none()
            || (self.young_bytes >= self.minor_threshold() && !self.young.is_empty())
            || self.allocated >= self.threshold.saturating_mul(2)
            || self.max_heap.is_some_and(|limit| self.allocated > limit)
        {
            return true;
        }
        self.gc_pending = false;
        false
    }

    /// Number of collections run so far (minor and major combined).
    pub fn collections(&self) -> u64 {
        self.collections
    }

    /// Number of minor (nursery-only) collections run so far.
    pub fn minor_collections(&self) -> u64 {
        self.minor_collections
    }

    /// Number of major (full-heap) collections run so far.
    pub fn major_collections(&self) -> u64 {
        self.major_collections
    }

    /// Total number of objects ever allocated (monotonic).
    pub fn total_allocations(&self) -> u64 {
        self.total_allocations
    }

    /// True if `handle` refers to a live object.
    pub fn contains(&self, handle: GcRef) -> bool {
        self.objects.contains_key(&handle)
    }

    /// Allocate a new object on the heap.
    pub fn allocate(&mut self, object: AuraObject) -> GcRef {
        let handle = GcRef(self.next_gen);
        self.next_gen += 1;
        let size = Self::approx_size(&object);
        self.allocated += size;
        self.young.insert(handle);
        self.young_bytes += size;
        self.objects.insert(
            handle,
            HeapObject {
                object,
                marked: false,
            },
        );

        self.total_allocations += 1;
        if self.allocated >= self.threshold
            || self.young_bytes >= self.minor_threshold()
            || self.max_heap.is_some_and(|limit| self.allocated > limit)
        {
            // Don't collect here: the caller may hold unrooted handles in
            // Rust locals. Flag the need and let the VM collect at a
            // safepoint where every live reference is visible.
            self.gc_pending = true;
        }

        handle
    }

    /// Borrow an object from the heap.
    pub fn get(&self, handle: GcRef) -> Result<&AuraObject, HeapError> {
        self.objects
            .get(&handle)
            .map(|ho| &ho.object)
            .ok_or(HeapError::InvalidRef(handle))
    }

    /// Mutably borrow an object from the heap. Doubles as the write
    /// barrier: a mutable borrow of an old object may store nursery
    /// references into it, so it joins the remembered set until the next
    /// collection.
    pub fn get_mut(&mut self, handle: GcRef) -> Result<&mut AuraObject, HeapError> {
        if !self.young.contains(&handle) && self.objects.contains_key(&handle) {
            self.remembered.insert(handle);
        }
        self.objects
            .get_mut(&handle)
            .map(|ho| &mut ho.object)
            .ok_or(HeapError::InvalidRef(handle))
    }

    /// Resolve a string object's contents.
    pub fn get_string(&self, handle: GcRef) -> Option<&str> {
        self.objects.get(&handle).and_then(|ho| match &ho.object {
            AuraObject::String(s) => Some(s.as_str()),
            _ => None,
        })
    }

    /// Borrow an enum payload from the heap.
    pub fn get_enum(&self, handle: GcRef) -> Result<&EnumValue, HeapError> {
        match self.get(handle)? {
            AuraObject::Enum(e) => Ok(e),
            _ => Err(HeapError::InvalidRef(handle)),
        }
    }

    /// Borrow a tuple payload from the heap.
    pub fn get_tuple(&self, handle: GcRef) -> Result<&TupleValue, HeapError> {
        match self.get(handle)? {
            AuraObject::Tuple(t) => Ok(t),
            _ => Err(HeapError::InvalidRef(handle)),
        }
    }

    /// Nursery pressure that triggers a minor collection. Half the full
    /// threshold for small heaps, but capped: the nursery stays small even
    /// when a large stable old generation has grown the full threshold, so
    /// minor collections stay frequent and cheap while majors stay rare.
    fn minor_threshold(&self) -> usize {
        self.nursery_override
            .unwrap_or_else(|| (self.threshold / 2).min(DEFAULT_GC_THRESHOLD))
            .max(1)
    }

    /// Run a collection given the root set: a minor (nursery-only) pass
    /// when only the nursery is under pressure, escalating to a full
    /// mark-and-sweep when the whole heap is.
    pub fn collect(&mut self, roots: &[GcRef]) {
        if let Some(limit) = self.max_heap {
            if self.allocated > limit {
                // Hard-limit pressure is handled synchronously so the
                // limit stays exact: finish any outstanding concurrent
                // cycle first, then take the stop-the-world path.
                self.finish_cycle_blocking();
                self.collect_major(roots);
                if self.allocated > limit && self.clear_soft_refs() {
                    // Softly-held memory is the last thing to go before a
                    // heap-limit error: clear every soft reference and
                    // sweep again with their targets unreachable.
                    self.collect_major(roots);
                }
                return;
            }
        }
        if self.concurrent {
            self.collect_concurrent(roots);
            return;
        }
        if self.allocated < self.threshold && !self.young.is_empty() {
            self.collect_minor(roots);
            if self.allocated < self.threshold {
                return;
            }
            // The nursery pass was not enough; fall through to a full pass.
        }
        self.collect_major(roots);
    }

    /// One concurrent-mode safepoint: apply finished marking results in a
    /// bounded chunk, keep the nursery collected with ordinary
    /// stop-the-world minors, start a new cycle when the heap is over
    /// threshold, and stall on a badly outrun marker (backpressure) so
    /// the heap cannot grow unboundedly.
    fn collect_concurrent(&mut self, roots: &[GcRef]) {
        self.poll_cycle();
        if !self.pending_dead.is_empty() {
            self.apply_dead_chunk();
            self.gc_pending = !self.pending_dead.is_empty()
                || self.allocated >= self.threshold
                || self.young_bytes >= self.minor_threshold();
            return;
        }
        if !self.young.is_empty()
            && (self.young_bytes >= self.minor_threshold() || self.allocated >= self.threshold)
        {
            self.collect_minor(roots);
        }
        if self.cycle.is_some() {
            if self.allocated >= self.threshold.saturating_mul(2) {
                // Backpressure: the mutator has outrun the marker badly;
                // stall on the cycle rather than let the heap balloon.
                self.finish_cycle_blocking();
            } else {
                self.gc_pending = false;
            }
            return;
        }
        if self.allocated >= self.threshold {
            self.start_cycle(roots);
            self.gc_pending = false;
        }
    }

    /// Begin a concurrent cycle: the stop-the-world cost is a deep clone
    /// of the object map (no tracing), handed to a background thread that
    /// does the entire mark over the snapshot.
    fn start_cycle(&mut self, roots: &[GcRef]) {
        debug_assert!(self.cycle.is_none() && self.pending_dead.is_empty());
        let started = Instant::now();
        let snapshot = self.objects.clone();
        let roots_v: Vec<GcRef> = roots.to_vec();
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mark_started = Instant::now();
            let mut visited: HashSet<GcRef> = HashSet::new();
            let mut work = roots_v;
            while let Some(h) = work.pop() {
                if !visited.insert(h) {
                    continue;
                }
                if let Some(ho) = snapshot.get(&h) {
                    work.extend(ho.object.references());
                }
            }
            let dead: Vec<GcRef> = snapshot
                .keys()
                .filter(|k| !visited.contains(k))
                .copied()
                .collect();
            let _ = tx.send(MarkResult {
                dead,
                mark_time: mark_started.elapsed(),
            });
            // The snapshot drops here — deallocation is off-thread too.
        });
        self.cycle = Some(ConcurrentCycle { rx, handle });
        let pause = started.elapsed();
        self.major_pause_total += pause;
        self.max_major_pause = self.max_major_pause.max(pause);
    }

    /// Non-blocking check for finished marking results.
    fn poll_cycle(&mut self) {
        let done = match &self.cycle {
            Some(c) => match c.rx.try_recv() {
                Ok(res) => Some(res),
                Err(mpsc::TryRecvError::Empty) => return,
                // The marker thread died without sending (it has no real
                // panic paths); drop the cycle and carry on uncollected.
                Err(mpsc::TryRecvError::Disconnected) => None,
            },
            None => return,
        };
        if let Some(c) = self.cycle.take() {
            let _ = c.handle.join();
        }
        if let Some(res) = done {
            self.pending_dead = res.dead;
            self.concurrent_mark_total += res.mark_time;
        }
    }

    /// Delete up to [`SWEEP_CHUNK`] dead objects (one bounded
    /// stop-the-world slice); finishes the cycle's accounting when the
    /// set drains.
    fn apply_dead_chunk(&mut self) {
        if self.pending_dead.is_empty() {
            return;
        }
        let started = Instant::now();
        let take = self.pending_dead.len().min(SWEEP_CHUNK);
        let split = self.pending_dead.len() - take;
        for h in self.pending_dead.split_off(split) {
            // A minor collection may already have freed a snapshot-young
            // object; `remove` returning None makes that a no-op.
            if let Some(ho) = self.objects.remove(&h) {
                let size = Self::approx_size(&ho.object);
                self.allocated = self.allocated.saturating_sub(size);
                if self.young.remove(&h) {
                    self.young_bytes = self.young_bytes.saturating_sub(size);
                }
                self.remembered.remove(&h);
                self.bytes_freed += size as u64;
            }
        }
        let pause = started.elapsed();
        self.major_pause_total += pause;
        self.max_major_pause = self.max_major_pause.max(pause);
        if self.pending_dead.is_empty() {
            // Cycle fully applied: adaptive growth, same as a
            // stop-the-world major.
            self.threshold = (self.allocated * self.growth_factor)
                .max(self.threshold.min(DEFAULT_GC_THRESHOLD))
                .max(1);
            self.collections += 1;
            self.major_collections += 1;
            self.concurrent_cycles += 1;
        }
    }

    /// Join the in-flight cycle (if any) and apply its entire dead set
    /// now. Used for hard-limit pressure, backpressure stalls, and tests
    /// that need reclamation settled.
    pub fn finish_cycle_blocking(&mut self) {
        if let Some(c) = self.cycle.take() {
            if let Ok(res) = c.rx.recv() {
                self.pending_dead.extend(res.dead);
                self.concurrent_mark_total += res.mark_time;
            }
            let _ = c.handle.join();
        }
        while !self.pending_dead.is_empty() {
            self.apply_dead_chunk();
        }
    }

    /// Clear every soft reference (pressure response). Returns whether
    /// any reference was actually cleared. Direct map mutation is safe
    /// here: clearing only removes an edge, so it can never create an
    /// old-to-young pointer the remembered set would need to see.
    fn clear_soft_refs(&mut self) -> bool {
        let mut cleared = false;
        for ho in self.objects.values_mut() {
            if let AuraObject::SoftRef { target } = &mut ho.object {
                cleared |= target.take().is_some();
            }
        }
        cleared
    }

    /// Minor collection: trace only the nursery. Old objects are terminal
    /// during the trace — an old object can only point into the nursery if
    /// it was mutated since the last collection, and `get_mut` logged every
    /// such object in the remembered set, whose children seed the trace.
    /// Survivors are promoted (the nursery empties either way).
    fn collect_minor(&mut self, roots: &[GcRef]) {
        let started = Instant::now();
        let allocated_before = self.allocated;
        let mut work: Vec<GcRef> = roots.to_vec();
        for handle in &self.remembered {
            if let Some(ho) = self.objects.get(handle) {
                work.extend(ho.object.references());
            }
        }
        let mut visited: HashSet<GcRef> = HashSet::new();
        while let Some(handle) = work.pop() {
            if !visited.insert(handle) {
                continue;
            }
            if !self.young.contains(&handle) {
                continue;
            }
            if let Some(ho) = self.objects.get_mut(&handle) {
                ho.marked = true;
                for child in ho.object.references() {
                    work.push(child);
                }
            }
        }
        for handle in std::mem::take(&mut self.young) {
            let dead = match self.objects.get_mut(&handle) {
                Some(ho) => {
                    if ho.marked {
                        ho.marked = false;
                        false
                    } else {
                        true
                    }
                }
                None => continue,
            };
            if dead {
                if let Some(ho) = self.objects.remove(&handle) {
                    self.allocated =
                        self.allocated.saturating_sub(Self::approx_size(&ho.object));
                }
            }
            // Survivors stay out of `young`: promoted to the old
            // generation in place.
        }
        self.young_bytes = 0;
        self.remembered.clear();
        self.gc_pending = self.allocated >= self.threshold;
        self.collections += 1;
        self.minor_collections += 1;
        self.bytes_freed += allocated_before.saturating_sub(self.allocated) as u64;
        let pause = started.elapsed();
        self.minor_pause_total += pause;
        self.max_minor_pause = self.max_minor_pause.max(pause);
        if let Some(target) = self.pause_target {
            // Best-effort feedback: shrink the nursery when minors run
            // over target (less work per pause), grow it back when far
            // under (fewer pauses). Clamped to [8KB, 1MB].
            let current = self.minor_threshold();
            if pause > target {
                self.nursery_override = Some((current / 2).max(8 * 1024));
            } else if pause * 4 < target {
                self.nursery_override = Some((current * 2).min(1024 * 1024));
            }
        }
    }

    /// Major collection: full mark-and-sweep over every generation.
    fn collect_major(&mut self, roots: &[GcRef]) {
        let started = Instant::now();
        let allocated_before = self.allocated;
        let mut work: Vec<GcRef> = roots.to_vec();
        let mut visited: HashSet<GcRef> = HashSet::new();

        while let Some(handle) = work.pop() {
            if !visited.insert(handle) {
                continue;
            }
            if let Some(ho) = self.objects.get_mut(&handle) {
                ho.marked = true;
                for child in ho.object.references() {
                    work.push(child);
                }
            }
        }

        self.objects.retain(|_, ho| {
            if ho.marked {
                ho.marked = false;
                true
            } else {
                false
            }
        });

        // Every survivor is old now.
        self.young.clear();
        self.young_bytes = 0;
        self.remembered.clear();

        self.allocated = self.objects.values().map(|ho| Self::approx_size(&ho.object)).sum();
        self.threshold = (self.allocated * self.growth_factor)
            .max(self.threshold.min(DEFAULT_GC_THRESHOLD))
            .max(1);
        self.gc_pending = self.allocated >= self.threshold
            || self.young_bytes >= self.minor_threshold();
        self.collections += 1;
        self.major_collections += 1;
        self.bytes_freed += allocated_before.saturating_sub(self.allocated) as u64;
        let pause = started.elapsed();
        self.major_pause_total += pause;
        self.max_major_pause = self.max_major_pause.max(pause);
    }

    /// Number of live objects.
    pub fn live_count(&self) -> usize {
        self.objects.len()
    }

    fn approx_size(object: &AuraObject) -> usize {
        match object {
            AuraObject::String(s) => s.len(),
            AuraObject::Instance { fields, .. }
            | AuraObject::Array { elements: fields }
            | AuraObject::Set { elements: fields, .. }
            | AuraObject::Closure { captured: fields, .. } => {
                fields.len() * std::mem::size_of::<Value>() + std::mem::size_of::<AuraObject>()
            }
            AuraObject::Map { entries, .. } => {
                entries.len() * 2 * std::mem::size_of::<Value>() + std::mem::size_of::<AuraObject>()
            }
            AuraObject::Enum(e) => {
                e.fields.len() * std::mem::size_of::<Value>() + std::mem::size_of::<AuraObject>()
            }
            AuraObject::Tuple(t) => {
                t.elements.len() * std::mem::size_of::<Value>() + std::mem::size_of::<AuraObject>()
            }
            AuraObject::Task { .. }
            | AuraObject::WeakRef { .. }
            | AuraObject::SoftRef { .. }
            | AuraObject::PhantomRef { .. } => std::mem::size_of::<AuraObject>(),
        }
    }
}
