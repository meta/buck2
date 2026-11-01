/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::sync::Weak;

use allocative::Allocative;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::DropcancelHandle;
use dice_futures::spawn::WeakFutureError;
use dice_futures::spawn::prepare_detached_cancellation;
use dice_futures::spawner::TokioSpawner;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::future::Shared;
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::liveliness_observer::LivelinessGuard;
use crate::liveliness_observer::LivelinessObserverSync;

/// Cancels an owner's tasks and tracks termination, including resources retained by blocking I/O.
#[derive(Allocative)]
pub struct CancellationScope {
    #[allocative(skip)]
    state: Mutex<ScopeState>,
    #[allocative(skip)]
    observer: Arc<dyn LivelinessObserverSync>,
}

struct ScopeState {
    guard: Option<LivelinessGuard>,
    work: Vec<TrackedWork>,
    /// Length at which finished work is next swept out of `work`.
    sweep_at: usize,
}

const MIN_SWEEP_LENGTH: usize = 64;

struct TrackedWork {
    owner: Weak<CancellationScopeGuard>,
    cancellation: Option<Weak<DropcancelHandle>>,
    completed: Shared<BoxFuture<'static, ()>>,
}

/// Keeps an owner's cancellation incomplete until the last resource holding this guard is gone.
pub struct CancellationScopeGuard {
    _completed: oneshot::Sender<()>,
}

impl CancellationScope {
    pub fn new() -> Self {
        let (observer, guard) = LivelinessGuard::create_sync();
        Self {
            state: Mutex::new(ScopeState {
                guard: Some(guard),
                work: Vec::new(),
                sweep_at: MIN_SWEEP_LENGTH,
            }),
            observer,
        }
    }

    pub fn observer(&self) -> Arc<dyn LivelinessObserverSync> {
        self.observer.clone()
    }

    fn track(
        state: &mut ScopeState,
        cancellation: Option<Weak<DropcancelHandle>>,
    ) -> Arc<CancellationScopeGuard> {
        let (sender, receiver) = oneshot::channel();
        let owner = Arc::new(CancellationScopeGuard { _completed: sender });
        // Sweeping whenever the list doubles keeps each call amortized constant time while
        // bounding retained entries to a small multiple of the live ones.
        if state.work.len() >= state.sweep_at {
            state.work.retain(|work| work.owner.strong_count() != 0);
            state.sweep_at = (state.work.len() * 2).max(MIN_SWEEP_LENGTH);
        }
        state.work.push(TrackedWork {
            owner: Arc::downgrade(&owner),
            cancellation,
            completed: receiver.map(|_| ()).boxed().shared(),
        });
        owner
    }

    /// Registers a resource that must be released before cancellation is complete.
    pub fn register(&self) -> buck2_error::Result<Arc<CancellationScopeGuard>> {
        let mut state = self.state.lock();
        if state.guard.is_none() {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Tenant generation was evicted"
            ));
        }
        Ok(Self::track(&mut state, None))
    }

    /// Spawns explicitly cancellable work. Dropping the returned future leaves cancellation to
    /// the scope or the returned handle, while termination remains tracked by the scope.
    pub fn spawn<F, T>(
        &self,
        runtime: &Handle,
        f: F,
    ) -> (
        BoxFuture<'static, Result<T, WeakFutureError>>,
        Arc<DropcancelHandle>,
    )
    where
        F: for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, T> + Send + 'static,
        T: Send + 'static,
    {
        let (spawner, cancellation) = prepare_detached_cancellation();
        let cancellation = Arc::new(cancellation.into_dropcancel());
        let ownership = {
            let mut state = self.state.lock();
            if state.guard.is_none() {
                cancellation.cancel();
                return (
                    futures::future::ready(Err(WeakFutureError::Cancelled)).boxed(),
                    cancellation,
                );
            }
            Self::track(&mut state, Some(Arc::downgrade(&cancellation)))
        };
        let task_cancellation = cancellation.clone();
        let task = {
            let _entered = runtime.enter();
            spawner.spawn(f, &TokioSpawner, &())
        };
        let joined = runtime.spawn(async move {
            let _cancellation = task_cancellation;
            let result = task.await;
            drop(ownership);
            result
        });
        (
            joined
                .map(|result| result.unwrap_or(Err(WeakFutureError::JoinError)))
                .boxed(),
            cancellation,
        )
    }

    /// Stops admission and requests cancellation without waiting for protected sections.
    pub fn cancel(&self) {
        let mut state = self.state.lock();
        state.guard.take();
        for work in &state.work {
            if let Some(handle) = work.cancellation.as_ref().and_then(Weak::upgrade) {
                handle.cancel();
            }
        }
    }

    /// Waits for registered work and retained resources after admission has been stopped.
    pub async fn wait(&self) {
        let completed = {
            let state = self.state.lock();
            assert!(state.guard.is_none(), "cancel the scope before waiting");
            state
                .work
                .iter()
                .map(|work| work.completed.clone())
                .collect::<Vec<_>>()
        };
        futures::stream::iter(completed)
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
    }
}

