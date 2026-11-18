/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The `IForwardRelativePath` handle, its encoding, and the core API that every invariant in this
//! module depends on.

use std::ptr::NonNull;
use std::slice;

use dupe::Dupe;
use mini_vec::packed_ptr::PackedPtr;

use super::interner::INTERNER;
use crate::paths::forward_rel_path::ForwardRelativePath;

/// An interned `ForwardRelativePath`. `Copy`, and one machine word.
///
/// Interned paths live forever: the interner never frees anything. So be thoughtful about what
/// gets interned. Typically don't intern things yourself at all and rely on the higher level types
/// that have made the decision to be interned (`IProjectRelativePath`, `ICellRelativePath`); in
/// buck2 those hold source paths and the like, which end up in DICE keys and are long-lived anyway.
///
/// Interning dedupes, but handles are not unique: sub-paths of an interned path (its parent, a
/// prefix, a tail) are handles into the same bytes with no table entry of their own. Equality and
/// hashing go through the bytes; equal pointers mean equal paths, but not the converse.
///
/// # Representation
///
/// A pointer with one byte of metadata packed into its high bits. For a path of up to
/// `MAX_INLINE_LEN` bytes, which is every path buck2 normally sees, the metadata is the length
/// and the pointer is to the bytes themselves. That is what makes sub-paths free: any slice of an
/// interned path's bytes at component boundaries is itself a valid handle, with no header to find
/// and no terminator to rely on. Longer paths carry the marker `LONG` instead, and point at a
/// `LongHeader` holding the length ahead of the bytes. A sub-path of such a path that is itself
/// long is interned as a copy; that case is not worth being clever about.
#[derive(Copy, Clone, Dupe)]
#[repr(transparent)]
pub struct IForwardRelativePath(PackedPtr<NonNull<u8>>);

mini_vec::size_assert::words_of_type!(IForwardRelativePath, 1);

// SAFETY: A handle points at bytes that are never written or freed (type invariant), so it is a
// `&'static [u8]` in all but name.
unsafe impl Send for IForwardRelativePath {}
unsafe impl Sync for IForwardRelativePath {}

/// The longest path stored with its length in the handle.
pub(super) const MAX_INLINE_LEN: usize = 254;

/// Metadata value of a handle that points at a `LongHeader`.
const LONG: u8 = 255;

/// What a `LONG` handle points at. The path's bytes follow it in memory.
#[repr(C)]
pub(super) struct LongHeader {
    pub(super) len: usize,
}

pub(super) const LONG_HEADER_SIZE: usize = size_of::<LongHeader>();

impl IForwardRelativePath {
    /// The empty path.
    #[inline]
    pub const fn empty() -> Self {
        // SAFETY: The pointer of a `&'static str` points at a static; its high bits are clear on
        // every platform `PackedPtr` supports.
        unsafe {
            Self(PackedPtr::from_ptr_zero_extra(NonNull::new_unchecked(
                "".as_ptr().cast_mut(),
            )))
        }
    }

    /// Intern a path.
    #[inline]
    pub fn intern_directly(path: &ForwardRelativePath) -> Self {
        INTERNER.intern(path)
    }

    /// Intern the path obtained by joining `parts` with `/`, skipping empty parts.
    ///
    /// Allocation free when the result is already interned.
    pub fn intern_concat_directly<'a, I>(parts: I) -> Self
    where
        I: IntoIterator<Item = &'a ForwardRelativePath>,
        I::IntoIter: Clone,
    {
        INTERNER.intern_parts(parts.into_iter())
    }

