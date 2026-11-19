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
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use buck2_build_signals::env::WaitingData;
use buck2_common::liveliness_observer::CancelledLivelinessGuard;
use buck2_common::liveliness_observer::LivelinessGuard;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_common::liveliness_observer::LivelinessObserverExt;
use buck2_core::execution_types::executor_config::HybridExecutionLevel;
use buck2_core::warn_user_or_panic;
use buck2_data::SchedulingMode;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::EventDispatcher;
use buck2_execute::execute::claim::Claim;
use buck2_execute::execute::claim::ClaimManager;
use buck2_execute::execute::claim::MutexClaimManager;
use buck2_execute::execute::low_pass_filter::LowPassFilter;
use buck2_execute::execute::manager::CommandExecutionManager;
use buck2_execute::execute::manager::CommandExecutionManagerExt;
use buck2_execute::execute::prepared::PreparedCommand;
use buck2_execute::execute::prepared::PreparedCommandExecutor;
use buck2_execute::execute::request::CommandExecutionPaths;
use buck2_execute::execute::request::ExecutorPreference;
use buck2_execute::execute::result::CommandExecutionErrorType;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::execute::result::CommandExecutionStatus;
use derivative::Derivative;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use futures::future::Either;
use futures::future::Future;
use host_sharing::HostSharingRequirements;

use crate::executors::local::LocalExecutor;

/// The [HybridExecutor] will accept requests and dispatch them to both a local and remote delegate
/// executor, unless the CommandExecutionRequest expresses a preference. That will allow them to
/// race and whichever claims the request first will get to execute it.
///
/// If the remote executor claims the request but does not produce a successful response, we will
/// enqueue the request again to the local executor.
pub struct HybridExecutor<R> {
    pub local: LocalExecutor,
    pub remote: R,
    pub level: HybridExecutionLevel,
    pub executor_preference: ExecutorPreference,
    pub low_pass_filter: Arc<LowPassFilter>,
    pub re_max_input_files_bytes: u64,
    pub fallback_tracker: Arc<FallbackTracker>,
}

impl<R> HybridExecutor<R>
where
    R: PreparedCommandExecutor,
{
    async fn local_exec_cmd(
        &self,
        command: &PreparedCommand<'_, '_>,
        claim_manager: Box<dyn ClaimManager>,
        events: EventDispatcher,
        liveliness_observer: Arc<dyn LivelinessObserver>,
        cancellations: &CancellationContext,
        waiting_data: WaitingData,
    ) -> CommandExecutionResult {
        let local_manager =
            CommandExecutionManager::new(claim_manager, events, liveliness_observer, waiting_data);
        self.local
            .exec_cmd(command, local_manager, cancellations)
            .await
    }

    async fn remote_exec_cmd(
        &self,
        command: &PreparedCommand<'_, '_>,
        remote_manager: CommandExecutionManager,
        cancellations: &CancellationContext,
    ) -> CommandExecutionResult {
        self.remote
            .exec_cmd(command, remote_manager, cancellations)
            .await
    }

    fn command_executor_preference(
        &self,
        command: &PreparedCommand<'_, '_>,
    ) -> buck2_error::Result<ExecutorPreference> {
        self.executor_preference
            .and(command.request.executor_preference())
    }

    /// Indicate whether an action is too big to run on RE.
    fn is_action_too_large_for_remote(&self, paths: &CommandExecutionPaths) -> bool {
        paths.input_files_bytes() > self.re_max_input_files_bytes
    }
}