impl Default for CancellationScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CancellationScope {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use tokio::sync::Semaphore;

    use super::*;

    #[tokio::test]
    async fn completed_tasks_preserve_their_results_without_an_external_handle() {
        let scope = CancellationScope::new();
        let (task, cancellation) = scope.spawn(&Handle::current(), |_| async { 42 }.boxed());
        drop(cancellation);
        assert_eq!(Ok(42), task.await);
        scope.cancel();
        scope.wait().await;
    }

    #[tokio::test]
    async fn cancellation_waits_for_protected_work() {
        let scope = CancellationScope::new();
        let (started, ready) = oneshot::channel();
        let (release, proceed) = oneshot::channel();
        let (task, _) = scope.spawn(&Handle::current(), move |cancellations| {
            async move {
                cancellations
                    .critical_section(|| async move {
                        started.send(()).unwrap();
                        proceed.await.unwrap();
                    })
                    .await;
                futures::future::pending::<()>().await;
            }
            .boxed()
        });
        ready.await.unwrap();
        scope.cancel();
        let stopped = scope.wait();
        futures::pin_mut!(stopped);
        assert!(futures::poll!(&mut stopped).is_pending());
        release.send(()).unwrap();
        assert_eq!(Err(WeakFutureError::Cancelled), task.await);
        stopped.await;
    }

    #[tokio::test]
    async fn retained_disk_ownership_blocks_completion_and_replacement() {
        let scope = CancellationScope::new();
        let gate = Arc::new(Semaphore::new(1));
        let lease = Arc::new((
            gate.clone().acquire_owned().await.unwrap(),
            scope.register().unwrap(),
        ));
        let outstanding_io = lease.clone();
        scope.cancel();
        let stopped = scope.wait();
        let replacement = gate.acquire();
        futures::pin_mut!(stopped, replacement);
        drop(lease);
        assert!(futures::poll!(&mut stopped).is_pending());
        assert!(futures::poll!(&mut replacement).is_pending());
        drop(outstanding_io);
        stopped.await;
        let _replacement = replacement.await.unwrap();
    }

    #[tokio::test]
    async fn abandoned_join_keeps_ownership_until_cancelled_work_terminates() {
        let scope = CancellationScope::new();
        let gate = Arc::new(Semaphore::new(1));
        let task_gate = gate.clone();
        let (started, ready) = oneshot::channel();
        let (release, proceed) = oneshot::channel();
        let (task, _) = scope.spawn(&Handle::current(), move |cancellations| {
            cancellations
                .critical_section(move || async move {
                    let _ownership = task_gate.acquire().await.unwrap();
                    started.send(()).unwrap();
                    proceed.await.unwrap();
                })
                .boxed()
        });
        ready.await.unwrap();
        scope.cancel();
        drop(task);
        let stopped = scope.wait();
        let replacement = gate.acquire();
        futures::pin_mut!(stopped, replacement);
        assert!(futures::poll!(&mut stopped).is_pending());
        assert!(futures::poll!(&mut replacement).is_pending());
        release.send(()).unwrap();
        stopped.await;
        let _replacement = replacement.await.unwrap();
    }

    #[tokio::test]
    async fn finished_work_is_swept_without_scanning_on_every_spawn() {
        let scope = CancellationScope::new();
        for _ in 0..(MIN_SWEEP_LENGTH * 50) {
            let (task, _) = scope.spawn(&Handle::current(), |_| async {}.boxed());
            assert_eq!(Ok(()), task.await);
            assert!(scope.state.lock().work.len() <= 2 * MIN_SWEEP_LENGTH);
        }
        scope.cancel();
        scope.wait().await;
    }

    #[tokio::test]
    async fn cancelled_scopes_reject_new_work() {
        let scope = CancellationScope::new();
        scope.cancel();
        assert!(scope.register().is_err());
        let ran = Arc::new(AtomicBool::new(false));
        let task_ran = ran.clone();
        let (task, _) = scope.spawn(&Handle::current(), move |_| {
            async move {
                task_ran.store(true, Ordering::SeqCst);
            }
            .boxed()
        });
        assert_eq!(Err(WeakFutureError::Cancelled), task.await);
        scope.wait().await;
        assert!(!ran.load(Ordering::SeqCst));
    }
}
