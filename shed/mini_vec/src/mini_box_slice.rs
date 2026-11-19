/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides [`MiniBoxSlice`].

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;
use std::ops::DerefMut;
use std::slice;

use allocative::Allocative;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use strong_hash::StrongHash;

use crate::IntoIter;
use crate::MiniVec;

/// A `Box<[T]>`-like sequence whose stack footprint is a single machine word.
///
/// This is a newtype wrapper around `MiniVec` that has the added invariant that length equals
/// capacity. Conversion to `MiniVec` is trivial; conversion back may require a `shrink_to_fit`
/// operation.
///
/// Construct one by converting from `Vec<T>`, `Box<[T]>`, `MiniVec<T>`, or any iterator
/// (`.collect()`).
pub struct MiniBoxSlice<T>(MiniVec<T>);

impl<T> MiniBoxSlice<T> {
    /// An empty boxed slice. Does not allocate.
    #[inline]
    pub const fn new() -> Self {
        Self(MiniVec::new())
    }

    /// Number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` iff the slice is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrow the elements as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        self.0.as_slice()
    }

    /// Mutably borrow the elements as a slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.0.as_mut_slice()
    }

    /// Pointer to the first element.
    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.0.as_ptr()
    }

    /// Iterator over `&T`.
    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.0.iter()
    }

    /// `true` iff length lives in the heap header rather than the packed
    /// pointer. Surfaced for tests/diagnostics; ordinary callers should
    /// not need it.
    #[inline]
    pub fn is_extended(&self) -> bool {
        self.0.is_extended()
    }

    /// Convert into a standard `Vec<T>`.
    ///
    /// Reuses the underlying allocation in the inline case, exactly like
    /// [`MiniVec::into_vec`]. Because of the `len == cap` invariant, the
    /// returned `Vec` has `capacity() == len()`.
    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        self.0.into_vec()
    }

    /// Convert into a `MiniVec<T>`. Free; just unwraps the newtype. The
    /// returned `MiniVec` is already shrunk (`len == cap`) but the user
    /// is free to grow it again.
    #[inline]
    pub fn into_mini_vec(self) -> MiniVec<T> {
        self.0
    }
}

impl<T> Default for MiniBoxSlice<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Deref for MiniBoxSlice<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        self.0.as_slice()
    }
}

impl<T> DerefMut for MiniBoxSlice<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut_slice()
    }
}

impl<T> AsRef<[T]> for MiniBoxSlice<T> {
    #[inline]
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> Borrow<[T]> for MiniBoxSlice<T> {
    #[inline]
    fn borrow(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: fmt::Debug> fmt::Debug for MiniBoxSlice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Clone> Clone for MiniBoxSlice<T> {
    fn clone(&self) -> Self {
        // `MiniVec::clone` collects via `with_capacity(len)` and pushes
        // exactly `len` items, so the result already has `cap == len`.
        // Wrap directly to avoid an unnecessary `shrink_to_fit` no-op.
        Self(self.0.clone())
    }
}

impl<T: PartialEq> PartialEq for MiniBoxSlice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for MiniBoxSlice<T> {}

impl<T: PartialEq> PartialEq<[T]> for MiniBoxSlice<T> {
    fn eq(&self, other: &[T]) -> bool {
        self.as_slice() == other
    }
}

impl<T: PartialOrd> PartialOrd for MiniBoxSlice<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}

impl<T: Ord> Ord for MiniBoxSlice<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<T: Hash> Hash for MiniBoxSlice<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<T> FromIterator<T> for MiniBoxSlice<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        // Build via MiniVec, then shrink. MiniVec's `from_iter` may
        // overshoot capacity if the iterator's `size_hint` is loose, so
        // delegating to `From<MiniVec>` (which calls `shrink_to_fit`) is
        // what enforces the invariant.
        Self::from(MiniVec::from_iter(iter))
    }
}

impl<'a, T> IntoIterator for &'a MiniBoxSlice<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> IntoIterator for MiniBoxSlice<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T> From<MiniVec<T>> for MiniBoxSlice<T> {
    fn from(mut mv: MiniVec<T>) -> Self {
        mv.shrink_to_fit();
        Self(mv)
    }
}

impl<T> From<MiniBoxSlice<T>> for MiniVec<T> {
    #[inline]
    fn from(bs: MiniBoxSlice<T>) -> Self {
        bs.0
    }
}

impl<T> From<Vec<T>> for MiniBoxSlice<T> {
    fn from(vec: Vec<T>) -> Self {
        Self::from(MiniVec::from(vec))
    }
}

