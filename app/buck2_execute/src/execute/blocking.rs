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
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::liveliness_observer::LivelinessObserverSync;
use buck2_core::buck2_env;
use buck2_core::fs::project::ProjectRoot;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::current_span;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_events::dispatch::maybe_proxy_current_span;
use buck2_events::dispatch::with_dispatcher_opt;
use buck2_events::span::SpanId;
use buck2_util::threads::directory_mutation_parallelism;
use buck2_util::threads::thread_spawn;
use crossbeam_channel::unbounded;
use dice::DiceComputations;
use dice::UserComputationData;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use tokio::sync::Semaphore;
use tokio::sync::oneshot;

#[async_trait]
pub trait BlockingExecutor: Allocative + Send + Sync + 'static {
    /// Execute a blocking I/O operation on the current thread. This should be used sparingly. It
    /// is appropriate to use in cases we are doing a minimal amount of I/O (e.g. writing to just
    /// one file), or where I/O is mixed with other blocking operations.  Those operations run with
    /// fairly high concurrency as they aren't expected to contend with each other.
    async fn execute_dyn_io_inline<'a>(
        &self,
        f: Box<dyn FnOnce() -> buck2_error::Result<()> + Send + 'a>,
    ) -> buck2_error::Result<()>;

    /// Execute a blocking I/O operation, possibly on a dedicated I/O pool. This should be used as
    /// the default for I/O. The operations executed here must perform _only_ I/O (since if they do
    /// something else they might contend for I/O threads with actual I/O).
    fn execute_io<'a>(
        &self,
        io: Box<dyn IoRequest>,
        cancellations: &'a CancellationContext,
    ) -> BoxFuture<'a, buck2_error::Result<()>>;

    /// The size of the queue of pending I/O.
    fn queue_size(&self) -> usize;

    /// Resources that must outlive queued I/O and databases opened through this executor.
    fn lifetime_guard(&self) -> Option<Arc<dyn Send + Sync>> {
        None
    }
}

#[derive(Allocative)]
struct GuardedBlockingExecutor {
    executor: Arc<dyn BlockingExecutor>,
    #[allocative(skip)]
    guard: Arc<dyn Send + Sync>,
    #[allocative(skip)]
    cancellation: Option<Arc<dyn LivelinessObserverSync>>,
}

struct GuardedIoRequest {
    request: Box<dyn IoRequest>,
    _guard: Arc<dyn Send + Sync>,
    cancellation: Option<Arc<dyn LivelinessObserverSync>>,
}

impl IoRequest for GuardedIoRequest {
    fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(|observer| !observer.is_alive_sync())
        {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Tenant I/O was cancelled before execution"
            ));
        }
        self.request.execute(project_fs)
    }
}

/// Retains `guard` until the executor and every request submitted through it have been dropped.
pub fn with_lifetime_guard(
    executor: Arc<dyn BlockingExecutor>,
    guard: Arc<dyn Send + Sync>,
    cancellation: Option<Arc<dyn LivelinessObserverSync>>,
) -> Arc<dyn BlockingExecutor> {
    Arc::new(GuardedBlockingExecutor {
        executor,
        guard,
        cancellation,
    })
}

#[async_trait]
impl BlockingExecutor for GuardedBlockingExecutor {
    async fn execute_dyn_io_inline<'a>(
        &self,
        f: Box<dyn FnOnce() -> buck2_error::Result<()> + Send + 'a>,
    ) -> buck2_error::Result<()> {
        self.executor.execute_dyn_io_inline(f).await
    }

    fn execute_io<'a>(
        &self,
        request: Box<dyn IoRequest>,
        cancellations: &'a CancellationContext,
    ) -> BoxFuture<'a, buck2_error::Result<()>> {
        self.executor.execute_io(
            Box::new(GuardedIoRequest {
                request,
                _guard: self.guard.clone(),
                cancellation: self.cancellation.clone(),
            }),
            cancellations,
        )
    }

    fn queue_size(&self) -> usize {
        self.executor.queue_size()
    }

    fn lifetime_guard(&self) -> Option<Arc<dyn Send + Sync>> {
        Some(self.guard.clone())
    }
}

impl dyn BlockingExecutor {
    pub async fn execute_io_inline<F, T>(&self, f: F) -> buck2_error::Result<T>
    where
        F: FnOnce() -> buck2_error::Result<T> + Send,
        T: Send,
    {
        let mut res = None;
        self.execute_dyn_io_inline(Box::new(|| {
            res = Some(f()?);
            Ok(())
        }))
        .await
        .tag(buck2_error::ErrorTag::IoBlockingExecutor)?;
        res.internal_error("Inline I/O did not execute")
    }
}

