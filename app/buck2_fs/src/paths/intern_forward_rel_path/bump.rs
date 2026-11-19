/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sharded bump arena for the path interner.
//!
//! `SHARDS` independent bump allocators, each behind a `Mutex`. Callers pick a
//! shard from the hash they already computed for the table lookup, so two
//! threads working in different shards don't contend on the arena either.
//!
//! Allocations are leaked forever: there is no `free`, no `Drop` on the arena,
//! and no way to reclaim space. Lost insert races in the interner waste the
//! bytes they bumped — that's tolerable because the whole interner is leaky
//! by design and we expect lost races to be rare. Rolling back a bump pointer
//! across threads isn't safe (another shard user could have advanced past us
//! by the time we'd try) so we simply don't.
//!
//! Alignment is always 1: the arena hands out byte buffers for NUL-terminated
//! UTF-8 paths, nothing requiring stricter alignment.
//!
//! Each shard's chunks grow geometrically (`INITIAL_CHUNK` → `MAX_CHUNK`) so
//! the per-shard fixed overhead stays small even when many shards see traffic.
//! Allocations larger than `OVERSIZED_THRESHOLD` get a dedicated chunk so a
//! single big request can't strand most of a normal chunk.

use std::alloc;
use std::alloc::Layout;
use std::ptr;
use std::ptr::NonNull;
use std::sync::Mutex;

const SHARDS: usize = 64;

/// First chunk allocated for a shard. Picked so that, with paths averaging
/// tens of bytes, a fresh shard fits hundreds of entries before growing.
const INITIAL_CHUNK: usize = 16 * 1024;

/// Cap on chunk size. Past this, a busy shard just allocates more chunks of
/// this size rather than ever-larger ones.
const MAX_CHUNK: usize = 256 * 1024;

/// Allocations larger than this get a dedicated chunk that the shard's bump
/// cursor never sees. Threshold = `MAX_CHUNK / 2` so a single accepted
/// oversized request never strands more than half a chunk.
const OVERSIZED_THRESHOLD: usize = MAX_CHUNK / 2;

struct Shard {
    /// Pointer to the next byte we'll hand out. Null only in the initial
    /// state, before this shard has ever allocated a chunk.
    cursor: *mut u8,
    /// Bytes remaining in the current chunk. Zero in the initial state.
    remaining: usize,
    /// Size of the next normal chunk this shard will allocate. Doubles each
    /// time we allocate one (capped at `MAX_CHUNK`); oversized one-off
    /// chunks don't advance this counter.
    next_chunk_size: usize,
    /// Running total of bytes this shard has requested from the global
    /// allocator (sum of every chunk size, including the unused tails of
    /// past chunks and any oversized one-offs). Read by `BumpArena::bytes_allocated`.
    bytes_allocated: usize,
}

impl Shard {
    const EMPTY: Self = Shard {
        cursor: ptr::null_mut(),
        remaining: 0,
        next_chunk_size: INITIAL_CHUNK,
        bytes_allocated: 0,
    };

    /// `const`-evaluable initializer used to fill the array of shards. Needed
    /// because `Mutex<Shard>` isn't `Copy`, so `[Mutex::new(...); SHARDS]`
    /// requires the repeat operand to be a const expression.
    #[expect(
        clippy::declare_interior_mutable_const,
        reason = "intentional — used as the per-element initializer for an array of independent mutexes; each array slot gets its own fresh `Mutex` value"
    )]
    const NEW_MUTEX: Mutex<Self> = Mutex::new(Self::EMPTY);

    /// Reserve `size` bytes and return a pointer to them. The bytes live
    /// forever.
    fn bump(&mut self, size: usize) -> NonNull<u8> {
        if size > OVERSIZED_THRESHOLD {
            self.bytes_allocated += size;
            return alloc_oversized(size);
        }
        if size > self.remaining {
            self.fresh_chunk();
        }
        // SAFETY: after `fresh_chunk` (or the early `if`-skipped path),
        // `cursor` is non-null and points to at least `remaining >= size`
        // writable bytes inside a chunk this shard owns.
        let ptr = unsafe { NonNull::new_unchecked(self.cursor) };
        // SAFETY: `remaining >= size`, so `cursor + size` is in-bounds (or
        // one past the end) of the same chunk allocation.
        self.cursor = unsafe { self.cursor.add(size) };
        self.remaining -= size;
        ptr
    }

    #[cold]
    fn fresh_chunk(&mut self) {
        // Whatever space remained in the previous chunk is forfeit — without
        // a free list there's nowhere to track it, and the arena is
        // leak-only anyway.
        let chunk_size = self.next_chunk_size;
        self.next_chunk_size = (chunk_size * 2).min(MAX_CHUNK);
        let layout = Layout::from_size_align(chunk_size, 1).expect("chunk size fits in Layout");
        // SAFETY: `chunk_size` is non-zero.
        let ptr = unsafe { alloc::alloc(layout) };
        if ptr.is_null() {
            alloc::handle_alloc_error(layout);
        }
        self.cursor = ptr;
        self.remaining = chunk_size;
        self.bytes_allocated += chunk_size;
    }
}

