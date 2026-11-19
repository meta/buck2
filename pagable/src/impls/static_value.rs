/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Pagable implementation for static values.
//!
//! This module provides [`StaticValue`], a wrapper for `&'static T` that supports
//! pagable serialization. Use the [`static_value!`](macro@crate::static_value) macro to create
//! and register instances.
//!
//! For convenience, type aliases are provided:
//! - [`StaticStr`] = `StaticValue<str>`
//! - [`StaticBytes`] = `StaticValue<[u8]>`
//!
//! Serialization uses an index into the registry instead of the actual value,
//! making it more compact. The index is cached in each registered entry for O(1) access.

use std::fmt;
use std::ops::Deref;

use allocative::Allocative;
use allocative::Visitor;
use dupe::Dupe;
use serde::Deserialize;
use serde::Serialize;

use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;

/// A registered static value entry. Created by `static_value!` macro.
/// The index is populated when the registry is initialized.
pub struct StaticValueEntry<T: 'static + ?Sized> {
    pub value: &'static T,
    /// Index is set once during registry initialization.
    /// Using Cell is safe because initialization happens once at startup
    /// before any concurrent access.
    /// Public for macro access.
    pub index: std::cell::Cell<u32>,
}

// SAFETY: StaticValueEntry is Sync because:
// - The Cell<u32> is only written once during single-threaded static initialization
// - After initialization, it's only read
unsafe impl<T: Sync + ?Sized> Sync for StaticValueEntry<T> {}

impl<T: PartialEq + ?Sized> PartialEq for StaticValueEntry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq + ?Sized> Eq for StaticValueEntry<T> {}

impl<T: std::hash::Hash + ?Sized> std::hash::Hash for StaticValueEntry<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

/// A registered static value for pagable serialization/deserialization.
///
/// During serialization, an index is used instead of the value.
/// During deserialization, the index is looked up to recover the static reference.
pub struct StaticValue<T: 'static + ?Sized>(pub &'static StaticValueEntry<T>);

impl<T: ?Sized> Clone for StaticValue<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for StaticValue<T> {}

impl<T: ?Sized> Dupe for StaticValue<T> {}

impl<T: PartialEq + ?Sized> PartialEq for StaticValue<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq + ?Sized> Eq for StaticValue<T> {}

impl<T: std::hash::Hash + ?Sized> std::hash::Hash for StaticValue<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: ?Sized> StaticValue<T> {
    pub const fn value(self) -> &'static T {
        self.0.value
    }
}

impl<T: ?Sized> Deref for StaticValue<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.value
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for StaticValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0.value, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for StaticValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.0.value, f)
    }
}

impl<T: ?Sized> Allocative for StaticValue<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        // StaticValue is just a reference to static data, no heap allocation
        let _ = visitor;
    }
}

/// Registry mapping indices to static values, built once at startup.
pub struct StaticValueRegistry<T: 'static + ?Sized> {
    /// Map from index to entry (for deserialization)
    entries: Vec<&'static StaticValueEntry<T>>,
}

impl<T: Ord + ?Sized> StaticValueRegistry<T> {
    pub fn from_entries<'a>(iter: impl Iterator<Item = &'a &'static StaticValueEntry<T>>) -> Self {
        let mut entries: Vec<&'static StaticValueEntry<T>> = iter.copied().collect();
        // Sort for deterministic ordering across runs
        entries.sort_by_key(|e| e.value);
        // Assign indices to each entry
        for (i, entry) in entries.iter().enumerate() {
            entry.index.set(i as u32);
        }
        StaticValueRegistry { entries }
    }
}

/// Trait for types that have a static value registry.
/// Implemented by types that use `static_value!` or `impl_static_value_registry!` macros.
pub trait HasStaticValueRegistry: Ord + 'static {
    fn registry() -> &'static StaticValueRegistry<Self>;

    /// Ensure the registry is initialized. Called automatically on first serialize/deserialize,
    /// but can be called explicitly if needed (e.g. in tests).
    fn ensure_registry_initialized() {
        let _ = Self::registry();
    }
}

impl<T: HasStaticValueRegistry + ?Sized> PagableSerialize for StaticValue<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        T::ensure_registry_initialized();
        let index = self.0.index.get();
        if index == u32::MAX {
            panic!("Unregistered static value. Register it with: pagable::static_value!(...)",);
        }
        Ok(index.serialize(serializer.serde())?)
    }
}

impl<'de, T: HasStaticValueRegistry + ?Sized> PagableDeserialize<'de> for StaticValue<T> {
    fn pagable_deserialize<De: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut De,
    ) -> crate::Result<Self> {
        T::ensure_registry_initialized();
        let index = u32::deserialize(deserializer.serde())?;
        let registry = T::registry();

        match registry.entries.get(index as usize) {
            Some(&entry) => Ok(StaticValue(entry)),
            None => panic!(
                "Invalid static value index: {}. Registry has {} entries.",
                index,
                registry.entries.len()
            ),
        }
    }
}

// --- StaticStr ---

pub type StaticStr = StaticValue<str>;
crate::declare_static_value_type!(str, StaticStrEntry);

impl StaticStr {
    pub const fn as_str(self) -> &'static str {
        self.0.value
    }
}

/// Create and register a `StaticStr` constant.
///
/// ```ignore
/// pagable::static_str!(MY_STR = "hello");
/// ```
#[macro_export]
macro_rules! static_str {
    ($vis:vis $name:ident = $val:expr) => {
        $crate::static_value!($vis $name: str = $val, $crate::static_value::StaticStrEntry);
    };
}

// --- StaticBytes ---

