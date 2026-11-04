/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A task stored by Dice that is shared for all transactions at the same version
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc as StdArc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use allocative::Allocative;
use dice_error::cycles::CycleError;
use dice_futures::atomic_waker_set::AtomicWakerSet;
use dice_futures::atomic_waker_set::AtomicWakerSetEntry;
use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::DropcancelHandle;
use dice_futures::spawn::CancellableFutureSpawner;
use dice_futures::spawn::prepare_detached_cancellation;
use dice_futures::spawner::Spawner;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe;
use dupe::Dupe_;
use futures::FutureExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;

use super::handle::DiceTaskHandle;
use crate::epoch::cycles::DetectedCycle;
use crate::epoch::cycles::TaskCycles;
use crate::epoch::task::PreviouslyCancelledTask;
use crate::epoch::task::promise::DicePromise;
use crate::epoch::worker::WorkerCancelled;
use crate::epoch::worker::WorkerResult;
use crate::introspection::DiceTaskState;
use crate::key::DiceKey;
use crate::key::ParentKey;
use crate::value::MaybeResidentComputedValue;

/// The shared, reference-counted state for computing a single key at a single version.
///
/// It behaves like a shared, spawned future: many dependents can await the same in-flight
/// computation, but none of them drive it. The value is produced by a separately spawned worker
/// (see `spawn_prepared_task`) that reports the result through a `DiceTaskCompletionHandle`.
/// Polling a dependent only registers a waker; it never does real work, and a waker is only woken
/// once the result is ready or the generation is cancelled/restarted.
///
/// Each `DicePromise` / `DiceTaskDependentFuture` is a dependent ("strong reference"), counted by
/// `strong_count`. When the last dependent goes away the in-flight computation is cancelled, but
/// the `DiceTask` is not discarded; the next dependent to arrive *restarts* it in place as a new
/// "generation", so the same `DiceTask` can outlive several cancelled computations. See
/// `depended_on_by` and the `current_generation` / `terminated_generation` fields for how
/// generations encode the task's state.
#[derive(Allocative, Clone_, Dupe_)]
#[repr(transparent)]
pub(crate) struct DiceTask<T = MaybeResidentComputedValue> {
    pub(crate) internal: crate::arc::Arc<DiceTaskInternal<T>>,
}

impl<T> DiceTask<T> {
    pub(crate) fn as_ref(&self) -> DiceTaskRef<'_, T> {
        DiceTaskRef {
            internal: self.internal.borrow_arc(),
        }
    }

    #[cfg(test)]
    pub(crate) fn depended_on_by(&self, k: ParentKey) -> DiceTaskDependedOnByResult<'_, T>
    where
        T: Dupe + Send + Sync + 'static,
    {
        self.as_ref().depended_on_by(k)
    }

    #[cfg(test)]
    pub(crate) fn waiters_count(&self) -> u32 {
        self.as_ref().waiters_count()
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        self.as_ref().is_ready()
    }

    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.as_ref().is_cancelled()
    }

    #[cfg(test)]
    pub(crate) fn is_pending(&self) -> bool {
        self.as_ref().is_pending()
    }
}

#[derive(Clone_, Copy_, Dupe_)]
pub(crate) struct DiceTaskRef<'d, T = MaybeResidentComputedValue> {
    pub(crate) internal: crate::arc::ArcBorrow<'d, DiceTaskInternal<T>>,
}

impl<T> DiceTaskRef<'_, T> {
    pub(crate) fn clone_arc(self) -> DiceTask<T> {
        DiceTask {
            internal: self.internal.clone_arc(),
        }
    }
}

