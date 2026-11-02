/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use buck2_common::cancellation::CancellationScope;
use buck2_common::liveliness_observer::LivelinessObserverSync;
use buck2_core::buck2_env;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::soft_error_or_panic;
use buck2_data::clean_stale_result::Origin;
use buck2_data::clean_stale_result::Trigger;
use buck2_data::error::ErrorTag;
use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_events::dispatch::maybe_proxy_current_span;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_events::dispatch::with_dispatcher_opt;
use buck2_events::span::SpanId;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::materialize::materializer::ArtifactNotMaterializedReason;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::MaterializationError;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_hash::BuckMutSet;
use buck2_util::threads::check_stack_overflow;
use dice_futures::cancellation::CancellationContext;
use dice_futures::spawn::WeakFutureError;
use dupe::Dupe;
use dupe::OptionDupedExt;
use futures::Future;
use futures::future;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::future::TryFutureExt;
use futures::stream;
use futures::stream::BoxStream;
use futures::stream::Stream;
use futures::stream::StreamExt;
use jiff::SignedDuration;
use jiff::Timestamp;
use pin_project::pin_project;
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::sync::oneshot::error::TryRecvError;
use tokio::time::Instant;
use tokio::time::Interval;
use tracing::instrument;

use crate::materializers::deferred::AccessTimesUpdates;
use crate::materializers::deferred::BackgroundCleanupGate;
use crate::materializers::deferred::DeferredMaterializerStats;
use crate::materializers::deferred::MaterializeEntryError;
use crate::materializers::deferred::MaterializerReceiver;
use crate::materializers::deferred::MaterializerSender;
use crate::materializers::deferred::SharedMaterializingError;
use crate::materializers::deferred::TtlRefreshConfiguration;
use crate::materializers::deferred::TtlRefreshHistoryEntry;
use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::ArtifactMaterializationData;
use crate::materializers::deferred::artifact_tree::ArtifactMaterializationMethod;
use crate::materializers::deferred::artifact_tree::ArtifactMaterializationStage;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;
use crate::materializers::deferred::artifact_tree::ArtifactRematerializationMethod;
use crate::materializers::deferred::artifact_tree::ArtifactTree;
use crate::materializers::deferred::artifact_tree::CleaningFuture;
use crate::materializers::deferred::artifact_tree::MaterializingFuture;
use crate::materializers::deferred::artifact_tree::Processing;
use crate::materializers::deferred::artifact_tree::ProcessingFuture;
use crate::materializers::deferred::artifact_tree::Version;
use crate::materializers::deferred::artifact_tree::artifact_metadata_matches_entry;
use crate::materializers::deferred::artifact_tree::artifact_metadata_size;
use crate::materializers::deferred::clean_stale::CleanFailure;
use crate::materializers::deferred::clean_stale::CleanResult;
use crate::materializers::deferred::clean_stale::CleanStaleArtifactsCommand;
use crate::materializers::deferred::clean_stale::CleanStaleConfig;
use crate::materializers::deferred::clean_stale::FinishUnmaterializationUpload;
use crate::materializers::deferred::clean_stale::disk_space_probe_path;
use crate::materializers::deferred::extension::ExtensionCommand;
use crate::materializers::deferred::io_handler::IoHandler;
use crate::materializers::deferred::join_all_existing_futs;
use crate::materializers::deferred::materialize_stack::MaterializeStack;
use crate::sqlite::materializer_db::MaterializerStateSqliteDb;

pub(super) struct DeferredMaterializerCommandProcessor<T: 'static> {
    pub(super) io: Arc<T>,
    pub(super) sqlite_db: Option<MaterializerStateSqliteDb>,
    /// The runtime the deferred materializer will spawn futures on. This is normally the runtime
    /// used by the rest of Buck.
    rt: Handle,
    pub(super) cancellation: Arc<CancellationScope>,
    pub(super) defer_write_actions: bool,
    /// Keep track of artifact versions to avoid callbacks clobbering state if the state has moved
    /// forward.
    version_tracker: VersionTracker,
    /// Send messages back to the materializer.
    pub(super) command_sender: Arc<MaterializerSender<T>>,
    /// The actual materializer state.
    pub(super) tree: ArtifactTree,
    /// History of refreshes. This *does* grow without bound, but considering the data is pretty
    /// small and we create it infrequently, that's fine.
    pub(super) ttl_refresh_history: Vec<TtlRefreshHistoryEntry>,
    /// The current ttl_refresh instance, if any exists.
    ttl_refresh_instance: Option<oneshot::Receiver<(Timestamp, buck2_error::Result<()>)>>,
    pub(super) cancellations: &'static CancellationContext,
    pub(super) stats: Arc<DeferredMaterializerStats>,
    access_times_buffer: Option<BuckMutSet<ProjectRelativePathBuf>>,
    verbose_materializer_log: bool,
    daemon_dispatcher: EventDispatcher,
    /// Buck-out path used for clean-stale `disk_space_stats` lookups.
    pub(super) buck_out_abs_path: Option<Arc<AbsPathBuf>>,
    pub(super) clean_stale_config: CleanStaleConfig,
    /// Minimum remaining CAS TTL required before local contents may be discarded.
    /// `None` when periodic TTL refresh cannot preserve a remotely-backed artifact.
    pub(super) rematerialization_ttl: Option<SignedDuration>,
    background_cleanup_gate: Arc<BackgroundCleanupGate>,
}

/// Message taken by the `DeferredMaterializer`'s command loop.
pub(super) enum MaterializerCommand<T: 'static> {
    // [Materializer trait methods -> Command thread]
    /// Takes a list of file paths, computes the materialized file paths of all
    /// of them, and sends the result through the oneshot.
    /// See `Materializer::get_materialized_file_paths` for more information.
    GetMaterializedFilePaths(
        Vec<ProjectRelativePathBuf>,
        oneshot::Sender<Vec<Result<ProjectRelativePathBuf, ArtifactNotMaterializedReason>>>,
    ),

    /// Declares that a set of artifacts already exist
    DeclareExisting(
        Vec<DeclareArtifactPayload>,
        Option<SpanId>,
        Option<EventDispatcher>,
    ),

    /// Declares an artifact: its path, value, and how to materialize it.
    Declare(
        DeclareArtifactPayload,
        Box<ArtifactMaterializationMethod>, // Boxed to avoid growing all variants
        EventDispatcher,
        Option<SpanId>,
    ),

    MatchArtifacts(
        Vec<(ProjectRelativePathBuf, ArtifactValue)>,
        oneshot::Sender<bool>,
    ),

    HasArtifact(
        ProjectRelativePathBuf,
        oneshot::Sender<bool>,
        Option<EventDispatcher>,
    ),

    /// Declares that given paths are no longer eligible to be materialized by this materializer.
    /// This typically should reflect a change made to the underlying filesystem, either because
    /// the file was created, or because it was removed..
    InvalidateFilePaths(
        Vec<ProjectRelativePathBuf>,
        oneshot::Sender<CleaningFuture>,
        EventDispatcher,
        Option<SpanId>,
    ),

    /// Takes a list of artifact paths, and materializes all artifacts in the
    /// list that have been declared but not yet been materialized. When the
    /// materialization starts, a future is sent back through the provided
    /// Sender; this future will be resolved when the materialization
    /// concludes (whether successfully or not).
    Ensure(
        Vec<ProjectRelativePathBuf>,
        MaterializationPurpose,
        EventDispatcher,
        Option<SpanId>,
        oneshot::Sender<BoxStream<'static, Result<(), MaterializationError>>>,
    ),

    Extension(Box<dyn ExtensionCommand<T>>, Option<EventDispatcher>),

    /// Terminate command processor loop, used by tests
    #[allow(dead_code)]
    Abort,

    GetArtifactEntriesForMaterializedPaths {
        paths: Vec<ProjectRelativePathBuf>,
        fetch_root_artifact_entries_for_subpaths: bool,
        sender: oneshot::Sender<
            Vec<
                Option<(
                    ProjectRelativePathBuf,
                    ActionDirectoryEntry<ActionSharedDirectory>,
                )>,
            >,
        >,
    },
}

