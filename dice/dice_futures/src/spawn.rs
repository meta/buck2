/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The future that is spawned, but has various more strict cancellation behaviour than
//! tokio's JoinHandle

use std::any::Any;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::future::Future;
use pin_project::pin_project;
use thiserror::Error;

use crate::cancellation::CancellationContext;
use crate::cancellation::CancellationHandle;
use crate::cancellation::DropcancelHandle;
use crate::cancellation::ExplicitlyCancellableResult;
use crate::details::cancellable_future::ExplicitlyCancellableFuture;
use crate::details::shared_state::CancellableFutureSharedStateView;
use crate::details::shared_state::CancellationContextSharedStateView;
use crate::owning_future::OwningFuture;
use crate::spawner::Spawner;

#[derive(Debug, Error, Copy, Clone, PartialEq)]
pub enum WeakFutureError {
    #[error("Join Error")]
    JoinError,

    #[error("Cancelled")]
    Cancelled,
}

/// Spawn a future and return a DropcancelJoinHandle. The future will begin execution even before
/// the handle is polled. The future will be able to observe and control its cancellation with the
/// provided CancellationContext.
///
/// When the handle is dropped, the task will be cancelled. The task can be detached by calling
/// [DropcancelJoinHandle::detach].
pub fn spawn_dropcancel<F, T, S>(f: F, spawner: &dyn Spawner<S>, ctx: &S) -> DropcancelJoinHandle<T>
where
    for<'a> F: FnOnce(&'a CancellationContext) -> BoxFuture<'a, T> + Send,
    T: Any + Send + 'static,
{
    let (cancellable_spawner, cancellation_handle) = prepare_detached_cancellation();

    let cancellable_join_handle = cancellable_spawner.spawn(f, spawner, ctx);

    cancellable_join_handle.reattach(cancellation_handle)
}

#[pin_project]
pub struct CancellableJoinHandle<T>(
    #[pin] pub(crate) BoxFuture<'static, Result<T, WeakFutureError>>,
);

impl<T> CancellableJoinHandle<T> {
    fn reattach(self, cancellation_handle: CancellationHandle) -> DropcancelJoinHandle<T> {
        DropcancelJoinHandle {
            guard: cancellation_handle.into_dropcancel(),
            fut: self.0,
        }
    }
}

impl<T> Future for CancellableJoinHandle<T> {
    type Output = Result<T, WeakFutureError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().0.poll(cx)
    }
}

/// An owned permission to join on a task (await its termination).
///
/// When dropped this will cancel the ongoing task.
///
/// This can be thought of as the equivalent of [tokio::task::JoinHandle] for cancellable
/// tasks.
///
/// This struct is created by `spawn_dropcancel` and related functions, and like
/// [tokio::task::spawn] the task associated with this will have started immediately on spawn,
/// even if this future has not been awaited.
#[pin_project]
pub struct DropcancelJoinHandle<T> {
    guard: DropcancelHandle,
    #[pin]
    fut: BoxFuture<'static, Result<T, WeakFutureError>>,
}

impl<T> DropcancelJoinHandle<T> {
    /// "Detaches" the task. This will return a pair of the Future for the task and a CancellationHandle for it. The task will no
    /// longer be cancelled on drop of anything (unless the CancellationHandle is explicitly converted into a new DropcancelHandle).
    pub fn detach(self) -> (CancellableJoinHandle<T>, CancellationHandle) {
        (
            CancellableJoinHandle(self.fut),
            self.guard.into_cancellable(),
        )
    }
}

impl<T> Future for DropcancelJoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // When we have a DropCancelJoinHandle, we expect the future to not have been cancelled.
        let this = self.project();
        this.fut.poll(cx).map(|r| r.unwrap())
    }
}

/// Prepare the two halves needed to run an explicitly cancellable future without
/// immediately tying cancellation to the lifetime of the join handle.
///
/// The returned [`CancellableFutureSpawner`] owns the state that will be embedded
/// in the spawned future and its [`CancellationContext`]. The returned
/// [`CancellationHandle`] owns the matching external cancellation capability.
/// Callers should keep these paired: spawn exactly one future with the spawner,
/// then either retain the handle for explicit cancellation or convert it into a
/// [`DropcancelHandle`] when dropping an owner should cancel the task.
///
/// This is the lower-level primitive behind [`spawn_dropcancel`]. Use this when
/// the future must be created or registered before deciding which object should
/// own cancellation.
pub fn prepare_detached_cancellation() -> (CancellableFutureSpawner, CancellationHandle) {
    let (handle_view, future_view, context_view) = CancellableFutureSharedStateView::new();

    (
        CancellableFutureSpawner {
            future_view,
            context_view,
        },
        CancellationHandle::new(handle_view),
    )
}