#[derive(Allocative)]
#[allocative(skip)]
pub(crate) struct DiceTaskInternal<T = MaybeResidentComputedValue> {
    pub(crate) key: DiceKey,
    /// Because things may be cancelled, it may take multiple attempts ("generations") before an
    /// execution is actually driven to completion. These fields track that.
    ///
    /// The `started_generation` is the most recently created generation. Normally, it is the case
    /// that `current_generation` is at most `terminated_generation + 1`, but in the face of rapid
    /// cancellation that is not strictly required.
    ///
    /// The `starter_lock` holds the cancellation handle for the most recently started generation
    /// (previous generations have already been cancelled).
    ///
    /// You should primarily think of the lock as protecting "generation start" events, ie
    /// preventing multiple threads from attempting to start the same generation. However, it is not
    /// only held by the generation starting code; it is also needed when there is other code that
    /// needs to synchronize itself with generation start logic.
    ///
    /// A new one is created when the strong count goes 0 -> 1, and the worker for the generation is
    /// cancelled when the strong count goes 1 -> 0.
    started_generation: AtomicU32,
    starter_lock: Mutex<Option<DropcancelHandle>>,
    /// The computed value, ie the eventual output of the task.
    ///
    /// This is set effectively whenever the value is ready. After it is set, no new generations
    /// will be started, though previously started ones may still be running.
    maybe_value: OnceLock<T>,
    /// The number of things waiting on the the task.
    ///
    /// When this is zero, the task is idle or its most recent generation has been cancelled;
    /// incrementing this `0 -> 1` requires starting a new generation.
    ///
    /// Changing this `0 -> 1` also requires a release store (and subsequent acquire loads) so that
    /// the changes to `Critical` and other fields in this type are visible to other waiters later
    /// grabbing handles by incrementing this.
    strong_count: AtomicU32,
    /// The most recent generation that has fully terminated, either by succeeding or by cancelling.
    ///
    /// Cancellation is an async operation, so there is some delay between strong count going to
    /// zero and the termination incrementing.
    ///
    /// The only sort of non-trivial ordering thing here is that incrementing
    /// `terminated_generation` is the last thing that the execution of a generation does, and it
    /// does so with a `Release` store; if it's going to produce any result in `maybe_value`, that
    /// will be populated first (and can be made visible by `acquire` loading the
    /// `terminated_generation`).
    terminated_generation: AtomicU32,

    /// Wakers of everything currently parked on this task; as a mental model this can be thought of
    /// as having a number of entries equalling the strong count.
    ///
    /// In practice though there can be a lot of other stuff here (such as `TerminationObserver`s,
    /// which don't hold a strong count), and wakers here may actually be waiting on different
    /// generations. We don't attempt to be smart; when we wake anything, we wake everything, and
    /// things that didn't need to be woken just re-register themselves.
    ///
    /// Each entry names the key that is waiting, see `for_each_waiting_key`.
    wakers: AtomicWakerSet<ParentKey>,
    /// The cycles this task has been found to be a key of, see `mark_in_cycle`.
    cycles: TaskCycles,
}

enum ReadValueResult<'d, T = MaybeResidentComputedValue> {
    Finished(&'d T),
    Pending { terminated_generation: u32 },
}

/// Future-like type that waits on a task.
///
/// Can either be used to wait on a specific generation or on general completion.
#[pin_project::pin_project(PinnedDrop)]
pub(crate) struct TaskWaiter<'d, T = MaybeResidentComputedValue> {
    task: DiceTaskRef<'d, T>,
    #[pin]
    waiter: AtomicWakerSetEntry<ParentKey>,
}

impl<'d, T> TaskWaiter<'d, T> {
    /// When using this to wait until task completion, poll this.
    ///
    /// Note that it's very ill-advised to do this if you don't hold a strong count to the task, as
    /// without one completion might never happen.
    ///
    /// Fails if the waiting key has been found to be in a cycle with this task's, see
    /// `DetectedCycle`.
    fn poll_complete(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<&'d T, CycleError>> {
        let this = self.as_mut().project();
        let internal = this.task.internal.get();
        let waiting = *this.waiter.data();

        // Everything here is rechecked after registering, since `mark_in_cycle` wakes only the
        // waiters registered at the time, just as completion does.
        let check = || {
            // The cycle is checked before the value so that every key of the cycle that was
            // waiting when it was found gets the cycle error: the first of them to be woken by
            // the detection may well have finished, with an error value of its own, by the time
            // the others poll.
            if let Some(error) = internal.cycles.error_for(waiting, internal.key) {
                return Poll::Ready(Err(error));
            }
            if let Some(r) = internal.maybe_value.get() {
                Poll::Ready(Ok(r))
            } else {
                Poll::Pending
            }
        };

        // Fast-path to avoid waker registration
        if let Poll::Ready(r) = check() {
            return Poll::Ready(r);
        }
        // SAFETY: We're pinned and this is the only waker set we use
        unsafe {
            this.waiter.register(&internal.wakers, cx.waker().clone());
        }
        check()
    }

