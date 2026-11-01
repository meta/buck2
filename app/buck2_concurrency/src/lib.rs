/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Handles command concurrency.
//!
//! `buck2` supports limited concurrency for commands.
//! If there are no buckconfig changes, nor file changes, then commands can be allowed to execute
//! concurrently. Otherwise, `buck2` will block waiting for other commands to finish.

use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_cli_proto::client_context::ExitWhen;
use buck2_cli_proto::client_context::PreemptibleWhen;
use buck2_core::soft_error;
use buck2_core::soft_error_or_panic;
use buck2_data::CommandPreempted;
use buck2_data::DiceBlockConcurrentCommandEnd;
use buck2_data::DiceBlockConcurrentCommandStart;
use buck2_data::DiceEqualityCheck;
use buck2_data::DiceSynchronizeSectionEnd;
use buck2_data::DiceSynchronizeSectionStart;
use buck2_data::ExclusiveCommandWaitEnd;
use buck2_data::ExclusiveCommandWaitStart;
use buck2_data::NoActiveDiceState;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_file_watcher::file_watcher::SyncOutcome;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_util::early_command_timing::EXCLUSIVE_COMMAND_WAIT;
use buck2_util::early_command_timing::EarlyCommandTimingBuilder;
use buck2_util::rtabort;
use buck2_util::truncate::truncate;
use buck2_wrapper_common::invocation_id::TraceId;
use derive_more::Display;
use dice::BranchId;
use dice::Dice;
use dice::DiceTransaction;
use dice::DiceTransactionUpdater;
use dice::UserComputationData;
use dice::Version;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::future;
use futures::future::BoxFuture;
use futures::future::Either;
use futures::future::Future;
use futures::future::FutureExt;
use futures::future::Shared;
use futures::pin_mut;
use itertools::Itertools;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum ConcurrencyHandlerError {
    #[error(
        "Recursive invocation of Buck, which is discouraged, but will probably work (using the same state). Trace Ids: {0}. Recursive invocation command: `{1}`"
    )]
    NestedInvocationWithSameStates(ConcurrentTraces, String),
    #[error(
        "Recursive invocation of Buck, with a different state. Use `--isolation-dir` on the inner invocation to fix this. Trace Ids: {0}. Recursive invocation command: `{1}`"
    )]
    #[buck2(input)]
    NestedInvocationWithDifferentStates(ConcurrentTraces, String),
    #[error("`--exit-when=differentstate` was set")]
    #[buck2(tag = DaemonIsBusy)]
    ExitWhenDifferentState,

    #[error("`--preemptible` was set, and buck daemon preempted this command as another came in.")]
    #[buck2(tag = DaemonPreempted)]
    ExitOnPreemption,

    #[error("`--exit-when=notidle` was set, and buck daemon is not idle.")]
    #[buck2(tag = DaemonIsBusy)]
    ExitOnDaemonNotIdle,
}

#[derive(Clone, Dupe, Copy, Debug)]
pub enum RunState {
    NestedSameState,
    ParallelSameState,
}

#[derive(Clone, Dupe, Copy, Debug)]
pub enum BypassSemaphore {
    Run(RunState),
    Block,
    Error,
}

#[derive(Debug)]
enum AdmissionDecision {
    RejectDifferentState {
        running: ConcurrentTraces,
        compared_with_active_state: bool,
    },
    RetryAfterCleanup,
    Admit(AdmittedCommand),
    StartCleanup(CleanupRequest),
    RejectNestedDifferentState(NestedInvocationWarning),
    Block(BlockingCommand),
}

#[derive(Debug)]
struct AdmittedCommand {
    state: AdmittedState,
    nested_warning: Option<NestedInvocationWarning>,
}

#[derive(Debug)]
struct NestedInvocationWarning {
    running: ConcurrentTraces,
    display_command: String,
}

#[derive(Debug)]
struct BlockingCommand {
    trace_id: TraceId,
    display_command: String,
}

#[derive(Debug)]
enum AdmittedState {
    Fresh {
        tainted: bool,
        previously_tainted: bool,
    },
    Concurrent {
        state: RunState,
        previously_tainted: bool,
    },
}

enum PreUpdateDecision {
    WaitForCleanupStart,
    WaitForCleanup {
        future: Shared<BoxFuture<'static, ()>>,
        epoch: usize,
    },
    RejectNotIdle {
        running: ConcurrentTraces,
    },
    Update {
        attempt: UpdateAttemptId,
        conflict_on_arrival: Option<Version>,
    },
}

/// Manages concurrent commands, blocking when appropriate.
///
/// Currently, we allow concurrency if two `DiceTransactions` are deemed equivalent, such that
/// any computation result that occurs in one is directly reusable by another.
#[derive(Allocative)]
pub struct ConcurrencyHandler {
    /// The mutable admission state lives in the coordinator task and cannot be traversed through
    /// this handle by `Allocative`.
    #[allocative(skip)]
    coordinator: AdmissionCoordinator,
    dice: Arc<Dice>,
    owns_branch: bool,
    /// Used to prevent commands (clean --stale) from running in parallel with dice commands
    exclusive_command_lock: ExclusiveCommandLock,
    /// Source of `CommandId`s. Deliberately outside the coordinator so that a command has an
    /// identity before it sends its first message.
    next_command_id: AtomicUsize,
}

#[derive(Allocative)]
struct CoordinatorState {
    /// the currently active `Dice` being used. Commands can only run concurrently if these are
    /// "equivalent".
    dice_status: DiceStatus,
    /// A list of the currently running commands.
    active_commands: BuckMutInsertionMap<CommandId, CommandData>,
    /// Commands that entered admission but have not begun execution.
    pending_commands: BuckMutInsertionMap<CommandId, PendingCommand>,
    next_update_attempt: usize,
    /// The epoch of the last ActiveDice we assigned.
    cleanup_epoch: usize,
    /// Whether this has been tainted previously.
    previously_tainted: bool,
}

/// Identifies one entry in `active_commands`. `TraceId` cannot serve this purpose because it is not
/// unique across concurrently live entries.
///
/// Values are distinct and increasing, but not contiguous: every error path between allocation and
/// registration burns one.
#[derive(Allocative, Debug, Display, Copy, Clone, Dupe, PartialEq, Eq, Hash)]
struct CommandId(usize);

#[derive(Allocative, Debug, Display, Copy, Clone, Dupe, PartialEq, Eq)]
struct UpdateAttemptId(usize);

#[derive(Debug)]
struct RegisteredCommand(CommandId);

enum AdmissionEvent {
    CleanupPrepared {
        request: CleanupRequest,
        future: Shared<BoxFuture<'static, ()>>,
    },
    CleanupCompleted {
        epoch: usize,
    },
    CommandExited(RegisteredCommand),
}

enum CoordinatorMessage {
    Retire,
    Begin {
        request: AdmissionRequest,
        response: oneshot::Sender<buck2_error::Result<PreUpdateDecision>>,
    },
    BeforeUpdate {
        command: CommandId,
        response: oneshot::Sender<buck2_error::Result<PreUpdateDecision>>,
    },
    AcquireUpdate {
        command: CommandId,
        attempt: UpdateAttemptId,
        response: oneshot::Sender<buck2_error::Result<BranchId>>,
    },
    MoveToBranch {
        command: CommandId,
        attempt: UpdateAttemptId,
        branch: BranchId,
        retire_previous: bool,
        response: oneshot::Sender<buck2_error::Result<()>>,
    },
    AfterUpdate {
        command: CommandId,
        update: CompletedUpdate,
        response: oneshot::Sender<buck2_error::Result<PostUpdateDecision>>,
    },
    CleanupCompleted {
        epoch: usize,
        response: Option<oneshot::Sender<buck2_error::Result<()>>>,
    },
    QueuedTraces {
        asking: CommandId,
        response: oneshot::Sender<buck2_error::Result<QueuedTraces>>,
    },
    Release {
        command: CommandId,
        response: Option<oneshot::Sender<buck2_error::Result<()>>>,
    },
    #[cfg(test)]
    Snapshot(oneshot::Sender<buck2_error::Result<CoordinatorSnapshot>>),
}

enum PostUpdateDecision {
    Outcome(AdmissionOutcome),
    AwaitCleanup {
        future: Shared<BoxFuture<'static, ()>>,
        epoch: usize,
    },
    Block {
        blocking: BlockingCommand,
        effects: AdmissionEffects,
        wake: oneshot::Receiver<()>,
    },
}

impl PostUpdateDecision {
    fn restore_preemptions(self, state: &mut CoordinatorState) {
        match self {
            Self::Outcome(outcome) => outcome.effects.restore_preemptions(state),
            Self::Block { effects, .. } => effects.restore_preemptions(state),
            Self::AwaitCleanup { .. } => {}
        }
    }
}

#[derive(Clone)]
struct AdmissionCoordinator {
    sender: mpsc::UnboundedSender<CoordinatorMessage>,
    #[cfg(test)]
    active_commands: Arc<AtomicUsize>,
}

#[cfg(test)]
struct CoordinatorSnapshot {
    active_commands: usize,
    pending_commands: usize,
    update_in_flight: bool,
    update_waiters: usize,
    cleanup_in_progress: bool,
    previously_tainted: bool,
}

#[derive(Debug, Eq, PartialEq)]
enum StateEffect {
    None,
    /// Every waiter must re-evaluate because several equivalent commands may now be admissible.
    WakeWaiters,
}

#[derive(Allocative)]
struct CommandData {
    trace_id: TraceId,
    display_command: String,
    preemption_setting: PreemptibleWhen,
    #[allocative(skip)]
    preempt: Option<oneshot::Sender<()>>,
}

#[derive(Allocative)]
struct AdmissionRequest {
    command_id: CommandId,
    command: CommandData,
    is_nested: bool,
    exit_when: ExitWhen,
}

#[derive(Allocative)]
struct PendingCommand {
    request: AdmissionRequest,
    update_attempt: Option<UpdateAttemptId>,
}

enum PendingAdmissionDecision {
    Retain(AdmissionOutcome),
    Admit {
        state: AdmittedState,
        effects: AdmissionEffects,
    },
}

struct CompletedUpdate {
    attempt: UpdateAttemptId,
    version: Version,
    conflict_on_arrival: Option<Version>,
    dice_was_draining: bool,
}

struct PendingUpdate {
    command: CommandId,
    attempt: UpdateAttemptId,
    conflict_on_arrival: Option<Version>,
    dice_was_draining: BoxFuture<'static, bool>,
}

struct UpdatedTransaction {
    transaction: DiceTransaction,
    update: CompletedUpdate,
}

struct ReadyToExecute {
    transaction: DiceTransaction,
    admitted: AdmittedCommand,
    effects: AdmissionEffects,
}

#[derive(Clone, Dupe, Copy, Debug, Eq, PartialEq)]
struct CleanupRequest {
    epoch: usize,
}

enum AdmissionEffect {
    Preempt {
        command: CommandId,
        sender: oneshot::Sender<()>,
    },
    DiceEquality(bool),
}

struct AdmissionEffects(Vec<AdmissionEffect>);

impl AdmissionEffects {
    fn none() -> Self {
        Self(Vec::new())
    }

    fn after_state_comparison(data: &mut CoordinatorState, is_same_state: bool) -> Self {
        let preemptions = data
            .active_commands
            .iter_mut()
            .filter_map(|(command_id, command)| match command.preemption_setting {
                PreemptibleWhen::Never => None,
                PreemptibleWhen::OnDifferentState if is_same_state => None,
                PreemptibleWhen::Always | PreemptibleWhen::OnDifferentState => command
                    .preempt
                    .take()
                    .map(|sender| AdmissionEffect::Preempt {
                        command: *command_id,
                        sender,
                    }),
            });

        if is_same_state {
            Self(
                std::iter::once(AdmissionEffect::DiceEquality(true))
                    .chain(preemptions)
                    .collect(),
            )
        } else {
            Self(
                preemptions
                    .chain(std::iter::once(AdmissionEffect::DiceEquality(false)))
                    .collect(),
            )
        }
    }

    fn execute<E: CommandEvents>(self, events: &E) {
        self.0.into_iter().for_each(|effect| match effect {
            AdmissionEffect::Preempt { sender, .. } => {
                let _ignored = sender.send(());
            }
            AdmissionEffect::DiceEquality(is_equal) => {
                events.instant(DiceEqualityCheck { is_equal }.into());
            }
        });
    }

    fn restore_preemptions(self, state: &mut CoordinatorState) {
        self.0.into_iter().for_each(|effect| {
            let AdmissionEffect::Preempt { command, sender } = effect else {
                return;
            };
            if let Some(command) = state.active_commands.get_mut(&command)
                && command.preempt.is_none()
            {
                command.preempt = Some(sender);
            }
        });
    }
}

struct AdmissionOutcome {
    decision: AdmissionDecision,
    effects: AdmissionEffects,
}

impl AdmissionOutcome {
    fn without_effects(decision: AdmissionDecision) -> Self {
        Self {
            decision,
            effects: AdmissionEffects::none(),
        }
    }

    fn after_state_comparison(
        decision: AdmissionDecision,
        data: &mut CoordinatorState,
        is_same_state: bool,
    ) -> Self {
        Self {
            decision,
            effects: AdmissionEffects::after_state_comparison(data, is_same_state),
        }
    }
}

#[derive(Allocative)]
enum DiceStatus {
    Available {
        active: Option<ActiveDice>,
    },
    CleanupStarting {
        epoch: usize,
    },
    Cleanup {
        future: Shared<BoxFuture<'static, ()>>,
        epoch: usize,
    },
}

#[derive(Allocative, Debug)]
struct ActiveDice {
    version: Version,
}

/// Hand-written only to elide the cleanup future. Deriving would print
/// `Shared { inner: .., waker_key: .. }`, which is noise in every log line and assertion failure.
impl fmt::Debug for DiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiceStatus::Available { active } => {
                f.debug_struct("Available").field("active", active).finish()
            }
            DiceStatus::CleanupStarting { epoch } => f
                .debug_struct("CleanupStarting")
                .field("epoch", epoch)
                .finish(),
            DiceStatus::Cleanup { epoch, .. } => f
                .debug_struct("Cleanup")
                .field("epoch", epoch)
                .finish_non_exhaustive(),
        }
    }
}

impl DiceStatus {
    fn idle() -> Self {
        Self::Available { active: None }
    }

    fn active(version: Version) -> Self {
        Self::Available {
            active: Some(ActiveDice { version }),
        }
    }
}

impl CoordinatorState {
    fn has_no_active_commands(&self) -> bool {
        self.active_commands.is_empty()
    }

    fn register_command(
        &mut self,
        command: CommandId,
        data: CommandData,
    ) -> buck2_error::Result<RegisteredCommand> {
        // Check before inserting: overwriting an existing entry would discard its preemption
        // sender, the coordinator's only way to interrupt that command.
        if self.active_commands.contains_key(&command) {
            return Err(internal_error!(
                "command id `{command}` is already registered"
            ));
        }
        self.active_commands.insert(command, data);
        Ok(RegisteredCommand(command))
    }

    fn begin_admission(
        &mut self,
        request: AdmissionRequest,
    ) -> buck2_error::Result<PreUpdateDecision> {
        let command = request.command_id;
        if self.active_commands.contains_key(&command)
            || self.pending_commands.contains_key(&command)
        {
            return Err(internal_error!(
                "command id `{}` is already registered",
                command
            ));
        }

        self.pending_commands.insert(
            command,
            PendingCommand {
                request,
                update_attempt: None,
            },
        );
        self.decide_before_update_for(command)
    }

    fn pending_command(&self, command: CommandId) -> buck2_error::Result<&AdmissionRequest> {
        self.pending_commands
            .get(&command)
            .map(|pending| &pending.request)
            .ok_or_else(|| internal_error!("command `{command}` is not pending admission"))
    }

    fn ensure_update_attempt_current(
        &self,
        command: CommandId,
        attempt: UpdateAttemptId,
    ) -> buck2_error::Result<()> {
        let pending = self
            .pending_commands
            .get(&command)
            .ok_or_else(|| internal_error!("command `{command}` is not pending admission"))?;
        if pending.update_attempt != Some(attempt) {
            return Err(internal_error!(
                "update attempt `{attempt}` is not current for command `{command}`"
            ));
        }
        Ok(())
    }

    fn admit_command(
        &mut self,
        request: AdmissionRequest,
        state: AdmittedState,
        effects: AdmissionEffects,
    ) -> AdmissionOutcome {
        let nested_warning = match &state {
            AdmittedState::Concurrent {
                state: RunState::NestedSameState,
                ..
            } => Some(NestedInvocationWarning {
                running: ConcurrentTraces::running_and(&self.active_commands, &request.command),
                display_command: request.command.display_command.clone(),
            }),
            AdmittedState::Fresh { .. }
            | AdmittedState::Concurrent {
                state: RunState::ParallelSameState,
                ..
            } => None,
        };

        let command_id = request.command_id;
        self.register_command(command_id, request.command)
            .expect("command availability was checked before registration");

        AdmissionOutcome {
            decision: AdmissionDecision::Admit(AdmittedCommand {
                state,
                nested_warning,
            }),
            effects,
        }
    }

    fn begin_cleanup(&mut self) -> Option<CleanupRequest> {
        if !self.has_no_active_commands() {
            return None;
        }

        tracing::info!("Transitioning ActiveDice to cleanup");
        self.cleanup_epoch += 1;
        let request = CleanupRequest {
            epoch: self.cleanup_epoch,
        };
        self.dice_status = DiceStatus::CleanupStarting {
            epoch: request.epoch,
        };

        Some(request)
    }

    fn apply_event(&mut self, event: AdmissionEvent) -> StateEffect {
        match event {
            AdmissionEvent::CleanupPrepared { request, future } => {
                if !matches!(
                    self.dice_status,
                    DiceStatus::CleanupStarting { epoch } if epoch == request.epoch
                ) {
                    return StateEffect::None;
                }

                self.dice_status = DiceStatus::Cleanup {
                    future,
                    epoch: request.epoch,
                };
                StateEffect::WakeWaiters
            }
            AdmissionEvent::CleanupCompleted { epoch } => {
                if matches!(
                    self.dice_status,
                    DiceStatus::Cleanup {
                        epoch: active_epoch,
                        ..
                    } if active_epoch == epoch
                ) {
                    self.dice_status = DiceStatus::idle();
                }
                StateEffect::None
            }
            AdmissionEvent::CommandExited(command) => {
                self.active_commands
                    .shift_remove(&command.0)
                    .expect("command was active but not in active_commands");
                tracing::info!("Active command was removed: {}", command.0);

                if self.has_no_active_commands() {
                    StateEffect::WakeWaiters
                } else {
                    StateEffect::None
                }
            }
        }
    }

    fn decide_before_update(
        &self,
        request: &AdmissionRequest,
        attempt: UpdateAttemptId,
    ) -> PreUpdateDecision {
        match &self.dice_status {
            DiceStatus::CleanupStarting { .. } => PreUpdateDecision::WaitForCleanupStart,
            DiceStatus::Cleanup { future, epoch } => PreUpdateDecision::WaitForCleanup {
                future: future.clone(),
                epoch: *epoch,
            },
            DiceStatus::Available { active } => {
                if matches!(request.exit_when, ExitWhen::ExitNotIdle)
                    && !self.active_commands.is_empty()
                {
                    return PreUpdateDecision::RejectNotIdle {
                        running: ConcurrentTraces::running(&self.active_commands),
                    };
                }

                PreUpdateDecision::Update {
                    attempt,
                    conflict_on_arrival: active
                        .as_ref()
                        .filter(|_| !self.active_commands.is_empty())
                        .map(|active| active.version),
                }
            }
        }
    }

    fn decide_before_update_for(
        &mut self,
        command: CommandId,
    ) -> buck2_error::Result<PreUpdateDecision> {
        let attempt = UpdateAttemptId(self.next_update_attempt);
        let decision = self.decide_before_update(self.pending_command(command)?, attempt);
        let pending = self
            .pending_commands
            .get_mut(&command)
            .expect("the pending command was read above");
        if matches!(decision, PreUpdateDecision::Update { .. }) {
            self.next_update_attempt += 1;
            pending.update_attempt = Some(attempt);
        } else {
            pending.update_attempt = None;
        }
        Ok(decision)
    }

    fn decide_after_update(
        &mut self,
        request: &AdmissionRequest,
        update: CompletedUpdate,
    ) -> buck2_error::Result<PendingAdmissionDecision> {
        if matches!(request.exit_when, ExitWhen::ExitDifferentState)
            && !request.is_nested
            && update
                .conflict_on_arrival
                .is_some_and(|version| update.version != version)
        {
            return Ok(PendingAdmissionDecision::Retain(
                AdmissionOutcome::without_effects(AdmissionDecision::RejectDifferentState {
                    compared_with_active_state: false,
                    running: ConcurrentTraces::running(&self.active_commands),
                }),
            ));
        }

        let active_version = match &self.dice_status {
            DiceStatus::CleanupStarting { .. } | DiceStatus::Cleanup { .. } => {
                return Ok(PendingAdmissionDecision::Retain(
                    AdmissionOutcome::without_effects(AdmissionDecision::RetryAfterCleanup),
                ));
            }
            DiceStatus::Available {
                active: Some(active),
            } => active.version,
            DiceStatus::Available { active: None } => {
                assert!(
                    self.has_no_active_commands(),
                    "an idle DICE state cannot have registered commands"
                );
                self.ensure_command_can_register(request.command_id)?;
                let tainted = update.dice_was_draining;
                let previously_tainted = self.previously_tainted;
                self.dice_status = DiceStatus::active(update.version);
                self.previously_tainted |= tainted;
                return Ok(PendingAdmissionDecision::Admit {
                    state: AdmittedState::Fresh {
                        tainted,
                        previously_tainted,
                    },
                    effects: AdmissionEffects::none(),
                });
            }
        };

        let is_same_state = update.version == active_version;
        if !is_same_state && self.has_no_active_commands() {
            let cleanup = self
                .begin_cleanup()
                .expect("cleanup decision requires no active commands");
            return Ok(PendingAdmissionDecision::Retain(
                AdmissionOutcome::without_effects(AdmissionDecision::StartCleanup(cleanup)),
            ));
        }

        match determine_bypass_semaphore(is_same_state, request.is_nested) {
            BypassSemaphore::Error => {
                let warning = NestedInvocationWarning {
                    running: ConcurrentTraces::running_and(&self.active_commands, &request.command),
                    display_command: request.command.display_command.clone(),
                };
                Ok(PendingAdmissionDecision::Retain(
                    AdmissionOutcome::after_state_comparison(
                        AdmissionDecision::RejectNestedDifferentState(warning),
                        self,
                        false,
                    ),
                ))
            }
            BypassSemaphore::Run(state) => {
                self.ensure_command_can_register(request.command_id)?;
                let effects = AdmissionEffects::after_state_comparison(self, true);
                Ok(PendingAdmissionDecision::Admit {
                    state: AdmittedState::Concurrent {
                        state,
                        previously_tainted: self.previously_tainted,
                    },
                    effects,
                })
            }
            BypassSemaphore::Block if matches!(request.exit_when, ExitWhen::ExitDifferentState) => {
                Ok(PendingAdmissionDecision::Retain(
                    AdmissionOutcome::after_state_comparison(
                        AdmissionDecision::RejectDifferentState {
                            running: ConcurrentTraces::running(&self.active_commands),
                            compared_with_active_state: true,
                        },
                        self,
                        false,
                    ),
                ))
            }
            BypassSemaphore::Block => {
                let active = self
                    .active_commands
                    .first()
                    .expect("blocking requires an active command")
                    .1;
                let blocking = BlockingCommand {
                    trace_id: active.trace_id.dupe(),
                    display_command: active.display_command.clone(),
                };
                Ok(PendingAdmissionDecision::Retain(
                    AdmissionOutcome::after_state_comparison(
                        AdmissionDecision::Block(blocking),
                        self,
                        false,
                    ),
                ))
            }
        }
    }

    fn ensure_command_can_register(&self, command: CommandId) -> buck2_error::Result<()> {
        if self.active_commands.contains_key(&command) {
            return Err(internal_error!(
                "command id `{}` is already registered",
                command
            ));
        }
        Ok(())
    }