impl<T> MaterializerCommand<T> {
    fn dispatcher(&self) -> Option<EventDispatcher> {
        match self {
            Self::DeclareExisting(_, _, dispatcher)
            | Self::HasArtifact(_, _, dispatcher)
            | Self::Extension(_, dispatcher) => dispatcher.clone(),
            Self::Declare(_, _, dispatcher, _)
            | Self::InvalidateFilePaths(_, _, dispatcher, _)
            | Self::Ensure(_, _, dispatcher, _, _) => Some(dispatcher.dupe()),
            Self::GetMaterializedFilePaths(..)
            | Self::MatchArtifacts(..)
            | Self::Abort
            | Self::GetArtifactEntriesForMaterializedPaths { .. } => None,
        }
    }
}

impl<T> std::fmt::Debug for MaterializerCommand<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaterializerCommand::GetMaterializedFilePaths(paths, _) => {
                write!(f, "GetMaterializedFilePaths({paths:?}, _)",)
            }
            MaterializerCommand::DeclareExisting(paths, current_span, dispatcher) => {
                write!(
                    f,
                    "DeclareExisting({paths:?}, {current_span:?}, {:?})",
                    dispatcher.as_ref().map(EventDispatcher::trace_id)
                )
            }
            MaterializerCommand::Declare(
                DeclareArtifactPayload { path, artifact },
                method,
                _dispatcher,
                _parent_id,
            ) => {
                write!(f, "Declare({path:?}, {artifact:?}, {method:?})",)
            }
            MaterializerCommand::MatchArtifacts(paths, _) => {
                write!(f, "MatchArtifacts({paths:?})")
            }
            MaterializerCommand::HasArtifact(path, ..) => {
                write!(f, "HasArtifact({path:?})")
            }
            MaterializerCommand::InvalidateFilePaths(paths, ..) => {
                write!(f, "InvalidateFilePaths({paths:?})")
            }
            MaterializerCommand::Ensure(paths, purpose, _, _, _) => {
                write!(f, "Ensure({paths:?}, {purpose:?}, _)",)
            }
            MaterializerCommand::Extension(ext, _) => write!(f, "Extension({ext:?})"),
            MaterializerCommand::Abort => write!(f, "Abort"),
            MaterializerCommand::GetArtifactEntriesForMaterializedPaths {
                paths,
                fetch_root_artifact_entries_for_subpaths,
                sender: _,
            } => {
                write!(
                    f,
                    "GetArtifactEntriesForMaterializedPaths({paths:?}, {fetch_root_artifact_entries_for_subpaths}, _)",
                )
            }
        }
    }
}

/// Materializer commands that can be reordered with regard to other commands.
#[derive(Debug)]
pub(super) enum LowPriorityMaterializerCommand {
    /// [Materialization task -> Command thread]
    /// Notifies the command thread that an artifact was materialized. It takes
    /// the artifact path and the version that was materialized, such that if
    /// a newer version was declared during materialization - which should not
    /// happen under normal conditions - we can react accordingly.
    MaterializationFinished {
        path: ProjectRelativePathBuf,
        timestamp: Timestamp,
        version: Version,
        result: Result<(), SharedMaterializingError>,
        dispatcher: Option<EventDispatcher>,
    },

    CleanupFinished {
        path: ProjectRelativePathBuf,
        version: Version,
        result: Result<(), SharedMaterializingError>,
        dispatcher: Option<EventDispatcher>,
    },

    FinishUnmaterializationUpload {
        command: FinishUnmaterializationUpload,
        dispatcher: Option<EventDispatcher>,
    },
}

impl LowPriorityMaterializerCommand {
    fn dispatcher(&self) -> Option<EventDispatcher> {
        match self {
            Self::MaterializationFinished { dispatcher, .. }
            | Self::CleanupFinished { dispatcher, .. }
            | Self::FinishUnmaterializationUpload { dispatcher, .. } => dispatcher.clone(),
        }
    }
}

#[derive(Debug)]
struct VersionTracker(Version);

impl VersionTracker {
    fn new() -> Self {
        // Each Declare bumps the version, so that if an artifact is declared
        // a second time mid materialization of its previous version, we don't
        // incorrectly assume we materialized the latest version. We start with
        // 1 with because any disk state restored will start with version 0.
        Self(Version(1))
    }

    fn current(&self) -> Version {
        self.0
    }

    /// Increment the current version, return the previous  value
    fn next(&mut self) -> Version {
        let ret = self.current();
        self.0.0 += 1;
        ret
    }
}

#[pin_project]
struct CommandStream<T: 'static> {
    high_priority: UnboundedReceiver<MaterializerCommand<T>>,
    low_priority: UnboundedReceiver<LowPriorityMaterializerCommand>,
    refresh_ttl_ticker: Option<Interval>,
    io_buffer_ticker: Interval,
    clean_stale_ticker: Option<Interval>,
    clean_stale_fut: Option<BoxFuture<'static, Result<CleanResult, CleanFailure>>>,
    cancellation: Arc<dyn LivelinessObserverSync>,
}

enum Op<T: 'static> {
    Command(MaterializerCommand<T>),
    LowPriorityCommand(LowPriorityMaterializerCommand),
    RefreshTtls,
    Tick,
    CleanStaleRequest,
}

/// What the disk-materialization path should actually materialize for an artifact.
enum EntryDetails {
    /// Materialize a declared entry to disk via its method.
    Entry {
        entry: ActionDirectoryEntry<ActionSharedDirectory>,
        method: Arc<ArtifactMaterializationMethod>,
    },
    /// Nothing to materialize for the entry itself (e.g. only its deps need materializing).
    DepsOnly,
}

impl EntryDetails {
    fn kind(&self) -> &'static str {
        match self {
            EntryDetails::Entry { .. } => "entry",
            EntryDetails::DepsOnly => "deps_only",
        }
    }
}

impl<T: 'static> Stream for CommandStream<T> {
    type Item = Op<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        if !this.cancellation.is_alive_sync() {
            return Poll::Ready(None);
        }

        if let Poll::Ready(Some(e)) = this.high_priority.poll_recv(cx) {
            if let MaterializerCommand::Abort = e {
                return Poll::Ready(None);
            }
            return Poll::Ready(Some(Op::Command(e)));
        }

        if let Poll::Ready(Some(e)) = this.low_priority.poll_recv(cx) {
            return Poll::Ready(Some(Op::LowPriorityCommand(e)));
        }

        if let Some(ticker) = this.refresh_ttl_ticker.as_mut() {
            if ticker.poll_tick(cx).is_ready() {
                return Poll::Ready(Some(Op::RefreshTtls));
            }
        }

        if this.io_buffer_ticker.poll_tick(cx).is_ready() {
            return Poll::Ready(Some(Op::Tick));
        }

        // Ensure last clean completed before requesting a new one.
        if let Some(fut) = this.clean_stale_fut.as_mut() {
            if std::pin::pin!(fut).poll(cx).is_ready() {
                *this.clean_stale_fut = None;
            }
        } else if let Some(ticker) = this.clean_stale_ticker.as_mut() {
            if ticker.poll_tick(cx).is_ready() {
                return Poll::Ready(Some(Op::CleanStaleRequest));
            }
        }

        // We can never be done because we never drop the senders, so let's not bother.
        Poll::Pending
    }
}

