/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! (Almost) lock-free insertion only hashtable.
//!
//! This module provides raw hashtable, which can be used to implement
//! higher-level hashtables.

use std::mem;
use std::ptr;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering;

#[cfg(feature = "allocative")]
use allocative::Allocative;
#[cfg(feature = "allocative")]
use allocative::Key;
#[cfg(feature = "allocative")]
use allocative::Visitor;
use parking_lot::RwLock;

use crate::atomic_value::AtomicValue;
use crate::cache_line::IsolatedCacheLine;
use crate::fixed_cap;
use crate::fixed_cap::FixedCapTable;
pub use crate::fixed_cap::Iter;

struct CurrentTable<T: AtomicValue> {
    /// Previous tables are kept forever because there's no way to know
    /// when they are no longer in use.
    /// We double the capacity every time we resize,
    /// so previous tables allocation overhead is linear in the current capacity.
    _prev: Option<Box<CurrentTable<T>>>,
    /// Table used for insertions and lookups.
    /// When resizing/resized, this table is stored in the `prev` field,
    /// so all the pointers to the table remain valid.
    table: FixedCapTable<T>,
}

#[cfg(feature = "allocative")]
impl<T: AtomicValue + Allocative> CurrentTable<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>, current: bool) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        {
            let mut visitor =
                visitor.enter_unique(Key::new("_prev"), mem::size_of_val(&self._prev));
            if let Some(prev) = &self._prev {
                prev.visit(&mut visitor, false);
            }
            visitor.exit();
        }
        {
            let mut visitor =
                visitor.enter_unique(Key::new("table"), mem::size_of_val(&self.table));
            self.table.visit(&mut visitor, current);
            visitor.exit();
        }
        visitor.exit();
    }
}

/// (Almost) lock-free insertion only hashtable.
pub struct LockFreeRawTable<T: AtomicValue> {
    /// Shared lock for insertion, exclusive lock for resizing.
    /// No lock for lookup.
    ///
    /// Insertions write this word while every lookup reads `current`, so the
    /// two must not share a cache line.
    write_lock: IsolatedCacheLine<RwLock<()>>,
    /// Current table. Readers can grab the pointer without locking
    /// and pointer remains valid even if the table is resized.
    current: IsolatedCacheLine<AtomicPtr<CurrentTable<T>>>,
}

impl<T: AtomicValue> Drop for LockFreeRawTable<T> {
    fn drop(&mut self) {
        let current = self.current.0.get_mut();
        if !current.is_null() {
            unsafe {
                let mut current = Box::from_raw(*current);
                // Only the current table owns the entries.
                // Previous tables store subset of the same entries, but we must not drop them.
                current.table.drop_entries();
            };
        }
    }
}

impl<T: AtomicValue> Default for LockFreeRawTable<T> {
    #[inline]
    fn default() -> LockFreeRawTable<T> {
        LockFreeRawTable::new()
    }
}

impl<T: AtomicValue> LockFreeRawTable<T> {
    /// Empty table.
    #[inline]
    pub const fn new() -> LockFreeRawTable<T> {
        LockFreeRawTable {
            write_lock: IsolatedCacheLine(RwLock::new(())),
            current: IsolatedCacheLine(AtomicPtr::new(ptr::null_mut())),
        }
    }