    fn decide_after_update_for(
        &mut self,
        command: CommandId,
        update: CompletedUpdate,
    ) -> buck2_error::Result<AdmissionOutcome> {
        self.ensure_update_attempt_current(command, update.attempt)?;

        let request = self
            .pending_commands
            .shift_remove(&command)
            .expect("the pending command was read above")
            .request;
        match self.decide_after_update(&request, update) {
            Ok(PendingAdmissionDecision::Admit { state, effects }) => {
                Ok(self.admit_command(request, state, effects))
            }
            Ok(PendingAdmissionDecision::Retain(outcome)) => {
                self.pending_commands.insert(
                    command,
                    PendingCommand {
                        request,
                        update_attempt: None,
                    },
                );
                Ok(outcome)
            }
            Err(error) => {
                self.pending_commands.insert(
                    command,
                    PendingCommand {
                        request,
                        update_attempt: None,
                    },
                );
                Err(error)
            }
        }
    }

    fn queued_traces(&self, asking: CommandId) -> QueuedTraces {
        QueuedTraces(ConcurrentTraces(
            self.pending_commands
                .iter()
                .filter(|(queued, _)| **queued != asking)
                .map(|(_, pending)| pending.request.command.trace_id.dupe())
                .collect(),
        ))
    }

    fn release_command(&mut self, command: CommandId) -> StateEffect {
        if self.pending_commands.shift_remove(&command).is_some() {
            return StateEffect::None;
        }

        if self.active_commands.contains_key(&command) {
            return self.apply_event(AdmissionEvent::CommandExited(RegisteredCommand(command)));
        }

        StateEffect::None
    }
}

impl AdmissionCoordinator {
    fn with_branch_ownership(
        dice: Arc<Dice>,
        branch: BranchId,
        owns_branch: bool,
        lifetime_guard: Option<Arc<dyn Send + Sync>>,
    ) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let task = AdmissionCoordinatorTask::new(dice, branch, sender.downgrade(), lifetime_guard);
        #[cfg(test)]
        let active_commands = task.active_commands.dupe();
        let task = tokio::task::spawn(task.run_with_branch_ownership(receiver, owns_branch));
        let _monitor = tokio::task::spawn(async move {
            if let Err(error) = task.await
                && error.is_panic()
            {
                // The coordinator owns all admission state, so it cannot be recovered in place.
                // Abort the daemon and let the client start a healthy replacement.
                rtabort!("concurrency admission coordinator panicked: {error}");
            }
        });
        Self {
            sender,
            #[cfg(test)]
            active_commands,
        }
    }

    async fn request<T>(
        &self,
        make_message: impl FnOnce(oneshot::Sender<buck2_error::Result<T>>) -> CoordinatorMessage,
    ) -> buck2_error::Result<T> {
        let (response, receiver) = oneshot::channel();
        self.sender
            .send(make_message(response))
            .map_err(|_| internal_error!("concurrency admission coordinator stopped"))?;
        receiver
            .await
            .map_err(|_| internal_error!("concurrency admission coordinator stopped"))?
    }

    async fn begin(&self, request: AdmissionRequest) -> buck2_error::Result<PreUpdateDecision> {
        self.request(|response| CoordinatorMessage::Begin { request, response })
            .await
    }

    async fn before_update(&self, command: CommandId) -> buck2_error::Result<PreUpdateDecision> {
        self.request(|response| CoordinatorMessage::BeforeUpdate { command, response })
            .await
    }

    /// Waits for the update lane, and returns the branch the command's update is to run on.
    async fn acquire_update(
        &self,
        command: CommandId,
        attempt: UpdateAttemptId,
    ) -> buck2_error::Result<BranchId> {
        self.request(|response| CoordinatorMessage::AcquireUpdate {
            command,
            attempt,
            response,
        })
        .await
    }

    /// Moves commands to `branch`, which later grants of the update lane return. Only the lane's
    /// holder may do this.
    #[cfg(test)]
    async fn move_to_branch(
        &self,
        command: CommandId,
        attempt: UpdateAttemptId,
        branch: BranchId,
    ) -> buck2_error::Result<()> {
        self.request(|response| CoordinatorMessage::MoveToBranch {
            command,
            attempt,
            branch,
            retire_previous: false,
            response,
        })
        .await
    }

    async fn replace_root(
        &self,
        command: CommandId,
        attempt: UpdateAttemptId,
        branch: BranchId,
    ) -> buck2_error::Result<()> {
        self.request(|response| CoordinatorMessage::MoveToBranch {
            command,
            attempt,
            branch,
            retire_previous: true,
            response,
        })
        .await
    }

    async fn after_update(
        &self,
        command: CommandId,
        update: CompletedUpdate,
    ) -> buck2_error::Result<PostUpdateDecision> {
        self.request(|response| CoordinatorMessage::AfterUpdate {
            command,
            update,
            response,
        })
        .await
    }

    async fn cleanup_completed(&self, epoch: usize) -> buck2_error::Result<()> {
        self.request(|response| CoordinatorMessage::CleanupCompleted {
            epoch,
            response: Some(response),
        })
        .await
    }

    async fn queued_traces(&self, asking: CommandId) -> buck2_error::Result<QueuedTraces> {
        self.request(|response| CoordinatorMessage::QueuedTraces { asking, response })
            .await
    }

    fn release(&self, command: CommandId) {
        let _ignored = self.sender.send(CoordinatorMessage::Release {
            command,
            response: None,
        });
    }

    async fn release_and_wait(&self, command: CommandId) -> buck2_error::Result<()> {
        self.request(|response| CoordinatorMessage::Release {
            command,
            response: Some(response),
        })
        .await
    }

    #[cfg(test)]
    fn active_command_count(&self) -> usize {
        self.active_commands.load(Ordering::Acquire)
    }

    #[cfg(test)]
    async fn snapshot(&self) -> buck2_error::Result<CoordinatorSnapshot> {
        self.request(CoordinatorMessage::Snapshot).await
    }
}

struct AdmissionCoordinatorTask {
    data: CoordinatorState,
    dice: Arc<Dice>,
    sender: mpsc::WeakUnboundedSender<CoordinatorMessage>,
    /// The branch commands run on. A command learns it by being granted the update lane, and only
    /// the lane's holder may move it, so every update runs on the branch the one before it left
    /// behind.
    branch: BranchId,
    update_in_flight: Option<(CommandId, UpdateAttemptId)>,
    update_waiters: VecDeque<UpdateWaiter>,
    blocked_waiters: BuckMutInsertionMap<CommandId, oneshot::Sender<()>>,
    #[cfg(test)]
    active_commands: Arc<AtomicUsize>,
    _lifetime_guard: Option<Arc<dyn Send + Sync>>,
}

struct UpdateWaiter {
    command: CommandId,
    attempt: UpdateAttemptId,
    response: oneshot::Sender<buck2_error::Result<BranchId>>,
}

impl AdmissionCoordinatorTask {
    fn new(
        dice: Arc<Dice>,
        branch: BranchId,
        sender: mpsc::WeakUnboundedSender<CoordinatorMessage>,
        lifetime_guard: Option<Arc<dyn Send + Sync>>,
    ) -> Self {
        Self {
            data: CoordinatorState {
                dice_status: DiceStatus::idle(),
                active_commands: BuckMutInsertionMap::new(),
                pending_commands: BuckMutInsertionMap::new(),
                next_update_attempt: 0,
                cleanup_epoch: 0,
                previously_tainted: false,
            },
            dice,
            _lifetime_guard: lifetime_guard,
            sender,
            branch,
            update_in_flight: None,
            update_waiters: VecDeque::new(),
            blocked_waiters: BuckMutInsertionMap::new(),
            #[cfg(test)]
            active_commands: Arc::new(AtomicUsize::new(0)),
        }
    }

    async fn run_with_branch_ownership(
        mut self,
        mut receiver: mpsc::UnboundedReceiver<CoordinatorMessage>,
        owns_branch: bool,
    ) {
        let mut retiring = false;
        while let Some(message) = receiver.recv().await {
            match message {
                CoordinatorMessage::Retire => retiring = true,
                CoordinatorMessage::Begin { request, response } => {
                    let command = request.command_id;
                    let result = if retiring {
                        Err(buck2_error!(
                            ErrorTag::Input,
                            "Tenant concurrency coordinator was retired"
                        ))
                    } else {
                        self.data.begin_admission(request)
                    };
                    let owns_command = result.is_ok();
                    self.respond(command, owns_command, response, result);
                }
                CoordinatorMessage::BeforeUpdate { command, response } => {
                    let result = self.data.decide_before_update_for(command);
                    let owns_command = self.data.pending_commands.contains_key(&command);
                    self.respond(command, owns_command, response, result);
                }
                CoordinatorMessage::AcquireUpdate {
                    command,
                    attempt,
                    response,
                } => self.acquire_update(command, attempt, response),
                CoordinatorMessage::MoveToBranch {
                    command,
                    attempt,
                    branch,
                    retire_previous,
                    response,
                } => {
                    let previous = self.branch;
                    let result = self.move_to_branch(command, attempt, branch);
                    if retire_previous {
                        let retired = if result.is_ok() { previous } else { branch };
                        let dice = self.dice.dupe();
                        let lifetime_guard = self._lifetime_guard.clone();
                        tokio::spawn(async move {
                            let _ownership = lifetime_guard;
                            dice.wait_for_idle_on(retired).await;
                            dice.delete_branch(retired);
                        });
                    }
                    let owns_command = self.data.pending_commands.contains_key(&command);
                    self.respond(command, owns_command, response, result);
                }
                CoordinatorMessage::AfterUpdate {
                    command,
                    update,
                    response,
                } => {
                    let result = self.after_update(command, update);
                    let owns_command = self.data.pending_commands.contains_key(&command)
                        || self.data.active_commands.contains_key(&command);
                    self.respond_after_update(command, owns_command, response, result);
                }
                CoordinatorMessage::CleanupCompleted { epoch, response } => {
                    let effect = self
                        .data
                        .apply_event(AdmissionEvent::CleanupCompleted { epoch });
                    self.apply_effect(effect);
                    if let Some(response) = response {
                        let _ignored = response.send(Ok(()));
                    }
                }
                CoordinatorMessage::QueuedTraces { asking, response } => {
                    let _ignored = response.send(Ok(self.data.queued_traces(asking)));
                }
                CoordinatorMessage::Release { command, response } => {
                    self.release_command(command);
                    if let Some(response) = response {
                        let _ignored = response.send(Ok(()));
                    }
                }
                #[cfg(test)]
                CoordinatorMessage::Snapshot(response) => {
                    let _ignored = response.send(Ok(CoordinatorSnapshot {
                        active_commands: self.data.active_commands.len(),
                        pending_commands: self.data.pending_commands.len(),
                        update_in_flight: self.update_in_flight.is_some(),
                        update_waiters: self.update_waiters.len(),
                        cleanup_in_progress: matches!(
                            self.data.dice_status,
                            DiceStatus::CleanupStarting { .. } | DiceStatus::Cleanup { .. }
                        ),
                        previously_tainted: self.data.previously_tainted,
                    }));
                }
            }
            #[cfg(test)]
            self.publish_active_commands();
            if retiring
                && self.data.active_commands.is_empty()
                && self.data.pending_commands.is_empty()
                && self.update_in_flight.is_none()
            {
                break;
            }
        }
        if owns_branch {
            self.data.dice_status = DiceStatus::idle();
            self.dice.wait_for_idle_on(self.branch).await;
            self.dice.delete_branch(self.branch);
        }
    }

    #[cfg(test)]
    fn publish_active_commands(&self) {
        self.active_commands
            .store(self.data.active_commands.len(), Ordering::Release);
    }

    fn respond<T>(
        &mut self,
        command: CommandId,
        owns_command: bool,
        response: oneshot::Sender<buck2_error::Result<T>>,
        result: buck2_error::Result<T>,
    ) {
        if response.send(result).is_err() && owns_command {
            self.release_command(command);
        }
    }

    fn respond_after_update(
        &mut self,
        command: CommandId,
        owns_command: bool,
        response: oneshot::Sender<buck2_error::Result<PostUpdateDecision>>,
        result: buck2_error::Result<PostUpdateDecision>,
    ) {
        if let Err(result) = response.send(result) {
            if let Ok(decision) = result {
                match decision {
                    PostUpdateDecision::AwaitCleanup { future, epoch } => {
                        self.finish_abandoned_cleanup(future, epoch);
                    }
                    decision => decision.restore_preemptions(&mut self.data),
                }
            }
            if owns_command {
                self.release_command(command);
            }
        }
    }

    fn finish_abandoned_cleanup(&self, future: Shared<BoxFuture<'static, ()>>, epoch: usize) {
        let Some(sender) = self.sender.upgrade() else {
            return;
        };
        let _cleanup = tokio::task::spawn(async move {
            future.await;
            let _ignored = sender.send(CoordinatorMessage::CleanupCompleted {
                epoch,
                response: None,
            });
        });
    }

    fn acquire_update(
        &mut self,
        command: CommandId,
        attempt: UpdateAttemptId,
        response: oneshot::Sender<buck2_error::Result<BranchId>>,
    ) {
        if let Err(error) = self.data.ensure_update_attempt_current(command, attempt) {
            let _ignored = response.send(Err(error));
            return;
        }
        if self
            .update_in_flight
            .is_some_and(|(updating, _)| updating == command)
            || self
                .update_waiters
                .iter()
                .any(|waiter| waiter.command == command)
        {
            let _ignored = response.send(Err(internal_error!(
                "command `{command}` requested the update lane more than once"
            )));
            return;
        }

        self.update_waiters.push_back(UpdateWaiter {
            command,
            attempt,
            response,
        });
        self.grant_next_update();
    }

    fn grant_next_update(&mut self) {
        if self.update_in_flight.is_some() {
            return;
        }

        while let Some(waiter) = self.update_waiters.pop_front() {
            let result = self
                .data
                .ensure_update_attempt_current(waiter.command, waiter.attempt);
            if let Err(error) = result {
                let _ignored = waiter.response.send(Err(error));
                continue;
            }

            self.update_in_flight = Some((waiter.command, waiter.attempt));
            if waiter.response.send(Ok(self.branch)).is_ok() {
                return;
            }
            self.update_in_flight = None;
        }
    }

    fn move_to_branch(
        &mut self,
        command: CommandId,
        attempt: UpdateAttemptId,
        branch: BranchId,
    ) -> buck2_error::Result<()> {
        if self.update_in_flight != Some((command, attempt)) {
            return Err(internal_error!(
                "command `{command}` moved to branch `{branch:?}` without owning the update lane"
            ));
        }
        self.branch = branch;
        Ok(())
    }

    fn finish_update(&mut self, command: CommandId, attempt: UpdateAttemptId) {
        if self.update_in_flight == Some((command, attempt)) {
            self.update_in_flight = None;
            self.grant_next_update();
        }
    }

    fn release_command(&mut self, command: CommandId) {
        self.blocked_waiters.shift_remove(&command);
        self.update_waiters
            .retain(|waiter| waiter.command != command);
        let effect = self.data.release_command(command);
        self.apply_effect(effect);
        if self
            .update_in_flight
            .is_some_and(|(updating, _)| updating == command)
        {
            self.update_in_flight = None;
            self.grant_next_update();
        }
    }

    fn after_update(
        &mut self,
        command: CommandId,
        update: CompletedUpdate,
    ) -> buck2_error::Result<PostUpdateDecision> {
        if self.update_in_flight != Some((command, update.attempt)) {
            return Err(internal_error!(
                "command `{command}` completed update attempt `{}` without owning the update lane",
                update.attempt
            ));
        }
        let attempt = update.attempt;
        let AdmissionOutcome { decision, effects } =
            match self.data.decide_after_update_for(command, update) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.finish_update(command, attempt);
                    return Err(error);
                }
            };
        let result = match decision {
            AdmissionDecision::RetryAfterCleanup => {
                let DiceStatus::Cleanup { future, epoch } = &self.data.dice_status else {
                    self.finish_update(command, attempt);
                    return Err(internal_error!(
                        "cleanup retry requested while DICE is not being cleaned"
                    ));
                };
                Ok(PostUpdateDecision::AwaitCleanup {
                    future: future.clone(),
                    epoch: *epoch,
                })
            }
            AdmissionDecision::StartCleanup(request) => {
                let epoch = request.epoch;
                // `wait_for_idle` enqueues its DICE query before returning. A live caller retains
                // its transaction until this reply; a cancelled caller has already dropped it, so
                // either ordering preserves the cleanup handshake.
                let future = self.dice.wait_for_idle().boxed().shared();
                let effect = self.data.apply_event(AdmissionEvent::CleanupPrepared {
                    request,
                    future: future.clone(),
                });
                self.apply_effect(effect);
                Ok(PostUpdateDecision::AwaitCleanup { future, epoch })
            }
            AdmissionDecision::Block(blocking) => {
                let (wake, receiver) = oneshot::channel();
                assert!(
                    self.blocked_waiters.insert(command, wake).is_none(),
                    "a command cannot register two blocked waits"
                );
                Ok(PostUpdateDecision::Block {
                    blocking,
                    effects,
                    wake: receiver,
                })
            }
            decision => Ok(PostUpdateDecision::Outcome(AdmissionOutcome {
                decision,
                effects,
            })),
        };
        self.finish_update(command, attempt);
        result
    }

    fn apply_effect(&mut self, effect: StateEffect) {
        if matches!(effect, StateEffect::WakeWaiters) {
            let waiters = std::mem::take(&mut self.blocked_waiters);
            waiters.into_iter().for_each(|(_, waiter)| {
                let _ignored = waiter.send(());
            });
        }
    }
}

/// Event interface used while admitting a command.
pub trait CommandEvents: Dupe + Send + Sync + 'static {
    fn instant(&self, data: buck2_data::instant_event::Data);
    fn trace_id(&self) -> &TraceId;
    /// Show a warning on the command's console.
    fn console_warning(&self, message: String);
    /// `span` must preserve span-entering semantics: spans created inside `fut` parent to this span,
    /// and poll time accumulates into the end event. Delegating to an implementation built on
    /// `EventDispatcher::span_async` does this; starting a span, awaiting, then ending it does not.
    fn span<'a, R: Send + 'a>(
        &self,
        start: buck2_data::span_start_event::Data,
        fut: BoxFuture<'a, (R, buck2_data::span_end_event::Data)>,
    ) -> BoxFuture<'a, R>;
}

#[async_trait]
pub trait DiceUpdater: Send + Sync {
    /// Records the command's changes on `ctx` and reports the file watcher's sync outcome. After
    /// a fresh instance the handler moves to a fresh branch.
    async fn update(
        &self,
        mut ctx: DiceTransactionUpdater,
        early_timings: &mut EarlyCommandTimingBuilder,
    ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>;
}

/// Per-command work that needs the committed `DiceTransaction` but is not part of deciding whether
/// the command may run.
///
/// The command is registered before this is invoked, and the concurrency lock is not held. Calls
/// for DICE-equivalent transactions may overlap and have no ordering guarantee, so implementations
/// must synchronize their own shared state. A different-state transaction is not admitted until
/// registered commands finish. Returning `Err` deregisters the command and wakes waiters.
#[async_trait]
pub trait CommandTransactionObserver: Send + Sync {
    async fn on_transaction_committed(
        &self,
        transaction: &DiceTransaction,
    ) -> buck2_error::Result<()>;
}

/// Exclusive commands that hold the lock or are queued for it, oldest first.
///
/// Entries are keyed by ticket rather than positional, because a command can leave the queue in
/// an order other than it joined: cancellation removes an entry that never reached the front.
type ExclusiveWaiters = Arc<parking_lot::Mutex<VecDeque<(u64, String)>>>;

#[derive(Allocative)]
struct ExclusiveCommandLock {
    lock: tokio::sync::RwLock<()>,
    waiters: ExclusiveWaiters,
    next_ticket: AtomicU64,
}

/// Removes its entry from the queue on drop, whether the command acquired the lock or was
/// cancelled while waiting for it.
struct ExclusiveWaiter {
    waiters: ExclusiveWaiters,
    ticket: u64,
}

impl Drop for ExclusiveWaiter {
    fn drop(&mut self) {
        self.waiters
            .lock()
            .retain(|(ticket, _)| *ticket != self.ticket);
    }
}

#[allow(dead_code)] // lock guards are held, not read
enum ExclusiveCommandLockGuard<'a> {
    Shared(tokio::sync::RwLockReadGuard<'a, ()>),
    /// The waiter is first so that it drops first: the name leaves the queue before the write
    /// lock is released, so `owning_command` never names a command that has already finished.
    Exclusive(ExclusiveWaiter, tokio::sync::RwLockWriteGuard<'a, ()>),
    /// A nested invocation, which does not take the gate at all. See `enter`.
    Bypassed,
}

impl ExclusiveCommandLock {
    pub fn new() -> Self {
        ExclusiveCommandLock {
            lock: tokio::sync::RwLock::new(()),
            waiters: Arc::new(parking_lot::Mutex::new(VecDeque::new())),
            next_ticket: AtomicU64::new(0),
        }
    }

    pub async fn exclusive_lock<'a>(&'a self, cmd_name: String) -> ExclusiveCommandLockGuard<'a> {
        // Joined before awaiting the lock, so a command that is queued but not yet holding can
        // still be named as the thing others are waiting on. `waiter` owns the removal, so
        // dropping this future before the lock is granted takes the entry with it.
        let waiter = ExclusiveWaiter {
            waiters: self.waiters.dupe(),
            ticket: self.next_ticket.fetch_add(1, Ordering::Relaxed),
        };
        self.waiters.lock().push_back((waiter.ticket, cmd_name));

        let guard = self.lock.write().await;
        ExclusiveCommandLockGuard::Exclusive(waiter, guard)
    }

    pub async fn shared_lock<'a>(&'a self) -> ExclusiveCommandLockGuard<'a> {
        ExclusiveCommandLockGuard::Shared(self.lock.read().await)
    }

    /// The exclusive command holding the lock, or the oldest one queued for it. `None` when no
    /// exclusive command is in play.
    pub fn owning_command(&self) -> Option<String> {
        self.waiters.lock().front().map(|(_, name)| name.clone())
    }
}

impl ConcurrencyHandler {
    /// A handler whose commands run on `branch`.
    pub fn new(dice: Arc<Dice>, branch: BranchId) -> Arc<Self> {
        Self::with_branch_ownership(dice, branch, false, None)
    }

    /// Retains disk ownership until the admission task and its DICE handle have shut down.
    pub fn new_with_lifetime_guard(
        dice: Arc<Dice>,
        branch: BranchId,
        lifetime_guard: Option<Arc<dyn Send + Sync>>,
    ) -> Arc<Self> {
        Self::with_branch_ownership(dice, branch, false, lifetime_guard)
    }

    /// Own an isolated root, releasing it after retirement and all admitted commands drain.
    pub fn new_owned(dice: Arc<Dice>, branch: BranchId) -> Arc<Self> {
        Self::new_owned_with_lifetime_guard(dice, branch, None)
    }

    /// Retains disk ownership through root retirement and admitted-command cleanup.
    pub fn new_owned_with_lifetime_guard(
        dice: Arc<Dice>,
        branch: BranchId,
        lifetime_guard: Option<Arc<dyn Send + Sync>>,
    ) -> Arc<Self> {
        Self::with_branch_ownership(dice, branch, true, lifetime_guard)
    }

    fn with_branch_ownership(
        dice: Arc<Dice>,
        branch: BranchId,
        owns_branch: bool,
        lifetime_guard: Option<Arc<dyn Send + Sync>>,
    ) -> Arc<Self> {
        Arc::new(ConcurrencyHandler {
            coordinator: AdmissionCoordinator::with_branch_ownership(
                dice.dupe(),
                branch,
                owns_branch,
                lifetime_guard,
            ),
            dice,
            owns_branch,
            exclusive_command_lock: ExclusiveCommandLock::new(),
            next_command_id: AtomicUsize::new(0),
        })
    }