impl<T> From<MiniBoxSlice<T>> for Vec<T> {
    fn from(bs: MiniBoxSlice<T>) -> Self {
        bs.into_vec()
    }
}

impl<T> From<Box<[T]>> for MiniBoxSlice<T> {
    fn from(b: Box<[T]>) -> Self {
        // `Box::into_vec` reuses the buffer (no copy) and returns a `Vec`
        // with `cap == len`, so the conversion through `MiniVec` adopts
        // the buffer in inline mode without reallocating.
        Self::from(b.into_vec())
    }
}

impl<T> From<MiniBoxSlice<T>> for Box<[T]> {
    fn from(bs: MiniBoxSlice<T>) -> Self {
        // After `into_vec` the `Vec` already has `cap == len` (either
        // because we transferred from inline mode unchanged, or because
        // the extended-mode copy used `Vec::with_capacity(len)`), so
        // `into_boxed_slice` does not reallocate.
        bs.into_vec().into_boxed_slice()
    }
}

impl<T: Clone> From<&[T]> for MiniBoxSlice<T> {
    fn from(s: &[T]) -> Self {
        // `to_vec` produces a `Vec` with `cap == len`; the conversion
        // through `MiniVec` then takes the buffer directly when inline.
        Self::from(s.to_vec())
    }
}

impl<T: Allocative> Allocative for MiniBoxSlice<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        self.0.visit_allocation(&mut visitor);
        visitor.exit();
    }
}

impl<T: StrongHash> StrongHash for MiniBoxSlice<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        // Delegate to the slice impl, which mixes in `len` followed by
        // each element. Hashing through `Borrow<[T]>` consistency means
        // a `MiniBoxSlice` and an equivalent `Vec<T>` / `Box<[T]>` /
        // `[T]` all hash the same.
        self.as_slice().strong_hash(state);
    }
}

