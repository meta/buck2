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

use std::cell::RefCell;
use std::cmp;
use std::mem;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;

use crate::values::layout::aligned_size::AlignedSize;
use crate::values::layout::heap::allocator::alloc::chain::ChunkChain;
use crate::values::layout::heap::allocator::alloc::chunk::Chunk;
use crate::values::layout::heap::allocator::alloc::chunk_part::ChunkPart;
use crate::values::layout::heap::arena::MIN_ALLOC;

/// Minimum usable cached allocation.
const MIN_USABLE_ALLOC: AlignedSize = AlignedSize::new_bytes(
    (
        // * All chunks are used to store chains, so we need to include chain header.
        // * We need to be able to store at least one object after the header.
        ChunkChain::HEADER_SIZE.bytes() + MIN_ALLOC.bytes()
    ) as usize,
);

#[derive(Debug)]
#[repr(align(128))]
struct PaddedByteCounter(AtomicUsize);

/// Total bytes currently parked in all threads' [`PerThreadChunkCache`]s.
///
/// These chunk parts pin their backing chunks while being reachable from no
/// heap, so without this counter they are invisible to `Allocative` reporting.
/// Part lengths are a lower bound on the pinned bytes: a cached part is
/// stored only while its chunk is shared, but once sibling parts drop it
/// pins the chunk's full power-of-two allocation.
///
/// The sum is exact when caches are quiescent. During mutation, separately
/// loaded counters can reflect different instants and give an approximate total.
static CACHED_CHUNK_PART_BYTES: CachedBytesRegistry = CachedBytesRegistry {
    counters: Mutex::new(Vec::new()),
};

#[derive(Default)]
struct CachedBytesRegistry {
    counters: Mutex<Vec<Weak<PaddedByteCounter>>>,
}

impl CachedBytesRegistry {
    fn new_counter(&self) -> CachedBytesCounter {
        let counter = Arc::new(PaddedByteCounter(AtomicUsize::new(0)));
        let mut counters = self
            .counters
            .lock()
            .expect("cached-byte registry executes no panicking user code");
        counters.retain(|counter| counter.strong_count() != 0);
        counters.push(Arc::downgrade(&counter));
        CachedBytesCounter(counter)
    }

    fn bytes(&self) -> usize {
        let mut bytes = 0;
        self.counters
            .lock()
            .expect("cached-byte registry executes no panicking user code")
            .retain(|counter| {
                let Some(counter) = counter.upgrade() else {
                    return false;
                };
                bytes += counter.0.load(Ordering::Relaxed);
                true
            });
        bytes
    }
}

// Only the owning cache writes; registry readers may temporarily retain the Arc.
#[derive(Debug)]
struct CachedBytesCounter(Arc<PaddedByteCounter>);

impl Default for CachedBytesCounter {
    fn default() -> Self {
        ensure_stats_registered();
        CACHED_CHUNK_PART_BYTES.new_counter()
    }
}

impl CachedBytesCounter {
    #[inline]
    fn set(&mut self, bytes: usize) {
        self.0.0.store(bytes, Ordering::Relaxed);
    }
}

impl Drop for CachedBytesCounter {
    fn drop(&mut self) {
        self.set(0);
    }
}

/// Reports [`CACHED_CHUNK_PART_BYTES`] to allocative memory profiles.
#[derive(Debug)]
struct PerThreadChunkCacheStats;

static PER_THREAD_CHUNK_CACHE_STATS: PerThreadChunkCacheStats = PerThreadChunkCacheStats;

/// Register the stats as an allocative global root.
///
/// Registered when a thread initializes its cache rather than via
/// `#[allocative::root]`, because the latter's `ctor` expansion requires a
/// direct `ctor` dependency this crate doesn't otherwise need.
fn ensure_stats_registered() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| allocative::register_root(&PER_THREAD_CHUNK_CACHE_STATS));
}

impl Allocative for PerThreadChunkCacheStats {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        // A unique node: its children are exempt from the inline-size check,
        // so reporting more bytes than the ZST self does not emit an
        // `Incorrect size declaration` warning.
        let mut bytes = visitor.enter_unique(allocative::Key::new("cached_chunk_parts"), 0);
        bytes.visit_simple(
            allocative::Key::new("bytes"),
            CACHED_CHUNK_PART_BYTES.bytes(),
        );
        bytes.exit();
        visitor.exit();
    }
}

