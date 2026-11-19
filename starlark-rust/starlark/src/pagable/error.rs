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

//! Error types for pagable serialization/deserialization.

use std::sync::Arc;

use thiserror::Error;

use crate::pagable::DeserTypeId;
use crate::pagable::heap_ref_id::HeapRefId;
use crate::values::layout::heap::sealed::HeapAllocationOrigin;

/// Errors that can occur during pagable serialization/deserialization.
#[derive(Debug, Error)]
pub enum PagableError {
    /// The type was not registered in the vtable registry.
    #[error("Type `{type_id}` was not registered for deserialization.")]
    TypeNotRegistered {
        /// The type identifier that was not found.
        type_id: DeserTypeId,
    },

    /// The total bytes declared for an arena bump do not match what was actually deserialized.
    #[error(
        "Corrupted data: {count} values consumed {actual_bytes} bytes but expected {expected_bytes}"
    )]
    InconsistentArenaSize {
        /// Number of values deserialized.
        count: usize,
        /// Bytes declared in the serialized header.
        expected_bytes: u32,
        /// Bytes actually consumed by deserialized values.
        actual_bytes: u32,
    },

    /// A value declared a zero `alloc_size`. Every arena value occupies at least
    /// its header, so this is never valid and indicates a corrupt page-out stream.
    #[error("Corrupted data: value {index} declared a zero alloc_size")]
    ZeroAllocSize {
        /// Index of the value in the heap.
        index: usize,
    },

    /// A serialized heap reference has no binding in the storage's Starlark scope.
    #[error("Heap {heap_id:?} is not bound in this storage")]
    HeapNotBound {
        /// The logical heap identity whose binding was not found.
        heap_id: HeapRefId,
    },

    /// An exact native heap allocation has no registered value for the serialized index.
    #[error("Native heap {heap_id:?} has no registered value at index {value_index}")]
    NativeHeapValueNotRegistered {
        /// The logical identity of the native heap.
        heap_id: HeapRefId,
        /// The serialized index that could not be resolved.
        value_index: u32,
    },

    /// Two live heap allocations in one storage have the same logical heap
    /// identity.
    #[error(
        "Heap {heap_id:?} is already bound to a different heap in this storage; heap name `{heap_name}`, bound allocation {bound_heap_ptr:#x} ({}), conflicting allocation {conflicting_heap_ptr:#x} ({})",
        .bound_origin.as_str(),
        .conflicting_origin.map_or("expired", HeapAllocationOrigin::as_str),
    )]
    ConflictingHeapBinding {
        /// The ambiguous logical heap identity.
        heap_id: HeapRefId,
        /// Display of the `FrozenHeapName` shared by both allocations.
        heap_name: String,
        /// Address of the allocation already bound in this scope.
        bound_heap_ptr: usize,
        /// How page-in obtained the already-bound allocation.
        bound_origin: HeapAllocationOrigin,
        /// Address of the allocation that could not be bound.
        conflicting_heap_ptr: usize,
        /// How page-in obtained the allocation that could not be bound, or
        /// `None` if it expired before it could be classified.
        conflicting_origin: Option<HeapAllocationOrigin>,
    },

    /// A lazily deserialized heap slot was claimed, but its construction or
    /// a construction dependency failed before the slot became ready.
    #[error(
        "Partial deserialization failed for heap {heap_id:?} value_index {value_index} type `{value_type}`; original error: {cause}"
    )]
    PartialDeserializationFailed {
        /// The logical identity of the heap containing the failed slot.
        heap_id: HeapRefId,
        /// The serialized index of the failed slot.
        value_index: u32,
        /// The Starlark value type registered for the slot.
        value_type: &'static str,
        /// The recorded cause from this slot or a failed construction dependency.
        cause: Arc<str>,
    },

    /// A frozen value being serialized points into a heap whose chunk index
    /// was never registered, so its `value_index` cannot be resolved.
    #[error(
        "Frozen value pointer {raw_ptr:#x} not found in any registered heap's chunk index; target type: `{target_type}`; chunk index: {chunk_index_diagnostic}; live heap scan: {live_heap_diagnostic}"
    )]
    FrozenValueNotRegistered {
        /// Payload address of the unresolved value.
        raw_ptr: usize,
        /// Starlark type of the unresolved value.
        target_type: &'static str,
        /// State of the serialization chunk index around `raw_ptr`.
        chunk_index_diagnostic: String,
        /// Result of scanning the live registered heap allocations directly.
        live_heap_diagnostic: String,
    },

    /// A `StarlarkPagable`-derived enum was deserialized with an unknown variant tag.
    #[error("Invalid `{enum_name}` variant tag: {tag}")]
    InvalidVariantTag {
        /// Name of the enum being deserialized.
        enum_name: &'static str,
        /// The tag byte read from the input.
        tag: u8,
    },

    /// The input claims to hold a value of an uninhabited type.
    #[error("Corrupted data: a value of the uninhabited type `{type_name}` was serialized")]
    UninhabitedType {
        /// Name of the uninhabited type.
        type_name: &'static str,
    },
}

impl From<PagableError> for crate::Error {
    fn from(e: PagableError) -> Self {
        crate::Error::new_kind(crate::ErrorKind::Other(anyhow::Error::new(e)))
    }
}
