/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! More efficient alternatives to `futures::future::join_all`/`try_join_all`.
//!
//! There are only two API differences compared to the `futures` types:
//!  - The constructors require `ExactSizeIterator`.
//!  - We avoid livelocks and other pathological cases, but otherwise do not concern ourselves too
//!    much with cooperative yielding.
//!
//! The memory backing these implementation is primarily a single allocation in the form
//! `[Either<FutureType, ReturnType>]`. This is usually the best you can hope for, but it does have
//! one disadvantage in that no memory can be freed until all futures have finished. If your futures
//! are very big and you expect many to finish much sooner than others, you may wish to box the
//! futures to prevent that.
//!
//! Other than that, these implementations are more or less strictly superior to the ones provided
//! by `futures`.

use std::future::Future;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::AtomicU32;
use std::task::Context;
use std::task::Poll;
use std::task::RawWaker;
use std::task::RawWakerVTable;
use std::task::Waker;

use futures::task::AtomicWaker;
use mini_vec::MiniBoxSlice;
use mini_vec::MiniVec;
use parking_lot::Mutex;

/// Drives all the futures to completion, returning their outputs in input order.
///
/// Equivalent to `futures::future::join_all`, but see the module docs.
pub fn join_all<I, F: Future>(futs: I) -> JoinAll<F>
where
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator<Item = F>,
{
    JoinAll {
        core: JoinCore::new(futs.into_iter()),
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct JoinAll<F: Future> {
    core: JoinCore<F, AllOutcomes>,
}

impl<F: Future> Future for JoinAll<F> {
    type Output = Vec<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.core.poll(cx).map(|r| match r {
            Ok(v) => v,
        })
    }
}

/// Like from `futures`, but better.
pub trait TryFuture
where
    Self: Future<Output = Result<Self::Ok, Self::Error>>,
{
    type Ok;
    type Error;
}

impl<T, E, F: Future<Output = Result<T, E>>> TryFuture for F {
    type Ok = T;
    type Error = E;
}

/// Drives all the futures to completion, returning their successes in input order, or the first
/// error. On error, the remaining futures are dropped immediately.
///
/// Equivalent to `futures::future::try_join_all`, but see the module docs. Also fixes
/// <https://github.com/rust-lang/futures-rs/issues/2866>.
pub fn try_join_all<I>(futs: I) -> TryJoinAll<I::Item>
where
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator,
    I::Item: TryFuture,
{
    TryJoinAll {
        core: JoinCore::new(futs.into_iter()),
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct TryJoinAll<F: TryFuture> {
    core: JoinCore<F, TryOutcomes>,
}

impl<F> Future for TryJoinAll<F>
where
    F: TryFuture,
{
    type Output = Result<Vec<F::Ok>, F::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Fine because the futures live behind a box; see `JoinCore::entries`.
        self.get_mut().core.poll(cx)
    }
}

/// What to do with each future's output: Either keep it for the final `Vec`, or bail out of the
/// whole join. The two impls are what distinguish `join_all` from `try_join_all`.
trait Outcomes<F: Future> {
    type Keep;
    type Bail;

    fn split(o: F::Output) -> Result<Self::Keep, Self::Bail>;
}

struct AllOutcomes;

enum Never {}

impl<F: Future> Outcomes<F> for AllOutcomes {
    type Keep = F::Output;
    type Bail = Never;

    fn split(o: F::Output) -> Result<F::Output, Never> {
        Ok(o)
    }
}

struct TryOutcomes;

impl<F> Outcomes<F> for TryOutcomes
where
    F: TryFuture,
{
    type Keep = F::Ok;
    type Bail = F::Error;

    fn split(o: Result<F::Ok, F::Error>) -> Result<F::Ok, F::Error> {
        o
    }
}

/// Below this many futures, skip per-future bookkeeping and just poll each nested future each time
/// we're polled. `futures` has a similar parameter but uses 30 for the value; given our improved
/// memory footprint that seems likely to be too high. That being said, this parameter has not been
/// tuned.
const TRACKING_THRESHOLD: usize = 8;

struct JoinCore<F: Future, M: Outcomes<F>> {
    /// The futures, each slot reused for its result once it completes.
    ///
    /// The futures in the entries are pinned.
    entries: MiniBoxSlice<Entry<F, M::Keep>>,
    /// Wake bookkeeping. Kept alive beyond `self` by any wakers that outlive us. `None` for
    /// small sets, which poll everything on every wake instead of tracking.
    shared: Option<SharedHandle>,
    /// Number of entries still `Running`.
    remaining: u32,
    /// Whether the initial poll-everything sweep has happened.
    started: bool,
}

/// One slot of the join: A future, then its (kept) output, in the same storage.
enum Entry<F, K> {
    Running(F),
    Done(K),
    Taken,
}

impl<F: Future, M: Outcomes<F>> JoinCore<F, M> {
    fn new(futs: impl ExactSizeIterator<Item = F>) -> Self {
        Self::with_threshold(futs, TRACKING_THRESHOLD)
    }

    fn with_threshold(futs: impl ExactSizeIterator<Item = F>, threshold: usize) -> Self {
        let n = futs.len();
        JoinCore {
            entries: futs.into_iter().map(Entry::Running).collect(),
            shared: if n >= threshold {
                Some(Shared::alloc(n))
            } else {
                None
            },
            remaining: n.try_into().unwrap(),
            started: false,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<Vec<M::Keep>, M::Bail>> {
        assert!(
            self.remaining != 0 || !self.entries.is_empty() || !self.started,
            "polled after completion"
        );
        let Self {
            entries,
            shared,
            remaining,
            started,
        } = self;
        match shared {
            None => {
                // Small set: Just poll everything that's still running, handing the branches
                // our own waker.
                *started = true;
                for i in 0..entries.len() {
                    if let Some(bail) = Self::poll_entry(entries, remaining, i, cx.waker()) {
                        return Poll::Ready(Err(bail));
                    }
                }
            }
            Some(shared) => {
                // Register first as always
                shared.get().parent.register(cx.waker());

                if !*started {
                    *started = true;
                    for i in 0..entries.len() {
                        let waker = Shared::borrowed_waker(shared, i);
                        if let Some(bail) = Self::poll_entry(entries, remaining, i, &waker) {
                            return Poll::Ready(Err(bail));
                        }
                    }
                }
                let mut guard = shared.get().woken.lock();
                // Bail early if this was a spurious wake
                if !guard.list.is_empty() {
                    // We cannot be caught holding this lock while polling, because a poll might wake
                    // one of our branches, which needs this same lock -- a deadlock. So we swap the
                    // woken list out for an empty one and release the lock before draining.
                    let mut woken = guard.take();
                    drop(guard);
                    for idx in woken.drain(..) {
                        let waker = Shared::borrowed_waker(shared, idx as usize);
                        if let Some(bail) =
                            Self::poll_entry(entries, remaining, idx as usize, &waker)
                        {
                            return Poll::Ready(Err(bail));
                        }
                    }
                    // `woken` is now an empty buffer; attempt to put it back so that we don't end
                    // up with each `wake` allocating.
                    let mut guard = shared.get().woken.lock();
                    if guard.list.is_empty() {
                        guard.list = woken;
                    } else {
                        // Another wake has happened in the meantime, so we certainly can't just
                        // clobber the existing buffer. There may be some temptation to swap and
                        // loop, polling all the items in the new buffer, however that is not
                        // alright either; an inner future may have woken itself on `poll`, in which
                        // case re-polling here risks fully live-locking. We must return to the main
                        // executor at some point.
                    }
                }
            }
        }

        if self.remaining == 0 {
            let mut entries = std::mem::take(&mut self.entries);
            let mut out = Vec::with_capacity(entries.len());
            for e in entries.iter_mut() {
                match std::mem::replace(e, Entry::Taken) {
                    Entry::Done(r) => out.push(r),
                    Entry::Running(_) | Entry::Taken => {
                        // This would mean that we have a bug somewhere and miscounted `remaining`.
                        // `abort` because the pin contract of the future has been violated at this
                        // point and there's nothing else to do
                        std::process::abort()
                    }
                }
            }
            Poll::Ready(Ok(out))
        } else {
            Poll::Pending
        }
    }

    /// Polls the entry with the given waker if it is still running. On completion the output is
    /// either stored back into the entry or, if it means the whole join is done for, returned;
    /// in that case all remaining futures are dropped before returning.
    ///
    /// Takes the fields instead of `&mut self` so that callers can keep `self.shared` borrowed.
    fn poll_entry(
        entries: &mut MiniBoxSlice<Entry<F, M::Keep>>,
        remaining: &mut u32,
        i: usize,
        waker: &Waker,
    ) -> Option<M::Bail> {
        let entry = &mut entries[i];
        let fut = match entry {
            // SAFETY: The futures are pinned as per the comment on the field
            Entry::Running(fut) => unsafe { Pin::new_unchecked(fut) },
            // A stale wake for a future that already completed.
            _ => return None,
        };
        let mut cx = Context::from_waker(waker);
        match fut.poll(&mut cx) {
            Poll::Pending => None,
            Poll::Ready(out) => {
                *remaining -= 1;
                match M::split(out) {
                    Ok(keep) => {
                        // Make sure to drop the future in-place, since it's pinned.
                        *entry = Entry::Done(keep);
                        None
                    }
                    Err(bail) => {
                        // Drop everything else right away.
                        *entries = MiniBoxSlice::new();
                        Some(bail)
                    }
                }
            }
        }
    }
}

/// The waker for each future needs to report the wake back to this shared instance. However, we
/// face a bit of a problem: The waker needs to both know the handle to this `Shared` as well as the
/// index `i` of the future that it's associated with (so that we know which future needs polling).
/// The representation of a waker, however has just one `*const ()` of data, which can't easily hold
/// both the pointer and the `i`. The easy solution would be to allocate a control block for each
/// future at a cost of 16 bytes.
///
/// We instead use the following trick: The `Shared` struct below is conceptually the header of an
/// allocation that is laid out like `[Shared, 0_u32, 1_u32, ..., {N-1}_u32]` for `N` the total
/// number of futures. Each waker `i` is a pointer to the `i_u32` in the above table. On `wake`, the
/// waker reads the `u32` that its data points to, thereby discovering its index `i`. It then uses
/// that `i` and offsets backwards to find the `Shared` header. In this way we avoid allocating a
/// traditional control block.
///
/// Actually, we make one more optimization on top of that model: We stuff the bottom two bits of
/// `i` into the low alignment bits of the data pointer. That way our index table only needs to go
/// to `N >> 2` and we end up with the `Shared` allocation having size `1 * N + constant` bytes.
#[repr(C)]
struct Shared {
    /// `Arc` doesn't really support interacting with funky allocations, so we do manual refcounting
    /// (its almost required with wakers anyway).
    refcount: AtomicU32,
    /// The number of branches; determines the allocation's layout.
    n: u32,
    /// Indexes of branches woken since the join was last polled. Pushes are deduplicated only in so
    /// far as `wake` on an already-woken branch is a no-op once recorded here and not yet consumed
    /// - a branch legitimately re-woken after being polled again simply appears again.
    ///
    /// The mutex also makes wake vs. drain ordering trivially correct, in the style of
    /// [`crate::atomic_waker_set::AtomicWakerSet`]: One uncontended lock per wake is cheap.
    ///
    /// As always with these locks, it absolutely must not be held across any unknown code without
    /// risking a deadlock, including particularly `poll` and `wake` calls. That's why this lock
    /// must also be separate from the `AtomicWaker` below.
    woken: Mutex<Woken>,
    /// The join future's own waker.
    parent: AtomicWaker,
}

/// Once this many wakes are recorded, `Woken` stops finding duplicates by scanning its list and
/// allocates a bitset instead. Joins whose wakes stay below it, including every join narrower
/// than it, never allocate the bitset.
///
/// In benchmarks, values from 16 to 256 performed within a few percent of each other; the
/// threshold mainly exists to spare narrow joins an allocation they do not need.
const WOKEN_BITSET_THRESHOLD: usize = 32;

/// The set of branches woken since the join was last polled, in wake order.
struct Woken {
    list: MiniVec<u32>,
    /// One bit per branch, set iff the index is in `list`. Allocated when `list` first reaches
    /// `WOKEN_BITSET_THRESHOLD` and kept for the rest of the join.
    seen: Option<Box<[u64]>>,
}

impl Woken {
    fn new() -> Self {
        Woken {
            list: MiniVec::new(),
            seen: None,
        }
    }

    /// Records branch `i` of `n`, returning whether it was newly added.
    fn insert(&mut self, i: u32, n: u32) -> bool {
        if self.seen.is_none() && self.list.len() >= WOKEN_BITSET_THRESHOLD {
            let mut seen = vec![0u64; (n as usize).div_ceil(64)].into_boxed_slice();
            for &idx in self.list.iter() {
                seen[idx as usize / 64] |= 1u64 << (idx % 64);
            }
            self.seen = Some(seen);
        }
        let added = match &mut self.seen {
            Some(seen) => {
                let (word, bit) = (&mut seen[i as usize / 64], 1u64 << (i % 64));
                let added = *word & bit == 0;
                *word |= bit;
                added
            }
            None => !self.list.contains(&i),
        };
        if added {
            self.list.push(i);
        }
        added
    }

    /// Takes the recorded indexes, leaving the set empty.
    ///
    /// The bits are cleared here rather than as the caller drains, because the caller
    /// releases the lock before polling: a branch that re-wakes mid-drain must see itself as
    /// absent, or its wake is dropped and the join hangs.
    fn take(&mut self) -> MiniVec<u32> {
        let list = std::mem::replace(&mut self.list, MiniVec::new());
        if let Some(seen) = &mut self.seen {
            // Every set bit belongs to an index in `list`, so zeroing each listed index's whole
            // word clears them all. Unlike clearing the whole bitset, this costs O(list) rather
            // than O(n) per poll.
            for &idx in list.iter() {
                seen[idx as usize / 64] = 0;
            }
        }
        list
    }
}

/// The type of the entries `i_u32` in the `[Shared, 0_u32, ...]` allocation. Just to make it
/// obvious when we're referring to it.
type IndexTableEntry = u32;
/// The index table sits directly after the header.
const INDEX_TABLE_OFFSET: usize = {
    assert!(std::mem::align_of::<IndexTableEntry>() <= std::mem::align_of::<Shared>());
    std::mem::size_of::<Shared>()
};
const PACK_BITS: u32 = std::mem::align_of::<IndexTableEntry>().ilog2();

impl Shared {
    fn layout(n: usize) -> (std::alloc::Layout, usize) {
        let table_entries = n.div_ceil(1 << PACK_BITS);
        (
            std::alloc::Layout::new::<Shared>()
                .extend(std::alloc::Layout::array::<IndexTableEntry>(table_entries).unwrap())
                .unwrap()
                .0
                .pad_to_align(),
            table_entries,
        )
    }

    fn alloc(n: usize) -> SharedHandle {
        let (layout, table_entries) = Shared::layout(n);
        // SAFETY: The layout is never zero-sized (there's always a header).
        let base = unsafe { std::alloc::alloc(layout) };
        let Some(ptr) = NonNull::new(base.cast::<Shared>()) else {
            std::alloc::handle_alloc_error(layout);
        };
        // SAFETY: Freshly allocated with room for the header followed by the index table.
        unsafe {
            ptr.write(Shared {
                refcount: AtomicU32::new(1),
                n: n.try_into().unwrap(),
                woken: Mutex::new(Woken::new()),
                parent: AtomicWaker::new(),
            });
            for entry in 0..table_entries {
                base.byte_add(INDEX_TABLE_OFFSET)
                    .cast::<IndexTableEntry>()
                    .add(entry)
                    .write(entry as IndexTableEntry);
            }
        }
        SharedHandle(ptr)
    }

    /// A waker for the `i`th branch that does NOT hold a refcount; the caller keeps the
    /// allocation alive for the duration of the poll. Clones of it (which is what futures store)
    /// do.
    fn borrowed_waker(handle: &SharedHandle, i: usize) -> ManuallyDrop<Waker> {
        debug_assert!(i < handle.get().n as usize);
        // SAFETY: In bounds of the allocation per the layout.
        let data = unsafe {
            handle
                .0
                .as_ptr()
                .cast::<()>()
                // This should be something like
                // ```
                // entry_index = i / align_of(entry)
                // to_add = INDEX_TABLE_OFFSET + entry_index * align_of(entry) + (i % align_of(entry))
                // ```
                // But that just simplifies to this
                .byte_add(INDEX_TABLE_OFFSET + i)
        };
        // SAFETY: See the vtable functions.
        ManuallyDrop::new(unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) })
    }
}

/// Recovers the header from a waker's data pointer (which points into the index table).
///
/// # Safety
///
/// `data` must be a waker data pointer produced by `borrowed_waker` (or a clone), and the
/// allocation must be live.
unsafe fn shared_from_data(data: *const ()) -> (*const Shared, u32) {
    const MASK: usize = (1 << PACK_BITS) - 1;
    let low = data.addr() & MASK;
    let slot = data.map_addr(|a| a & (!MASK)).cast::<u32>();
    // SAFETY: The slot holds this branch's index, written at allocation time and never changed.
    let entry = unsafe { *slot } as usize;
    let i = (entry << PACK_BITS) + low;
    // SAFETY: Walking backwards within the same allocation, exactly undoing `borrowed_waker`.
    let shared = unsafe { slot.byte_sub(INDEX_TABLE_OFFSET + (entry << PACK_BITS)) };
    (shared.cast::<Shared>(), i as u32)
}

/// # Safety
///
/// `shared` must be live and the caller must own a count on it.
unsafe fn decref(shared: *const Shared) {
    // SAFETY: Per the contract, this releases a count we own.
    if unsafe { &(*shared).refcount }.fetch_sub(1, std::sync::atomic::Ordering::Release) == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        // SAFETY: That was the last count; nothing can touch the allocation anymore.
        unsafe {
            let n = (*shared).n as usize;
            let ptr = shared.cast_mut();
            std::ptr::drop_in_place(ptr);
            std::alloc::dealloc(ptr.cast::<u8>(), Shared::layout(n).0);
        }
    }
}

/// The waker data pointer points into a `Shared`'s index table; each waker clone owns one count
/// on the allocation (borrowed wakers, which hold none, are never dropped).
static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

unsafe fn clone_raw(data: *const ()) -> RawWaker {
    // SAFETY: Data pointer contract of the vtable.
    let (shared, _) = unsafe { shared_from_data(data) };
    // SAFETY: The existing waker holds (or its poll-scope caller guarantees) a count, so the
    // allocation is live and we can take another count.
    unsafe { &(*shared).refcount }.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    RawWaker::new(data, &VTABLE)
}

unsafe fn wake_raw(data: *const ()) {
    // SAFETY: Forwarding the vtable contract.
    unsafe {
        wake_by_ref_raw(data);
        drop_raw(data);
    }
}

unsafe fn wake_by_ref_raw(data: *const ()) {
    // SAFETY: Data pointer contract of the vtable.
    let (shared, i) = unsafe { shared_from_data(data) };
    // SAFETY: The waker holds a count, so the allocation is live.
    let shared = unsafe { &*shared };
    {
        let mut woken = shared.woken.lock();
        if !woken.insert(i, shared.n) {
            // Already recorded; whoever recorded it also woke the parent.
            return;
        }
    }
    shared.parent.wake();
}

unsafe fn drop_raw(data: *const ()) {
    // SAFETY: Data pointer contract of the vtable.
    let (shared, _) = unsafe { shared_from_data(data) };
    // SAFETY: Releases the count this waker owns.
    unsafe { decref(shared) };
}

/// Owns one count on a `Shared` allocation.
struct SharedHandle(NonNull<Shared>);

// SAFETY: Acts like the `where` clauses say
unsafe impl Send for SharedHandle where std::sync::Arc<Shared>: Send {}
unsafe impl Sync for SharedHandle where std::sync::Arc<Shared>: Sync {}

impl SharedHandle {
    fn get(&self) -> &Shared {
        // SAFETY: We hold a refcount.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for SharedHandle {
    fn drop(&mut self) {
        // SAFETY: Releases the count this handle owns.
        unsafe { decref(self.0.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::task::Wake;

    use futures::FutureExt;

    use super::*;

    /// A waker that counts how many times it has been woken.
    struct CountingWaker(AtomicUsize);

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn poll_once<F: Future>(fut: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
        fut.as_mut().poll(&mut Context::from_waker(waker))
    }

    /// A future that stashes its waker and completes with `value` once externally triggered.
    struct Triggered {
        value: u32,
        fired: Arc<Mutex<(bool, Option<Waker>)>>,
    }

    fn triggered(value: u32) -> (Triggered, Arc<Mutex<(bool, Option<Waker>)>>) {
        let state = Arc::new(Mutex::new((false, None)));
        (
            Triggered {
                value,
                fired: state.clone(),
            },
            state,
        )
    }

    fn fire(state: &Arc<Mutex<(bool, Option<Waker>)>>) {
        let waker = {
            let mut guard = state.lock();
            guard.0 = true;
            guard.1.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    impl Future for Triggered {
        type Output = u32;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
            let mut guard = self.fired.lock();
            if guard.0 {
                Poll::Ready(self.value)
            } else {
                guard.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    #[test]
    fn test_join_all_immediate() {
        let futs: Vec<_> = (0..100u32).map(|i| async move { i * 2 }).collect();
        let mut j = Box::pin(join_all(futs));
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(
            poll_once(&mut j, &waker),
            Poll::Ready((0..100u32).map(|i| i * 2).collect::<Vec<_>>())
        );
    }

    /// Outputs are moved out exactly once for joins on both sides of `MiniVec`'s inline limit.
    ///
    /// `MiniVec` stores up to `MAX_INLINE_CAP` (14) elements inline and converts those into a
    /// `Vec` without copying. Wider arrays use an extended layout with a heap header that `Vec`
    /// cannot adopt. The widths below cover the last inline size, the first extended size and a
    /// much wider join.
    #[test]
    fn test_wide_join_outputs_are_not_dropped_or_leaked() {
        struct Counted(Arc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        for n in [14, 15, 100] {
            let drops = Arc::new(AtomicUsize::new(0));
            let futs: Vec<_> = (0..n)
                .map(|_| {
                    let drops = drops.clone();
                    async move { Counted(drops) }
                })
                .collect();
            let mut j = Box::pin(join_all(futs));
            let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
            let Poll::Ready(out) = poll_once(&mut j, &waker) else {
                panic!("not ready")
            };
            assert_eq!(out.len(), n);
            assert_eq!(
                drops.load(Ordering::SeqCst),
                0,
                "output dropped during assembly, n={n}"
            );
            drop(out);
            assert_eq!(
                drops.load(Ordering::SeqCst),
                n,
                "output leaked or double dropped, n={n}"
            );
        }
    }

    #[test]
    fn test_join_all_empty() {
        let mut j = Box::pin(join_all(Vec::<std::future::Ready<u32>>::new()));
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(poll_once(&mut j, &waker), Poll::Ready(vec![]));
    }

    #[test]
    fn test_results_in_input_order_despite_reverse_completion() {
        let (f0, s0) = triggered(0);
        let (f1, s1) = triggered(1);
        let (f2, s2) = triggered(2);
        let mut j = Box::pin(join_all(vec![f0, f1, f2]));

        let counter = Arc::new(CountingWaker(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);

        // Complete in reverse order, polling in between.
        fire(&s2);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);
        fire(&s1);
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);
        fire(&s0);
        assert_eq!(poll_once(&mut j, &waker), Poll::Ready(vec![0, 1, 2]));
    }

    #[test]
    fn test_wake_is_edge_triggered_per_future() {
        let (f0, s0) = triggered(0);
        let (f1, _s1) = triggered(1);
        let mut j = Box::pin(join_all(vec![f0, f1]));

        let counter = Arc::new(CountingWaker(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);

        // Two wakes of the same future before the next poll: only one parent wake.
        fire(&s0);
        let stray = {
            let guard = s0.lock();
            guard.1.clone()
        };
        assert!(stray.is_none(), "waker should have been consumed by fire");
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }

    /// A branch that counts its polls and stays pending, parking its waker for the test.
    struct PollCounter(Arc<(AtomicUsize, Mutex<Option<Waker>>)>);

    impl Future for PollCounter {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            self.0.0.fetch_add(1, Ordering::SeqCst);
            *self.0.1.lock() = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    /// Duplicate wakes must be ignored both below and above `WOKEN_BITSET_THRESHOLD`, including
    /// wakes recorded before the bitset existed, and a drain must forget every wake it took.
    #[test]
    fn test_wake_dedup_across_bitset_threshold() {
        let n = WOKEN_BITSET_THRESHOLD * 2;
        let states: Vec<_> = (0..n)
            .map(|_| Arc::new((AtomicUsize::new(0), Mutex::new(None))))
            .collect();
        let mut j = Box::pin(join_all(states.iter().cloned().map(PollCounter)));
        let waker = Waker::noop();
        let polls = || {
            states
                .iter()
                .map(|s| s.0.load(Ordering::SeqCst))
                .collect::<Vec<_>>()
        };

        assert_eq!(poll_once(&mut j, waker), Poll::Pending);
        let branch_wakers: Vec<Waker> =
            states.iter().map(|s| s.1.lock().clone().unwrap()).collect();

        for _ in 0..2 {
            branch_wakers[..WOKEN_BITSET_THRESHOLD]
                .iter()
                .for_each(Waker::wake_by_ref);
        }
        for _ in 0..2 {
            branch_wakers.iter().for_each(Waker::wake_by_ref);
        }
        assert_eq!(poll_once(&mut j, waker), Poll::Pending);
        assert_eq!(
            polls(),
            vec![2; n],
            "each branch should be polled by the opening sweep and then once by the drain"
        );

        branch_wakers.iter().for_each(Waker::wake_by_ref);
        assert_eq!(poll_once(&mut j, waker), Poll::Pending);
        assert_eq!(
            polls(),
            vec![3; n],
            "the drain should have forgotten every wake it took"
        );
    }

    #[test]
    fn test_try_join_all_ok() {
        let futs: Vec<_> = (0..50u32)
            .map(|i| async move { Ok::<_, String>(i) })
            .collect();
        let mut j = Box::pin(try_join_all(futs));
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(
            poll_once(&mut j, &waker),
            Poll::Ready(Ok((0..50).collect::<Vec<_>>()))
        );
    }

    /// The remaining futures must be dropped as soon as one future fails. This is the semantic
    /// difference to `futures`' `try_join_all`, and this test fails with that version.
    #[test]
    fn test_try_join_all_cancels_eagerly() {
        struct SetOnDrop(Arc<AtomicUsize>);
        impl Drop for SetOnDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let mut futs = Vec::new();
        for _ in 0..10 {
            let guard = SetOnDrop(drops.clone());
            futs.push(
                async move {
                    let _guard = guard;
                    // Never completes.
                    poll_fn(|_| Poll::<()>::Pending).await;
                    Ok(())
                }
                .boxed_local(),
            );
        }
        futs.push(async move { Err("boom") }.boxed_local());

        let mut j = Box::pin(try_join_all(futs));
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(poll_once(&mut j, &waker), Poll::Ready(Err("boom")));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            10,
            "all pending futures must be dropped when the join fails"
        );
    }

    /// Dropping the join must cancel (drop) all the futures.
    #[test]
    fn test_drop_cancels() {
        struct SetOnDrop(Arc<AtomicUsize>);
        impl Drop for SetOnDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let futs: Vec<_> = (0..5)
            .map(|_| {
                let guard = SetOnDrop(drops.clone());
                async move {
                    let _guard = guard;
                    poll_fn(|_| Poll::<()>::Pending).await
                }
            })
            .collect();

        let mut j = Box::pin(join_all(futs));
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);
        drop(j);
        assert_eq!(drops.load(Ordering::SeqCst), 5);
    }

    /// A waker that outlives the join must stay safe to invoke.
    #[test]
    fn test_stray_waker_after_drop() {
        let (f0, s0) = triggered(0);
        let mut j = Box::pin(join_all(vec![f0]));

        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);

        // Steal the branch waker that `f0` stashed, then kill the join.
        let stray = s0.lock().1.take().unwrap();
        drop(j);

        // Must not crash or touch freed memory (the Arc<Shared> is kept alive by the waker).
        stray.wake_by_ref();
        #[expect(clippy::waker_clone_wake, reason = "that's the point")]
        stray.clone().wake();
    }

    struct SelfWake(bool);
    impl Future for SelfWake {
        type Output = u32;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
            if self.0 {
                Poll::Ready(7)
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// In tracked mode, a branch that wakes itself once and then completes is finished within a
    /// single poll: the initial sweep polls it, and the one drain pass then polls the index it
    /// re-woke, by which point it is ready. (A branch that re-wakes itself on *every* poll is
    /// instead deferred to the next poll; see `tracked_mode_does_not_livelock_on_self_waking_branch`.)
    #[test]
    fn test_self_wake_is_drained_in_same_poll_when_tracked() {
        let mut j = Box::pin(JoinAll {
            // Forced into tracked mode
            core: JoinCore::with_threshold(vec![SelfWake(false)].into_iter(), 1),
        });
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(poll_once(&mut j, &waker), Poll::Ready(vec![7]));
    }

    /// In small (untracked) mode the branches hold the join's own waker, so a self-wake goes to
    /// the executor and completion needs a re-poll, like `futures`' `join_all`.
    #[test]
    fn test_self_wake_defers_to_repoll_when_small() {
        let mut j = Box::pin(join_all(vec![SelfWake(false)]));
        let counter = Arc::new(CountingWaker(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert_eq!(poll_once(&mut j, &waker), Poll::Ready(vec![7]));
    }

    /// The tracked path must also work end to end with real cross-"thread" completions.
    #[test]
    fn test_tracked_mode_large_set() {
        let mut futs = Vec::new();
        let mut states = Vec::new();
        for i in 0..40u32 {
            let (f, s) = triggered(i);
            futs.push(f);
            states.push(s);
        }
        let mut j = Box::pin(join_all(futs));
        let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
        assert_eq!(poll_once(&mut j, &waker), Poll::Pending);
        // Fire in a scrambled order, polling intermittently.
        let mut result = Poll::Pending;
        for (i, s) in states.iter().enumerate().rev() {
            fire(s);
            if i % 3 == 0 {
                result = poll_once(&mut j, &waker);
            }
        }
        if result.is_pending() {
            result = poll_once(&mut j, &waker);
        }
        assert_eq!(result, Poll::Ready((0..40u32).collect::<Vec<_>>()));
    }

    /// Deterministic reproduction of the tracked-mode livelock.
    ///
    /// Ensures that a future which always reschedules itself doesn't cause a livelock.
    #[test]
    fn tracked_mode_does_not_livelock_on_self_waking_branch() {
        use std::sync::atomic::AtomicBool;

        /// Wakes itself and returns `Pending` on every poll; never completes.
        struct AlwaysReschedule;
        impl Future for AlwaysReschedule {
            type Output = ();
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }

        let returned = Arc::new(AtomicBool::new(false));
        let returned_thread = returned.clone();
        let worker = std::thread::spawn(move || {
            // `>= TRACKING_THRESHOLD` forces tracked mode, where the bug lives.
            let futs: Vec<_> = (0..16).map(|_| AlwaysReschedule).collect();
            let mut j = Box::pin(join_all(futs));
            let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
            // A single poll must return `Pending`; it must not spin forever
            // just because the branches keep re-waking themselves.
            assert!(
                j.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            returned_thread.store(true, Ordering::SeqCst);
        });

        std::thread::sleep(std::time::Duration::from_secs(3));
        assert!(
            returned.load(Ordering::SeqCst),
            "JoinCore::poll livelocked: a single poll never returned when a \
             branch re-wakes itself every poll (as coop-throttled tokio \
             futures do)"
        );
        worker.join().unwrap();
    }

    // ===== concurrency stress tests on a real multi-threaded runtime =====

    /// Drives `join_all` over many futures that complete concurrently from
    /// other tasks. Exercises the tracked-mode wake/drain path, which the
    /// single-threaded manual-poll tests above cannot reach.
    ///
    /// The join is run on a *spawned* task and awaited with a timeout from a
    /// different task on purpose: a livelock inside a single `poll()` call
    /// (e.g. a branch future that self-wakes every poll) pins one worker
    /// thread and cannot be interrupted by a same-task timeout, so the timeout
    /// must live on a separate task to observe it.
    #[test]
    #[cfg_attr(miri, ignore = "stress test, far too slow interpreted")]
    fn stress_join_all_concurrent_completion() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            for iter in 0..200u64 {
                for n in [1usize, 2, 7, 8, 9, 16, 33, 100] {
                    let mut rxs = Vec::new();
                    let mut txs: Vec<tokio::sync::oneshot::Sender<()>> = Vec::new();
                    for _ in 0..n {
                        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
                        rxs.push(rx);
                        txs.push(tx);
                    }
                    // Fire each completion from its own task; the scheduler
                    // interleaves the sends (and thus the wakes) arbitrarily.
                    for (k, tx) in txs.into_iter().enumerate() {
                        let spins = (k.wrapping_mul(2654435761).wrapping_add(iter as usize)) % 7;
                        tokio::spawn(async move {
                            for _ in 0..spins {
                                tokio::task::yield_now().await;
                            }
                            let _ = tx.send(());
                        });
                    }
                    let futs: Vec<_> = rxs
                        .into_iter()
                        .enumerate()
                        .map(|(i, rx)| async move {
                            rx.await.unwrap();
                            i
                        })
                        .collect();
                    let handle = tokio::spawn(join_all(futs));
                    let out = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
                        .await
                        .unwrap_or_else(|_| panic!("HANG in join_all n={n} iter={iter}"))
                        .unwrap();
                    assert_eq!(out, (0..n).collect::<Vec<_>>(), "n={n} iter={iter}");
                }
            }
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "stress test, far too slow interpreted")]
    fn stress_try_join_all_error_cancels() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            for _ in 0..200u64 {
                for n in [8usize, 16, 50] {
                    let drops = Arc::new(AtomicUsize::new(0));
                    struct SetOnDrop(Arc<AtomicUsize>);
                    impl Drop for SetOnDrop {
                        fn drop(&mut self) {
                            self.0.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    let mut futs: Vec<
                        futures::future::LocalBoxFuture<'static, Result<usize, &'static str>>,
                    > = Vec::new();
                    for _ in 0..n {
                        let g = SetOnDrop(drops.clone());
                        futs.push(
                            async move {
                                let _g = g;
                                poll_fn(|_| Poll::<()>::Pending).await;
                                Ok(0)
                            }
                            .boxed_local(),
                        );
                    }
                    futs.push(async move { Err("boom") }.boxed_local());
                    let res = try_join_all(futs).await;
                    assert_eq!(res, Err("boom"));
                    assert_eq!(drops.load(Ordering::SeqCst), n);
                }
            }
        });
    }

    /// A branch that re-wakes from inside its own `poll` a fixed number of times.
    ///
    /// This is the interesting case for the `woken` bookkeeping: `JoinCore::poll` releases
    /// the lock before draining, so a branch waking itself during the drain races whatever
    /// the drain is doing to that branch's record.
    struct SelfWakingBranch {
        idx: usize,
        left: u32,
    }

    impl Future for SelfWakingBranch {
        type Output = usize;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
            let this = self.get_mut();
            if this.left == 0 {
                return Poll::Ready(this.idx);
            }
            this.left -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    /// A branch woken entirely from outside, so that its wakes can be issued in parallel
    /// with the drain rather than from inside it.
    struct Slot {
        done: AtomicBool,
        waker: Mutex<Option<Waker>>,
    }

    struct Driven {
        idx: usize,
        slot: Arc<Slot>,
    }

    impl Future for Driven {
        type Output = usize;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
            let this = self.get_mut();
            // Park before testing `done`, so a driver that flips it in between still finds a
            // waker to fire.
            *this.slot.waker.lock() = Some(cx.waker().clone());
            if this.slot.done.load(Ordering::Acquire) {
                return Poll::Ready(this.idx);
            }
            Poll::Pending
        }
    }

    /// Wide-join stress over the `woken` bookkeeping, in two phases.
    ///
    /// The deferred materializer joins one future per stale artifact path, which reaches
    /// ~200k branches on a large buck-out. Both phases hold the woken list near full width
    /// for the whole run, which is the state a production daemon was found in and what makes
    /// any per-wake cost that scales with the list length dominate everything else.
    ///
    /// Phase 1 sweeps width using self-waking branches: the drain takes the list, then every
    /// branch it polls re-adds itself. This measures the bookkeeping cost cleanly, but it
    /// cannot produce lock contention -- `join_all` is a single tokio task, so the drain
    /// always runs on one worker, and a self-wake is issued from inside that same poll. The
    /// worker count is irrelevant to that.
    ///
    /// Phase 2 therefore delivers every wake from a pool of OS threads running in parallel
    /// with the drain, which is the only way to race `take()` across a thread boundary --
    /// the case where a cleared record and an in-flight wake can lose each other.
    ///
    /// Phase 2 is a correctness probe, not a second benchmark. Its timing is reported for
    /// completeness but should not be read as a speedup: externally woken branches do not
    /// re-add themselves on poll, so the list does not stay wide and the scan never
    /// dominates. Lock contention is measurably not the mechanism here -- the cost comes
    /// from the drain regenerating a full-width list on a single thread, which phase 1
    /// captures.
    ///
    /// A dropped wake shows up as a hang rather than a wrong answer, hence the timeouts.
    #[test]
    #[cfg_attr(miri, ignore = "stress test, far too slow interpreted")]
    fn stress_wide_join_scaling_and_contention() {
        const SELF_WAKES: u32 = 2;
        const DRIVER_THREADS: usize = 8;
        const SPURIOUS_ROUNDS: u32 = 2;
        const CONTENDED_N: usize = 50_000;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .enable_time()
            .build()
            .unwrap();

        let mut report = String::new();
        rt.block_on(async {
            for n in [1_000usize, 10_000, 50_000, 100_000, 200_000] {
                let futs: Vec<_> = (0..n)
                    .map(|idx| SelfWakingBranch {
                        idx,
                        left: SELF_WAKES,
                    })
                    .collect();

                let start = std::time::Instant::now();
                let handle = tokio::spawn(join_all(futs));
                let out = tokio::time::timeout(std::time::Duration::from_secs(180), handle)
                    .await
                    .unwrap_or_else(|_| panic!("HANG in wide join, n={n} -- a wake was lost"))
                    .unwrap();
                let elapsed = start.elapsed();

                assert_eq!(out, (0..n).collect::<Vec<_>>(), "wrong outputs at n={n}");
                report.push_str(&format!("uncontended n={n:>7}  elapsed={elapsed:>12.1?}\n"));
            }

            let n = CONTENDED_N;
            let slots: Vec<Arc<Slot>> = (0..n)
                .map(|_| {
                    Arc::new(Slot {
                        done: AtomicBool::new(false),
                        waker: Mutex::new(None),
                    })
                })
                .collect();
            let futs: Vec<_> = slots
                .iter()
                .enumerate()
                .map(|(idx, slot)| Driven {
                    idx,
                    slot: slot.clone(),
                })
                .collect();

            let start = std::time::Instant::now();
            let handle = tokio::spawn(join_all(futs));

            // Interleaved rather than contiguous shares, so the threads' wakes land on
            // adjacent indexes and contend the same bookkeeping rather than partitioning it.
            let drivers: Vec<_> = (0..DRIVER_THREADS)
                .map(|t| {
                    let mine: Vec<Arc<Slot>> = slots
                        .iter()
                        .skip(t)
                        .step_by(DRIVER_THREADS)
                        .cloned()
                        .collect();
                    std::thread::spawn(move || {
                        // Take one waker clone per slot up front, after the join's opening
                        // sweep has registered them all. Cloning per wake instead puts a mutex
                        // and a refcount bump in front of every wake, which throttles the
                        // drivers enough that the drain keeps up and the list never grows --
                        // hiding the very cost this phase exists to measure.
                        let mut wakers = Vec::with_capacity(mine.len());
                        for s in &mine {
                            loop {
                                if let Some(w) = s.waker.lock().clone() {
                                    wakers.push(w);
                                    break;
                                }
                                std::thread::yield_now();
                            }
                        }
                        for _ in 0..SPURIOUS_ROUNDS {
                            for w in &wakers {
                                w.wake_by_ref();
                            }
                        }
                        for s in &mine {
                            s.done.store(true, Ordering::Release);
                        }
                        for w in &wakers {
                            w.wake_by_ref();
                        }
                    })
                })
                .collect();

            let out = tokio::time::timeout(std::time::Duration::from_secs(300), handle)
                .await
                .unwrap_or_else(|_| panic!("HANG in contended join -- a wake was lost"))
                .unwrap();
            let elapsed = start.elapsed();
            for d in drivers {
                d.join().unwrap();
            }

            assert_eq!(
                out,
                (0..n).collect::<Vec<_>>(),
                "wrong outputs under contention"
            );
            report.push_str(&format!(
                "contended  n={n:>7}  elapsed={elapsed:>12.1?}  ({DRIVER_THREADS} driver threads)\n"
            ));
        });
        drop(std::fs::write("/tmp/join_stress.txt", &report));
        println!("{report}");
    }
}