pub trait IoRequest: Send + Sync + 'static {
    fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()>;
}

/// What the inline I/O path is doing right now. Inline I/O runs on its caller's thread; the
/// pooled executor first takes a permit from a semaphore sized to the host's parallelism, the
/// direct one runs at once, so `waiting` is only ever non-zero under the pooled one.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct InlineIoStats {
    pub in_flight: u64,
    pub waiting: u64,
}

#[derive(Allocative, Default)]
pub struct InlineIoCounters {
    #[allocative(skip)]
    in_flight: AtomicU64,
    #[allocative(skip)]
    waiting: AtomicU64,
}

impl InlineIoCounters {
    fn stats(&self) -> InlineIoStats {
        InlineIoStats {
            in_flight: self.in_flight.load(Ordering::Relaxed),
            waiting: self.waiting.load(Ordering::Relaxed),
        }
    }
}

/// Holds a counter up for as long as it lives, so that a caller cancelled while waiting is
/// counted out again.
struct Gauge<'a>(&'a AtomicU64);

impl<'a> Gauge<'a> {
    fn up(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Gauge(counter)
    }
}

impl Drop for Gauge<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

struct ThreadPoolIoRequest {
    project_fs: ProjectRoot,
    io: Box<dyn IoRequest>,
    parent_id: Option<SpanId>,
    dispatcher: Option<EventDispatcher>,
    sender: oneshot::Sender<buck2_error::Result<()>>,
}

#[derive(Allocative)]
struct BuckBlockingExecutorShared {
    #[allocative(skip)]
    io_data_semaphore: Semaphore,
    #[allocative(skip)]
    command_sender: crossbeam_channel::Sender<ThreadPoolIoRequest>,
}

impl BuckBlockingExecutorShared {
    /// We choose the default concurrency as follows:
    ///
    /// - For operations executed by the thread pool, we use `directory_mutation_parallelism()`:
    ///   those operations do exclusively I/O work that modifies the directory structure of
    ///   the FS, which scales negatively past a small number of concurrent workers.
    ///
    /// - For operations that primarily write data, we default to the number of threads on the
    ///   host. This is because those operations often have to do CPU bound work to generate the data
    ///   they are trying to write, and writing to multiple files doesn't have the negative scaling
    ///   issues modifying the directory structure does.
    fn default_concurrency() -> buck2_error::Result<Self> {
        let io_threads =
            buck2_env!("BUCK2_IO_THREADS", type=usize, default=directory_mutation_parallelism())?;
        let io_semaphore = buck2_env!("BUCK2_IO_SEMAPHORE", type=usize, default=buck2_util::threads::available_parallelism())?;

        let (command_sender, command_receiver) = unbounded();

        for i in 0..io_threads {
            let command_receiver = command_receiver.clone();
            thread_spawn(&format!("buck-io-{i}"), move || {
                for ThreadPoolIoRequest {
                    project_fs,
                    sender,
                    parent_id,
                    dispatcher,
                    io,
                } in command_receiver.iter()
                {
                    let res = with_dispatcher_opt(dispatcher, || {
                        maybe_proxy_current_span(parent_id, || io.execute(&project_fs))
                    });
                    let _ignored = sender.send(res);
                }
            })
            .buck_error_context("Failed to spawn io worker")?;
        }

        Ok(Self {
            io_data_semaphore: Semaphore::new(io_semaphore),
            command_sender,
        })
    }
}

#[derive(Allocative)]
struct BuckBlockingExecutor {
    shared: Arc<BuckBlockingExecutorShared>,
    inline: Arc<InlineIoCounters>,
    #[allocative(skip)]
    project_fs: ProjectRoot,
}

#[async_trait]
impl BlockingExecutor for BuckBlockingExecutor {
    async fn execute_dyn_io_inline<'a>(
        &self,
        f: Box<dyn FnOnce() -> buck2_error::Result<()> + Send + 'a>,
    ) -> buck2_error::Result<()> {
        let _permit = {
            let _waiting = Gauge::up(&self.inline.waiting);
            self.shared
                .io_data_semaphore
                .acquire()
                .await
                .expect("This semaphore is never closed")
        };
        let _in_flight = Gauge::up(&self.inline.in_flight);

        tokio::task::block_in_place(f)
    }

    fn execute_io<'a>(
        &self,
        io: Box<dyn IoRequest>,
        cancellations: &'a CancellationContext,
    ) -> BoxFuture<'a, buck2_error::Result<()>> {
        let (sender, receiver) = oneshot::channel();

        // Ignore errors sending as they'll translate to an error receiving once we drop the
        // sender.
        let _ignored = self.shared.command_sender.send(ThreadPoolIoRequest {
            project_fs: self.project_fs.dupe(),
            io,
            parent_id: current_span(),
            dispatcher: get_dispatcher_opt(),
            sender,
        });

        cancellations
            .critical_section(
                || async move { receiver.await.buck_error_context("Pool shut down")? },
            )
            .boxed()
    }

    fn queue_size(&self) -> usize {
        self.shared.command_sender.len()
    }
}

