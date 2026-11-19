/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use dupe::Dupe;
use futures::task::AtomicWaker;

use crate::atomic_waker_set::AtomicWakerSet;
use crate::atomic_waker_set::AtomicWakerSetEntry;
use crate::cancellation::CancelledError;
use crate::cancellation::CriticalSectionGuard;
use crate::cancellation::DisableCancellationGuard;
use crate::cancellation::ExplicitlyCancellableResult;

/// Shared cancellation related execution context for a cancellable task.
///
/// This is the view on the context used by a CancellationHandle to signal a request for cancellation.
pub(crate) struct CancellationHandleSharedStateView {
    inner: Arc<SharedStateData>,
}

impl CancellationHandleSharedStateView {
    pub(crate) fn cancel(&self) {
        self.inner.notify_cancelled();
    }
}

/// Shared cancellation related execution context for a cancellable task.
///
/// This is the "outer" context, held by the task's spawned future impl within
/// its poll loop.
pub(crate) struct CancellableFutureSharedStateView {
    inner: Arc<SharedStateData>,
}

impl CancellableFutureSharedStateView {
    pub(crate) fn new() -> (
        CancellationHandleSharedStateView,
        CancellableFutureSharedStateView,
        CancellationContextSharedStateView,
    ) {
        let shared_data = Arc::new(SharedStateData {
            state: AtomicU8::new(State::Normal.into()),
            cancellation_waker: AtomicWaker::new(),
            prevent_cancellation: AtomicU32::new(0),
            observer_wakers: AtomicWakerSet::new(),
        });

        (
            CancellationHandleSharedStateView {
                inner: shared_data.dupe(),
            },
            CancellableFutureSharedStateView {
                inner: shared_data.dupe(),
            },
            CancellationContextSharedStateView { inner: shared_data },
        )
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    pub(crate) fn register_waker(&self, cx: &mut Context<'_>) {
        self.inner.cancellation_waker.register(cx.waker());
    }

    pub(crate) fn set_exited(&self) -> ExplicitlyCancellableResult<()> {
        self.inner.set_exited()
    }

    pub(crate) fn has_open_critical_sections(&self) -> HasOpenCriticalSections {
        if self.inner.prevent_cancellation.load(Ordering::Relaxed) > 0 {
            HasOpenCriticalSections::Yes
        } else {
            HasOpenCriticalSections::No
        }
    }
}

/// Shared cancellation related execution context for a cancellable task.
///
/// This is the "inner" context, used by a CancellationContext to observe
/// cancellation and enter critical sections. Normal async uses of this only
/// need to refer to the `context_data` member, which is updated by the poll
/// loop on the associated ExplicitCancellationFuture. Synchronous tasks that
/// want to cancel can poll the `inner.cancel_requested` member.
pub(crate) struct CancellationContextSharedStateView {
    inner: Arc<SharedStateData>,
}

#[derive(Debug)]
pub(crate) enum HasOpenCriticalSections {
    Yes,
    No,
}

impl CancellationContextSharedStateView {
    pub(crate) fn enter_critical_section(&self) -> CriticalSectionGuard<'_> {
        self.inner
            .prevent_cancellation
            .fetch_add(1, Ordering::Relaxed);

        CriticalSectionGuard::new_explicit(self)
    }

    /// Also consumes an instance of a `CriticalSectionGuard`
    pub(crate) fn try_disable_cancellation(&self) -> Option<DisableCancellationGuard> {
        // Note that the requirement that this consume a critical section is entirely cosmetic.
        // However, there are legitimate ways in which we may one day want to change the API - such
        // as statically promising that we either succeed or that the future was cancelled - for
        // which this may be useful, so keep it like this for now.
        self.exit_critical_section();
        self.inner.try_disable_cancellation()
    }

    pub(crate) fn exit_critical_section(&self) -> HasOpenCriticalSections {
        if self
            .inner
            .prevent_cancellation
            .fetch_sub(1, Ordering::Relaxed)
            == 1
        {
            HasOpenCriticalSections::No
        } else {
            HasOpenCriticalSections::Yes
        }
    }

    #[inline(always)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

struct SharedStateData {
    state: AtomicU8,

    /// This is the waker associated with the main future which we are executing; we store it here
    /// so that we can wake it if there is a cancellation.
    ///
    /// The ordering associated with this thing is the expected one for wakers; registrars should
    /// register themselves with this thing before checking other state, and wakers should wake this
    /// thing after modifying said state.
    cancellation_waker: AtomicWaker,

    /// Tracks the number of open critical sections, generally so that we can know if there are any
    ///
    /// It's important to keep in mind that the concept of "critical sections" is pretty much
    /// orthogonal to the concept of being "cancelled." The existence or non-existence of critical
    /// sections doesn't prevent a future from being cancelled. The future can still be cancelled by
    /// calling `.cancel`, and that cancellation is immediately observable eg via checking
    /// `.is_cancelled` or by cancellation observer wakeups.
    ///
    /// Holding open a critical section only causes a future that was cancelled to continue to be
    /// polled, either to completion or until the number of open critical sections goes to zero.
    /// Importantly, even if the future was polled to completion, it will still resolve to
    /// "cancelled." Returning the output of the inner future would be dangerous, since the
    /// cancellation did really happen and was observable to the inner future.
    ///
    /// As a result, even if a future has been cancelled, it may still be running, not just for the
    /// reason above but also maybe because it has not yet reached an await point since being
    /// cancelled. We make no effort to prevent opening new critical sections if that happens.
    /// That's actually not ideal API design, but besides that it's semantically completely ok.
    ///
    /// Any cancellation observers created from such critical sections will still resolve
    /// immediately.
    prevent_cancellation: AtomicU32,