    /// When using this to wait until a particular generation completes, poll this.
    fn poll_at_generation(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        generation: u32,
    ) -> Poll<WorkerResult<&'d T>> {
        let this = self.as_mut().project();
        let internal = this.task.internal.get();

        let check = || {
            if internal.terminated_generation.load(Ordering::Acquire) >= generation {
                Poll::Ready(internal.maybe_value.get().ok_or(WorkerCancelled))
            } else {
                Poll::Pending
            }
        };

        // Fast-path to avoid waker registration
        if let Poll::Ready(r) = check() {
            return Poll::Ready(r);
        }
        // SAFETY: We're pinnned and this is the only waker set we use
        unsafe {
            this.waiter.register(&internal.wakers, cx.waker().clone());
        }
        check()
    }
}

#[pin_project::pinned_drop]
impl<'d, T> PinnedDrop for TaskWaiter<'d, T> {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: We're pinnned and this is the only waker set we use
        unsafe {
            let this = self.project();
            this.waiter.disconnect(&this.task.internal.wakers);
        }
    }
}

/// Result of registering as a dependent of a task via `depended_on_by`.
pub(crate) enum DiceTaskDependedOnByResult<'d, T = MaybeResidentComputedValue> {
    Finished(&'d T),
    Pending(DicePromise<'d, T>),
    /// This caller won the race to start an idle or cancelled task. The caller must spawn the
    /// worker after awaiting the previous generation, which is already terminated for an idle task.
    NeedsRestart(PreparedDiceTask<'d, T>, PreviouslyCancelledTask<T>),
}

impl<'d, T> DiceTaskDependedOnByResult<'d, T> {
    /// Unwrap to a `DicePromise`. `Finished(v)` returns a ready promise; `Pending`
    /// returns the inner promise. Panics on `NeedsRestart` because callers using
    /// `unwrap` haven't agreed to drive the restart. Used in tests where the
    /// caller knows the task is in-flight.
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn unwrap(self) -> DicePromise<'d, T> {
        match self {
            DiceTaskDependedOnByResult::Pending(p) => p,
            DiceTaskDependedOnByResult::Finished(v) => DicePromise::ready(v),
            DiceTaskDependedOnByResult::NeedsRestart(_, _) => {
                panic!("DiceTaskDependedOnByResult::unwrap called on NeedsRestart");
            }
        }
    }
}

/// Everything needed to start (or restart) one generation of a `DiceTask`: hand it to
/// `spawn_prepared_task` to launch the worker and obtain a `DicePromise`. All three members are
/// minted for the same generation.
pub(crate) struct PreparedDiceTask<'d, T = MaybeResidentComputedValue> {
    /// Lets the worker report the value or cancellation for this generation.
    pub(crate) completion_handle: DiceTaskCompletionHandle<T>,
    /// The first dependent of this generation; becomes the promise returned to the spawner.
    pub(crate) dependent_future: DiceTaskDependentFuture<'d, T>,
    /// Spawns the cancellable worker future for this generation.
    pub(crate) task_spawner: DiceTaskSpawner,
}

impl<'d, T> PreparedDiceTask<'d, T> {
    #[cfg(test)]
    pub(crate) fn task(&self) -> DiceTaskRef<'d, T> {
        self.dependent_future.0.task
    }
}