#[async_trait]
impl<R> PreparedCommandExecutor for HybridExecutor<R>
where
    R: PreparedCommandExecutor,
{
    async fn exec_cmd(
        &self,
        command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        cancellations: &CancellationContext,
    ) -> CommandExecutionResult {
        let executor_preference = self.command_executor_preference(command);

        let executor_preference = match executor_preference {
            Ok(executor_preference) => executor_preference,
            Err(e) => return manager.error("prepare_hybrid", e),
        };

        // inspect that and construct our own result. Especially in the case of secondary fallback,
        // the current approach effectively loses the data from the primary result (for example, we
        // should have stage events the reflect the full duration not just the fallback part).

        // Construct our claim manager and a liveliness guard for local commands. The way this
        // works is as follows: when RE takes a claim, we cancel local commands. This means that we
        // can truly race RE and local: if RE finishes even after local started, it'll cancel the
        // local execution, we'll get back here with a ClaimCancelled from local execution, cancel
        // local's claim, and then resume RE.
        let (local_execution_liveliness_observer, local_execution_liveliness_guard) =
            LivelinessGuard::create();

        // Used to monitor that the RE execution is alive.
        let (remote_execution_liveliness_observer, remote_execution_liveliness_guard) =
            LivelinessGuard::create();

        let claim_manager = MutexClaimManager::new();

        let (is_limited, fallback_only, fallback_on_failure, low_pass_filter) = match self.level {
            HybridExecutionLevel::Limited => (true, false, false, false),
            HybridExecutionLevel::Fallback {
                fallback_on_failure,
            } => (false, true, fallback_on_failure, false),
            HybridExecutionLevel::Full {
                fallback_on_failure,
                low_pass_filter,
            } => (
                false,
                false,
                fallback_on_failure,
                low_pass_filter && command.request.low_pass_filter(),
            ),
        };

        // Note that this only sets up these futures, nothing will happen until they are awaited
        // (this is important in the case where we shouldn't be sending one of them).
        let local_result = self.local_exec_cmd(
            command,
            Box::new(claim_manager.dupe()),
            manager.inner.events.dupe(),
            Arc::new(
                manager
                    .inner
                    .liveliness_observer
                    .dupe()
                    .and(local_execution_liveliness_observer.dupe()),
            ),
            cancellations,
            manager.inner.waiting_data.clone(),
        );

        let remote_manager = CommandExecutionManager::new(
            Box::new(ReClaimManager::new(
                local_execution_liveliness_guard,
                Box::new(claim_manager),
                remote_execution_liveliness_guard,
            )),
            manager.inner.events.dupe(),
            manager.inner.liveliness_observer.dupe(),
            manager.inner.waiting_data.clone(),
        )
        .with_intend_to_fallback_on_failure(fallback_on_failure);
        let was_result_delayed = remote_manager.inner.was_result_delayed.dupe();
        let remote_result = self.remote_exec_cmd(command, remote_manager, cancellations);

        let action_too_large = self.is_action_too_large_for_remote(command.request.paths());
        if executor_preference.requires_remote() && action_too_large {
            return manager.error(
                "remote_only_action_too_large",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Action inputs are {} bytes, above the remote execution limit of {} bytes, and local execution is not allowed",
                    command.request.paths().input_files_bytes(),
                    self.re_max_input_files_bytes
                ),
            );
        }
        if executor_preference.requires_local() || action_too_large {
            let mut res = local_result.await;
            if action_too_large {
                res.scheduling_mode = Some(SchedulingMode::LocalActionTooLarge);
            } else {
                res.scheduling_mode = Some(SchedulingMode::LocalOnly);
            }
            return res;
        };

        if executor_preference.requires_remote() {
            let mut res = remote_result.await;
            res.scheduling_mode = Some(SchedulingMode::RemoteOnly);
            return res;
        }

        let jobs = HybridExecutorJobs {
            local: local_result.map(|r| (r, JobPriority(1))),
            remote: remote_result.map(|r| (r, JobPriority(0))),
            executor_preference,
        };

        if is_limited {
            let mut res = jobs.into_primary().await.0;
            res.scheduling_mode = Some(SchedulingMode::NoFallback);
            return res;
        }

        let weight = match command.request.host_sharing_requirements() {
            HostSharingRequirements::ExclusiveAccess => self.low_pass_filter.capacity(),
            HostSharingRequirements::OnePerToken(.., class)
            | HostSharingRequirements::OnePerTokens(.., class)
            | HostSharingRequirements::Shared(class) => self
                .local
                .host_sharing_broker
                .requested_permits(class)
                .into_count_uncapped(),
        };

        let is_retryable_status =
            move |r: &CommandExecutionResult, ignore_fallback_tracker: bool| {
                match &r.report.status {
                    // This does need be retried since if we get a cancelled result that would
                    // typically mean the other result asked for cancellation and we're about to
                    // receive the result here, or it could mean we're being asked to cancel by our
                    // caller.
                    CommandExecutionStatus::Cancelled { .. } => true,
                    // If the execution is successful, use the result.
                    CommandExecutionStatus::Success { .. } => false,
                    // Retry commands that failed (i.e. exit 1) only if we're instructed to do so.
                    CommandExecutionStatus::Failure { .. }
                    | CommandExecutionStatus::WorkerFailure { .. } => fallback_on_failure,
                    // Don't retry timeouts. They are used for tests and falling back on a timeout is
                    // sort of the opposite of what's been requested.
                    CommandExecutionStatus::TimedOut { .. } => false,
                    // Don't retry storage resource exhaustion errors as retries might only increase the traffic to storage.
                    CommandExecutionStatus::Error {
                        typ: CommandExecutionErrorType::StorageResourceExhausted,
                        ..
                    } => ignore_fallback_tracker,
                    // Errors are infra errors and are always retried because that is the point of
                    // falling back.
                    CommandExecutionStatus::Error { .. } => {
                        ignore_fallback_tracker || self.fallback_tracker.can_fallback()
                    }
                }
            };

        let fallback_only = fallback_only && !command.request.force_full_hybrid_if_capable();

        let scheduling_mode: SchedulingMode;
        let ((mut first_res, first_priority), second) =
            if executor_preference.prefers_local() || executor_preference.prefers_remote() {
                // Don't race in this scenario, since this is typically used for
                // actions that are too expensive to run on RE.
                if executor_preference.prefers_local() {
                    scheduling_mode = SchedulingMode::PreferLocal;
                } else {
                    scheduling_mode = SchedulingMode::PreferRemote;
                }
                jobs.execute_sequential().await
            } else {
                // In the full-hybrid case, we do race both executors. If the low-pass filter is in
                // use, then we wrap the local execution with that.
                let jobs = if fallback_only {
                    scheduling_mode = SchedulingMode::Fallback;
                    jobs.map_local(move |local| {
                        async move {
                            // Block local until the remote executor aborts (that's remote_execution_liveliness_guard)
                            // The claim actually comes back to us via the execution report so there's no race condition
                            // where local unblocks just when RE finishes
                            remote_execution_liveliness_observer.while_alive().await;
                            let (mut res, priority) = local.await;
                            if was_result_delayed.load(std::sync::atomic::Ordering::Relaxed) {
                                // RE queuing but not cancelled, will race with local.
                                res.scheduling_mode = Some(SchedulingMode::FallbackReQueueEstimate);
                            }
                            (res, priority)
                        }
                        .boxed()
                    })
                } else if low_pass_filter {
                    scheduling_mode = SchedulingMode::FullHybrid;
                    jobs.map_local(move |local| {
                        async move {
                            // Block local until either condition is met:
                            // - we only have a few actions (that's low_pass_filter)
                            // - the remote executor aborts (that's remote_execution_liveliness_guard)
                            let access = self.low_pass_filter.access(weight);
                            let alive = remote_execution_liveliness_observer.while_alive();
                            futures::pin_mut!(access);
                            futures::pin_mut!(alive);
                            let _guard = futures::future::select(access, alive).await;
                            local.await
                        }
                        .boxed()
                    })
                } else {
                    scheduling_mode = SchedulingMode::FullHybrid;
                    jobs.map_local(|local| local.boxed())
                };
                jobs.execute_concurrent().await
            };

        let mut res = if is_retryable_status(&first_res, false) {
            // If the first result had made a claim, then cancel it now to let the other result
            // proceed.
            if let Some(claim) = first_res.report.claim.take() {
                if let Err(e) = claim.release() {
                    return manager.error(
                        "hybrid",
                        e.context("Local execution started executing without a Claim"),
                    );
                }
            }

            let (second_res, second_priority) = second.await;

            // For the purposes of giving users a good UX, if both things failed, give them the
            // local executor's error, which is likely to not have failed because of e.g.
            // sandboxing.
            let (mut primary_res, mut secondary_res) = if is_retryable_status(&second_res, true) {
                if first_priority > second_priority {
                    (first_res, second_res)
                } else {
                    (second_res, first_res)
                }
            } else {
                (second_res, first_res)
            };

            // But if the first result was a cancelled result then we definitely don't want that.
            if matches!(
                &primary_res.report.status,
                CommandExecutionStatus::Cancelled { .. }
            ) {
                std::mem::swap(&mut primary_res, &mut secondary_res);
            }

            primary_res.rejected_execution = Some(secondary_res.report);
            primary_res
        } else {
            // Everyone is happy, we got our result.
            first_res
        };
        // Don't overwrite outcome if set by local job.
        if res.scheduling_mode.is_none() {
            res.scheduling_mode = Some(scheduling_mode);
        }
        res.eligible_for_full_hybrid = !fallback_only;
        res
    }

    fn is_local_execution_possible(&self, executor_preference: ExecutorPreference) -> bool {
        let executor_preference = match self.executor_preference.and(executor_preference) {
            Ok(p) => p,
            Err(..) => return false,
        };
        if executor_preference.requires_remote() {
            return false;
        }
        match self.level {
            HybridExecutionLevel::Limited => !executor_preference.prefers_remote(),
            HybridExecutionLevel::Fallback { .. } | HybridExecutionLevel::Full { .. } => true,
        }
    }
}

