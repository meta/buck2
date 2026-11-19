/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering;
use std::task::Waker;

use parking_lot::Mutex;

/// A synchronization primitive that represents a set of wakers.
///
/// You might alternatively know this primitive under the terms "event listener" or "event counter."
///
/// Tokio provides a higher level `Notify` type that can provide the same API, but with different
/// tradeoffs. In particular, this type prioritizes simplicity and minimal memory footprint at the
/// expense of most other properties. Throughput is only mediocre.
///
/// Each entry carries a `T`, which the set can enumerate for its registered entries (see
/// [`AtomicWakerSet::for_each_registered`]).
pub struct AtomicWakerSet<T> {
    /// Our synchronization approach is quite straightforward: We have one mutex here, and this one
    /// mutex protects everything in this data structure. All operations on any nodes in the
    /// structure must hold the mutex.
    ///
    /// There are alternative implementation strategies with different tradeoffs:
    ///  - You could go slab based instead of intrusive. That makes some problems easier, but it
    ///    costs extra memory and you have to worry about excess capacity, so is avoided here.
    ///  - You could switch to a singly linked list. That has a bunch of benefits including saved
    ///    memory and non-blocking inserts. However, it means quadratic behavior on cancellation,
    ///    which is probably unacceptable.
    mutex: Mutex<*mut AtomicWakerSetEntry<T>>,
}

/// An entry in an `AtomicWakerSet`.
///
/// This type is inherently quite unsafe. Most importantly, it does not internally hold a reference
/// to the set of which the entry is a member. This is a small memory optimization as usually the
/// user of this type will have such a reference available anyway. However, that means that
/// management of the lifetime of this object is entirely on the user.
pub struct AtomicWakerSetEntry<T> {
    /// Never written after construction, which is what makes it safe to read both through the
    /// owner's `&self` and, under the set's lock, from `for_each_registered`.
    data: T,
    /// The three fields below are protected by the lock in the `AtomicWakerSet`, unless the value
    /// is in a state where it has not been inserted into a set, in which case this value owns all
    /// the fields.
    waker: UnsafeCell<Option<Waker>>,
    /// Together, the `next` and `prev` fields form an intrusive linked list of registered entries,
    /// with the mutex in the set holding a pointer to the head.
    ///
    /// `next` is:
    ///
    ///  - 1 to indicate that this entry is not inserted into any set.
    ///  - 0/null to indicate that this is the last element of the set/tail of the list.
    ///  - Otherwise a pointer to the next element of the set.
    ///
    /// This field also makes an exception to the standard ownership rule, as it's important that we
    /// don't need to acquire the lock to check if we're disconnected. As such, we allow the owner
    /// to check for disconnection by performing an acquire load on this field. `wake_all` finalizes
    /// an entry from another thread, so it must release-store `1` here to synchronize with that
    /// acquire load; an owner finalizing its own entry in `disconnect` is on the same thread and so
    /// can store plainly. Writing another entry's `next` (the splice in `disconnect`) still has to
    /// be atomic, since that entry's owner may be performing its lock-free load concurrently.
    next: AtomicPtr<AtomicWakerSetEntry<T>>,
    prev: UnsafeCell<*mut AtomicWakerSetEntry<T>>,
    _pinned: PhantomPinned,
    /// Prevent autotrait impls
    _marker: PhantomData<*mut ()>,
}

impl<T> AtomicWakerSet<T> {
    /// Prepare a new set
    pub fn new() -> Self {
        AtomicWakerSet {
            mutex: Mutex::new(std::ptr::null_mut()),
        }
    }

    /// Calls `f` with the data of every entry that is currently registered.
    ///
    /// An entry counts as registered from its `register` until the next `wake_all` or its
    /// `disconnect`, whichever comes first. So an entry whose waker was woken but which has not
    /// been polled again yet is not seen, even though its owner may still be waiting.
    ///
    /// The set is locked for the duration; `f` must not use the set.
    pub fn for_each_registered(&self, mut f: impl FnMut(&T)) {
        let guard = self.mutex.lock();
        let mut cur = *guard;
        while !cur.is_null() {
            // SAFETY: Registered entries are alive and their list structure is protected by the
            // lock that we hold; `data` is never written after construction.
            unsafe {
                f(&(*cur).data);
                cur = *(*cur).next.as_ptr();
            }
        }
    }