impl<T> DiceTask<T> {
    /// Create an idle task. Its first dependent starts it through `depended_on_by`, just as
    /// for a cancelled task. Generation zero has no worker and is already terminated.
    pub(crate) fn new_idle(key: DiceKey) -> Self {
        Self {
            internal: crate::arc::Arc::new(DiceTaskInternal {
                key,
                strong_count: AtomicU32::new(0),
                started_generation: AtomicU32::new(0),
                terminated_generation: AtomicU32::new(0),
                maybe_value: OnceLock::new(),
                wakers: AtomicWakerSet::new(),
                cycles: TaskCycles::default(),
                starter_lock: Mutex::new(None),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn prepare_testing(key: DiceKey) -> PreparedDiceTask<'static, T>
    where
        T: 'static,
    {
        // Spawning a dice task normally needs some kind of arena to allocate the task into; we don't
        // have one, but this is tests, so just leak the thing
        DiceTask::prepare::<()>(
            key,
            ParentKey::None,
            |t| Ok(Box::leak(Box::new(t)).as_ref()),
        )
        .unwrap()
    }

    /// Prepare a task for execution, with `parent` as its first dependent.
    ///
    /// Takes an alloc callback which should allocate the task into something long lived so that
    /// further code can use references to it.
    ///
    /// Note: This API is just a little bit unsound, don't do anything funky in `alloc` by returning
    /// a ref to an unrelated dice task. Should be fine.
    pub(crate) fn prepare<'d, E>(
        key: DiceKey,
        parent: ParentKey,
        alloc: impl FnOnce(DiceTask<T>) -> Result<DiceTaskRef<'d, T>, E>,
    ) -> Result<PreparedDiceTask<'d, T>, E> {
        let (future_spawner, cancellation_handle) = prepare_detached_cancellation();

        let cancellation_handle = cancellation_handle.into_dropcancel();

        let task = Self {
            internal: crate::arc::Arc::new(DiceTaskInternal {
                key,
                strong_count: AtomicU32::new(1),
                // First generation.
                started_generation: AtomicU32::new(1),
                terminated_generation: AtomicU32::new(0),
                maybe_value: OnceLock::new(),
                wakers: AtomicWakerSet::new(),
                cycles: TaskCycles::default(),
                starter_lock: Mutex::new(Some(cancellation_handle)),
            }),
        };

        let task = alloc(task)?;
        debug_assert_eq!(
            task.internal.key, key,
            "alloc returned a ref for the wrong key"
        );

        Ok(PreparedDiceTask {
            task_spawner: DiceTaskSpawner {
                inner: future_spawner,
            },
            dependent_future: DiceTaskDependentFuture(TaskWaiter {
                task,
                waiter: AtomicWakerSetEntry::new(parent),
            }),
            completion_handle: DiceTaskCompletionHandle {
                generation: 1,
                task: task.clone_arc(),
            },
        })
    }
}

impl<'d, T> DiceTaskRef<'d, T> {
    /// `k` depends on this task, returning a `DicePromise` that will complete when this task
    /// completes
    pub(crate) fn depended_on_by(self, k: ParentKey) -> DiceTaskDependedOnByResult<'d, T>
    where
        T: Dupe + Send + Sync + 'static,
    {
        if let Some(v) = self.get_finished_value() {
            return DiceTaskDependedOnByResult::Finished(v);
        }

        let internal = &*self.internal;
        if internal
            .strong_count
            .try_update(Ordering::Relaxed, Ordering::Acquire, |v| {
                if v == 0 { None } else { Some(v + 1) }
            })
            .is_ok()
        {
            // fast path, strong_count > 0: task is in progress
            return DiceTaskDependedOnByResult::Pending(DicePromise::pending(
                DiceTaskDependentFuture(TaskWaiter {
                    task: self,
                    waiter: AtomicWakerSetEntry::new(k),
                }),
            ));
        }

        // strong_count == 0: the task is idle or cancelled and needs a new generation.
        let mut guard = self.internal.starter_lock.lock();

        // Now need to recheck in case we raced with something.
        if let Some(v) = self.get_finished_value() {
            // Something already finished it.
            return DiceTaskDependedOnByResult::Finished(v);
        }

        // We cannot update this 0 -> 1 until after we've stored our generation as current, but we can't do that
        // until we confirm that we are actually the restarter.
        if internal
            .strong_count
            .try_update(Ordering::Relaxed, Ordering::Acquire, |v| {
                if v == 0 { None } else { Some(v + 1) }
            })
            .is_ok()
        {
            // Something already restarted it.
            return DiceTaskDependedOnByResult::Pending(DicePromise::pending(
                DiceTaskDependentFuture(TaskWaiter {
                    task: self,
                    waiter: AtomicWakerSetEntry::new(k),
                }),
            ));
        }

        // We are the restarter.
        let prev_generation = internal.started_generation.load(Ordering::Relaxed);
        let new_generation = prev_generation + 1;
        self.internal
            .started_generation
            .store(new_generation, Ordering::Relaxed);

        // Build new cancellation infrastructure for the next generation.
        let (future_spawner, cancellation_handle) = prepare_detached_cancellation();
        let cancellation_handle = cancellation_handle.into_dropcancel();
        *guard = Some(cancellation_handle);

        // Setting this allows other calls to follow the fast path. We could probably do this just after
        // updating current_generation, but it's a bit easier to reason about if we finish all our updates first.
        self.internal.strong_count.store(1, Ordering::Release);

        drop(guard);

        let previously_cancelled =
            PreviouslyCancelledTask::new(TerminationObserver::new(self, prev_generation));

        let prepared = PreparedDiceTask {
            task_spawner: DiceTaskSpawner {
                inner: future_spawner,
            },
            dependent_future: DiceTaskDependentFuture(TaskWaiter {
                task: self,
                waiter: AtomicWakerSetEntry::new(k),
            }),
            completion_handle: DiceTaskCompletionHandle {
                generation: new_generation,
                task: self.clone_arc(),
            },
        };

        DiceTaskDependedOnByResult::NeedsRestart(prepared, previously_cancelled)
    }

    pub(crate) fn get_finished_value(self) -> Option<&'d T> {
        match self.internal.get().read_value() {
            ReadValueResult::Finished(v) => Some(v),
            ReadValueResult::Pending { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn waiters_count(&self) -> u32 {
        self.internal.strong_count.load(Ordering::Relaxed)
    }

    /// Returns whether the task may have any outstanding work.
    ///
    /// Note that this operation is basically only well-defined on tasks that have been cancelled
    /// (or finished); otherwise there's nothing stopping this from racing with a new worker
    /// starting.
    pub(crate) fn is_pending(&self) -> bool {
        let started = self.internal.started_generation.load(Ordering::Relaxed);
        let terminated = self.internal.terminated_generation.load(Ordering::Relaxed);
        terminated < started
    }

    /// Calls `f` with the key of every computation that is blocked on this task: one that polled
    /// its `DicePromise` for the task, found it pending, and has not been woken since. Requests
    /// from outside of any computation (`ParentKey::None`) are not reported.
    pub(crate) fn for_each_waiting_key(&self, mut f: impl FnMut(DiceKey)) {
        self.internal.wakers.for_each_registered(|parent| {
            if let ParentKey::Some(k) = *parent {
                f(k)
            }
        })
    }

    /// Records that this task's key is in `cycle`, and wakes its dependents so that those which
    /// the cycle fails find out.
    pub(crate) fn mark_in_cycle(&self, cycle: &StdArc<DetectedCycle>) {
        debug_assert!(cycle.keys().any(|k| k == self.internal.key));
        self.internal.cycles.add(cycle);
        self.internal.wakers.wake_all();
    }

    pub(crate) fn introspect_state(&self) -> DiceTaskState {
        match self.internal.read_value() {
            ReadValueResult::Finished(_) => DiceTaskState::Ready,
            ReadValueResult::Pending {
                terminated_generation,
            } if terminated_generation
                < self.internal.started_generation.load(Ordering::Relaxed) =>
            {
                DiceTaskState::InProgress
            }
            ReadValueResult::Pending { .. } => DiceTaskState::Terminated,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        match self.internal.read_value() {
            ReadValueResult::Finished(_) => true,
            ReadValueResult::Pending { .. } => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        // The worker was cancelled iff it produced no value at all (`Pending`, i.e. `maybe_value`
        // unset) and its most recent generation has terminated (`terminated >= started`). `>=`
        // (rather than `==`) is defensive against a brief ordering window where `terminated`
        // could momentarily exceed `started`.
        match self.internal.read_value() {
            ReadValueResult::Finished(_) => false,
            ReadValueResult::Pending {
                terminated_generation,
            } => terminated_generation >= self.internal.started_generation.load(Ordering::Relaxed),
        }
    }

    /// Returns a future that resolves when this task finishes or is fully cancelled
    /// and terminated.
    pub(crate) fn await_termination(self) -> TerminationObserver<T>
    where
        T: Dupe + Send + Sync + 'static,
    {
        let generation = self.internal.started_generation.load(Ordering::Relaxed);
        TerminationObserver::new(self, generation)
    }

    fn task_finished(&'_ self, generation: u32) {
        let prev = self
            .internal
            .terminated_generation
            .fetch_add(1, Ordering::Release);
        // We verify that the generations are cancelled in order, not out of order. To see why this
        // is locally important for correctness, consider what happens if 9 reports completion
        // before 8. 9 must have been cancelled for that to be possible, but we would not be allowed
        // to mark it as terminated, since that would imply that 8 finished too. But when 8
        // eventually does come back as fully cancelled, we won't know to mark 9 as cancelled too,
        // and everything will stall.
        //
        // FIXME(JakobDegen): It's a very non-local property that this is ok; in practice the worker
        // disables cancellations at the right times to make things work out, but it'd be good if
        // that were a lot more obvious.
        assert_eq!(
            prev + 1,
            generation,
            "Generations did not terminate in order"
        );
        let mut guard = self.internal.starter_lock.lock();
        let current = self.internal.started_generation.load(Ordering::Relaxed);
        if current == generation {
            // The cancellation handle corresponds to this task; make sure to discard it to ensure
            // memory is released. Note that the requirement that `guard` is held when `current` is
            // changed makes the generation comparison and this operation "atomic"
            guard
                .take()
                // Convert into a cancellation handle so that we don't do the "cancel" part of the
                // "dropcancel". It wouldn't really matter since the thing being cancelled is done
                // already anyway, but seems mildly cleaner.
                .map(|dropcancel| dropcancel.into_cancellable());
        }
        drop(guard);
        self.internal.wakers.wake_all();
    }

    /// Mark that a generation has terminated successfully.
    fn task_finished_result(&self, generation: u32, value: T) {
        drop(self.internal.maybe_value.set(value));
        self.task_finished(generation);
    }

    /// Mark that a generation has terminated in cancellation.
    fn task_finished_no_result(&self, generation: u32, _token: WorkerCancelled) {
        self.task_finished(generation);
    }

    /// Drop one dependent.
    fn drop_waiter(&self) {
        // Fast path: decrement strong_count if > 1.
        if self
            .internal
            .strong_count
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                if v == 1 { None } else { Some(v - 1) }
            })
            .is_ok()
        {
            return;
        }

        // Slow path: we appear to be the last dependent.
        let mut guard = self.internal.starter_lock.lock();
        let prev = self.internal.strong_count.fetch_sub(1, Ordering::Relaxed);
        if prev != 1 {
            // A new dependent registered.
            return;
        }

        // We were the last dependent. Cancel the spawned task if needed.
        drop(guard.take());
    }
}

impl<T> DiceTaskInternal<T> {
    fn read_value(&self) -> ReadValueResult<'_, T> {
        let terminated = self.terminated_generation.load(Ordering::Acquire);
        match self.maybe_value.get() {
            Some(v) => ReadValueResult::Finished(v),
            None => ReadValueResult::Pending {
                terminated_generation: terminated,
            },
        }
    }
}

pub(crate) fn spawn_prepared_task<'d, S, T: Send + Sync + 'static>(
    task: PreparedDiceTask<'d, T>,
    spawner: &dyn dice_futures::spawner::Spawner<S>,
    ctx: &S,
    f: impl for<'a> FnOnce(&'a mut DiceTaskHandle<'_, T>) -> BoxFuture<'a, ()> + Send,
) -> DicePromise<'d, T> {
    use dice_futures::owning_future::OwningFuture;
    let PreparedDiceTask {
        completion_handle,
        dependent_future,
        task_spawner,
    } = task;

    task_spawner.spawn_cancellable(
        |cancellations| {
            let handle = DiceTaskHandle::new(completion_handle, cancellations);
            OwningFuture::new(handle, f).boxed()
        },
        spawner,
        ctx,
    );

    DicePromise::pending(dependent_future)
}

/// Spawns a task outside of any cache, returning it along with its first dependent, which keeps
/// it running for as long as it is held.
#[cfg(test)]
pub(crate) fn spawn_dice_task<S>(
    key: DiceKey,
    spawner: &dyn dice_futures::spawner::Spawner<S>,
    ctx: &S,
    f: impl for<'a> FnOnce(&'a mut DiceTaskHandle) -> BoxFuture<'a, ()> + Send,
) -> (DiceTask, DicePromise<'static>) {
    let prepared_task = DiceTask::prepare_testing(key);
    let task = prepared_task.task().clone_arc();
    let promise = spawn_prepared_task(prepared_task, spawner, ctx, f);
    (task, promise)
}

pub struct DiceTaskSpawner {
    pub(crate) inner: CancellableFutureSpawner,
}

impl DiceTaskSpawner {
    pub(crate) fn spawn_cancellable<S>(
        self,
        f: impl for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, ()> + Send,
        spawner: &dyn Spawner<S>,
        ctx: &S,
    ) {
        self.inner.spawn(f, spawner, ctx);
    }
}

/// The sole right to finalize one generation of a `DiceTask`. Held by the spawned worker (inside
/// `DiceTaskHandle`) or by an in-flight sync projection. Finalization is generation-checked: if the
/// task has since been restarted, `completed`/`terminated` for this stale generation are no-ops, so
/// a slow or duplicate completion can't clobber a newer computation.
pub(crate) struct DiceTaskCompletionHandle<T = MaybeResidentComputedValue> {
    /// The generation this handle is allowed to finalize.
    pub(super) generation: u32,
    pub(super) task: DiceTask<T>,
}

impl<T> DiceTaskCompletionHandle<T> {
    pub(crate) fn cancelled(self, token: WorkerCancelled) {
        self.task
            .as_ref()
            .task_finished_no_result(self.generation, token);
    }

