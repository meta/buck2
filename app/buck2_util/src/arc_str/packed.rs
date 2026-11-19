/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ops::Range;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::arc_str::ThinArcSlice;
use crate::arc_str::ThinArcStr;
use crate::arc_str::string_like::StringInside;

/// A sorted, immutable, refcount-shared list of `(string, tag)` pairs.
///
/// Two thin allocations: one for the concatenated string bytes, one for the
/// per-entry metadata. Stack size is two thin pointers (16 bytes on 64-bit).
///
/// Designed as a memory-compact replacement for `Arc<[(SomeString, Tag)]>` /
/// `SortedSet<ArcS<...>>` when entries are short and the collection is
/// constructed once.
///
/// `T` is the typed view of each string entry (defaulting to `str`). All
/// strings stored in the slice must be valid `T`s; this is enforced at the
/// builder API except for the `_str_unchecked` escape hatches.
#[derive(Pagable)]
#[pagable(bound = "")]
pub struct PackedStringSlice<T: ?Sized + StringInside = str, Tag: PackedTag = ()> {
    bytes: ThinArcStr,
    meta: ThinArcSlice<Entry<Tag>>,
    _phantom: PhantomData<*const T>,
}

impl<T: ?Sized + StringInside, Tag: PackedTag> Clone for PackedStringSlice<T, Tag> {
    fn clone(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
            meta: self.meta.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized + StringInside, Tag: PackedTag> Dupe for PackedStringSlice<T, Tag> {}

impl<T: ?Sized + StringInside, Tag: PackedTag> Allocative for PackedStringSlice<T, Tag> {
    fn visit<'b, 'a: 'b>(&self, visitor: &'b mut allocative::Visitor<'a>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(allocative::Key::new("bytes"), &self.bytes);
        visitor.visit_field(allocative::Key::new("meta"), &self.meta);
        visitor.exit();
    }
}

// SAFETY: `T` is only used as a phantom marker — the actual storage is
// already-`Send`/`Sync` `ThinArcStr`/`ThinArcSlice`. The `T`-cast on `Deref`
// is layout-only and introduces no shared mutable state.
unsafe impl<T: ?Sized + StringInside + Send + Sync, Tag: PackedTag + Send> Send
    for PackedStringSlice<T, Tag>
{
}
unsafe impl<T: ?Sized + StringInside + Send + Sync, Tag: PackedTag + Sync> Sync
    for PackedStringSlice<T, Tag>
{
}

#[derive(Copy, Clone, Dupe, Allocative, Pagable, Debug)]
struct Entry<Tag: PackedTag> {
    /// Exclusive end offset into `bytes` for this entry. The start offset is
    /// the previous entry's `end` (or 0 for entry 0).
    end: u32,
    tag: Tag,
}

/// Trait alias for the bounds [`PackedStringSlice`] requires of its tag type.
/// Implemented automatically for any `Copy + Allocative + Pagable` type.
pub trait PackedTag: Copy + Allocative + Pagable + 'static {}
impl<T: Copy + Allocative + Pagable + 'static> PackedTag for T {}

impl<Tag: PackedTag + PartialEq> PartialEq for Entry<Tag> {
    fn eq(&self, other: &Self) -> bool {
        self.end == other.end && self.tag == other.tag
    }
}

impl<Tag: PackedTag + Eq> Eq for Entry<Tag> {}

impl<Tag: PackedTag + Hash> Hash for Entry<Tag> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.end.hash(state);
        self.tag.hash(state);
    }
}

impl<T: ?Sized + StringInside, Tag: PackedTag + PartialEq> PartialEq for PackedStringSlice<T, Tag> {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.meta == other.meta
    }
}

impl<T: ?Sized + StringInside, Tag: PackedTag + Eq> Eq for PackedStringSlice<T, Tag> {}

impl<T: ?Sized + StringInside, Tag: PackedTag + Hash> Hash for PackedStringSlice<T, Tag> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
        self.meta.hash(state);
    }
}

impl<T: ?Sized + StringInside + fmt::Debug, Tag: PackedTag + fmt::Debug> fmt::Debug
    for PackedStringSlice<T, Tag>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries((0..self.len()).map(|i| (self.get(i), self.get_tag(i))))
            .finish()
    }
}