/// Budget for reusable part lengths, not the full allocations they keep alive.
/// Shared chunks can retain more backing memory than their cached parts cover.
const MAX_CACHED_BYTES: usize = 16 * 1024 * 1024;

/// Slot bound so a flood of near-`MIN_USABLE_ALLOC` parts cannot make the
/// cache scans O(thousands) before the byte budget kicks in.
const MAX_CACHED_CHUNKS: usize = 32;

#[derive(Debug, Default)]
struct PerThreadChunkCache {
    cached_bytes_counter: CachedBytesCounter,
    /// The first `len` parts are sorted by ascending length; the rest are empty.
    last_chunks: [ChunkPart; MAX_CACHED_CHUNKS],
    len: usize,
    /// Sum of `last_chunks` lengths.
    cached_bytes: usize,
}

impl PerThreadChunkCache {
    fn remove(&mut self, index: usize) -> ChunkPart {
        let result = mem::take(&mut self.last_chunks[index]);
        self.last_chunks[index..self.len].rotate_left(1);
        self.len -= 1;
        self.cached_bytes -= result.len().bytes() as usize;
        self.cached_bytes_counter.set(self.cached_bytes);
        result
    }

    /// Save a part for reuse. Smaller cached parts make way for it; if they
    /// cannot make enough room, the part is dropped and the cache is left as
    /// it is.
    fn store(&mut self, chunk: ChunkPart) {
        let len = chunk.len().bytes() as usize;
        let over_budget = |cached_bytes: usize| cached_bytes + len > MAX_CACHED_BYTES;
        if over_budget(self.cached_bytes) || self.len == MAX_CACHED_CHUNKS {
            let cached = &self.last_chunks[..self.len];
            let smaller = cached.partition_point(|c| c.len() < chunk.len());
            let smaller_bytes: usize = cached[..smaller]
                .iter()
                .map(|c| c.len().bytes() as usize)
                .sum();
            if smaller == 0 || over_budget(self.cached_bytes - smaller_bytes) {
                // Dropping a part frees its allocation only when the last reference drops.
                // TODO(drollman): Reclaim rejected shared parts when their siblings are released.
                return;
            }
            while over_budget(self.cached_bytes) || self.len == MAX_CACHED_CHUNKS {
                drop(self.remove(0));
            }
        }
        let idx = self.last_chunks[..self.len].partition_point(|c| c.len() <= chunk.len());
        self.last_chunks[self.len] = chunk;
        self.last_chunks[idx..=self.len].rotate_right(1);
        self.len += 1;
        self.cached_bytes += len;
        self.cached_bytes_counter.set(self.cached_bytes);
    }

    /// Fetch a chunk from the thread-local cache if the cache has a chunk large enough.
    fn fetch(&mut self, len: AlignedSize) -> Option<ChunkPart> {
        // Best fit conserves cached capacity. A hit can be smaller than the
        // preferred fresh chunk, trading extra chain links for reuse of tails.
        let idx = self.last_chunks[..self.len]
            .iter()
            .position(|c| c.len() >= len)?;
        Some(self.remove(idx))
    }
}

thread_local! {
    static PER_THREAD_ALLOCATOR: RefCell<PerThreadChunkCache> = RefCell::new(PerThreadChunkCache::default());
}

/// A heap's chunks double from 512 bytes up to this size, then stay this size.
const MAX_CHUNK_BYTES: usize = 1024 * 1024;

/// How many times a heap's chunks double.
pub(crate) const MAX_CHUNK_DOUBLINGS: usize = (MAX_CHUNK_BYTES / 512).ilog2() as usize;

fn next_chunk_size(chunk_count_in_bump: usize) -> AlignedSize {
    AlignedSize::new_bytes(512 << chunk_count_in_bump.min(MAX_CHUNK_DOUBLINGS))
}

/// A request for more than a full-size chunk holds gets a chunk of exactly its
/// size. That chunk is never split, so it is freed with its heap; a split chunk
/// stays allocated for as long as the cache holds a part of it.
const MAX_SHARED_CHUNK_LEN: AlignedSize =
    AlignedSize::new_bytes(MAX_CHUNK_BYTES - Chunk::HEADER_SIZE.bytes() as usize);

