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
use std::mem;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Acquire;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::Ordering::Release;

use allocative::Allocative;
use allocative::Key;
use allocative::Visitor;

/// Remembers pairs `(container, contained)` of shared values, for a caller that has found that
/// `container` already contains everything `contained` does.
///
/// The cache is a fixed-size, direct-mapped table: a pair lives in the one slot its addresses
/// hash to until another pair that hashes there replaces it. Forgetting a pair is always fine, it
/// only costs the caller the work of finding the fact out again.
///
/// A pair is identified by the addresses of its two values. Each entry holds weak references to
/// both, which keeps those addresses from being reused for other values while the entry exists,
/// so that addresses that match an entry belong to the values the entry was made for. The weak
/// references keep the allocations of the two values alive but not the values themselves.
pub(crate) struct ContainmentCache<T> {
    slots: Box<[Slot<T>]>,
    /// `64 - log2(slots.len())`.
    shift: u32,
    _owns: PhantomData<Weak<T>>,
}

/// One entry, or none when the pointers are null.
///
/// `seq` is a sequence lock: odd while a writer is replacing the entry. A reader that sees the
/// same even value before and after reading both pointers has read one entry as a whole.
///
/// A writer makes `seq` odd, stores the pointers and then stores the next even value, the last
/// three with release ordering. A reader loads `seq` and the pointers with acquire ordering,
/// and then `seq` again:
///  - The first load of `seq` puts the stores of the write that it saw the end of before the
///    reader's loads, so the reader sees no pointer older than that write's.
///  - If the reader sees a pointer of a later write, that load puts the write's update of `seq`
///    before the reader's second load of it, which then differs from the first.
struct Slot<T> {
    seq: AtomicUsize,
    /// `Weak::into_raw` of the container, owned by the slot.
    container: AtomicPtr<T>,
    /// `Weak::into_raw` of the contained value, owned by the slot.
    contained: AtomicPtr<T>,
}

impl<T> ContainmentCache<T> {
    /// A cache with `1 << slots_log2` slots.
    pub(crate) fn new(slots_log2: u32) -> Self {
        // SAFETY: A `Slot` is an atomic integer and two atomic pointers, for which all zeroes is
        // a valid value: zero and null. Asking the allocator for zeroed memory, instead of
        // writing zeroes, lets it hand out pages that are not resident until an entry lands on
        // them.
        let slots = unsafe { Box::<[Slot<T>]>::new_zeroed_slice(1 << slots_log2).assume_init() };
        Self {
            slots,
            shift: u64::BITS - slots_log2,
            _owns: PhantomData,
        }
    }

    fn slot(&self, container: *const T, contained: *const T) -> &Slot<T> {
        const K: u64 = 0x9E37_79B9_7F4A_7C15;
        let hash = ((container.addr() as u64).wrapping_mul(K)
            ^ (contained.addr() as u64).rotate_left(32))
        .wrapping_mul(K);
        // A shift by 64, for a table of one slot, would overflow.
        &self.slots[hash.checked_shr(self.shift).unwrap_or(0) as usize]
    }

    /// Whether `insert` was called with this pair, and the pair has not been replaced since.
    pub(crate) fn contains(&self, container: &Arc<T>, contained: &Arc<T>) -> bool {
        let container = Arc::as_ptr(container);
        let contained = Arc::as_ptr(contained);
        let slot = self.slot(container, contained);
        let seq = slot.seq.load(Acquire);
        seq & 1 == 0
            && slot.container.load(Acquire).cast_const() == container
            && slot.contained.load(Acquire).cast_const() == contained
            && slot.seq.load(Relaxed) == seq
    }

    pub(crate) fn insert(&self, container: &Arc<T>, contained: &Arc<T>) {
        let slot = self.slot(Arc::as_ptr(container), Arc::as_ptr(contained));
        let seq = slot.seq.load(Relaxed);
        if seq & 1 != 0
            || slot
                .seq
                .compare_exchange(seq, seq + 1, Acquire, Relaxed)
                .is_err()
        {
            // Another thread is writing this slot. Not recording this pair is fine. That is no
            // reason for the weak exchange though: it may fail with nobody else around, and
            // there is no loop here that would try again.
            return;
        }
        // The acquire ordering of the exchange makes these the pointers that the write before
        // this one stored.
        let replaced = [slot.container.load(Relaxed), slot.contained.load(Relaxed)];
        slot.container.store(
            Weak::into_raw(Arc::downgrade(container)).cast_mut(),
            Release,
        );
        slot.contained.store(
            Weak::into_raw(Arc::downgrade(contained)).cast_mut(),
            Release,
        );
        slot.seq.store(seq + 2, Release);
        // SAFETY: Only this thread could write the slot between the two updates of `seq`, so
        // the pointers it took out are the slot's to release.
        unsafe { Self::release(replaced) }
    }

