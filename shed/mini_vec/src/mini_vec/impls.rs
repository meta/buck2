/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Standard trait implementations for [`MiniVec`].
//!
//! These thin glue impls forward to `MiniVec`'s own (correctness-sensitive)
//! API and contain no `unsafe` of their own. Anything that pokes at the
//! packed representation lives in the parent module.

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

use super::IntoIter;
use super::MiniVec;

impl<T> Default for MiniVec<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Deref for MiniVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> DerefMut for MiniVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: fmt::Debug> fmt::Debug for MiniVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Clone> Clone for MiniVec<T> {
    fn clone(&self) -> Self {
        self.iter().cloned().collect()
    }
}

impl<T: PartialEq> PartialEq for MiniVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for MiniVec<T> {}

impl<T: PartialEq> PartialEq<[T]> for MiniVec<T> {
    fn eq(&self, other: &[T]) -> bool {
        self.as_slice() == other
    }
}

impl<T> FromIterator<T> for MiniVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut v = Self::with_capacity(lower);
        v.extend(iter);
        v
    }
}

impl<T> Extend<T> for MiniVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        iter.for_each(|item| self.push(item));
    }
}

impl<'a, T> IntoIterator for &'a MiniVec<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut MiniVec<T> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T> IntoIterator for MiniVec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self)
    }
}

impl<T: Clone> From<&[T]> for MiniVec<T> {
    fn from(s: &[T]) -> Self {
        s.iter().cloned().collect()
    }
}

impl<T> From<MiniVec<T>> for Vec<T> {
    fn from(v: MiniVec<T>) -> Self {
        v.into_vec()
    }
}

impl<T: PartialOrd> PartialOrd for MiniVec<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}

impl<T: Ord> Ord for MiniVec<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<T: Hash> Hash for MiniVec<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<T> AsRef<[T]> for MiniVec<T> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> AsMut<[T]> for MiniVec<T> {
    fn as_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T> Borrow<[T]> for MiniVec<T> {
    fn borrow(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: Allocative> Allocative for MiniVec<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        self.visit_allocation(&mut visitor);
        visitor.exit();
    }
}

impl<T: StrongHash> StrongHash for MiniVec<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        // Delegating to the slice impl ensures consistency with
        // `Borrow<[T]>` and matches how `Vec<T>` hashes.
        self.as_slice().strong_hash(state);
    }
}

impl<T: PagableSerialize> PagableSerialize for MiniVec<T> {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        // Wire-compatible with `Vec<T>` and `[T]`: a `usize` length
        // followed by each element.
        self.as_slice().pagable_serialize(serializer)
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for MiniVec<T> {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        // `Vec<T>::pagable_deserialize` already reads `len + items` and
        // produces a `Vec` with `cap == len`; the conversion through
        // `From<Vec<T>>` reuses that buffer in inline mode.
        Ok(Self::from(Vec::<T>::pagable_deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let v: MiniVec<i32> = MiniVec::default();
        assert_eq!(v.len(), 0);
        assert!(!v.is_extended());
    }

    #[test]
    fn deref_to_slice() {
        let v: MiniVec<i32> = (0..5).collect();
        // Use slice methods directly via `Deref`.
        assert_eq!(v.first(), Some(&0));
        assert_eq!(v.last(), Some(&4));
        assert_eq!(v.len(), 5);
    }

    #[test]
    fn debug_format() {
        let v: MiniVec<u32> = (0..3).collect();
        assert_eq!(format!("{v:?}"), "[0, 1, 2]");
    }

    #[test]
    fn clone_and_eq() {
        let v: MiniVec<String> = (0..10).map(|i| format!("x{i}")).collect();
        let v2 = v.clone();
        assert_eq!(v, v2);
        // Distinct allocations — mutating one doesn't touch the other.
        let mut v2 = v2;
        v2.pop();
        assert_ne!(v, v2);
    }

    fn flame_total(root: &dyn allocative::Allocative) -> usize {
        let mut builder = allocative::FlameGraphBuilder::default();
        builder.visit_root(root);
        let out = builder.finish();
        assert_eq!(out.warnings(), "", "unexpected flame graph size warnings");
        out.flamegraph().total_size()
    }

    #[test]
    fn allocative_smoke() {
        assert_eq!(flame_total(&MiniVec::<u32>::new()), 8);

        let inline: MiniVec<u32> = (0..10).collect();
        assert!(!inline.is_extended());
        assert_eq!(flame_total(&inline), 40 + 8);

        let extended: MiniVec<u32> = (0..1000).collect();
        assert!(extended.is_extended());
        assert_eq!(flame_total(&extended), 4000 + 16 + 8);

        let mut with_excess: MiniVec<u32> = MiniVec::with_capacity(100);
        with_excess.push(1);
        assert_eq!(flame_total(&with_excess), 400 + 16 + 8);
    }

    #[test]
    fn allocative_accounts_for_header_padding_when_over_aligned() {
        // When `T` is more aligned than the 16-byte `Header`, the elements sit
        // behind alignment padding. Both the header and that padding must be
        // accounted for (align 64 here leaves 48 bytes of padding after the
        // header).
        #[derive(Clone, Copy, Allocative)]
        #[repr(align(64))]
        struct Aligned64(#[expect(dead_code)] u64);

        let v: MiniVec<Aligned64> = (0..100).map(Aligned64).collect();
        assert!(v.is_extended());
        assert_eq!(v.capacity(), v.len());

        assert_eq!(flame_total(&v), 6400 + 64 + 8);
    }

    #[test]
    fn strong_hash_matches_slice() {
        // Hashing through `MiniVec` must match hashing the slice directly,
        // for `Borrow<[T]>` consistency.
        let mv: MiniVec<u32> = (0..10).collect();
        let v: Vec<u32> = (0..10).collect();

        let mut h1 = std::hash::DefaultHasher::new();
        mv.strong_hash(&mut h1);
        let mut h2 = std::hash::DefaultHasher::new();
        v.as_slice().strong_hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn pagable_round_trip() {
        use pagable::testing::TestingDeserializer;
        use pagable::testing::TestingSerializer;

        // Bracket the inline/extended boundary (`MAX_INLINE_CAP == 14`).
        for len in [0usize, 1, 13, 14, 15, 1000] {
            let original: MiniVec<u32> = (0..len as u32).collect();

            let mut ser = TestingSerializer::new();
            original.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();

            let mut de = TestingDeserializer::new(&bytes);
            let restored: MiniVec<u32> = MiniVec::pagable_deserialize(&mut de).unwrap();

            assert_eq!(original, restored, "len={len}");
            // Deserialization produces a shrunk vector (`cap == len`).
            assert_eq!(restored.capacity(), restored.len(), "len={len}");
        }
    }

    #[test]
    fn pagable_format_matches_vec() {
        use pagable::testing::TestingSerializer;

        // Wire compatibility: a `MiniVec` and an equivalent `Vec`
        // serialize to the same byte stream.
        let mv: MiniVec<u32> = (0..50).collect();
        let v: Vec<u32> = (0..50).collect();

        let mut ser_mv = TestingSerializer::new();
        mv.pagable_serialize(&mut ser_mv).unwrap();
        let mut ser_v = TestingSerializer::new();
        v.pagable_serialize(&mut ser_v).unwrap();

        assert_eq!(ser_mv.finish(), ser_v.finish());
    }
}
