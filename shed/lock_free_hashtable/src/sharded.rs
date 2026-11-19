/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sharded (almost) lock-free hashtable.

use std::marker;

#[cfg(feature = "allocative")]
use allocative::Allocative;

use crate::atomic_value::AtomicValue;
use crate::raw;
use crate::raw::LockFreeRawTable;

// Insertions update each shard's lock word. Keep independently hot shard headers on separate
// effective cache lines; 128 bytes covers 64-byte lines plus adjacent-line prefetching.
#[repr(align(128))]
#[cfg_attr(feature = "allocative", derive(Allocative))]
struct CachePaddedLockFreeRawTable<T: AtomicValue>(LockFreeRawTable<T>);

/// Lock-free hashtable sharded by key hash.
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct ShardedLockFreeRawTable<T: AtomicValue, const SHARDS: usize> {
    shards: [CachePaddedLockFreeRawTable<T>; SHARDS],
}

impl<T: AtomicValue, const SHARDS: usize> Default for ShardedLockFreeRawTable<T, SHARDS> {
    fn default() -> Self {
        ShardedLockFreeRawTable::new()
    }
}

impl<T: AtomicValue, const SHARDS: usize> ShardedLockFreeRawTable<T, SHARDS> {
    /// Checked when a table is created: a shard count that is not a power of two (zero
    /// included) does not compile.
    ///
    /// ```compile_fail
    /// let _ = lock_free_hashtable::sharded::ShardedLockFreeRawTable::<Box<u32>, 3>::new();
    /// ```
    const ASSERTIONS: () = assert!(SHARDS.is_power_of_two());

    const SHARD_BITS: usize = SHARDS.trailing_zeros() as usize;

    /// Create a new empty hashtable.
    pub const fn new() -> ShardedLockFreeRawTable<T, SHARDS> {
        let () = Self::ASSERTIONS;

        struct Empty<A>(marker::PhantomData<A>);
        impl<A: AtomicValue> Empty<A> {
            #[allow(clippy::declare_interior_mutable_const)]
            const EMPTY: LockFreeRawTable<A> = LockFreeRawTable::new();
        }

        ShardedLockFreeRawTable {
            shards: [const { CachePaddedLockFreeRawTable(Empty::<T>::EMPTY) }; SHARDS],
        }
    }

    #[inline]
    fn table_for_hash(&self, hash: u64) -> &LockFreeRawTable<T> {
        // `LockFreeRawTable` uses low bits of hash, so we use high bits to select a shard.
        let shard_index = if Self::SHARD_BITS == 0 {
            0
        } else {
            (hash >> (64 - Self::SHARD_BITS)) as usize
        };
        &self.shards[shard_index].0
    }

    /// Find an entry.
    #[inline]
    pub fn lookup(&self, hash: u64, eq: impl Fn(T::Ref<'_>) -> bool) -> Option<T::Ref<'_>> {
        self.table_for_hash(hash).lookup(hash, eq)
    }

    /// Insert an entry.
    ///
    /// If the entry does not exist, the value is inserted
    /// and a pointer to the inserted entry is returned.
    ///
    /// Otherwise the pointer to existing entry along with the given value is returned.
    #[inline]
    pub fn insert(
        &self,
        hash: u64,
        value: T,
        eq: impl Fn(T::Ref<'_>, T::Ref<'_>) -> bool,
        hash_fn: impl Fn(T::Ref<'_>) -> u64,
    ) -> (T::Ref<'_>, Option<T>) {
        self.table_for_hash(hash).insert(hash, value, eq, hash_fn)
    }

    /// Iterate entries in unspecified order.
    #[inline]
    pub fn iter(&self) -> Iter<'_, T, SHARDS> {
        Iter {
            table: self,
            shard: 0,
            iter: self.shards[0].0.iter(),
        }
    }

    /// Number of entries in the table.
    #[inline]
    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.0.len()).sum()
    }

    /// Number of entries in the table is zero.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Synchronize against all insert operations.
    ///
    /// See [`LockFreeRawTable::synchronize_with_inserts`].
    #[inline]
    pub fn synchronize_with_inserts(&self) {
        for shard in &self.shards {
            shard.0.synchronize_with_inserts();
        }
    }
}