    /// Intern the path obtained by joining `parts` with `/` (skipping empty parts), together with
    /// each tail obtained by joining `parts[K..]` for each `K` in `tail_skips`.
    ///
    /// All `N + 1` results share one allocation: each tail is a handle into the bytes of the full
    /// path, and each is registered in the interner so that later requests for it find it.
    /// Allocation free when the full path is already interned.
    ///
    /// This is how a project relative path and the cell relative path inside it are interned as
    /// one: `intern_concat_with_tails([cell_root, cell_relative], [1])`.
    ///
    /// `tail_skips` must be sorted in non-decreasing order. Returns `None` if any element exceeds
    /// the number of items in `parts`.
    pub fn intern_concat_with_tails<'a, I, const N: usize>(
        parts: I,
        tail_skips: [usize; N],
    ) -> Option<(IForwardRelativePath, [IForwardRelativePath; N])>
    where
        I: IntoIterator<Item = &'a ForwardRelativePath>,
        I::IntoIter: Clone,
    {
        INTERNER.intern_concat_with_tails(parts.into_iter(), tail_skips)
    }

    /// The underlying string.
    #[inline]
    pub fn as_str(self) -> &'static str {
        // SAFETY: Every handle points at valid UTF-8 (type invariant).
        unsafe { std::str::from_utf8_unchecked(self.as_bytes()) }
    }

    /// View as a `&ForwardRelativePath`.
    #[inline]
    pub fn as_forward_relative_path(self) -> &'static ForwardRelativePath {
        ForwardRelativePath::unchecked_new(self.as_str())
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(super) fn len(self) -> usize {
        self.parts().1
    }

    #[inline]
    pub(super) fn as_bytes(self) -> &'static [u8] {
        let (ptr, len) = self.parts();
        // SAFETY: Every handle points at `len` readable bytes that live forever (type invariant).
        unsafe { slice::from_raw_parts(ptr, len) }
    }

    /// The handle for a sub-path of an interned path.
    ///
    /// This is what turns the borrowed sub-paths that `ForwardRelativePath`'s methods return
    /// (`parent`, `strip_prefix`, ...) back into handles when called on an interned path. Free for
    /// paths of up to `MAX_INLINE_LEN` bytes; longer ones are interned as copies.
    pub(super) fn from_sub_path(path: &'static ForwardRelativePath) -> Self {
        let bytes = path.as_str().as_bytes();
        if bytes.len() <= MAX_INLINE_LEN {
            // SAFETY: `path` is a valid `ForwardRelativePath` that lives forever, and it is short
            // enough for an inline handle.
            unsafe { Self::inline(NonNull::from(bytes).cast(), bytes.len()) }
        } else {
            INTERNER.intern(path)
        }
    }

    /// A handle to `len` bytes of path at `ptr`.
    ///
    /// # Safety
    ///
    /// `ptr` must point at `len` bytes that form a valid `ForwardRelativePath` and that are never
    /// freed or written again, and `len` must be at most `MAX_INLINE_LEN`.
    #[inline]
    pub(super) unsafe fn inline(ptr: NonNull<u8>, len: usize) -> Self {
        debug_assert!(len <= MAX_INLINE_LEN);
        Self(PackedPtr::new(ptr, len as u8))
    }

    /// A handle to the path stored behind `header`.
    ///
    /// # Safety
    ///
    /// `header` must point at an initialized `LongHeader` immediately followed by `header.len`
    /// bytes that form a valid `ForwardRelativePath`, none of which are ever freed or written
    /// again.
    #[inline]
    pub(super) unsafe fn long(header: NonNull<LongHeader>) -> Self {
        Self(PackedPtr::new(header.cast::<u8>(), LONG))
    }

    /// The handle as a single pointer, for the interner's table. Not for dereferencing: the
    /// metadata sits in its high bits.
    pub(super) fn to_ptr(self) -> NonNull<()> {
        self.0.to_packed_ptr().cast()
    }

    /// # Safety
    ///
    /// `ptr` must have been produced by `to_ptr`.
    pub(super) unsafe fn from_ptr(ptr: NonNull<()>) -> Self {
        // SAFETY: `to_ptr` only ever packs handles, and what they point at lives forever.
        Self(unsafe { PackedPtr::from_packed_ptr(ptr.cast()) })
    }

    /// Pointer to and length of the path's bytes.
    #[inline]
    fn parts(self) -> (*const u8, usize) {
        let (ptr, extra) = self.0.into_parts();
        let ptr: *const u8 = ptr.as_ptr();
        if extra == LONG {
            // SAFETY: A `LONG` handle points at a `LongHeader` (type invariant), which was
            // allocated with the header's alignment and is followed by its `len` bytes.
            unsafe { (ptr.add(LONG_HEADER_SIZE), (*ptr.cast::<LongHeader>()).len) }
        } else {
            (ptr, extra as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::paths::forward_rel_path::ForwardRelativePath;
    use crate::paths::forward_rel_path::ForwardRelativePathBuf;
    use crate::paths::intern_forward_rel_path::IForwardRelativePath;
    use crate::paths::intern_forward_rel_path::base::MAX_INLINE_LEN;

    fn path(s: &str) -> &ForwardRelativePath {
        ForwardRelativePath::new(s).unwrap()
    }

    /// A path of `len` bytes whose components are unique to `tag`, so tests don't find each
    /// other's entries.
    fn long_path(tag: &str, len: usize) -> ForwardRelativePathBuf {
        let mut s = String::new();
        while s.len() < len {
            if !s.is_empty() {
                s.push('/');
            }
            s.push_str(tag);
            s.push_str(&s.len().to_string());
        }
        s.truncate(len);
        // Don't end on a separator.
        if s.ends_with('/') {
            s.pop();
            s.push('x');
        }
        ForwardRelativePathBuf::new(s).unwrap()
    }

    #[test]
    fn test_empty() {
        let p = IForwardRelativePath::empty();
        assert!(p.is_empty());
        assert_eq!("", p.as_str());
        assert_eq!(IForwardRelativePath::empty(), p);
    }

    #[test]
    fn test_intern_and_equality() {
        let a = IForwardRelativePath::intern_directly(path("foo/bar"));
        let b = IForwardRelativePath::intern_directly(path("foo/bar"));
        assert_eq!(a, b);
        // Same pointer (interned).
        assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr());
    }

    #[test]
    fn test_concat() {
        let result =
            IForwardRelativePath::intern_concat_directly([path("foo"), path("bar"), path("baz")]);
        assert_eq!("foo/bar/baz", result.as_str());
    }

    #[test]
    fn test_concat_with_empties() {
        let e = ForwardRelativePath::empty();
        let result =
            IForwardRelativePath::intern_concat_directly([e, path("foo"), e, path("bar"), e]);
        assert_eq!("foo/bar", result.as_str());
    }

    #[test]
    fn test_concat_all_empty() {
        let e = ForwardRelativePath::empty();
        let result = IForwardRelativePath::intern_concat_directly([e, e]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_concat_cache_hit() {
        let direct = IForwardRelativePath::intern_directly(path("hit_a/hit_b/hit_c"));
        // Whatever the chunking, the same entry is found.
        let from_parts = IForwardRelativePath::intern_concat_directly([
            path("hit_a"),
            path("hit_b"),
            path("hit_c"),
        ]);
        let from_parts2 =
            IForwardRelativePath::intern_concat_directly([path("hit_a/hit_b"), path("hit_c")]);
        assert_eq!(direct, from_parts);
        assert_eq!(direct.as_str().as_ptr(), from_parts.as_str().as_ptr());
        assert_eq!(direct.as_str().as_ptr(), from_parts2.as_str().as_ptr());
    }

    #[test]
    fn test_sub_paths_share_bytes() {
        let p = IForwardRelativePath::intern_directly(path("share_a/share_b/share_c"));
        let parent = p.parent().unwrap();
        assert_eq!("share_a/share_b", parent.as_str());
        assert_eq!(p.as_str().as_ptr(), parent.as_str().as_ptr());
        let (_, rest) = p.split_first().unwrap();
        assert_eq!("share_b/share_c", rest.as_str());
        assert_eq!(rest.as_str().as_ptr(), unsafe {
            p.as_str().as_ptr().add("share_a/".len())
        });
        // Equal by content to a freshly interned copy, even though the pointers differ.
        assert_eq!(
            parent,
            IForwardRelativePath::intern_directly(path("share_a/share_b"))
        );
    }

    #[test]
    fn test_long_path() {
        let long = long_path("longp", MAX_INLINE_LEN * 3);
        assert!(long.as_str().len() > MAX_INLINE_LEN);
        let a = IForwardRelativePath::intern_directly(&long);
        assert_eq!(long.as_str(), a.as_str());
        let b = IForwardRelativePath::intern_directly(&long);
        assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr());
        // A short sub-path of a long path is a free handle into the same bytes.
        let components = a.iter().count();
        let tail = a.strip_prefix_components(components - 1).unwrap();
        assert!(tail.as_str().len() <= MAX_INLINE_LEN);
        assert_eq!(tail.as_str().as_ptr(), unsafe {
            a.as_str()
                .as_ptr()
                .add(a.as_str().len() - tail.as_str().len())
        });
        // A long sub-path is a copy, and round-trips.
        let parent = a.parent().unwrap();
        assert!(parent.as_str().len() > MAX_INLINE_LEN);
        assert_eq!(long.parent().unwrap().as_str(), parent.as_str());
        assert_eq!(
            parent,
            IForwardRelativePath::intern_directly(long.parent().unwrap())
        );
    }

    #[test]
    fn test_long_path_boundary() {
        for len in [MAX_INLINE_LEN - 1, MAX_INLINE_LEN, MAX_INLINE_LEN + 1] {
            let p = long_path("bound", len);
            let i = IForwardRelativePath::intern_directly(&p);
            assert_eq!(p.as_str(), i.as_str());
            assert_eq!(p.as_str().len(), i.as_str().len());
            let again = IForwardRelativePath::intern_concat_directly([
                path(p.as_str().split_once('/').unwrap().0),
                path(p.as_str().split_once('/').unwrap().1),
            ]);
            assert_eq!(i.as_str().as_ptr(), again.as_str().as_ptr());
        }
    }

    #[test]
    fn test_intern_concat_with_tails_shares_allocation() {
        // Unique components per test so we don't hit an entry another test
        // inserted — sharing only happens when this call is the cache miss.
        let (full, [cell_relative]) = IForwardRelativePath::intern_concat_with_tails(
            [path("cell_a"), path("cell_b/cell_c")],
            [1],
        )
        .unwrap();
        assert_eq!("cell_a/cell_b/cell_c", full.as_str());
        assert_eq!("cell_b/cell_c", cell_relative.as_str());
        // Tail's bytes sit inside the parent's allocation.
        assert_eq!(cell_relative.as_str().as_ptr(), unsafe {
            full.as_str().as_ptr().add("cell_a/".len())
        });
        // And the tail is registered: interning it directly finds that entry.
        let direct = IForwardRelativePath::intern_directly(path("cell_b/cell_c"));
        assert_eq!(direct.as_str().as_ptr(), cell_relative.as_str().as_ptr());
    }

    #[test]
    fn test_intern_concat_with_tails_finds_existing_tail() {
        // Pre-intern the tail as its own entry. The construction-time call
        // should return a tail equal-by-content to the existing entry; the
        // exact pointer depends on which side wins the table race.
        let direct = IForwardRelativePath::intern_directly(path("existing_b/existing_c"));
        let (_full, [tail]) = IForwardRelativePath::intern_concat_with_tails(
            [path("existing_a"), path("existing_b/existing_c")],
            [1],
        )
        .unwrap();
        assert_eq!(direct, tail);
    }

    #[test]
    fn test_intern_concat_with_tails_finds_existing_parent() {
        // Pre-intern the full joined path. A later intern_concat_with_tails
        // dedupes the parent against the existing entry; the tail bytes then
        // point into the pre-existing allocation.
        let direct = IForwardRelativePath::intern_directly(path("dup_a/dup_b/dup_c"));
        let (parent, [tail]) = IForwardRelativePath::intern_concat_with_tails(
            [path("dup_a"), path("dup_b/dup_c")],
            [1],
        )
        .unwrap();
        assert_eq!(direct.as_str().as_ptr(), parent.as_str().as_ptr());
        assert_eq!(tail.as_str().as_ptr(), unsafe {
            parent.as_str().as_ptr().add("dup_a/".len())
        });
    }

    #[test]
    fn test_intern_concat_with_tails_empty_tail() {
        // Skip count == part count → empty tail.
        let (full, [tail]) = IForwardRelativePath::intern_concat_with_tails(
            [path("emptytail_a"), path("emptytail_b")],
            [2],
        )
        .unwrap();
        assert_eq!("emptytail_a/emptytail_b", full.as_str());
        assert!(tail.is_empty());
        assert_eq!(IForwardRelativePath::empty(), tail);
    }

    #[test]
    fn test_intern_concat_with_tails_too_many_skips() {
        // skip 3 with only 2 parts → None.
        assert!(
            IForwardRelativePath::intern_concat_with_tails(
                [path("toomany_a"), path("toomany_b")],
                [3]
            )
            .is_none()
        );
    }

    #[test]
    fn test_intern_concat_with_tails_multi() {
        // Multiple tails at different skip counts, all sharing the parent.
        let (full, [t1, t2, t3]) = IForwardRelativePath::intern_concat_with_tails(
            [
                path("multi_a"),
                path("multi_b"),
                path("multi_c"),
                path("multi_d"),
            ],
            [1, 2, 3],
        )
        .unwrap();
        assert_eq!("multi_a/multi_b/multi_c/multi_d", full.as_str());
        assert_eq!("multi_b/multi_c/multi_d", t1.as_str());
        assert_eq!("multi_c/multi_d", t2.as_str());
        assert_eq!("multi_d", t3.as_str());
        let p = full.as_str().as_ptr();
        assert_eq!(t1.as_str().as_ptr(), unsafe { p.add("multi_a/".len()) });
        assert_eq!(t2.as_str().as_ptr(), unsafe {
            p.add("multi_a/multi_b/".len())
        });
        assert_eq!(t3.as_str().as_ptr(), unsafe {
            p.add("multi_a/multi_b/multi_c/".len())
        });
    }

    #[test]
    fn test_intern_concat_with_tails_skips_empty_parts() {
        // Empty parts contribute no content but still count as parts for
        // skip indexing.
        let e = ForwardRelativePath::empty();
        let (full, [t1, t2, t3]) = IForwardRelativePath::intern_concat_with_tails(
            [path("empties_a"), e, path("empties_b")],
            [1, 2, 3],
        )
        .unwrap();
        assert_eq!("empties_a/empties_b", full.as_str());
        // Skip 1 = drop "empties_a"; tail = join of ["", "empties_b"] = "empties_b".
        assert_eq!("empties_b", t1.as_str());
        // Skip 2 = drop "empties_a" and ""; same tail.
        assert_eq!("empties_b", t2.as_str());
        assert!(t3.is_empty());
        // Both non-empty tails point to the same offset in the parent.
        assert_eq!(t1.as_str().as_ptr(), t2.as_str().as_ptr());
        assert_eq!(t1.as_str().as_ptr(), unsafe {
            full.as_str().as_ptr().add("empties_a/".len())
        });
    }

    #[test]
    fn test_intern_concat_with_tails_multi_component_parts() {
        // Parts can themselves contain `/`. Skip count is by part index,
        // not component index.
        let (full, [t1]) =
            IForwardRelativePath::intern_concat_with_tails([path("mc_a/mc_b"), path("mc_c")], [1])
                .unwrap();
        assert_eq!("mc_a/mc_b/mc_c", full.as_str());
        assert_eq!("mc_c", t1.as_str());
        assert_eq!(t1.as_str().as_ptr(), unsafe {
            full.as_str().as_ptr().add("mc_a/mc_b/".len())
        });
    }

    #[test]
    fn test_intern_concat_with_tails_no_tails() {
        // N=0 — equivalent to `intern_concat_directly` via this path.
        let (full, []) = IForwardRelativePath::intern_concat_with_tails(
            [path("notails_a"), path("notails_b")],
            [],
        )
        .unwrap();
        assert_eq!("notails_a/notails_b", full.as_str());
    }

    // With panic=abort the panic kills the in-process Miri harness.
    #[cfg(not(all(miri, panic = "abort")))]
    #[test]
    #[should_panic(expected = "tail_skips must be sorted")]
    fn test_intern_concat_with_tails_unsorted_panics() {
        let _ = IForwardRelativePath::intern_concat_with_tails(
            [path("unsorted_a"), path("unsorted_b"), path("unsorted_c")],
            [2, 1],
        );
    }

    #[test]
    fn test_intern_concat_with_tails_trailing_empty_part() {
        // Skip target lands on a part whose only successors are empty —
        // the recorded "next would-be-separator" position ends up past
        // total_len and must be clamped to produce an empty tail.
        let e = ForwardRelativePath::empty();
        let (full, [tail]) =
            IForwardRelativePath::intern_concat_with_tails([path("trailing_a"), e], [1]).unwrap();
        assert_eq!("trailing_a", full.as_str());
        assert!(tail.is_empty());
    }

    #[test]
    fn test_intern_concat_with_tails_all_empty_parts() {
        let e = ForwardRelativePath::empty();
        let (full, [t1, t2]) =
            IForwardRelativePath::intern_concat_with_tails([e, e], [1, 2]).unwrap();
        assert!(full.is_empty());
        assert!(t1.is_empty());
        assert!(t2.is_empty());
    }

    #[test]
    fn test_intern_concat_with_tails_long() {
        // A long parent with a short tail: the tail is a free handle into the parent. A long tail
        // is a copy that still compares equal.
        let head = long_path("ltail_head", MAX_INLINE_LEN + 10);
        let short_tail = path("ltail_x/ltail_y");
        let (full, [tail]) =
            IForwardRelativePath::intern_concat_with_tails([&*head, short_tail], [1]).unwrap();
        assert_eq!(
            format!("{}/{}", head.as_str(), short_tail.as_str()),
            full.as_str()
        );
        assert_eq!(short_tail.as_str(), tail.as_str());
        assert_eq!(tail.as_str().as_ptr(), unsafe {
            full.as_str().as_ptr().add(head.as_str().len() + 1)
        });

        let long_tail = long_path("ltail_tail", MAX_INLINE_LEN + 10);
        let (full, [tail]) =
            IForwardRelativePath::intern_concat_with_tails([path("ltail_h"), &*long_tail], [1])
                .unwrap();
        assert_eq!(long_tail.as_str(), tail.as_str());
        assert_eq!(format!("ltail_h/{}", long_tail.as_str()), full.as_str());
        assert_eq!(tail, IForwardRelativePath::intern_directly(&long_tail));
    }

    #[test]
    fn test_concurrent_interning_dedupes() {
        let paths: Vec<ForwardRelativePathBuf> = (0..64)
            .map(|i| ForwardRelativePathBuf::new(format!("conc_{i}/dir/file{i}.txt")).unwrap())
            .collect();
        let results: Vec<Vec<IForwardRelativePath>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        paths
                            .iter()
                            .map(|p| IForwardRelativePath::intern_directly(p))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (i, p) in paths.iter().enumerate() {
            for r in &results {
                assert_eq!(p.as_str(), r[i].as_str());
                assert_eq!(results[0][i].as_str().as_ptr(), r[i].as_str().as_ptr());
            }
        }
    }
}