#[cold]
fn alloc_oversized(size: usize) -> NonNull<u8> {
    let layout = Layout::from_size_align(size, 1).expect("oversized request fits in Layout");
    // SAFETY: `size > OVERSIZED_THRESHOLD > 0`.
    let ptr = unsafe { alloc::alloc(layout) };
    if ptr.is_null() {
        alloc::handle_alloc_error(layout);
    }
    // SAFETY: just checked non-null.
    unsafe { NonNull::new_unchecked(ptr) }
}

// SAFETY: `Shard`'s `*mut u8` points into a chunk allocated via the global
// allocator and never freed. No other shard ever reads or writes through
// that pointer, and access from the owning shard is serialized by the
// surrounding `Mutex`.
unsafe impl Send for Shard {}

pub(super) struct BumpArena {
    shards: [Mutex<Shard>; SHARDS],
}

impl BumpArena {
    pub(super) const fn new() -> Self {
        BumpArena {
            shards: [Shard::NEW_MUTEX; SHARDS],
        }
    }

    /// Reserve `size` bytes in the shard selected by `hash`. The returned
    /// pointer is writable for `size` bytes and the bytes live forever.
    pub(super) fn alloc(&self, hash: u64, size: usize) -> NonNull<u8> {
        debug_assert!(size > 0);
        let shard = &self.shards[(hash as usize) % SHARDS];
        let mut shard = shard.lock().expect("bump shard mutex poisoned");
        shard.bump(size)
    }

    /// Total bytes the arena has requested from the global allocator across
    /// all shards. Includes unused tails of past chunks and any oversized
    /// one-off allocations. Locks each shard briefly; intended for profiling,
    /// not hot paths.
    pub(super) fn bytes_allocated(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.lock().expect("bump shard mutex poisoned").bytes_allocated)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;

    #[test]
    fn small_allocations_share_a_chunk() {
        let arena = BumpArena::new();
        let a = arena.alloc(0, 16);
        let b = arena.alloc(0, 16);
        // Same shard, sequential allocations: `b` should sit immediately
        // after `a` in the same chunk.
        let expected_b = unsafe { a.as_ptr().add(16) };
        assert_eq!(b.as_ptr(), expected_b);
    }

    #[test]
    fn different_shards_are_independent() {
        let arena = BumpArena::new();
        // Pick two hashes that fall in different shards.
        let a = arena.alloc(0, 16);
        let b = arena.alloc(1, 16);
        // No relationship between the two pointers — they come from
        // independent chunks.
        assert_ne!(a.as_ptr(), b.as_ptr());
        // And in particular `b` is not the byte after `a`.
        assert_ne!(b.as_ptr(), unsafe { a.as_ptr().add(16) });
    }

    #[test]
    fn allocations_spanning_a_chunk_boundary() {
        let arena = BumpArena::new();
        // Fill most of the initial chunk, then ask for more than what's left
        // to force a fresh chunk. The new pointer should not equal where the
        // old cursor would have continued.
        let big = arena.alloc(7, INITIAL_CHUNK - 4);
        let small_after = arena.alloc(7, 8);
        let would_be_old_chunk = unsafe { big.as_ptr().add(INITIAL_CHUNK - 4) };
        assert_ne!(small_after.as_ptr(), would_be_old_chunk);
    }

    #[test]
    fn oversized_allocation_does_not_disturb_cursor() {
        let arena = BumpArena::new();
        let before = arena.alloc(11, 16);
        // Force an oversized one-off chunk.
        let _huge = arena.alloc(11, OVERSIZED_THRESHOLD + 1);
        let after = arena.alloc(11, 16);
        // The post-oversized small allocation should continue from where
        // `before` left off.
        assert_eq!(after.as_ptr(), unsafe { before.as_ptr().add(16) });
    }

    #[test]
    fn writing_through_returned_pointer_works() {
        let arena = BumpArena::new();
        let p = arena.alloc(42, 4);
        unsafe {
            ptr::write(p.as_ptr(), 1);
            ptr::write(p.as_ptr().add(1), 2);
            ptr::write(p.as_ptr().add(2), 3);
            ptr::write(p.as_ptr().add(3), 4);
            assert_eq!(*p.as_ptr(), 1);
            assert_eq!(*p.as_ptr().add(3), 4);
        }
    }
}
