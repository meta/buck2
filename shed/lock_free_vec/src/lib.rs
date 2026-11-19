/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Lock-free vector-like data structure.

#![deny(missing_docs)]

use std::cell::UnsafeCell;
use std::cmp;
use std::mem;
use std::mem::MaybeUninit;
use std::ptr;
use std::slice;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use allocative::Visitor;

/// Compute the number of buckets needed for a given capacity.
/// The number of bucket is `log2(capacity) - 3`.
#[inline]
pub const fn buckets_for_max_capacity(max_capacity: usize) -> usize {
    assert!(max_capacity.is_power_of_two());
    assert!(max_capacity >= 16);
    max_capacity.trailing_zeros() as usize - (FIRST_BUCKET_CAP_EXP as usize - 1)
}

/// `Box<[MaybeUninit<T>]>`.
struct BucketAlloc<T> {
    data: *mut T,
    cap: usize,
}

impl<T> BucketAlloc<T> {
    #[inline]
    fn new(cap: usize) -> BucketAlloc<T> {
        debug_assert!(cap.is_power_of_two());
        let mut data = Vec::with_capacity(cap);
        data.resize_with(cap, MaybeUninit::uninit);
        let data = data.into_boxed_slice();
        let data: *mut [MaybeUninit<T>] = Box::into_raw(data);
        let data: *mut T = data as *mut T;
        BucketAlloc { data, cap }
    }

    #[inline]
    unsafe fn drop_data(&self, len: usize) {
        assert!(len <= self.cap);
        let data: *mut [T] = std::ptr::slice_from_raw_parts_mut(self.data, len);
        unsafe { ptr::drop_in_place(data) };
    }

    #[inline]
    unsafe fn from_raw_parts(data: *mut T, cap: usize) -> BucketAlloc<T> {
        debug_assert!(cap.is_power_of_two());
        BucketAlloc { data, cap }
    }
}

/// Deallocate.
impl<T> Drop for BucketAlloc<T> {
    fn drop(&mut self) {
        unsafe {
            let data: *mut MaybeUninit<T> = self.data as *mut MaybeUninit<T>;
            let data: *mut [MaybeUninit<T>] = std::ptr::slice_from_raw_parts_mut(data, self.cap);
            let data: Box<[MaybeUninit<T>]> = Box::from_raw(data);
            drop(data);
        }
    }
}

const LOCKED_FLAG: usize = 1 << (mem::size_of::<usize>() * 8 - 1);
/// `log2` of first bucket capacity.
const FIRST_BUCKET_CAP_EXP: u32 = 4;
/// 16.
const FIRST_BUCKET_CAP: usize = 1 << FIRST_BUCKET_CAP_EXP;

/// Maximum number of buckets.
pub const MAX_BUCKETS: usize = mem::size_of::<usize>() * 8 - FIRST_BUCKET_CAP_EXP as usize;

/// Data structure with push and get operations.
///
/// * `get` is wait-free and no locks.
/// * `push` is single-threaded.
///
/// Both operations are `O(1)`, hence the `Vec` part in name.
/// But data is not stored in contiguous memory.
// This can be parameterized by capacity when `feature(generic_const_exprs)` is stable.
// https://github.com/rust-lang/rust/issues/76560
pub struct LockFreeVec<T, const BUCKETS: usize = MAX_BUCKETS> {
    /// Buckets containing the actual data.
    /// ```ignore
    /// [0..15] // first is special
    /// [16..31]
    /// [32..63]
    /// [64..127]
    /// [128..255]
    /// [256..511]
    /// ...
    /// ```
    /// The number of initialized fields in this array is given by the size. If, for
    /// example, the size is 100 then the first four fields of this array are
    /// initialized. Those fields can then also be read without need for any
    /// synchronization.
    buckets: [UnsafeCell<*mut T>; BUCKETS],
    /// Size, and high bit is locked flag.
    /// Operations are synchronized on this field.
    size: AtomicUsize,
}

impl<T, const BUCKETS: usize> Default for LockFreeVec<T, BUCKETS> {
    #[inline]
    fn default() -> LockFreeVec<T, BUCKETS> {
        LockFreeVec::new()
    }
}

unsafe impl<T: Sync + Send, const BUCKETS: usize> Send for LockFreeVec<T, BUCKETS> {}
unsafe impl<T: Sync + Send, const BUCKETS: usize> Sync for LockFreeVec<T, BUCKETS> {}

impl<T, const BUCKETS: usize> LockFreeVec<T, BUCKETS> {
    const _ASSERTS: () = {
        assert!(BUCKETS > 0);
        assert!(BUCKETS <= MAX_BUCKETS);
        assert!(Self::max_capacity() < LOCKED_FLAG);
    };