struct ReClaimManager {
    inner: Option<ReClaimManagerInner>,
}

impl ReClaimManager {
    fn new(
        local_execution_liveliness_guard: LivelinessGuard,
        claim_manager: Box<dyn ClaimManager>,
        remote_execution_liveliness_guard: LivelinessGuard,
    ) -> Self {
        Self {
            inner: Some(ReClaimManagerInner {
                local_execution_liveliness_guard,
                claim_manager,
                remote_execution_liveliness_guard: Some(remote_execution_liveliness_guard),
            }),
        }
    }
}

impl Drop for ReClaimManager {
    /// If RE decides to not execute, then allow local execution to proceed.
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            inner.local_execution_liveliness_guard.forget();
        }
    }
}

struct ReClaimManagerInner {
    /// Used to control and potentially cancel the execution of local commands.
    local_execution_liveliness_guard: LivelinessGuard,
    claim_manager: Box<dyn ClaimManager>,

    /// Only kept alive while the ReClaimManager (or its Claim) is alive.
    remote_execution_liveliness_guard: Option<LivelinessGuard>,
}

#[async_trait::async_trait]
impl ClaimManager for ReClaimManager {
    async fn claim(mut self: Box<Self>) -> Box<dyn Claim> {
        let inner = self.inner.take().expect("This is only taken once");

        // Kill in-flight local commands.
        let released_liveliness_guard = inner.local_execution_liveliness_guard.cancel();

        // Ask for the lock. If we never get it then we just exit as that would mean local
        // execution finished.
        let claim = inner.claim_manager.claim().await;

        Box::new(ReClaim {
            released_liveliness_guard,
            claim,
            _remote_execution_liveliness_guard: inner.remote_execution_liveliness_guard,
        })
    }