    /// Wake all the wakers in the set.
    pub fn wake_all(&self) {
        unsafe {
            let mut guard = self.mutex.lock();
            let mut cur = *guard;
            // We're about to disconnect everything, so just do this now
            *guard = std::ptr::null_mut();

            // Accumulate all the wakers. We cannot wake them until we release the lock
            let mut wakers: smallvec::SmallVec<[Waker; 128]> = Default::default();
            while !cur.is_null() {
                wakers.extend((*(*cur).waker.get()).take());
                // Disconnect the entry. This is important so that when it itself invokes
                // `disconnect`, it gets the fast path. This must be the last thing we do, since as
                // soon as we execute this our lock no longer protects the entry and the memory
                // might be deallocated.
                let next_cur = (*cur)
                    .next
                    .swap(AtomicWakerSetEntry::NOT_INSERTED, Ordering::Release);
                cur = next_cur;
            }
            // Need to hold the guard until here since we're changing list structure
            drop(guard);
            for w in wakers {
                w.wake();
            }
        }
    }
}

impl<T> Default for AtomicWakerSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: For things other than the `T`, this is straightforward; for the `T: Sync` bound, an
// `AtomicWakerSet<T>` is semantically something like a `Vec<&T>` and so gets matching bounds
unsafe impl<T: Sync> Send for AtomicWakerSet<T> {}
unsafe impl<T: Sync> Sync for AtomicWakerSet<T> {}

impl<T> AtomicWakerSetEntry<T> {
    /// The value of `next` for an entry that is not inserted into any set.
    const NOT_INSERTED: *mut Self = std::ptr::without_provenance_mut(1);

    /// Prepare a new entry carrying `data`
    pub fn new(data: T) -> Self {
        AtomicWakerSetEntry {
            data,
            waker: UnsafeCell::new(None),
            next: AtomicPtr::new(Self::NOT_INSERTED),
            prev: UnsafeCell::new(std::ptr::null_mut()),
            _pinned: PhantomPinned,
            _marker: PhantomData,
        }
    }

    /// The data this entry was created with
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Register the entry into the set with the given waker.
    ///
    /// This is the analogue to the `AtomicWaker::register` API with largely similar semantics: The
    /// waker will be woken next time `wake_all` is called on the set. If a waker was already
    /// registered, it is replaced. Furthermore, this operation happens-after any previous
    /// `wake_all` calls that did not include this operation.
    ///
    /// Unlike `AtomicWaker`, this does not even notionally support being called concurrently; the
    /// self argument is taken by `&mut`.
    ///
    /// # Safety
    ///
    ///  - Eventually, after calling `register`, you must call `disconnect`, evicting this entry
    ///    from the set. `disconnect` must be called with the same exact `set` as `register`.
    ///    Important: You may not drop the entry until after you have disconnected.
    ///  - There is no lifetime attached to this type. It is your responsibility to ensure that
    ///    `set` is kept alive until the `disconnect` call.
    ///  - Until you next disconnect, you may only invoke `register` again with the same `set`.
    pub unsafe fn register(self: Pin<&mut Self>, set: &AtomicWakerSet<T>, waker: Waker) {
        unsafe { self.register_impl(set, waker, || {}) }
    }

    /// Implementation of `register`, with a `pause` hook invoked just before the lock
    /// acquisition. Tests use the hook to deterministically interleave a concurrent
    /// operation into that window; `register` passes a no-op.
    ///
    /// # Safety
    ///
    /// Same contract as `register`. Additionally, `pause` may operate on the set (e.g.
    /// `wake_all`) but must not `register` or `disconnect` this entry.
    unsafe fn register_impl(
        self: Pin<&mut Self>,
        set: &AtomicWakerSet<T>,
        waker: Waker,
        pause: impl FnOnce(),
    ) {
        unsafe {
            let this = Pin::into_inner_unchecked(self.into_ref());
            // With some more work, re-registration by a still-inserted entry could fast path out
            // lock-free when the new waker `will_wake`s the old one, but so far we haven't
            // bothered.
            pause();
            // The wake -> register happens before relationship is established by this lock
            let mut guard = set.mutex.lock();
            // Owning the lock gives us the right to change the waker
            #[expect(clippy::mem_replace_option_with_some)]
            let old = std::mem::replace(&mut *this.waker.get(), Some(waker));
            // Checking this under the lock is important: A concurrent `wake_all` is free to evict
            // us at any point before we acquired it.
            let is_inserted = *this.next.as_ptr() != Self::NOT_INSERTED;
            if !is_inserted {
                // Insert at the head of the list
                *this.prev.get() = std::ptr::null_mut();
                let old_head = *guard;
                let this_ptr = this as *const _ as *mut _;
                *this.next.as_ptr() = old_head;
                // We become the new head, so the old head (if any) must point back at us.
                if !old_head.is_null() {
                    *(*old_head).prev.get() = this_ptr;
                }
                *guard = this_ptr;
            }
            // Don't drop the previous waker while we hold the lock
            drop(guard);
            drop(old);
        }
    }

