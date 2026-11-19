/*
 * Copyright 2026 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Lookup maps for registered static heaps.
//!
//! Heaps registered through [`StaticHeapEntry`] (`globals_static!` /
//! `methods_static!`) are process-wide singletons: they are always resident,
//! and every value allocated in them serializes as a `StaticValueId`.
//! Serialized data therefore references these heaps by [`StaticHeapId`] and
//! resolves back to the live heap, instead of paging their bodies through
//! storage.

use std::collections::HashMap;
use std::sync::LazyLock;

use pagable::Pagable;

use crate::pagable::static_value::registry::StaticHeapEntry;
use crate::values::layout::heap::sealed::FrozenHeapArc;
use crate::values::layout::heap::sealed::FrozenHeapPtr;

/// A unique identifier for a registered static heap.
///
/// A dense index assigned at registry-build time by enumerating the
/// [`StaticHeapEntry`] inventory in deterministic (file, line) order, so
/// serializer and deserializer in the same binary always agree (the same
/// assumption `StaticValueId` already makes). Heap names cannot serve as the
/// identifier: distinct static heaps can share a name (`<module_path>::<NAME>`),
/// e.g. two `globals_static!` in different `fn` bodies of one module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Pagable)]
pub(crate) struct StaticHeapId(u32);

struct StaticHeapMaps {
    /// Heap allocation identity → id (serialization).
    ptr_to_id: HashMap<FrozenHeapPtr, StaticHeapId>,
    /// Id → registered static heap (deserialization resolution).
    id_to_heap: Vec<&'static FrozenHeapArc>,
}

static STATIC_HEAP_MAPS: LazyLock<StaticHeapMaps> = LazyLock::new(|| {
    let mut ptr_to_id = HashMap::new();
    let mut id_to_heap: Vec<&'static FrozenHeapArc> = Vec::new();
    for entry in StaticHeapEntry::iter_sorted() {
        let heap = (entry.get_heap)().heap_arc();
        // An empty static heap has no inner allocation; it serializes as the
        // `None` ref and never needs an id.
        let Some(ptr) = heap.downgrade().map(|weak| weak.heap_ptr()) else {
            continue;
        };
        // One id per distinct allocation, even if the same allocation is
        // registered through more than one entry.
        if ptr_to_id.contains_key(&ptr) {
            continue;
        }
        let id = StaticHeapId(
            u32::try_from(id_to_heap.len()).expect("static heap count should fit in u32"),
        );
        ptr_to_id.insert(ptr, id);
        id_to_heap.push(heap);
    }
    StaticHeapMaps {
        ptr_to_id,
        id_to_heap,
    }
});

/// The id of `heap`, if it is one of the process-wide static heaps registered
/// through [`StaticHeapEntry`].
pub(crate) fn get_static_heap_id(heap: &FrozenHeapArc) -> Option<StaticHeapId> {
    let weak = heap.downgrade()?;
    STATIC_HEAP_MAPS.ptr_to_id.get(&weak.heap_ptr()).copied()
}

/// Resolve a registered static heap by id.
pub(crate) fn get_static_heap_by_id(id: StaticHeapId) -> Option<&'static FrozenHeapArc> {
    STATIC_HEAP_MAPS.id_to_heap.get(id.0 as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_round_trips_to_the_same_allocation() {
        let mut checked = 0;
        for entry in StaticHeapEntry::iter_sorted() {
            let heap = (entry.get_heap)().heap_arc();
            let Some(id) = get_static_heap_id(heap) else {
                // Empty heaps are not registered.
                continue;
            };
            let resolved = get_static_heap_by_id(id).expect("assigned ids should resolve");
            assert!(
                *resolved == *heap,
                "id {id:?} should resolve to the allocation it was assigned to"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "the starlark crate itself registers static heaps"
        );
    }
}