    /// Allocates the next `CommandId`. Returns a distinct value to every caller.
    fn allocate_command_id(&self) -> CommandId {
        CommandId(self.next_command_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Enters a critical section that requires concurrent command synchronization,
    /// and runs the given `exec` function in the critical section.
    pub async fn enter<F, Fut, R, E>(
        self: &Arc<Self>,
        events: E,
        updates: &dyn DiceUpdater,
        exec: F,
        is_nested_invocation: bool,
        sanitized_argv: Vec<String>,
        exclusive_cmd: Option<String>,
        cancellations: &CancellationContext,
        preemptible: PreemptibleWhen,
        transaction_observer: &dyn CommandTransactionObserver,
        exit_when: ExitWhen,
        mut early_command_timing: EarlyCommandTimingBuilder,
    ) -> buck2_error::Result<R>
    where
        F: FnOnce(DiceTransaction, EarlyCommandTimingBuilder) -> Fut,
        Fut: Future<Output = R> + Send,
        E: CommandEvents,
    {
        let _exclusive_command_guard = events
            .span(
                ExclusiveCommandWaitStart {
                    command_name: self.exclusive_command_lock.owning_command(),
                }
                .into(),
                Box::pin({
                    let early_command_timing = &mut early_command_timing;
                    async move {
                        let guard = if let Some(cmd_name) = exclusive_cmd {
                            early_command_timing.start_span(EXCLUSIVE_COMMAND_WAIT.to_owned());
                            let guard = self.exclusive_command_lock.exclusive_lock(cmd_name).await;
                            self.dice.wait_for_idle().await;

                            guard
                        } else if is_nested_invocation {
                            // A writer-preferring lock can deadlock a nested invocation behind a
                            // writer waiting for its parent. Once a writer holds the gate, no parent
                            // remains from which a nested invocation can originate.
                            ExclusiveCommandLockGuard::Bypassed
                        } else {
                            self.exclusive_command_lock.shared_lock().await
                        };
                        (guard, ExclusiveCommandWaitEnd {}.into())
                    }
                }),
            )
            .await;

        let command_id = self.allocate_command_id();
        let (preempt_sender, preempt_receiver) = oneshot::channel::<()>();
        let request = AdmissionRequest {
            command_id,
            command: CommandData {
                trace_id: events.trace_id().dupe(),
                display_command: format_command(&sanitized_argv),
                preemption_setting: preemptible,
                preempt: Some(preempt_sender),
            },
            is_nested: is_nested_invocation,
            exit_when,
        };
        let lease = CommandLease::new(self.coordinator.clone(), command_id);

        let inner_events = events.dupe();
        let admission = events
            .span(DiceSynchronizeSectionStart {}.into(), {
                let early_command_timing = &mut early_command_timing;

                Box::pin(async move {
                    (
                        cancellations
                            .critical_section(|| {
                                self.wait_for_others(
                                    updates,
                                    early_command_timing,
                                    inner_events,
                                    request,
                                    transaction_observer,
                                )
                            })
                            .await,
                        DiceSynchronizeSectionEnd {}.into(),
                    )
                })
            })
            .await;
        let transaction = match admission {
            Ok(transaction) => transaction,
            Err(error) => {
                lease.release().await;
                return Err(error);
            }
        };

        let result = exec(transaction, early_command_timing);
        pin_mut!(result);
        pin_mut!(preempt_receiver);

        let result = match future::select(result, preempt_receiver).await {
            Either::Left((result, _)) => Ok(result),
            Either::Right((_preemption, _)) => {
                events.instant(CommandPreempted {}.into());
                Err(ConcurrencyHandlerError::ExitOnPreemption.into())
            }
        };
        lease.release().await;
        result
    }

    /// How long a command may block before its user is first told what it is queued
    /// behind. Short, so that a wedged blocking command is identifiable quickly.
    const BLOCKED_COMMAND_FIRST_WARNING: Duration = Duration::from_secs(60);

    /// How often to repeat the warning after the first one. Longer, so a legitimately
    /// slow blocking command doesn't flood the console.
    const BLOCKED_COMMAND_WARNING_INTERVAL: Duration = Duration::from_secs(5 * 60);

    async fn complete_update<E: CommandEvents>(
        &self,
        pending: PendingUpdate,
        updates: &dyn DiceUpdater,
        early_timings: &mut EarlyCommandTimingBuilder,
        events: &E,
    ) -> buck2_error::Result<UpdatedTransaction> {
        // `enter` holds the command lease across this call. Any error or cancellation releases
        // the command, and `release_command` also releases an update lane owned by that command.
        let branch = self
            .coordinator
            .acquire_update(pending.command, pending.attempt)
            .await?;
        let dice_was_draining = pending.dice_was_draining.await;

        let updater = self.dice.updater(branch);
        let (mut updater, user_data, outcome) = updates.update(updater, early_timings).await?;
        if outcome == SyncOutcome::FreshInstance {
            self.start_over(pending.command, pending.attempt, &mut updater, branch)
                .await?;
        }
        let transaction = events
            .span(
                buck2_data::DiceStateUpdateStart {}.into(),
                Box::pin(async {
                    (
                        buck2_error::Ok(updater.commit_with_data(user_data).await),
                        buck2_data::DiceStateUpdateEnd {}.into(),
                    )
                }),
            )
            .await?;
        let version = transaction.version();

        Ok(UpdatedTransaction {
            transaction,
            update: CompletedUpdate {
                attempt: pending.attempt,
                version,
                conflict_on_arrival: pending.conflict_on_arrival,
                dice_was_draining,
            },
        })
    }

    async fn await_cleanup<E: CommandEvents>(
        &self,
        events: &E,
        future: Shared<BoxFuture<'static, ()>>,
        epoch: usize,
    ) -> buck2_error::Result<()> {
        events
            .span(
                buck2_data::DiceCleanupStart { epoch: epoch as _ }.into(),
                Box::pin(async move { (future.await, buck2_data::DiceCleanupEnd {}.into()) }),
            )
            .await;

        self.coordinator.cleanup_completed(epoch).await
    }

    async fn wait_for_blocked_command<E: CommandEvents>(
        &self,
        events: &E,
        effects: AdmissionEffects,
        blocking: BlockingCommand,
        wake: oneshot::Receiver<()>,
    ) -> buck2_error::Result<()> {
        effects.execute(events);

        events
            .span(
                DiceBlockConcurrentCommandStart {
                    current_active_trace_id: blocking.trace_id.to_string(),
                    cmd_args: blocking.display_command.clone(),
                }
                .into(),
                Box::pin(async {
                    pin_mut!(wake);
                    let mut waited = Duration::ZERO;
                    let mut next_warning = Self::BLOCKED_COMMAND_FIRST_WARNING;
                    let result = loop {
                        match timeout(next_warning, &mut wake).await {
                            Ok(Ok(())) => break buck2_error::Ok(()),
                            Ok(Err(_)) => {
                                break Err(internal_error!(
                                    "concurrency admission coordinator stopped"
                                ));
                            }
                            Err(_elapsed) => {
                                waited += next_warning;
                                next_warning = Self::BLOCKED_COMMAND_WARNING_INTERVAL;
                                events.console_warning(format!(
                                    "This command has been waiting for {} for another command to finish: [{}] (trace ID: {}). \
                                     If that command is not making progress, restarting the buck2 daemon with `buck2 kill` will unblock both",
                                    format_elapsed(waited),
                                    blocking.display_command,
                                    blocking.trace_id,
                                ));
                            }
                        }
                    };
                    (
                        result,
                        DiceBlockConcurrentCommandEnd {
                            ending_active_trace_id: blocking.trace_id.to_string(),
                        }
                        .into(),
                    )
                }),
            )
            .await
    }

    async fn finish_admission<E: CommandEvents>(
        &self,
        events: &E,
        transaction_observer: &dyn CommandTransactionObserver,
        ready: ReadyToExecute,
    ) -> buck2_error::Result<DiceTransaction> {
        let AdmittedCommand {
            state,
            nested_warning,
        } = ready.admitted;
        let (tainted, previously_tainted, no_active_dice_state) = match state {
            AdmittedState::Fresh {
                tainted,
                previously_tainted,
            } => (tainted, previously_tainted, true),
            AdmittedState::Concurrent {
                previously_tainted, ..
            } => (false, previously_tainted, false),
        };

        ready.effects.execute(events);

        if no_active_dice_state {
            events.instant(NoActiveDiceState {}.into());
        }
        if previously_tainted {
            events.instant(
                buck2_data::TagEvent {
                    tags: vec!["concurrency-previously-tainted".to_owned()],
                }
                .into(),
            );
        }
        if tainted {
            events.instant(
                buck2_data::TagEvent {
                    tags: vec!["concurrency-tainted".to_owned()],
                }
                .into(),
            );
        }

        if let Some(NestedInvocationWarning {
            running,
            display_command,
        }) = nested_warning
        {
            soft_error!(
                "nested_invocation_same_dice_state",
                ConcurrencyHandlerError::NestedInvocationWithSameStates(running, display_command)
                    .into(),
            );
        }

        transaction_observer
            .on_transaction_committed(&ready.transaction)
            .await?;

        Ok(ready.transaction)
    }

    async fn wait_for_others<E: CommandEvents>(
        self: &Arc<Self>,
        updates: &dyn DiceUpdater,
        early_timings: &mut EarlyCommandTimingBuilder,
        events: E,
        request: AdmissionRequest,
        transaction_observer: &dyn CommandTransactionObserver,
    ) -> buck2_error::Result<DiceTransaction> {
        #![expect(
            clippy::await_holding_invalid_type,
            reason = "the tracing span must remain entered across this operation"
        )]

        let trace = events.trace_id().dupe();

        let span = tracing::span!(tracing::Level::DEBUG, "wait_for_others", trace = %trace);
        // FIXME(JakobDegen): Clippy points out that tracing won't know when this future gets
        // descheduled from this executor thread, so this may show up in the wrong places
        let _enter = span.enter();

        let command_id = request.command_id;
        let mut before_update = self.coordinator.begin(request).await?;

        let ready = loop {
            match &before_update {
                PreUpdateDecision::WaitForCleanupStart
                | PreUpdateDecision::WaitForCleanup { .. } => {
                    tracing::debug!("ActiveDice is in cleanup");
                }
                PreUpdateDecision::RejectNotIdle { .. } | PreUpdateDecision::Update { .. } => {
                    tracing::debug!("ActiveDice is available");
                }
            }

            let (attempt, conflict_on_arrival) = match before_update {
                PreUpdateDecision::WaitForCleanupStart => {
                    return Err(internal_error!("coordinator exposed an unprepared cleanup"));
                }
                PreUpdateDecision::WaitForCleanup { future, epoch } => {
                    self.await_cleanup(&events, future, epoch).await?;
                    before_update = self.coordinator.before_update(command_id).await?;
                    continue;
                }
                PreUpdateDecision::RejectNotIdle { running } => {
                    let queued = self.coordinator.queued_traces(command_id).await?;
                    return Err(ConcurrencyHandlerError::ExitOnDaemonNotIdle)
                        .with_buck_error_context(|| {
                            format!(
                                "Buck daemon is busy processing another command: {running}{queued}"
                            )
                        });
                }
                PreUpdateDecision::Update {
                    attempt,
                    conflict_on_arrival,
                } => (attempt, conflict_on_arrival),
            };

            let pending_update = PendingUpdate {
                command: command_id,
                attempt,
                conflict_on_arrival,
                // Only work left over from dropped transactions is interference. Concurrent
                // commands that are still syncing hold transactions that compute nothing yet, so
                // live transactions are deliberately not counted. Enqueued before the update so
                // that the sample describes the state the command arrived at; the returned future
                // can be awaited after the coordinator reply.
                dice_was_draining: self.dice.is_draining().boxed(),
            };

            // we rerun the updates in case that files on disk have changed between commands.
            // this might cause some churn, but concurrent commands don't happen much and
            // isn't a big perf bottleneck. Dice should be able to resurrect nodes properly.
            //
            // The coordinator serializes updates without awaiting them, so it can continue
            // processing command exits while this command is talking to the file watcher.
            let UpdatedTransaction {
                transaction,
                update,
            } = self
                .complete_update(pending_update, updates, early_timings, &events)
                .await?;

            match self.coordinator.after_update(command_id, update).await? {
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision:
                        AdmissionDecision::RejectDifferentState {
                            running,
                            compared_with_active_state,
                        },
                    effects,
                }) => {
                    if compared_with_active_state {
                        tracing::debug!("ActiveDice has an active_transaction");
                    }
                    effects.execute(&events);
                    let queued = self.coordinator.queued_traces(command_id).await?;
                    return Err(ConcurrencyHandlerError::ExitWhenDifferentState)
                        .with_buck_error_context(|| {
                            format!(
                                "Buck daemon is busy processing another command: {running}{queued}"
                            )
                        });
                }
                PostUpdateDecision::AwaitCleanup { future, epoch } => {
                    // Dropping the transaction releases its `ActiveTransactionGuard`. The retry
                    // awaits the cleanup future, which cannot complete while that guard is alive.
                    drop(transaction);
                    self.await_cleanup(&events, future, epoch).await?;
                    before_update = self.coordinator.before_update(command_id).await?;
                    continue;
                }
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision:
                        AdmissionDecision::Admit(
                            admitted @ AdmittedCommand {
                                state: AdmittedState::Fresh { .. },
                                ..
                            },
                        ),
                    effects,
                }) => {
                    tracing::debug!("ActiveDice has no active_transaction");
                    break ReadyToExecute {
                        transaction,
                        admitted,
                        effects,
                    };
                }
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision:
                        AdmissionDecision::RejectNestedDifferentState(NestedInvocationWarning {
                            running,
                            display_command,
                        }),
                    effects,
                }) => {
                    tracing::debug!("ActiveDice has an active_transaction");
                    effects.execute(&events);
                    return Err(
                        ConcurrencyHandlerError::NestedInvocationWithDifferentStates(
                            running,
                            display_command,
                        )
                        .into(),
                    );
                }
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision:
                        AdmissionDecision::Admit(
                            admitted @ AdmittedCommand {
                                state: AdmittedState::Concurrent { .. },
                                ..
                            },
                        ),
                    effects,
                }) => {
                    tracing::debug!("ActiveDice has an active_transaction");
                    break ReadyToExecute {
                        transaction,
                        admitted,
                        effects,
                    };
                }
                PostUpdateDecision::Block {
                    blocking,
                    effects,
                    wake,
                } => {
                    tracing::debug!("ActiveDice has an active_transaction");
                    drop(transaction);
                    self.wait_for_blocked_command(&events, effects, blocking, wake)
                        .await?;
                    before_update = self.coordinator.before_update(command_id).await?;
                }
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision:
                        AdmissionDecision::RetryAfterCleanup
                        | AdmissionDecision::StartCleanup(_)
                        | AdmissionDecision::Block(_),
                    ..
                }) => {
                    return Err(internal_error!(
                        "coordinator returned an unprocessed admission decision"
                    ));
                }
            }
        };

        tracing::info!("Acquired access to DICE");
        let transaction = self
            .finish_admission(&events, transaction_observer, ready)
            .await?;
        Ok(transaction)
    }

    /// Moves the handler to a fresh root, with `updater`'s changes going there. The branch left
    /// behind is deleted once nothing runs on it any more.
    ///
    /// Deleting it at once would strand the commands still running on it: a lookup on a deleted
    /// branch resolves nothing, injected keys included, so such a command would hit the
    /// missing-injection panic on the first injected key it had not seen. Waiting lets them finish
    /// with their cache, and releases everything the branch held then.
    async fn start_over(
        &self,
        command: CommandId,
        attempt: UpdateAttemptId,
        updater: &mut DiceTransactionUpdater,
        old: BranchId,
    ) -> buck2_error::Result<()> {
        let fresh = if self.owns_branch {
            self.dice.new_root_from(old).await
        } else {
            self.dice.new_root().await
        };
        self.coordinator
            .replace_root(command, attempt, fresh)
            .await?;
        updater.retarget(fresh);
        Ok(())
    }

    /// Stop admission and release the root after cancelled commands and their work terminate.
    pub fn retire(&self) {
        let _ignored = self.coordinator.sender.send(CoordinatorMessage::Retire);
    }

    /// Access dice without locking for dumps.
    pub fn unsafe_dice(&self) -> &Arc<Dice> {
        &self.dice
    }
}

fn determine_bypass_semaphore(is_same_state: bool, is_nested_invocation: bool) -> BypassSemaphore {
    if is_same_state {
        if is_nested_invocation {
            BypassSemaphore::Run(RunState::NestedSameState)
        } else {
            BypassSemaphore::Run(RunState::ParallelSameState)
        }
    } else if is_nested_invocation {
        BypassSemaphore::Error
    } else {
        BypassSemaphore::Block
    }
}

fn format_command(argv: &[String]) -> String {
    let mut iter = argv.iter();
    // Skip the executable path so the displayed command consistently starts with `buck2`.
    iter.next();

    truncate(&format!("buck2 {}", iter.join(" ")), 500)
}

/// Formats an elapsed wait in whole minutes, or seconds while under a minute, so the
/// message stays meaningful for any `BLOCKED_COMMAND_WARNING_INTERVAL`.
fn format_elapsed(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{}s", elapsed.as_secs())
    } else {
        format!("{}m", elapsed.as_secs() / 60)
    }
}

/// Trace IDs captured by the coordinator and formatted by the command task.
#[derive(Debug)]
struct ConcurrentTraces(Vec<TraceId>);

impl ConcurrentTraces {
    fn running_and(
        active_commands: &BuckMutInsertionMap<CommandId, CommandData>,
        current: &CommandData,
    ) -> Self {
        Self(
            active_commands
                .values()
                .chain(std::iter::once(current))
                .map(|cmd| cmd.trace_id.dupe())
                .collect(),
        )
    }

    fn running(active_commands: &BuckMutInsertionMap<CommandId, CommandData>) -> Self {
        Self(
            active_commands
                .values()
                .map(|cmd| cmd.trace_id.dupe())
                .collect(),
        )
    }
}

/// Releases a command from the coordinator whether it is pending or active.
struct CommandLease {
    coordinator: AdmissionCoordinator,
    command: Option<CommandId>,
}

impl CommandLease {
    fn new(coordinator: AdmissionCoordinator, command: CommandId) -> Self {
        Self {
            coordinator,
            command: Some(command),
        }
    }

    async fn release(mut self) {
        let command = self
            .command
            .take()
            .expect("a command lease can only be released once");
        if let Err(error) = self.coordinator.release_and_wait(command).await {
            soft_error_or_panic!("command_lease_release_failed", error);
        }
    }
}

impl Drop for CommandLease {
    fn drop(&mut self) {
        if let Some(command) = self.command.take() {
            tracing::info!("Command has exited: {command}");
            self.coordinator.release(command);
        }
    }
}

/// A trailing clause that renders nothing when the queue is empty.
struct QueuedTraces(ConcurrentTraces);

impl fmt::Display for QueuedTraces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.0.is_empty() {
            return Ok(());
        }
        write!(f, ". Queued behind: {}", self.0)
    }
}