    pub(crate) fn completed(self, v: T) {
        self.task.as_ref().task_finished_result(self.generation, v);
    }
}

impl<T> Drop for DiceTaskCompletionHandle<T> {
    fn drop(&mut self) {
        // Intentionally does nothing, and this is important because of the FIXME in the sync case
        // below
    }
}

/// A dependent's view of an in-flight `DiceTask`, pollable for its result.
// This is exactly a `TaskWaiter` with the additional semantics that it holds a strong count (and
// releases one on drop).
#[pin_project::pin_project(PinnedDrop)]
pub(crate) struct DiceTaskDependentFuture<'d, T = MaybeResidentComputedValue>(
    #[pin] TaskWaiter<'d, T>,
);

impl<'d, T> DiceTaskDependentFuture<'d, T> {
    pub(crate) fn task(&self) -> DiceTaskRef<'d, T> {
        self.0.task
    }
}

#[pin_project::pinned_drop]
impl<'d, T> PinnedDrop for DiceTaskDependentFuture<'d, T> {
    fn drop(self: Pin<&mut Self>) {
        self.task().drop_waiter();
    }
}

impl<'d, T> Future for DiceTaskDependentFuture<'d, T> {
    type Output = Result<&'d T, CycleError>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.project().0.poll_complete(cx)
    }
}