impl<T: PagableSerialize> PagableSerialize for MiniBoxSlice<T> {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        // Wire-compatible with `Vec<T>` and `[T]`: a `usize` length
        // followed by each element.
        self.as_slice().pagable_serialize(serializer)
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for MiniBoxSlice<T> {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        // Round-tripping through `Vec<T>` keeps the deserialization logic
        // in one place; the subsequent `From<Vec<T>>` reuses the buffer
        // when it fits inline.
        Ok(Self::from(Vec::<T>::pagable_deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn size_is_one_pointer() {
        // The newtype must not introduce any overhead vs. `MiniVec` — one word
        // when packing, two without it. Comparing against `MiniVec` keeps this
        // correct under either representation.
        assert_eq!(size_of::<MiniBoxSlice<u8>>(), size_of::<MiniVec<u8>>());
        assert_eq!(size_of::<MiniBoxSlice<u64>>(), size_of::<MiniVec<u64>>());
        assert_eq!(
            size_of::<MiniBoxSlice<String>>(),
            size_of::<MiniVec<String>>()
        );
    }

    #[test]
    fn empty_does_not_allocate() {
        let bs: MiniBoxSlice<u32> = MiniBoxSlice::new();
        assert_eq!(bs.len(), 0);
        assert!(bs.is_empty());
        assert!(!bs.is_extended());
    }

    #[test]
    fn from_vec_inline() {
        let v: Vec<u32> = (0..10).collect();
        let bs = MiniBoxSlice::from(v);
        assert_eq!(bs.len(), 10);
        assert!(!bs.is_extended());
        assert!(bs.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn from_vec_extended() {
        let v: Vec<u32> = (0..1000).collect();
        let bs = MiniBoxSlice::from(v);
        assert_eq!(bs.len(), 1000);
        assert!(bs.is_extended());
        assert!(bs.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn from_vec_with_excess_capacity_shrinks() {
        // The Vec has 1000 capacity but only 2 elements — after conversion
        // the resulting `MiniBoxSlice` must hold exactly 2 elements
        // worth of allocation (i.e. `cap == len`). We observe this by
        // round-tripping through `Vec`, whose `capacity()` we can read.
        let mut v: Vec<u32> = Vec::with_capacity(1000);
        v.push(1);
        v.push(2);
        let bs = MiniBoxSlice::from(v);
        assert_eq!(bs.len(), 2);
        let back: Vec<u32> = bs.into();
        assert_eq!(back.capacity(), back.len(), "boxed slice must be shrunk");
        assert_eq!(back.as_slice(), &[1, 2]);
    }

    #[test]
    fn from_mini_vec_with_excess_capacity_shrinks() {
        // Same idea but starting from a `MiniVec` with unused capacity
        // (extended mode), to exercise the demote-to-inline path.
        let mut mv: MiniVec<u32> = MiniVec::with_capacity(1000);
        mv.push(1);
        mv.push(2);
        assert!(mv.is_extended());
        let bs: MiniBoxSlice<u32> = mv.into();
        assert_eq!(bs.len(), 2);
        // After shrinking 2 elements, we should fit inline again.
        assert!(!bs.is_extended());
        assert_eq!(bs.as_slice(), &[1, 2]);
    }

    #[test]
    fn round_trip_box_slice() {
        let original: Box<[u32]> = vec![1, 2, 3, 4, 5].into_boxed_slice();
        let bs: MiniBoxSlice<u32> = original.clone().into();
        assert_eq!(bs.as_slice(), &*original);
        let back: Box<[u32]> = bs.into();
        assert_eq!(back, original);
    }

    #[test]
    fn round_trip_mini_vec() {
        let mv: MiniVec<u32> = (0..100).collect();
        let bs: MiniBoxSlice<u32> = mv.into();
        assert_eq!(bs.len(), 100);
        let back: MiniVec<u32> = bs.into();
        assert_eq!(back.len(), 100);
        // The MiniVec we get back is shrunk.
        assert_eq!(back.capacity(), 100);
    }

    #[test]
    fn from_iter_collects() {
        let bs: MiniBoxSlice<i32> = (0..10).collect();
        assert_eq!(bs.iter().sum::<i32>(), 45);
        // Collected slice is shrunk.
        let v: Vec<i32> = bs.into();
        assert_eq!(v.capacity(), 10);
    }

    #[test]
    fn deref_to_slice_methods() {
        let bs: MiniBoxSlice<i32> = (0..5).collect();
        // Use slice methods through `Deref`.
        assert_eq!(bs.first(), Some(&0));
        assert_eq!(bs.last(), Some(&4));
        assert_eq!(bs[2], 2);
        assert_eq!(&bs[1..3], &[1, 2]);
    }

    #[test]
    fn debug_format() {
        let bs: MiniBoxSlice<u32> = (0..3).collect();
        assert_eq!(format!("{bs:?}"), "[0, 1, 2]");
    }

    #[test]
    fn clone_eq() {
        let bs: MiniBoxSlice<String> = (0..5).map(|i| i.to_string()).collect();
        let bs2 = bs.clone();
        assert_eq!(bs, bs2);
        // Cloning preserves the shrunk invariant.
        let v: Vec<String> = bs2.into();
        assert_eq!(v.capacity(), 5);
    }

    #[test]
    fn ord_and_hash() {
        let a: MiniBoxSlice<i32> = vec![1, 2, 3].into();
        let b: MiniBoxSlice<i32> = vec![1, 2, 4].into();
        assert!(a < b);

        let mut map: HashMap<MiniBoxSlice<i32>, &str> = HashMap::new();
        map.insert(a.clone(), "first");
        map.insert(b.clone(), "second");
        assert_eq!(map.get(&a), Some(&"first"));
        assert_eq!(map.get(&b), Some(&"second"));
    }

    #[test]
    fn drop_runs_inline() {
        let r = Rc::new(());
        let bs: MiniBoxSlice<Rc<()>> = (0..10).map(|_| r.clone()).collect();
        assert!(!bs.is_extended());
        assert_eq!(Rc::strong_count(&r), 11);
        drop(bs);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn drop_runs_extended() {
        let r = Rc::new(());
        let bs: MiniBoxSlice<Rc<()>> = (0..1000).map(|_| r.clone()).collect();
        assert!(bs.is_extended());
        assert_eq!(Rc::strong_count(&r), 1001);
        drop(bs);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn from_slice_clone() {
        let s: &[i32] = &[1, 2, 3];
        let bs: MiniBoxSlice<i32> = s.into();
        assert_eq!(bs.as_slice(), s);
    }

    #[test]
    fn from_empty_vec() {
        let v: Vec<i32> = Vec::new();
        let bs: MiniBoxSlice<i32> = v.into();
        assert_eq!(bs.len(), 0);
    }

    #[test]
    fn into_iter_owned() {
        let bs: MiniBoxSlice<String> = vec!["a".into(), "b".into(), "c".into()].into();
        let collected: Vec<String> = bs.into_iter().collect();
        assert_eq!(collected, vec!["a", "b", "c"]);
    }

    #[test]
    fn drop_correctness_on_round_trip() {
        // Make sure no element gets double-dropped or leaked across the
        // various conversions.
        let r = Rc::new(());
        let bs: MiniBoxSlice<Rc<()>> = (0..100).map(|_| r.clone()).collect();
        assert_eq!(Rc::strong_count(&r), 101);
        let mv: MiniVec<Rc<()>> = bs.into();
        assert_eq!(Rc::strong_count(&r), 101);
        let v: Vec<Rc<()>> = mv.into();
        assert_eq!(Rc::strong_count(&r), 101);
        let bs: MiniBoxSlice<Rc<()>> = v.into();
        assert_eq!(Rc::strong_count(&r), 101);
        drop(bs);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn shrink_invariant_via_drop_count() {
        // The `len == cap` invariant means dropping a `MiniBoxSlice`
        // drops exactly `len` elements, no more.
        struct DropCounter<'a>(&'a Cell<usize>);
        impl Drop for DropCounter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let count = Cell::new(0);
        {
            // Source `Vec` has excess capacity (300 cap, 200 len). The
            // boxed slice must drop 200 elements, not 300.
            let mut src: Vec<DropCounter> = Vec::with_capacity(300);
            for _ in 0..200 {
                src.push(DropCounter(&count));
            }
            let bs: MiniBoxSlice<DropCounter> = src.into();
            assert_eq!(bs.len(), 200);
        }
        assert_eq!(count.get(), 200);
    }

    #[test]
    fn allocative_smoke() {
        // Just exercise the visitor without panicking; a regression in
        // the visitor structure would surface as a panic from
        // `Visitor::enter_*` mismatch.
        let bs: MiniBoxSlice<u32> = (0..10).collect();
        let mut builder = allocative::FlameGraphBuilder::default();
        builder.visit_root(&bs);
        drop(builder.finish_and_write_flame_graph());

        let bs_ext: MiniBoxSlice<u32> = (0..1000).collect();
        assert!(bs_ext.is_extended());
        let mut builder = allocative::FlameGraphBuilder::default();
        builder.visit_root(&bs_ext);
        drop(builder.finish_and_write_flame_graph());

        let empty: MiniBoxSlice<u32> = MiniBoxSlice::new();
        let mut builder = allocative::FlameGraphBuilder::default();
        builder.visit_root(&empty);
        drop(builder.finish_and_write_flame_graph());
    }

    #[test]
    fn strong_hash_matches_slice() {
        // Hashing through `MiniBoxSlice` must produce the same digest as
        // hashing the equivalent `&[T]` directly — required for
        // `Borrow<[T]>` consistency with strong-hash-keyed maps.
        use std::hash::DefaultHasher;
        let bs: MiniBoxSlice<u32> = (0..10).collect();
        let v: Vec<u32> = (0..10).collect();

        let mut h1 = DefaultHasher::new();
        bs.strong_hash(&mut h1);
        let mut h2 = DefaultHasher::new();
        v.as_slice().strong_hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn pagable_round_trip() {
        use pagable::testing::TestingDeserializer;
        use pagable::testing::TestingSerializer;

        // Bracket the inline/extended boundary (`MAX_INLINE_CAP == 14`).
        for len in [0usize, 1, 13, 14, 15, 1000] {
            let original: MiniBoxSlice<u32> = (0..len as u32).collect();

            let mut ser = TestingSerializer::new();
            original.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();

            let mut de = TestingDeserializer::new(&bytes);
            let restored: MiniBoxSlice<u32> = MiniBoxSlice::pagable_deserialize(&mut de).unwrap();

            assert_eq!(original, restored, "len={len}");
        }
    }

    #[test]
    fn pagable_format_matches_vec() {
        use pagable::testing::TestingDeserializer;
        use pagable::testing::TestingSerializer;

        // Wire compatibility: a `MiniBoxSlice` and an equivalent `Vec`
        // should produce identical byte streams.
        let bs: MiniBoxSlice<u32> = (0..50).collect();
        let v: Vec<u32> = (0..50).collect();

        let mut ser_bs = TestingSerializer::new();
        bs.pagable_serialize(&mut ser_bs).unwrap();
        let mut ser_v = TestingSerializer::new();
        v.pagable_serialize(&mut ser_v).unwrap();

        assert_eq!(ser_bs.finish(), ser_v.finish());

        // And we can deserialize each into the other.
        let mut ser = TestingSerializer::new();
        v.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        let mut de = TestingDeserializer::new(&bytes);
        let from_vec: MiniBoxSlice<u32> = MiniBoxSlice::pagable_deserialize(&mut de).unwrap();
        assert_eq!(from_vec.as_slice(), v.as_slice());
    }
}