    /// Drops the weak references behind pointers taken out of a slot.
    ///
    /// # Safety
    ///
    /// Each pointer is null or came from `Weak::<T>::into_raw`, and nothing else releases it.
    unsafe fn release(pointers: [*mut T; 2]) {
        for pointer in pointers {
            if !pointer.is_null() {
                // SAFETY: Per the contract of this function.
                drop(unsafe { Weak::from_raw(pointer.cast_const()) });
            }
        }
    }
}

impl<T> Drop for ContainmentCache<T> {
    fn drop(&mut self) {
        for slot in self.slots.iter_mut() {
            // SAFETY: `&mut self` rules out any other access to the slots.
            unsafe { Self::release([*slot.container.get_mut(), *slot.contained.get_mut()]) }
        }
    }
}

impl<T> Allocative for ContainmentCache<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_simple(
            Key::new("slots"),
            self.slots.len() * mem::size_of::<Slot<T>>(),
        );
        visitor.exit();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use crate::directory::containment_cache::ContainmentCache;

    #[test]
    fn test_remembers_a_pair_in_one_direction() {
        let cache = ContainmentCache::new(4);
        let (a, b, c) = (Arc::new(1), Arc::new(2), Arc::new(3));

        assert!(!cache.contains(&a, &b));
        cache.insert(&a, &b);
        assert!(cache.contains(&a, &b));
        assert!(!cache.contains(&b, &a));
        assert!(!cache.contains(&a, &c));
        assert!(!cache.contains(&c, &b));
    }

    #[test]
    fn test_does_not_keep_values_alive() {
        let cache = ContainmentCache::new(4);
        let (a, b) = (Arc::new(1), Arc::new(2));
        cache.insert(&a, &b);
        assert_eq!(Arc::strong_count(&a), 1);
        assert_eq!(Arc::weak_count(&a), 1);

        let a_weak = Arc::downgrade(&a);
        drop(a);
        assert!(a_weak.upgrade().is_none());
    }

    #[test]
    fn test_address_of_a_dropped_value_is_not_reused_while_cached() {
        let cache = ContainmentCache::new(4);
        let (a, b) = (Arc::new(1), Arc::new(2));
        cache.insert(&a, &b);
        let a_address = Arc::as_ptr(&a);
        drop(a);

        // The allocator would hand the freed address of `a` straight back for a value of the
        // same size. It cannot while the entry holds on to the allocation.
        let later: Vec<Arc<i32>> = (0..64).map(Arc::new).collect();
        for value in &later {
            assert_ne!(Arc::as_ptr(value), a_address);
            assert!(!cache.contains(value, &b));
        }
    }

    #[test]
    fn test_replaced_and_dropped_entries_release_their_values() {
        // A single slot, so that the second pair replaces the first.
        let cache = ContainmentCache::new(0);
        let (a, b, c, d) = (Arc::new(1), Arc::new(2), Arc::new(3), Arc::new(4));

        cache.insert(&a, &b);
        cache.insert(&c, &d);
        assert!(!cache.contains(&a, &b));
        assert!(cache.contains(&c, &d));
        assert_eq!(Arc::weak_count(&a), 0);
        assert_eq!(Arc::weak_count(&b), 0);
        assert_eq!(Arc::weak_count(&c), 1);

        drop(cache);
        assert_eq!(Arc::weak_count(&c), 0);
        assert_eq!(Arc::weak_count(&d), 0);
    }

    #[test]
    fn test_concurrent_writers_never_produce_a_pair_nobody_inserted() {
        // A single slot that every thread fights over.
        let cache = ContainmentCache::new(0);
        let values: Vec<Arc<usize>> = (0..4).map(Arc::new).collect();
        let rounds = if cfg!(miri) { 200 } else { 200_000 };

        thread::scope(|scope| {
            // Writers insert `(values[0], values[1])` and `(values[2], values[3])`.
            for first in [0, 2] {
                let (cache, values) = (&cache, &values);
                scope.spawn(move || {
                    for _ in 0..rounds {
                        cache.insert(&values[first], &values[first + 1]);
                    }
                });
            }
            // A reader that saw one half of each pair would report one of these.
            for _ in 0..2 {
                let (cache, values) = (&cache, &values);
                scope.spawn(move || {
                    for _ in 0..rounds {
                        assert!(!cache.contains(&values[0], &values[3]));
                        assert!(!cache.contains(&values[2], &values[1]));
                    }
                });
            }
        });

        assert!(cache.contains(&values[0], &values[1]) || cache.contains(&values[2], &values[3]));
    }
}