    /// Empty.
    #[inline]
    pub const fn new() -> LockFreeVec<T, BUCKETS> {
        LockFreeVec {
            // SAFETY: we want zeros.
            buckets: unsafe { MaybeUninit::zeroed().assume_init() },
            size: AtomicUsize::new(0),
        }
    }

    /// Maximum capacity.
    #[inline]
    pub const fn max_capacity() -> usize {
        // Does not overflow because static assertions.
        1 << (BUCKETS + FIRST_BUCKET_CAP_EXP as usize - 1)
    }

    /// Capacity of a bucket given its index.
    #[inline]
    fn bucket_capacity(bucket: usize) -> usize {
        debug_assert!(bucket < BUCKETS);
        cmp::max(
            FIRST_BUCKET_CAP,
            1 << (bucket + FIRST_BUCKET_CAP_EXP as usize - 1),
        )
    }

    /// Unpack index into bucket index and offset in bucket.
    #[inline]
    fn unpack_index(index: usize) -> (usize, usize) {
        debug_assert!(index < Self::max_capacity());
        let bucket =
            mem::size_of::<usize>() * 8 - (index >> FIRST_BUCKET_CAP_EXP).leading_zeros() as usize;
        let index_in_bucket = index & (Self::bucket_capacity(bucket) - 1);
        (bucket, index_in_bucket)
    }

    /// Number of elements.
    ///
    /// This operation has acquire semantics.
    #[inline]
    pub fn len(&self) -> usize {
        // We want acquire because the returned value is to be used in `push_at`.
        self.size.load(Ordering::Acquire) & !LOCKED_FLAG
    }

    /// Is empty?
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get element.
    ///
    /// Wait-free, no locks.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&T> {
        let size = self.len();
        if index >= size {
            return None;
        }
        let (bucket, index_in_bucket) = Self::unpack_index(index);
        let bucket: *mut T = unsafe { *self.buckets[bucket].get() };
        Some(unsafe { &*bucket.add(index_in_bucket) })
    }

    #[inline]
    unsafe fn init_bucket(&self, bucket: usize) -> *mut T {
        let bucket_ptr_ptr: *mut *mut T = self.buckets[bucket].get();
        let bucket_ptr = unsafe { *bucket_ptr_ptr };
        if !bucket_ptr.is_null() {
            return bucket_ptr;
        }
        unsafe { self.init_bucket_slow(bucket) }
    }

    #[cold]
    unsafe fn init_bucket_slow(&self, bucket: usize) -> *mut T {
        let bucket_ptr_ptr: *mut *mut T = self.buckets[bucket].get();
        assert!(unsafe { *bucket_ptr_ptr }.is_null());
        let bucket_alloc = BucketAlloc::new(Self::bucket_capacity(bucket));
        let bucket_ptr = bucket_alloc.data;
        unsafe { *bucket_ptr_ptr = bucket_ptr };
        mem::forget(bucket_alloc);
        bucket_ptr
    }

    /// Push an element to the end.
    ///
    /// This operation returns error if index is not equal to the current size.
    ///
    /// Wait-free.
    #[inline]
    pub fn push_at(&self, index: usize, value: T) -> Result<(), T> {
        if index == Self::max_capacity() {
            panic!("capacity overflow: {}", Self::max_capacity());
        }

        if self
            .size
            .compare_exchange(
                index,
                index | LOCKED_FLAG,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            // We could retry, but we promised not to lock.
            return Err(value);
        }

        let (bucket, index_in_bucket) = Self::unpack_index(index);

        unsafe {
            let bucket_ptr = self.init_bucket(bucket);
            ptr::write(bucket_ptr.add(index_in_bucket), value);
        }

        self.size.store(index + 1, Ordering::Release);
        Ok(())
    }

    /// Iterate over all elements.
    ///
    /// Iterates over snapshot of the vector, newly pushed elements may not be visible.
    #[inline]
    pub fn iter(&self) -> Iter<'_, T, BUCKETS> {
        let len = self.len();
        let slice = if len == 0 {
            &[]
        } else {
            unsafe {
                slice::from_raw_parts(*self.buckets[0].get(), cmp::min(len, FIRST_BUCKET_CAP))
            }
        };
        Iter {
            vec: self,
            bucket: 0,
            slice: slice.iter(),
            rem: len - slice.len(),
        }
    }
}