impl<T: ?Sized + StringInside, Tag: PackedTag> Default for PackedStringSlice<T, Tag> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T: ?Sized + StringInside, Tag: PackedTag> PackedStringSlice<T, Tag> {
    pub fn empty() -> Self {
        Self {
            bytes: ThinArcStr::default(),
            meta: ThinArcSlice::empty(),
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.meta.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.meta.is_empty()
    }

    #[inline]
    fn entry_range(&self, i: usize) -> Range<u32> {
        let end = self.meta[i].end;
        let start = if i == 0 { 0 } else { self.meta[i - 1].end };
        start..end
    }

    #[inline]
    pub fn get(&self, i: usize) -> &T {
        let r = self.entry_range(i);
        T::from_str(&self.bytes[r.start as usize..r.end as usize])
    }

    #[inline]
    pub fn get_tag(&self, i: usize) -> Tag {
        self.meta[i].tag
    }

    /// Iterate `(value, tag)` pairs, in sorted order.
    pub fn iter_with_tag(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&T, Tag)> + ExactSizeIterator + '_ {
        (0..self.len()).map(|i| (self.get(i), self.get_tag(i)))
    }

    /// Iterate just the typed values, in sorted order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    /// Binary search for `key`. Entries must be sorted (which is guaranteed by
    /// construction via [`PackedStringSliceBuilder::sort_and_build`]).
    pub fn binary_search(&self, key: &T) -> Result<usize, usize> {
        let mut lo = 0usize;
        let mut hi = self.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match T::as_str(self.get(mid)).cmp(T::as_str(key)) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(mid),
            }
        }
        Err(lo)
    }

    pub fn contains(&self, key: &T) -> bool {
        self.binary_search(key).is_ok()
    }

    /// The tag of the entry equal to `key`, if there is one. Entries must be sorted.
    pub fn lookup(&self, key: &T) -> Option<Tag> {
        self.binary_search(key).ok().map(|i| self.get_tag(i))
    }

    /// Range of indices whose string starts with `prefix`. Empty range if no
    /// entries match. Entries must be sorted.
    ///
    /// Takes a raw `&str` because a prefix is not necessarily itself a valid
    /// `T` (e.g. `"src/"` is a prefix into a `PackageRelativePath` but is not
    /// itself one).
    pub fn prefix_range(&self, prefix: &str) -> Range<usize> {
        // First index where `x >= prefix`. Any prefix-match also satisfies
        // `x >= prefix`, so the lower bound of the prefix run is here too.
        let lower = self.partition_point(|x| x < prefix);
        // First index where `x` neither matches the prefix nor is less than
        // it — i.e., the first entry after the prefix run.
        let upper = self.partition_point(|x| x < prefix || x.starts_with(prefix));
        lower..upper
    }

    fn partition_point(&self, pred: impl Fn(&str) -> bool) -> usize {
        let mut lo = 0usize;
        let mut hi = self.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(T::as_str(self.get(mid))) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Construct a long-lived [`PackedStr<T>`] view of entry `i`. The handle
    /// shares the bytes buffer with this slice; cloning the handle bumps the
    /// shared refcount only.
    pub fn get_handle(&self, i: usize) -> PackedStr<T> {
        let r = self.entry_range(i);
        PackedStr {
            bytes: self.bytes.dupe(),
            start: r.start,
            end: r.end,
            _phantom: PhantomData,
        }
    }
}

/// Long-lived view of a single string within a [`PackedStringSlice`]'s shared
/// bytes buffer. Stack size is one thin pointer + two `u32`s = 16 bytes on
/// 64-bit (matches `ArcS<T>`).
///
/// Cloning is a single refcount bump on the shared buffer; no per-string
/// allocation. As a tradeoff, any live `PackedStr` keeps the entire originating
/// bytes buffer alive.
#[derive(Allocative, Pagable)]
#[allocative(bound = "")]
#[pagable(bound = "")]
pub struct PackedStr<T: ?Sized + StringInside = str> {
    bytes: ThinArcStr,
    start: u32,
    end: u32,
    _phantom: PhantomData<*const T>,
}

// SAFETY: Same reasoning as `ArcS<T>`. The contained `ThinArcStr` is
// `Send`/`Sync` and the `T`-cast on `Deref` is layout-only (no shared mutable
// state introduced).
unsafe impl<T: ?Sized + StringInside + Send + Sync> Send for PackedStr<T> {}
unsafe impl<T: ?Sized + StringInside + Send + Sync> Sync for PackedStr<T> {}

impl<T: ?Sized + StringInside> Clone for PackedStr<T> {
    fn clone(&self) -> Self {
        Self {
            bytes: self.bytes.dupe(),
            start: self.start,
            end: self.end,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized + StringInside> Dupe for PackedStr<T> {}

impl<T: ?Sized + StringInside> PackedStr<T> {
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.bytes[self.start as usize..self.end as usize]
    }

    /// Construct a standalone handle whose bytes buffer holds only this
    /// string. Useful when there is no enclosing [`PackedStringSlice`] to
    /// share a buffer with.
    pub fn new(s: &T) -> Self {
        let s = T::as_str(s);
        Self {
            bytes: ThinArcStr::from(s),
            start: 0,
            end: s.len() as u32,
            _phantom: PhantomData,
        }
    }
}

// Intentionally no `Deserialize` impl: nothing deserializes these, and the only one
// possible would give each string a buffer of its own (`PackedStr::new`), hiding behind
// a derive the allocation this type is here to avoid.
impl<T: ?Sized + StringInside> Serialize for PackedStr<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<T: ?Sized + StringInside> Deref for PackedStr<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        T::from_str(self.as_str())
    }
}

