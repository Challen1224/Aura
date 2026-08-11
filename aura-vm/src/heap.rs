//! Managed heap and mark-and-sweep garbage collector.
//!
//! The heap stores [`AuraObject`] values behind opaque [`GcRef`] handles. It
//! keeps an object map indexed by generation counter so handles stay stable
//! across collections. A GC is triggered when allocated bytes exceed a
//! configurable threshold.

use aura_bytecode::{AuraObject, GcRef, Value};
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
        }
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

        if self.allocated >= self.threshold {
            // A real VM would invoke GC here. We expose `collect` explicitly
            // so the VM can provide root references safely.
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
        self.threshold = (self.allocated * 2).max(DEFAULT_GC_THRESHOLD);
    }

    /// Number of live objects.
    pub fn live_count(&self) -> usize {
        self.objects.len()
    }

    fn approx_size(object: &AuraObject) -> usize {
        match object {
            AuraObject::String(s) => s.len(),
            AuraObject::Instance { fields, .. } | AuraObject::Array { elements: fields } => {
                fields.len() * std::mem::size_of::<Value>() + std::mem::size_of::<AuraObject>()
            }
        }
    }
}
