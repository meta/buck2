/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The interner: a lock-free table of `IForwardRelativePath` handles over storage that is never
//! freed.
//!
//! # Storage model
//!
//! The table holds handles and nothing else. The bytes a handle points at live in the sharded bump
//! arena (see [`bump`]) or, for the rare path longer than `MAX_INLINE_LEN`, in a dedicated
//! allocation with a length header. Neither is ever freed: handles are `Copy`, have no `Drop`, and
//! expose no deallocation API, and a thread that allocates bytes and then loses the insert race
//! simply orphans them. Insert races are expected to be rare enough for that to be fine.
//!
//! Several table entries may point into one allocation: `intern_concat_with_tails` registers the
//! tails of a path as handles into the path's own bytes.
//!
//! # Hashing
//!
//! Paths are hashed component by component (split on `/`), followed by a terminator, so that
//! interning `a/b/c` as one string, as the parts `["a", "b", "c"]`, or as the parts `["a/b", "c"]`
//! all land on the same entry. `BuckHasher` is not streaming safe across `write` calls, so how the
//! input is chunked has to be fixed by convention, and this is the convention.

use std::alloc;
use std::alloc::Layout;
use std::hash::Hasher;
use std::ptr::NonNull;
use std::slice;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use buck2_hash::BuckHasher;
use lock_free_hashtable::atomic_value::AtomicValue;
use lock_free_hashtable::sharded::ShardedLockFreeRawTable;

use super::base::IForwardRelativePath;
use super::base::LONG_HEADER_SIZE;
use super::base::LongHeader;
use super::base::MAX_INLINE_LEN;
use super::bump::BumpArena;
use crate::paths::forward_rel_path::ForwardRelativePath;

// ---------------------------------------------------------------------------
// Table slots
// ---------------------------------------------------------------------------

/// A table slot: a handle, stored as its packed pointer.
#[derive(Copy, Clone)]
struct Slot(IForwardRelativePath);

impl AtomicValue for Slot {
    type Ref<'a> = Slot;

    #[inline]
    fn into_raw(this: Self) -> NonNull<()> {
        this.0.to_ptr()
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> Self {
        // SAFETY: The table only ever hands back pointers produced by `into_raw`.
        Slot(unsafe { IForwardRelativePath::from_ptr(raw) })
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Slot
    where
        Self: 'a,
    {
        // SAFETY: See `from_raw`.
        Slot(unsafe { IForwardRelativePath::from_ptr(raw) })
    }
}

// ---------------------------------------------------------------------------
// Hashing and comparison against parts
// ---------------------------------------------------------------------------

/// Written after the last component so that the hash is nearly prefix free.
const HASH_TERMINATOR: u64 = 15167991606973351771;

fn hash_components_into(hasher: &mut BuckHasher, path: &ForwardRelativePath) {
    for component in path.as_str().split('/') {
        hasher.write(component.as_bytes());
    }
}

fn hash_path(path: &ForwardRelativePath) -> u64 {
    let mut hasher = BuckHasher::default();
    hash_components_into(&mut hasher, path);
    hasher.write_u64(HASH_TERMINATOR);
    hasher.finish()
}

/// The hash and joined length of `parts`, or `None` if the joined path is empty.
fn hash_and_len_parts<'a>(
    parts: impl Iterator<Item = &'a ForwardRelativePath>,
) -> Option<(u64, usize)> {
    let mut hasher = BuckHasher::default();
    let mut total_len: usize = 0;
    let mut has_content = false;
    for part in parts {
        if part.as_str().is_empty() {
            continue;
        }
        if has_content {
            total_len += 1;
        }
        has_content = true;
        total_len += part.as_str().len();
        hash_components_into(&mut hasher, part);
    }
    if !has_content {
        return None;
    }
    hasher.write_u64(HASH_TERMINATOR);
    Some((hasher.finish(), total_len))
}

/// Whether `stored` is the path obtained by joining `parts` with `/`, skipping empty parts.
fn eq_parts<'a>(stored: &[u8], parts: impl Iterator<Item = &'a ForwardRelativePath>) -> bool {
    let mut idx = 0;
    let mut first = true;
    for part in parts {
        let s = part.as_str().as_bytes();
        if s.is_empty() {
            continue;
        }
        if !first {
            if stored.get(idx) != Some(&b'/') {
                return false;
            }
            idx += 1;
        }
        first = false;
        if stored.len() < idx + s.len() || &stored[idx..idx + s.len()] != s {
            return false;
        }
        idx += s.len();
    }
    idx == stored.len()
}