/// A future that resolves when the underlying task is complete and idle.
// Unlike `DiceTaskDependentFuture`, this does not hold a strong count. It's also less well
// optimized.
pub(crate) struct TerminationObserver<T = MaybeResidentComputedValue>(
    BoxFuture<'static, Option<T>>,
);

impl<T> TerminationObserver<T> {
    fn new(t: DiceTaskRef<'_, T>, generation: u32) -> Self
    where
        T: Dupe + Send + Sync + 'static,
    {
        let task = t.clone_arc();
        TerminationObserver(
            async move {
                let mut wait = std::pin::pin!(TaskWaiter {
                    task: task.as_ref(),
                    waiter: AtomicWakerSetEntry::new(ParentKey::None),
                });
                std::pin::pin!(std::future::poll_fn(|cx| {
                    TaskWaiter::poll_at_generation(wait.as_mut(), cx, generation)
                }))
                .await
                .ok()
                .map(|t| t.dupe())
            }
            .boxed(),
        )
    }
}

impl<T> Future for TerminationObserver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

#[cfg(test)]
pub(crate) mod testing_helpers {
    use crate::api::invalidation_tracking::InvalidationSources;
    use crate::api::key::Key;
    use crate::core::graph::revision::Revision;
    use crate::epoch::task::dice::DiceTask;
    use crate::key::DiceKey;
    use crate::value::DiceKeyValue;
    use crate::value::DiceValidValue;
    use crate::value::MaybeResidentComputedValue;
    use crate::value::MaybeValidDiceValue;

    pub(crate) fn make_completed_task<K: Key>(key: DiceKey, val: K::Value) -> DiceTask {
        make_completed_task_with_computed_value(
            key,
            MaybeResidentComputedValue::new_resident(
                MaybeValidDiceValue::valid(DiceValidValue::testing_new(DiceKeyValue::<K>::new(
                    val,
                ))),
                InvalidationSources::NONE,
                Revision::FIRST,
            ),
        )
    }

    pub(crate) fn make_completed_task_with_computed_value(
        key: DiceKey,
        val: MaybeResidentComputedValue,
    ) -> DiceTask {
        let prepared = DiceTask::prepare_testing(key);
        let task = prepared.task().clone_arc();
        prepared.completion_handle.completed(val);
        task
    }
}
