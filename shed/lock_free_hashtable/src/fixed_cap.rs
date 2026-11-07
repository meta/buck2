/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;
#[cfg(feature = "allocative")]
use std::mem;
#[cfg(feature = "allocative")]
use std::mem::ManuallyDrop;
use std::ptr;
use std::ptr::NonNull;
use std::slice;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[cfg(feature = "allocative")]
use allocative::Allocative;
#[cfg(feature = "allocative")]
use allocative::Visitor;

use crate::atomic_value::AtomicValue;
use crate::cache_line::IsolatedCacheLine;

/// Fixed capacity hashtable.
/// When dropped, it will not drop the entries.
pub(crate) struct FixedCapTable<T: AtomicValue> {
    /// Current list of entries. Null if the entry is missing, otherwise the word of a `T`.
    entries: Box<[AtomicPtr<()>]>,
    /// Number of entries in the table. We always use relaxed operations for this, meaning
    /// that any particular return value can always be wrong in either direction.
    ///
    /// Insertions increment this while every lookup reads the `entries` fat
    /// pointer, so the two must not share a cache line.
    size: IsolatedCacheLine<AtomicUsize>,
    /// The table owns the `T`s behind the entries, so its auto traits are theirs.
    _marker: PhantomData<T>,
}

pub(crate) struct IterPtrs<'a, T: AtomicValue> {
    iter: slice::Iter<'a, AtomicPtr<()>>,
    _marker: PhantomData<T>,
}

impl<T: AtomicValue> Iterator for IterPtrs<'_, T> {
    type Item = NonNull<()>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next() {
                Some(entry) => {
                    if let Some(ptr) = NonNull::new(entry.load(Ordering::Acquire)) {
                        return Some(ptr);
                    }
                }
                None => return None,
            }
        }
    }
}

#[cfg(feature = "allocative")]
impl<T: AtomicValue + Allocative> FixedCapTable<T> {
    pub(crate) fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>, current: bool) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        {
            let mut visitor = visitor.enter_unique(
                allocative::Key::new("entries"),
                mem::size_of_val(&self.entries),
            );
            {
                let mut visitor = visitor.enter(
                    allocative::Key::new("capacity"),
                    mem::size_of_val::<[AtomicPtr<()>]>(&*self.entries),
                );
                // We only visit pointers from current table.
                if current {
                    for ptr in self.iter_ptrs() {
                        let mut visitor = visitor.enter(
                            allocative::Key::new("entry"),
                            mem::size_of::<AtomicPtr<()>>(),
                        );
                        unsafe {
                            // SAFETY: We "forget" the value after the iteration,
                            //   so it is safe to convert it to owned value.
                            let value = T::from_raw(ptr);
                            let value = ManuallyDrop::new(value);
                            if mem::size_of_val(&value) > mem::size_of::<AtomicPtr<()>>() {
                                // Size will be computed incorrectly.
                            }

                            T::visit(&value, &mut visitor);
                        }
                        visitor.exit();
                    }
                }
                visitor.exit();
            }
            visitor.exit();
        }
        visitor.exit();
    }
}

/// Iterator over the entries.
pub struct Iter<'a, T: AtomicValue> {
    iter: IterPtrs<'a, T>,
}

impl<'a, T: AtomicValue> Iter<'a, T> {
    #[inline]
    pub(crate) fn empty() -> Iter<'a, T> {
        Iter {
            iter: IterPtrs {
                iter: [].iter(),
                _marker: PhantomData,
            },
        }
    }
}

impl<'a, T: AtomicValue + 'a> Iterator for Iter<'a, T> {
    type Item = T::Ref<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|ptr| unsafe { T::deref(ptr) })
    }
}

/// Consuming iterator over entries in a `FixedCapTable`.
pub(crate) struct IntoIter<T: AtomicValue> {
    entries: Vec<AtomicPtr<()>>,
    index: usize,
    _marker: PhantomData<T>,
}