impl<T, const BUCKETS: usize> Drop for LockFreeVec<T, BUCKETS> {
    fn drop(&mut self) {
        unsafe {
            let mut rem_len = *self.size.get_mut();
            for (bucket, bucket_ptr_ptr) in self.buckets.iter().enumerate() {
                let bucket_ptr: *mut T = *bucket_ptr_ptr.get();
                if bucket_ptr.is_null() {
                    assert_eq!(0, rem_len);
                    break;
                }

                let bucket_len = cmp::min(rem_len, Self::bucket_capacity(bucket));
                rem_len -= bucket_len;

                let bucket_alloc: BucketAlloc<T> =
                    BucketAlloc::from_raw_parts(bucket_ptr, Self::bucket_capacity(bucket));

                bucket_alloc.drop_data(bucket_len);

                drop(bucket_alloc);
            }

            assert_eq!(0, rem_len);
        }
    }
}

impl<T: Allocative, const SHARDS: usize> Allocative for LockFreeVec<T, SHARDS> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        let mut rem_len = self.len();
        for (bucket, bucket_ptr_ptr) in self.buckets.iter().enumerate() {
            if rem_len == 0 {
                break;
            }

            let bucket_ptr: *mut T = unsafe { *bucket_ptr_ptr.get() };
            if bucket_ptr.is_null() {
                break;
            }

            let bucket_len = cmp::min(rem_len, Self::bucket_capacity(bucket));
            rem_len -= bucket_len;

            let mut visitor = visitor.enter_unique(
                allocative::Key::new("bucket"),
                mem::size_of_val(&bucket_ptr),
            );
            {
                let mut visitor = visitor.enter(
                    allocative::Key::new("capacity"),
                    mem::size_of::<T>() * Self::bucket_capacity(bucket),
                );
                for i in 0..bucket_len {
                    let value: &T = unsafe { &*bucket_ptr.add(i) };
                    visitor.visit_field(allocative::Key::new("item"), value);
                }
                visitor.exit();
            }

            visitor.exit();
        }
        visitor.exit();
    }
}

/// Iterator over all elements.
pub struct Iter<'a, T, const SHARDS: usize> {
    vec: &'a LockFreeVec<T, SHARDS>,
    /// Current bucket index.
    bucket: usize,
    /// Remaining elements in the current bucket.
    slice: slice::Iter<'a, T>,
    /// Remaining elements after the current bucket iterator.
    rem: usize,
}

impl<'a, T, const SHARDS: usize> Iterator for Iter<'a, T, SHARDS> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self.slice.next() {
            Some(value) => Some(value),
            None if self.rem == 0 => None,
            None => {
                self.bucket += 1;
                let bucket_len = cmp::min(
                    self.rem,
                    LockFreeVec::<T, SHARDS>::bucket_capacity(self.bucket),
                );
                let slice = unsafe {
                    slice::from_raw_parts(*self.vec.buckets[self.bucket].get(), bucket_len)
                };
                self.rem -= slice.len();
                self.slice = slice.iter();
                self.slice.next()
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.rem + self.slice.len();
        (rem, Some(rem))
    }
}

impl<T, const SHARDS: usize> ExactSizeIterator for Iter<'_, T, SHARDS> {}