/// Executor that bypasses the queue and executes IO directly using Tokio's
/// blocking thread pool.

#[derive(Allocative)]
struct DirectIoExecutor {
    inline: Arc<InlineIoCounters>,
    #[allocative(skip)]
    project_fs: ProjectRoot,
}

#[async_trait]
impl BlockingExecutor for DirectIoExecutor {
    async fn execute_dyn_io_inline<'a>(
        &self,
        f: Box<dyn FnOnce() -> buck2_error::Result<()> + Send + 'a>,
    ) -> buck2_error::Result<()> {
        let _in_flight = Gauge::up(&self.inline.in_flight);
        tokio::task::block_in_place(f)
    }

    fn execute_io<'a>(
        &self,
        io: Box<dyn IoRequest>,
        cancellations: &'a CancellationContext,
    ) -> BoxFuture<'a, buck2_error::Result<()>> {
        let project_fs = self.project_fs.dupe();
        let dispatcher = get_dispatcher_opt();

        cancellations
            .critical_section(|| async move {
                // Execute IO operation in Tokio's blocking thread pool
                tokio::task::spawn_blocking(move || {
                    with_dispatcher_opt(dispatcher, || io.execute(&project_fs))
                })
                .await
                .buck_error_context("Direct IO spawn_blocking failed")?
            })
            .boxed()
    }

    fn queue_size(&self) -> usize {
        // This executor does not maintain its own queue. We are logging Tokio
        // IO thread metrics separately.
        0
    }
}

#[derive(Allocative)]
enum BlockingExecutorFactoryKind {
    Pooled(Arc<BuckBlockingExecutorShared>),
    Direct,
}

/// Owns the daemon-wide scheduling resources used to create repo-bound blocking executors.
#[derive(Allocative)]
pub struct BlockingExecutorFactory {
    kind: BlockingExecutorFactoryKind,
    inline: Arc<InlineIoCounters>,
}

impl BlockingExecutorFactory {
    pub fn create() -> buck2_error::Result<Self> {
        let kind = if cfg!(any(target_os = "macos", target_os = "windows")) {
            BlockingExecutorFactoryKind::Direct
        } else {
            BlockingExecutorFactoryKind::Pooled(Arc::new(
                BuckBlockingExecutorShared::default_concurrency()?,
            ))
        };

        Ok(Self {
            kind,
            inline: Arc::new(InlineIoCounters::default()),
        })
    }

    pub fn for_project(&self, project_fs: ProjectRoot) -> Arc<dyn BlockingExecutor> {
        match &self.kind {
            BlockingExecutorFactoryKind::Pooled(shared) => Arc::new(BuckBlockingExecutor {
                shared: shared.dupe(),
                inline: self.inline.dupe(),
                project_fs,
            }),
            BlockingExecutorFactoryKind::Direct => Arc::new(DirectIoExecutor {
                inline: self.inline.dupe(),
                project_fs,
            }),
        }
    }

    pub fn queue_size(&self) -> usize {
        match &self.kind {
            BlockingExecutorFactoryKind::Pooled(shared) => shared.command_sender.len(),
            BlockingExecutorFactoryKind::Direct => 0,
        }
    }

    /// Inline I/O across every executor this factory made.
    pub fn inline_io_stats(&self) -> InlineIoStats {
        self.inline.stats()
    }
}

pub trait SetBlockingExecutor {
    fn set_blocking_executor(&mut self, exec: Arc<dyn BlockingExecutor>);
}

pub trait HasBlockingExecutor<'d> {
    fn get_blocking_executor(&self) -> &'d dyn BlockingExecutor;
}

impl SetBlockingExecutor for UserComputationData {
    fn set_blocking_executor(&mut self, exec: Arc<dyn BlockingExecutor>) {
        self.data.set(exec);
    }
}