    /// Find an entry.
    #[inline]
    pub fn lookup(&self, hash: u64, eq: impl Fn(T::Ref<'_>) -> bool) -> Option<T::Ref<'_>> {
        let current = self.current.0.load(Ordering::Acquire);

        if current.is_null() {
            return None;
        }

        let current = unsafe { &*current };
        current.table.lookup(hash, eq)
    }

    /// Insert an entry.
    ///
    /// If the entry does not exist, the value is inserted
    /// and a pointer to the inserted entry is returned.
    ///
    /// Otherwise the pointer to existing entry along with the given value is returned.
    pub fn insert(
        &self,
        hash: u64,
        mut value: T,
        eq: impl Fn(T::Ref<'_>, T::Ref<'_>) -> bool,
        hash_fn: impl Fn(T::Ref<'_>) -> u64,
    ) -> (T::Ref<'_>, Option<T>) {
        loop {
            // Acquire shared lock.
            let guard = self.write_lock.0.read();

            let current = self.current.0.load(Ordering::Relaxed);

            if current.is_null() {
                // The table is just created. Allocate capacity and start over.
                drop(guard);
                self.resize_if_needed(|v| hash_fn(v));
                continue;
            }

            let current = unsafe { &*current };
            match current.table.insert(hash, value, |a, b| eq(a, b)) {
                Ok((reference, value)) => {
                    drop(guard);
                    // Insert was successful. However, we or other threads
                    // may have exceeded the load factor.
                    // So resize the table if needed to make lookup faster.
                    if current.table.need_resize() {
                        self.resize_if_needed(|v| hash_fn(v));
                    }
                    return (reference, value);
                }
                Err(ret_value) => {
                    drop(guard);
                    // Table is full. Resize the table and try again.
                    self.resize_if_needed(|v| hash_fn(v));
                    value = ret_value;
                }
            }
        }
    }

    #[cold]
    fn resize_if_needed(&self, hash: impl Fn(T::Ref<'_>) -> u64) {
        // Acquire exclusive lock.
        // Readers still read the old table, but new insertions will wait.
        let _guard = self.write_lock.0.write();

        let current_ptr = self.current.0.load(Ordering::Relaxed);

        if current_ptr.is_null() {
            let new_table: FixedCapTable<T> = FixedCapTable::with_capacity(16);
            let new_current = Box::new(CurrentTable {
                _prev: None,
                table: new_table,
            });
            self.current
                .0
                .store(Box::into_raw(new_current), Ordering::Release);
            return;
        }

        let current = unsafe { &*current_ptr };

        if !current.table.need_resize() {
            return;
        }

        let new_cap = current.table.capacity().checked_mul(2).unwrap();
        let mut new_table: FixedCapTable<T> = FixedCapTable::with_capacity(new_cap);

        for entry_ptr in current.table.iter_ptrs() {
            let entry = unsafe { T::deref(entry_ptr) };
            let hash = hash(entry);
            new_table.insert_unique_unchecked(hash, entry_ptr);
        }

        let new_current = Box::new(CurrentTable {
            _prev: Some(unsafe { Box::from_raw(current_ptr) }),
            table: new_table,
        });

        self.current
            .0
            .store(Box::into_raw(new_current), Ordering::Release);
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> Iter<'_, T> {
        let current = self.current.0.load(Ordering::Acquire);

        if current.is_null() {
            return Iter::empty();
        }

        let current = unsafe { &*current };
        current.table.iter()
    }

    /// Number of entries in the table.
    #[inline]
    pub fn len(&self) -> usize {
        let current = self.current.0.load(Ordering::Acquire);

        if current.is_null() {
            return 0;
        }

        let current = unsafe { &*current };
        current.table.len()
    }

    /// Number of entries in the table is zero.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Synchronize against all insert operations.
    ///
    /// Consider two threads and a shared flag variable, like this:
    ///
    /// ```text
    /// # Thread 1              # Thread 2
    /// table.insert(k, v);     store(flag, true, Relaxed);
    /// load(flag, Relaxed);    table.iter() // Alternatively: table.get(k)
    /// ```
    ///
    /// You would like to gurantee that either Thread 1 sees the flag as set or that Thread 2 sees
    /// the inserted item.
    ///
    /// Under the C++ memory model, no strength of ordering on the `flag` operations is enough to
    /// get that guarantee. The only way to get there is to insert a full SeqCst fence in between
    /// both operations in each thread. If you'd like to learn more about this, ask your favorite
    /// LLM about Store Buffering Pairs.
    ///
    /// This API is here to let you use the table's internal synchronization mechanisms to achieve
    /// that guarantee more efficiently. In C++ terms, this operation creates a happens-before edge
    /// with every `insert` operation on this table. As a result, calling this between the two
    /// statements on Thread 2 suffices in the example above.
    ///
    /// Note that this is not particularly fast, and may become even slower in the future. It's
    /// intended for cases where Thread 2 is the coldpath and you care about keeping expensive
    /// synchronization out of Thread 1.
    #[inline]
    pub fn synchronize_with_inserts(&self) {
        drop(self.write_lock.0.write());
    }
}

/// Consuming iterator over entries in a `LockFreeRawTable`.
pub struct IntoIter<T: AtomicValue> {
    /// Previous tables are kept alive for memory deallocation. Their entries are
    /// duplicates of entries in the current table and must NOT be dropped as owned values.
    /// `FixedCapTable` does not drop entries on `Drop` (by design), so keeping
    /// prev tables alive only frees their slot arrays.
    _prev: Option<Box<CurrentTable<T>>>,
    iter: fixed_cap::IntoIter<T>,
}

impl<T: AtomicValue> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

impl<T: AtomicValue> IntoIterator for LockFreeRawTable<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    /// Consume the table and return a consuming iterator over entries.
    ///
    /// Only the current (most recent) table owns entries. Previous tables store
    /// duplicate raw pointers to the same entries, so we must NOT drop entries
    /// from them — they just free their slot arrays when the `_prev` chain drops.
    fn into_iter(mut self) -> IntoIter<T> {
        // Take the pointer and replace with null so `Drop` doesn't double-free.
        let current_ptr = mem::replace(self.current.0.get_mut(), ptr::null_mut());
        if current_ptr.is_null() {
            return IntoIter {
                _prev: None,
                iter: fixed_cap::IntoIter::empty(),
            };
        }
        let current = unsafe { Box::from_raw(current_ptr) };
        let CurrentTable { _prev, table } = *current;
        IntoIter {
            _prev,
            iter: table.into_iter(),
        }
    }
}

#[cfg(feature = "allocative")]
impl<T: AtomicValue + Allocative> Allocative for LockFreeRawTable<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        {
            let mut visitor = visitor.enter_unique(
                allocative::Key::new("current"),
                mem::size_of_val(&self.current),
            );
            let current = self.current.0.load(Ordering::Acquire);
            if !current.is_null() {
                let current = unsafe { &*current };
                current.visit(&mut visitor, true);
            }
            visitor.exit();
        }
        visitor.exit();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::num::NonZeroU32;
    use std::ptr;
    use std::ptr::NonNull;

    use crate::atomic_value::RawPtr;
    use crate::raw::LockFreeRawTable;

    fn hash<T: Hash>(key: T) -> u64 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn hash_fn(key: &u32) -> u64 {
        hash(*key)
    }

    #[test]
    fn test_write_hot_lock_isolated_from_read_hot_current() {
        let write_lock = std::mem::offset_of!(LockFreeRawTable<Box<u32>>, write_lock);
        let current = std::mem::offset_of!(LockFreeRawTable<Box<u32>>, current);
        assert_eq!(
            0,
            write_lock % 128,
            "lock word must start its own 128-byte block"
        );
        assert_eq!(
            0,
            current % 128,
            "current pointer must start its own 128-byte block"
        );
        assert_ne!(
            write_lock / 128,
            current / 128,
            "insertions write the lock word while lookups read `current`; \
             they must not share an effective cache line"
        );
    }

    #[test]
    fn test_simple() {
        let t = LockFreeRawTable::new();
        let v0 = t.insert(hash(1), Box::new(1), |a, b| a == b, hash_fn).0;
        assert_eq!(&1, v0);
        let v1 = t.insert(hash(1), Box::new(1), |a, b| a == b, hash_fn).0;
        assert_eq!(&1, v1);
        assert!(ptr::eq(v0, v1));

        let v2 = t.lookup(hash(1), |a| a == &1).unwrap();
        assert_eq!(&1, v2);
        assert!(ptr::eq(v0, v2));

        assert_eq!(vec![1], t.iter().copied().collect::<Vec<_>>());
    }

    #[test]
    fn test_million() {
        let n = if cfg!(miri) { 1000 } else { 1000000 };
        let t = LockFreeRawTable::new();
        for i in 0..n {
            t.insert(hash(i), Box::new(i), |a, b| a == b, hash_fn);
        }

        for i in 0..n {
            let v = t.lookup(hash(i), |a| a == &i).unwrap();
            assert_eq!(&i, v);
        }

        assert_eq!(n as usize, t.iter().count());
    }

    #[test]
    fn test_u32() {
        let n = if cfg!(miri) { 500 } else { 10000 };
        let t = LockFreeRawTable::new();
        for i in 1..n {
            t.insert(hash(i), NonZeroU32::new(i).unwrap(), |a, b| a == b, hash);
        }

        for i in 1..n {
            let v = t.lookup(hash(i), |a| a.get() == i).unwrap();
            assert_eq!(NonZeroU32::new(i).unwrap(), v);
        }
    }

    #[test]
    fn test_raw_pointers() {
        let t = LockFreeRawTable::new();
        let p = NonNull::<u32>::dangling();
        let p = RawPtr(p);
        let r = t.insert(hash(p.0), p, |a, b| a == b, hash).0;
        assert_eq!(p.0, r);
    }

    #[test]
    fn test_into_iter() {
        let t = LockFreeRawTable::new();
        for i in 0..10 {
            t.insert(hash(i), Box::new(i), |a, b| a == b, hash_fn);
        }

        let mut collect: Vec<u32> = t.into_iter().map(|b| *b).collect();
        collect.sort_unstable();
        assert_eq!((0..10).collect::<Vec<_>>(), collect);
    }

    #[test]
    fn test_into_iter_empty() {
        let t = LockFreeRawTable::<Box<u32>>::new();
        let collect: Vec<Box<u32>> = t.into_iter().collect();
        assert!(collect.is_empty());
    }

    #[cfg(feature = "allocative")]
    #[test]
    fn test_allocative() {
        let table = LockFreeRawTable::<Box<u16>>::new();

        for i in 0..100 {
            let (_r, inserted) = table.insert(hash(i), Box::new(i), |a, b| a == b, |v| hash(v));
            assert!(inserted.is_none());
        }

        let mut builder = allocative::FlameGraphBuilder::default();
        builder.visit_root(&table);
        let _flame_graph = builder.finish_and_write_flame_graph();
        // Proper automated test is possible but hard to maintain.
        // The best option is to print the flame graph and check resulting image.
        // println!("{}", _flame_graph);
        // At the moment of writing it looks like this: https://www.internalfb.com/intern/px/p/2Gb33

        // Alternatively, we can set up golden test.
    }
}