/// Iterator over all entries in sharded raw table.
pub struct Iter<'a, T: AtomicValue, const SHARDS: usize> {
    table: &'a ShardedLockFreeRawTable<T, SHARDS>,
    /// Current iterator shard index.
    shard: usize,
    iter: crate::raw::Iter<'a, T>,
}

impl<'a, T: AtomicValue + 'a, const SHARDS: usize> Iterator for Iter<'a, T, SHARDS> {
    type Item = T::Ref<'a>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(next) = self.iter.next() {
                return Some(next);
            }
            if self.shard >= SHARDS - 1 {
                debug_assert!(self.shard == SHARDS - 1);
                return None;
            }
            self.shard += 1;
            self.iter = self.table.shards[self.shard].0.iter();
        }
    }
}

impl<T: AtomicValue, const SHARDS: usize> IntoIterator for ShardedLockFreeRawTable<T, SHARDS> {
    type Item = T;
    type IntoIter = IntoIter<T, SHARDS>;

    fn into_iter(self) -> IntoIter<T, SHARDS> {
        let mut shards = self.shards.into_iter();
        // SAFETY: SHARDS is guaranteed >= 1 by the power-of-two const assertion.
        let current = shards.next().expect("SHARDS >= 1").0.into_iter();
        IntoIter { shards, current }
    }
}

/// Consuming iterator over all entries in a sharded raw table.
pub struct IntoIter<T: AtomicValue, const SHARDS: usize> {
    shards: std::array::IntoIter<CachePaddedLockFreeRawTable<T>, SHARDS>,
    current: raw::IntoIter<T>,
}

impl<T: AtomicValue, const SHARDS: usize> Iterator for IntoIter<T, SHARDS> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(next) = self.current.next() {
                return Some(next);
            }
            self.current = self.shards.next()?.0.into_iter();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::mem::align_of;

    use crate::sharded::CachePaddedLockFreeRawTable;
    use crate::sharded::ShardedLockFreeRawTable;

    #[test]
    fn test_shards_are_cache_padded() {
        assert_eq!(
            128,
            align_of::<CachePaddedLockFreeRawTable<Box<u32>>>(),
            "shard headers must not share effective cache lines",
        );
    }

    /// With a single shard every hash selects shard 0.
    #[test]
    fn test_one_shard() {
        let table = ShardedLockFreeRawTable::<Box<u32>, 1>::new();
        for (hash, value) in [(0, 1), (1, 2), (u64::MAX, 3)] {
            table.insert(hash, Box::new(value), |a, b| a == b, |_| hash);
            assert_eq!(Some(&value), table.lookup(hash, |v| *v == value));
        }
        assert_eq!(3, table.iter().count());
    }

    #[test]
    fn test_shard_bits() {
        assert_eq!(0, ShardedLockFreeRawTable::<Box<u32>, 1>::SHARD_BITS);
        assert_eq!(1, ShardedLockFreeRawTable::<Box<u32>, 2>::SHARD_BITS);
        assert_eq!(2, ShardedLockFreeRawTable::<Box<u32>, 4>::SHARD_BITS);
        assert_eq!(3, ShardedLockFreeRawTable::<Box<u32>, 8>::SHARD_BITS);
        assert_eq!(4, ShardedLockFreeRawTable::<Box<u32>, 16>::SHARD_BITS);
    }

    fn hash(key: u32) -> u64 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn hash_fn(key: &u32) -> u64 {
        hash(*key)
    }

    #[test]
    fn test_iter() {
        let table = ShardedLockFreeRawTable::<Box<u32>, 8>::new();
        let mut expected = Vec::new();
        for i in 0..1000 {
            table.insert(hash(i), Box::new(i), |a, b| a == b, hash_fn);
            expected.push(i);
        }

        let mut collect = HashSet::new();
        for i in table.iter() {
            let inserted = collect.insert(*i);
            assert!(inserted);
        }

        let mut collect = Vec::from_iter(collect);
        collect.sort_unstable();
        assert_eq!(expected, collect);
    }

    #[test]
    fn test_into_iter() {
        let table = ShardedLockFreeRawTable::<Box<u32>, 8>::new();
        for i in 0..10 {
            table.insert(hash(i), Box::new(i), |a, b| a == b, hash_fn);
        }

        let mut collect: Vec<u32> = table.into_iter().map(|b| *b).collect();
        collect.sort_unstable();
        assert_eq!((0..10).collect::<Vec<_>>(), collect);
    }
}