/// Allocate chunk which is large enough for given number of words.
pub(crate) fn thread_local_alloc_at_least(
    len: AlignedSize,
    chunk_count_in_bump: usize,
) -> ChunkPart {
    let chunk = match PER_THREAD_ALLOCATOR.with_borrow_mut(|allocator| allocator.fetch(len)) {
        Some(chunk) => chunk,
        _ if len > MAX_SHARED_CHUNK_LEN => ChunkPart::alloc_exact(len),
        _ => {
            let next_chunk_size = next_chunk_size(chunk_count_in_bump) - Chunk::HEADER_SIZE;
            let len = cmp::max(len, next_chunk_size);
            ChunkPart::alloc_at_least(len)
        }
    };
    debug_assert!(chunk.len() >= len);
    chunk
}

/// Release chunk part to thread-local pool.
#[allow(clippy::if_same_then_else)]
#[inline]
pub(crate) fn thread_local_release(chunk: ChunkPart) {
    if chunk.is_full() {
        // Chunk part is the full chunk. Better return it to malloc.
        drop(chunk)
    } else if chunk.len() < MIN_USABLE_ALLOC {
        // It is not reusable.
        drop(chunk)
    } else if chunk.chunk_ref_count() == 1 {
        // We could reuse the chunk, but since it is not shared,
        // better return it to malloc.
        drop(chunk);
    } else {
        PER_THREAD_ALLOCATOR.with_borrow_mut(|allocator| allocator.store(chunk));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::Ordering;

    use crate::values::layout::aligned_size::AlignedSize;
    use crate::values::layout::heap::allocator::alloc::allocator::ChunkAllocator;
    use crate::values::layout::heap::allocator::alloc::chunk::Chunk;
    use crate::values::layout::heap::allocator::alloc::chunk_part::ChunkPart;
    use crate::values::layout::heap::allocator::alloc::per_thread::CachedBytesRegistry;
    use crate::values::layout::heap::allocator::alloc::per_thread::MAX_CACHED_BYTES;
    use crate::values::layout::heap::allocator::alloc::per_thread::MAX_CACHED_CHUNKS;
    use crate::values::layout::heap::allocator::alloc::per_thread::PER_THREAD_ALLOCATOR;
    use crate::values::layout::heap::allocator::alloc::per_thread::PerThreadChunkCache;
    use crate::values::layout::heap::allocator::alloc::per_thread::next_chunk_size;
    use crate::values::layout::heap::allocator::alloc::per_thread::thread_local_alloc_at_least;
    use crate::values::layout::heap::allocator::api::ArenaAllocator;
    use crate::values::layout::heap::repr::AValueHeader;
    use crate::values::layout::value_alloc_size::ValueAllocSize;
    use crate::wasm::is_wasm;

    #[test]
    fn test_next_chunk_size_doubles_then_caps() {
        for depth in 0..=11 {
            assert_eq!(AlignedSize::new_bytes(512 << depth), next_chunk_size(depth));
        }
        for depth in [12, 22, 23, 31, 32, 64, usize::MAX] {
            assert_eq!(AlignedSize::new_bytes(1024 * 1024), next_chunk_size(depth));
        }
    }

    #[test]
    fn test_cached_byte_registry_prunes_after_owner_drop() {
        let registry = CachedBytesRegistry::default();
        let mut counter = registry.new_counter();
        counter.set(64);
        let reader = registry.counters.lock().unwrap()[0].upgrade().unwrap();
        drop(counter);
        assert_eq!(0, reader.0.load(Ordering::Relaxed));
        assert_eq!(0, registry.bytes());
        drop(reader);
        assert_eq!(0, registry.bytes());
        assert!(registry.counters.lock().unwrap().is_empty());

        drop(registry.new_counter());
        let counter = registry.new_counter();
        assert_eq!(1, registry.counters.lock().unwrap().len());
        drop(counter);
        assert_eq!(0, registry.bytes());
        assert!(registry.counters.lock().unwrap().is_empty());
    }

    #[test]
    fn test_store_gives_up_smaller_parts_only_to_admit() {
        let registry = CachedBytesRegistry::default();
        const UNIT: usize = MAX_CACHED_BYTES / 16;
        let mut cache = PerThreadChunkCache {
            cached_bytes_counter: registry.new_counter(),
            last_chunks: Default::default(),
            len: 0,
            cached_bytes: 0,
        };
        let part = |units| {
            let len = AlignedSize::new_bytes(units * UNIT);
            ChunkPart::alloc_at_least(len).split_at_offset(len).0
        };

        cache.store(part(2));
        cache.store(part(12));
        // Giving up the smaller part would not make room for this one.
        cache.store(part(6));
        assert_eq!(14 * UNIT, registry.bytes());
        assert_eq!(cache.cached_bytes, registry.bytes());
        drop(cache.fetch(AlignedSize::new_bytes(12 * UNIT)).unwrap());
        drop(cache.fetch(AlignedSize::new_bytes(2 * UNIT)).unwrap());
        assert_eq!(0, registry.bytes());

        // Two smaller parts make way for the last one.
        for units in [4, 5, 6, 7] {
            cache.store(part(units));
        }
        assert_eq!(13 * UNIT, registry.bytes());
        assert_eq!(cache.cached_bytes, registry.bytes());
        drop(cache);
        assert_eq!(0, registry.bytes());
    }

    #[test]
    fn test_store_evicts_smallest_beyond_byte_budget() {
        let mut cache = PerThreadChunkCache::default();
        // The parts are a chunk header short of 4 MiB and 8 MiB.
        let small = AlignedSize::new_bytes(3 * 1024 * 1024);
        let large = AlignedSize::new_bytes(5 * 1024 * 1024);
        for _ in 0..3 {
            cache.store(ChunkPart::alloc_at_least(small));
        }
        cache.store(ChunkPart::alloc_at_least(large));
        assert!(cache.cached_bytes <= MAX_CACHED_BYTES);
        assert!(cache.fetch(large).is_some());
        assert!(cache.fetch(small).is_some());
        assert!(cache.fetch(small).is_some());
        assert!(
            cache.fetch(small).is_none(),
            "one small part should have made way for the large one"
        );

        // A part larger than the whole budget is never cached.
        cache.store(ChunkPart::alloc_at_least(AlignedSize::new_bytes(
            17 * 1024 * 1024,
        )));
        assert_eq!(0, cache.len);
    }

    #[test]
    fn test_fetch_takes_the_smallest_part_that_fits() {
        let mut cache = PerThreadChunkCache::default();
        for bytes in [4096, 1024, 16384] {
            let len = AlignedSize::new_bytes(bytes);
            cache.store(ChunkPart::alloc_at_least(len).split_at_offset(len).0);
        }
        let mut fetch = |bytes| {
            cache
                .fetch(AlignedSize::new_bytes(bytes))
                .map(|part| part.len().bytes())
        };
        assert_eq!(Some(4096), fetch(2000));
        assert_eq!(Some(1024), fetch(64));
        assert_eq!(None, fetch(32768));
        assert_eq!(Some(16384), fetch(64));
    }

    #[test]
    fn test_store_at_slot_limit_rejects_smaller_parts() {
        let mut cache = PerThreadChunkCache::default();
        let len = AlignedSize::new_bytes(1024);
        for _ in 0..MAX_CACHED_CHUNKS {
            cache.store(ChunkPart::alloc_at_least(len));
        }
        let bytes = cache.cached_bytes;
        cache.store(ChunkPart::alloc_at_least(AlignedSize::new_bytes(512)));
        assert_eq!(bytes, cache.cached_bytes);
        cache.store(ChunkPart::alloc_at_least(AlignedSize::new_bytes(2048)));
        assert_eq!(MAX_CACHED_CHUNKS, cache.len);
        assert!(cache.fetch(AlignedSize::new_bytes(2048)).is_some());
        for _ in 1..MAX_CACHED_CHUNKS {
            assert!(cache.fetch(len).is_some());
        }
        assert_eq!(0, cache.cached_bytes);
    }

    #[test]
    fn test_request_above_a_full_chunk_gets_exactly_its_size() {
        let full = AlignedSize::new_bytes(1024 * 1024) - Chunk::HEADER_SIZE;
        assert_eq!(full, thread_local_alloc_at_least(full, 0).len());
        let above = full + AlignedSize::new_bytes(AValueHeader::ALIGN);
        assert_eq!(above, thread_local_alloc_at_least(above, 0).len());
    }

    #[test]
    fn test_heap_with_one_oversized_value_caches_nothing() {
        if is_wasm() {
            // Threads don't work in wasm.
            return;
        }
        std::thread::spawn(|| {
            let mut allocator = ChunkAllocator::default();
            allocator.alloc(ValueAllocSize::new(AlignedSize::new_bytes(3 * 1024 * 1024)));
            allocator.finish();
            drop(allocator);
            PER_THREAD_ALLOCATOR.with_borrow(|cache| assert_eq!(0, cache.len));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_tiny_cached_parts_fall_back_to_preferred_chunk() {
        if is_wasm() {
            // Threads don't work in wasm.
            return;
        }
        std::thread::spawn(|| {
            let small = AlignedSize::new_bytes(64);
            PER_THREAD_ALLOCATOR.with_borrow_mut(|cache| {
                for _ in 0..MAX_CACHED_CHUNKS {
                    cache.store(ChunkPart::alloc_at_least(small).split_at_offset(small).0);
                }
            });
            for _ in 0..MAX_CACHED_CHUNKS {
                assert_eq!(small, thread_local_alloc_at_least(small, 20).len());
            }
            let fresh = thread_local_alloc_at_least(small, 20);
            assert_eq!(
                AlignedSize::new_bytes(1024 * 1024) - Chunk::HEADER_SIZE,
                fresh.len()
            );
            PER_THREAD_ALLOCATOR.with_borrow_mut(|cache| {
                assert_eq!(0, cache.len);
                assert_eq!(0, cache.cached_bytes);
            });
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_capped_chunk_includes_allocation_header() {
        if is_wasm() {
            // Threads don't work in wasm.
            return;
        }
        std::thread::spawn(|| {
            let chunk = thread_local_alloc_at_least(AlignedSize::new_bytes(64), usize::MAX);
            assert_eq!(1024 * 1024, chunk.allocated_bytes_with_metadata());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_cached_byte_counter_balances_threads() {
        if is_wasm() {
            // Threads don't work in wasm.
            return;
        }
        let registry = CachedBytesRegistry::default();
        let barrier = Barrier::new(3);
        let parked = std::thread::scope(|scope| {
            for bytes in [64, 256] {
                let barrier = &barrier;
                let registry = &registry;
                scope.spawn(move || {
                    let mut cache = PerThreadChunkCache {
                        cached_bytes_counter: registry.new_counter(),
                        last_chunks: Default::default(),
                        len: 0,
                        cached_bytes: 0,
                    };
                    let len = AlignedSize::new_bytes(bytes);
                    cache.store(ChunkPart::alloc_at_least(len).split_at_offset(len).0);
                    barrier.wait();
                    barrier.wait();
                });
            }
            barrier.wait();
            let parked = registry.bytes();
            barrier.wait();
            parked
        });
        assert_eq!(64 + 256, parked);
        assert_eq!(0, registry.bytes());
    }

    #[test]
    fn test_cached_byte_counter_balances_caches() {
        let registry = CachedBytesRegistry::default();
        let mut first = PerThreadChunkCache {
            cached_bytes_counter: registry.new_counter(),
            last_chunks: Default::default(),
            len: 0,
            cached_bytes: 0,
        };
        let mut second = PerThreadChunkCache {
            cached_bytes_counter: registry.new_counter(),
            last_chunks: Default::default(),
            len: 0,
            cached_bytes: 0,
        };

        let small = ChunkPart::alloc_at_least(AlignedSize::new_bytes(64));
        let small_len = small.len().bytes() as usize;
        let large = ChunkPart::alloc_at_least(AlignedSize::new_bytes(256));
        let large_len = large.len().bytes() as usize;
        first.store(small);
        second.store(large);
        assert_eq!(small_len + large_len, registry.bytes());

        let fetched = first.fetch(AlignedSize::new_bytes(64)).unwrap();
        assert_eq!(small_len, fetched.len().bytes() as usize);
        assert_eq!(large_len, registry.bytes());
        drop(first);
        assert_eq!(large_len, registry.bytes());
        drop(second);
        assert_eq!(0, registry.bytes());
    }

    #[test]
    fn test_release_partial() {
        let mut allocator = PerThreadChunkCache::default();
        let chunk = ChunkPart::alloc_at_least(AlignedSize::new_bytes(10 * AValueHeader::ALIGN));
        let (a, b) = chunk.split_at_offset(AlignedSize::new_bytes(5 * AValueHeader::ALIGN));
        let old_a_ptr = a.begin().as_ptr();
        let old_b_ptr = b.begin().as_ptr();
        allocator.store(a);
        allocator.store(b);
        let a = allocator
            .fetch(AlignedSize::new_bytes(3 * AValueHeader::ALIGN))
            .unwrap();
        let b = allocator
            .fetch(AlignedSize::new_bytes(3 * AValueHeader::ALIGN))
            .unwrap();
        assert!(
            std::ptr::eq(old_a_ptr, a.begin().as_ptr())
                || std::ptr::eq(old_a_ptr, b.begin().as_ptr())
        );
        assert!(
            std::ptr::eq(old_b_ptr, a.begin().as_ptr())
                || std::ptr::eq(old_b_ptr, b.begin().as_ptr())
        );
    }
}