    /// Wakers that expect to be woken up in case of a cancellation
    ///
    /// Again, the ordering contract here is the same as with all other wakers; these are registered
    /// *before* checking the state for whether the future was already cancelled, and woken up after
    /// the state is updatd.
    observer_wakers: AtomicWakerSet<()>,
}

impl SharedStateData {
    #[inline(always)]
    fn is_cancelled(&self) -> bool {
        matches!(
            State::from(self.state.load(Ordering::Relaxed)),
            State::Cancelled
        )
    }
}

enum State {
    /// This future is running normally (or has not yet been started)
    ///
    /// This is the only state which is non-terminal, ie all states other than this one will never
    /// change again once they're reached.
    Normal,
    /// This future has already been cancelled.
    Cancelled,
    /// Cancellations on this future have been permanently disabled
    CancellationsDisabled,
    /// This future has finished running normally.
    Exited,
}

impl From<u8> for State {
    fn from(value: u8) -> Self {
        match value {
            0 => State::Normal,
            1 => State::Cancelled,
            2 => State::CancellationsDisabled,
            3 => State::Exited,
            _ => panic!("invalid status"),
        }
    }
}

impl From<State> for u8 {
    fn from(value: State) -> Self {
        match value {
            State::Normal => 0,
            State::Cancelled => 1,
            State::CancellationsDisabled => 2,
            State::Exited => 3,
        }
    }
}

impl SharedStateData {
    /// Returns whether we were already cancelled
    fn notify_cancelled(&self) {
        if self.move_normal_state_to(State::Cancelled).is_ok() {
            self.cancellation_waker.wake();
            self.observer_wakers.wake_all();
        }
    }

    fn try_disable_cancellation(&self) -> Option<DisableCancellationGuard> {
        match self.move_normal_state_to(State::CancellationsDisabled) {
            Ok(_) => Some(DisableCancellationGuard),
            Err(State::CancellationsDisabled) => Some(DisableCancellationGuard),
            Err(_) => None,
        }
    }

    fn set_exited(&self) -> ExplicitlyCancellableResult<()> {
        match self.move_normal_state_to(State::Exited) {
            Ok(_) => Ok(()),
            Err(State::Cancelled) => Err(CancelledError),
            Err(_) => Ok(()),
        }
    }

    /// If the state is currently Normal, update it to the given one and returns `Ok(Normal)`.
    ///
    /// Otherwise, returns `Err(current_state)`
    fn move_normal_state_to(&self, state: State) -> Result<State, State> {
        // On the orderings: Our choice to use `Relaxed` here instead of `Release` means that no
        // synchronize-with edge is created between - say - the thread calling `.cancel` and the
        // thread observing the cancel. That's fine insofar that there's nothing that guarantees
        // that we would, but we may wish to change that either if some code wants to rely on it or
        // if we think it's too much of a footgun
        self.state
            .compare_exchange(
                State::Normal.into(),
                state.into(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .map_err(State::from)
            .map(State::from)
    }
}

#[pin_project::pin_project(PinnedDrop)]
pub(crate) struct CancellationObserverFuture {
    inner: Arc<SharedStateData>,
    #[pin]
    entry: AtomicWakerSetEntry<()>,
}

impl CancellationObserverFuture {
    pub(crate) fn new(view: &CancellationContextSharedStateView) -> Self {
        Self::new_from_state(view.inner.dupe())
    }

    fn new_from_state(inner: Arc<SharedStateData>) -> Self {
        CancellationObserverFuture {
            inner,
            entry: AtomicWakerSetEntry::new(()),
        }
    }

    fn disconnect(self: Pin<&mut Self>) {
        let this = self.project();
        // SAFETY: this is the only set we use
        unsafe {
            this.entry.disconnect(&this.inner.observer_wakers);
        }
    }

    #[inline(always)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

impl Clone for CancellationObserverFuture {
    fn clone(&self) -> Self {
        CancellationObserverFuture::new_from_state(self.inner.dupe())
    }
}

impl Dupe for CancellationObserverFuture {}

#[pin_project::pinned_drop]
impl PinnedDrop for CancellationObserverFuture {
    fn drop(self: Pin<&mut Self>) {
        self.disconnect();
    }
}

impl Future for CancellationObserverFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Fast path avoiding the register
        if self.is_cancelled() {
            // Won't bother with the `disconnect` here, we'll do it on drop
            return Poll::Ready(());
        }

        unsafe {
            let this = self.as_mut().project();
            // SAFETY: We're pinned, we keep the state alive via our arc, and this is the only set
            // we use
            this.entry
                .register(&this.inner.observer_wakers, cx.waker().clone());
        }

        // Note that the semantics of `AtomicWaker` are exactly designed for this to be correct, but
        // nonetheless it's quite subtle. If this is racing with a `notify_cancelled` call, then
        // either:
        //  1. Our `register` came before the wake, in which case we'll get woken up and get to see
        //     whatever happened at that time
        //  2. Our `register` came after the wake, in which case the `register` acquires the change
        //     to the state that preceded the `wake` in `notify_cancelled`; as such, it's
        //     guaranteed it'll be visible here.
        if self.is_cancelled() {
            // Again, we could disconnect here but don't bother
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