impl<T: IoHandler> DeferredMaterializerCommandProcessor<T> {
    pub(super) fn new(
        io: Arc<T>,
        sqlite_db: Option<MaterializerStateSqliteDb>,
        rt: Handle,
        defer_write_actions: bool,
        command_sender: Arc<MaterializerSender<T>>,
        tree: ArtifactTree,
        cancellations: &'static CancellationContext,
        stats: Arc<DeferredMaterializerStats>,
        access_times_buffer: Option<BuckMutSet<ProjectRelativePathBuf>>,
        verbose_materializer_log: bool,
        daemon_dispatcher: EventDispatcher,
        clean_stale_config: CleanStaleConfig,
        rematerialization_ttl: Option<SignedDuration>,
        background_cleanup_gate: Arc<BackgroundCleanupGate>,
        cancellation: Arc<CancellationScope>,
    ) -> Self {
        let ttl_refresh_history = Vec::new();
        let ttl_refresh_instance = None;
        let version_tracker = VersionTracker::new();
        let buck_out_abs_path = disk_space_probe_path(io.fs(), io.buck_out_path());
        Self {
            io,
            sqlite_db,
            rt,
            cancellation,
            defer_write_actions,
            version_tracker,
            command_sender,
            tree,
            ttl_refresh_history,
            ttl_refresh_instance,
            cancellations,
            stats,
            access_times_buffer,
            verbose_materializer_log,
            daemon_dispatcher,
            buck_out_abs_path,
            clean_stale_config,
            rematerialization_ttl,
            background_cleanup_gate,
        }
    }

    pub(super) fn next_version(&mut self) -> Version {
        self.version_tracker.next()
    }

    fn spawn_from_rt<F, R>(
        rt: &Handle,
        dispatcher: &EventDispatcher,
        cancellation: &CancellationScope,
        f: F,
    ) -> BoxFuture<'static, Result<R, WeakFutureError>>
    where
        F: for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, R> + Send + 'static,
        R: Send + 'static,
    {
        let dispatcher = dispatcher.dupe();
        cancellation
            .spawn(rt, move |context| {
                with_dispatcher_async(dispatcher, f(context)).boxed()
            })
            .0
    }

    pub(super) fn configured_clean_stale_command(
        &self,
        dispatcher: EventDispatcher,
        dry_run: bool,
        tracked_only: bool,
        trigger: Trigger,
        origin: Origin,
    ) -> CleanStaleArtifactsCommand {
        CleanStaleArtifactsCommand::configured(
            &self.clean_stale_config,
            self.buck_out_abs_path.dupe(),
            dispatcher,
            dry_run,
            tracked_only,
            trigger,
            origin,
        )
    }

    pub(super) fn spawn<F, R>(
        &self,
        dispatcher: &EventDispatcher,
        f: F,
    ) -> BoxFuture<'static, Result<R, WeakFutureError>>
    where
        F: for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, R> + Send + 'static,
        R: Send + 'static,
    {
        Self::spawn_from_rt(&self.rt, dispatcher, &self.cancellation, f)
    }

    /// Loop that runs for as long as the materializer is alive.
    ///
    /// It takes commands via the `Materializer` trait methods.
    pub(super) async fn run(
        mut self,
        commands: MaterializerReceiver<T>,
        ttl_refresh: TtlRefreshConfiguration,
        access_time_updates: AccessTimesUpdates,
    ) {
        let MaterializerReceiver {
            high_priority,
            low_priority,
            counters,
        } = commands;

        let refresh_ttl_ticker = if ttl_refresh.enabled {
            Some(tokio::time::interval_at(
                tokio::time::Instant::now() + ttl_refresh.frequency,
                ttl_refresh.frequency,
            ))
        } else {
            None
        };

        let clean_stale_ticker = self.clean_stale_config.schedule.as_ref().map(|schedule| {
            tokio::time::interval_at(
                tokio::time::Instant::now() + schedule.start_offset,
                schedule.clean_period,
            )
        });

        let io_buffer_ticker = tokio::time::interval(std::time::Duration::from_secs(5));

        let mut stream = CommandStream {
            high_priority,
            low_priority,
            refresh_ttl_ticker,
            io_buffer_ticker,
            clean_stale_ticker,
            clean_stale_fut: None,
            cancellation: self.cancellation.observer(),
        };

        while let Some(op) = stream.next().await {
            match op {
                Op::Command(command) => {
                    self.process_one_command(command);
                    counters.ack_received();
                }
                Op::LowPriorityCommand(command) => {
                    self.process_one_low_priority_command(command);
                    counters.ack_received();
                }
                Op::RefreshTtls => {
                    // It'd be neat to just implement this in the refresh_stream itself and simply
                    // have this loop implicitly drive it, but we can't do that as the stream's
                    // and_then callback would have to capture `&tree`. So, instead, we store the
                    // JoinHandle and just avoid scheduling more than one, though this means we'll
                    // just miss ticks if we do take longer than a tick to run.

                    self.poll_current_ttl_refresh();

                    if self.ttl_refresh_instance.is_none() {
                        let ttl_refresh = self
                            .io
                            .create_ttl_refresh(&self.tree, ttl_refresh.min_ttl)
                            .map(|fut| {
                                // We sue a channel here and not JoinHandle so we get blocking
                                // `try_recv`.
                                let (tx, rx) = oneshot::channel();

                                let _refresh =
                                    self.spawn(&EventDispatcher::error_on_event(), move |_| {
                                        async move {
                                            let res = fut.await;
                                            let _ignored = tx.send((Timestamp::now(), res));
                                        }
                                        .boxed()
                                    });

                                rx
                            });

                        match ttl_refresh {
                            Some(ttl_refresh) => {
                                self.ttl_refresh_instance = Some(ttl_refresh);
                            }
                            None => self.ttl_refresh_history.push(TtlRefreshHistoryEntry {
                                at: Timestamp::now(),
                                outcome: None,
                            }),
                        }
                    }
                }
                Op::Tick => {
                    if matches!(access_time_updates, AccessTimesUpdates::Full) {
                        // Force a periodic flush.
                        self.flush_access_times();
                    };
                }
                Op::CleanStaleRequest => {
                    let Some(clean_guard) = self.background_cleanup_gate.try_start_cleanup() else {
                        continue;
                    };
                    let dispatcher = self.daemon_dispatcher.dupe();
                    let daemon_id = dispatcher.daemon_id().dupe();
                    let cmd = self.configured_clean_stale_command(
                        dispatcher,
                        false,
                        false,
                        Trigger::Scheduled,
                        Origin::DaemonSchedule,
                    );
                    let clean = cmd.create_clean_fut(&mut self, None, daemon_id);
                    stream.clean_stale_fut = Some(
                        async move {
                            let _clean_guard = clean_guard;
                            clean.await
                        }
                        .boxed(),
                    );
                }
            }
        }
    }

