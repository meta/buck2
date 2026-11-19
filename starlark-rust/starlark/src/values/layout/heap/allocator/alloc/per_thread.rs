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
    fn add(&mut self, bytes: usize) {
        if bytes != 0 {
            self.set(self.0.0.load(Ordering::Relaxed) + bytes);
        }
    }

    #[inline]
    fn sub(&mut self, bytes: usize) {
        if bytes != 0 {
            self.set(self.0.0.load(Ordering::Relaxed) - bytes);
        }
    }

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

#[derive(Debug, Default)]
struct PerThreadChunkCache {
    cached_bytes_counter: CachedBytesCounter,
    /// Keep a few last chunks.
    ///
    /// Frozen heap has two arenas: drop and non-drop. So we should keep at least two chunks.
    last_chunks: [ChunkPart; 4],
}

impl PerThreadChunkCache {
    /// Save a chunk to the thread-local cache if it is large enough.
    fn store(&mut self, mut chunk: ChunkPart) {
        let incoming = chunk.len().bytes() as usize;
        for next in &mut self.last_chunks {
            // Keep the largest chunks in the pool.
            if chunk.len() > next.len() {
                mem::swap(next, &mut chunk);
            }
        }
        // After the swaps, `chunk` holds the smallest part, which drops here.
        let evicted = chunk.len().bytes() as usize;
        self.cached_bytes_counter.add(incoming - evicted);
    }

    /// Fetch a chunk from the thread-local cache if the cache has a chunk large enough.
    fn fetch(&mut self, len: AlignedSize) -> Option<ChunkPart> {
        for next in &mut self.last_chunks {
            // Pick any chunk which is large enough.
            if next.len() >= len {
                let result = mem::take(next);
                self.cached_bytes_counter.sub(result.len().bytes() as usize);
                return Some(result);
            }
        }
        None
    }
}

thread_local! {
    static PER_THREAD_ALLOCATOR: RefCell<PerThreadChunkCache> = RefCell::new(PerThreadChunkCache::default());
}

fn next_chunk_size(chunk_count_in_bump: usize) -> AlignedSize {
    // Replicate `bumpalo` behavior: 512 in the first chunk, double each next,
    // but not greater than 2G.
    // TODO(nga): we should stop doubling after 1M or so.
    const MAX_CHUNK_BYTES: usize = 1 << 31;
    // `512 << 22` is the cap itself; a heap with more chunks than that is a
    // real state, hit by a large target during page-in, and stays at the cap.
    let doubled = 512usize << chunk_count_in_bump.min(22);
    AlignedSize::new_bytes(doubled.min(MAX_CHUNK_BYTES))
}

/// Allocate chunk which is large enough for given number of words.
pub(crate) fn thread_local_alloc_at_least(
    len: AlignedSize,
    chunk_count_in_bump: usize,
) -> ChunkPart {
    let chunk = match PER_THREAD_ALLOCATOR.with_borrow_mut(|allocator| allocator.fetch(len)) {
        Some(chunk) => chunk,
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
    use crate::values::layout::heap::allocator::alloc::chunk_part::ChunkPart;
    use crate::values::layout::heap::allocator::alloc::per_thread::CachedBytesRegistry;
    use crate::values::layout::heap::allocator::alloc::per_thread::PerThreadChunkCache;
    use crate::values::layout::heap::allocator::alloc::per_thread::next_chunk_size;
    use crate::values::layout::heap::repr::AValueHeader;

    #[test]
    fn test_next_chunk_size_doubles_then_caps() {
        const CAP: usize = 1 << 31;
        // 512 = 2^9 doubled per chunk: 2^(9+n) up to the cap.
        assert_eq!(AlignedSize::new_bytes(512), next_chunk_size(0));
        assert_eq!(AlignedSize::new_bytes(1024), next_chunk_size(1));
        // 22 is the last exact doubling: 2^31 is the cap itself.
        assert_eq!(AlignedSize::new_bytes(CAP), next_chunk_size(22));
        // Every count past that clamps to the cap, including the ones that
        // used to shift the bit out (23 and up) or overflow the shift width
        // and panic (32 and up).
        assert_eq!(AlignedSize::new_bytes(CAP), next_chunk_size(23));
        assert_eq!(AlignedSize::new_bytes(CAP), next_chunk_size(31));
        assert_eq!(AlignedSize::new_bytes(CAP), next_chunk_size(32));
        assert_eq!(AlignedSize::new_bytes(CAP), next_chunk_size(64));
        assert_eq!(AlignedSize::new_bytes(CAP), next_chunk_size(usize::MAX));
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
    fn test_cached_byte_counter_balances_threads() {
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
        };
        let mut second = PerThreadChunkCache {
            cached_bytes_counter: registry.new_counter(),
            last_chunks: Default::default(),
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
