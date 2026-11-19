/*
 * Copyright 2019 The Starlark in Rust Authors.
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

//! VTable registry for Starlark value deserialization.
//!
//! This module provides a mechanism for registering and looking up vtables
//! by their deserialization type identifiers. During deserialization, we need
//! to know which vtable to use for a given type, and this registry provides
//! that mapping.

use std::collections::HashMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::LazyLock;
use std::sync::Mutex;

use crate::pagable::error::PagableError;
use crate::values::layout::vtable::AValueVTable;

/// Deserialization type identifier for vtable lookup.
///
/// This is a newtype wrapper around `&'static str` that holds the result of
/// `std::any::type_name`. It uniquely identifies a concrete Rust type for
/// deserialization purposes, unlike `StarlarkValue::TYPE` which can be shared
/// (e.g., "function" for EnumType and NativeFunction).
///
/// In the serialized form a `DeserTypeId` is encoded not as its (long) type-name
/// string but as a compact `u32` index into the vtable registry. The index is
/// assigned by sorting all registered type names, so it depends only on the set
/// of registered types and is therefore stable across runs of the same binary.
/// See `VtableRegistry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeserTypeId(pub &'static str);

impl pagable::PagableSerialize for DeserTypeId {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        let index = VTABLE_REGISTRY
            .type_to_id
            .get(self)
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!("Type `{}` was not registered for serialization", self.0)
            })?;
        index.pagable_serialize(serializer)
    }
}

impl<'de> pagable::PagableDeserialize<'de> for DeserTypeId {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let index = u32::pagable_deserialize(deserializer)?;
        VTABLE_REGISTRY
            .id_to_type
            .get(index as usize)
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Type index `{index}` is out of range ({} types registered for deserialization)",
                    VTABLE_REGISTRY.id_to_type.len()
                )
            })
    }
}

impl DeserTypeId {
    /// Create a `DeserTypeId` for a type.
    #[inline]
    pub const fn of<T: ?Sized>() -> Self {
        DeserTypeId(std::any::type_name::<T>())
    }

    /// Get the underlying type name string.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl Display for DeserTypeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.0, f)
    }
}

/// Registry entry for vtable lookup during deserialization.
/// Collected at compile time via the `inventory` crate.
pub struct VTableRegistryEntry {
    /// Deserialization type identifier (from `std::any::type_name`).
    /// Used as the key for vtable lookup during deserialization.
    pub deser_type_id: DeserTypeId,
    /// The vtable for this type.
    pub vtable: &'static AValueVTable,
}

inventory::collect!(VTableRegistryEntry);

/// Accumulator for vtable entries of generic matcher monomorphizations,
/// discovered at image-load time.
///
/// Concrete types register vtables with `inventory::submit!`, but a generic
/// `TypeCompiledImplAsStarlarkValue<W<X>>` has no single source site to
/// submit from. Instead, each monomorphization emits a program constructor
/// (see `TypeCompiled::alloc`) that pushes its entry here when the image
/// loads; [`VTABLE_REGISTRY`] drains the accumulator when it is first built,
/// strictly after constructors have run.
pub(crate) struct GenericVtableAccumulator {
    // Pushes happen in program constructors and the drain happens later
    // inside the `LazyLock` build, so contention is not expected; the mutex
    // makes the static safe to touch from safe code regardless.
    entries: Mutex<Vec<VTableRegistryEntry>>,
}

pub(crate) static GENERIC_VTABLE_ACCUMULATOR: GenericVtableAccumulator = GenericVtableAccumulator {
    entries: Mutex::new(Vec::new()),
};

impl GenericVtableAccumulator {
    pub(crate) fn push(&self, entry: VTableRegistryEntry) {
        self.entries
            .lock()
            .expect("generic vtable accumulator mutex should not be poisoned")
            .push(entry);
    }

    fn drain(&self) -> Vec<VTableRegistryEntry> {
        std::mem::take(
            &mut *self
                .entries
                .lock()
                .expect("generic vtable accumulator mutex should not be poisoned"),
        )
    }
}

/// Bidirectional registry of all `inventory`-collected vtable entries, built
/// once on first use.
///
/// Each type gets a dense index, assigned by sorting type names so it depends
/// only on the *set* of registered types (not inventory order) and is stable
/// across runs of the same binary. This is what lets the serialized form refer
/// to a type by index instead of by its full name.
struct VtableRegistry {
    /// Type id → vtable. Backs [`lookup_vtable`].
    by_type: HashMap<DeserTypeId, &'static AValueVTable>,
    /// Index → type id. Backs deserialization (`u32` index → `DeserTypeId`).
    id_to_type: Vec<DeserTypeId>,
    /// Type id → index. Backs serialization (`DeserTypeId` → `u32` index).
    type_to_id: HashMap<DeserTypeId, u32>,
}

/// Lookup tables for vtable resolution, built lazily from inventory.
static VTABLE_REGISTRY: LazyLock<VtableRegistry> = LazyLock::new(|| {
    // Deduplicate by type id first so a type submitted more than once still
    // gets a single index (matching the previous `HashMap`-based registry).
    // Generic monomorphizations are pushed by load-time constructors, one
    // per codegen unit that instantiated them, so duplicates are expected
    // there as well.
    let by_type: HashMap<DeserTypeId, &'static AValueVTable> =
        inventory::iter::<VTableRegistryEntry>()
            .map(|e| (e.deser_type_id, e.vtable))
            .chain(
                GENERIC_VTABLE_ACCUMULATOR
                    .drain()
                    .into_iter()
                    .map(|e| (e.deser_type_id, e.vtable)),
            )
            .collect();

    // Sort by type name so each type's index depends only on the set of
    // registered types, making it stable across runs of the same binary.
    let mut id_to_type: Vec<DeserTypeId> = by_type.keys().copied().collect();
    id_to_type.sort_unstable_by_key(|id| id.0);

    let type_to_id = id_to_type
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index as u32))
        .collect();

    VtableRegistry {
        by_type,
        id_to_type,
        type_to_id,
    }
});

/// Look up a vtable by its deserialization type id.
/// Returns an error if the type is not registered.
#[allow(dead_code)]
pub fn lookup_vtable(deser_type_id: DeserTypeId) -> crate::Result<&'static AValueVTable> {
    VTABLE_REGISTRY
        .by_type
        .get(&deser_type_id)
        .copied()
        .ok_or_else(|| {
            PagableError::TypeNotRegistered {
                type_id: deser_type_id,
            }
            .into()
        })
}

/// Get a list of all registered type IDs (for debugging/testing).
#[cfg(test)]
pub(crate) fn registered_type_ids() -> Vec<DeserTypeId> {
    VTABLE_REGISTRY.by_type.keys().copied().collect()
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use derive_more::Display;
    use starlark_derive::Freeze;
    use starlark_derive::NoSerialize;
    use starlark_derive::StarlarkPagable;
    use starlark_derive::Trace;
    use starlark_derive::starlark_value;

    use super::*;
    // Alias crate as starlark so proc macro generated paths work
    use crate as starlark;
    use crate::starlark_complex_value;
    use crate::starlark_simple_value;
    use crate::values::ProvidesStaticType;
    use crate::values::StarlarkValue;
    use crate::values::Value;

    /// A simple test type to verify vtable registration works for simple values.
    #[derive(
        Debug,
        Display,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("TestSimpleType")]
    struct TestSimpleType;

    starlark_simple_value!(TestSimpleType);

    #[starlark_value(type = "TestSimpleType")]
    impl<'v> StarlarkValue<'v> for TestSimpleType {}

    /// A complex type to verify vtable registration works for complex values.
    #[derive(
        Debug,
        Display,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        Clone,
        Trace,
        Freeze,
        StarlarkPagable
    )]
    #[display("TestComplex")]
    struct TestComplex<'v> {
        _value: Value<'v>,
    }

    starlark_complex_value!(TestComplex);

    #[starlark_value(type = "TestComplex")]
    impl<'v> StarlarkValue<'v> for TestComplex<'v> {}

    #[test]
    fn test_type_id_index_round_trips_through_registry() {
        // Every registered type maps to an index that maps back to the same type.
        for id in registered_type_ids() {
            let index = VTABLE_REGISTRY.type_to_id.get(&id).copied().unwrap();
            assert_eq!(VTABLE_REGISTRY.id_to_type[index as usize], id);
        }
    }

    #[test]
    fn test_type_id_indices_are_dense_and_unique() {
        // Indices form a contiguous 0..N range with no gaps or duplicates,
        // so they round-trip through the `id_to_type` Vec.
        let n = VTABLE_REGISTRY.id_to_type.len();
        assert_eq!(VTABLE_REGISTRY.type_to_id.len(), n);
        let mut indices: Vec<u32> = VTABLE_REGISTRY.type_to_id.values().copied().collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..n as u32).collect::<Vec<_>>());
    }

    #[test]
    fn test_type_id_indices_are_sorted_by_name() {
        // Indices are assigned in sorted type-name order; this is what makes
        // them stable across runs of the same binary.
        let names: Vec<&str> = VTABLE_REGISTRY.id_to_type.iter().map(|id| id.0).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "ids must be assigned in sorted type-name order"
        );
    }

    #[test]
    fn test_deser_type_id_serializes_as_compact_index() -> crate::Result<()> {
        use pagable::PagableDeserialize;
        use pagable::PagableSerialize;

        use crate::values::types::string::str_type::StarlarkStr;

        let id = DeserTypeId::of::<StarlarkStr>();

        let mut ser = pagable::testing::TestingSerializer::new();
        id.pagable_serialize(&mut ser)
            .map_err(crate::Error::new_other)?;
        let bytes = ser.finish();

        // The encoded form is a small varint index, not the full type-name
        // string — that is the whole point of the index encoding.
        assert!(
            bytes.len() < id.0.len(),
            "expected index encoding ({} bytes) to be smaller than type name `{}` ({} bytes)",
            bytes.len(),
            id.0,
            id.0.len(),
        );

        let mut de = pagable::testing::TestingDeserializer::new(&bytes);
        let restored =
            DeserTypeId::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
        assert_eq!(restored, id);
        Ok(())
    }

    #[test]
    fn test_simple_type_is_registered() {
        // Verify that our simple test type was registered
        let deser_type_id = DeserTypeId::of::<TestSimpleType>();
        let vtable = lookup_vtable(deser_type_id);
        assert!(
            vtable.is_ok(),
            "Expected TestSimpleType to be registered. Available types: {:?}",
            registered_type_ids()
        );
        let vt = vtable.unwrap();
        assert_eq!(vt.type_name, "TestSimpleType");
    }

    #[test]
    fn test_complex_type_frozen_is_registered() {
        // Verify that the frozen variant of complex type was registered
        let type_id = DeserTypeId::of::<TestComplex<'static>>();
        let vtable = lookup_vtable(type_id);
        assert!(
            vtable.is_ok(),
            "Expected TestComplex to be registered. Available types: {:?}",
            registered_type_ids()
        );
        let vt = vtable.unwrap();
        assert_eq!(vt.type_name, "TestComplex");
    }

    #[test]
    fn test_lookup_nonexistent_type() {
        // Looking up a non-existent type should return an error
        let result = lookup_vtable(DeserTypeId("this_type_does_not_exist_12345"));
        assert!(result.is_err());
        match result {
            Err(err) => match err.kind() {
                crate::ErrorKind::Other(e) => {
                    let pagable_err = e.downcast_ref::<PagableError>().unwrap();
                    assert!(
                        matches!(pagable_err, PagableError::TypeNotRegistered { .. }),
                        "Expected TypeNotRegistered error"
                    );
                }
                _ => panic!("Expected ErrorKind::Other"),
            },
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_starlark_str_is_registered() {
        use crate::values::types::string::str_type::StarlarkStr;
        let type_id = DeserTypeId::of::<StarlarkStr>();
        let vtable = lookup_vtable(type_id);
        assert!(
            vtable.is_ok(),
            "Expected StarlarkStr to be registered. Available types: {:?}",
            registered_type_ids()
        );
        let vt = vtable.unwrap();
        assert!(vt.is_str);
    }

    #[test]
    fn test_frozen_tuple_is_registered() {
        use crate::values::types::tuple::value::Tuple;
        let type_id = DeserTypeId::of::<Tuple<'static>>();
        let vtable = lookup_vtable(type_id);
        assert!(
            vtable.is_ok(),
            "Expected Tuple to be registered. Available types: {:?}",
            registered_type_ids()
        );
        let vt = vtable.unwrap();
        assert_eq!(vt.type_name, "tuple");
    }

    #[test]
    fn test_frozen_list_is_registered() {
        use crate::values::list::value::FrozenList;
        let type_id = DeserTypeId::of::<FrozenList>();
        let vtable = lookup_vtable(type_id);
        assert!(
            vtable.is_ok(),
            "Expected FrozenList to be registered. Available types: {:?}",
            registered_type_ids()
        );
        let vt = vtable.unwrap();
        assert_eq!(vt.type_name, "list");
    }

    #[test]
    fn test_type_compiled_non_generic_matcher_is_registered() {
        // IsAnyOf is a non-generic TypeMatcher, so TypeCompiledImplAsStarlarkValue<IsAnyOf>
        // should be registered by the #[type_matcher] macro.
        use crate::values::typing::type_compiled::compiled::TypeCompiledImplAsStarlarkValue;
        use crate::values::typing::type_compiled::matchers::IsAnyOf;
        let type_id = DeserTypeId::of::<TypeCompiledImplAsStarlarkValue<IsAnyOf>>();
        let vtable = lookup_vtable(type_id);
        assert!(
            vtable.is_ok(),
            "Expected TypeCompiledImplAsStarlarkValue<IsAnyOf> to be registered. Available types: {:?}",
            registered_type_ids()
        );
    }

    #[test]
    fn test_type_compiled_generic_matcher_is_registered() {
        // IsListOf is a generic TypeMatcher (IsListOf<I>). Each instantiation
        // in the binary registers its vtable through a load-time constructor,
        // so this lookup succeeding proves the constructor path ran.
        use crate::values::typing::type_compiled::compiled::TypeCompiledImplAsStarlarkValue;
        use crate::values::typing::type_compiled::matcher::TypeMatcherBox;
        use crate::values::typing::type_compiled::matchers::IsListOf;
        let type_id =
            DeserTypeId::of::<TypeCompiledImplAsStarlarkValue<IsListOf<TypeMatcherBox>>>();
        let vtable = lookup_vtable(type_id);
        assert!(
            vtable.is_ok(),
            "Expected TypeCompiledImplAsStarlarkValue<IsListOf<TypeMatcherBox>> to be registered. Available types: {:?}",
            registered_type_ids()
        );
    }
}