    /// Evicts this entry from the provided set.
    ///
    /// After disconnecting, the entry is safe to either discard or reuse.
    ///
    /// # Safety
    ///
    /// Must be called on the same set on which the entry is registered.
    pub unsafe fn disconnect(self: Pin<&mut Self>, set: &AtomicWakerSet<T>) {
        unsafe {
            let this = Pin::into_inner_unchecked(self.into_ref());
            let is_inserted = this.next.load(Ordering::Acquire) != Self::NOT_INSERTED;
            if is_inserted {
                // Slow removal, indicative of a cancellation instead of a previous wakeup
                let mut guard = set.mutex.lock();
                // Need to check that we're still inserted
                let next = *this.next.as_ptr();
                let still_inserted = next != Self::NOT_INSERTED;
                if still_inserted {
                    let prev = *this.prev.get();
                    if !next.is_null() {
                        *(*next).prev.get() = prev;
                    }
                    if !prev.is_null() {
                        // `prev.next` is read lock-free by `prev`'s owner, so this write must be
                        // atomic to avoid a data race. The value is only consumed under the lock, so
                        // `Relaxed` is sufficient.
                        (*prev).next.store(next, Ordering::Relaxed);
                    } else {
                        // We were the first entry
                        *guard = next;
                    }
                    // Finalize: mark ourselves not-inserted so a later `register` reuses us and a
                    // later `disconnect` fast-paths. Plain store is fine here - see `next`'s docs.
                    *this.next.as_ptr() = Self::NOT_INSERTED;
                }
            }
        }
    }
}

impl<T: Default> Default for AtomicWakerSetEntry<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

