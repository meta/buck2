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

//! Registry for static frozen values.
//!
//! Static values are values that exist at fixed memory addresses and are not
//! heap-allocated, such as static strings created by `const_frozen_string!`
//! and singletons like None, True, False, empty collections.
//!
//! These need special handling during serialization/deserialization.
//! We use the inventory crate to collect all registered static values at
//! compile time and build lookup tables at runtime.

use itertools::Itertools;

use crate::values::OwnedFrozenRef;
use crate::values::Value;

/// Unified registry entry for a static frozen value.
///
/// Each entry stores the source location where it was registered
/// (used to compute a deterministic hash-based ID at runtime)
/// and a function to obtain the value.
pub struct StaticValueEntry {
    /// Source file where this static value was registered (from `file!()`).
    /// May include a `::name` suffix to disambiguate multiple values
    /// registered from the same macro invocation (e.g. `static_starlark_value!`
    /// multi-value variant).
    pub file: &'static str,
    /// Source line where this static value was registered (from `line!()`).
    pub line: u32,
    /// Function to get the value.
    ///
    /// We use a function pointer instead of storing the value directly
    /// because `inventory::submit!` requires const expressions, and
    /// creating the value involves non-const method calls.
    pub get_value: fn() -> Value<'static>,
}

impl StaticValueEntry {
    /// Create a new registry entry.
    pub const fn new(file: &'static str, line: u32, get_value: fn() -> Value<'static>) -> Self {
        Self {
            file,
            line,
            get_value,
        }
    }
}

inventory::collect!(StaticValueEntry);

/// Registry entry for a static globals or methods heap.
///
/// The `globals_static!` and `methods_static!` macros register their backing
/// heaps here, so all values allocated in those heaps are treated as static
/// values during pagable serialization.
pub struct StaticHeapEntry {
    /// Source file where this static heap was registered (from `file!()`).
    pub file: &'static str,
    /// Source line where this static heap was registered (from `line!()`).
    pub line: u32,
    /// Function to get the static heap.
    ///
    /// We use a function pointer instead of storing the heap directly so the
    /// `inventory::submit!` payload can remain const-compatible.
    pub get_heap: fn() -> OwnedFrozenRef<'static, ()>,
}

inventory::collect!(StaticHeapEntry);

impl StaticHeapEntry {
    /// All registered entries, in deterministic (file, line) order — the
    /// order that assigns dense static ids, so serializer and deserializer
    /// in the same binary agree on them.
    pub(crate) fn iter_sorted() -> impl Iterator<Item = &'static StaticHeapEntry> {
        inventory::iter::<StaticHeapEntry>().sorted_by_key(|e| (e.file, e.line))
    }
}
