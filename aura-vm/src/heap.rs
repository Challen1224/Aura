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
    /// Total objects ever allocated (monotonic; for tests and diagnostics).
    total_allocations: u64,
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
            total_allocations: 0,
        }
    }

    /// Replace the collection threshold (also re-arms the pending check).
    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
        if self.allocated >= self.threshold {
            self.gc_pending = true;
        }
    }

    /// True if a collection should run at the next safepoint.
    pub fn needs_collect(&self) -> bool {
        self.gc_pending
    }

    /// Number of collections run so far.
    pub fn collections(&self) -> u64 {
        self.collections
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
        self.allocated += Self::approx_size(&object);
        self.objects.insert(
            handle,
            HeapObject {
                object,
                marked: false,
            },
        );

        self.total_allocations += 1;
        if self.allocated >= self.threshold {
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

    /// Mutably borrow an object from the heap.
    pub fn get_mut(&mut self, handle: GcRef) -> Result<&mut AuraObject, HeapError> {
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

    /// Run a mark-and-sweep collection given the root set.
    pub fn collect(&mut self, roots: &[GcRef]) {
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

        self.allocated = self.objects.values().map(|ho| Self::approx_size(&ho.object)).sum();
        self.threshold = (self.allocated * 2).max(self.threshold.min(DEFAULT_GC_THRESHOLD)).max(1);
        self.gc_pending = self.allocated >= self.threshold;
        self.collections += 1;
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
            | AuraObject::Set { elements: fields, .. } => {
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
        }
    }
}