impl<'d> HasBlockingExecutor<'d> for DiceComputations<'d> {
    fn get_blocking_executor(&self) -> &'d dyn BlockingExecutor {
        &**self
            .per_transaction_data()
            .data
            .get::<Arc<dyn BlockingExecutor>>()
            .expect("BlockingExecutor should be set")
    }
}

pub mod testing {
    use super::*;

    #[derive(Allocative)]
    pub struct DummyBlockingExecutor {
        pub fs: ProjectRoot,
    }

    #[async_trait]
    impl BlockingExecutor for DummyBlockingExecutor {
        async fn execute_dyn_io_inline<'a>(
            &self,
            f: Box<dyn FnOnce() -> buck2_error::Result<()> + Send + 'a>,
        ) -> buck2_error::Result<()> {
            f()
        }

        fn execute_io<'a>(
            &self,
            io: Box<dyn IoRequest>,
            _cancellations: &'a CancellationContext,
        ) -> BoxFuture<'a, buck2_error::Result<()>> {
            futures::future::ready(io.execute(&self.fs)).boxed()
        }

        fn queue_size(&self) -> usize {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    use buck2_core::error::SoftErrorContext;
    use buck2_core::error::capture_soft_error_context;
    use buck2_core::error::with_soft_error_context;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_events::dispatch::with_dispatcher_async;
    use buck2_fs::async_fs_util::spawn_blocking as spawn_blocking_with_context;
    use buck2_fs::error::IoResultExt;
    use buck2_fs::fs_util;

    use super::*;

    struct HoldIo {
        started: oneshot::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl IoRequest for HoldIo {
        fn execute(self: Box<Self>, _project_fs: &ProjectRoot) -> buck2_error::Result<()> {
            self.started.send(()).expect("test should await I/O start");
            self.release
                .lock()
                .expect("test lock should not be poisoned")
                .recv()
                .expect("test should release I/O");
            Ok(())
        }
    }

    async fn assert_cancelled_io_retains_ownership(
        executor: Arc<dyn BlockingExecutor>,
    ) -> buck2_error::Result<()> {
        let gate = Arc::new(Semaphore::new(1));
        let lease = Arc::new(gate.clone().acquire_owned().await?);
        let executor = with_lifetime_guard(executor, lease.clone(), None);
        let (started, start) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let task = tokio::spawn(executor.execute_io(
            Box::new(HoldIo {
                started,
                release: Mutex::new(wait),
            }),
            CancellationContext::never_cancelled(),
        ));
        start.await?;
        task.abort();
        assert!(
            task.await
                .expect_err("waiter should be cancelled")
                .is_cancelled()
        );
        drop(executor);
        drop(lease);
        let ownership_available = gate.clone().try_acquire_owned().is_ok();
        release.send(()).expect("worker should still be waiting");
        assert!(
            !ownership_available,
            "cancelled waiters must not release ownership while I/O runs"
        );
        let _next_generation =
            tokio::time::timeout(Duration::from_secs(5), gate.acquire_owned()).await??;
        Ok(())
    }

    #[tokio::test]
    async fn pooled_io_retains_ownership_after_waiter_cancellation() -> buck2_error::Result<()> {
        let root = ProjectRootTemp::new()?;
        let executor = Arc::new(BuckBlockingExecutor {
            shared: Arc::new(BuckBlockingExecutorShared::default_concurrency()?),
            inline: Arc::new(InlineIoCounters::default()),
            project_fs: root.path().dupe(),
        });
        assert_cancelled_io_retains_ownership(executor).await
    }

    #[tokio::test]
    async fn direct_io_retains_ownership_after_waiter_cancellation() -> buck2_error::Result<()> {
        let root = ProjectRootTemp::new()?;
        assert_cancelled_io_retains_ownership(Arc::new(DirectIoExecutor {
            inline: Arc::new(InlineIoCounters::default()),
            project_fs: root.path().dupe(),
        }))
        .await
    }

    struct WriteMarker(&'static str);

    #[tokio::test]
    async fn cancelled_queued_io_does_not_write_outputs() -> buck2_error::Result<()> {
        let root = ProjectRootTemp::new()?;
        let (sender, receiver) = crossbeam_channel::unbounded();
        let executor = Arc::new(BuckBlockingExecutor {
            shared: Arc::new(BuckBlockingExecutorShared {
                io_data_semaphore: Semaphore::new(1),
                command_sender: sender,
            }),
            inline: Arc::new(InlineIoCounters::default()),
            project_fs: root.path().dupe(),
        });
        let (observer, alive) = buck2_common::liveliness_observer::LivelinessGuard::create_sync();
        let executor = with_lifetime_guard(executor, Arc::new(()), Some(observer));
        let result = executor.execute_io(
            Box::new(WriteMarker("old")),
            CancellationContext::never_cancelled(),
        );
        drop(alive);
        let request = receiver.try_recv().expect("I/O should have been queued");
        let outcome = request.io.execute(&request.project_fs);
        assert!(outcome.is_err());
        let _ignored = request.sender.send(outcome);
        assert!(result.await.is_err());
        assert!(
            !root
                .path()
                .resolve(ProjectRelativePath::new("marker")?)
                .exists()
        );
        Ok(())
    }

    impl IoRequest for WriteMarker {
        fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()> {
            project_fs.write_file(ProjectRelativePath::new("marker")?, self.0, false)
        }
    }

    struct RequireSoftErrorContext;

    impl IoRequest for RequireSoftErrorContext {
        fn execute(self: Box<Self>, _project_fs: &ProjectRoot) -> buck2_error::Result<()> {
            let _context = get_dispatcher_opt()
                .and_then(|dispatcher| dispatcher.soft_error_context())
                .internal_error("blocking worker should inherit the soft-error context")?;
            Ok(())
        }
    }

    async fn assert_propagates_soft_error_context(
        executor: &dyn BlockingExecutor,
    ) -> buck2_error::Result<()> {
        let context = Arc::new(SoftErrorContext::new("", "")?);
        let dispatcher = EventDispatcher::null().with_soft_error_context(context);
        with_dispatcher_async(dispatcher, async {
            executor
                .execute_io(
                    Box::new(RequireSoftErrorContext),
                    CancellationContext::never_cancelled(),
                )
                .await
        })
        .await
    }

    #[tokio::test]
    async fn factory_routes_io_to_each_project_root() -> buck2_error::Result<()> {
        let first_root = ProjectRootTemp::new()?;
        let second_root = ProjectRootTemp::new()?;
        let factory = BlockingExecutorFactory::create()?;
        let first_executor = factory.for_project(first_root.path().dupe());
        let second_executor = factory.for_project(second_root.path().dupe());

        first_executor
            .execute_io(
                Box::new(WriteMarker("first")),
                CancellationContext::never_cancelled(),
            )
            .await?;
        second_executor
            .execute_io(
                Box::new(WriteMarker("second")),
                CancellationContext::never_cancelled(),
            )
            .await?;

        let marker = ProjectRelativePath::new("marker")?;
        assert_eq!(
            fs_util::read_to_string(first_root.path().resolve(marker)).categorize_internal()?,
            "first"
        );
        assert_eq!(
            fs_util::read_to_string(second_root.path().resolve(marker)).categorize_internal()?,
            "second"
        );

        Ok(())
    }

    #[tokio::test]
    async fn executors_propagate_soft_error_context() -> buck2_error::Result<()> {
        let root = ProjectRootTemp::new()?;
        let pooled = BuckBlockingExecutor {
            shared: Arc::new(BuckBlockingExecutorShared::default_concurrency()?),
            inline: Arc::new(InlineIoCounters::default()),
            project_fs: root.path().dupe(),
        };
        assert_propagates_soft_error_context(&pooled).await?;

        let direct = DirectIoExecutor {
            inline: Arc::new(InlineIoCounters::default()),
            project_fs: root.path().dupe(),
        };
        assert_propagates_soft_error_context(&direct).await?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inline_io_is_counted_while_it_runs() -> buck2_error::Result<()> {
        let root = ProjectRootTemp::new()?;
        let factory = BlockingExecutorFactory::create()?;
        let executor = factory.for_project(root.path().dupe());
        assert_eq!(factory.inline_io_stats(), InlineIoStats::default());

        let seen = executor
            .execute_io_inline(|| Ok(factory.inline_io_stats()))
            .await?;
        assert_eq!(
            seen,
            InlineIoStats {
                in_flight: 1,
                waiting: 0
            }
        );
        assert_eq!(factory.inline_io_stats(), InlineIoStats::default());
        Ok(())
    }

    #[tokio::test]
    async fn generic_spawn_blocking_propagates_soft_error_context() -> buck2_error::Result<()> {
        let context = Arc::new(SoftErrorContext::new("", "")?);
        let handle = with_soft_error_context(Some(context.dupe()), || {
            spawn_blocking_with_context(capture_soft_error_context)
        });
        let observed = handle.await?;

        assert!(observed.is_some_and(|observed| Arc::ptr_eq(&context, &observed)));
        Ok(())
    }
}