pub struct CancellableFutureSpawner {
    future_view: CancellableFutureSharedStateView,
    context_view: CancellationContextSharedStateView,
}

impl CancellableFutureSpawner {
    pub fn spawn<F, T, S>(self, f: F, spawner: &dyn Spawner<S>, ctx: &S) -> CancellableJoinHandle<T>
    where
        for<'a> F: FnOnce(&'a CancellationContext) -> BoxFuture<'a, T> + Send,
        T: Any + Send + 'static,
    {
        let fut = self.wrap_future(f);

        let fut = fut.map(|v| Box::new(v) as _);
        let task = spawner.spawn(ctx, fut.boxed());
        let task = task
            .map(|v| {
                v.map_err(|_e: tokio::task::JoinError| WeakFutureError::JoinError)?
                    .downcast::<ExplicitlyCancellableResult<T>>()
                    .expect("Spawned task returned the wrong type")
                    .map_err(|_| WeakFutureError::Cancelled)
            })
            .boxed();

        CancellableJoinHandle(task)
    }

    /// Prepare the `ExplicitlyCancellableFuture` that wraps the future requiring the cancellation
    /// context.
    ///
    /// `ExplicitlyCancellableFuture`s only really make sense as something that will be directly
    /// spawned, so `spawn` above is the public API; this function should remain private.
    pub(crate) fn wrap_future<F, T>(self, f: F) -> ExplicitlyCancellableFuture<T>
    where
        F: for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, T> + Send,
    {
        let Self {
            context_view,
            future_view,
        } = self;

        // For Ready<()> and BoxFuture<()> futures we get these sizes:
        // future alone: 196/320 bits
        // future via async block: 448/704 bits

        // As the spawner is going to take a boxed future and erase its concrete type,
        // we can have different future types for different scenarios in order to
        // minimize the size of them.
        //
        // While we could feasibly distinguish the no-op preamble case, one extra pointer
        // is an okay cost for the simpler api (for now).
        let fut = {
            let cancel = CancellationContext::new_explicit(context_view);

            OwningFuture::new(cancel, |d| f(d))
        };
        ExplicitlyCancellableFuture::new(fut, future_view)
    }
}

// `#[tokio::test]` starts tokio's IO driver: epoll, kqueue or IOCP by OS, and Miri models only
// epoll.
#[cfg(all(test, any(not(miri), target_os = "linux")))]
mod tests {
    use std::sync::Arc;

    use tokio::sync::oneshot;

    use super::*;
    use crate::spawner::TokioSpawner;

    #[derive(Default)]
    struct MockCtx;

    #[tokio::test]
    async fn test_cancellation_of_cancellable() {
        let (release_task, recv_release_task) = oneshot::channel();
        let (notify_success, recv_success) = oneshot::channel();

        let sp = Arc::new(TokioSpawner);

        let (_, cancellation_handle) = spawn_dropcancel(
            move |_| {
                async move {
                    recv_release_task.await.unwrap();
                    notify_success.send(()).unwrap();
                }
                .boxed()
            },
            sp.as_ref(),
            &MockCtx,
        )
        .detach();

        // Trigger cancellation
        cancellation_handle.cancel();

        // Now, release the task. In all likelihood it will have already exited, but
        let _ignored = release_task.send(());

        // The task should never get to sending in notify_success since cancellation was trigger,
        // but it *should* drop the channel itself.
        recv_success.await.unwrap_err();
    }

    #[tokio::test]
    async fn test_cancellation_of_dropcancel() {
        let (release_task, recv_release_task) = oneshot::channel();
        let (notify_success, recv_success) = oneshot::channel();

        let sp = Arc::new(TokioSpawner);

        let future = spawn_dropcancel(
            move |_| {
                async move {
                    recv_release_task.await.unwrap();
                    notify_success.send(()).unwrap();
                }
                .boxed()
            },
            sp.as_ref(),
            &MockCtx,
        );

        drop(future);

        // Now, release the task. In all likelihood it will have already exited, but
        let _ignored = release_task.send(());

        // The task should never get to sending in notify_success since all its referenced had been
        // dropped at that point, but it *should* drop the channel itself.
        recv_success.await.unwrap_err();
    }

    #[tokio::test]
    async fn test_spawn_and_detach() {
        let sp = Arc::new(TokioSpawner);
        let fut = async { "Hello world!" }.boxed();

        let (task, _) = spawn_dropcancel(|_| fut, sp.as_ref(), &MockCtx).detach();

        let res = task.await;
        assert_eq!(res, Ok("Hello world!"));
    }
}
