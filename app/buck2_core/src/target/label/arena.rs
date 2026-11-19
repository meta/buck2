/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Bump arenas for immortal target label allocations.

use std::alloc;
use std::alloc::Layout;
use std::mem;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use allocative::Key;
use allocative::Visitor;

/// Labels are never freed, so they are carved out of leaked chunks instead of
/// paying jemalloc per-allocation metadata and size-class rounding. Racy
/// insert losers leave small holes; those and end-of-chunk remainders are the
/// arena's only waste, reported as `label_arena_slack`.
pub(in crate::target::label) struct LabelArenas {
    arenas: [Mutex<BumpState>; ARENA_COUNT],
    /// Total bytes reserved from the system allocator.
    reserved: AtomicUsize,
    /// Total bytes handed out to labels.
    used: AtomicUsize,
}

const ARENA_COUNT: usize = 64;

/// Bump step and guaranteed alignment of every carve. `LabelData::arena_size`
/// rounds with this same constant, which is what keeps the `abandon` and
/// allocative accounting equal to what `alloc` actually consumed.
pub(in crate::target::label) const BUMP_STEP: usize = mem::align_of::<u64>();

/// Small chunks in tests so chunk rollover is exercised constantly.
#[cfg(test)]
const CHUNK: usize = 4096;
// 64KiB: large enough to amortize chunk installs, small enough that the 64
// arenas' partially-filled current chunks bound average slack to ~2MiB
// process-wide (measured: 1MiB chunks cost ~30MB of slack on an
// adfinder-sized graph, more than the packing saves).
#[cfg(not(test))]
const CHUNK: usize = 64 << 10;

struct BumpState {
    cur: *mut u8,
    remaining: usize,
    /// Every chunk ever allocated, with its size, so `Drop` can free them.
    chunks: Vec<(NonNull<u8>, usize)>,
}

/// Only ever runs for arenas that are dropped — in practice, test-local
/// arenas. The global static never drops, which is what makes handing out
/// `'static` label storage from it sound; a dropped arena must not have
/// outstanding labels.
impl Drop for BumpState {
    fn drop(&mut self) {
        for (chunk, size) in &self.chunks {
            // SAFETY: allocated in `alloc` with exactly this layout and
            // never freed elsewhere.
            unsafe {
                alloc::dealloc(
                    chunk.as_ptr(),
                    Layout::from_size_align(*size, BUMP_STEP)
                        .expect("layout was valid at allocation"),
                );
            }
        }
    }
}

// SAFETY: `cur` and `chunks` point into leaked allocations owned solely by
// this state; access is serialized by the enclosing `Mutex`.
unsafe impl Send for BumpState {}

impl LabelArenas {
    pub(in crate::target::label) const fn new() -> LabelArenas {
        LabelArenas {
            arenas: [const {
                Mutex::new(BumpState {
                    cur: std::ptr::null_mut(),
                    remaining: 0,
                    chunks: Vec::new(),
                })
            }; ARENA_COUNT],
            reserved: AtomicUsize::new(0),
            used: AtomicUsize::new(0),
        }
    }

    /// Carve `layout` out of the arena selected by `hash`'s high bits (the
    /// same bits that select the interner shard, so allocation contention
    /// tracks table contention). The memory is never reclaimed.
    pub(in crate::target::label) fn alloc(&self, hash: u64, layout: Layout) -> NonNull<u8> {
        assert!(layout.align() <= BUMP_STEP);
        // Keep the cursor 8-aligned so every allocation is validly aligned
        // for `LabelData`.
        let size = layout.size().next_multiple_of(BUMP_STEP);
        let index = (hash >> 58) as usize & (ARENA_COUNT - 1);

        let mut state = self.arenas[index].lock().expect("label arena poisoned");
        if state.remaining < size {
            let chunk_size = CHUNK.max(size);
            let chunk_layout =
                Layout::from_size_align(chunk_size, BUMP_STEP).expect("label arena chunk layout");
            // SAFETY: non-zero size; the chunk is intentionally leaked (label
            // storage is immortal) but stays reachable through `chunks`.
            let base = unsafe { alloc::alloc(chunk_layout) };
            let Some(base) = NonNull::new(base) else {
                alloc::handle_alloc_error(chunk_layout);
            };
            state.chunks.push((base, chunk_size));
            state.cur = base.as_ptr();
            state.remaining = chunk_size;
            self.reserved.fetch_add(chunk_size, Ordering::Relaxed);
        }
        let out = state.cur;
        // SAFETY: `size <= state.remaining`, so the cursor stays inside the
        // current chunk (one-past-the-end at most).
        state.cur = unsafe { out.add(size) };
        state.remaining -= size;
        self.used.fetch_add(size, Ordering::Relaxed);
        // The cursor is never null once a chunk is installed.
        NonNull::new(out).expect("bump cursor is non-null")
    }

    /// Move an abandoned carve's bytes from `used` to slack. The memory
    /// itself is not reused — an abandoned carve (the losing candidate of a
    /// racy insert) stays a hole in its chunk — but reclassifying it keeps
    /// `used` equal to live label bytes and makes `label_arena_slack` report
    /// the hole. `size` must be the value `alloc` accounted, i.e. the layout
    /// size rounded up to the 8-byte bump step.
    pub(in crate::target::label) fn abandon(&self, size: usize) {
        self.used.fetch_sub(size, Ordering::Relaxed);
    }
}

impl Allocative for LabelArenas {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        let reserved = self.reserved.load(Ordering::Relaxed);
        let used = self.used.load(Ordering::Relaxed);
        // Label payload bytes are accounted per entry through the interner
        // (`label_data`); only the arena's overhead is reported here.
        visitor.visit_simple(Key::new("label_arena_slack"), reserved.saturating_sub(used));
        visitor.exit();
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::Layout;

    use crate::target::label::arena::CHUNK;
    use crate::target::label::arena::LabelArenas;

    #[test]
    fn test_chunk_rollover_and_alignment() {
        let arenas = LabelArenas::new();
        let mut ptrs = Vec::new();
        // Far more than one test-sized chunk per arena.
        for i in 0..10_000u64 {
            let layout = Layout::from_size_align(16 + (i % 40) as usize, 8).unwrap();
            let p = arenas.alloc(i << 58, layout);
            assert_eq!(0, p.as_ptr() as usize % 8, "every allocation is 8-aligned");
            ptrs.push((p, layout.size()));
        }
        // Every allocation is writable over its full size (would trip ASAN
        // if the carve overlapped or escaped a chunk).
        for (p, size) in &ptrs {
            unsafe { std::ptr::write_bytes(p.as_ptr(), 0xAB, *size) };
        }
        let reserved = arenas.reserved.load(std::sync::atomic::Ordering::Relaxed);
        assert!(reserved >= 10_000 * 16, "chunks were actually reserved");
        assert_eq!(0, reserved % CHUNK, "reservations are whole chunks");
    }
}