/// Write the path obtained by joining `parts` with `/` (skipping empty parts) into `dst`, which
/// must be exactly the joined length long.
fn write_parts<'a>(dst: &mut [u8], parts: impl Iterator<Item = &'a ForwardRelativePath>) {
    let mut idx = 0;
    let mut first = true;
    for part in parts {
        let s = part.as_str().as_bytes();
        if s.is_empty() {
            continue;
        }
        if !first {
            dst[idx] = b'/';
            idx += 1;
        }
        first = false;
        dst[idx..idx + s.len()].copy_from_slice(s);
        idx += s.len();
    }
    assert_eq!(idx, dst.len());
}

/// What one pass over `parts` learns on behalf of `intern_concat_with_tails`.
struct TailsWalk<const N: usize> {
    /// Hash of the joined path. Only meaningful if `total_len > 0`.
    hash: u64,
    /// Joined length.
    total_len: usize,
    /// Byte offset in the joined path at which each tail begins, in the caller's order.
    tail_offsets: [usize; N],
}

/// Returns `None` if any element of `tail_skips` exceeds the number of items `parts` yields.
///
/// `tail_skips` must be sorted in non-decreasing order so that the cursors over `parts` and
/// `tail_skips` can advance in lockstep.
fn walk_parts_for_tails<'a, const N: usize>(
    parts: impl Iterator<Item = &'a ForwardRelativePath>,
    tail_skips: [usize; N],
) -> Option<TailsWalk<N>> {
    assert!(
        tail_skips.windows(2).all(|w| w[0] <= w[1]),
        "tail_skips must be sorted in non-decreasing order",
    );

    let mut hasher = BuckHasher::default();
    let mut total_len: usize = 0;
    let mut has_content = false;
    let mut tail_offsets = [0usize; N];
    let mut cursor = 0;
    let mut part_count: usize = 0;

    for part in parts {
        // Where this part begins (or would begin, if it is empty) in the joined path.
        let here = if has_content { total_len + 1 } else { 0 };
        while cursor < N && tail_skips[cursor] == part_count {
            tail_offsets[cursor] = here;
            cursor += 1;
        }
        if !part.as_str().is_empty() {
            if has_content {
                total_len += 1;
            }
            has_content = true;
            total_len += part.as_str().len();
            hash_components_into(&mut hasher, part);
        }
        part_count += 1;
    }

    // Anything left is `>= part_count`: equal means an empty tail, greater is a caller bug.
    while cursor < N {
        if tail_skips[cursor] > part_count {
            return None;
        }
        tail_offsets[cursor] = total_len;
        cursor += 1;
    }

    // `here` predicted that a part's content would follow a separator. If that part and every
    // part after it turned out empty, no separator was written and the prediction points one past
    // the end; such a tail is empty.
    for offset in &mut tail_offsets {
        if *offset > total_len {
            *offset = total_len;
        }
    }

    hasher.write_u64(HASH_TERMINATOR);
    Some(TailsWalk {
        hash: hasher.finish(),
        total_len,
        tail_offsets,
    })
}

// ---------------------------------------------------------------------------
// PathInterner
// ---------------------------------------------------------------------------

pub(super) struct PathInterner {
    table: ShardedLockFreeRawTable<Slot, 64>,
    arena: BumpArena,
    /// Bytes handed out for paths too long for the arena, headers included.
    long_bytes: AtomicUsize,
}

pub(super) static INTERNER: PathInterner = PathInterner::new();

impl PathInterner {
    pub(super) const fn new() -> Self {
        PathInterner {
            table: ShardedLockFreeRawTable::new(),
            arena: BumpArena::new(),
            long_bytes: AtomicUsize::new(0),
        }
    }