impl<T: ?Sized + StringInside> AsRef<T> for PackedStr<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized + StringInside> std::borrow::Borrow<T> for PackedStr<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized + StringInside> PartialEq for PackedStr<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<T: ?Sized + StringInside> Eq for PackedStr<T> {}

impl<T: ?Sized + StringInside> Hash for PackedStr<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl<T: ?Sized + StringInside> StrongHash for PackedStr<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().strong_hash(state);
    }
}

impl<T: ?Sized + StringInside> PartialOrd for PackedStr<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ?Sized + StringInside> Ord for PackedStr<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl<T: ?Sized + StringInside + fmt::Display> fmt::Display for PackedStr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + StringInside + fmt::Debug> fmt::Debug for PackedStr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

/// Builder for [`PackedStringSlice`]. Accepts any number of `(value, tag)`
/// entries and produces a single `PackedStringSlice` at the end. `build_*`
/// performs one bytes allocation and one metadata allocation total.
pub struct PackedStringSliceBuilder<T: ?Sized + StringInside = str, Tag: PackedTag = ()> {
    entries: Vec<(String, Tag)>,
    _phantom: PhantomData<*const T>,
}

impl<T: ?Sized + StringInside, Tag: PackedTag> Default for PackedStringSliceBuilder<T, Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized + StringInside, Tag: PackedTag> PackedStringSliceBuilder<T, Tag> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            _phantom: PhantomData,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            _phantom: PhantomData,
        }
    }

    /// Push a typed value.
    pub fn push(&mut self, s: &T, tag: Tag) {
        self.entries.push((T::as_str(s).to_owned(), tag));
    }

    /// Push an owned `String`, avoiding a copy. Caller asserts validity as `T`.
    pub fn push_owned_unchecked(&mut self, s: String, tag: Tag) {
        self.entries.push((s, tag));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn sort_and_build(mut self) -> PackedStringSlice<T, Tag> {
        self.entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        self.build_assumed_sorted()
    }

    /// Caller asserts the entries are already sorted by string content.
    pub fn build_sorted(self) -> PackedStringSlice<T, Tag> {
        debug_assert!(self.entries.windows(2).all(|w| w[0].0 <= w[1].0));
        self.build_assumed_sorted()
    }

    fn build_assumed_sorted(self) -> PackedStringSlice<T, Tag> {
        if self.entries.is_empty() {
            return PackedStringSlice::empty();
        }
        let total_bytes: usize = self.entries.iter().map(|(s, _)| s.len()).sum();
        // Offsets are `u32`, which keeps `Entry` and `PackedStr` small. `ThinArcStr` has the
        // same bound and panics past it; checking here keeps the casts below honest.
        assert!(
            u32::try_from(total_bytes).is_ok(),
            "{total_bytes} bytes of strings in one PackedStringSlice; offsets are u32"
        );
        let mut bytes = String::with_capacity(total_bytes);
        let meta: Vec<Entry<Tag>> = self
            .entries
            .into_iter()
            .map(|(s, tag)| {
                bytes.push_str(&s);
                Entry {
                    end: bytes.len() as u32,
                    tag,
                }
            })
            .collect();
        PackedStringSlice {
            bytes: ThinArcStr::from(bytes),
            meta: ThinArcSlice::from_iter(meta),
            _phantom: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(entries: &[(&str, u8)]) -> PackedStringSlice<str, u8> {
        let mut b: PackedStringSliceBuilder<str, u8> =
            PackedStringSliceBuilder::with_capacity(entries.len());
        for (s, t) in entries {
            b.push(s, *t);
        }
        b.sort_and_build()
    }

    #[test]
    fn test_empty() {
        let s: PackedStringSlice<str, u8> = PackedStringSlice::empty();
        assert_eq!(0, s.len());
        assert!(s.is_empty());
        assert_eq!(Err(0), s.binary_search("anything"));
        assert_eq!(0..0, s.prefix_range(""));
        assert_eq!(0..0, s.prefix_range("x"));
    }

    #[test]
    fn test_single() {
        let s = build(&[("hello", 7)]);
        assert_eq!(1, s.len());
        assert_eq!("hello", s.get(0));
        assert_eq!(7, s.get_tag(0));
        assert_eq!(Ok(0), s.binary_search("hello"));
        assert_eq!(Err(0), s.binary_search("a"));
        assert_eq!(Err(1), s.binary_search("z"));
    }

    #[test]
    fn test_iter_sorted() {
        let s = build(&[("banana", 2), ("apple", 1), ("cherry", 3)]);
        let v: Vec<_> = s.iter_with_tag().collect();
        assert_eq!(
            vec![("apple", 1), ("banana", 2), ("cherry", 3)],
            v,
            "iteration order should be sorted by string"
        );
    }

    #[test]
    fn test_binary_search() {
        let s = build(&[("a", 0), ("b", 0), ("d", 0), ("e", 0)]);
        assert_eq!(Ok(0), s.binary_search("a"));
        assert_eq!(Ok(1), s.binary_search("b"));
        assert_eq!(Err(2), s.binary_search("c"));
        assert_eq!(Ok(2), s.binary_search("d"));
        assert_eq!(Ok(3), s.binary_search("e"));
        assert_eq!(Err(4), s.binary_search("f"));
    }

    #[test]
    fn test_prefix_range() {
        let s = build(&[
            ("a/1", 0),
            ("a/2", 0),
            ("b/1", 0),
            ("b/2", 0),
            ("c/1", 0),
            ("c/2", 0),
        ]);
        let names = |r: Range<usize>| r.map(|i| s.get(i)).collect::<Vec<_>>();
        assert_eq!(vec!["a/1", "a/2"], names(s.prefix_range("a")));
        assert_eq!(vec!["b/1", "b/2"], names(s.prefix_range("b")));
        assert_eq!(vec!["c/1", "c/2"], names(s.prefix_range("c")));
        assert_eq!(Vec::<&str>::new(), names(s.prefix_range("d")));
        assert_eq!(vec!["a/1"], names(s.prefix_range("a/1")));
        let all = names(s.prefix_range(""));
        assert_eq!(6, all.len(), "empty prefix matches all entries");
    }

    #[test]
    fn test_handle_shares_bytes() {
        let s = build(&[("aaa", 1), ("bbbbbb", 2), ("cc", 3)]);
        let h0: PackedStr = s.get_handle(0);
        let h1: PackedStr = s.get_handle(1);
        let h2: PackedStr = s.get_handle(2);
        assert_eq!("aaa", h0.as_str());
        assert_eq!("bbbbbb", h1.as_str());
        assert_eq!("cc", h2.as_str());
        // Drop the slice; handles should still be valid because they share
        // refcount on the bytes buffer.
        drop(s);
        assert_eq!("aaa", h0.as_str());
        assert_eq!("bbbbbb", h1.as_str());
        assert_eq!("cc", h2.as_str());
    }

    #[test]
    fn test_handle_clone_is_cheap() {
        let s = build(&[("hello", 0)]);
        let h: PackedStr = s.get_handle(0);
        let h2 = h.dupe();
        assert_eq!(h.as_str(), h2.as_str());
    }

    #[test]
    fn test_eq_and_hash() {
        use std::collections::hash_map::DefaultHasher;

        let a = build(&[("x", 1), ("y", 2)]);
        let b = build(&[("y", 2), ("x", 1)]);
        let c = build(&[("x", 1), ("y", 3)]);
        assert_eq!(a, b);
        assert_ne!(a, c);

        fn hash<H: Hash>(h: &H) -> u64 {
            let mut s = DefaultHasher::new();
            h.hash(&mut s);
            s.finish()
        }
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn test_unit_tag_is_compact() {
        // Sanity check: with `Tag = ()`, each Entry costs 4 bytes.
        assert_eq!(4, std::mem::size_of::<Entry<()>>());
        // And the slice itself is 16 bytes (two thin pointers).
        assert_eq!(16, std::mem::size_of::<PackedStringSlice<str, ()>>());
    }

    #[test]
    fn test_packed_str_size() {
        assert_eq!(16, std::mem::size_of::<PackedStr<str>>());
    }

    #[test]
    fn test_files_within_style_query() {
        // Mimics `PackageFileListing::files_within` behavior on top of
        // `prefix_range` + manual filter.
        let s = build(&[("a/1", 0), ("a/1/2", 0), ("aa/2", 0), ("b/1", 0)]);
        let within = |dir: &str| {
            let len = dir.len();
            s.prefix_range(dir)
                .map(|i| s.get(i))
                .filter(|x| len == 0 || len == x.len() || x.as_bytes()[len] == b'/')
                .collect::<Vec<_>>()
        };
        assert_eq!(vec!["aa/2"], within("aa"));
        assert_eq!(vec!["a/1", "a/1/2"], within("a"));
        assert_eq!(vec!["a/1", "a/1/2"], within("a/1"));
        assert_eq!(vec!["b/1"], within("b/1"));
        assert_eq!(vec!["a/1", "a/1/2", "aa/2", "b/1"], within(""));
    }
}
