/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Defines a future with explicit cancellation

use std::future::Future;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::sync::LazyLock;
use std::task::Context;
use std::task::Poll;

use dupe::Dupe;
use thiserror::Error;

use crate::details::cancellation_context::CancellationContextInner;
use crate::details::cancellation_context::ExplicitCancellationContext;
use crate::details::cancellation_context::ExplicitCriticalSectionGuard;
use crate::details::shared_state::CancellationContextSharedStateView;
use crate::details::shared_state::CancellationHandleSharedStateView;
use crate::details::shared_state::CancellationObserverFuture;

static NEVER_CANCELLED: LazyLock<CancellationContext> =
    LazyLock::new(|| CancellationContext(CancellationContextInner::NeverCancelled));

/// Context available to the function running inside the future to control and manage it's own
/// cancellation
pub struct CancellationContext(CancellationContextInner);

impl CancellationContext {
    pub fn testing() -> &'static Self {
        &NEVER_CANCELLED
    }

    /// Create a new context from a thread that is never canceled
    pub fn never_cancelled() -> &'static Self {
        &NEVER_CANCELLED
    }

    /// Ignore cancellations while 'CriticalSectionGuard' is held
    pub fn enter_critical_section(&self) -> CriticalSectionGuard<'_> {
        self.0.enter_critical_section()
    }

    /// Enter a critical section during which the current future (if it supports explicit cancellation)
    /// should not be dropped. If the future was not cancelled before entering the critical section,
    /// it becomes non-cancellable during the critical section. If it *was* cancelled before
    /// entering the critical section (i.e. the last ref was dropped during `poll`), then the
    /// future is allowed to continue executing until this future resolves.
    pub fn critical_section<'a, F, Fut>(
        &'a self,
        make: F,
    ) -> impl Future<Output = <Fut as Future>::Output> + 'a
    where
        F: FnOnce() -> Fut + 'a,
        Fut: Future + 'a,
    {
        // placeholder to just delegate to existing critical_section code for now until we migrate
        // the callsites
        self.0.critical_section(make)
    }

    /// Enter a structured cancellation section. The caller receives a CancellationObserver. The
    /// CancellationObserver is a future that resolves when cancellation is requested (or when this
    /// section exits).
    pub fn with_structured_cancellation<'a, F, Fut>(
        &'a self,
        make: F,
    ) -> impl Future<Output = <Fut as Future>::Output> + 'a
    where
        Fut: Future + 'a,
        F: FnOnce(CancellationObserver) -> Fut + 'a,
    {
        self.0.with_structured_cancellation(make)
    }

    /// Tries to disable cancellation for this task from here on. If cancellations are successfully disabled,
    /// a DisableCancellationGuard will be returned.
    pub fn try_disable_cancellation(&self) -> Option<DisableCancellationGuard> {
        self.enter_critical_section().try_disable_cancellation()
    }

    pub(crate) fn new_explicit(inner: CancellationContextSharedStateView) -> CancellationContext {
        Self(CancellationContextInner::Explicit(
            ExplicitCancellationContext { inner },
        ))
    }

    /// Check whether the future has been cancelled.
    ///
    /// This is intended to be polled by synchronous code to decide to exit early. It should
    /// typically not be used in an async context, `with_structured_cancellation` tends to be more
    /// appropriate there.
    #[inline(always)]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// When held, prevents cancellation of the current explicitly cancellable future.
/// When no longer held (unless drop or cancellations are disabled was not ran), cancellations will be allowed.
/// `disable_cancellations`ing this will leak the guard, causing cancellations to be disabled forever, regardless
/// of whether or not the task is already cancelled.
pub enum CriticalSectionGuard<'a> {
    NeverCancelled,
    Explicit(ExplicitCriticalSectionGuard<'a>),
}

impl<'a> CriticalSectionGuard<'a> {
    pub fn cancellation_observer(&self) -> CancellationObserver {
        match self {
            CriticalSectionGuard::NeverCancelled => {
                CancellationObserver(CancellationObserverInner::NeverCancelled)
            }
            CriticalSectionGuard::Explicit(inner) => inner.cancellation_observer(),
        }
    }

    /// Allow cancellations again, but unlike dropping it, also checks if we should be cancelled
    /// right now, at this specific await point.
    pub async fn exit_critical_section(self) {
        match self {
            CriticalSectionGuard::NeverCancelled => {
                // nothing to do.
            }
            CriticalSectionGuard::Explicit(inner) => inner.exit_critical_section().await,
        }
    }

    /// Ignores cancellations forever if we are not already cancelled. Otherwise, begin allowing
    /// cancellations again
    pub fn try_disable_cancellation(self) -> Option<DisableCancellationGuard> {
        match self {
            CriticalSectionGuard::NeverCancelled => Some(DisableCancellationGuard),
            CriticalSectionGuard::Explicit(inner) => inner.try_disable_cancellation(),
        }
    }

