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

//! Lookup function for static value identifiers.
//!
//! This module provides the main entry point for checking if a value
//! is a static value and retrieving its identifier.

use std::collections::HashMap;
use std::sync::LazyLock;

use itertools::Itertools;
use pagable::Pagable;

use super::registry::StaticValueEntry;
use crate::pagable::static_value::registry::StaticHeapEntry;
use crate::pagable::static_value::static_string::get_static_strings;
use crate::values::Value;
use crate::values::layout::vtable::StarlarkValueRawPtr;

/// A unique identifier for a statically-allocated Starlark value.
///
/// This is a dense index assigned at registry-build time: each statically
/// allocated value (static strings and inventory-registered entries) gets
/// the next available index in `id_to_value`, so IDs form a contiguous
/// `0..N` range and round-trip through the `Vec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Pagable)]
pub(crate) struct StaticValueId(u64);

impl StaticValueId {
    /// Create a `StaticValueId` from a raw index.
    #[inline]
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Bidirectional lookup maps for static values.
///
/// Built once from both static strings and inventory-registered entries.
/// The two maps are inverses of each other:
/// - `addr_to_id`: pointer address → `StaticValueId` (for serialization)
/// - `id_to_value`: `StaticValueId` → value (for deserialization)
struct StaticValueMaps {
    addr_to_id: HashMap<usize, StaticValueId>,
    id_to_value: Vec<Value<'static>>,
}

static STATIC_VALUE_MAPS: LazyLock<StaticValueMaps> = LazyLock::new(|| {
    let mut addr_to_id = HashMap::new();
    let mut id_to_value = Vec::new();

    for (id, v) in get_all_static_values().enumerate() {
        addr_to_id.insert(value_to_raw(v).ptr as usize, StaticValueId::new(id as u64));
        id_to_value.push(v);
    }

    StaticValueMaps {
        addr_to_id,
        id_to_value,
    }
});

fn value_to_raw(v: Value<'_>) -> StarlarkValueRawPtr {
    v.get_ref().value
}

fn get_all_static_values() -> impl Iterator<Item = Value<'static>> {
    get_static_strings()
        .chain(get_static_values())
        .chain(get_static_heap_values())
}

fn get_static_values() -> impl Iterator<Item = Value<'static>> {
    inventory::iter::<StaticValueEntry>()
        .sorted_by_key(|v| (&v.file, &v.line))
        .map(|e| (e.get_value)())
}

fn get_static_heap_values() -> impl Iterator<Item = Value<'static>> {
    StaticHeapEntry::iter_sorted().flat_map(|e| (e.get_heap)().heap_arc().iter_values())
}

/// Look up a static value by its `StaticValueId`.
///
/// Used during deserialization to resolve static value references.
pub(crate) fn get_static_value_by_id(id: StaticValueId) -> Option<Value<'static>> {
    STATIC_VALUE_MAPS.id_to_value.get(id.0 as usize).copied()
}

/// Check if a value points to a static value.
///
/// Returns `Some(StaticValueId)` if the value is a registered static value,
/// `None` otherwise.
pub(crate) fn get_static_value_id(v: Value<'_>) -> Option<StaticValueId> {
    // Only check for pointer values, not inline integers
    if v.ptr_value().is_int() {
        return None;
    }

    let addr = value_to_raw(v).ptr as usize;
    STATIC_VALUE_MAPS.addr_to_id.get(&addr).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::const_frozen_string;
    use crate::values::layout::static_string::constant_string;

    #[test]
    fn test_empty_string_has_id() {
        let empty = constant_string("").unwrap();
        let id = get_static_value_id(empty.to_value());
        assert!(id.is_some(), "empty string should be registered");
    }

    #[test]
    fn test_single_char_has_id() {
        let a = constant_string("a").unwrap();
        let id = get_static_value_id(a.to_value());
        assert!(id.is_some(), "'a' should be registered");
    }

    #[test]
    fn test_multi_char_string_is_registered() {
        let s = const_frozen_string!("hello");
        let id = get_static_value_id(s.to_value());
        assert!(id.is_some(), "const_frozen_string! should be registered");
    }

    #[test]
    fn test_all_ascii_chars_registered() {
        for i in 0u8..128 {
            let bytes = [i];
            let s = std::str::from_utf8(&bytes).unwrap();
            let fv = constant_string(s).unwrap();
            let id = get_static_value_id(fv.to_value());
            assert!(id.is_some(), "ASCII char {} should be registered", i);
        }
    }

    #[test]
    fn test_all_short_string_ids_unique() {
        let mut ids = std::collections::HashSet::new();
        // empty string
        let empty = constant_string("").unwrap();
        ids.insert(get_static_value_id(empty.to_value()).unwrap());
        // all 128 ASCII chars
        for i in 0u8..128 {
            let bytes = [i];
            let s = std::str::from_utf8(&bytes).unwrap();
            let fv = constant_string(s).unwrap();
            ids.insert(get_static_value_id(fv.to_value()).unwrap());
        }
        assert_eq!(
            ids.len(),
            129,
            "all 129 short strings should have unique IDs"
        );
    }

    #[test]
    fn test_determinism() {
        let a1 = constant_string("a").unwrap();
        let a2 = constant_string("a").unwrap();
        let id1 = get_static_value_id(a1.to_value());
        let id2 = get_static_value_id(a2.to_value());
        assert_eq!(id1, id2, "same value should produce same ID");
    }

    #[test]
    fn test_singleton_none_is_registered() {
        let none_val = Value::new_none();
        let id = get_static_value_id(none_val);
        assert!(id.is_some(), "None should be registered");
    }

    #[test]
    fn test_singleton_bools_are_registered() {
        let false_val = Value::new_bool(false);
        let true_val = Value::new_bool(true);
        let false_id = get_static_value_id(false_val);
        let true_id = get_static_value_id(true_val);
        assert!(false_id.is_some(), "false should be registered");
        assert!(true_id.is_some(), "true should be registered");
        assert_ne!(
            false_id, true_id,
            "true and false should have different IDs"
        );
    }

    #[test]
    fn test_singleton_empty_tuple_is_registered() {
        use crate::values::tuple::value::VALUE_EMPTY_TUPLE;

        let empty_tuple = VALUE_EMPTY_TUPLE.unpack().to_value();
        let id = get_static_value_id(empty_tuple);
        assert!(id.is_some(), "empty tuple should be registered");
    }

    #[test]
    fn test_singleton_empty_array_is_registered() {
        use crate::values::types::array::VALUE_EMPTY_ARRAY;

        let empty_array = VALUE_EMPTY_ARRAY.unpack().to_value();
        let id = get_static_value_id(empty_array);
        assert!(id.is_some(), "empty array should be registered");
    }

    #[test]
    fn test_type_compiled_any_is_registered() {
        use crate::values::typing::type_compiled::compiled::TypeCompiled;

        let type_any = TypeCompiled::any();
        let id = get_static_value_id(type_any.to_inner());
        assert!(id.is_some(), "TypeCompiled::any() should be registered");
    }
}