    pub(super) fn intern(&'static self, path: &ForwardRelativePath) -> IForwardRelativePath {
        if path.is_empty() {
            return IForwardRelativePath::empty();
        }
        let bytes = path.as_str().as_bytes();
        let hash = hash_path(path);
        if let Some(found) = self.table.lookup(hash, |slot| slot.0.as_bytes() == bytes) {
            return found.0;
        }
        let fresh = self.alloc(hash, bytes.len(), |dst| dst.copy_from_slice(bytes));
        self.commit(fresh, hash)
    }

    /// Intern the path obtained by joining `parts` with `/`, skipping empty parts.
    pub(super) fn intern_parts<'p, I>(&'static self, parts: I) -> IForwardRelativePath
    where
        I: Iterator<Item = &'p ForwardRelativePath> + Clone,
    {
        let Some((hash, total_len)) = hash_and_len_parts(parts.clone()) else {
            return IForwardRelativePath::empty();
        };
        self.lookup_or_alloc_parts(parts, hash, total_len)
    }

    /// `hash` and `total_len` must be what `hash_and_len_parts(parts)` computes; callers that
    /// already walked `parts` pass them in to avoid a second walk.
    fn lookup_or_alloc_parts<'p, I>(
        &'static self,
        parts: I,
        hash: u64,
        total_len: usize,
    ) -> IForwardRelativePath
    where
        I: Iterator<Item = &'p ForwardRelativePath> + Clone,
    {
        if let Some(found) = self
            .table
            .lookup(hash, |slot| eq_parts(slot.0.as_bytes(), parts.clone()))
        {
            return found.0;
        }
        let fresh = self.alloc(hash, total_len, |dst| write_parts(dst, parts));
        self.commit(fresh, hash)
    }

    /// See `IForwardRelativePath::intern_concat_with_tails`.
    pub(super) fn intern_concat_with_tails<'p, I, const N: usize>(
        &'static self,
        parts: I,
        tail_skips: [usize; N],
    ) -> Option<(IForwardRelativePath, [IForwardRelativePath; N])>
    where
        I: Iterator<Item = &'p ForwardRelativePath> + Clone,
    {
        let walk = walk_parts_for_tails(parts.clone(), tail_skips)?;
        if walk.total_len == 0 {
            let empty = IForwardRelativePath::empty();
            return Some((empty, [empty; N]));
        }
        let parent = self.lookup_or_alloc_parts(parts, walk.hash, walk.total_len);
        let tails = std::array::from_fn(|i| self.tail_at(parent, walk.tail_offsets[i]));
        Some((parent, tails))
    }

    /// The tail of `parent` that begins `offset` bytes in, registered in the table. `offset` must
    /// be the end of the path or the start of a component.
    fn tail_at(&'static self, parent: IForwardRelativePath, offset: usize) -> IForwardRelativePath {
        let bytes = parent.as_bytes();
        if offset == bytes.len() {
            return IForwardRelativePath::empty();
        }
        if offset == 0 {
            return parent;
        }
        debug_assert_eq!(bytes[offset - 1], b'/');
        let tail = &bytes[offset..];
        // SAFETY: A suffix of a valid path that starts after a `/` is a valid path.
        let tail_path =
            ForwardRelativePath::unchecked_new(unsafe { std::str::from_utf8_unchecked(tail) });
        if tail.len() <= MAX_INLINE_LEN {
            // SAFETY: `tail` is a slice of `parent`'s bytes, which live forever.
            let handle =
                unsafe { IForwardRelativePath::inline(NonNull::from(tail).cast(), tail.len()) };
            self.commit(handle, hash_path(tail_path))
        } else {
            self.intern(tail_path)
        }
    }

    /// Storage for a new path of `len` bytes, filled in by `fill`. The result is not yet in the
    /// table.
    fn alloc(
        &'static self,
        hash: u64,
        len: usize,
        fill: impl FnOnce(&mut [u8]),
    ) -> IForwardRelativePath {
        debug_assert!(len > 0);
        if len <= MAX_INLINE_LEN {
            let ptr = self.arena.alloc(hash, len);
            // SAFETY: `alloc` returned `len` writable bytes that no one else references yet.
            fill(unsafe { slice::from_raw_parts_mut(ptr.as_ptr(), len) });
            // SAFETY: Just filled with a valid path; arena bytes live forever.
            unsafe { IForwardRelativePath::inline(ptr, len) }
        } else {
            let layout = Layout::from_size_align(LONG_HEADER_SIZE + len, align_of::<LongHeader>())
                .expect("path length overflow");
            // SAFETY: `layout` has a non-zero size.
            let ptr = unsafe { alloc::alloc(layout) };
            let Some(ptr) = NonNull::new(ptr) else {
                alloc::handle_alloc_error(layout)
            };
            // SAFETY: The allocation holds a header followed by `len` bytes, all exclusively ours.
            unsafe {
                ptr.cast::<LongHeader>().write(LongHeader { len });
                fill(slice::from_raw_parts_mut(
                    ptr.as_ptr().add(LONG_HEADER_SIZE),
                    len,
                ));
            }
            self.long_bytes.fetch_add(layout.size(), Ordering::Relaxed);
            // SAFETY: Header and bytes are initialized and never freed.
            unsafe { IForwardRelativePath::long(ptr.cast()) }
        }
    }

    /// Insert `handle` at `hash`, returning whichever handle the table holds for that path
    /// afterwards: `handle`, or an equal one that another thread inserted first.
    fn commit(&'static self, handle: IForwardRelativePath, hash: u64) -> IForwardRelativePath {
        let (winner, _loser) = self.table.insert(
            hash,
            Slot(handle),
            |a, b| a.0.as_bytes() == b.0.as_bytes(),
            |slot| hash_path(slot.0.as_forward_relative_path()),
        );
        winner.0
    }

    /// Bytes held from the global allocator: the arena's chunks (used or not) plus every long
    /// path's allocation.
    pub(super) fn bytes_allocated(&self) -> usize {
        self.arena.bytes_allocated() + self.long_bytes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> &ForwardRelativePath {
        ForwardRelativePath::new(s).unwrap()
    }

    #[test]
    fn test_hash_agrees_across_chunkings() {
        let whole = hash_path(path("ha/hb/hc"));
        assert_eq!(
            Some((whole, "ha/hb/hc".len())),
            hash_and_len_parts([path("ha"), path("hb"), path("hc")].into_iter())
        );
        assert_eq!(
            Some((whole, "ha/hb/hc".len())),
            hash_and_len_parts([path("ha/hb"), path("hc")].into_iter())
        );
        assert_eq!(
            Some((whole, "ha/hb/hc".len())),
            hash_and_len_parts([ForwardRelativePath::empty(), path("ha/hb/hc")].into_iter())
        );
        assert_ne!(whole, hash_path(path("ha/hb")));
        assert_eq!(
            None,
            hash_and_len_parts([ForwardRelativePath::empty()].into_iter())
        );
    }

    #[test]
    fn test_eq_parts() {
        let e = ForwardRelativePath::empty();
        assert!(eq_parts(
            b"a/b/c",
            [path("a"), path("b"), path("c")].into_iter()
        ));
        assert!(eq_parts(b"a/b/c", [path("a/b"), e, path("c")].into_iter()));
        assert!(!eq_parts(b"a/b/c", [path("a"), path("b")].into_iter()));
        assert!(!eq_parts(
            b"a/b",
            [path("a"), path("b"), path("c")].into_iter()
        ));
        assert!(!eq_parts(b"a/bb", [path("a"), path("b")].into_iter()));
        assert!(!eq_parts(b"ab", [path("a"), path("b")].into_iter()));
    }

    #[test]
    fn test_bytes_allocated_grows() {
        let before = INTERNER.bytes_allocated();
        let long = "x".repeat(MAX_INLINE_LEN + 1);
        INTERNER.intern(path(&format!("bytes_alloc_test/{long}")));
        assert!(INTERNER.bytes_allocated() > before);
    }
}