    fn on_result_delayed(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.remote_execution_liveliness_guard.take();
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug)] // None of the fields have meaningful debug here.
struct ReClaim {
    #[derivative(Debug = "ignore")]
    released_liveliness_guard: CancelledLivelinessGuard,
    #[derivative(Debug = "ignore")]
    claim: Box<dyn Claim>,

    /// Only used for its lifetime.
    #[derivative(Debug = "ignore")]
    _remote_execution_liveliness_guard: Option<LivelinessGuard>,
}

impl Claim for ReClaim {
    /// Releasing the ReClaim. When we do this, we need to let local execution proceed again. This
    /// can happen if we do a fallback without full hybrid: RE goes, and fails, and then we want to
    /// proceed with local.
    ///
    /// NOTE: this does NOT work in the following scenario:
    ///
    /// - Local claims
    /// - RE has a result, asks local to cancel (successfully)
    /// - RE claims
    /// - RE fails
    ///
    /// In that case, RE will have failed. To fix this, we need local to release its claim instead
    /// of returning ClaimCancelled.
    fn release(self: Box<Self>) -> buck2_error::Result<()> {
        // An error here should only occur if local execution had started without the claim.
        self.released_liveliness_guard
            .restore()
            .internal_error("Unable to restore CancelledLivelinessGuard!")?
            .forget();

        self.claim.release()?;

        Ok(())
    }
}