// SAFETY: Other than the `T` bounds, this is straightforward; for the `T` it's slightly tricky.
// Although the `AtomicWakerSetEntry<T>` itself only provides access to the one `T` and so behaves
// like it just owns a `T`, it leaves behind access to a `&T` on the original thread, via that set
// that it's registered in. That behaves like sending a `&T`, so we must bound `T` appropriately.
unsafe impl<T: Send + Sync> Send for AtomicWakerSetEntry<T> {}
unsafe impl<T: Sync> Sync for AtomicWakerSetEntry<T> {}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::task::Wake;
    use std::task::Waker;

    use super::*;

    /// A waker that counts how many times it has been woken.
    struct CountingWaker(AtomicUsize);

    impl CountingWaker {
        fn new() -> Arc<Self> {
            Arc::new(CountingWaker(AtomicUsize::new(0)))
        }

        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn registered_entry_is_woken() {
        let set = AtomicWakerSet::new();
        let waker = CountingWaker::new();

        let mut entry = pin!(AtomicWakerSetEntry::new(()));
        unsafe {
            entry.as_mut().register(&set, Waker::from(waker.clone()));
            set.wake_all();
            entry.as_mut().disconnect(&set);
        }

        assert_eq!(waker.count(), 1, "a registered entry must be woken once");
    }

    #[test]
    fn disconnected_entry_is_not_woken() {
        let set = AtomicWakerSet::new();
        let waker = CountingWaker::new();

        let mut entry = pin!(AtomicWakerSetEntry::new(()));
        unsafe {
            entry.as_mut().register(&set, Waker::from(waker.clone()));
            entry.as_mut().disconnect(&set);
            set.wake_all();
        }

        assert_eq!(waker.count(), 0, "a disconnected entry must not be woken");
    }

    #[test]
    fn disconnecting_one_entry_keeps_the_others_registered() {
        let set = AtomicWakerSet::new();
        let waker_a = CountingWaker::new();
        let waker_b = CountingWaker::new();

        let mut a = pin!(AtomicWakerSetEntry::new(()));
        let mut b = pin!(AtomicWakerSetEntry::new(()));
        unsafe {
            a.as_mut().register(&set, Waker::from(waker_a.clone()));
            b.as_mut().register(&set, Waker::from(waker_b.clone()));

            // Cancel only A. B must stay registered.
            a.as_mut().disconnect(&set);
            set.wake_all();

            b.as_mut().disconnect(&set);
        }

        assert_eq!(waker_a.count(), 0, "A was disconnected, must not be woken");
        assert_eq!(waker_b.count(), 1, "B was still registered, must be woken");
    }

    #[test]
    fn entry_can_be_reused_after_disconnect() {
        let set = AtomicWakerSet::new();
        let waker = CountingWaker::new();

        let mut entry = pin!(AtomicWakerSetEntry::new(()));
        unsafe {
            entry.as_mut().register(&set, Waker::from(waker.clone()));
            entry.as_mut().disconnect(&set);

            // Reuse the same entry. It must once again be woken by `wake_all`.
            entry.as_mut().register(&set, Waker::from(waker.clone()));
            set.wake_all();
            entry.as_mut().disconnect(&set);
        }

        assert_eq!(
            waker.count(),
            1,
            "a reused entry must be woken after re-registering"
        );
    }

    /// Regression test for a lost-wakeup race between a re-`register` and a concurrent
    /// `wake_all` that evicts the entry just before `register` acquires the lock:
    /// `register` must notice the eviction under the lock and re-insert, so the new waker
    /// stays visible to subsequent `wake_all` calls.
    ///
    /// A real concurrent thread has to land its entire `wake_all` inside the
    /// couple-of-instructions window before the lock acquisition, which is far too rare to
    /// exercise reliably by brute force. Instead, the `pause` hook of `register_impl`
    /// executes the `wake_all` deterministically inside that window, on a single thread.
    #[test]
    fn register_racing_with_wake_all_does_not_lose_wakeups() {
        let set = AtomicWakerSet::new();
        let racing_waker = CountingWaker::new();

        let mut entry = pin!(AtomicWakerSetEntry::new(()));
        unsafe {
            entry
                .as_mut()
                .register(&set, Waker::from(CountingWaker::new()));
            entry
                .as_mut()
                .register_impl(&set, Waker::from(racing_waker.clone()), || set.wake_all());
            // The racing `register` completed before this `wake_all` starts, so the racing
            // waker must be woken by now at the latest.
            set.wake_all();
            entry.as_mut().disconnect(&set);
        }

        assert_eq!(racing_waker.count(), 1, "the race must not lose the wakeup");
    }

    #[test]
    fn all_entries_in_a_larger_set_are_woken() {
        let set = AtomicWakerSet::new();
        let wakers: Vec<_> = (0..5).map(|_| CountingWaker::new()).collect();

        let mut entries: Vec<_> = (0..5)
            .map(|_| Box::pin(AtomicWakerSetEntry::new(())))
            .collect();
        unsafe {
            for (entry, waker) in entries.iter_mut().zip(&wakers) {
                entry.as_mut().register(&set, Waker::from(waker.clone()));
            }
            set.wake_all();
            for entry in &mut entries {
                entry.as_mut().disconnect(&set);
            }
        }

        for (i, waker) in wakers.iter().enumerate() {
            assert_eq!(waker.count(), 1, "entry {i} must be woken exactly once");
        }
    }

    #[test]
    fn for_each_registered_sees_exactly_the_registered_entries() {
        let set = AtomicWakerSet::new();
        let registered = || {
            let mut data = Vec::new();
            set.for_each_registered(|d: &u32| data.push(*d));
            data.sort();
            data
        };

        let mut a = pin!(AtomicWakerSetEntry::new(1));
        let mut b = pin!(AtomicWakerSetEntry::new(2));
        assert_eq!(registered(), Vec::<u32>::new());
        unsafe {
            a.as_mut().register(&set, Waker::from(CountingWaker::new()));
            b.as_mut().register(&set, Waker::from(CountingWaker::new()));
            assert_eq!(registered(), vec![1, 2]);

            // Re-registering an entry does not duplicate it.
            a.as_mut().register(&set, Waker::from(CountingWaker::new()));
            assert_eq!(registered(), vec![1, 2]);

            a.as_mut().disconnect(&set);
            assert_eq!(registered(), vec![2]);

            // Waking evicts the entries, so they are gone until they register again.
            set.wake_all();
            assert_eq!(registered(), Vec::<u32>::new());

            b.as_mut().register(&set, Waker::from(CountingWaker::new()));
            assert_eq!(registered(), vec![2]);
            b.as_mut().disconnect(&set);
        }
    }
}