pub type StaticBytes = StaticValue<[u8]>;
crate::declare_static_value_type!([u8], StaticBytesEntry);

impl StaticBytes {
    pub const fn as_bytes(self) -> &'static [u8] {
        self.0.value
    }
}

/// Create and register a `StaticBytes` constant.
///
/// ```ignore
/// pagable::static_bytes!(MY_DATA = b"hello");
/// ```
#[macro_export]
macro_rules! static_bytes {
    ($vis:vis $name:ident = $val:expr) => {
        $crate::static_value!($vis $name: [u8] = $val, $crate::static_value::StaticBytesEntry);
    };
}

/// Create and register a static value constant.
///
/// # Example
///
/// ```ignore
/// pagable::static_value!(MY_VAL: MyType = &MY_STATIC, MyTypeWrapper);
/// ```
#[macro_export]
macro_rules! static_value {
    ($vis:vis $name:ident : $ty:ty = $val:expr, $wrapper:path) => {
        $vis static $name: $crate::StaticValue<$ty> = {
            static ENTRY: $crate::static_value::StaticValueEntry<$ty> = $crate::static_value::StaticValueEntry {
                value: $val,
                index: $crate::__internal::Cell::new(u32::MAX),
            };
            $crate::__internal::inventory::submit! { $wrapper(&ENTRY) }
            $crate::StaticValue(&ENTRY)
        };
    };
}

/// Declare a static value type for pagable serialization.
///
/// Generates a wrapper struct for `inventory::collect!` and implements
/// `HasStaticValueRegistry` for the type.
///
/// # Example
///
/// ```ignore
/// pagable::declare_static_value_type!(MyType, MyTypeStaticEntry);
/// pagable::static_value!(MY_VAL: MyType = &MY_INSTANCE, MyTypeStaticEntry);
/// ```
#[macro_export]
macro_rules! declare_static_value_type {
    ($ty:ty, $wrapper:ident) => {
        #[doc(hidden)]
        pub struct $wrapper(pub &'static $crate::static_value::StaticValueEntry<$ty>);

        $crate::__internal::inventory::collect!($wrapper);

        impl $crate::static_value::HasStaticValueRegistry for $ty {
            fn registry() -> &'static $crate::static_value::StaticValueRegistry<$ty> {
                static REGISTRY: $crate::__internal::once_cell::sync::Lazy<
                    $crate::static_value::StaticValueRegistry<$ty>,
                > = $crate::__internal::once_cell::sync::Lazy::new(|| {
                    $crate::static_value::StaticValueRegistry::from_entries(
                        $crate::__internal::inventory::iter::<$wrapper>
                            .into_iter()
                            .map(|w| &w.0),
                    )
                });
                &REGISTRY
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestingDeserializer;
    use crate::testing::TestingSerializer;
    use crate::traits::PagableDeserialize;
    use crate::traits::PagableSerialize;

    // --- StaticStr tests ---

    crate::static_str!(STRING_ALPHA = "alpha");
    crate::static_str!(STRING_BETA = "beta");
    crate::static_str!(STRING_GAMMA = "gamma");
    crate::static_str!(STRING_DELTA = "delta");

    #[test]
    fn test_str_indices_are_unique_and_sorted_alphabetically() {
        <str as HasStaticValueRegistry>::ensure_registry_initialized();
        // Strings sorted: alpha < beta < delta < gamma
        let strings = [STRING_ALPHA, STRING_BETA, STRING_DELTA, STRING_GAMMA];
        let indices: Vec<_> = strings.iter().map(|s| s.0.index.get()).collect();

        for (i, &idx) in indices.iter().enumerate() {
            assert_ne!(idx, u32::MAX);
            if i > 0 {
                assert!(
                    indices[i - 1] < idx,
                    "indices should be strictly increasing"
                );
            }
        }
    }

    #[test]
    fn test_str_serialize_multiple() -> crate::Result<()> {
        let input = [STRING_GAMMA, STRING_ALPHA, STRING_DELTA, STRING_BETA];

        let mut serializer = TestingSerializer::new();
        for s in &input {
            s.pagable_serialize(&mut serializer)?;
        }
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        for expected in &input {
            let restored = StaticStr::pagable_deserialize(&mut deserializer)?;
            assert_eq!(expected.as_str(), restored.as_str());
        }
        Ok(())
    }

    // --- StaticBytes tests ---

    crate::static_bytes!(BYTES_HELLO = b"hello");
    crate::static_bytes!(BYTES_WORLD = b"world");
    crate::static_bytes!(BYTES_EMPTY = b"");

    #[test]
    fn test_bytes_roundtrip() -> crate::Result<()> {
        let input = [BYTES_HELLO, BYTES_WORLD, BYTES_EMPTY];

        let mut serializer = TestingSerializer::new();
        for b in &input {
            b.pagable_serialize(&mut serializer)?;
        }
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        for expected in &input {
            let restored = StaticBytes::pagable_deserialize(&mut deserializer)?;
            assert_eq!(expected.as_bytes(), restored.as_bytes());
        }
        Ok(())
    }

    // --- Wrapper arm tests ---

    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    pub struct WrappedType(u32);

    crate::declare_static_value_type!(WrappedType, WrappedTypeEntry);

    static VAL: WrappedType = WrappedType(42);
    crate::static_value!(STATIC_VAL: WrappedType = &VAL, WrappedTypeEntry);

    #[test]
    fn test_wrapper_roundtrip() -> crate::Result<()> {
        let mut serializer = TestingSerializer::new();
        STATIC_VAL.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored = StaticValue::<WrappedType>::pagable_deserialize(&mut deserializer)?;
        assert_eq!(restored.value().0, 42);
        Ok(())
    }
}