    pub(crate) fn new_explicit(context: &'a CancellationContextSharedStateView) -> Self {
        Self::Explicit(ExplicitCriticalSectionGuard {
            context: Some(context),
        })
    }
}

/// A Future that resolves only when the associated task has been canceled.
#[pin_project::pin_project]
#[derive(Clone, Dupe)]
pub struct CancellationObserver(#[pin] pub(crate) CancellationObserverInner);

/// Similar to a CancellationObserver, but meant for use in synchronous code for
/// detecting and exiting early from work when it's been cancelled.
/// CancellationPoller may be created inside or outside structured cancellation,
/// it offers no guarantees that asynchronous callers will be able to clean up
/// state on a cancellation. Asynchronous code that needs to guarantee being
/// able to cleanup state before honoring a cancellation must hold a
/// CancellationObserver, via `CancellationContext::with_structured_cancellation`
#[derive(Clone, Dupe)]
pub struct CancellationPoller(pub(crate) CancellationObserverInner);

impl CancellationObserver {
    pub(crate) fn never_cancelled() -> Self {
        CancellationObserver(CancellationObserverInner::NeverCancelled)
    }

    #[inline(always)]
    pub fn is_cancelled(&self) -> bool {
        match &self.0 {
            CancellationObserverInner::Explicit(fut) => fut.is_cancelled(),
            CancellationObserverInner::NeverCancelled => false,
        }
    }
}

impl CancellationPoller {
    pub(crate) fn never_cancelled() -> Self {
        CancellationPoller(CancellationObserverInner::NeverCancelled)
    }

    #[inline(always)]
    pub fn is_cancelled(&self) -> bool {
        match &self.0 {
            CancellationObserverInner::Explicit(fut) => fut.is_cancelled(),
            CancellationObserverInner::NeverCancelled => false,
        }
    }
}

impl From<CancellationObserver> for CancellationPoller {
    fn from(observer: CancellationObserver) -> Self {
        CancellationPoller(observer.0)
    }
}

impl From<&CancellationObserver> for CancellationPoller {
    fn from(observer: &CancellationObserver) -> Self {
        CancellationPoller(observer.0.clone())
    }
}

impl From<&CancellationContext> for CancellationPoller {
    fn from(context: &CancellationContext) -> Self {
        context.0.poller()
    }
}

#[pin_project::pin_project(project = CancellationObserverInnerProj)]
#[derive(Clone, Dupe)]
pub(crate) enum CancellationObserverInner {
    NeverCancelled,
    Explicit(#[pin] CancellationObserverFuture),
}

impl Future for CancellationObserver {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().0.project() {
            CancellationObserverInnerProj::Explicit(fut) => fut.poll(cx),
            CancellationObserverInnerProj::NeverCancelled => Poll::Pending,
        }
    }
}

/// A marker that indicates that cancellations have been disabled indefinitely for this task.
pub struct DisableCancellationGuard;

/// A handle providing the ability to explicitly cancel the associated ExplicitlyCancellableFuture.
pub struct CancellationHandle {
    view: CancellationHandleSharedStateView,
}

impl CancellationHandle {
    pub(crate) fn new(view: CancellationHandleSharedStateView) -> Self {
        CancellationHandle { view }
    }

    /// Attempts to cancel the future this handle is associated with as soon as possible, returning
    /// a future that completes when the future is canceled.
    pub fn cancel(self) {
        self.view.cancel();
    }

    pub fn into_dropcancel(self) -> DropcancelHandle {
        DropcancelHandle::new(self.view)
    }
}

/// A handle that will cancel the associated ExplicitlyCancellableFuture when it is dropped.
pub struct DropcancelHandle {
    view: ManuallyDrop<CancellationHandleSharedStateView>,
}

impl DropcancelHandle {
    fn new(view: CancellationHandleSharedStateView) -> Self {
        Self {
            view: ManuallyDrop::new(view),
        }
    }

    pub fn cancel(&self) {
        self.view.cancel();
    }

    pub fn into_cancellable(mut self) -> CancellationHandle {
        let view = unsafe { ManuallyDrop::take(&mut self.view) };
        std::mem::forget(self);
        CancellationHandle { view }
    }
}

impl Drop for DropcancelHandle {
    fn drop(&mut self) {
        self.view.cancel();
        unsafe { ManuallyDrop::drop(&mut self.view) };
    }
}

/// A marker that indicates that an evaluation has been cancelled
///
/// FIXME(JakobDegen): Probably find a way to unify this with dice_error?
#[derive(Debug, Error)]
#[error("Cancelled")]
pub struct CancelledError;

pub type ExplicitlyCancellableResult<T> = Result<T, CancelledError>;