/// The local and remote side of the executor, and a preference to select which future is the
/// primary and which is the secondary.
struct HybridExecutorJobs<L, R> {
    local: L,
    remote: R,
    executor_preference: ExecutorPreference,
}

impl<L, R, O> HybridExecutorJobs<L, R>
where
    L: Future<Output = O>,
    R: Future<Output = O>,
{
    /// Modify the local side future.
    fn map_local<L2>(self, f: impl FnOnce(L) -> L2) -> HybridExecutorJobs<L2, R> {
        HybridExecutorJobs {
            local: f(self.local),
            remote: self.remote,
            executor_preference: self.executor_preference,
        }
    }

    /// Return only the primary future.
    fn into_primary(self) -> Either<L, R> {
        self.into_futures().0
    }

    /// Return both futures, but box them, which makes them Upin and also erases their type.
    fn into_boxfutures<'a>(self) -> (BoxFuture<'a, O>, BoxFuture<'a, O>)
    where
        L: Send + 'a,
        R: Send + 'a,
    {
        let (primary, secondary) = self.into_futures();
        (primary.boxed(), secondary.boxed())
    }

    /// Return both futures.
    fn into_futures(self) -> (Either<L, R>, Either<L, R>) {
        let local = self.local.left_future();
        let remote = self.remote.right_future();

        if self.executor_preference.prefers_local() {
            (local, remote)
        } else {
            (remote, local)
        }
    }

    /// Race both futures, return the first result and the second future.
    async fn execute_concurrent<'a>(self) -> (O, BoxFuture<'a, O>)
    where
        L: Send + 'a,
        R: Send + 'a,
    {
        let (primary, secondary) = self.into_boxfutures();
        let (out, futs) = Either::factor_first(futures::future::select(primary, secondary).await);
        (out, futs.into_inner())
    }

    /// Run primary then return secondary.
    async fn execute_sequential<'a>(self) -> (O, BoxFuture<'a, O>)
    where
        L: Send + 'a,
        R: Send + 'a,
    {
        let (primary, secondary) = self.into_boxfutures();
        (primary.await, secondary)
    }
}

#[derive(PartialOrd, Ord, PartialEq, Eq)]
struct JobPriority(u8);

pub struct FallbackTracker {
    count_fallbacks: AtomicI64,
}

impl FallbackTracker {
    pub fn new() -> Self {
        FallbackTracker {
            count_fallbacks: AtomicI64::new(0),
        }
    }

    pub fn can_fallback(&self) -> bool {
        let retried = self.count_fallbacks.fetch_add(1, Ordering::Relaxed);

        #[cfg(all(fbcode_build, target_os = "linux"))]
        let max = if hostcaps::is_prod() {
            justknobs::get("buck2/buck2:max_fallback", None).ok()
        } else {
            None
        };

        #[cfg(not(all(fbcode_build, target_os = "linux")))]
        let max = None::<i64>;

        if let Some(max) = max {
            if retried >= max {
                warn_user_or_panic!(
                    "local_fallback_limit_reached",
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Environment,
                        "Not falling back to local because too many actions have fallen back already.  \
                    If you would like to proceed anyway, pass `--prefer-local`."
                    )
                );
                return false;
            }
        }

        true
    }
}