    fn process_one_command(&mut self, command: MaterializerCommand<T>) {
        let dispatcher = command.dispatcher();
        with_dispatcher_opt(dispatcher, || match command {
            // Entry point for `get_materialized_file_paths` calls
            MaterializerCommand::GetMaterializedFilePaths(paths, result_sender) => {
                let result = paths
                    .into_iter()
                    .map(|p| self.tree.file_contents_path(p, self.io.digest_config()))
                    .collect::<Vec<_>>();
                result_sender.send(result).ok();
            }
            MaterializerCommand::DeclareExisting(artifacts, parent_id, _) => {
                maybe_proxy_current_span(parent_id, || {
                    for DeclareArtifactPayload { path, artifact } in artifacts {
                        self.declare_existing(&path, artifact);
                    }
                })
            }
            // Entry point for `declare_{copy|cas}` calls
            MaterializerCommand::Declare(
                DeclareArtifactPayload {
                    path,
                    artifact: value,
                },
                method,
                event_dispatcher,
                parent_id,
            ) => maybe_proxy_current_span(parent_id, || {
                self.maybe_log_command(&event_dispatcher, || {
                    buck2_data::materializer_command::Data::Declare(
                        buck2_data::materializer_command::Declare {
                            path: path.to_string(),
                        },
                    )
                });

                self.declare(&path, value, method, &event_dispatcher);
            }),
            MaterializerCommand::MatchArtifacts(paths, sender) => {
                let all_matches = paths
                    .into_iter()
                    .all(|(path, value)| self.match_artifact(path, value));
                sender.send(all_matches).ok();
            }
            MaterializerCommand::HasArtifact(path, sender, _) => {
                sender.send(self.has_artifact(path)).ok();
            }
            MaterializerCommand::InvalidateFilePaths(
                paths,
                sender,
                event_dispatcher,
                parent_id,
            ) => maybe_proxy_current_span(parent_id, || {
                tracing::trace!(
                    paths = ?paths,
                    "invalidate paths",
                );
                self.maybe_log_command(&event_dispatcher, || {
                    buck2_data::materializer_command::Data::InvalidateFilePaths(
                        buck2_data::materializer_command::InvalidateFilePaths {
                            paths: paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                        },
                    )
                });

                let existing_futs = self.tree.invalidate_paths_and_collect_futures(
                    paths,
                    self.sqlite_db.as_mut(),
                    &self.stats,
                );

                // TODO: This probably shouldn't return a CleanFuture
                sender
                    .send(
                        async move { join_all_existing_futs(existing_futs?).await }
                            .boxed()
                            .shared(),
                    )
                    .ok();
            }),
            // Entry point for `ensure_materialized` calls
            MaterializerCommand::Ensure(
                paths,
                purpose,
                event_dispatcher,
                parent_id,
                fut_sender,
            ) => maybe_proxy_current_span(parent_id, || {
                self.maybe_log_command(&event_dispatcher, || {
                    buck2_data::materializer_command::Data::Ensure(
                        buck2_data::materializer_command::Ensure {
                            paths: paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                        },
                    )
                });

                if let MaterializationPurpose::FinalOutput { .. } = purpose {
                    self.promote_final_output_closure(&paths);
                }

                fut_sender
                    .send(self.materialize_many_artifacts(paths, event_dispatcher))
                    .ok();
            }),
            MaterializerCommand::Extension(ext, _) => ext.execute(self),
            MaterializerCommand::Abort => unreachable!(),
            MaterializerCommand::GetArtifactEntriesForMaterializedPaths {
                paths,
                fetch_root_artifact_entries_for_subpaths,
                sender,
            } => {
                sender
                    .send(self.get_artifact_entries_for_materialized_paths(
                        paths,
                        fetch_root_artifact_entries_for_subpaths,
                    ))
                    .ok();
            }
        })
    }

    fn process_one_low_priority_command(&mut self, command: LowPriorityMaterializerCommand) {
        let dispatcher = command.dispatcher();
        with_dispatcher_opt(dispatcher, || match command {
            // Materialization of artifact succeeded
            LowPriorityMaterializerCommand::MaterializationFinished {
                path,
                timestamp,
                version,
                result,
                dispatcher: _,
            } => {
                self.materialization_finished(path, timestamp, version, result);
            }
            LowPriorityMaterializerCommand::CleanupFinished {
                path,
                version,
                result,
                dispatcher: _,
            } => {
                self.tree.cleanup_finished(path, version, result);
            }
            LowPriorityMaterializerCommand::FinishUnmaterializationUpload {
                command,
                dispatcher: _,
            } => command.execute(self),
        })
    }

    /// Poll the current TTL refresh and remove it if it's done. Add the outcome to
    /// ttl_refresh_history.
    pub(super) fn poll_current_ttl_refresh(&mut self) {
        self.ttl_refresh_instance = match self.ttl_refresh_instance.take() {
            Some(mut curr) => match curr.try_recv() {
                Ok((at, outcome)) => {
                    // Done
                    self.ttl_refresh_history.push(TtlRefreshHistoryEntry {
                        at,
                        outcome: Some(outcome),
                    });
                    None
                }
                Err(TryRecvError::Empty) => {
                    // Leave it alone.
                    Some(curr)
                }
                Err(TryRecvError::Closed) => {
                    // Shouldnt really happen unless Tokio is shutting down, but be safe.
                    self.ttl_refresh_history.push(TtlRefreshHistoryEntry {
                        at: Timestamp::now(),
                        outcome: Some(Err(buck2_error!(buck2_error::ErrorTag::Tier0, "Shutdown"))),
                    });
                    None
                }
            },
            None => None,
        };
    }

    pub(super) fn flush_access_times(&mut self) -> String {
        if let Some(access_times_buffer) = self.access_times_buffer.as_mut() {
            if access_times_buffer.is_empty() {
                return "Access times buffer is empty".to_owned();
            }

            let size = access_times_buffer.len();
            let buffer = std::mem::take(access_times_buffer);
            let now = Instant::now();
            tracing::debug!("Flushing access times buffer");
            if let Some(sqlite_db) = self.sqlite_db.as_mut() {
                if let Err(e) = sqlite_db
                    .materializer_state_table()
                    .update_access_times(buffer.iter().collect::<Vec<_>>())
                {
                    soft_error_or_panic!("materializer_materialize_error", e);
                    return "Found error while updating access times in sqlite db".to_owned();
                }
            }
            return format!(
                "Finished flushing {} entries in {} ms",
                size,
                (Instant::now() - now).as_millis(),
            );
        }
        "Access time updates are disabled. Consider removing `update_access_times = false` from your .buckconfig".to_owned()
    }

    /// One result per path, in order. A path with nothing to wait for (already materialized,
    /// or never declared) yields `Ok(())` immediately.
    fn materialize_many_artifacts(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
        event_dispatcher: EventDispatcher,
    ) -> BoxStream<'static, Result<(), MaterializationError>> {
        // Jobs start eagerly and are only awaited in order, keeping order and per-path errors.
        let tasks: Vec<_> = paths
            .into_iter()
            .map(
                |path| match self.materialize_artifact(path.as_ref(), event_dispatcher.dupe()) {
                    Some(fut) => future::Either::Left(fut.map_err(move |e| match e {
                        SharedMaterializingError::Error(source) => {
                            MaterializationError::Error { path, source }
                        }
                        SharedMaterializingError::NotFound(source) => {
                            MaterializationError::NotFound { source }
                        }
                    })),
                    None => future::Either::Right(future::ready(Ok(()))),
                },
            )
            .collect();