impl fmt::Display for ConcurrentTraces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `TraceId` is not unique across concurrently live entries — a command with a null
        // dispatcher reports `TraceId::null()` — so the same id can appear twice.
        write!(
            f,
            "{}",
            self.0
                .iter()
                .collect::<BuckMutInsertionSet<_>>()
                .iter()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::task::Poll;
    use std::time::Duration;
    use std::time::Instant;

    use allocative::Allocative;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use buck2_core::is_open_source;
    use buck2_util::early_command_timing::EXCLUSIVE_COMMAND_WAIT;
    use buck2_util::early_command_timing::FILE_WATCHER_WAIT;
    use derivative::Derivative;
    use dice::DetectCycles;
    use dice::DiceComputations;
    use dice::EqualityBehavior;
    use dice::InjectedKey;
    use dice::Key;
    use dice::PagableValueSerialize;
    use dice::ValueSerialize;
    use dice_futures::cancellation::CancellationContext;
    use dupe::Dupe;
    use futures::pin_mut;
    use futures::poll;
    use pagable::Pagable;
    use pagable::pagable_typetag;
    use parking_lot::Mutex;
    use tokio::sync::Barrier;
    use tokio::sync::RwLock;
    use tokio::sync::Semaphore;

    use super::*;

    /// Recording stand-in for `EventDispatcher`. The real one lives in `buck2_events`, which this
    /// crate deliberately does not depend on — including in `test_deps`, since those are linked
    /// into the `-unittest` binary that coverage instruments.
    #[derive(Clone, Dupe)]
    struct TestEvents(Arc<TestEventsInner>);

    struct TestEventsInner {
        trace_id: TraceId,
        recorded: Mutex<Vec<RecordedEvent>>,
        expected_active_commands_on_equality: Option<(Arc<ConcurrencyHandler>, usize)>,
    }

    #[derive(Clone, Debug)]
    enum RecordedEvent {
        Instant(buck2_data::instant_event::Data),
        SpanStart(buck2_data::span_start_event::Data),
        SpanEnd(buck2_data::span_end_event::Data),
    }

    impl TestEvents {
        fn new() -> Self {
            Self::with_trace(TraceId::new())
        }

        fn with_trace(trace_id: TraceId) -> Self {
            Self(Arc::new(TestEventsInner {
                trace_id,
                recorded: Mutex::new(Vec::new()),
                expected_active_commands_on_equality: None,
            }))
        }

        fn expect_active_commands_on_equality(
            concurrency: Arc<ConcurrencyHandler>,
            expected: usize,
        ) -> Self {
            Self(Arc::new(TestEventsInner {
                trace_id: TraceId::new(),
                recorded: Mutex::new(Vec::new()),
                expected_active_commands_on_equality: Some((concurrency, expected)),
            }))
        }

        fn recorded(&self) -> Vec<RecordedEvent> {
            self.0.recorded.lock().clone()
        }

        /// Waits for a recorded event matching `pred`. Bounded, because a regression should fail in
        /// seconds rather than hang the harness.
        async fn wait_for<F>(&self, pred: F) -> buck2_error::Result<RecordedEvent>
        where
            F: Fn(&RecordedEvent) -> bool,
        {
            self.wait_from(&mut 0, pred).await
        }

        /// As [`Self::wait_for`], but resumes from `cursor` and advances it past the match, so a
        /// sequence of waits observes distinct events the way reading from a channel did.
        async fn wait_from<F>(
            &self,
            cursor: &mut usize,
            pred: F,
        ) -> buck2_error::Result<RecordedEvent>
        where
            F: Fn(&RecordedEvent) -> bool,
        {
            // Short timeouts are too flaky in OD environments under load.
            let (idx, event) = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let recorded = self.recorded();
                    if let Some((i, e)) = recorded
                        .iter()
                        .enumerate()
                        .skip(*cursor)
                        .find(|(_, e)| pred(e))
                    {
                        break (i, e.clone());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .buck_error_context("Timed out waiting for a matching event")?;
            *cursor = idx + 1;
            Ok(event)
        }
    }

    impl CommandEvents for TestEvents {
        fn instant(&self, data: buck2_data::instant_event::Data) {
            if matches!(&data, buck2_data::instant_event::Data::DiceEqualityCheck(_))
                && let Some((concurrency, expected)) = &self.0.expected_active_commands_on_equality
            {
                assert_eq!(
                    concurrency.coordinator.active_command_count(),
                    *expected,
                    "DICE equality was reported before registration"
                );
            }
            self.0.recorded.lock().push(RecordedEvent::Instant(data));
        }

        fn trace_id(&self) -> &TraceId {
            &self.0.trace_id
        }

        fn console_warning(&self, _message: String) {
            // User-facing chrome only; no test asserts on it.
        }

        /// Records only: no span parenting, poll timing, or `SpanCancelled` on drop.
        fn span<'a, R: Send + 'a>(
            &self,
            start: buck2_data::span_start_event::Data,
            fut: BoxFuture<'a, (R, buck2_data::span_end_event::Data)>,
        ) -> BoxFuture<'a, R> {
            self.0.recorded.lock().push(RecordedEvent::SpanStart(start));
            let this = self.dupe();
            Box::pin(async move {
                let (r, end) = fut.await;
                this.0.recorded.lock().push(RecordedEvent::SpanEnd(end));
                r
            })
        }
    }

    /// Matches a recorded `TagEvent` carrying `tag`.
    fn is_tag_event(tag: &'static str) -> impl Fn(&RecordedEvent) -> bool {
        move |e: &RecordedEvent| match e {
            RecordedEvent::Instant(buck2_data::instant_event::Data::TagEvent(t)) => {
                t.tags.iter().any(|it| it == tag)
            }
            _ => false,
        }
    }

    /// The production observer emits buckconfig telemetry; concurrency behaviour does not depend
    /// on it, so tests use one that does nothing.
    struct NoTelemetry;

    #[async_trait]
    impl CommandTransactionObserver for NoTelemetry {
        async fn on_transaction_committed(
            &self,
            _transaction: &DiceTransaction,
        ) -> buck2_error::Result<()> {
            Ok(())
        }
    }

    struct FailingUpdater;

    #[async_trait]
    impl DiceUpdater for FailingUpdater {
        async fn update(
            &self,
            _ctx: DiceTransactionUpdater,
            _early_timings: &mut EarlyCommandTimingBuilder,
        ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
        {
            Err(internal_error!("updater failed"))
        }
    }

    struct BlockingUpdater {
        entered: Arc<Barrier>,
    }

    #[async_trait]
    impl DiceUpdater for BlockingUpdater {
        async fn update(
            &self,
            _ctx: DiceTransactionUpdater,
            _early_timings: &mut EarlyCommandTimingBuilder,
        ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
        {
            self.entered.wait().await;
            future::pending().await
        }
    }

    struct FailingObserver;

    #[async_trait]
    impl CommandTransactionObserver for FailingObserver {
        async fn on_transaction_committed(
            &self,
            _transaction: &DiceTransaction,
        ) -> buck2_error::Result<()> {
            Err(internal_error!("observer failed"))
        }
    }

    /// Signals entry into `on_transaction_committed`, then blocks until released.
    struct BlockingObserver {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
        fail: bool,
    }

    #[async_trait]
    impl CommandTransactionObserver for BlockingObserver {
        async fn on_transaction_committed(
            &self,
            _transaction: &DiceTransaction,
        ) -> buck2_error::Result<()> {
            self.entered.wait().await;
            self.release.wait().await;
            if self.fail {
                return Err(internal_error!("observer failed"));
            }
            Ok(())
        }
    }

    struct NoChanges;

    #[async_trait]
    impl DiceUpdater for NoChanges {
        async fn update(
            &self,
            ctx: DiceTransactionUpdater,
            _early_timings: &mut EarlyCommandTimingBuilder,
        ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
        {
            Ok((ctx, Default::default(), SyncOutcome::Incremental))
        }
    }

    struct CtxDifferent;

    #[async_trait]
    impl DiceUpdater for CtxDifferent {
        async fn update(
            &self,
            mut ctx: DiceTransactionUpdater,
            _early_timings: &mut EarlyCommandTimingBuilder,
        ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
        {
            ctx.changed_to(vec![(K, ())])?;
            Ok((ctx, Default::default(), SyncOutcome::Incremental))
        }
    }

    fn updater_for_state_match(same_state: bool) -> &'static dyn DiceUpdater {
        if same_state {
            &NoChanges
        } else {
            &CtxDifferent
        }
    }

    #[derive(Clone, Dupe, Display, Debug, Hash, Eq, PartialEq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct K;

    #[async_trait]
    impl InjectedKey for K {
        type Value = ();

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| false)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            PagableValueSerialize::<Self::Value>::new()
        }
    }

    /// The concurrency manager itself reads no injected keys; the buckconfig data the old inline
    /// telemetry needed is now the observer's concern, and tests use `NoTelemetry`.
    async fn make_default_dice() -> (Arc<Dice>, BranchId) {
        let dice = Dice::builder().build(DetectCycles::Enabled);
        let root = dice.new_root().await;
        (dice, root)
    }

    async fn make_handler() -> Arc<ConcurrencyHandler> {
        let (dice, root) = make_default_dice().await;
        ConcurrencyHandler::new(dice, root)
    }

    /// Distinctness of `CommandId` used to be a consequence of the read-modify-write happening
    /// under the state lock. It is now the atomic's job, so it is worth pinning directly: a
    /// collision otherwise surfaces far from its cause, as the duplicate-registration
    /// `internal_error!` in `register_command`.
    ///
    /// The multi-threaded flavour is load-bearing: on the default current-thread runtime the tasks
    /// never overlap, and a non-atomic read-modify-write passes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrently_allocated_command_ids_are_distinct() {
        const TASKS: usize = 8;
        const PER_TASK: usize = 1024;

        let concurrency = make_handler().await;
        let start = Arc::new(tokio::sync::Barrier::new(TASKS));

        let handles: Vec<_> = (0..TASKS)
            .map(|_| {
                let concurrency = concurrency.dupe();
                let start = start.dupe();
                tokio::spawn(async move {
                    start.wait().await;
                    (0..PER_TASK)
                        .map(|_| concurrency.allocate_command_id())
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        let mut seen = BuckMutInsertionSet::new();
        for handle in handles {
            for id in handle.await.unwrap() {
                assert!(seen.insert(id), "duplicate CommandId `{id}`");
            }
        }

        assert_eq!(seen.len(), TASKS * PER_TASK);
    }

    #[tokio::test]
    async fn a_queued_command_is_named_only_while_it_waits() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;
        let asking = concurrency.allocate_command_id();
        assert_eq!(
            concurrency
                .coordinator
                .queued_traces(asking)
                .await?
                .to_string(),
            ""
        );

        let trace = TraceId::new();
        let command_id = concurrency.allocate_command_id();
        {
            let _lease = CommandLease::new(concurrency.coordinator.clone(), command_id);
            let _decision = concurrency
                .coordinator
                .begin(AdmissionRequest {
                    command_id,
                    command: CommandData {
                        trace_id: trace.dupe(),
                        display_command: "buck2".to_owned(),
                        preemption_setting: PreemptibleWhen::Never,
                        preempt: None,
                    },
                    is_nested: false,
                    exit_when: ExitWhen::ExitNever,
                })
                .await?;
            assert_eq!(
                concurrency
                    .coordinator
                    .queued_traces(asking)
                    .await?
                    .to_string(),
                format!(". Queued behind: {trace}")
            );

            // The command being told is queued too, and must not be named to itself.
            assert_eq!(
                concurrency
                    .coordinator
                    .queued_traces(command_id)
                    .await?
                    .to_string(),
                ""
            );
            assert_eq!(
                concurrency.coordinator.snapshot().await?.pending_commands,
                1
            );
        }

        assert_eq!(
            concurrency
                .coordinator
                .queued_traces(asking)
                .await?
                .to_string(),
            ""
        );
        assert_eq!(
            concurrency.coordinator.snapshot().await?.pending_commands,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_dropped_begin_reply_releases_pending_ownership() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;
        let command_id = concurrency.allocate_command_id();
        let (response, receiver) = oneshot::channel();
        drop(receiver);

        assert!(
            concurrency
                .coordinator
                .sender
                .send(CoordinatorMessage::Begin {
                    request: AdmissionRequest {
                        command_id,
                        command: CommandData {
                            trace_id: TraceId::new(),
                            display_command: "buck2".to_owned(),
                            preemption_setting: PreemptibleWhen::Never,
                            preempt: None,
                        },
                        is_nested: false,
                        exit_when: ExitWhen::ExitNever,
                    },
                    response,
                })
                .is_ok(),
            "the coordinator should still be running"
        );

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(snapshot.pending_commands, 0);
        assert_eq!(snapshot.active_commands, 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_dropped_admission_reply_releases_active_ownership() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);
        let command_id = concurrency.allocate_command_id();
        let decision = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?;
        let PreUpdateDecision::Update {
            attempt,
            conflict_on_arrival: None,
        } = decision
        else {
            panic!("an idle coordinator should request an update");
        };

        let version = dice.updater(root).commit().await.version();
        concurrency
            .coordinator
            .acquire_update(command_id, attempt)
            .await?;
        let (response, receiver) = oneshot::channel();
        drop(receiver);
        assert!(
            concurrency
                .coordinator
                .sender
                .send(CoordinatorMessage::AfterUpdate {
                    command: command_id,
                    update: CompletedUpdate {
                        attempt,
                        version,
                        conflict_on_arrival: None,
                        dice_was_draining: false,
                    },
                    response,
                })
                .is_ok(),
            "the coordinator should still be running"
        );

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(snapshot.pending_commands, 0);
        assert_eq!(snapshot.active_commands, 0);

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_dropped_cleanup_reply_still_finishes_cleanup() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);
        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        let command_id = concurrency.allocate_command_id();
        let PreUpdateDecision::Update {
            conflict_on_arrival,
            attempt,
        } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("an available coordinator should request an update");
        };

        let mut updater = dice.updater(root);
        updater.changed_to(vec![(K, ())])?;
        let transaction = updater.commit().await;
        concurrency
            .coordinator
            .acquire_update(command_id, attempt)
            .await?;
        let (response, receiver) = oneshot::channel();
        drop(receiver);
        assert!(
            concurrency
                .coordinator
                .sender
                .send(CoordinatorMessage::AfterUpdate {
                    command: command_id,
                    update: CompletedUpdate {
                        attempt,
                        version: transaction.version(),
                        conflict_on_arrival,
                        dice_was_draining: true,
                    },
                    response,
                })
                .is_ok(),
            "the coordinator should still be running"
        );

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert!(snapshot.cleanup_in_progress);
        assert_eq!(snapshot.pending_commands, 0);
        assert_eq!(snapshot.active_commands, 0);
        drop(transaction);

        timeout(Duration::from_secs(10), async {
            loop {
                if !concurrency
                    .coordinator
                    .snapshot()
                    .await?
                    .cleanup_in_progress
                {
                    return Ok::<(), buck2_error::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the abandoned cleanup should finish")?;
        Ok(())
    }

    #[tokio::test]
    async fn coordinator_shutdown_wakes_blocked_waiters_and_stops() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let first_version = dice.updater(root).commit().await.version();
        let different_version = {
            let mut updater = dice.updater(root);
            updater.changed_to(vec![(K, ())])?;
            updater.commit().await.version()
        };
        let (sender, receiver) = mpsc::unbounded_channel();
        let task = AdmissionCoordinatorTask::new(dice, root, sender.downgrade(), None);
        let active_commands = task.active_commands.dupe();
        let task = tokio::spawn(task.run_with_branch_ownership(receiver, false));
        let coordinator = AdmissionCoordinator {
            sender,
            active_commands,
        };

        let first = CommandId(0);
        let PreUpdateDecision::Update {
            attempt: first_attempt,
            ..
        } = coordinator
            .begin(AdmissionRequest {
                command_id: first,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 first".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("the first command should update");
        };
        coordinator.acquire_update(first, first_attempt).await?;
        let first_decision = coordinator
            .after_update(
                first,
                CompletedUpdate {
                    attempt: first_attempt,
                    version: first_version,
                    conflict_on_arrival: None,
                    dice_was_draining: false,
                },
            )
            .await?;
        assert!(
            matches!(
                first_decision,
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision: AdmissionDecision::Admit(_),
                    ..
                })
            ),
            "the first command should be admitted"
        );

        let blocked = CommandId(1);
        let PreUpdateDecision::Update {
            attempt: blocked_attempt,
            ..
        } = coordinator
            .begin(AdmissionRequest {
                command_id: blocked,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 blocked".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("the blocked command should update");
        };
        coordinator.acquire_update(blocked, blocked_attempt).await?;
        let PostUpdateDecision::Block { wake, .. } = coordinator
            .after_update(
                blocked,
                CompletedUpdate {
                    attempt: blocked_attempt,
                    version: different_version,
                    conflict_on_arrival: Some(first_version),
                    dice_was_draining: true,
                },
            )
            .await?
        else {
            panic!("the different-state command should block");
        };

        drop(coordinator);
        assert!(
            timeout(Duration::from_secs(10), wake)
                .await
                .expect("the blocked waiter should wake when the coordinator stops")
                .is_err(),
            "a stopped coordinator must close blocked wake channels"
        );
        timeout(Duration::from_secs(10), task)
            .await
            .expect("the coordinator task should stop after its last sender drops")
            .expect("the coordinator task should not panic");
        Ok(())
    }

    async fn dropped_reply_preserves_preemption(
        preemption_setting: PreemptibleWhen,
        same_state: bool,
    ) -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let first_version = dice.updater(root).commit().await.version();
        let different_version = {
            let mut updater = dice.updater(root);
            updater.changed_to(vec![(K, ())])?;
            updater.commit().await.version()
        };
        let arriving_version = if same_state {
            first_version
        } else {
            different_version
        };
        let concurrency = ConcurrencyHandler::new(dice, root);

        let first_id = concurrency.allocate_command_id();
        let (preempt, mut preempted) = oneshot::channel();
        let PreUpdateDecision::Update {
            attempt: first_attempt,
            ..
        } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id: first_id,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 first".to_owned(),
                    preemption_setting,
                    preempt: Some(preempt),
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("the first command should update");
        };
        concurrency
            .coordinator
            .acquire_update(first_id, first_attempt)
            .await?;
        assert!(
            matches!(
                concurrency
                    .coordinator
                    .after_update(
                        first_id,
                        CompletedUpdate {
                            attempt: first_attempt,
                            version: first_version,
                            conflict_on_arrival: None,
                            dice_was_draining: false,
                        },
                    )
                    .await?,
                PostUpdateDecision::Outcome(AdmissionOutcome {
                    decision: AdmissionDecision::Admit(_),
                    ..
                })
            ),
            "the first command should be admitted"
        );

        let cancelled_id = concurrency.allocate_command_id();
        let PreUpdateDecision::Update {
            attempt: cancelled_attempt,
            ..
        } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id: cancelled_id,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 cancelled".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("the cancelled command should update");
        };
        concurrency
            .coordinator
            .acquire_update(cancelled_id, cancelled_attempt)
            .await?;
        let (response, receiver) = oneshot::channel();
        drop(receiver);
        assert!(
            concurrency
                .coordinator
                .sender
                .send(CoordinatorMessage::AfterUpdate {
                    command: cancelled_id,
                    update: CompletedUpdate {
                        attempt: cancelled_attempt,
                        version: arriving_version,
                        conflict_on_arrival: Some(first_version),
                        dice_was_draining: true,
                    },
                    response,
                })
                .is_ok(),
            "the coordinator should still be running"
        );

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(snapshot.pending_commands, 0);
        assert_eq!(snapshot.active_commands, 1);
        assert_matches!(
            preempted.try_recv(),
            Err(oneshot::error::TryRecvError::Empty),
            "a cancelled arrival must not preempt the active command"
        );

        let retry_id = concurrency.allocate_command_id();
        let PreUpdateDecision::Update {
            attempt: retry_attempt,
            ..
        } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id: retry_id,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 retry".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("the retry should update");
        };
        concurrency
            .coordinator
            .acquire_update(retry_id, retry_attempt)
            .await?;
        let effects = match concurrency
            .coordinator
            .after_update(
                retry_id,
                CompletedUpdate {
                    attempt: retry_attempt,
                    version: arriving_version,
                    conflict_on_arrival: Some(first_version),
                    dice_was_draining: true,
                },
            )
            .await?
        {
            PostUpdateDecision::Outcome(AdmissionOutcome {
                decision: AdmissionDecision::Admit(_),
                effects,
            }) => effects,
            PostUpdateDecision::Block { effects, .. } => effects,
            _ => panic!("the retry should compare against the active command"),
        };
        effects.execute(&TestEvents::new());
        preempted
            .await
            .expect("the retry should retain the ability to preempt");

        concurrency.coordinator.release_and_wait(retry_id).await?;
        concurrency.coordinator.release_and_wait(first_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_dropped_same_state_reply_preserves_always_preemption() -> buck2_error::Result<()> {
        dropped_reply_preserves_preemption(PreemptibleWhen::Always, true).await
    }

    #[tokio::test]
    async fn a_dropped_different_state_reply_preserves_conditional_preemption()
    -> buck2_error::Result<()> {
        dropped_reply_preserves_preemption(PreemptibleWhen::OnDifferentState, false).await
    }

    /// Blocked commands are not in `active_commands` — nothing registers until after the wait —
    /// so the pending registry is what makes them visible in a "daemon is busy" message.
    #[tokio::test]
    async fn a_blocked_command_is_named_as_queued() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;

        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let barrier = Arc::new(Barrier::new(2));

        let running_trace = TraceId::new();
        let queued_trace = TraceId::new();

        let running = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier.dupe();
            let block = block.dupe();
            let trace = running_trace.dupe();
            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(trace),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = block.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier.wait().await;

        // A differing state, so this one blocks rather than joining.
        let queued = tokio::spawn({
            let concurrency = concurrency.dupe();
            let trace = queued_trace.dupe();
            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(trace),
                        &CtxDifferent,
                        |_, _timing| async move {},
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        // Waiting for it to reach the blocking path, rather than merely to be spawned. `probe` is
        // never registered, so nothing is filtered out of the view it gets.
        let probe = concurrency.allocate_command_id();
        tokio::time::timeout(Duration::from_secs(10), async {
            while concurrency
                .coordinator
                .queued_traces(probe)
                .await
                .expect("coordinator should be running")
                .to_string()
                .is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second command never blocked");

        let refused = concurrency
            .enter(
                TestEvents::new(),
                &NoChanges,
                |_, _timing| async move {},
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNotIdle,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await
            .expect_err("the daemon is not idle");

        let message = format!("{refused:?}");
        assert!(
            message.contains(&running_trace.to_string()),
            "the running command should be named: {message}"
        );
        assert!(
            message.contains(&queued_trace.to_string()),
            "the queued command should be named: {message}"
        );

        drop(blocked);
        running.await??;
        queued.await??;

        Ok(())
    }

    /// A command cancelled while queued for the exclusive lock never acquires it, so it never
    /// produces a guard whose drop could remove it.
    #[tokio::test]
    async fn a_cancelled_exclusive_command_stops_being_named() {
        let lock = ExclusiveCommandLock::new();

        let held = lock.exclusive_lock("first".to_owned()).await;
        assert_eq!(lock.owning_command().as_deref(), Some("first"));

        {
            let mut queued = Box::pin(lock.exclusive_lock("second".to_owned()));
            assert!(poll!(&mut queued).is_pending(), "`first` still holds it");
            assert_eq!(
                lock.owning_command().as_deref(),
                Some("first"),
                "a queued command must not displace the holder"
            );
            // Dropping `queued` cancels `second` before it ever acquires.
        }

        drop(held);

        // Asserted via a third command rather than by expecting `None` here: the previous
        // implementation would also report `None` at this point, because it inferred emptiness
        // from `try_read()` succeeding rather than from the queue. Taking the lock again is what
        // exposes the abandoned entry.
        let third = lock.exclusive_lock("third".to_owned()).await;
        assert_eq!(
            lock.owning_command().as_deref(),
            Some("third"),
            "a cancelled command was still being reported as the owner"
        );

        drop(third);
        assert_eq!(lock.owning_command(), None);
    }

    /// The duplicate-id path is argued unreachable while `CommandId` is monotonic, so this pins
    /// what happens if that argument ever stops holding: a refusal that leaves the existing
    /// registration intact, rather than one that reports an error about state it just destroyed.
    #[tokio::test]
    async fn duplicate_registration_refuses_without_evicting_the_first() -> buck2_error::Result<()>
    {
        fn command_with(preempt: Option<oneshot::Sender<()>>) -> CommandData {
            CommandData {
                trace_id: TraceId::new(),
                display_command: "buck2".to_owned(),
                preemption_setting: PreemptibleWhen::Never,
                preempt,
            }
        }

        let command_id = CommandId(0);
        let mut data = CoordinatorState {
            dice_status: DiceStatus::idle(),
            active_commands: BuckMutInsertionMap::new(),
            pending_commands: BuckMutInsertionMap::new(),
            next_update_attempt: 0,
            cleanup_epoch: 0,
            previously_tainted: false,
        };

        let (preempt_sender, _preempt_receiver) = oneshot::channel::<()>();
        data.register_command(command_id, command_with(Some(preempt_sender)))?;

        let duplicate = data.register_command(command_id, command_with(None));
        assert!(duplicate.is_err(), "a repeated id should be refused");

        let registered = data
            .active_commands
            .get(&command_id)
            .expect("the first command was evicted by the refused one");
        assert!(
            registered.preempt.is_some(),
            "the first command's preempt channel was replaced"
        );

        Ok(())
    }

    #[tokio::test]
    async fn command_exit_wakes_waiters_only_after_the_last_command() -> buck2_error::Result<()> {
        let command = || CommandData {
            trace_id: TraceId::new(),
            display_command: "buck2".to_owned(),
            preemption_setting: PreemptibleWhen::Never,
            preempt: None,
        };
        let first_id = CommandId(0);
        let second_id = CommandId(1);
        let mut data = CoordinatorState {
            dice_status: DiceStatus::idle(),
            active_commands: BuckMutInsertionMap::new(),
            pending_commands: BuckMutInsertionMap::new(),
            next_update_attempt: 0,
            cleanup_epoch: 0,
            previously_tainted: false,
        };
        let first = data.register_command(first_id, command())?;
        let second = data.register_command(second_id, command())?;

        assert_eq!(
            data.apply_event(AdmissionEvent::CommandExited(first)),
            StateEffect::None
        );
        assert_eq!(data.active_commands.len(), 1);
        assert_eq!(
            data.apply_event(AdmissionEvent::CommandExited(second)),
            StateEffect::WakeWaiters
        );
        assert!(data.active_commands.is_empty());

        Ok(())
    }

    /// Rendering moved out of the critical section into this `Display`, and nothing else covers
    /// it: no test asserts on the text of the messages it feeds.
    #[test]
    fn concurrent_traces_names_each_command_once() {
        let a = TraceId::new();
        let b = TraceId::new();

        assert_eq!(ConcurrentTraces(vec![]).to_string(), "");
        assert_eq!(ConcurrentTraces(vec![a.dupe()]).to_string(), a.to_string());
        assert_eq!(
            ConcurrentTraces(vec![a.dupe(), b.dupe()]).to_string(),
            format!("{a}, {b}")
        );

        // A command with a null dispatcher reports `TraceId::null()`, so the same id genuinely
        // reaches this type twice. Naming it once is the point of the dedup.
        assert_eq!(
            ConcurrentTraces(vec![a.dupe(), b.dupe(), a.dupe()]).to_string(),
            format!("{a}, {b}")
        );
    }

    /// The `Debug` impl is hand-written, so it needs its own check — in particular that it elides
    /// the cleanup future, which is the only reason it is not derived.
    #[tokio::test]
    async fn debug_reports_the_state_and_elides_the_cleanup_future() {
        let idle = format!("{:?}", DiceStatus::idle());
        assert!(idle.contains("Available"), "{idle}");
        assert!(idle.contains("None"), "{idle}");

        let (dice, root) = make_default_dice().await;
        let active = format!(
            "{:?}",
            DiceStatus::active(dice.updater(root).commit().await.version())
        );
        assert!(active.contains("Available"), "{active}");
        assert!(active.contains("ActiveDice"), "{active}");

        let cleanup = format!(
            "{:?}",
            DiceStatus::Cleanup {
                future: futures::future::ready(()).boxed().shared(),
                epoch: 4,
            }
        );
        assert!(cleanup.contains("Cleanup"), "{cleanup}");
        assert!(cleanup.contains('4'), "epoch should be visible: {cleanup}");
        assert!(
            !cleanup.contains("waker_key"),
            "the cleanup future should be elided, not printed: {cleanup}"
        );
    }

    /// Builder for a test call to [`ConcurrencyHandler::enter`], which otherwise takes twelve
    /// arguments of which most tests care about one or two. Defaults describe a plain command:
    /// not nested, not exclusive, never preemptible, never exiting early.
    struct TestCommand {
        dispatcher: TestEvents,
        preemptible: PreemptibleWhen,
        exit_when: ExitWhen,
        is_nested_invocation: bool,
        exclusive_cmd: Option<String>,
    }

    impl TestCommand {
        fn new() -> Self {
            Self {
                dispatcher: TestEvents::new(),
                preemptible: PreemptibleWhen::Never,
                exit_when: ExitWhen::ExitNever,
                is_nested_invocation: false,
                exclusive_cmd: None,
            }
        }

        fn dispatcher(mut self, dispatcher: TestEvents) -> Self {
            self.dispatcher = dispatcher;
            self
        }

        fn preemptible(mut self, preemptible: PreemptibleWhen) -> Self {
            self.preemptible = preemptible;
            self
        }

        fn exit_when(mut self, exit_when: ExitWhen) -> Self {
            self.exit_when = exit_when;
            self
        }

        fn nested_invocation(mut self, is_nested_invocation: bool) -> Self {
            self.is_nested_invocation = is_nested_invocation;
            self
        }

        fn exclusive_cmd(mut self, cmd_name: &str) -> Self {
            self.exclusive_cmd = Some(cmd_name.to_owned());
            self
        }

        async fn run<F, Fut, R>(
            self,
            concurrency: &Arc<ConcurrencyHandler>,
            updates: &dyn DiceUpdater,
            exec: F,
        ) -> buck2_error::Result<R>
        where
            F: FnOnce(DiceTransaction, EarlyCommandTimingBuilder) -> Fut,
            Fut: Future<Output = R> + Send,
        {
            self.run_with_observer(concurrency, updates, &NoTelemetry, exec)
                .await
        }

        async fn run_with_observer<F, Fut, R>(
            self,
            concurrency: &Arc<ConcurrencyHandler>,
            updates: &dyn DiceUpdater,
            observer: &dyn CommandTransactionObserver,
            exec: F,
        ) -> buck2_error::Result<R>
        where
            F: FnOnce(DiceTransaction, EarlyCommandTimingBuilder) -> Fut,
            Fut: Future<Output = R> + Send,
        {
            concurrency
                .enter(
                    self.dispatcher,
                    updates,
                    exec,
                    self.is_nested_invocation,
                    Vec::new(),
                    self.exclusive_cmd,
                    CancellationContext::testing(),
                    self.preemptible,
                    observer,
                    self.exit_when,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        }
    }

    /// Waits until the coordinator has processed all command releases.
    async fn wait_for_commands_to_be_reaped(
        concurrency: &ConcurrencyHandler,
    ) -> buck2_error::Result<()> {
        // Short timeouts are too flaky in OD environments under load.
        tokio::time::timeout(Duration::from_secs(10), async {
            while concurrency.coordinator.active_command_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .buck_error_context("Timed out waiting for finished commands to be deregistered")
    }

    async fn wait_for_coordinator_counts(
        concurrency: &ConcurrencyHandler,
        pending: usize,
        active: usize,
    ) -> buck2_error::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = concurrency.coordinator.snapshot().await?;
                if snapshot.pending_commands == pending && snapshot.active_commands == active {
                    return buck2_error::Ok(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .buck_error_context("Timed out waiting for coordinator command counts")?
    }

    #[tokio::test]
    async fn command_release_precedes_the_next_admission() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        TestCommand::new()
            .exit_when(ExitWhen::ExitNotIdle)
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        Ok(())
    }

    #[tokio::test]
    async fn cancellation_during_and_behind_update_releases_pending_commands()
    -> buck2_error::Result<()> {
        let concurrency = make_handler().await;
        let entered = Arc::new(Barrier::new(2));
        let updater = Arc::new(BlockingUpdater {
            entered: entered.dupe(),
        });

        let updating = tokio::spawn({
            let concurrency = concurrency.dupe();
            let updater = updater.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, updater.as_ref(), |_, _timing| async move {})
                    .await
            }
        });
        entered.wait().await;

        let queued = tokio::spawn({
            let concurrency = concurrency.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, &NoChanges, |_, _timing| async move {})
                    .await
            }
        });
        wait_for_coordinator_counts(&concurrency, 2, 0).await?;
        let snapshot = concurrency.coordinator.snapshot().await?;
        assert!(snapshot.update_in_flight);
        assert_eq!(snapshot.update_waiters, 1);

        queued.abort();
        assert!(
            queued
                .await
                .expect_err("the queued update should be cancelled")
                .is_cancelled()
        );
        wait_for_coordinator_counts(&concurrency, 1, 0).await?;
        let snapshot = concurrency.coordinator.snapshot().await?;
        assert!(snapshot.update_in_flight);
        assert_eq!(snapshot.update_waiters, 0);

        updating.abort();
        assert!(
            updating
                .await
                .expect_err("the running update should be cancelled")
                .is_cancelled()
        );
        wait_for_coordinator_counts(&concurrency, 0, 0).await?;
        let snapshot = concurrency.coordinator.snapshot().await?;
        assert!(!snapshot.update_in_flight);
        assert_eq!(snapshot.update_waiters, 0);

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn update_lane_survives_a_thousand_cancelled_waiters() -> buck2_error::Result<()> {
        const COMMANDS: usize = 1_000;

        let concurrency = make_handler().await;
        let mut command_ids = Vec::with_capacity(COMMANDS);
        let mut receivers = Vec::with_capacity(COMMANDS);

        for index in 0..COMMANDS {
            let command = concurrency.allocate_command_id();
            let PreUpdateDecision::Update { attempt, .. } = concurrency
                .coordinator
                .begin(AdmissionRequest {
                    command_id: command,
                    command: CommandData {
                        trace_id: TraceId::new(),
                        display_command: format!("buck2 storm {index}"),
                        preemption_setting: PreemptibleWhen::Never,
                        preempt: None,
                    },
                    is_nested: false,
                    exit_when: ExitWhen::ExitNever,
                })
                .await?
            else {
                panic!("an idle coordinator should prepare every update");
            };
            let (response, receiver) = oneshot::channel();
            concurrency
                .coordinator
                .sender
                .send(CoordinatorMessage::AcquireUpdate {
                    command,
                    attempt,
                    response,
                })
                .map_err(|_| internal_error!("concurrency admission coordinator stopped"))?;
            command_ids.push(command);
            receivers.push(receiver);
        }

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(snapshot.pending_commands, COMMANDS);
        assert!(snapshot.update_in_flight);
        assert_eq!(snapshot.update_waiters, COMMANDS - 1);

        drop(receivers);
        command_ids
            .into_iter()
            .for_each(|command| concurrency.coordinator.release(command));

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(snapshot.pending_commands, 0);
        assert!(!snapshot.update_in_flight);
        assert_eq!(snapshot.update_waiters, 0);

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_a_blocked_command_releases_pending_ownership() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;
        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let entered = Arc::new(Barrier::new(2));

        let active = tokio::spawn({
            let concurrency = concurrency.dupe();
            let block = block.dupe();
            let entered = entered.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, &NoChanges, |_, _timing| async move {
                        entered.wait().await;
                        let _guard = block.read().await;
                    })
                    .await
            }
        });
        entered.wait().await;

        let events = TestEvents::new();
        let waiter = tokio::spawn({
            let concurrency = concurrency.dupe();
            let events = events.dupe();
            async move {
                TestCommand::new()
                    .dispatcher(events)
                    .run(&concurrency, &CtxDifferent, |_, _timing| async move {})
                    .await
            }
        });
        events
            .wait_for(|event| {
                matches!(
                    event,
                    RecordedEvent::SpanStart(
                        buck2_data::span_start_event::Data::DiceBlockConcurrentCommand(..)
                    )
                )
            })
            .await?;

        waiter.abort();
        assert!(
            waiter
                .await
                .expect_err("the blocked command should be cancelled")
                .is_cancelled()
        );
        wait_for_coordinator_counts(&concurrency, 0, 1).await?;

        drop(blocked);
        active.await??;
        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_during_observation_releases_active_ownership() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;
        let entered = Arc::new(Barrier::new(2));
        let observer = Arc::new(BlockingObserver {
            entered: entered.dupe(),
            release: Arc::new(Barrier::new(2)),
            fail: false,
        });

        let command = tokio::spawn({
            let concurrency = concurrency.dupe();
            let observer = observer.dupe();
            async move {
                TestCommand::new()
                    .run_with_observer(
                        &concurrency,
                        &NoChanges,
                        observer.as_ref(),
                        |_, _timing| async move {},
                    )
                    .await
            }
        });
        entered.wait().await;
        wait_for_coordinator_counts(&concurrency, 0, 1).await?;

        command.abort();
        assert!(
            command
                .await
                .expect_err("the observed command should be cancelled")
                .is_cancelled()
        );
        wait_for_coordinator_counts(&concurrency, 0, 0).await?;

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_while_awaiting_cleanup_leaves_the_handler_usable() -> buck2_error::Result<()>
    {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        let key = ControlledCleanupTestKey {
            entered: Arc::new(Barrier::new(2)),
            release: Arc::new(Barrier::new(2)),
        };
        {
            let transaction = dice.updater(root).commit().await;
            let compute = transaction.compute(&key).fuse();
            let entered = key.entered.wait().fuse();
            futures::pin_mut!(compute);
            futures::pin_mut!(entered);

            futures::select! {
                _ = compute => panic!("compute finished before it was released"),
                _ = entered => {}
            }
        }
        assert!(
            !dice.is_idle().await,
            "DICE should have a task pending cancellation"
        );

        let events = TestEvents::new();
        let cleaning = tokio::spawn({
            let concurrency = concurrency.dupe();
            let events = events.dupe();
            async move {
                TestCommand::new()
                    .dispatcher(events)
                    .run(&concurrency, &CtxDifferent, |_, _timing| async move {})
                    .await
            }
        });
        events
            .wait_for(|event| {
                matches!(
                    event,
                    RecordedEvent::SpanStart(buck2_data::span_start_event::Data::DiceCleanup(..))
                )
            })
            .await?;

        cleaning.abort();
        assert!(
            cleaning
                .await
                .expect_err("the cleanup waiter should be cancelled")
                .is_cancelled()
        );
        wait_for_coordinator_counts(&concurrency, 0, 0).await?;

        key.release.wait().await;
        TestCommand::new()
            .run(&concurrency, &CtxDifferent, |_, _timing| async move {})
            .await?;
        Ok(())
    }

    /// Direct tests of the `CoordinatorState` state machine.
    ///
    /// These exercise the transition methods as plain functions rather than through `enter`. That
    /// makes cases reachable that the command path cannot produce deterministically — notably the
    /// stale-epoch guard, which via `enter` needs three commands and a specific scheduling delay.
    ///
    /// The trade is that these pin the transitions' **contract**, not their **reachability**. That
    /// a guard behaves correctly when handed a stale epoch does not demonstrate that a stale epoch
    /// can arise in practice; only an interleaving test would show that, and it needs a
    /// deterministically drivable arbiter.
    mod state_machine {
        use super::*;

        fn cleanup_at(epoch: usize) -> DiceStatus {
            DiceStatus::Cleanup {
                future: futures::future::ready(()).boxed().shared(),
                epoch,
            }
        }

        async fn active_status(dice: &Arc<Dice>, root: BranchId) -> DiceStatus {
            DiceStatus::active(dice.updater(root).commit().await.version())
        }

        fn data_with(dice_status: DiceStatus, cleanup_epoch: usize) -> CoordinatorState {
            CoordinatorState {
                dice_status,
                active_commands: BuckMutInsertionMap::new(),
                pending_commands: BuckMutInsertionMap::new(),
                next_update_attempt: 0,
                cleanup_epoch,
                previously_tainted: false,
            }
        }

        fn a_command() -> CommandData {
            CommandData {
                trace_id: TraceId::new(),
                display_command: "buck2".to_owned(),
                preemption_setting: PreemptibleWhen::Never,
                preempt: None,
            }
        }

        fn request(is_nested: bool, exit_when: ExitWhen) -> AdmissionRequest {
            static NEXT_COMMAND_ID: AtomicUsize = AtomicUsize::new(10_000);

            AdmissionRequest {
                command_id: CommandId(NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed)),
                command: a_command(),
                is_nested,
                exit_when,
            }
        }

        fn completed_update(
            version: Version,
            conflict_on_arrival: Option<Version>,
        ) -> CompletedUpdate {
            CompletedUpdate {
                attempt: UpdateAttemptId(0),
                version,
                conflict_on_arrival,
                dice_was_draining: false,
            }
        }

        fn decide_after_update(
            data: &mut CoordinatorState,
            request: AdmissionRequest,
            update: CompletedUpdate,
        ) -> AdmissionOutcome {
            match data
                .decide_after_update(&request, update)
                .expect("test command should be eligible for admission")
            {
                PendingAdmissionDecision::Retain(outcome) => outcome,
                PendingAdmissionDecision::Admit { state, effects } => {
                    data.admit_command(request, state, effects)
                }
            }
        }

        async fn distinct_versions(dice: &Arc<Dice>, root: BranchId) -> (Version, Version) {
            let first = dice.updater(root).commit().await.version();
            let mut updater = dice.updater(root);
            updater
                .changed_to(vec![(K, ())])
                .expect("test update should be valid");
            let second = updater.commit().await.version();
            assert_ne!(first, second, "test requires distinct DICE states");
            (first, second)
        }

        #[tokio::test]
        async fn transition_to_idle_completes_the_matching_cleanup() {
            let mut data = data_with(cleanup_at(3), 3);
            assert_eq!(
                data.apply_event(AdmissionEvent::CleanupCompleted { epoch: 3 }),
                StateEffect::None
            );
            assert_matches!(data.dice_status, DiceStatus::Available { active: None });
        }

        /// Guard A. Two commands can await the same cleanup future; the first to reacquire the lock
        /// completes the transition and may install a fresh `ActiveDice`. The second must not then
        /// clear it.
        #[tokio::test]
        async fn transition_to_idle_is_a_noop_once_already_available() {
            let (dice, root) = make_default_dice().await;
            let mut data = data_with(active_status(&dice, root).await, 3);
            assert_eq!(
                data.apply_event(AdmissionEvent::CleanupCompleted { epoch: 3 }),
                StateEffect::None
            );
            assert_matches!(
                data.dice_status,
                DiceStatus::Available { active: Some(..) },
                "an already-completed cleanup must not discard the active DICE version"
            );
        }

        /// Guard B. A waiter parked across an entire later command can return holding an epoch that
        /// has since been superseded by a *new* cleanup. Completing that stale cleanup would
        /// release commands to run against a DICE state that has not drained.
        #[tokio::test]
        async fn transition_to_idle_is_a_noop_for_a_superseded_epoch() {
            let mut data = data_with(cleanup_at(4), 4);
            assert_eq!(
                data.apply_event(AdmissionEvent::CleanupCompleted { epoch: 3 }),
                StateEffect::None
            );
            assert_matches!(
                data.dice_status,
                DiceStatus::Cleanup { epoch: 4, .. },
                "a stale waiter must not complete a newer cleanup"
            );
        }

        #[tokio::test]
        async fn cleanup_handshake_advances_the_epoch_when_idle() {
            let (dice, root) = make_default_dice().await;
            let mut data = data_with(active_status(&dice, root).await, 7);

            let request = data
                .begin_cleanup()
                .expect("idle state should begin cleanup");
            assert_eq!(data.cleanup_epoch, 8);
            assert_eq!(request.epoch, 8);
            assert_matches!(
                data.dice_status,
                DiceStatus::CleanupStarting { epoch: 8 },
                "the reducer should reserve the cleanup epoch before external work"
            );

            assert_eq!(
                data.apply_event(AdmissionEvent::CleanupPrepared {
                    request,
                    future: futures::future::ready(()).boxed().shared(),
                }),
                StateEffect::WakeWaiters
            );
            assert_matches!(data.dice_status, DiceStatus::Cleanup { epoch: 8, .. });
        }

        fn preemptible_command(setting: PreemptibleWhen) -> CommandData {
            // The receiver is dropped immediately; what is under test is whether the effect takes
            // the sender from the command.
            let (tx, _rx) = oneshot::channel();
            CommandData {
                trace_id: TraceId::new(),
                display_command: "buck2".to_owned(),
                preemption_setting: setting,
                preempt: Some(tx),
            }
        }

        /// Exhaustive mapping for `determine_bypass_semaphore`.
        ///
        /// Region coverage already shows every arm of this function executes, and would continue to
        /// show that if the arms were permuted — which input produces which outcome is precisely
        /// what region coverage cannot see, and what branch coverage would have caught. Hence a
        /// table.
        #[tokio::test]
        async fn bypass_semaphore_mapping() {
            assert_matches!(
                determine_bypass_semaphore(true, true),
                BypassSemaphore::Run(RunState::NestedSameState)
            );
            assert_matches!(
                determine_bypass_semaphore(true, false),
                BypassSemaphore::Run(RunState::ParallelSameState)
            );
            assert_matches!(
                determine_bypass_semaphore(false, true),
                BypassSemaphore::Error
            );
            assert_matches!(
                determine_bypass_semaphore(false, false),
                BypassSemaphore::Block
            );
        }

        #[tokio::test]
        async fn pre_update_decision_mapping() {
            let cleanup = data_with(cleanup_at(3), 3);
            let PreUpdateDecision::WaitForCleanup { epoch, .. } = cleanup
                .decide_before_update(&request(false, ExitWhen::ExitNever), UpdateAttemptId(0))
            else {
                panic!("cleanup must be awaited before updating");
            };
            assert_eq!(epoch, 3);

            let idle = data_with(DiceStatus::idle(), 0);
            let PreUpdateDecision::Update {
                conflict_on_arrival,
                ..
            } = idle
                .decide_before_update(&request(false, ExitWhen::ExitNotIdle), UpdateAttemptId(0))
            else {
                panic!("an idle handler should proceed to the update");
            };
            assert_eq!(conflict_on_arrival, None);

            let (dice, root) = make_default_dice().await;
            let active_version = dice.updater(root).commit().await.version();
            let mut active = data_with(DiceStatus::active(active_version), 0);

            let PreUpdateDecision::Update {
                conflict_on_arrival,
                ..
            } = active
                .decide_before_update(&request(false, ExitWhen::ExitNotIdle), UpdateAttemptId(0))
            else {
                panic!("an active DICE version without commands is not busy");
            };
            assert_eq!(conflict_on_arrival, None);

            let command = a_command();
            let command_trace = command.trace_id.dupe();
            active.active_commands.insert(CommandId(0), command);

            let PreUpdateDecision::RejectNotIdle { running } = active
                .decide_before_update(&request(false, ExitWhen::ExitNotIdle), UpdateAttemptId(0))
            else {
                panic!("an active command must trigger ExitNotIdle");
            };
            assert_eq!(running.to_string(), command_trace.to_string());

            let PreUpdateDecision::Update {
                conflict_on_arrival,
                ..
            } = active.decide_before_update(
                &request(false, ExitWhen::ExitDifferentState),
                UpdateAttemptId(0),
            )
            else {
                panic!("ExitDifferentState is decided after the update");
            };
            assert_eq!(conflict_on_arrival, Some(active_version));
        }

        #[tokio::test]
        async fn post_update_decision_mapping() {
            let (dice, root) = make_default_dice().await;
            let (active_version, different_version) = distinct_versions(&dice, root).await;

            let mut cleanup = data_with(cleanup_at(1), 1);
            assert_matches!(
                decide_after_update(
                    &mut cleanup,
                    request(false, ExitWhen::ExitNever),
                    completed_update(active_version, None),
                )
                .decision,
                AdmissionDecision::RetryAfterCleanup
            );

            let mut idle = data_with(DiceStatus::idle(), 0);
            assert_matches!(
                decide_after_update(
                    &mut idle,
                    request(false, ExitWhen::ExitNever),
                    completed_update(active_version, None),
                )
                .decision,
                AdmissionDecision::Admit(AdmittedCommand {
                    state: AdmittedState::Fresh {
                        tainted: false,
                        previously_tainted: false,
                    },
                    ..
                })
            );
            assert_matches!(
                idle.dice_status,
                DiceStatus::Available {
                    active: Some(ActiveDice { version })
                } if version == active_version,
                "fresh admission installs the completed transaction"
            );

            let mut active_without_commands = data_with(DiceStatus::active(active_version), 0);
            assert_matches!(
                decide_after_update(
                    &mut active_without_commands,
                    request(false, ExitWhen::ExitNever),
                    completed_update(different_version, None),
                )
                .decision,
                AdmissionDecision::StartCleanup(_)
            );

            let mut active = data_with(DiceStatus::active(active_version), 0);
            let blocking_command = a_command();
            let blocking_trace = blocking_command.trace_id.dupe();
            active
                .active_commands
                .insert(CommandId(0), blocking_command);

            assert_matches!(
                decide_after_update(
                    &mut active,
                    request(false, ExitWhen::ExitNever),
                    completed_update(active_version, None),
                )
                .decision,
                AdmissionDecision::Admit(AdmittedCommand {
                    state: AdmittedState::Concurrent {
                        state: RunState::ParallelSameState,
                        previously_tainted: false,
                    },
                    ..
                })
            );
            assert_matches!(
                decide_after_update(
                    &mut active,
                    request(true, ExitWhen::ExitNever),
                    completed_update(active_version, None),
                )
                .decision,
                AdmissionDecision::Admit(AdmittedCommand {
                    state: AdmittedState::Concurrent {
                        state: RunState::NestedSameState,
                        previously_tainted: false,
                    },
                    ..
                })
            );
            assert_matches!(
                decide_after_update(
                    &mut active,
                    request(true, ExitWhen::ExitNever),
                    completed_update(different_version, None),
                )
                .decision,
                AdmissionDecision::RejectNestedDifferentState(_)
            );
            let blocked = decide_after_update(
                &mut active,
                request(false, ExitWhen::ExitNever),
                completed_update(different_version, None),
            );
            let AdmissionDecision::Block(blocking) = blocked.decision else {
                panic!("a different-state command should block");
            };
            assert_eq!(blocking.trace_id, blocking_trace);
            assert_eq!(blocking.display_command, "buck2");
            let reject_after_update = decide_after_update(
                &mut active,
                request(false, ExitWhen::ExitDifferentState),
                completed_update(different_version, None),
            );
            let AdmissionDecision::RejectDifferentState {
                running,
                compared_with_active_state: true,
            } = reject_after_update.decision
            else {
                panic!("ExitDifferentState should reject the command");
            };
            assert!(running.to_string().contains(&blocking_trace.to_string()));
            assert!(
                matches!(
                    reject_after_update.effects.0.as_slice(),
                    [AdmissionEffect::DiceEquality(false)]
                ),
                "post-update rejection reports a failed state comparison"
            );

            let reject_from_arrival = decide_after_update(
                &mut active,
                request(false, ExitWhen::ExitDifferentState),
                completed_update(different_version, Some(active_version)),
            );
            assert_matches!(
                reject_from_arrival.decision,
                AdmissionDecision::RejectDifferentState {
                    compared_with_active_state: false,
                    ..
                }
            );
            assert!(
                reject_from_arrival.effects.0.is_empty(),
                "an arrival-time rejection does not compare against the current state"
            );
        }

        #[tokio::test]
        async fn admission_registers_and_carries_nested_warning() {
            let (dice, root) = make_default_dice().await;
            let version = dice.updater(root).commit().await.version();
            let mut data = data_with(DiceStatus::active(version), 0);

            let parent = a_command();
            let parent_trace = parent.trace_id.dupe();
            data.active_commands.insert(CommandId(0), parent);

            let request = request(true, ExitWhen::ExitNever);
            let command_id = request.command_id;
            let command_trace = request.command.trace_id.dupe();

            let outcome = decide_after_update(&mut data, request, completed_update(version, None));
            let AdmissionDecision::Admit(AdmittedCommand {
                nested_warning: Some(warning),
                ..
            }) = outcome.decision
            else {
                panic!("nested admission should carry a warning");
            };

            assert!(
                data.active_commands.contains_key(&command_id),
                "admission should atomically register the command"
            );
            assert!(
                !data.pending_commands.contains_key(&command_id),
                "the admitted command should leave pending state"
            );
            assert_eq!(
                warning.running.to_string(),
                format!("{parent_trace}, {command_trace}")
            );
        }

        #[tokio::test]
        async fn stale_update_attempt_does_not_mutate_pending_command() -> buck2_error::Result<()> {
            let (dice, root) = make_default_dice().await;
            let version = dice.updater(root).commit().await.version();
            let mut data = data_with(DiceStatus::idle(), 0);
            let request = request(false, ExitWhen::ExitNever);
            let command = request.command_id;

            let PreUpdateDecision::Update {
                attempt: stale_attempt,
                ..
            } = data.begin_admission(request)?
            else {
                panic!("the first admission should request an update");
            };
            let PreUpdateDecision::Update {
                attempt: current_attempt,
                ..
            } = data.decide_before_update_for(command)?
            else {
                panic!("the repeated decision should request an update");
            };
            assert_ne!(stale_attempt, current_attempt);

            let stale = data.decide_after_update_for(
                command,
                CompletedUpdate {
                    attempt: stale_attempt,
                    version,
                    conflict_on_arrival: None,
                    dice_was_draining: false,
                },
            );
            assert!(stale.is_err(), "a stale update must be rejected");
            assert!(data.pending_commands.contains_key(&command));
            assert!(data.active_commands.is_empty());

            let outcome = data.decide_after_update_for(
                command,
                CompletedUpdate {
                    attempt: current_attempt,
                    version,
                    conflict_on_arrival: None,
                    dice_was_draining: false,
                },
            )?;
            assert_matches!(outcome.decision, AdmissionDecision::Admit(_));
            assert!(!data.pending_commands.contains_key(&command));
            assert!(data.active_commands.contains_key(&command));
            Ok(())
        }

        #[tokio::test]
        async fn admission_reducer_sequence() {
            let (dice, root) = make_default_dice().await;
            let (first_version, second_version) = distinct_versions(&dice, root).await;
            let first_request = request(false, ExitWhen::ExitNever);
            let first_command = first_request.command_id;
            let mut data = data_with(DiceStatus::idle(), 0);

            assert!(
                matches!(
                    data.begin_admission(first_request)
                        .expect("the first command should begin admission"),
                    PreUpdateDecision::Update {
                        conflict_on_arrival: None,
                        ..
                    }
                ),
                "idle state proceeds without a conflict snapshot"
            );
            let AdmissionDecision::Admit(AdmittedCommand {
                state: AdmittedState::Fresh { .. },
                ..
            }) = data
                .decide_after_update_for(first_command, completed_update(first_version, None))
                .expect("the first command should be admitted")
                .decision
            else {
                panic!("the first command should install fresh state");
            };

            let (preempt, mut preempted) = oneshot::channel();
            data.active_commands.insert(
                CommandId(0),
                CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 build".to_owned(),
                    preemption_setting: PreemptibleWhen::OnDifferentState,
                    preempt: Some(preempt),
                },
            );

            let same_state_request = request(false, ExitWhen::ExitNever);
            let same_state_command = same_state_request.command_id;
            let same_state = decide_after_update(
                &mut data,
                same_state_request,
                completed_update(first_version, None),
            );
            let AdmissionDecision::Admit(AdmittedCommand {
                state:
                    AdmittedState::Concurrent {
                        state: RunState::ParallelSameState,
                        ..
                    },
                ..
            }) = same_state.decision
            else {
                panic!("same-state command should run concurrently");
            };
            same_state.effects.execute(&TestEvents::new());
            assert_matches!(
                preempted.try_recv(),
                Err(oneshot::error::TryRecvError::Empty),
                "a same-state command must not preempt OnDifferentState"
            );

            let different_state = decide_after_update(
                &mut data,
                request(false, ExitWhen::ExitNever),
                completed_update(second_version, None),
            );
            assert_matches!(different_state.decision, AdmissionDecision::Block(_));
            assert_matches!(
                preempted.try_recv(),
                Err(oneshot::error::TryRecvError::Empty),
                "effects are deferred until the state transition returns"
            );
            different_state.effects.execute(&TestEvents::new());
            preempted
                .await
                .expect("the different-state effect should preempt the active command");

            data.active_commands.shift_remove(&CommandId(0));
            data.active_commands.shift_remove(&first_command);
            data.active_commands.shift_remove(&same_state_command);
            let AdmissionDecision::StartCleanup(cleanup) = decide_after_update(
                &mut data,
                request(false, ExitWhen::ExitNever),
                completed_update(second_version, None),
            )
            .decision
            else {
                panic!("different state without active commands should begin cleanup");
            };
            assert!(
                matches!(
                    data.decide_before_update(
                        &request(false, ExitWhen::ExitNever),
                        UpdateAttemptId(0),
                    ),
                    PreUpdateDecision::WaitForCleanupStart
                ),
                "commands should wait until the cleanup future is installed"
            );
            assert_eq!(
                data.apply_event(AdmissionEvent::CleanupPrepared {
                    request: cleanup,
                    future: futures::future::ready(()).boxed().shared(),
                }),
                StateEffect::WakeWaiters
            );

            let PreUpdateDecision::WaitForCleanup { future, epoch } =
                data.decide_before_update(&request(false, ExitWhen::ExitNever), UpdateAttemptId(0))
            else {
                panic!("the next attempt must wait for cleanup");
            };
            future.await;
            assert_eq!(
                data.apply_event(AdmissionEvent::CleanupCompleted { epoch }),
                StateEffect::None
            );

            assert_matches!(
                decide_after_update(
                    &mut data,
                    request(false, ExitWhen::ExitNever),
                    completed_update(second_version, None),
                )
                .decision,
                AdmissionDecision::Admit(AdmittedCommand {
                    state: AdmittedState::Fresh { .. },
                    ..
                })
            );
            assert_matches!(
                data.dice_status,
                DiceStatus::Available {
                    active: Some(ActiveDice { version })
                } if version == second_version
            );
        }

        /// Exhaustive `PreemptibleWhen` x `is_same_state` matrix, for the same reason.
        ///
        /// All six are reachable — `Never` is the proto default, so it is the most common setting
        /// in production. What each call site fixes is `is_same_state`, not the setting: the site
        /// inside `if !is_same_state` always passes `false`, and the one under
        /// `BypassSemaphore::Run` always passes `true`. So both columns occur, and every row is
        /// exercised at both.
        #[tokio::test]
        async fn preemption_matrix() {
            let settings = [
                PreemptibleWhen::Never,
                PreemptibleWhen::Always,
                PreemptibleWhen::OnDifferentState,
            ];

            //                     Never, Always, OnDifferentState
            for (is_same_state, expected) in
                [(true, [false, true, false]), (false, [false, true, true])]
            {
                let mut data = data_with(cleanup_at(0), 0);
                for (i, setting) in settings.iter().enumerate() {
                    data.active_commands
                        .insert(CommandId(i), preemptible_command(*setting));
                }

                let effects = AdmissionEffects::after_state_comparison(&mut data, is_same_state);

                for (i, setting) in settings.iter().enumerate() {
                    let preempted = data
                        .active_commands
                        .get(&CommandId(i))
                        .unwrap()
                        .preempt
                        .is_none();
                    assert_eq!(
                        preempted, expected[i],
                        "{setting:?} with is_same_state={is_same_state}"
                    );
                }

                if is_same_state {
                    assert!(
                        matches!(effects.0.first(), Some(AdmissionEffect::DiceEquality(true))),
                        "same-state equality is reported before preemption"
                    );
                } else {
                    assert!(
                        matches!(effects.0.last(), Some(AdmissionEffect::DiceEquality(false))),
                        "different-state equality is reported after preemption"
                    );
                }
            }
        }

        #[tokio::test]
        async fn begin_cleanup_refuses_while_commands_are_active() {
            let (dice, root) = make_default_dice().await;
            let mut data = data_with(active_status(&dice, root).await, 7);
            data.active_commands.insert(CommandId(0), a_command());

            assert_eq!(data.begin_cleanup(), None);
            assert_eq!(
                data.cleanup_epoch, 7,
                "a refused transition must not burn an epoch"
            );
            assert_matches!(
                data.dice_status,
                DiceStatus::Available { active: Some(..) },
                "a refused transition must leave the active DICE version in place"
            );
        }
    }

    #[tokio::test]
    async fn nested_invocation_same_transaction() {
        // FIXME: This times out on open source, and we don't know why
        if is_open_source() {
            return;
        }
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice, root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();
        let traces3 = TraceId::new();

        let barrier = Arc::new(Barrier::new(3));

        let fut1 = concurrency.enter(
            TestEvents::with_trace(traces1),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            true,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );
        let fut2 = concurrency.enter(
            TestEvents::with_trace(traces2),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            true,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );
        let fut3 = concurrency.enter(
            TestEvents::with_trace(traces3),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            true,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );

        let (r1, r2, r3) = futures::future::join3(fut1, fut2, fut3).await;
        r1.unwrap();
        r2.unwrap();
        r3.unwrap();
    }

    #[tokio::test]
    async fn nested_invocation_should_error() {
        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice, root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();

        let barrier = Arc::new(Barrier::new(2));

        let fut1 = concurrency.enter(
            TestEvents::with_trace(traces1),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            true,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );

        let fut2 = concurrency.enter(
            TestEvents::with_trace(traces2),
            &CtxDifferent,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            true,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );

        match futures::future::try_join(fut1, fut2).await {
            Err(e) => assert!(e.to_string().contains("Recursive invocation")),
            Ok(_) => {
                panic!("Futures should not have completed successfully")
            }
        }
    }

    #[tokio::test]
    async fn parallel_invocation_same_transaction() {
        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice, root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();
        let traces3 = TraceId::new();

        let barrier = Arc::new(Barrier::new(3));

        let fut1 = concurrency.enter(
            TestEvents::with_trace(traces1),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            false,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );
        let fut2 = concurrency.enter(
            TestEvents::with_trace(traces2),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            false,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );
        let fut3 = concurrency.enter(
            TestEvents::with_trace(traces3),
            &NoChanges,
            |_, _timing| {
                let b = barrier.dupe();
                async move {
                    b.wait().await;
                }
            },
            false,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );

        let (r1, r2, r3) = futures::future::join3(fut1, fut2, fut3).await;
        r1.unwrap();
        r2.unwrap();
        r3.unwrap();
    }

    #[tokio::test]
    async fn parallel_invocation_different_traceid_blocks() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = traces1.dupe();
        let traces_different = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let block2 = Arc::new(RwLock::new(()));
        let blocked2 = block2.write().await;

        let barrier1 = Arc::new(Barrier::new(3));
        let barrier2 = Arc::new(Barrier::new(2));

        let arrived = Arc::new(AtomicBool::new(false));

        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier1.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        let fut2 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier1.dupe();
            let b = block2.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces2),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier1.wait().await;

        let fut3 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier2.dupe();
            let arrived = arrived.dupe();

            async move {
                barrier.wait().await;
                concurrency
                    .enter(
                        TestEvents::with_trace(traces_different),
                        &CtxDifferent,
                        |_, _timing| async move {
                            arrived.store(true, Ordering::Relaxed);
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier2.wait().await;

        assert!(!arrived.load(Ordering::Relaxed));

        drop(blocked1);
        fut1.await??;

        assert!(!arrived.load(Ordering::Relaxed));

        drop(blocked2);
        fut2.await??;

        fut3.await??;

        assert!(arrived.load(Ordering::Relaxed));

        Ok(())
    }

    #[tokio::test]
    async fn parallel_invocation_exit_when_different_state() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = traces1.dupe();
        let traces_different = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let block2 = Arc::new(RwLock::new(()));
        let blocked2 = block2.write().await;

        let barrier1 = Arc::new(Barrier::new(3));
        let barrier2 = Arc::new(Barrier::new(2));

        let arrived = Arc::new(AtomicBool::new(false));

        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier1.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitDifferentState,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        let fut2 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier1.dupe();
            let b = block2.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces2),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitDifferentState,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier1.wait().await;

        let fut3 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier2.dupe();
            let arrived = arrived.dupe();

            async move {
                barrier.wait().await;
                concurrency
                    .enter(
                        TestEvents::with_trace(traces_different),
                        &CtxDifferent,
                        |_, _timing| async move {
                            arrived.store(true, Ordering::Relaxed);
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitDifferentState,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier2.wait().await;

        assert!(!arrived.load(Ordering::Relaxed));

        drop(blocked1);
        fut1.await??;

        assert!(!arrived.load(Ordering::Relaxed));

        drop(blocked2);
        fut2.await??;

        let fut3_result = fut3.await?;

        let fut3_error: buck2_error::Error = fut3_result.unwrap_err();
        assert!(
            fut3_error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonIsBusy),
        );

        Ok(())
    }

    #[tokio::test]
    async fn parallel_invocation_exit_when_preemptible() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = traces1.dupe();
        let traces_different = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let block2 = Arc::new(RwLock::new(()));
        let blocked2 = block2.write().await;

        let barrier1 = Arc::new(Barrier::new(3));
        let barrier2 = Arc::new(Barrier::new(2));

        let arrived = Arc::new(AtomicBool::new(false));

        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier1.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Always,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        let fut2 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier1.dupe();
            let b = block2.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces2),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier1.wait().await;

        let fut3 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier2.dupe();
            let arrived = arrived.dupe();

            async move {
                barrier.wait().await;
                concurrency
                    .enter(
                        TestEvents::with_trace(traces_different),
                        &CtxDifferent,
                        |_, _timing| async move {
                            arrived.store(true, Ordering::Relaxed);
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier2.wait().await;

        assert!(!arrived.load(Ordering::Relaxed));

        drop(blocked1);
        let fut1_result = fut1.await?;
        let fut1_error: buck2_error::Error = fut1_result.unwrap_err();
        assert!(
            fut1_error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonPreempted),
        );

        assert!(!arrived.load(Ordering::Relaxed));

        drop(blocked2);
        fut2.await??;
        fut3.await??;

        Ok(())
    }

    /// `PreemptibleWhen::OnDifferentState` must not preempt when the arriving command shares the
    /// active state. This is the `is_same_state` short circuit in `cancel_preemptible_commands`,
    /// reached only from the `BypassSemaphore::Run` call site.
    #[tokio::test]
    async fn on_different_state_survives_a_same_state_command() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice, root);

        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let entered = Arc::new(AtomicBool::new(false));

        let updater = NoChanges;
        let preemptible = TestCommand::new()
            .preemptible(PreemptibleWhen::OnDifferentState)
            .run(&concurrency, &updater, {
                let entered = entered.dupe();
                let block = block.dupe();
                |_, _timing| async move {
                    entered.store(true, Ordering::Relaxed);
                    let _g = block.read().await;
                }
            });
        pin_mut!(preemptible);

        // Drive it until it is inside `exec` and therefore registered as active.
        while !entered.load(Ordering::Relaxed) {
            assert_matches!(poll!(&mut preemptible), Poll::Pending);
            tokio::task::yield_now().await;
        }

        // A command with the same state runs concurrently and completes.
        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        // Still blocked on the guard, so it cannot have completed normally. Had it been preempted,
        // `enter`'s select would resolve with `DaemonPreempted` regardless of `exec` being stuck —
        // so staying pending is what proves it was not preempted.
        //
        // Asserting this *before* releasing the guard is load-bearing. Releasing first lets the
        // command finish normally in the same window, and `future::select` polls `exec` ahead of
        // the preempt channel, so a real preemption would be discarded and the test would pass
        // whether or not preemption fired.
        assert_matches!(poll!(&mut preemptible), Poll::Pending);

        drop(blocked);
        preemptible.await?;

        Ok(())
    }

    #[tokio::test]
    async fn same_state_effects_follow_registration_and_lock_release() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;
        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let entered = Arc::new(Barrier::new(2));

        let active = tokio::spawn({
            let concurrency = concurrency.dupe();
            let block = block.dupe();
            let entered = entered.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, &NoChanges, |_, _timing| async move {
                        entered.wait().await;
                        let _guard = block.read().await;
                    })
                    .await
            }
        });
        entered.wait().await;

        let events = TestEvents::expect_active_commands_on_equality(concurrency.dupe(), 2);
        TestCommand::new()
            .dispatcher(events)
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        drop(blocked);
        active.await??;

        Ok(())
    }

    #[tokio::test]
    async fn same_state_preemption_preserves_the_arriving_registration() -> buck2_error::Result<()>
    {
        let concurrency = make_handler().await;
        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let entered = Arc::new(Barrier::new(2));

        let preemptible = tokio::spawn({
            let concurrency = concurrency.dupe();
            let block = block.dupe();
            let entered = entered.dupe();
            async move {
                TestCommand::new()
                    .preemptible(PreemptibleWhen::Always)
                    .run(&concurrency, &NoChanges, |_, _timing| async move {
                        entered.wait().await;
                        let _guard = block.read().await;
                    })
                    .await
            }
        });
        entered.wait().await;

        let observer_entered = Arc::new(Barrier::new(2));
        let observer_release = Arc::new(Barrier::new(2));
        let observer = Arc::new(BlockingObserver {
            entered: observer_entered.dupe(),
            release: observer_release.dupe(),
            fail: false,
        });
        let arriving = tokio::spawn({
            let concurrency = concurrency.dupe();
            let observer = observer.dupe();
            async move {
                TestCommand::new()
                    .run_with_observer(
                        &concurrency,
                        &NoChanges,
                        observer.as_ref(),
                        |_, _timing| async move {},
                    )
                    .await
            }
        });

        tokio::time::timeout(Duration::from_secs(10), observer_entered.wait())
            .await
            .expect("the arriving command was never registered");

        let preempted = tokio::time::timeout(Duration::from_secs(10), preemptible)
            .await
            .buck_error_context("the active command was not preempted")??
            .expect_err("the active command should report preemption");
        assert!(
            preempted
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonPreempted),
            "unexpected preemption error: {preempted}"
        );

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let active = concurrency.coordinator.active_command_count();
                if active == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the preempted command was not reaped");

        observer_release.wait().await;
        arriving.await??;
        drop(blocked);

        Ok(())
    }

    /// The other half of the matrix: `OnDifferentState` is preempted when the arriving command has
    /// a different state. Note the blocking guard is deliberately never released — preemption is
    /// what unblocks the first command, by dropping its `exec` future.
    #[tokio::test]
    async fn on_different_state_is_preempted_by_a_different_state_command()
    -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice, root);

        let block = Arc::new(RwLock::new(()));
        let _blocked = block.write().await;
        let entered = Arc::new(Barrier::new(2));

        let preemptible = tokio::spawn({
            let concurrency = concurrency.dupe();
            let entered = entered.dupe();
            let block = block.dupe();

            async move {
                TestCommand::new()
                    .preemptible(PreemptibleWhen::OnDifferentState)
                    .run(&concurrency, &NoChanges, |_, _timing| async move {
                        entered.wait().await;
                        let _g = block.read().await;
                    })
                    .await
            }
        });

        entered.wait().await;

        let different = tokio::spawn({
            let concurrency = concurrency.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, &CtxDifferent, |_, _timing| async move {})
                    .await
            }
        });

        // Bounded: if preemption regresses, the first command stays blocked on a guard that is
        // never released, so without this the test deadlocks rather than failing.
        let result = tokio::time::timeout(Duration::from_secs(10), preemptible)
            .await
            .buck_error_context("Command was never preempted")?;

        let error: buck2_error::Error = result?.unwrap_err();
        assert!(
            error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonPreempted),
            "Command should have been preempted, got: {error}"
        );

        different.await??;

        Ok(())
    }

    #[tokio::test]
    async fn cancelling_an_active_command_wakes_a_different_state_waiter() -> buck2_error::Result<()>
    {
        let concurrency = make_handler().await;
        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let entered = Arc::new(Barrier::new(2));

        let active = tokio::spawn({
            let concurrency = concurrency.dupe();
            let block = block.dupe();
            let entered = entered.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, &NoChanges, |_, _timing| async move {
                        entered.wait().await;
                        let _guard = block.read().await;
                    })
                    .await
            }
        });
        entered.wait().await;

        let events = TestEvents::new();
        let waiter = tokio::spawn({
            let concurrency = concurrency.dupe();
            let events = events.dupe();
            async move {
                TestCommand::new()
                    .dispatcher(events)
                    .run(&concurrency, &CtxDifferent, |_, _timing| async move {})
                    .await
            }
        });

        events
            .wait_for(|event| {
                matches!(
                    event,
                    RecordedEvent::Instant(buck2_data::instant_event::Data::DiceEqualityCheck(
                        DiceEqualityCheck { is_equal: false }
                    ))
                )
            })
            .await?;

        active.abort();
        assert!(
            active
                .await
                .expect_err("the active command should be cancelled")
                .is_cancelled(),
            "the active task ended for a reason other than cancellation"
        );

        let waiter_result = tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .buck_error_context("the different-state waiter was not woken")??;
        waiter_result?;
        drop(blocked);

        Ok(())
    }

    /// A failing `DiceUpdater` fails its command without wedging the handler for the next one.
    #[tokio::test]
    async fn a_failing_updater_leaves_the_handler_usable() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;

        let failed = TestCommand::new()
            .run(&concurrency, &FailingUpdater, |_, _timing| async move {})
            .await;
        assert!(
            failed.is_err(),
            "the command should surface the update failure"
        );

        TestCommand::new()
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        Ok(())
    }

    /// A queued exclusive command must not deadlock a nested invocation with its parent.
    #[tokio::test]
    async fn a_queued_exclusive_command_does_not_block_nested_invocations()
    -> buck2_error::Result<()> {
        // Matches the existing nested-invocation test's OSS exclusion.
        if is_open_source() {
            return Ok(());
        }

        let concurrency = make_handler().await;

        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let parent_running = Arc::new(Barrier::new(2));

        let parent = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = parent_running.dupe();
            let block = block.dupe();
            async move {
                TestCommand::new()
                    .run(&concurrency, &NoChanges, |_, _timing| async move {
                        barrier.wait().await;
                        let _g = block.read().await;
                    })
                    .await
            }
        });

        parent_running.wait().await;

        let exclusive = tokio::spawn({
            let concurrency = concurrency.dupe();
            async move {
                TestCommand::new()
                    .exclusive_cmd("clean")
                    .run(&concurrency, &NoChanges, |_, _timing| async move {})
                    .await
            }
        });

        // Wait until the writer starts turning away new readers.
        tokio::time::timeout(Duration::from_secs(10), async {
            while concurrency
                .exclusive_command_lock
                .owning_command()
                .is_none()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the exclusive command never queued");

        // Bound the child because the regression is a deadlock.
        tokio::time::timeout(
            Duration::from_secs(20),
            TestCommand::new().nested_invocation(true).run(
                &concurrency,
                &NoChanges,
                |_, _timing| async move {},
            ),
        )
        .await
        .expect("a nested invocation was blocked behind the queued exclusive command")?;

        drop(blocked);
        parent.await??;
        exclusive.await??;

        Ok(())
    }

    /// A slow observer runs after registration without blocking the coordinator.
    #[tokio::test]
    async fn a_slow_observer_runs_after_registration_outside_the_coordinator()
    -> buck2_error::Result<()> {
        let concurrency = make_handler().await;

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let observer = Arc::new(BlockingObserver {
            entered: entered.dupe(),
            release: release.dupe(),
            fail: false,
        });
        let exec_ran = Arc::new(AtomicBool::new(false));

        let command = tokio::spawn({
            let concurrency = concurrency.dupe();
            let observer = observer.dupe();
            let exec_ran = exec_ran.dupe();
            async move {
                TestCommand::new()
                    .run_with_observer(
                        &concurrency,
                        &NoChanges,
                        observer.as_ref(),
                        move |_, _timing| async move {
                            exec_ran.store(true, Ordering::SeqCst);
                        },
                    )
                    .await
            }
        });

        tokio::time::timeout(Duration::from_secs(10), entered.wait())
            .await
            .buck_error_context("the observer was never reached")?;

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(
            snapshot.active_commands, 1,
            "the command must register before its observer runs"
        );
        assert!(
            !exec_ran.load(Ordering::SeqCst),
            "command execution started before its observer completed"
        );
        release.wait().await;
        let joined = tokio::time::timeout(Duration::from_secs(10), command)
            .await
            .buck_error_context("the command did not finish")?;
        joined??;
        assert!(exec_ran.load(Ordering::SeqCst));

        Ok(())
    }

    /// Observer failure must wake a command waiting for a different state.
    #[tokio::test]
    async fn a_failing_observer_wakes_a_different_state_waiter() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let observer = Arc::new(BlockingObserver {
            entered: entered.dupe(),
            release: release.dupe(),
            fail: true,
        });
        let failed_exec_ran = Arc::new(AtomicBool::new(false));

        let first = tokio::spawn({
            let concurrency = concurrency.dupe();
            let observer = observer.dupe();
            let failed_exec_ran = failed_exec_ran.dupe();
            async move {
                TestCommand::new()
                    .run_with_observer(
                        &concurrency,
                        &NoChanges,
                        observer.as_ref(),
                        move |_, _timing| async move {
                            failed_exec_ran.store(true, Ordering::SeqCst);
                        },
                    )
                    .await
            }
        });

        tokio::time::timeout(Duration::from_secs(10), entered.wait())
            .await
            .buck_error_context("the observer was never reached")?;

        let snapshot = concurrency.coordinator.snapshot().await?;
        assert_eq!(
            snapshot.active_commands, 1,
            "the command must be registered before its observer runs"
        );

        let waiter_events = TestEvents::new();
        let waiter_ran = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn({
            let concurrency = concurrency.dupe();
            let waiter_events = waiter_events.dupe();
            let waiter_ran = waiter_ran.dupe();
            async move {
                TestCommand::new()
                    .dispatcher(waiter_events)
                    .run(&concurrency, &CtxDifferent, move |_, _timing| async move {
                        waiter_ran.store(true, Ordering::SeqCst);
                    })
                    .await
            }
        });

        waiter_events
            .wait_for(|event| {
                matches!(
                    event,
                    RecordedEvent::SpanStart(
                        buck2_data::span_start_event::Data::DiceBlockConcurrentCommand(..)
                    )
                )
            })
            .await?;
        assert!(
            !waiter_ran.load(Ordering::SeqCst),
            "the different-state command ran while the observer was active"
        );

        release.wait().await;

        let joined = tokio::time::timeout(Duration::from_secs(10), first)
            .await
            .buck_error_context("the failing command did not finish")?;
        let failed = joined?;
        assert!(failed.is_err(), "the observer failure was not returned");
        assert!(
            !failed_exec_ran.load(Ordering::SeqCst),
            "command execution ran after its observer failed"
        );

        let joined = tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .buck_error_context("the different-state waiter was not woken")?;
        joined??;
        assert!(waiter_ran.load(Ordering::SeqCst));

        wait_for_commands_to_be_reaped(&concurrency).await?;
        assert_eq!(concurrency.coordinator.active_command_count(), 0);

        Ok(())
    }

    /// A failing observer must deregister its already-registered command.
    #[tokio::test]
    async fn a_failing_observer_leaves_the_handler_usable() -> buck2_error::Result<()> {
        let concurrency = make_handler().await;

        let failed = TestCommand::new()
            .run_with_observer(
                &concurrency,
                &NoChanges,
                &FailingObserver,
                |_, _timing| async move {},
            )
            .await;
        assert!(
            failed.is_err(),
            "the command should surface the observer failure"
        );

        // The failed command registered before its observer ran, so its lease must release it.
        wait_for_commands_to_be_reaped(&concurrency).await?;
        assert_eq!(
            concurrency.coordinator.active_command_count(),
            0,
            "a command that failed after registering must still be released"
        );

        // Same state: reuses the version the failed command installed, rather than installing a
        // fresh one. `DiceEqualityCheck` is only emitted when an existing `ActiveDice` is compared
        // against, so observing it is what distinguishes reuse from teardown-and-reinstall.
        let reuse = TestEvents::new();
        TestCommand::new()
            .dispatcher(reuse.dupe())
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;
        reuse
            .wait_for(|e| {
                matches!(
                    e,
                    RecordedEvent::Instant(buck2_data::instant_event::Data::DiceEqualityCheck(
                        DiceEqualityCheck { is_equal: true }
                    ))
                )
            })
            .await?;

        // Deregistration is asynchronous, so without this the next command can still see the
        // previous one registered, take the `Block` path instead of cleanup, and pass without
        // exercising the transition this is meant to cover.
        wait_for_commands_to_be_reaped(&concurrency).await?;

        // Different state: must transition through cleanup despite the orphaned version, which
        // ends with a fresh `ActiveDice` installed and therefore `NoActiveDiceState` reported.
        let cleanup = TestEvents::new();
        TestCommand::new()
            .dispatcher(cleanup.dupe())
            .run(&concurrency, &CtxDifferent, |_, _timing| async move {})
            .await?;
        cleanup
            .wait_for(|e| {
                matches!(
                    e,
                    RecordedEvent::Instant(buck2_data::instant_event::Data::NoActiveDiceState(..))
                )
            })
            .await?;

        Ok(())
    }

    /// The state machine's only externally visible signals. Integration tests key off these, so
    /// they are part of the contract: a command that installs a fresh DICE state reports
    /// `NoActiveDiceState`, and one that joins an equivalent state reports `DiceEqualityCheck`.
    #[tokio::test]
    async fn dice_state_transitions_are_reported_as_events() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice, root);

        let first = TestEvents::new();
        TestCommand::new()
            .dispatcher(first.dupe())
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        first
            .wait_for(|e| {
                matches!(
                    e,
                    RecordedEvent::Instant(buck2_data::instant_event::Data::NoActiveDiceState(..))
                )
            })
            .await?;

        // The active DICE version is not cleared when a command exits, so the next command with an
        // equivalent state reuses it rather than installing a new one.
        let second = TestEvents::new();
        TestCommand::new()
            .dispatcher(second.dupe())
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        second
            .wait_for(|e| {
                matches!(
                    e,
                    RecordedEvent::Instant(buck2_data::instant_event::Data::DiceEqualityCheck(
                        DiceEqualityCheck { is_equal: true }
                    ))
                )
            })
            .await?;

        // Deliberately not asserted: whether a *different* state command emits
        // `DiceEqualityCheck { is_equal: false }` before blocking depends on whether the previous
        // command's entry has been reaped yet, and reaping happens on a detached task.

        Ok(())
    }

    /// A command that takes ownership of an unowned DICE state while work from a previous
    /// transaction is still winding down is tainted, and says so.
    #[tokio::test]
    async fn command_taking_over_non_idle_dice_is_tagged_tainted() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;

        let key = CleanupTestKey {
            is_executing: Arc::new(Mutex::new(())),
        };
        let key = &key;

        // Abandon a transaction with a computation still running, leaving DICE a task pending
        // cancellation. `CleanupTestKey` holds a cancellation critical section for a second, so it
        // stays pending for the rest of the test.
        {
            let transaction = dice.updater(root).commit().await;

            let compute = transaction.compute(key).fuse();
            let started = async {
                while !key.is_executing.is_locked() {
                    tokio::task::yield_now().await;
                }
            }
            .fuse();

            futures::pin_mut!(compute);
            futures::pin_mut!(started);

            futures::select! {
                _ = compute => panic!("compute finished before started?"),
                _ = started => {}
            }
        }

        assert!(
            !dice.is_idle().await,
            "DICE should have a task pending cancellation"
        );

        let concurrency = ConcurrencyHandler::new(dice, root);
        let events = TestEvents::new();

        TestCommand::new()
            .dispatcher(events.dupe())
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        events.wait_for(is_tag_event("concurrency-tainted")).await?;

        assert!(
            !events
                .recorded()
                .iter()
                .any(is_tag_event("concurrency-previously-tainted")),
            "the command that causes the taint should not also report inheriting it"
        );

        assert!(
            concurrency.coordinator.snapshot().await?.previously_tainted,
            "Taint should latch for subsequent commands"
        );

        // The latch is only observable through the next command: taint is reported once by the
        // command that caused it, and thereafter as `concurrency-previously-tainted` by every
        // command that inherits the tainted state.
        let later = TestEvents::new();
        TestCommand::new()
            .dispatcher(later.dupe())
            .run(&concurrency, &NoChanges, |_, _timing| async move {})
            .await?;

        later
            .wait_for(is_tag_event("concurrency-previously-tainted"))
            .await?;

        assert!(
            !later
                .recorded()
                .iter()
                .any(is_tag_event("concurrency-tainted")),
            "A command inheriting taint should not report itself as the cause"
        );

        Ok(())
    }

    #[derive(Clone, Dupe, Derivative, Allocative, Display, Pagable)]
    #[derivative(Hash, Eq, PartialEq, Debug)]
    #[display("CleanupTestKey")]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct CleanupTestKey {
        #[derivative(Debug = "ignore", Hash = "ignore", PartialEq = "ignore")]
        #[pagable(discard = "Arc::new(Mutex::new(()))")]
        is_executing: Arc<Mutex<()>>,
    }

    #[derive(Clone, Dupe, Derivative, Allocative, Display, Pagable)]
    #[derivative(Hash, Eq, PartialEq, Debug)]
    #[display("ControlledCleanupTestKey")]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct ControlledCleanupTestKey {
        #[allocative(skip)]
        #[derivative(Debug = "ignore", Hash = "ignore", PartialEq = "ignore")]
        #[pagable(discard = "Arc::new(Barrier::new(2))")]
        entered: Arc<Barrier>,
        #[allocative(skip)]
        #[derivative(Debug = "ignore", Hash = "ignore", PartialEq = "ignore")]
        #[pagable(discard = "Arc::new(Barrier::new(2))")]
        release: Arc<Barrier>,
    }

    #[async_trait::async_trait]
    impl Key for ControlledCleanupTestKey {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            cancellation: &CancellationContext,
        ) -> Self::Value {
            cancellation
                .critical_section(|| async {
                    self.entered.wait().await;
                    self.release.wait().await;
                })
                .await;
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_me, _other| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            PagableValueSerialize::<Self::Value>::new()
        }
    }

    #[async_trait::async_trait]
    impl Key for CleanupTestKey {
        type Value = ();

        #[allow(clippy::await_holding_lock)] // Intentional: testing exclusive access
        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            cancellation: &CancellationContext,
        ) -> Self::Value {
            let _guard = self.is_executing.lock();

            cancellation
                .critical_section(|| tokio::time::sleep(Duration::from_secs(1)))
                .await;
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_me, _other| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            PagableValueSerialize::<Self::Value>::new()
        }
    }

    #[tokio::test]
    async fn test_cleanup_stage() -> buck2_error::Result<()> {
        let key = CleanupTestKey {
            is_executing: Arc::new(Mutex::new(())),
        };

        let key = &key;

        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        // Kick off our computation and wait until it's running.

        concurrency
            .enter(
                TestEvents::new(),
                &NoChanges,
                |dice, _timing| async move {
                    let compute = dice.compute(key).fuse();

                    let started = async {
                        while !key.is_executing.is_locked() {
                            tokio::task::yield_now().await;
                        }
                    }
                    .fuse();

                    // NOTE: We still need to poll `compute` for it to actually spawn, hence the
                    // select below.

                    futures::pin_mut!(compute);
                    futures::pin_mut!(started);

                    futures::select! {
                        _ = compute => panic!("compute finished before started?"),
                        _ = started => {}
                    }
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        // Now, re-enter. We expect to reuse and therefore to not wait.

        concurrency
            .enter(
                TestEvents::new(),
                &NoChanges,
                |_dice, _timing| async move {
                    // The key should still be evaluating by now.
                    assert!(key.is_executing.is_locked());
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        // Now, enter with a different context. This time, we expect to not reuse.

        concurrency
            .enter(
                TestEvents::new(),
                &CtxDifferent,
                |_dice, _timing| async move {
                    assert!(!key.is_executing.is_locked());
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        Ok(())
    }

    /// Waits for the start of an `ExclusiveCommandWait` span reporting `cmd` as the current owner.
    async fn wait_for_exclusive_span_start(
        events: &TestEvents,
        cursor: &mut usize,
        cmd: Option<&str>,
    ) -> buck2_error::Result<()> {
        let cmd = cmd.map(|c| c.to_owned());
        events
            .wait_from(cursor, |e| match e {
                RecordedEvent::SpanStart(
                    buck2_data::span_start_event::Data::ExclusiveCommandWait(
                        ExclusiveCommandWaitStart {
                            command_name: event_cmd,
                        },
                    ),
                ) => event_cmd == &cmd,
                _ => false,
            })
            .await?;
        Ok(())
    }

    /// Waits for the next `ExclusiveCommandWait` span end after `cursor`. The channel-based version
    /// of this test paired ends to starts by span id; the fake has no ids, so ordering after the
    /// cursor stands in for that pairing.
    async fn wait_for_exclusive_span_end(
        events: &TestEvents,
        cursor: &mut usize,
    ) -> buck2_error::Result<()> {
        events
            .wait_from(cursor, |e| {
                matches!(
                    e,
                    RecordedEvent::SpanEnd(buck2_data::span_end_event::Data::ExclusiveCommandWait(
                        _
                    ))
                )
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // Intentional: testing exclusive access
    async fn exclusive_command_lock() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);
        let events = TestEvents::new();
        let mut cursor = 0usize;

        let mutex = Arc::new(Mutex::new(()));
        let command = |exclusive_cmd: Option<&str>, barriers: Option<&Arc<(Barrier, Barrier)>>| {
            tokio::spawn({
                let concurrency = concurrency.dupe();
                let dispatcher = events.dupe();
                let barriers = barriers.map(|b| b.dupe());
                let exclusive_cmd = exclusive_cmd.map(|b| b.to_owned());
                let mutex = mutex.dupe();
                async move {
                    concurrency
                        .enter(
                            dispatcher,
                            &NoChanges,
                            |_, _timing| async move {
                                let _guard = mutex.try_lock().expect("Not exclusive!");
                                if let Some(barriers) = barriers {
                                    barriers.0.wait().await;
                                    barriers.1.wait().await;
                                }
                                tokio::task::yield_now().await;
                            },
                            false,
                            Vec::new(),
                            exclusive_cmd,
                            CancellationContext::testing(),
                            PreemptibleWhen::Never,
                            &NoTelemetry,
                            ExitWhen::ExitNever,
                            EarlyCommandTimingBuilder::new(Instant::now()),
                        )
                        .await
                }
            })
        };

        let non_exclusive_barriers = Arc::new((Barrier::new(2), Barrier::new(2)));
        // Start non_exclusive command and enter critical section
        let non_exclusive_fut = command(None, Some(&non_exclusive_barriers.dupe()));
        non_exclusive_barriers.0.wait().await;

        wait_for_exclusive_span_start(&events, &mut cursor, None).await?;
        wait_for_exclusive_span_end(&events, &mut cursor).await?;

        let command_barriers = Arc::new((Barrier::new(2), Barrier::new(2)));
        // Start exclusive command, blocked by non_exclusive
        let exclusive_fut_1 = command(Some("exclusive_1"), Some(&command_barriers.dupe()));

        wait_for_exclusive_span_start(&events, &mut cursor, None).await?;

        // Finish non_exclusive, enter exclusive_1 critical section
        non_exclusive_barriers.1.wait().await;
        non_exclusive_fut.await??;
        command_barriers.0.wait().await;

        wait_for_exclusive_span_end(&events, &mut cursor).await?;

        // Start series of exclusive commands and another second non_exclusive
        let exclusive_fut_2 = command(Some("exclusive_2"), None);
        wait_for_exclusive_span_start(&events, &mut cursor, Some("exclusive_1")).await?;
        let exclusive_fut_3 = command(Some("exclusive_3"), None);
        wait_for_exclusive_span_start(&events, &mut cursor, Some("exclusive_1")).await?;
        let non_exclusive_fut = command(None, None);
        wait_for_exclusive_span_start(&events, &mut cursor, Some("exclusive_1")).await?;

        // Unblock first exclusive command, remaining commands are unblocked
        command_barriers.1.wait().await;
        exclusive_fut_1.await??;
        exclusive_fut_2.await??;
        exclusive_fut_3.await??;
        non_exclusive_fut.await??;

        wait_for_exclusive_span_end(&events, &mut cursor).await?;
        wait_for_exclusive_span_end(&events, &mut cursor).await?;
        wait_for_exclusive_span_end(&events, &mut cursor).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_thundering_herd() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let concurrency = &concurrency;

        let tasks = (0..3).map(|_i| async {
            concurrency
                .enter(
                    TestEvents::new(),
                    &CtxDifferent,
                    |dice, _timing| async move {
                        // NOTE: We need to actually compute something for DICE to be not-idle.
                        dice.compute(&K).await.unwrap();
                        tokio::task::yield_now().await;
                    },
                    false,
                    Vec::new(),
                    None,
                    CancellationContext::testing(),
                    PreemptibleWhen::Never,
                    &NoTelemetry,
                    ExitWhen::ExitNever,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        });

        buck2_util::future::try_join_all(tasks).await?;

        assert!(!concurrency.coordinator.snapshot().await?.previously_tainted);

        Ok(())
    }

    #[tokio::test]
    async fn test_updates_are_synchronized() -> buck2_error::Result<()> {
        async fn wait_on(b: &AtomicBool) {
            while !b.load(Ordering::Relaxed) {
                tokio::task::yield_now().await;
            }
        }

        let (dice, root) = make_default_dice().await;

        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        struct Updater {
            // Set when the updater enters the update function
            on_enter: AtomicBool,
            // Set to indicate that the updater should exit its update function
            allow_exit: AtomicBool,
        }
        #[async_trait]
        impl DiceUpdater for Updater {
            async fn update(
                &self,
                ctx: DiceTransactionUpdater,
                _early_timings: &mut EarlyCommandTimingBuilder,
            ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
            {
                self.on_enter.store(true, Ordering::Relaxed);
                wait_on(&self.allow_exit).await;
                Ok((ctx, Default::default(), SyncOutcome::Incremental))
            }
        }

        let updater1 = Updater {
            on_enter: AtomicBool::new(false),
            allow_exit: AtomicBool::new(false),
        };
        let fut1 = concurrency.enter(
            TestEvents::new(),
            &updater1,
            |_dice, _timing| async move {
                tokio::task::yield_now().await;
            },
            false,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );
        pin_mut!(fut1);

        let updater2 = Updater {
            on_enter: AtomicBool::new(false),
            // We can set this to true immediately as we don't ever need the
            // second one to wait on anything
            allow_exit: AtomicBool::new(true),
        };
        let fut2 = concurrency.enter(
            TestEvents::new(),
            &updater2,
            |_dice, _timing| async move {
                tokio::task::yield_now().await;
            },
            false,
            Vec::new(),
            None,
            CancellationContext::testing(),
            PreemptibleWhen::Never,
            &NoTelemetry,
            ExitWhen::ExitNever,
            EarlyCommandTimingBuilder::new(Instant::now()),
        );
        pin_mut!(fut2);

        // Wait for the first updater's update to be entered
        tokio::select! {
            _ = &mut fut1 => panic!("First should not be able to exit yet"),
            _ = wait_on(&updater1.on_enter) => (),
        }

        // Now the first updater is blocked within its update function. Poll the
        // second one many times so that it makes as much progress as it can.
        //
        // The `yield_now` is load-bearing. `poll!` drives this future and nothing else, but
        // reaching the update means first awaiting DICE, which only answers when the runtime gets
        // to run its own tasks. Without a yield the second command stalls before it ever reaches
        // the permit, and the assertion below holds for a reason that has nothing to do with
        // synchronization — it passes just as happily with two permits.
        for _ in 0..100 {
            assert_matches!(poll!(&mut fut2), Poll::Pending);
            tokio::task::yield_now().await;
        }
        // But it should not have entered its update yet
        assert!(
            !updater2.on_enter.load(Ordering::Relaxed),
            "Updaters are not correctly synchronized"
        );

        // Now unblock the first one and let both finish
        updater1.allow_exit.store(true, Ordering::Relaxed);
        let (a, b) = tokio::join!(fut1, fut2);
        a.unwrap();
        b.unwrap();

        Ok(())
    }

    async fn check_exit_when_not_idle_while_busy(same_state: bool) -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let barrier = Arc::new(Barrier::new(2));

        // Start first command.
        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier.wait().await;

        // A second command with --exit-when=notidle should fail regardless of state equality.
        let fut2 = tokio::spawn(buck2_util::async_move_clone!(concurrency, {
            concurrency
                .enter(
                    TestEvents::with_trace(traces2),
                    updater_for_state_match(same_state),
                    |_, _timing| async move {
                        // Should never reach here
                        panic!("Command should have failed before execution");
                    },
                    false,
                    Vec::new(),
                    None,
                    CancellationContext::testing(),
                    PreemptibleWhen::Never,
                    &NoTelemetry,
                    ExitWhen::ExitNotIdle,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        }));

        // Second command should fail immediately
        // Bounded: without the `--exit-when=notidle` gate this command blocks forever rather
        // than refusing, so an unbounded await turns a regression into a 10 minute harness
        // timeout instead of a fast failure.
        let fut2_result = tokio::time::timeout(Duration::from_secs(10), fut2)
            .await
            .expect("`--exit-when=notidle` should refuse immediately, not block")?;
        let fut2_error: buck2_error::Error = fut2_result.unwrap_err();
        assert!(
            fut2_error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonIsBusy),
            "Expected DaemonIsBusy error tag"
        );

        // Clean up first command
        drop(blocked1);
        fut1.await??;

        Ok(())
    }

    #[tokio::test]
    async fn test_exit_when_not_idle_while_busy_with_same_or_different_state()
    -> buck2_error::Result<()> {
        check_exit_when_not_idle_while_busy(true).await?;
        check_exit_when_not_idle_while_busy(false).await
    }

    async fn check_multiple_exit_when_not_idle_commands(
        same_state: bool,
    ) -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();
        let traces3 = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let barrier = Arc::new(Barrier::new(2));

        // Start first command with --exit-when=notidle
        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNotIdle,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier.wait().await;

        // Start second and third commands with --exit-when=notidle (should both fail)
        let fut2 = tokio::spawn(buck2_util::async_move_clone!(concurrency, {
            concurrency
                .enter(
                    TestEvents::with_trace(traces2),
                    updater_for_state_match(same_state),
                    |_, _timing| async move {
                        panic!("Should not execute");
                    },
                    false,
                    Vec::new(),
                    None,
                    CancellationContext::testing(),
                    PreemptibleWhen::Never,
                    &NoTelemetry,
                    ExitWhen::ExitNotIdle,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        }));

        let fut3 = tokio::spawn(buck2_util::async_move_clone!(concurrency, {
            concurrency
                .enter(
                    TestEvents::with_trace(traces3),
                    updater_for_state_match(same_state),
                    |_, _timing| async move {
                        panic!("Should not execute");
                    },
                    false,
                    Vec::new(),
                    None,
                    CancellationContext::testing(),
                    PreemptibleWhen::Never,
                    &NoTelemetry,
                    ExitWhen::ExitNotIdle,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        }));

        // Both second and third commands should fail
        // Bounded: without the `--exit-when=notidle` gate this command blocks forever rather
        // than refusing, so an unbounded await turns a regression into a 10 minute harness
        // timeout instead of a fast failure.
        let fut2_result = tokio::time::timeout(Duration::from_secs(10), fut2)
            .await
            .expect("`--exit-when=notidle` should refuse immediately, not block")?;
        let fut2_error: buck2_error::Error = fut2_result.unwrap_err();
        assert!(
            fut2_error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonIsBusy)
        );

        let fut3_result = fut3.await?;
        let fut3_error: buck2_error::Error = fut3_result.unwrap_err();
        assert!(
            fut3_error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonIsBusy)
        );

        // Clean up first command
        drop(blocked1);
        fut1.await??;

        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_exit_when_not_idle_commands_with_same_or_different_state()
    -> buck2_error::Result<()> {
        check_multiple_exit_when_not_idle_commands(true).await?;
        check_multiple_exit_when_not_idle_commands(false).await
    }

    #[tokio::test]
    async fn test_exit_when_not_idle_with_preemptible_command() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let barrier = Arc::new(Barrier::new(2));

        // Start first command with --preemptible=always (could be preempted)
        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Always, // This command is preemptible
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier.wait().await;

        // Start second command with --exit-when=notidle (should fail)
        // Even though the first command is preemptible, this should still fail
        // because --exit-when=notidle means "only run if daemon is completely idle"
        let fut2 = tokio::spawn(buck2_util::async_move_clone!(concurrency, {
            concurrency
                .enter(
                    TestEvents::with_trace(traces2),
                    &NoChanges,
                    |_, _timing| async move {
                        // Should never reach here
                        panic!("Command should have failed before execution");
                    },
                    false,
                    Vec::new(),
                    None,
                    CancellationContext::testing(),
                    PreemptibleWhen::Never,
                    &NoTelemetry,
                    ExitWhen::ExitNotIdle,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        }));

        // Second command should fail immediately, even though first is preemptible
        // Bounded: without the `--exit-when=notidle` gate this command blocks forever rather
        // than refusing, so an unbounded await turns a regression into a 10 minute harness
        // timeout instead of a fast failure.
        let fut2_result = tokio::time::timeout(Duration::from_secs(10), fut2)
            .await
            .expect("`--exit-when=notidle` should refuse immediately, not block")?;
        let fut2_error: buck2_error::Error = fut2_result.unwrap_err();
        assert!(
            fut2_error
                .tags()
                .contains(&buck2_error::ErrorTag::DaemonIsBusy),
            "Expected DaemonIsBusy error tag, even though previous command is preemptible"
        );

        // The first command should still be running (not preempted)
        // because --exit-when=notidle doesn't preempt, it just fails
        assert!(
            block1.try_write().is_err(),
            "First command should still be running"
        );

        // Clean up first command
        drop(blocked1);
        fut1.await??;

        Ok(())
    }

    #[tokio::test]
    async fn test_exit_when_not_idle_gets_preempted() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let barrier = Arc::new(Barrier::new(2));
        let preempted = Arc::new(AtomicBool::new(false));

        // Start first command with --exit-when=notidle
        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier.dupe();
            let b = block1.dupe();
            let preempted = preempted.dupe();

            async move {
                let result = concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            // This should never complete because we'll be preempted
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Always,
                        &NoTelemetry,
                        ExitWhen::ExitNotIdle,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await;

                // Check if we got preempted
                if let Err(ref e) = result {
                    let error: buck2_error::Error = e.clone();
                    if error
                        .tags()
                        .contains(&buck2_error::ErrorTag::DaemonPreempted)
                    {
                        preempted.store(true, Ordering::Relaxed);
                    }
                }
                result
            }
        });

        barrier.wait().await;

        // Start second command (without any preemptible flag)
        // This should preempt the first command
        let fut2 = tokio::spawn(buck2_util::async_move_clone!(concurrency, {
            concurrency
                .enter(
                    TestEvents::with_trace(traces2),
                    &NoChanges,
                    |_, _timing| async move {
                        // Just a quick task
                        tokio::task::yield_now().await;
                    },
                    false,
                    Vec::new(),
                    None,
                    CancellationContext::testing(),
                    PreemptibleWhen::Never, // Not preemptible
                    &NoTelemetry,
                    ExitWhen::ExitNever,
                    EarlyCommandTimingBuilder::new(Instant::now()),
                )
                .await
        }));

        // Second command should succeed
        fut2.await??;

        // First command should have been preempted
        let fut1_result = fut1.await?;
        assert!(fut1_result.is_err(), "First command should have failed");
        assert!(
            preempted.load(Ordering::Relaxed),
            "First command should have been preempted"
        );

        // Clean up
        drop(blocked1);

        Ok(())
    }

    async fn check_exit_when_not_idle_allows_command_when_daemon_idle(
        same_state: bool,
    ) -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();

        // First command runs to completion
        concurrency
            .enter(
                TestEvents::with_trace(traces1),
                &NoChanges,
                |_, _timing| async move {
                    // Quick task that finishes
                    tokio::task::yield_now().await;
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        wait_for_commands_to_be_reaped(&concurrency).await?;

        // The daemon is idle, so state equality must not affect admission.
        let result = concurrency
            .enter(
                TestEvents::with_trace(traces2),
                updater_for_state_match(same_state),
                |_, _timing| async move {
                    // Quick task
                    tokio::task::yield_now().await;
                    "success"
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNotIdle,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await;

        // Should succeed since daemon is idle
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");

        Ok(())
    }

    #[tokio::test]
    async fn test_exit_when_not_idle_allows_command_when_daemon_idle_with_any_state()
    -> buck2_error::Result<()> {
        check_exit_when_not_idle_allows_command_when_daemon_idle(true).await?;
        check_exit_when_not_idle_allows_command_when_daemon_idle(false).await
    }

    fn get_early_command_timing_duration(
        timing: EarlyCommandTimingBuilder,
        key: &str,
    ) -> Option<Duration> {
        let timing = timing.finish_early_command_timing();
        let mut end = timing.early_command_end;
        let mut duration = None;
        for s in timing.early_spans.iter().rev() {
            if s.1 == key {
                let d = end - s.0;
                if let Some(s) = &mut duration {
                    *s += d;
                } else {
                    duration = Some(d)
                }
            }
            end = s.0;
        }
        duration
    }

    fn get_exclusive_command_wait_duration(timing: EarlyCommandTimingBuilder) -> Option<Duration> {
        get_early_command_timing_duration(timing, EXCLUSIVE_COMMAND_WAIT)
    }

    #[tokio::test]
    async fn test_enter_duration_parameter_populated() -> buck2_error::Result<()> {
        // Test that the duration parameter passed to the enter() callback is properly populated
        // when waiting for an exclusive command lock.
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces1 = TraceId::new();
        let traces2 = TraceId::new();

        let block1 = Arc::new(RwLock::new(()));
        let blocked1 = block1.write().await;

        let barrier = Arc::new(Barrier::new(2));
        let duration_captured: Arc<Mutex<Duration>> = Arc::new(Mutex::new(Duration::ZERO));

        // Start first exclusive command
        let fut1 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let barrier = barrier.dupe();
            let b = block1.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces1),
                        &NoChanges,
                        |_, _timing| async move {
                            barrier.wait().await;
                            let _g = b.read().await;
                        },
                        false,
                        Vec::new(),
                        Some("exclusive_test".to_owned()),
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        barrier.wait().await;

        // Start second exclusive command - it should wait and capture non-zero duration
        let fut2 = tokio::spawn({
            let concurrency = concurrency.dupe();
            let duration_captured = duration_captured.dupe();

            async move {
                concurrency
                    .enter(
                        TestEvents::with_trace(traces2),
                        &NoChanges,
                        |_, timing| {
                            *duration_captured.lock() =
                                get_exclusive_command_wait_duration(timing).unwrap();
                            async move {
                                tokio::task::yield_now().await;
                            }
                        },
                        false,
                        Vec::new(),
                        Some("exclusive_test_2".to_owned()),
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });

        // Give fut2 time to start waiting
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Unblock the first command
        drop(blocked1);
        fut1.await??;

        // Complete the second command
        fut2.await??;

        // Verify that the duration was captured and is non-zero
        let duration = *duration_captured.lock();
        assert!(
            !duration.is_zero(),
            "Duration should be non-zero since we waited for exclusive lock. Got: {:?}",
            duration
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_enter_duration_parameter_zero_for_non_exclusive() -> buck2_error::Result<()> {
        // Test that the duration parameter is zero when no exclusive command lock is needed.
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let traces = TraceId::new();
        let duration_captured: Arc<Mutex<Duration>> = Arc::new(Mutex::new(Duration::ZERO));

        // Run a non-exclusive command (None for exclusive_cmd parameter)
        concurrency
            .enter(
                TestEvents::with_trace(traces),
                &NoChanges,
                |_, timing| {
                    *duration_captured.lock() =
                        get_exclusive_command_wait_duration(timing).unwrap_or(Duration::ZERO);
                    async move {
                        tokio::task::yield_now().await;
                    }
                },
                false,
                Vec::new(),
                None, // No exclusive command
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        // Verify that the duration was captured and is zero
        let duration = *duration_captured.lock();
        assert!(
            duration.is_zero(),
            "Duration should be zero for non-exclusive commands. Got: {:?}",
            duration
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_file_watcher_sync_duration_captured() -> buck2_error::Result<()> {
        // Test that file_watcher_sync_duration is properly captured when the updater
        // returns a non-zero duration.
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        struct UpdaterWithDelay;
        #[async_trait]
        impl DiceUpdater for UpdaterWithDelay {
            async fn update(
                &self,
                ctx: DiceTransactionUpdater,
                early_timings: &mut EarlyCommandTimingBuilder,
            ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
            {
                // Simulate file watcher sync taking 50ms
                early_timings.start_span(FILE_WATCHER_WAIT.to_owned());
                tokio::time::sleep(Duration::from_millis(50)).await;
                early_timings.end_known_span();
                Ok((ctx, Default::default(), SyncOutcome::Incremental))
            }
        }

        let traces = TraceId::new();
        let file_watcher_duration_captured: Arc<Mutex<Duration>> =
            Arc::new(Mutex::new(Duration::ZERO));

        concurrency
            .enter(
                TestEvents::with_trace(traces),
                &UpdaterWithDelay,
                |_, timing| {
                    let duration_captured = file_watcher_duration_captured.dupe();
                    async move {
                        // Capture the file watcher sync duration (sum of all syncs)
                        let total_duration: std::time::Duration =
                            get_early_command_timing_duration(timing, FILE_WATCHER_WAIT).unwrap();
                        *duration_captured.lock() = total_duration;
                        tokio::task::yield_now().await;
                    }
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        // Verify that the file watcher sync duration was captured
        let duration = *file_watcher_duration_captured.lock();
        assert!(
            !duration.is_zero(),
            "File watcher sync duration should be non-zero. Got: {:?}",
            duration
        );
        assert!(
            duration >= Duration::from_millis(50),
            "File watcher sync duration should be at least 50ms. Got: {:?}",
            duration
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_file_watcher_sync_duration_accumulated_across_loop_iterations()
    -> buck2_error::Result<()> {
        // Test that file_watcher_sync_duration is accumulated across multiple loop iterations
        // when the dice state transitions through cleanup.
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        // First, establish an active DICE state by running a command
        let traces_init = TraceId::new();
        concurrency
            .enter(
                TestEvents::with_trace(traces_init),
                &NoChanges,
                |_, _timing| async move {
                    // Just establish the initial state
                    tokio::task::yield_now().await;
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        // Now run the test that changes state, which should trigger a cleanup and re-update
        struct UpdaterWithDelayAndStateChange {
            call_count: AtomicBool,
        }

        #[async_trait]
        impl DiceUpdater for UpdaterWithDelayAndStateChange {
            async fn update(
                &self,
                mut ctx: DiceTransactionUpdater,
                early_timings: &mut EarlyCommandTimingBuilder,
            ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
            {
                // First call changes state, second call doesn't
                let is_first = !self.call_count.swap(true, Ordering::Relaxed);
                if is_first {
                    ctx.changed_to(vec![(K, ())])?;
                }
                // Each call simulates 30ms of file watcher sync
                early_timings.start_span(FILE_WATCHER_WAIT.to_owned());
                tokio::time::sleep(Duration::from_millis(30)).await;
                early_timings.end_known_span();
                Ok((ctx, Default::default(), SyncOutcome::Incremental))
            }
        }

        let traces = TraceId::new();
        let file_watcher_duration_captured: Arc<Mutex<Duration>> =
            Arc::new(Mutex::new(Duration::ZERO));

        let updater = UpdaterWithDelayAndStateChange {
            call_count: AtomicBool::new(false),
        };

        concurrency
            .enter(
                TestEvents::with_trace(traces),
                &updater,
                |_, timing| {
                    *file_watcher_duration_captured.lock() =
                        get_early_command_timing_duration(timing, FILE_WATCHER_WAIT).unwrap();
                    async move {
                        tokio::task::yield_now().await;
                    }
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?;

        // Verify that the file watcher sync duration was accumulated
        // It should be at least the sum of both iterations (60ms total)
        let duration = *file_watcher_duration_captured.lock();
        assert!(
            duration >= Duration::from_millis(60),
            "File watcher sync duration should be accumulated across loop iterations. Expected at least 60ms, got: {:?}",
            duration
        );

        Ok(())
    }

    /// An updater that injects `K`, and reports a fresh instance on the first call after `armed`
    /// is set.
    struct FreshInstanceWhenArmed {
        armed: AtomicBool,
    }

    #[async_trait]
    impl DiceUpdater for FreshInstanceWhenArmed {
        async fn update(
            &self,
            mut ctx: DiceTransactionUpdater,
            _early_timings: &mut EarlyCommandTimingBuilder,
        ) -> buck2_error::Result<(DiceTransactionUpdater, UserComputationData, SyncOutcome)>
        {
            ctx.changed_to(vec![(K, ())])?;
            let outcome = if self.armed.swap(false, Ordering::SeqCst) {
                SyncOutcome::FreshInstance
            } else {
                SyncOutcome::Incremental
            };
            Ok((ctx, Default::default(), outcome))
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn owned_root_resets_preserve_tenant_data_and_retire_the_current_branch()
    -> buck2_error::Result<()> {
        let dice = Dice::builder().build(DetectCycles::Enabled);
        let mut data = dice::DiceData::new();
        data.set(7usize);
        let root = dice.new_root_with_data(data).await;
        let concurrency = ConcurrencyHandler::new_owned(dice.dupe(), root);
        let updater = FreshInstanceWhenArmed {
            armed: AtomicBool::new(true),
        };
        let (fresh, _) = run_counting(&concurrency, &updater).await?;
        assert_ne!(fresh.branch(), root);
        let transaction = dice.updater(fresh.branch()).commit().await;
        assert_eq!(7, *transaction.global_data().get::<usize>().unwrap());
        drop(transaction);
        concurrency.retire();
        tokio::time::timeout(Duration::from_secs(5), async {
            while dice.metrics().branch_count != 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("both the discarded and current roots should be released");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_a_root_replacement_releases_both_roots_on_retirement()
    -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new_owned(dice.dupe(), root);
        let command = concurrency.allocate_command_id();
        let PreUpdateDecision::Update { attempt, .. } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id: command,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 build".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("an idle coordinator should prepare the update");
        };
        concurrency
            .coordinator
            .acquire_update(command, attempt)
            .await?;
        let fresh = dice.new_root_from(root).await;
        let (response, receiver) = oneshot::channel();
        drop(receiver);
        assert!(
            concurrency
                .coordinator
                .sender
                .send(CoordinatorMessage::MoveToBranch {
                    command,
                    attempt,
                    branch: fresh,
                    retire_previous: true,
                    response,
                })
                .is_ok(),
            "the coordinator should still accept the cancelled replacement"
        );
        concurrency.retire();
        tokio::time::timeout(Duration::from_secs(5), async {
            while dice.metrics().branch_count != 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("retirement should release the old root and the unobserved replacement");
        Ok(())
    }

    fn retirement_request(command_id: CommandId) -> AdmissionRequest {
        AdmissionRequest {
            command_id,
            command: CommandData {
                trace_id: TraceId::new(),
                display_command: "buck2 build".to_owned(),
                preemption_setting: PreemptibleWhen::Never,
                preempt: None,
            },
            is_nested: false,
            exit_when: ExitWhen::ExitNever,
        }
    }

    #[tokio::test]
    async fn retiring_coordinators_reject_admission_before_existing_commands_release()
    -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new_owned(dice, root);
        let existing = concurrency.allocate_command_id();
        concurrency
            .coordinator
            .begin(retirement_request(existing))
            .await?;
        concurrency.retire();
        let rejected = concurrency
            .coordinator
            .begin(retirement_request(concurrency.allocate_command_id()))
            .await;
        assert!(
            rejected.is_err(),
            "retirement must reject admission while existing work drains"
        );
        concurrency.coordinator.release_and_wait(existing).await?;
        Ok(())
    }

    #[tokio::test]
    async fn discarded_root_cleanup_retains_disk_ownership_after_coordinator_retirement()
    -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let held = dice.updater(root).commit().await;
        let gate = Arc::new(Semaphore::new(1));
        let ownership = Arc::new(gate.clone().acquire_owned().await?);
        let concurrency = ConcurrencyHandler::new_owned_with_lifetime_guard(
            dice.dupe(),
            root,
            Some(ownership.clone()),
        );
        drop(ownership);
        let command = concurrency.allocate_command_id();
        let PreUpdateDecision::Update { attempt, .. } = concurrency
            .coordinator
            .begin(retirement_request(command))
            .await?
        else {
            panic!("an idle coordinator should grant an update");
        };
        concurrency
            .coordinator
            .acquire_update(command, attempt)
            .await?;
        let fresh = dice.new_root_from(root).await;
        concurrency
            .coordinator
            .replace_root(command, attempt, fresh)
            .await?;
        concurrency.coordinator.release_and_wait(command).await?;
        let sender = concurrency.coordinator.sender.clone();
        concurrency.retire();
        drop(concurrency);
        tokio::time::timeout(Duration::from_secs(5), sender.closed())
            .await
            .expect("the current root should retire while the discarded root is still held");
        assert!(
            gate.clone().try_acquire_owned().is_err(),
            "discarded-root work must retain ownership after the current coordinator stops"
        );
        drop(held);
        let _replacement = tokio::time::timeout(Duration::from_secs(5), gate.acquire())
            .await
            .expect("ownership should release after discarded-root work terminates")?;
        Ok(())
    }

    /// A computed value with nothing behind it, whose value tells one computation from another.
    #[derive(Clone, Dupe, Display, Debug, Hash, Eq, PartialEq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct Counted;

    static COUNTED_COMPUTES: AtomicUsize = AtomicUsize::new(0);

    #[async_trait]
    impl Key for Counted {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            COUNTED_COMPUTES.fetch_add(1, Ordering::SeqCst) + 1
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            PagableValueSerialize::<Self::Value>::new()
        }
    }

    /// Runs a command that requests `K`, which its update injected, and `Counted`, returning the
    /// latter with the transaction's version.
    async fn run_counting(
        concurrency: &Arc<ConcurrencyHandler>,
        updater: &dyn DiceUpdater,
    ) -> buck2_error::Result<(Version, usize)> {
        concurrency
            .enter(
                TestEvents::new(),
                updater,
                |transaction, _timing| async move {
                    transaction.compute(&K).await?;
                    let counted = *transaction.compute(&Counted).await?;
                    buck2_error::Ok((transaction.version(), counted))
                },
                false,
                Vec::new(),
                None,
                CancellationContext::testing(),
                PreemptibleWhen::Never,
                &NoTelemetry,
                ExitWhen::ExitNever,
                EarlyCommandTimingBuilder::new(Instant::now()),
            )
            .await?
    }

    /// Only the holder of the update lane may move the handler to another branch.
    #[tokio::test]
    async fn a_move_without_the_update_lane_is_refused() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice, root);
        let command = concurrency.allocate_command_id();
        let PreUpdateDecision::Update { attempt, .. } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id: command,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 build".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("an idle coordinator should prepare the update");
        };

        let fresh = concurrency.dice.new_root().await;
        assert!(
            concurrency
                .coordinator
                .move_to_branch(command, attempt, fresh)
                .await
                .is_err()
        );
        assert_eq!(
            concurrency
                .coordinator
                .acquire_update(command, attempt)
                .await?,
            root
        );
        concurrency
            .coordinator
            .move_to_branch(command, attempt, fresh)
            .await?;
        concurrency.coordinator.release_and_wait(command).await?;

        let next = concurrency.allocate_command_id();
        let PreUpdateDecision::Update { attempt, .. } = concurrency
            .coordinator
            .begin(AdmissionRequest {
                command_id: next,
                command: CommandData {
                    trace_id: TraceId::new(),
                    display_command: "buck2 build".to_owned(),
                    preemption_setting: PreemptibleWhen::Never,
                    preempt: None,
                },
                is_nested: false,
                exit_when: ExitWhen::ExitNever,
            })
            .await?
        else {
            panic!("an idle coordinator should prepare the update");
        };
        assert_eq!(
            concurrency
                .coordinator
                .acquire_update(next, attempt)
                .await?,
            fresh
        );
        concurrency.coordinator.release_and_wait(next).await?;
        Ok(())
    }

    /// A fresh instance moves the handler to a fresh root, where nothing computed before is
    /// reused. The changes recorded before the watcher spoke go with it, and the commands after
    /// it stay there.
    #[tokio::test]
    async fn a_fresh_instance_starts_over_on_a_fresh_root() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);
        let updater = FreshInstanceWhenArmed {
            armed: AtomicBool::new(false),
        };

        let (before, first) = run_counting(&concurrency, &updater).await?;
        assert_eq!(before.branch(), root);

        updater.armed.store(true, Ordering::SeqCst);
        let (fresh, second) = run_counting(&concurrency, &updater).await?;
        assert_ne!(fresh.branch(), root);
        assert_ne!(second, first);

        let (after, third) = run_counting(&concurrency, &updater).await?;
        assert_eq!(after.branch(), fresh.branch());
        assert_eq!(third, second);
        Ok(())
    }

    /// The branch a fresh instance leaves behind is deleted only once its commands are done. Until
    /// then a command still running on it keeps its state, injected keys included; afterwards
    /// nothing the branch held remains.
    #[tokio::test]
    async fn the_branch_a_fresh_instance_leaves_behind_outlives_its_commands()
    -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);

        let block = Arc::new(RwLock::new(()));
        let blocked = block.write().await;
        let entered = Arc::new(Barrier::new(2));

        // Injects `K` and computes `Counted`, then holds its transaction until released and only
        // then requests `K`, which it has not seen yet.
        let running = tokio::spawn({
            let concurrency = concurrency.dupe();
            let entered = entered.dupe();
            let block = block.dupe();
            async move {
                concurrency
                    .enter(
                        TestEvents::new(),
                        &CtxDifferent,
                        |transaction, _timing| async move {
                            transaction.compute(&Counted).await?;
                            entered.wait().await;
                            let _g = block.read().await;
                            transaction.compute(&K).await?;
                            buck2_error::Ok(transaction.version())
                        },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });
        entered.wait().await;
        assert_eq!(dice.pagable_node_counts().await.resident, 1);

        // A fresh instance while the first command runs: the handler moves to a fresh root, and
        // this command blocks behind the first, whose state differs.
        let fresh_events = TestEvents::new();
        let queued = tokio::spawn({
            let concurrency = concurrency.dupe();
            let events = fresh_events.dupe();
            async move {
                concurrency
                    .enter(
                        events,
                        &FreshInstanceWhenArmed {
                            armed: AtomicBool::new(true),
                        },
                        |transaction, _timing| async move { transaction.version() },
                        false,
                        Vec::new(),
                        None,
                        CancellationContext::testing(),
                        PreemptibleWhen::Never,
                        &NoTelemetry,
                        ExitWhen::ExitNever,
                        EarlyCommandTimingBuilder::new(Instant::now()),
                    )
                    .await
            }
        });
        fresh_events
            .wait_for(|event| {
                matches!(
                    event,
                    RecordedEvent::Instant(buck2_data::instant_event::Data::DiceEqualityCheck(
                        DiceEqualityCheck { is_equal: false }
                    ))
                )
            })
            .await?;
        // The branch left behind is still whole.
        assert_eq!(dice.pagable_node_counts().await.resident, 1);

        drop(blocked);
        let old = tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .expect("the running command should finish")
            .expect("the running command should not panic")??;
        let fresh = queued.await??;
        assert_ne!(fresh.branch(), old.branch());

        // With its last command gone, the branch is deleted and what it held released.
        tokio::time::timeout(Duration::from_secs(10), async {
            while dice.pagable_node_counts().await.resident != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the branch left behind should have been deleted");
        Ok(())
    }

    /// A dump taken after a fresh instance, as `buck2 rage` takes one, shows the fresh root and
    /// nothing of the branch left behind.
    #[tokio::test]
    async fn a_dump_after_a_fresh_instance_covers_the_fresh_root() -> buck2_error::Result<()> {
        let (dice, root) = make_default_dice().await;
        let concurrency = ConcurrencyHandler::new(dice.dupe(), root);
        let branch_count = |dice: Arc<Dice>| async move {
            tokio::task::spawn_blocking(move || dice.metrics().branch_count)
                .await
                .unwrap()
        };
        let before = branch_count(dice.dupe()).await;
        let (fresh, _) = run_counting(
            &concurrency,
            &FreshInstanceWhenArmed {
                armed: AtomicBool::new(true),
            },
        )
        .await?;
        assert_ne!(fresh.branch(), root);

        // The branch left behind goes once its work has drained, which the command on the
        // fresh root does not wait for.
        tokio::time::timeout(Duration::from_secs(10), async {
            while branch_count(dice.dupe()).await != before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the branch left behind should have been deleted");

        let dump = tokio::task::spawn_blocking({
            let dice = dice.dupe();
            move || dice.to_introspectable()
        })
        .await
        .unwrap();
        let branches: Vec<u32> = dump.nodes().map(|node| node.branch).collect();
        assert!(!branches.is_empty());
        assert!(
            branches
                .iter()
                .all(|branch| *branch == fresh.branch().as_u32())
        );
        Ok(())
    }
}
