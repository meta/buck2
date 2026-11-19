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

use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::values::layout::value_alloc_size::ValueAllocSize;

pub(crate) enum ChunkAllocationDirection {
    /// Next allocation in the chunk has higher address than the previous one.
    Up,
    /// Next allocation in the chunk has lower address than the previous one.
    Down,
}

/// An allocated range whose bytes may still be under construction.
/// The lifetime retains the allocator, without borrowing its mutable bytes.
#[derive(Clone, Copy)]
pub(crate) struct AllocatedChunk<'a> {
    ptr: *const u8,
    len: usize,
    owner: PhantomData<&'a ()>,
}

impl<'a> AllocatedChunk<'a> {
    /// # Safety
    /// The range must remain allocated for `'a`. This does not require its
    /// contents to be initialized or exclude concurrent writes to them.
    pub(crate) unsafe fn new(ptr: *const u8, len: usize) -> Self {
        Self {
            ptr,
            len,
            owner: PhantomData,
        }
    }

    pub(crate) fn as_ptr(self) -> *const u8 {
        self.ptr
    }

    pub(crate) fn len(self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(self) -> bool {
        self.len == 0
    }

    pub(crate) fn advance(&mut self, bytes: usize) {
        assert!(bytes <= self.len);
        // SAFETY: the remaining range lies within the original allocation.
        self.ptr = unsafe { self.ptr.add(bytes) };
        self.len -= bytes;
    }
}

/// Fast memory allocator for the heap.
pub(crate) trait ArenaAllocator {
    /// Number of bytes allocated by this allocator.
    ///
    /// That is:
    /// * space occupied by allocated values
    /// * padding
    /// * reserved but not yet allocated space
    /// * does not include metadata
    fn allocated_bytes(&self) -> usize;
    /// Number of bytes reserved but not yet allocated by this allocator.
    fn remaining_capacity(&self) -> usize;
    /// Estimate the size of allocated metadata.
    fn allocation_overhead(&self) -> usize;
    /// Allocate given number of words.
    fn alloc(&self, size: ValueAllocSize) -> NonNull<u8>;

    /// This allocator chunk allocation direction.
    const CHUNK_ALLOCATION_DIRECTION: ChunkAllocationDirection;

    type ChunkRevIterator<'a>: Iterator<Item = AllocatedChunk<'a>>
    where
        Self: 'a;

    /// Iterate allocated chunks in the reverse order.
    ///
    /// # Safety
    /// Exclude allocation until the iterator is exhausted or dropped. The
    /// yielded ranges remain allocated even after subsequent allocations.
    unsafe fn iter_allocated_chunks_rev(&self) -> Self::ChunkRevIterator<'_>;

    /// No more allocation, reclaim memory if possible.
    fn finish(&mut self);
}