impl<T: AtomicValue> IntoIter<T> {
    /// Create an empty consuming iterator for the null-table case.
    pub(crate) fn empty() -> IntoIter<T> {
        IntoIter {
            entries: Vec::new(),
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<T: AtomicValue> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.entries.len() {
            // We own the vec, so `get_mut()` is safe and avoids atomic overhead.
            let raw = *self.entries[self.index].get_mut();
            self.index += 1;
            if let Some(raw) = NonNull::new(raw) {
                return Some(unsafe { T::from_raw(raw) });
            }
        }
        None
    }
}

impl<T: AtomicValue> Drop for IntoIter<T> {
    fn drop(&mut self) {
        // Drop any remaining non-null entries that weren't yielded.
        while self.index < self.entries.len() {
            let raw = *self.entries[self.index].get_mut();
            self.index += 1;
            if let Some(raw) = NonNull::new(raw) {
                unsafe {
                    let _drop = T::from_raw(raw);
                }
            }
        }
    }
}

impl<T: AtomicValue> FixedCapTable<T> {
    pub(crate) fn with_capacity(cap: usize) -> FixedCapTable<T> {
        assert!(cap.is_power_of_two());
        let mut entries = Vec::new();
        entries.resize_with(cap, || AtomicPtr::new(ptr::null_mut()));
        FixedCapTable {
            entries: entries.into_boxed_slice(),
            size: IsolatedCacheLine(AtomicUsize::new(0)),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub(crate) fn need_resize(&self) -> bool {
        self.size.0.load(Ordering::Relaxed) >= self.entries.len() / 8 * 7
    }

    #[inline]
    pub(crate) fn lookup(&self, hash: u64, eq: impl Fn(T::Ref<'_>) -> bool) -> Option<T::Ref<'_>> {
        let mut index = hash as usize & (self.entries.len() - 1);
        for _ in 0..self.entries.len() {
            let Some(entry) = NonNull::new(self.entries[index].load(Ordering::Acquire)) else {
                return None;
            };
            let entry = unsafe { T::deref(entry) };
            if eq(entry) {
                return Some(entry);
            }
            index = (index + 1) & (self.entries.len() - 1);
        }
        None
    }

    pub(crate) fn insert_unique_unchecked(&mut self, hash: u64, value: NonNull<()>) {
        let mut index = hash as usize & (self.entries.len() - 1);
        loop {
            let entry = self.entries[index].get_mut();
            if entry.is_null() {
                *entry = value.as_ptr();
                *self.size.0.get_mut() += 1;
                return;
            }
            index = (index + 1) & (self.entries.len() - 1);
        }
    }

    /// Insert or lookup value.
    ///
    /// If the value is already in the table, it will be returned.
    /// If the capacity is full, the value will be returned as an error.
    /// Otherwise, the value will be inserted and returned.
    pub(crate) fn insert<'a>(
        &self,
        hash: u64,
        value: T,
        eq: impl Fn(T::Ref<'_>, T::Ref<'_>) -> bool,
    ) -> Result<(T::Ref<'a>, Option<T>), T> {
        let value = T::into_raw(value);

        let mut index = hash as usize & (self.entries.len() - 1);

        // We do not know if the table is full already,
        // or becoming full concurrently with us.
        // So we have to stop after `capacity` iterations.
        for _ in 0..self.entries.len() {
            let entry = self.entries[index].load(Ordering::Acquire);
            let entry = if let Some(entry) = NonNull::new(entry) {
                entry
            } else {
                match self.entries[index].compare_exchange(
                    ptr::null_mut(),
                    value.as_ptr(),
                    // If the compare_exchange succeeds, the pointer has been inserted
                    // into the table. This means we need `Release` to ensure that the
                    // data behind the `Box<T>` is visible to other threads.
                    Ordering::Release,
                    // The code below reads the data behind the pointer, this means we
                    // need acquire so that this thread sees any previous changes to that
                    // data.
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.size.0.fetch_add(1, Ordering::Relaxed);
                        return Ok((unsafe { T::deref(value) }, None));
                    }
                    Err(entry) => {
                        // Someone has just inserted the value into the bucket.
                        // SAFETY: the exchange failed, so the slot no longer holds null.
                        unsafe { NonNull::new_unchecked(entry) }
                    }
                }
            };
            if eq(unsafe { T::deref(value) }, unsafe { T::deref(entry) }) {
                return Ok(unsafe { (T::deref(entry), Some(T::from_raw(value))) });
            }
            index = (index + 1) & (self.entries.len() - 1);
        }
        Err(unsafe { T::from_raw(value) })
    }

    /// Iterate over the pointers to the entries.
    #[inline]
    pub(crate) fn iter_ptrs(&self) -> IterPtrs<'_, T> {
        IterPtrs {
            iter: self.entries.iter(),
            _marker: PhantomData,
        }
    }

    /// Iterate over the entries.
    #[inline]
    pub(crate) fn iter(&self) -> Iter<'_, T> {
        Iter {
            iter: self.iter_ptrs(),
        }
    }

    /// Drop all entries.
    pub(crate) unsafe fn drop_entries(&mut self) {
        for entry in self.iter_ptrs() {
            unsafe {
                let _drop = T::from_raw(entry);
            }
        }
    }

    /// Consume the table and return a consuming iterator over entries.
    /// After calling this, the caller is responsible for dropping all yielded entries.
    /// Entries that are not yielded (e.g. if the iterator is dropped early) are
    /// dropped by the `IntoIter::drop` implementation.
    pub(crate) fn into_iter(self) -> IntoIter<T> {
        // Prevent FixedCapTable's implicit drop from running (it doesn't drop entries,
        // but we're taking ownership of the entries vec).
        let entries: Vec<AtomicPtr<()>> = self.entries.into_vec();
        IntoIter {
            entries,
            index: 0,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.size.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use crate::fixed_cap::FixedCapTable;

    #[test]
    fn test_write_hot_size_isolated_from_read_hot_entries() {
        let entries = std::mem::offset_of!(FixedCapTable<Box<u32>>, entries);
        let size = std::mem::offset_of!(FixedCapTable<Box<u32>>, size);
        assert_ne!(
            entries / 128,
            size / 128,
            "insertions increment `size` while lookups read the `entries` fat \
             pointer; they must not share an effective cache line"
        );
    }
}