        stream::iter(tasks).then(|task| task).boxed()
    }

    fn promote_final_output_closure(&mut self, paths: &[ProjectRelativePathBuf]) {
        let mut pending = paths.to_vec();
        let mut visited: BuckMutSet<ProjectRelativePathBuf> = BuckMutSet::default();
        let mut materialized_roots = Vec::new();

        // Traverse all artifacts and deps transitively
        while let Some(path) = pending.pop() {
            let Some((root, data)) = Self::find_artifact_containing_path(&mut self.tree, &path)
            else {
                continue;
            };
            let root = root.to_owned();
            if !visited.insert(root.clone()) {
                continue;
            }

            let deps = data.deps.dupe();
            let promotion = if data.classification == ArtifactClassification::FinalOutput {
                None
            } else {
                data.classification = ArtifactClassification::FinalOutput;
                Some((
                    matches!(
                        data.stage,
                        ArtifactMaterializationStage::Materialized { .. }
                    ),
                    data.logical_size_bytes,
                ))
            };

            if let Some(deps) = deps.as_ref() {
                pending.extend(self.tree.find_artifacts(deps));
            }

            if let Some((true, logical_size_bytes)) = promotion {
                self.stats.promote_materialized(logical_size_bytes);
                materialized_roots.push(root);
            }
        }

        if let Some(sqlite_db) = self.sqlite_db.as_mut()
            && let Err(error) = sqlite_db
                .materializer_state_table()
                .update_classifications(&materialized_roots, ArtifactClassification::FinalOutput)
        {
            soft_error_or_panic!("materializer_update_classification_error", error);
        }
    }

    fn declare_existing(&mut self, path: &ProjectRelativePath, value: ArtifactValue) {
        let metadata = value.entry().dupe();
        let logical_size_bytes = artifact_metadata_size(&metadata);
        let classification = {
            let mut path_iter = path.iter();
            match self.tree.prefix_get(&mut path_iter) {
                Some(data)
                    if path_iter.next().is_none()
                        && matches!(
                            &data.stage,
                            ArtifactMaterializationStage::Materialized {
                                metadata: previous_metadata,
                                ..
                            } if artifact_metadata_matches_entry(previous_metadata, &metadata)
                        ) =>
                {
                    data.classification
                }
                _ => ArtifactClassification::IntermediateOnly,
            }
        };
        if let Err(error) = self.tree.invalidate_paths_and_collect_futures(
            vec![path.to_owned()],
            self.sqlite_db.as_mut(),
            &self.stats,
        ) {
            soft_error_or_panic!("materializer_declare_existing_invalidation_error", error);
        }
        on_materialization(
            self.sqlite_db.as_mut(),
            path,
            &metadata,
            Timestamp::now(),
            classification,
            "materializer_declare_existing_error",
        );

        self.tree.insert(
            path.iter().map(|f| f.to_owned()),
            Box::new(ArtifactMaterializationData {
                deps: value.deps().duped(),
                classification,
                logical_size_bytes,
                stage: ArtifactMaterializationStage::Materialized {
                    metadata,
                    last_access_time: Timestamp::now(),
                    rematerialization_method: None,
                    active: true,
                },
                processing: Processing::Done(self.version_tracker.next()),
            }),
        );
        self.stats
            .add_materialized(classification, logical_size_bytes);
    }

    fn declare(
        &mut self,
        path: &ProjectRelativePath,
        value: ArtifactValue,
        method: Box<ArtifactMaterializationMethod>,
        event_dispatcher: &EventDispatcher,
    ) {
        self.stats.declares.fetch_add(1, Ordering::Relaxed);

        // Check if artifact to be declared is same as artifact that's already materialized.
        let mut path_iter = path.iter();
        if let Some(data) = self.tree.prefix_get_mut(&mut path_iter) {
            match &data.stage {
                ArtifactMaterializationStage::Materialized {
                    metadata,
                    last_access_time,
                    rematerialization_method,
                    ..
                } => {
                    // NOTE: This is for testing performance when hitting mismatches with disk
                    // state. Unwrapping isn't ideal, but we can't report errors here.
                    let force_mismatch = buck2_env!(
                        "BUCK2_TEST_FORCE_DECLARE_MISMATCH",
                        bool,
                        applicability = testing
                    )
                    .unwrap();

                    if path_iter.next().is_none()
                        && artifact_metadata_matches_entry(metadata, value.entry())
                        && !force_mismatch
                    {
                        // In this case, the entry declared matches the already materialized
                        // entry on disk, so just update the deps field but leave
                        // the artifact as materialized.
                        tracing::trace!(
                            path = %path,
                            "already materialized, updating deps only",
                        );
                        let deps = value.deps().duped();
                        data.stage = ArtifactMaterializationStage::Materialized {
                            metadata: metadata.dupe(),
                            last_access_time: *last_access_time,
                            rematerialization_method: rematerialization_method.dupe(),
                            active: true,
                        };
                        data.deps = deps;

                        self.stats.declares_reused.fetch_add(1, Ordering::Relaxed);

                        return;
                    }
                }
                ArtifactMaterializationStage::Declared { entry, .. } => {
                    if path_iter.next().is_none() && entry == value.entry() {
                        // In this case, the entry declared matches the already declared entry.
                        tracing::trace!(
                            path = %path,
                            "already declared, updating deps only",
                        );
                        let deps = value.deps().duped();
                        data.deps = deps;

                        return;
                    }
                }
            }
        }

        // We don't have a matching artifact. Declare it.
        let version = self.version_tracker.next();

        tracing::trace!(
            path = %path,
            method = %method,
            value = %value.entry(),
            version = %version,
            "declare artifact",
        );

        // Always invalidate materializer state before actual deleting from filesystem
        // so there will never be a moment where artifact is deleted but materializer
        // thinks it still exists.
        let existing_futs = self.tree.invalidate_paths_and_collect_futures(
            vec![path.to_owned()],
            self.sqlite_db.as_mut(),
            &self.stats,
        );

        let existing_futs = ExistingFutures(existing_futs);

        let method = Arc::from(method);

        let future = ProcessingFuture::Cleaning(clean_path(
            &self.io,
            path.to_owned(),
            version,
            self.command_sender.dupe(),
            existing_futs,
            &self.rt,
            &self.cancellation,
            event_dispatcher,
        ));

        let data = Box::new(ArtifactMaterializationData {
            deps: value.deps().duped(),
            classification: ArtifactClassification::IntermediateOnly,
            logical_size_bytes: artifact_metadata_size(value.entry()),
            stage: ArtifactMaterializationStage::Declared {
                entry: value.entry().dupe(),
                method,
            },
            processing: Processing::active(future, version),
        });
        self.tree.insert(path.iter().map(|f| f.to_owned()), data);
    }

    /// Check if artifact to be declared is same as artifact that's already materialized.
    #[instrument(level = "debug", skip(self), fields(path = %path, value = %value.entry()))]
    fn match_artifact(&mut self, path: ProjectRelativePathBuf, value: ArtifactValue) -> bool {
        let mut path_iter = path.iter();
        let data = match self.tree.prefix_get_mut(&mut path_iter) {
            Some(data) => data,
            None => {
                tracing::trace!("overlapping below");
                return false;
            }
        };

        // Something was declared above our path.
        if path_iter.next().is_some() {
            tracing::trace!("overlapping above");
            return false;
        }

        let is_match = match &data.stage {
            ArtifactMaterializationStage::Materialized { metadata, .. } => {
                let is_match = artifact_metadata_matches_entry(metadata, value.entry());
                tracing::trace!("materialized: found {}, is_match: {}", metadata, is_match);
                is_match
            }
            ArtifactMaterializationStage::Declared { entry, .. } => {
                // NOTE: In theory, if something was declared here, we should probably be able to
                // just re-declare over it?
                let is_match = value.entry() == entry;
                tracing::trace!("declared: found {}, is_match: {}", entry, is_match);
                is_match
            }
        };

        // In practice, having a matching artifact with different deps isn't actually *possible*
        // right now, because the deps are derived from the artifact value and we'll always have
        // declared them before. But, if we have a local action cache and persist that as well as
        // materializer state across restarts, then eventually we could have a match with something
        // that hasn't had its deps populated yet (since the materializer state does not know about
        // deps).
        if is_match {
            if let ArtifactMaterializationStage::Materialized {
                last_access_time,
                active,
                ..
            } = &mut data.stage
            {
                *active = true;
                *last_access_time = Timestamp::now();
                if let Some(sqlite_db) = &mut self.sqlite_db {
                    if let Err(e) = sqlite_db
                        .materializer_state_table()
                        .update_access_times(vec![&path])
                    {
                        soft_error_or_panic!("match_artifact_update_time", e);
                    }
                }
            }
            if let Some(deps) = value.deps() {
                data.deps = Some(deps.dupe())
            }
        }

        is_match
    }

    fn has_artifact(&mut self, path: ProjectRelativePathBuf) -> bool {
        let mut path_iter = path.iter();
        let Some(data) = self.tree.prefix_get_mut(&mut path_iter) else {
            return false;
        };
        // Something was declared above our path.
        if path_iter.next().is_some() {
            return false;
        }

        match &mut data.stage {
            ArtifactMaterializationStage::Materialized {
                metadata: _,
                last_access_time,
                active,
                ..
            } => {
                // Treat this case much like a `declare_existing`
                *active = true;
                *last_access_time = Timestamp::now();
                if let Some(sqlite_db) = &mut self.sqlite_db {
                    if let Err(e) = sqlite_db
                        .materializer_state_table()
                        .update_access_times(vec![&path])
                    {
                        soft_error_or_panic!("has_artifact_update_time", e);
                    }
                }
            }
            ArtifactMaterializationStage::Declared { .. } => {
                // Nothing to do here
            }
        }

        true
    }

    #[instrument(level = "debug", skip(self), fields(path = %path))]
    pub(super) fn materialize_artifact(
        &mut self,
        path: &ProjectRelativePath,
        event_dispatcher: EventDispatcher,
    ) -> Option<MaterializingFuture> {
        self.materialize_artifact_recurse(MaterializeStack::Empty, path, event_dispatcher)
    }

    fn materialize_artifact_recurse(
        &mut self,
        stack: MaterializeStack<'_>,
        path: &ProjectRelativePath,
        event_dispatcher: EventDispatcher,
    ) -> Option<MaterializingFuture> {
        let stack = MaterializeStack::Child(&stack, path);
        // We only add context to outer error, because adding context to the future
        // is expensive. Errors in futures should add stack context themselves.
        match self.materialize_artifact_inner(stack, path, event_dispatcher) {
            Ok(res) => res,
            Err(e) => Some(
                future::err(SharedMaterializingError::Error(
                    e.context(format!("materializing {stack}")),
                ))
                .boxed()
                .shared(),
            ),
        }
    }

    fn get_artifact_entries_for_materialized_paths(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
        fetch_root_artifact_entries_for_subpaths: bool,
    ) -> Vec<
        Option<(
            ProjectRelativePathBuf,
            ActionDirectoryEntry<ActionSharedDirectory>,
        )>,
    > {
        paths
            .into_iter()
            .map(|p| {
                let (root_path, data) = Self::find_artifact_containing_path(&mut self.tree, &p)?;

                let base_entry = match &data.stage {
                    ArtifactMaterializationStage::Materialized { metadata, .. } => {
                        Some(metadata.dupe())
                    }
                    ArtifactMaterializationStage::Declared { entry, .. } => Some(entry.dupe()),
                }?;

                if root_path == p || fetch_root_artifact_entries_for_subpaths {
                    // Path exactly matches artifact root, or we are fetching base entries for projected paths
                    Some((p, base_entry))
                } else {
                    // Not at root and not fetching projected entries
                    None
                }
            })
            .collect()
    }

    /// For a given `path` (which could point inside the artifact) returns the path and data for the artifact which contains it.
    fn find_artifact_containing_path<'a, 'b>(
        tree: &'a mut ArtifactTree,
        path: &'b ProjectRelativePath,
    ) -> Option<(&'b ProjectRelativePath, &'a mut ArtifactMaterializationData)> {
        let mut path_iter = path.iter();
        let data = tree.prefix_get_mut(&mut path_iter)?;
        let path = path.strip_suffix(path_iter.as_path()).unwrap();
        Some((path, data))
    }

    fn materialize_artifact_inner(
        &mut self,
        stack: MaterializeStack<'_>,
        path: &ProjectRelativePath,
        event_dispatcher: EventDispatcher,
    ) -> buck2_error::Result<Option<MaterializingFuture>> {
        // TODO(nga): rewrite without recursion or figure out why we overflow stack here.
        check_stack_overflow().tag(ErrorTag::ServerStackOverflow)?;

        // Get the data about the artifact, or return early if materializing/materialized
        let (path, data) = match Self::find_artifact_containing_path(&mut self.tree, path) {
            None => {
                // Never declared, nothing to do
                tracing::debug!("not known");
                return Ok(None);
            }
            Some(x) => x,
        };

        let cleaning_fut = match data.processing.active_ref() {
            Some(active) => match &active.future {
                ProcessingFuture::Cleaning(f) => Some(f.clone()),
                ProcessingFuture::Materializing(f) => {
                    tracing::debug!("join existing future");
                    return Ok(Some(f.clone()));
                }
            },
            None => None,
        };

        let deps = data.deps.dupe();
        let check_deps = deps.is_some();
        let entry_details = match &mut data.stage {
            ArtifactMaterializationStage::Declared { entry, method } => EntryDetails::Entry {
                entry: entry.dupe(),
                method: method.dupe(),
            },
            ArtifactMaterializationStage::Materialized {
                last_access_time, ..
            } => match check_deps {
                true => EntryDetails::DepsOnly,
                false => {
                    if let Some(ref mut buffer) = self.access_times_buffer.as_mut() {
                        // TODO (torozco): Why is it legal for something to be Materialized + Cleaning?
                        let timestamp = Timestamp::now();
                        *last_access_time = timestamp;

                        // NOTE (T142264535): Artifacts are normally declared before they are
                        // materialized, but incremental actions materialize outputs of a previous
                        // run without declaring them, so `active` is not checked here.
                        if buffer.insert(path.to_buf()) {
                            tracing::debug!(
                                "nothing to materialize, adding to access times buffer"
                            );
                        }
                    }

                    return Ok(None);
                }
            },
        };

        let version = self.version_tracker.next();

        let method = match &entry_details {
            EntryDetails::Entry { method, .. } => Some(method.as_ref()),
            EntryDetails::DepsOnly => None,
        };

        // `method` is grepped for by tests/core/trace_io/test_trace_io.py (it asserts on the
        // Debug method name, e.g. `LocalCopy`). Keep logging it so refactors don't break those tests.
        tracing::debug!(
            entry_details = entry_details.kind(),
            method = ?method,
            has_deps = deps.is_some(),
            version = %version,
            cleaning = cleaning_fut.is_some(),
            "materialize artifact"
        );
        // Those are special because if the artifact copies from other artifacts, we must materialize them first
        let materialize_copy_source_tasks =
            self.materialize_copy_source_tasks(&stack, &event_dispatcher, path, method);

        // The artifact might have symlinks pointing to other artifacts. We must
        // materialize them as well, to avoid dangling symlinks.
        let materialize_symlink_destination_tasks =
            self.materialize_symlink_destination_tasks(&stack, &event_dispatcher, path, deps);

        let spawn_dispatcher = event_dispatcher.dupe();
        // Create a task to await deps and materialize ourselves
        let path_buf = path.to_buf();
        let entry_path = path.to_buf();
        let io = self.io.dupe();
        let command_sender = self.command_sender.dupe();
        let task = self
            .spawn(&spawn_dispatcher, move |cancellations| {
                async move {
                    let materialize_entry: BoxFuture<'_, Result<(), MaterializeEntryError>> =
                        match entry_details {
                            EntryDetails::Entry { entry, method } => async move {
                                io.materialize_entry(
                                    entry_path,
                                    method,
                                    entry,
                                    event_dispatcher,
                                    cancellations,
                                )
                                .await
                            }
                            .boxed(),
                            EntryDetails::DepsOnly => future::ready(Ok(())).boxed(),
                        };
                    let timestamp = Timestamp::now();
                    // Materialize the deps and this entry. Regardless of whether this succeeds or fails we
                    // need to notify the materializer, so don't check the result.
                    let res = Self::perform_materialization(
                        cleaning_fut,
                        materialize_copy_source_tasks,
                        materialize_symlink_destination_tasks,
                        materialize_entry,
                    )
                    .await;

                    // Materialization finished, notify the command thread
                    let _ignored = command_sender.send_low_priority(
                        LowPriorityMaterializerCommand::MaterializationFinished {
                            path: path_buf,
                            timestamp,
                            version,
                            result: res.dupe(),
                            dispatcher: get_dispatcher_opt(),
                        },
                    );

                    res
                }
                .boxed()
            })
            .map(|r| r.unwrap_or_else(|e| Err(SharedMaterializingError::Error(task_stopped(e)))))
            .boxed()
            .shared();

        let data = self.tree.prefix_get_mut(&mut path.iter()).unwrap();
        data.processing =
            Processing::active(ProcessingFuture::Materializing(task.clone()), version);

        Ok(Some(task))
    }

    async fn perform_materialization(
        cleaning_future: Option<CleaningFuture>,
        materialize_copy_source_tasks: Vec<MaterializingFuture>,
        materialize_symlink_destination_tasks: Vec<MaterializingFuture>,
        materialize_entry: impl Future<Output = Result<(), MaterializeEntryError>>,
    ) -> Result<(), SharedMaterializingError> {
        // If there is an existing future trying to delete conflicting paths, we must wait for it
        // to finish before we can start materialization.
        if let Some(cleaning_fut) = cleaning_future {
            cleaning_fut
                .await
                .with_buck_error_context(|| "Error cleaning output path")
                .map_err(SharedMaterializingError::Error)?;
        };

        // In case this is a local copy, we first need to materialize the
        // artifacts we are copying from, before we can copy them.
        for t in materialize_copy_source_tasks {
            t.await?;
        }

        // Windows symlinks need to be specified whether it is to a file or target. We rely on the
        // target file existing to determine this. Ensure symlink targets exist before the entry
        // is materialized for Windows. For non-Windows, do everything concurrently.
        if cfg!(windows) {
            for t in materialize_symlink_destination_tasks {
                t.await?;
            }
            materialize_entry.await?;
        } else {
            materialize_entry.await?;
            for t in materialize_symlink_destination_tasks {
                t.await?;
            }
        }
        Ok(())
    }

    fn materialize_symlink_destination_tasks(
        &mut self,
        stack: &MaterializeStack,
        event_dispatcher: &EventDispatcher,
        path: &ProjectRelativePath,
        deps: Option<ActionSharedDirectory>,
    ) -> Vec<MaterializingFuture> {
        if let Some(deps) = deps.as_ref() {
            self.tree
                .find_artifacts(deps)
                .into_iter()
                .filter_map(|p| {
                    self.materialize_artifact_recurse(
                        MaterializeStack::Child(stack, path),
                        p.as_ref(),
                        event_dispatcher.dupe(),
                    )
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    }

    fn materialize_copy_source_tasks(
        &mut self,
        stack: &MaterializeStack,
        event_dispatcher: &EventDispatcher,
        path: &ProjectRelativePath,
        method: Option<&ArtifactMaterializationMethod>,
    ) -> Vec<MaterializingFuture> {
        match method {
            Some(ArtifactMaterializationMethod::LocalCopy(_, copied_artifacts)) => copied_artifacts
                .iter()
                .filter_map(|a| {
                    self.materialize_artifact_recurse(
                        MaterializeStack::Child(stack, path),
                        a.src.as_ref(),
                        event_dispatcher.dupe(),
                    )
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    }

    #[instrument(level = "debug", skip(self, result), fields(path = %artifact_path))]
    fn materialization_finished(
        &mut self,
        artifact_path: ProjectRelativePathBuf,
        timestamp: Timestamp,
        version: Version,
        result: Result<(), SharedMaterializingError>,
    ) {
        match self.tree.prefix_get_mut(&mut artifact_path.iter()) {
            Some(info) => {
                if info.processing.current_version() > version {
                    // We can only unset the future if version matches.
                    // Otherwise, we may be unsetting a different future from a newer version.
                    tracing::debug!("version conflict");
                    return;
                }

                if result.is_err() {
                    let version = self.version_tracker.next();
                    match &info.stage {
                        ArtifactMaterializationStage::Materialized { .. } => {
                            tracing::debug!("artifact deps materialization failed, doing nothing");
                            // If already materialized, we only attempted to materialize deps, which means the error did
                            // not occur when materializing the artifact itself. There is no need to clean the artifact path
                            // and doing so will make the filesystem out of sync with materializer state.
                            info.processing = Processing::Done(version);
                        }
                        ArtifactMaterializationStage::Declared { .. } => {
                            tracing::debug!("materialization failed, redeclaring artifact");
                            // Even though materialization failed, something may have still materialized at artifact_path,
                            // so we need to delete anything at artifact_path before we ever retry materializing it.
                            // TODO(scottcao): Once command processor accepts an ArtifactTree instead of initializing one,
                            // add a test case to ensure this behavior.
                            let dispatcher = get_dispatcher_opt()
                                .unwrap_or_else(EventDispatcher::error_on_event);
                            let future = ProcessingFuture::Cleaning(clean_path(
                                &self.io,
                                artifact_path.clone(),
                                version,
                                self.command_sender.dupe(),
                                ExistingFutures::empty(),
                                &self.rt,
                                &self.cancellation,
                                &dispatcher,
                            ));
                            info.processing = Processing::active(future, version);
                        }
                    }
                } else {
                    tracing::debug!(has_deps = info.deps.is_some(), "transition to Materialized");
                    let new_stage = match &info.stage {
                        ArtifactMaterializationStage::Materialized { .. } => {
                            // This happens if deps = true. In this case, the entry itself was not
                            // materialized again, but its deps have been. We need to clear the
                            // waiting future regardless.
                            tracing::debug!("artifact is already materialized");
                            None
                        }
                        ArtifactMaterializationStage::Declared { entry, method } => {
                            let metadata = entry.dupe();
                            // NOTE: We only insert this artifact if there isn't an in-progress cleanup
                            // future on this path.
                            on_materialization(
                                self.sqlite_db.as_mut(),
                                &artifact_path,
                                &metadata,
                                timestamp,
                                info.classification,
                                "materializer_finished_error",
                            );

                            Some(ArtifactMaterializationStage::Materialized {
                                metadata,
                                last_access_time: timestamp,
                                rematerialization_method:
                                    ArtifactRematerializationMethod::from_materialization_method(
                                        method,
                                    ),
                                active: true,
                            })
                        }
                    };

                    if let Some(new_stage) = new_stage {
                        info.stage = new_stage;
                        self.stats
                            .add_materialized(info.classification, info.logical_size_bytes);
                    }

                    info.processing = Processing::Done(version);
                }
            }
            None => {
                // NOTE: This can happen if a path got invalidated while it was being materialized.
                tracing::debug!("materialization_finished but path is vacant!")
            }
        }
    }

    fn maybe_log_command<F>(&self, event_dispatcher: &EventDispatcher, f: F)
    where
        F: FnOnce() -> buck2_data::materializer_command::Data,
    {
        if self.verbose_materializer_log {
            let data = Some(f());
            event_dispatcher.instant_event(buck2_data::MaterializerCommand { data });
        }
    }
}

/// Record an artifact materialized at `path` in the materializer state database.
fn on_materialization(
    sqlite_db: Option<&mut MaterializerStateSqliteDb>,
    path: &ProjectRelativePath,
    metadata: &ArtifactMetadata,
    timestamp: Timestamp,
    classification: ArtifactClassification,
    error_name: &'static str,
) {
    if let Some(sqlite_db) = sqlite_db {
        if let Err(e) =
            sqlite_db
                .materializer_state_table()
                .insert(path, metadata, timestamp, classification)
        {
            soft_error_or_panic!(error_name, e);
        }
    }
}

/// Spawns a future to clean output paths while waiting for any
/// pending future to finish.
fn clean_path<T: IoHandler>(
    io: &Arc<T>,
    path: ProjectRelativePathBuf,
    version: Version,
    command_sender: Arc<MaterializerSender<T>>,
    existing_futs: ExistingFutures,
    rt: &Handle,
    cancellation: &CancellationScope,
    dispatcher: &EventDispatcher,
) -> CleaningFuture {
    if existing_futs.is_empty() {
        let io = io.dupe();
        return DeferredMaterializerCommandProcessor::<T>::spawn_from_rt(
            rt,
            dispatcher,
            cancellation,
            move |cancellations| io.clean_path(path, version, command_sender, cancellations),
        )
        .map(|result| result.unwrap_or_else(|error| Err(task_stopped(error))))
        .boxed()
        .shared();
    }
    DeferredMaterializerCommandProcessor::<T>::spawn_from_rt(rt, dispatcher, cancellation, {
        let io = io.dupe();
        move |cancellations| {
            async move {
                join_all_existing_futs(existing_futs.into_result()?).await?;
                io.clean_path(path, version, command_sender, cancellations)
                    .await
            }
            .boxed()
        }
    })
    .map(|r| r.unwrap_or_else(|e| Err(task_stopped(e))))
    .boxed()
    .shared()
}

/// A wrapper type around the Result it contains. Used to expose some extra methods.
struct ExistingFutures(buck2_error::Result<Vec<(ProjectRelativePathBuf, ProcessingFuture)>>);

impl ExistingFutures {
    fn is_empty(&self) -> bool {
        self.0.as_ref().is_ok_and(Vec::is_empty)
    }

    fn into_result(self) -> buck2_error::Result<Vec<(ProjectRelativePathBuf, ProcessingFuture)>> {
        self.0
    }

    fn empty() -> Self {
        Self(Ok(Vec::new()))
    }
}

/// The error for a materializer task that did not finish. A join error is a panic or abort, as a
/// tokio join error was before tasks were tracked; cancellation only happens when the tenant is
/// evicted or the materializer is dropped at shutdown.
pub(super) fn task_stopped(error: WeakFutureError) -> buck2_error::Error {
    match error {
        WeakFutureError::JoinError => {
            buck2_error!(ErrorTag::Tokio, "Materializer task panicked or was aborted")
        }
        WeakFutureError::Cancelled => buck2_error!(
            ErrorTag::InterruptedByDaemonShutdown,
            "Materializer task was cancelled"
        ),
    }
}

/// Exposing methods for testing purposes only.
#[cfg(test)]
#[doc(hidden)]
pub(super) trait TestingDeferredMaterializerCommandProcessor<T> {
    fn testing_has_artifact(&mut self, path: ProjectRelativePathBuf) -> bool;
    fn testing_is_path_materialized(&self, path: &ProjectRelativePath) -> bool;
    fn testing_declare_existing(&mut self, path: &ProjectRelativePath, value: ArtifactValue);

    fn testing_process_one_low_priority_command(&mut self, command: LowPriorityMaterializerCommand);

    fn testing_get_artifact_entries_for_materialized_paths(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> Vec<
        Option<(
            ProjectRelativePathBuf,
            ActionDirectoryEntry<ActionSharedDirectory>,
        )>,
    >;

    fn testing_get_root_artifact_entries_for_subpaths(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> Vec<
        Option<(
            ProjectRelativePathBuf,
            ActionDirectoryEntry<ActionSharedDirectory>,
        )>,
    >;

    fn testing_declare(&mut self, path: &ProjectRelativePath, value: ArtifactValue);

    fn testing_process_one_command(&mut self, command: MaterializerCommand<T>);

    fn testing_materialization_finished(
        &mut self,
        artifact_path: ProjectRelativePathBuf,
        timestamp: Timestamp,
        result: Result<(), SharedMaterializingError>,
    );
}

#[cfg(test)]
#[doc(hidden)]
impl<T: IoHandler> TestingDeferredMaterializerCommandProcessor<T>
    for DeferredMaterializerCommandProcessor<T>
{
    fn testing_has_artifact(&mut self, path: ProjectRelativePathBuf) -> bool {
        self.has_artifact(path)
    }

    fn testing_is_path_materialized(&self, path: &ProjectRelativePath) -> bool {
        match self.tree.prefix_get(&mut path.iter()) {
            None => false,
            Some(data) => {
                matches!(
                    data.stage,
                    ArtifactMaterializationStage::Materialized { .. }
                )
            }
        }
    }

    fn testing_declare_existing(&mut self, path: &ProjectRelativePath, value: ArtifactValue) {
        self.declare_existing(path, value)
    }

    fn testing_process_one_low_priority_command(
        &mut self,
        command: LowPriorityMaterializerCommand,
    ) {
        self.process_one_low_priority_command(command)
    }

    fn testing_get_artifact_entries_for_materialized_paths(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> Vec<
        Option<(
            ProjectRelativePathBuf,
            ActionDirectoryEntry<ActionSharedDirectory>,
        )>,
    > {
        self.get_artifact_entries_for_materialized_paths(paths, false)
    }

    fn testing_get_root_artifact_entries_for_subpaths(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> Vec<
        Option<(
            ProjectRelativePathBuf,
            ActionDirectoryEntry<ActionSharedDirectory>,
        )>,
    > {
        self.get_artifact_entries_for_materialized_paths(paths, true)
    }

    fn testing_declare(&mut self, path: &ProjectRelativePath, value: ArtifactValue) {
        let dispatcher = self.daemon_dispatcher.dupe();
        self.declare(
            path,
            value,
            Box::new(ArtifactMaterializationMethod::Test),
            &dispatcher,
        )
    }

    fn testing_process_one_command(&mut self, command: MaterializerCommand<T>) {
        self.process_one_command(command)
    }

    fn testing_materialization_finished(
        &mut self,
        artifact_path: ProjectRelativePathBuf,
        timestamp: Timestamp,
        result: Result<(), SharedMaterializingError>,
    ) {
        self.materialization_finished(
            artifact_path,
            timestamp,
            self.version_tracker.current(),
            result,
        )
    }
}