impl<'a, T, const SHARDS: usize> IntoIterator for &'a LockFreeVec<T, SHARDS> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T, SHARDS>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use allocative::FlameGraphBuilder;

    use crate::LockFreeVec;
    use crate::buckets_for_max_capacity;

    #[test]
    fn test_max_capacity() {
        assert_eq!(16, LockFreeVec::<u8, 1>::max_capacity());
        assert_eq!(32, LockFreeVec::<u8, 2>::max_capacity());
        assert_eq!(64, LockFreeVec::<u8, 3>::max_capacity());
        assert_eq!(128, LockFreeVec::<u8, 4>::max_capacity());
    }

    #[test]
    fn test_buckets_for_max_capacity() {
        assert_eq!(1, buckets_for_max_capacity(16));
        assert_eq!(2, buckets_for_max_capacity(32));
        assert_eq!(3, buckets_for_max_capacity(64));
        assert_eq!(4, buckets_for_max_capacity(128));
    }

    #[test]
    fn test_bucket_capacity() {
        assert_eq!(16, LockFreeVec::<u8, 1>::bucket_capacity(0));
        assert_eq!(16, LockFreeVec::<u8, 2>::bucket_capacity(1));
        assert_eq!(32, LockFreeVec::<u8, 3>::bucket_capacity(2));
        assert_eq!(64, LockFreeVec::<u8, 4>::bucket_capacity(3));
    }

    #[test]
    fn test_unpack_index() {
        // Special
        assert_eq!((0, 0), LockFreeVec::<u8, 10>::unpack_index(0));
        assert_eq!((0, 1), LockFreeVec::<u8, 10>::unpack_index(1));
        assert_eq!((0, 2), LockFreeVec::<u8, 10>::unpack_index(2));
        assert_eq!((0, 14), LockFreeVec::<u8, 10>::unpack_index(14));
        assert_eq!((0, 15), LockFreeVec::<u8, 10>::unpack_index(15));
        // Regular
        assert_eq!((1, 0), LockFreeVec::<u8, 10>::unpack_index(16));
        assert_eq!((1, 15), LockFreeVec::<u8, 10>::unpack_index(31));
        assert_eq!((2, 0), LockFreeVec::<u8, 10>::unpack_index(32));
        assert_eq!((2, 31), LockFreeVec::<u8, 10>::unpack_index(63));
        assert_eq!((3, 0), LockFreeVec::<u8, 10>::unpack_index(64));
        assert_eq!((3, 63), LockFreeVec::<u8, 10>::unpack_index(127));
        assert_eq!((4, 0), LockFreeVec::<u8, 10>::unpack_index(128));
        assert_eq!((4, 127), LockFreeVec::<u8, 10>::unpack_index(255));
        assert_eq!((5, 0), LockFreeVec::<u8, 10>::unpack_index(256));
    }

    #[test]
    fn test_simple() {
        let vec = LockFreeVec::<u32, 10>::new();
        for i in 0..100 {
            assert_eq!(i as usize, vec.len());
            vec.push_at(i as usize, i).unwrap();
        }
        for i in 0..100 {
            assert_eq!(i, *vec.get(i as usize).unwrap());
        }
    }

    #[test]
    fn test_full() {
        type V = LockFreeVec<u32, 4>;
        let vec = V::new();
        for i in 0..V::max_capacity() {
            vec.push_at(i, i as u32).unwrap();
        }
        assert_eq!(V::max_capacity(), vec.len());
    }

    #[test]
    fn test_max_buckets() {
        let v = LockFreeVec::<u32>::new();
        for i in 0..100 {
            assert_eq!(i as usize, v.len());
            v.push_at(i as usize, i).unwrap();
        }
        assert_eq!(
            (0..100).collect::<Vec<_>>(),
            v.iter().copied().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_million() {
        let n = if cfg!(miri) { 1_000 } else { 1_000_000 };
        let vec = LockFreeVec::<String, 20>::new();
        for i in 0..n {
            assert_eq!(i, vec.len());
            vec.push_at(i, i.to_string()).unwrap();
        }
    }

    #[test]
    fn test_drop() {
        struct RecordDrops(u32, Arc<Mutex<Vec<u32>>>);
        impl Drop for RecordDrops {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(self.0);
            }
        }

        let drops = Arc::new(Mutex::new(Vec::new()));
        let vec = LockFreeVec::<RecordDrops, 20>::new();

        for i in 0..1000 {
            vec.push_at(i as usize, RecordDrops(i, drops.clone()))
                .ok()
                .unwrap();
        }

        drop(vec);

        let drops = Arc::try_unwrap(drops).unwrap().into_inner().unwrap();
        assert_eq!((0..1000).collect::<Vec<_>>(), drops);
    }

    #[test]
    fn test_iter_empty() {
        let v = LockFreeVec::<u32, 10>::new();
        assert!(v.iter().next().is_none());
    }

    #[test]
    fn test_iter_max() {
        type V = LockFreeVec<u32, 10>;
        let vec = V::new();
        for i in 0..V::max_capacity() {
            vec.push_at(i, i as u32).unwrap();
        }
        let mut iter = vec.iter().enumerate();
        assert_eq!(vec.len(), iter.len());
        while let Some((i, v)) = iter.next() {
            assert_eq!(vec.len() - i - 1, iter.len());
            assert_eq!(i, *v as usize);
        }
        assert_eq!(0, iter.len());
    }

    #[test]
    fn test_iter_partial() {
        type V = LockFreeVec<u32, 10>;
        let vec = V::new();
        for i in 0..100 {
            vec.push_at(i, i as u32).unwrap();
        }
        let mut iter = vec.iter().enumerate();
        assert_eq!(100, iter.len());
        while let Some((i, v)) = iter.next() {
            // Pushing elements does not affect the iterator.
            vec.push_at(vec.len(), 12345).unwrap();

            assert_eq!(99 - i, iter.len());
            assert_eq!(i, *v as usize);
        }
        assert_eq!(0, iter.len());
    }

    #[test]
    fn test_allocative() {
        let v = LockFreeVec::<String, 10>::new();
        for i in 0..100 {
            v.push_at(v.len(), format!("{i:<3}")).unwrap();
        }

        let mut builder = FlameGraphBuilder::default();
        builder.visit_root(&v);
        let flame_graph = builder.finish_and_write_flame_graph();
        // Proper test is possible but expensive to maintain.
        // So print it out and eyeball it.
        // At the moment of writing it looks like this:
        // https://www.internalfb.com/intern/px/p/2GgX5
        // Alternatively we can set up a golden test.
        if false {
            println!("{flame_graph}");
        }
    }
}
