//! Managed heap and mark-and-sweep garbage collector.
//!
//! The heap stores [`AuraObject`] values behind opaque [`GcRef`] handles. It
//! keeps an object map indexed by generation counter so handles stay stable
//! across collections. A GC is triggered when allocated bytes exceed a
//! configurable threshold.

use aura_bytecode::{AuraObject, EnumValue, GcRef, TupleValue, Value};
use std::collections::{HashMap, HashSet};

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
        if self.allocated >= self.threshold || self.young_bytes >= self.minor_threshold() {
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
        (self.threshold / 2).min(DEFAULT_GC_THRESHOLD).max(1)
    }

    /// Run a collection given the root set: a minor (nursery-only) pass
    /// when only the nursery is under pressure, escalating to a full
    /// mark-and-sweep when the whole heap is.
    pub fn collect(&mut self, roots: &[GcRef]) {
        if self.allocated < self.threshold && !self.young.is_empty() {
            self.collect_minor(roots);
            if self.allocated < self.threshold {
                return;
            }
            // The nursery pass was not enough; fall through to a full pass.
        }
        self.collect_major(roots);
    }

    /// Minor collection: trace only the nursery. Old objects are terminal
    /// during the trace — an old object can only point into the nursery if
    /// it was mutated since the last collection, and `get_mut` logged every
    /// such object in the remembered set, whose children seed the trace.
    /// Survivors are promoted (the nursery empties either way).
    fn collect_minor(&mut self, roots: &[GcRef]) {
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
    }

    /// Major collection: full mark-and-sweep over every generation.
    fn collect_major(&mut self, roots: &[GcRef]) {
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
        self.threshold = (self.allocated * 2).max(self.threshold.min(DEFAULT_GC_THRESHOLD)).max(1);
        self.gc_pending = self.allocated >= self.threshold
            || self.young_bytes >= self.minor_threshold();
        self.collections += 1;
        self.major_collections += 1;
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
            AuraObject::Task { .. } => std::mem::size_of::<AuraObject>(),
        }
    }
}
