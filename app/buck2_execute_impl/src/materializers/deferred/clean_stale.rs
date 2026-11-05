/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use buck2_common::file_ops::metadata::FileType;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::liveliness_observer::LivelinessGuard;
use buck2_common::liveliness_observer::LivelinessObserverSync;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::soft_error_or_panic;
use buck2_core::tag_error;
use buck2_data::CleanStaleResultKind;
use buck2_data::CleanStaleStats;
use buck2_data::clean_stale_result::AdaptiveOutcome;
use buck2_data::clean_stale_result::FailurePhase;
use buck2_data::clean_stale_result::Origin;
use buck2_data::clean_stale_result::PolicyMode;
use buck2_data::clean_stale_result::Trigger;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_events::daemon_id::DaemonId;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_events::metadata;
use buck2_execute::execute::blocking::IoRequest;
use buck2_execute::execute::clean_output_paths::cleanup_path;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::CleanStaleArtifactsArgs;
use buck2_execute::materialize::materializer::CleanStaleArtifactsPolicy;
use buck2_fs::fs_util;
use buck2_fs::fs_util::DiskSpaceStats;
use buck2_fs::fs_util::disk_space_stats;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_wrapper_common::invocation_id::TraceId;
use derivative::Derivative;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use jiff::SignedDuration;
use jiff::Timestamp;
use tokio::sync::oneshot::Sender;

use crate::materializers::deferred::ArtifactMaterializationStage;
use crate::materializers::deferred::DeferredMaterializerCommandProcessor;
use crate::materializers::deferred::DeferredMaterializerStats;
use crate::materializers::deferred::LowPriorityMaterializerCommand;
use crate::materializers::deferred::MaterializerSender;
use crate::materializers::deferred::MaterializerSizeStats;
use crate::materializers::deferred::SharedMaterializingError;
use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::ArtifactMaterializationData;
use crate::materializers::deferred::artifact_tree::ArtifactTree;
use crate::materializers::deferred::artifact_tree::CleaningFuture;
use crate::materializers::deferred::artifact_tree::ProcessingFuture;
use crate::materializers::deferred::artifact_tree::UnmaterializationEligibility;
use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibilityReason;
use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibleArtifact;
use crate::materializers::deferred::artifact_tree::UnmaterializationUpload;
use crate::materializers::deferred::artifact_tree::UnmaterializeArtifactsResult;
use crate::materializers::deferred::artifact_tree::Version;
use crate::materializers::deferred::artifact_tree::artifact_metadata_size;
use crate::materializers::deferred::data_tree::DataTreeChildren;
use crate::materializers::deferred::extension::ExtensionCommand;
use crate::materializers::deferred::io_handler::IoHandler;
use crate::materializers::deferred::join_all_existing_futs;
use crate::sqlite::materializer_db::MaterializerStateSqliteDb;

pub const DEFAULT_CLEAN_STALE_TTL_DAYS: u64 = 7;

#[derive(Debug, Clone)]
pub struct CleanStaleArtifactsCommand {
    pub keep_since_time: Timestamp,
    pub dry_run: bool,
    pub tracked_only: bool,
    pub dispatcher: EventDispatcher,
    /// When set, after the normal stale scan also promote the oldest
    /// non-active retained artifacts to stale until projected free disk %
    /// rises above the threshold.
    pub adaptive_low_disk: Option<AdaptiveLowDiskParams>,
    pub unmaterialize_upload: Option<UnmaterializationUploadConfig>,
    /// Buck-out path probed by `disk_space_stats` during the adaptive promotion pass.
    /// `disk_space_stats` accepts any path on the volume, so probing buck-out
    /// measures the volume that holds the artifacts on every platform.
    pub buck_out_abs_path: Option<Arc<AbsPathBuf>>,
    pub trigger: Trigger,
    pub origin: Origin,
    pub policy_mode: PolicyMode,
}

#[derive(Debug, Clone)]
pub struct UnmaterializationUploadConfig {
    pub re_use_case: RemoteExecutorUseCase,
    pub max_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct AdaptiveLowDiskParams {
    pub threshold_percent: f64,
    /// Free disk percentage to recover to by unmaterializing active artifacts.
    pub unmaterialization_threshold_percent: f64,
    /// Retained artifacts last accessed at or after this instant are protected
    /// from adaptive promotion, regardless of free disk pressure.
    pub min_access_time: Timestamp,
    /// Whether non-active intermediate-only artifacts may be promoted even
    /// when they are within the adaptive minimum TTL.
    pub delete_intermediate_within_min_ttl: bool,
    /// Whether active, remote-backed intermediate artifacts may be
    /// unmaterialized as a final escalation step.
    pub unmaterialize_active: bool,
}

impl CleanStaleArtifactsCommand {
    /// The run that `clean --stale` without a duration, or the schedule, asks for: the configured
    /// TTL, or the low-disk policy when the disk is at or under its threshold.
    pub(crate) fn configured(
        config: &CleanStaleConfig,
        buck_out_abs_path: Option<Arc<AbsPathBuf>>,
        dispatcher: EventDispatcher,
        dry_run: bool,
        tracked_only: bool,
        trigger: Trigger,
        origin: Origin,
    ) -> Self {
        let (artifact_ttl, adaptive_low_disk) = resolve_clean_stale_params(
            config.low_disk.as_ref(),
            config.artifact_ttl,
            buck_out_abs_path.as_deref(),
        );
        Self {
            keep_since_time: Timestamp::now()
                .checked_sub(artifact_ttl)
                .unwrap_or(Timestamp::MIN),
            dry_run: dry_run || config.dry_run,
            tracked_only,
            dispatcher,
            adaptive_low_disk,
            unmaterialize_upload: config.unmaterialize_upload.clone(),
            buck_out_abs_path,
            trigger,
            origin,
            policy_mode: match config.low_disk.as_ref().map(|config| &config.mode) {
                None => PolicyMode::ConfiguredTtl,
                Some(LowDiskCleanMode::Fixed(_)) => PolicyMode::ConfiguredFixedTtl,
                Some(LowDiskCleanMode::Adaptive { .. }) => PolicyMode::ConfiguredAdaptive,
            },
        }
    }

    /// The run an explicit `clean --stale=<duration>` asks for; nothing of the configured policy
    /// applies, and the adaptive flags on the command line stand on their own.
    pub(crate) fn explicit(
        keep_since_time: Timestamp,
        dry_run: bool,
        tracked_only: bool,
        adaptive_low_disk_threshold: Option<f64>,
        adaptive_min_ttl: Option<Duration>,
        adaptive_unmaterialize_active: bool,
        origin: Origin,
        dispatcher: EventDispatcher,
        buck_out_abs_path: Option<Arc<AbsPathBuf>>,
    ) -> Self {
        let adaptive_low_disk = adaptive_low_disk_threshold.map(|threshold_percent| {
            let min_ttl = adaptive_min_ttl.unwrap_or_else(|| Duration::from_secs(12 * 60 * 60));
            AdaptiveLowDiskParams {
                threshold_percent,
                unmaterialization_threshold_percent: threshold_percent,
                min_access_time: Timestamp::now()
                    .checked_sub(min_ttl)
                    .unwrap_or(Timestamp::MIN),
                delete_intermediate_within_min_ttl: true,
                unmaterialize_active: adaptive_unmaterialize_active,
            }
        });
        Self {
            keep_since_time,
            dry_run,
            tracked_only,
            dispatcher,
            adaptive_low_disk,
            unmaterialize_upload: None,
            buck_out_abs_path,
            trigger: Trigger::ManualExplicit,
            origin,
            policy_mode: if adaptive_low_disk_threshold.is_some() {
                PolicyMode::ExplicitAdaptive
            } else {
                PolicyMode::ExplicitTtl
            },
        }
    }
}

impl CleanStaleArtifactsCommand {
    /// The run the `Materializer` trait's arguments ask for.
    pub(crate) fn from_args(
        args: CleanStaleArtifactsArgs,
        config: &CleanStaleConfig,
        buck_out_abs_path: Option<Arc<AbsPathBuf>>,
        dispatcher: EventDispatcher,
    ) -> Self {
        match args.policy {
            CleanStaleArtifactsPolicy::Configured => Self::configured(
                config,
                buck_out_abs_path,
                dispatcher,
                args.dry_run,
                args.tracked_only,
                Trigger::ManualConfigured,
                args.origin,
            ),
            CleanStaleArtifactsPolicy::Explicit {
                keep_since_time,
                adaptive_low_disk_threshold,
                adaptive_min_ttl,
                adaptive_unmaterialize_active,
            } => Self::explicit(
                keep_since_time,
                args.dry_run,
                args.tracked_only,
                adaptive_low_disk_threshold,
                adaptive_min_ttl,
                adaptive_unmaterialize_active,
                args.origin,
                dispatcher,
                buck_out_abs_path,
            ),
        }
    }
}

/// Path probed by `disk_space_stats` for the low-disk policy: the buck-out
/// directory itself. Any path on the volume reports that volume's space.
pub(crate) fn disk_space_probe_path(
    fs: &ProjectRoot,
    buck_out: &ProjectRelativePath,
) -> Option<Arc<AbsPathBuf>> {
    Some(Arc::new(fs.resolve(buck_out).into_abs_path_buf()))
}

/// The TTL and the adaptive parameters a run uses under `low_disk`, given the disk as it is now.
pub(crate) fn resolve_clean_stale_params(
    low_disk: Option<&LowDiskCleanConfig>,
    default_ttl: Duration,
    buck_out_abs_path: Option<&AbsPathBuf>,
) -> (Duration, Option<AdaptiveLowDiskParams>) {
    let Some(cfg) = low_disk else {
        return (default_ttl, None);
    };

    match &cfg.mode {
        LowDiskCleanMode::Adaptive {
            min_ttl,
            delete_intermediate_within_min_ttl,
            unmaterialize_active,
            unmaterialization_threshold_percent,
        } => (
            default_ttl,
            Some(AdaptiveLowDiskParams {
                threshold_percent: cfg.threshold_percent,
                // Clamping to MIN keeps everything protected, which is the safe direction
                // for something that deletes artifacts.
                min_access_time: Timestamp::now()
                    .checked_sub(*min_ttl)
                    .unwrap_or(Timestamp::MIN),
                delete_intermediate_within_min_ttl: *delete_intermediate_within_min_ttl,
                unmaterialize_active: *unmaterialize_active,
                unmaterialization_threshold_percent: *unmaterialization_threshold_percent,
            }),
        ),
        LowDiskCleanMode::Fixed(duration) => {
            let Some(buck_out_abs_path) = buck_out_abs_path else {
                return (default_ttl, None);
            };
            let disk_stats = match disk_space_stats(buck_out_abs_path) {
                Ok(stats) => stats,
                Err(e) => {
                    soft_error_or_panic!("disk_space_stats", e);
                    return (default_ttl, None);
                }
            };
            let free_pct = disk_stats.free_space as f64 / disk_stats.total_space as f64 * 100.0;
            if free_pct > cfg.threshold_percent {
                (default_ttl, None)
            } else {
                (*duration, None)
            }
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub(super) struct CleanStaleArtifactsExtensionCommand {
    pub kind: CleanStaleArtifactsExtensionCommandKind,
    #[derivative(Debug = "ignore")]
    pub sender: Sender<BoxFuture<'static, CleanOutcome>>,
}

#[derive(Debug)]
pub(super) enum CleanStaleArtifactsExtensionCommandKind {
    Configured {
        dispatcher: EventDispatcher,
        dry_run: bool,
        tracked_only: bool,
        origin: Origin,
    },
    Explicit(CleanStaleArtifactsCommand),
}

#[derive(Clone)]
pub struct CleanResult {
    kind: CleanStaleResultKind,
    stats: Box<CleanStaleStats>,
    adaptive_outcome: AdaptiveOutcome,
}

impl CleanResult {
    pub(crate) fn new(
        kind: CleanStaleResultKind,
        stats: CleanStaleStats,
        adaptive_outcome: AdaptiveOutcome,
    ) -> Self {
        Self {
            kind,
            stats: Box::new(stats),
            adaptive_outcome,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CleanFailurePhase {
    RootDiscovery,
    Scan,
    StateValidation,
    Invalidation,
    Unmaterialization,
    Clean,
}

#[derive(Clone)]
pub(crate) struct CleanFailure {
    phase: CleanFailurePhase,
    stats: Box<CleanStaleStats>,
    adaptive_outcome: AdaptiveOutcome,
    error: buck2_error::Error,
}

impl CleanFailure {
    pub(crate) fn new(
        phase: CleanFailurePhase,
        stats: CleanStaleStats,
        adaptive_outcome: AdaptiveOutcome,
        error: buck2_error::Error,
    ) -> Self {
        Self {
            phase,
            stats: Box::new(stats),
            adaptive_outcome,
            error,
        }
    }

    pub(crate) fn into_error(self) -> buck2_error::Error {
        self.error
    }
}

/// A failure noted while a run goes on, to be reported with the stats the run ends with.
#[derive(Clone)]
pub(crate) struct PendingCleanFailure {
    pub(crate) phase: CleanFailurePhase,
    pub(crate) error: buck2_error::Error,
}

impl PendingCleanFailure {
    pub(crate) fn with_stats(
        self,
        stats: CleanStaleStats,
        adaptive_outcome: AdaptiveOutcome,
    ) -> CleanFailure {
        CleanFailure::new(self.phase, stats, adaptive_outcome, self.error)
    }
}

pub(crate) type CleanOutcome = Result<CleanResult, CleanFailure>;

enum PendingCleanResult {
    Finished(CleanResult),
    Pending(BoxFuture<'static, CleanOutcome>),
}

impl From<CleanResult> for PendingCleanResult {
    fn from(val: CleanResult) -> Self {
        PendingCleanResult::Finished(val)
    }
}

impl From<CleanResult> for buck2_cli_proto::CleanStaleResponse {
    fn from(result: CleanResult) -> Self {
        let message = match result.kind {
            CleanStaleResultKind::SkippedNoGenDir => Some("Nothing to clean"),
            CleanStaleResultKind::SkippedDeferWriteDisabled => {
                Some("Skipping clean, set buck2.defer_write_actions to use clean --stale")
            }
            CleanStaleResultKind::SkippedSqliteDisabled => {
                Some("Skipping clean, set buck2.sqlite_materializer_state to use clean --stale")
            }
            CleanStaleResultKind::SkippedDryRun => None,
            CleanStaleResultKind::Interrupted => Some("Interrupted"),
            CleanStaleResultKind::Finished => None,
            CleanStaleResultKind::Failed => None,
        };
        Self {
            message: message.map(|m| m.to_owned()),
            stats: Some(*result.stats),
        }
    }
}

/// Samples used bytes on the buck-out filesystem using the same resolved path at the start and end
/// of a clean.
pub(crate) struct CleanDiskSpace {
    path: Option<AbsPathBuf>,
    start: Option<u64>,
}

impl CleanDiskSpace {
    pub(crate) fn new(path: Option<&AbsPathBuf>) -> Self {
        let path = path.and_then(|path| {
            let mut path: &AbsPath = path;
            loop {
                match fs_util::symlink_metadata_if_exists(path).ok()? {
                    Some(_) => {
                        return fs_util::canonicalize(path)
                            .ok()
                            .map(|path| path.into_abs_path_buf());
                    }
                    None => path = path.parent()?,
                }
            }
        });
        let start = path
            .as_ref()
            .and_then(|path| disk_space_stats(path).ok())
            .map(used_disk_space);
        Self { path, start }
    }

    pub(crate) fn record(self, result: &mut CleanOutcome) {
        let end = self
            .path
            .as_ref()
            .and_then(|path| disk_space_stats(path).ok())
            .map(used_disk_space);
        record_used_disk_space(result, self.start, end);
    }
}

fn used_disk_space(stats: DiskSpaceStats) -> u64 {
    stats.total_space.saturating_sub(stats.free_space)
}

fn record_used_disk_space(result: &mut CleanOutcome, start: Option<u64>, end: Option<u64>) {
    let stats = match result {
        Ok(result) => &mut result.stats,
        Err(failure) => &mut failure.stats,
    };
    stats.used_disk_space_bytes_start = start;
    stats.used_disk_space_bytes_end = end;
}

/// The event a run ends with, for either outcome.
pub(crate) fn create_result(
    result: CleanOutcome,
    trace_id: Option<TraceId>,
    daemon_id: &DaemonId,
    total_duration_s: u64,
    trigger: Trigger,
    origin: Origin,
    policy_mode: PolicyMode,
    tracked_only: bool,
) -> buck2_data::CleanStaleResult {
    let (kind, mut stats, error, failure_phase, adaptive_outcome) = match result {
        Ok(result) => (
            result.kind,
            result.stats,
            None,
            FailurePhase::None,
            result.adaptive_outcome,
        ),
        Err(failure) => (
            CleanStaleResultKind::Failed,
            failure.stats,
            Some((&failure.error).into()),
            failure.phase.as_proto(),
            failure.adaptive_outcome,
        ),
    };
    stats.total_duration_s = total_duration_s;
    buck2_data::CleanStaleResult {
        kind: kind.into(),
        stats: Some(*stats),
        metadata: metadata::collect(daemon_id),
        error,
        command_uuid: trace_id.map(|id| id.to_string()),
        trigger: trigger.into(),
        policy_mode: policy_mode.into(),
        failure_phase: failure_phase.into(),
        adaptive_outcome: adaptive_outcome.into(),
        tracked_only,
        origin: origin.into(),
    }
}

impl CleanFailurePhase {
    fn as_proto(self) -> FailurePhase {
        match self {
            Self::RootDiscovery => FailurePhase::RootDiscovery,
            Self::Scan => FailurePhase::Scan,
            Self::StateValidation => FailurePhase::StateValidation,
            Self::Invalidation => FailurePhase::Invalidation,
            Self::Unmaterialization => FailurePhase::Unmaterialization,
            Self::Clean => FailurePhase::Clean,
        }
    }
}

impl<T: IoHandler> ExtensionCommand<T> for CleanStaleArtifactsExtensionCommand {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        let cmd = match self.kind {
            CleanStaleArtifactsExtensionCommandKind::Configured {
                dispatcher,
                dry_run,
                tracked_only,
                origin,
            } => processor.configured_clean_stale_command(
                dispatcher,
                dry_run,
                tracked_only,
                Trigger::ManualConfigured,
                origin,
            ),
            CleanStaleArtifactsExtensionCommandKind::Explicit(mut cmd) => {
                // The sender-side constructor has no `IoHandler`, so it cannot
                // resolve buck-out; the processor's volume is authoritative.
                cmd.buck_out_abs_path = processor.buck_out_abs_path.dupe();
                cmd
            }
        };
        let trace_id = cmd.dispatcher.trace_id().clone();
        let daemon_id = cmd.dispatcher.daemon_id().clone();
        let fut = cmd.create_clean_fut(processor, Some(trace_id), daemon_id);
        let _ignored = self.sender.send(fut);
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub(super) struct FinishUnmaterializationUpload {
    upload: UnmaterializationUpload,
    info: Arc<CasDownloadInfo>,
    #[derivative(Debug = "ignore")]
    cleaning_fut: CleaningFuture,
    #[derivative(Debug = "ignore")]
    sender: Sender<buck2_error::Result<Option<UnmaterializationIneligibleArtifact>>>,
}

impl FinishUnmaterializationUpload {
    pub(super) fn execute<T: IoHandler>(
        self,
        processor: &mut DeferredMaterializerCommandProcessor<T>,
    ) {
        let Self {
            upload,
            info,
            cleaning_fut,
            sender,
        } = self;
        let result = processor
            .sqlite_db
            .as_mut()
            .internal_error("Materializer sqlite state disappeared during unmaterialization")
            .and_then(|sqlite_db| {
                processor.tree.finish_unmaterialization_upload(
                    upload,
                    info,
                    cleaning_fut,
                    sqlite_db,
                    &processor.stats,
                )
            });
        let _ignored = sender.send(result);
    }
}

impl CleanStaleArtifactsCommand {
    pub(super) fn create_clean_fut<T: IoHandler>(
        &self,
        processor: &mut DeferredMaterializerCommandProcessor<T>,
        trace_id: Option<TraceId>,
        daemon_id: DaemonId,
    ) -> BoxFuture<'static, CleanOutcome> {
        let start_time = Instant::now();
        let trigger = self.trigger;
        let origin = self.origin;
        let policy_mode = self.policy_mode;
        let tracked_only = self.tracked_only;
        let disk_space = CleanDiskSpace::new(self.buck_out_abs_path.as_deref());
        let pending_result = self.create_pending_clean_result(processor);
        let dispatcher_dup = self.dispatcher.dupe();
        async move {
            let mut result = match pending_result {
                Ok(res) => match res {
                    PendingCleanResult::Finished(result) => Ok(result),
                    PendingCleanResult::Pending(fut) => fut.await,
                },
                Err(e) => Err(e),
            };
            disk_space.record(&mut result);
            let result_event: buck2_data::CleanStaleResult = create_result(
                result.clone(),
                trace_id,
                &daemon_id,
                (Instant::now() - start_time).as_secs(),
                trigger,
                origin,
                policy_mode,
                tracked_only,
            );
            dispatcher_dup.instant_event(result_event);
            result
        }
        .boxed()
    }

    fn create_pending_clean_result<T: IoHandler>(
        &self,
        processor: &mut DeferredMaterializerCommandProcessor<T>,
    ) -> Result<PendingCleanResult, CleanFailure> {
        let (liveliness_observer, liveliness_guard) = LivelinessGuard::create_sync();
        *processor.command_sender.clean_guard.write() = Some(liveliness_guard);
        let cleaning_version = processor.next_version();
        let stats = initial_stats(processor.stats.sizes());
        let adaptive_outcome = if self.adaptive_low_disk.is_some() {
            AdaptiveOutcome::Unspecified
        } else {
            AdaptiveOutcome::Disabled
        };

        if let Some(sqlite_db) = processor.sqlite_db.as_mut() {
            if !processor.defer_write_actions {
                Ok(CleanResult::new(
                    CleanStaleResultKind::SkippedDeferWriteDisabled,
                    stats,
                    adaptive_outcome,
                )
                .into())
            } else {
                if self
                    .adaptive_low_disk
                    .as_ref()
                    .is_some_and(|params| params.unmaterialize_active)
                    && processor.rematerialization_ttl.is_none()
                {
                    tracing::info!(
                        "Disabling adaptive active-artifact unmaterialization because no TTL refresh interval is available"
                    );
                }
                self.scan_and_create_clean_fut(
                    &mut processor.tree,
                    sqlite_db,
                    &processor.io,
                    &processor.stats,
                    processor.cancellations,
                    liveliness_observer.clone(),
                    processor.rematerialization_ttl,
                    self.unmaterialize_upload.as_ref(),
                    cleaning_version,
                    processor.command_sender.dupe(),
                    stats,
                )
            }
        } else {
            Ok(CleanResult::new(
                CleanStaleResultKind::SkippedSqliteDisabled,
                stats,
                adaptive_outcome,
            )
            .into())
        }
    }

    fn scan_and_create_clean_fut<T: IoHandler>(
        &self,
        tree: &mut ArtifactTree,
        sqlite_db: &mut MaterializerStateSqliteDb,
        io: &Arc<T>,
        materializer_stats: &Arc<DeferredMaterializerStats>,
        cancellations: &'static CancellationContext,
        liveliness_observer: Arc<dyn LivelinessObserverSync>,
        rematerialization_ttl: Option<SignedDuration>,
        upload_config: Option<&UnmaterializationUploadConfig>,
        cleaning_version: Version,
        command_sender: Arc<MaterializerSender<T>>,
        mut stats: CleanStaleStats,
    ) -> Result<PendingCleanResult, CleanFailure> {
        let start_time = Instant::now();
        let mut adaptive_outcome = if self.adaptive_low_disk.is_some() {
            AdaptiveOutcome::Unspecified
        } else {
            AdaptiveOutcome::Disabled
        };
        let mut record_unmaterialization_ineligibility = false;

        let mut artifact_dirs = Vec::new();
        let mut scan_error: Option<PendingCleanFailure> = None;
        for dir_name in &["gen", "gen-anon", "gen-bxl", "art", "art-anon", "art-bxl"] {
            let dir_path = io
                .buck_out_path()
                .join(ProjectRelativePathBuf::unchecked_new(dir_name.to_string()));
            let dir_abs = io.fs().resolve(&dir_path);
            match fs_util::try_exists(&dir_abs) {
                Ok(true) => artifact_dirs.push(dir_path),
                Ok(false) => {}
                Err(error) => {
                    let error = error.context(format!(
                        "Error checking clean-stale artifact directory `{dir_abs}`"
                    ));
                    if scan_error.is_none() {
                        scan_error = Some(PendingCleanFailure {
                            phase: CleanFailurePhase::RootDiscovery,
                            error,
                        });
                    }
                }
            }
        }
        if artifact_dirs.is_empty() {
            return match scan_error {
                Some(error) => {
                    stats.scan_duration_s = (Instant::now() - start_time).as_secs();
                    Err(error.with_stats(stats, adaptive_outcome))
                }
                None => Ok(CleanResult::new(
                    CleanStaleResultKind::SkippedNoGenDir,
                    stats,
                    adaptive_outcome,
                )
                .into()),
            };
        }

        let mut found_paths = Vec::new();
        if self.tracked_only {
            find_stale_tracked_only(tree, self.keep_since_time, &mut found_paths)
        } else {
            for dir_path in &artifact_dirs {
                tracing::trace!(dir = %io.fs().resolve(dir_path), "Scanning");

                let dir_subtree = tree
                    .get_subtree(&mut dir_path.iter())
                    .with_buck_error_context(|| {
                        format!("Found a file where directory was expected: {}", dir_path)
                    });
                let dir_subtree = match dir_subtree {
                    Ok(dir_subtree) => dir_subtree,
                    Err(error) => {
                        stats.scan_failed_directory_count += 1;
                        if scan_error.is_none() {
                            scan_error = Some(PendingCleanFailure {
                                phase: CleanFailurePhase::Scan,
                                error,
                            });
                        }
                        continue;
                    }
                };

                let empty;

                let dir_subtree = match dir_subtree {
                    Some(t) => t,
                    None => {
                        empty = DataTreeChildren::Empty;
                        &empty
                    }
                };

                let outcome = StaleFinder {
                    io: io.dupe(),
                    keep_since_time: self.keep_since_time,
                    rematerialization_deadline: rematerialization_deadline(rematerialization_ttl),
                    found_paths: &mut found_paths,
                    unmaterialize_upload_enabled: self.unmaterialize_upload.is_some(),
                    stats: &mut stats,
                    liveliness_observer: liveliness_observer.clone(),
                }
                .visit_recursively(dir_path.clone(), dir_subtree);
                if let ScanDirectoryOutcome::Failed(error) = outcome {
                    let error = error.context(format!(
                        "Error scanning clean-stale artifact directory `{}`",
                        io.fs().resolve(dir_path)
                    ));
                    stats.scan_failed_directory_count += 1;
                    if scan_error.is_none() {
                        scan_error = Some(PendingCleanFailure {
                            phase: CleanFailurePhase::Scan,
                            error,
                        });
                    }
                }
            }
        };

        if let Some(params) = self.adaptive_low_disk.as_ref() {
            adaptive_outcome = match self.buck_out_abs_path.as_ref() {
                Some(buck_out_abs_path) => match disk_space_stats(&**buck_out_abs_path) {
                    Ok(disk_stats) => {
                        let mut params = params.clone();
                        params.unmaterialize_active &= rematerialization_ttl.is_some();
                        let adaptive = apply_adaptive_low_disk(
                            &mut found_paths,
                            disk_stats.free_space,
                            disk_stats.total_space,
                            &params,
                        );
                        stats.adaptive_free_bytes_before = adaptive.free_bytes_before;
                        stats.adaptive_total_bytes = adaptive.total_bytes;
                        stats.adaptive_bytes_needed = adaptive.bytes_needed;
                        stats.adaptive_shortfall_bytes = adaptive.shortfall_bytes;
                        record_unmaterialization_ineligibility = adaptive.reached_unmaterialization;
                        adaptive.outcome
                    }
                    Err(e) => {
                        soft_error_or_panic!("disk_space_stats", e);
                        AdaptiveOutcome::DiskStatFailure
                    }
                },
                None => AdaptiveOutcome::RootUnavailable,
            };
        }

        stats.record_paths(&found_paths, record_unmaterialization_ineligibility);
        stats.scan_duration_s = (Instant::now() - start_time).as_secs();

        // Log limited number of untracked artifacts to avoid logging spikes if schema changes.
        for (path, file_type) in found_paths
            .iter()
            .filter_map(|x| match x {
                FoundPath::Untracked(path, file_type, _) => Some((path, file_type)),
                _ => None,
            })
            .take(2000)
        {
            self.dispatcher.instant_event(buck2_data::UntrackedFile {
                path: path.to_string(),
                file_type: format!("{file_type:?}"),
            });
        }

        if !liveliness_observer.is_alive_sync() {
            return Ok(PendingCleanResult::Finished(CleanResult::new(
                CleanStaleResultKind::Interrupted,
                stats,
                adaptive_outcome,
            )));
        }

        // If no stale or retained artifact founds, the db should be empty.
        if scan_error.is_none() && stats.stale_artifact_count + stats.retained_artifact_count == 0 {
            // Just need to know if any entries exist, could be a simpler query.
            // Checking the db directly in case tree is somehow not in sync.
            let materializer_state = sqlite_db
                .materializer_state_table()
                .read_materializer_state(io.digest_config())
                .map_err(|error| {
                    CleanFailure::new(
                        CleanFailurePhase::StateValidation,
                        stats,
                        adaptive_outcome,
                        error,
                    )
                })?;

            // Entries in the db should have been found in buck-out, return error and skip cleaning untracked artifacts.
            if !materializer_state.is_empty() {
                let state_error = CleanStaleError {
                    db_size: materializer_state.len(),
                    stats,
                };
                // The error is also returned, so report it quietly.
                let error = tag_error!("clean_stale_error", state_error.into());
                return Err(CleanFailure::new(
                    CleanFailurePhase::StateValidation,
                    stats,
                    adaptive_outcome,
                    error,
                ));
            }
        }

        if self.dry_run {
            match scan_error {
                Some(error) => Err(error.with_stats(stats, adaptive_outcome)),
                None => Ok(PendingCleanResult::Finished(CleanResult::new(
                    CleanStaleResultKind::SkippedDryRun,
                    stats,
                    adaptive_outcome,
                ))),
            }
        } else {
            Ok(PendingCleanResult::Pending(create_clean_fut(
                found_paths,
                stats,
                tree,
                sqlite_db,
                io,
                materializer_stats,
                cancellations,
                liveliness_observer,
                rematerialization_ttl,
                upload_config,
                cleaning_version,
                command_sender,
                scan_error,
                adaptive_outcome,
            )?))
        }
    }
}

#[derive(Debug, Clone, buck2_error::Error)]
#[error("Internal error: materializer state exists (num db entries: {}) but no artifacts were found by clean ({:?}). Not cleaning untracked artifacts.", .db_size, .stats)]
#[buck2(tag = CleanStale)]
pub(crate) struct CleanStaleError {
    pub(crate) db_size: usize,
    pub(crate) stats: buck2_data::CleanStaleStats,
}

pub(crate) fn initial_stats(sizes: MaterializerSizeStats) -> CleanStaleStats {
    CleanStaleStats {
        materialized_final_output_bytes_before: sizes.final_output,
        materialized_final_output_bytes_after: sizes.final_output,
        materialized_intermediate_only_bytes_before: sizes.intermediate_only,
        materialized_intermediate_only_bytes_after: sizes.intermediate_only,
        ..Default::default()
    }
}

pub(crate) trait CleanStaleStatsExt {
    fn record_materialized_sizes_after(&mut self, sizes: MaterializerSizeStats);

    fn record_paths(&mut self, paths: &[FoundPath], record_unmaterialization_ineligibility: bool);

    fn record_unmaterialization_result(&mut self, result: &UnmaterializeArtifactsResult) -> u64;

    fn record_unmaterialization_ineligibility_reason(
        &mut self,
        reason: UnmaterializationIneligibilityReason,
        size: u64,
    );

    fn record_scan_unreadable(&mut self);

    /// Records how one path's cleanup ended. A failure that should fail the run comes back.
    fn record_clean_outcome(
        &mut self,
        outcome: CleanPathOutcome,
        origin: CleanupOrigin,
    ) -> Option<PendingCleanFailure>;
}

impl CleanStaleStatsExt for CleanStaleStats {
    fn record_materialized_sizes_after(&mut self, sizes: MaterializerSizeStats) {
        self.materialized_final_output_bytes_after = sizes.final_output;
        self.materialized_intermediate_only_bytes_after = sizes.intermediate_only;
    }

    fn record_paths(&mut self, paths: &[FoundPath], record_unmaterialization_ineligibility: bool) {
        for path in paths {
            match path {
                FoundPath::Untracked(_, _, size) => {
                    self.untracked_artifact_count += 1;
                    self.untracked_bytes += *size;
                }
                FoundPath::Tracked {
                    size,
                    state: TrackedState::Stale(StaleOrigin::Ttl),
                    ..
                } => {
                    self.stale_artifact_count += 1;
                    self.stale_bytes += *size;
                    self.ttl_stale_artifact_count += 1;
                    self.ttl_stale_bytes += *size;
                }
                FoundPath::Tracked {
                    size,
                    state: TrackedState::Stale(StaleOrigin::Adaptive),
                    ..
                } => {
                    self.stale_artifact_count += 1;
                    self.stale_bytes += *size;
                    self.adaptive_stale_artifact_count += 1;
                    self.adaptive_stale_bytes += *size;
                }
                FoundPath::Tracked {
                    size,
                    state: TrackedState::Retained { .. },
                    ..
                } => {
                    self.retained_artifact_count += 1;
                    self.retained_bytes += *size;
                    self.retained_inactive_artifact_count += 1;
                    self.retained_inactive_bytes += *size;
                }
                FoundPath::Tracked {
                    size,
                    state:
                        TrackedState::ActiveRetained {
                            unmaterialization_eligibility,
                            ..
                        },
                    ..
                } => {
                    self.retained_artifact_count += 1;
                    self.retained_bytes += *size;
                    self.retained_active_artifact_count += 1;
                    self.retained_active_bytes += *size;
                    if record_unmaterialization_ineligibility
                        && let UnmaterializationEligibility::Ineligible(reason) =
                            unmaterialization_eligibility
                    {
                        self.adaptive_unmaterialization_statically_ineligible_artifact_count += 1;
                        self.adaptive_unmaterialization_statically_ineligible_bytes += *size;
                        self.record_unmaterialization_ineligibility_reason(*reason, *size);
                    }
                }
                FoundPath::Tracked {
                    size,
                    state: TrackedState::Unmaterialize,
                    ..
                } => {
                    self.retained_artifact_count += 1;
                    self.retained_bytes += *size;
                    self.adaptive_unmaterialization_selected_artifact_count += 1;
                    self.adaptive_unmaterialization_selected_bytes += *size;
                }
            }
        }
    }

    fn record_unmaterialization_result(&mut self, result: &UnmaterializeArtifactsResult) -> u64 {
        let unmaterialized_bytes = result
            .unmaterialized
            .iter()
            .map(|(_, size)| *size)
            .sum::<u64>();
        for artifact in &result.ineligible {
            self.unmaterialization_ineligible_artifact_count += 1;
            self.unmaterialization_ineligible_bytes += artifact.size;
            self.record_unmaterialization_ineligibility_reason(artifact.reason, artifact.size);
        }
        unmaterialized_bytes
    }

    fn record_unmaterialization_ineligibility_reason(
        &mut self,
        reason: UnmaterializationIneligibilityReason,
        size: u64,
    ) {
        let (count, bytes) = match reason {
            UnmaterializationIneligibilityReason::FinalOutput => (
                &mut self.unmaterialization_ineligible_final_output_artifact_count,
                &mut self.unmaterialization_ineligible_final_output_bytes,
            ),
            UnmaterializationIneligibilityReason::NoRematerializationMethod => (
                &mut self.unmaterialization_ineligible_no_rematerialization_method_artifact_count,
                &mut self.unmaterialization_ineligible_no_rematerialization_method_bytes,
            ),
            UnmaterializationIneligibilityReason::UploadTooLarge => {
                self.unmaterialization_upload_skipped_oversize_artifact_count += 1;
                self.unmaterialization_upload_skipped_oversize_bytes += size;
                return;
            }
            UnmaterializationIneligibilityReason::RemoteTtlTooShort => (
                &mut self.unmaterialization_ineligible_remote_ttl_too_short_artifact_count,
                &mut self.unmaterialization_ineligible_remote_ttl_too_short_bytes,
            ),
            UnmaterializationIneligibilityReason::Processing => (
                &mut self.unmaterialization_ineligible_processing_artifact_count,
                &mut self.unmaterialization_ineligible_processing_bytes,
            ),
            UnmaterializationIneligibilityReason::StateChanged => (
                &mut self.unmaterialization_ineligible_state_changed_artifact_count,
                &mut self.unmaterialization_ineligible_state_changed_bytes,
            ),
        };
        *count += 1;
        *bytes += size;
    }

    fn record_scan_unreadable(&mut self) {
        self.scan_unreadable_count += 1;
        self.skipped_unreadable_count += 1;
    }

    fn record_clean_outcome(
        &mut self,
        outcome: CleanPathOutcome,
        origin: CleanupOrigin,
    ) -> Option<PendingCleanFailure> {
        match outcome {
            CleanPathOutcome::Cleaned(size) => {
                self.cleaned_artifact_count += 1;
                self.cleaned_bytes += size;
                match origin {
                    CleanupOrigin::Untracked => {
                        self.cleaned_untracked_artifact_count += 1;
                        self.cleaned_untracked_bytes += size;
                    }
                    CleanupOrigin::Stale => {
                        self.cleaned_stale_artifact_count += 1;
                        self.cleaned_stale_bytes += size;
                    }
                    CleanupOrigin::Unmaterialized | CleanupOrigin::Uploaded => {
                        self.unmaterialized_only_artifact_count += 1;
                        self.unmaterialized_only_bytes += size;
                        if origin == CleanupOrigin::Uploaded {
                            self.unmaterialization_upload_succeeded_artifact_count += 1;
                            self.unmaterialization_upload_succeeded_bytes += size;
                        }
                    }
                }
                None
            }
            CleanPathOutcome::UploadFailed { size, .. } => {
                self.unmaterialization_upload_failed_artifact_count += 1;
                self.unmaterialization_upload_failed_bytes += size;
                None
            }
            CleanPathOutcome::UnmaterializationIneligible(artifact) => {
                self.unmaterialization_ineligible_artifact_count += 1;
                self.unmaterialization_ineligible_bytes += artifact.size;
                self.record_unmaterialization_ineligibility_reason(artifact.reason, artifact.size);
                None
            }
            CleanPathOutcome::SkippedPermissionDenied(size) => {
                self.skipped_unreadable_count += 1;
                self.delete_permission_denied_artifact_count += 1;
                self.delete_permission_denied_bytes += size;
                None
            }
            CleanPathOutcome::Interrupted(size) => {
                self.delete_interrupted_artifact_count += 1;
                self.delete_interrupted_bytes += size;
                None
            }
            CleanPathOutcome::Failed { size, error } => {
                self.delete_failed_artifact_count += 1;
                self.delete_failed_bytes += size;
                Some(PendingCleanFailure {
                    phase: CleanFailurePhase::Clean,
                    error,
                })
            }
        }
    }
}

fn create_clean_fut<T: IoHandler>(
    found_paths: Vec<FoundPath>,
    mut stats: CleanStaleStats,
    tree: &mut ArtifactTree,
    sqlite_db: &mut MaterializerStateSqliteDb,
    io: &Arc<T>,
    materializer_stats: &Arc<DeferredMaterializerStats>,
    cancellations: &'static CancellationContext,
    liveliness_observer: Arc<dyn LivelinessObserverSync>,
    rematerialization_ttl: Option<SignedDuration>,
    upload_config: Option<&UnmaterializationUploadConfig>,
    cleaning_version: Version,
    command_sender: Arc<MaterializerSender<T>>,
    scan_error: Option<PendingCleanFailure>,
    mut adaptive_outcome: AdaptiveOutcome,
) -> Result<BoxFuture<'static, CleanOutcome>, CleanFailure> {
    let io = io.dupe();

    let paths_to_invalidate: Vec<ProjectRelativePathBuf> = found_paths
        .iter()
        .filter_map(|x| match x {
            FoundPath::Tracked {
                path,
                state: TrackedState::Stale(_),
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .collect();

    let existing_clean_futs = tree
        .invalidate_paths_and_collect_futures(
            paths_to_invalidate,
            Some(sqlite_db),
            materializer_stats,
        )
        .map_err(|error| {
            let mut failure_stats = stats;
            failure_stats.record_materialized_sizes_after(materializer_stats.sizes());
            CleanFailure::new(
                CleanFailurePhase::Invalidation,
                failure_stats,
                adaptive_outcome,
                error,
            )
        })?;
    let mut existing_materialization_futs = vec![];
    for data in tree.iter_without_paths() {
        if let Some(active) = data.processing.active_ref()
            && let ProcessingFuture::Materializing(future) = &active.future
        {
            existing_materialization_futs.push(future.clone());
        }
    }
    let wait_for_existing_futs = async move {
        join_all_existing_futs(existing_clean_futs).await?;
        for fut in existing_materialization_futs {
            fut.await.ok();
        }
        Ok::<(), buck2_error::Error>(())
    }
    .boxed()
    .shared();

    let paths_to_unmaterialize = found_paths
        .iter()
        .filter_map(|path| match path {
            FoundPath::Tracked {
                path,
                size,
                state: TrackedState::Unmaterialize,
            } => Some((path.clone(), *size)),
            _ => None,
        })
        .collect();
    let rematerialization_deadline = rematerialization_deadline(rematerialization_ttl);
    let unmaterialization = tree
        .unmaterialize_artifacts(
            paths_to_unmaterialize,
            rematerialization_deadline,
            upload_config.map(|config| config.max_bytes),
            sqlite_db,
            materializer_stats,
        )
        .map_err(|error| {
            let mut failure_stats = stats;
            failure_stats.record_materialized_sizes_after(materializer_stats.sizes());
            CleanFailure::new(
                CleanFailurePhase::Unmaterialization,
                failure_stats,
                adaptive_outcome,
                error,
            )
        })?;
    let unmaterialized_bytes = stats.record_unmaterialization_result(&unmaterialization);
    let uploads = unmaterialization.uploads;
    if matches!(
        adaptive_outcome,
        AdaptiveOutcome::SatisfiedByUnmaterialization | AdaptiveOutcome::InsufficientCandidates
    ) {
        stats.adaptive_shortfall_bytes = stats.adaptive_bytes_needed.saturating_sub(
            stats
                .adaptive_stale_bytes
                .saturating_add(unmaterialized_bytes),
        );
        adaptive_outcome = if stats.adaptive_shortfall_bytes == 0 {
            AdaptiveOutcome::SatisfiedByUnmaterialization
        } else {
            AdaptiveOutcome::InsufficientCandidates
        };
    }

    let mut paths_to_clean: Vec<CleanupPath> = found_paths
        .into_iter()
        .filter_map(|path| match path {
            FoundPath::Untracked(path, _, size) => Some(CleanupPath {
                path,
                size,
                origin: CleanupOrigin::Untracked,
            }),
            FoundPath::Tracked {
                path,
                size,
                state: TrackedState::Stale(_),
            } => Some(CleanupPath {
                path,
                size,
                origin: CleanupOrigin::Stale,
            }),
            _ => None,
        })
        .collect();
    paths_to_clean.extend(
        unmaterialization
            .unmaterialized
            .into_iter()
            .map(|(path, size)| CleanupPath {
                path,
                size,
                origin: CleanupOrigin::Unmaterialized,
            }),
    );

    let mut clean_futs = Vec::with_capacity(paths_to_clean.len());
    for CleanupPath { path, size, origin } in paths_to_clean {
        let wait_for_existing_futs = wait_for_existing_futs.clone();
        let io = io.dupe();
        let liveliness_observer = liveliness_observer.dupe();
        let path_to_clean = path.clone();
        let clean_fut = async move {
            match wait_for_existing_futs.await {
                Ok(()) => {
                    clean_artifact(path_to_clean, size, cancellations, &io, liveliness_observer)
                        .await
                }
                Err(error) => CleanPathOutcome::Failed { size, error },
            }
        }
        .boxed()
        .shared();

        if origin == CleanupOrigin::Unmaterialized {
            let cleaning_fut = {
                let clean_fut = clean_fut.clone();
                async move {
                    match clean_fut.await {
                        CleanPathOutcome::Failed { error, .. } => Err(error),
                        _ => Ok(()),
                    }
                }
                .boxed()
                .shared()
            };
            tree.attach_unmaterialization_future(&path, cleaning_fut, cleaning_version)
                .map_err(|error| {
                    let mut failure_stats = stats;
                    failure_stats.record_materialized_sizes_after(materializer_stats.sizes());
                    CleanFailure::new(
                        CleanFailurePhase::Unmaterialization,
                        failure_stats,
                        adaptive_outcome,
                        error,
                    )
                })?;
        }

        clean_futs.push((clean_fut, origin));
    }

    if let Some(upload_config) = upload_config.cloned() {
        for upload in uploads {
            stats.unmaterialization_upload_attempted_artifact_count += 1;
            stats.unmaterialization_upload_attempted_bytes += upload.size;
            let io = io.dupe();
            let command_sender = command_sender.dupe();
            let liveliness_observer = liveliness_observer.dupe();
            let wait_for_existing_futs = wait_for_existing_futs.clone();
            let path = upload.path.clone();
            let size = upload.size;
            let info = Arc::new(CasDownloadInfo::new_uploaded(upload_config.re_use_case));
            let clean_fut = async move {
                if let Err(error) = wait_for_existing_futs.await {
                    return CleanPathOutcome::UploadFailed { size, error };
                }
                if !liveliness_observer.is_alive_sync() {
                    return CleanPathOutcome::Interrupted(size);
                }
                if let Err(error) = io
                    .upload_materialized_artifact(path.clone(), upload.entry.dupe(), info.dupe())
                    .await
                {
                    return CleanPathOutcome::UploadFailed { size, error };
                }

                let (completion_sender, completion_receiver) = tokio::sync::oneshot::channel();
                let attached_fut = async move {
                    let _ignored = completion_receiver.await;
                    Ok(())
                }
                .boxed()
                .shared();
                let (finish_sender, finish_receiver) = tokio::sync::oneshot::channel();
                if !command_sender.send_low_priority(
                    LowPriorityMaterializerCommand::FinishUnmaterializationUpload {
                        command: FinishUnmaterializationUpload {
                            upload,
                            info,
                            cleaning_fut: attached_fut,
                            sender: finish_sender,
                        },
                        dispatcher: get_dispatcher_opt(),
                    },
                ) {
                    return CleanPathOutcome::Failed {
                        size,
                        error: buck2_error!(
                            ErrorTag::Tier0,
                            "Materializer command processor stopped during unmaterialization"
                        ),
                    };
                }
                match finish_receiver.await {
                    Ok(Ok(None)) => {}
                    Ok(Ok(Some(ineligible))) => {
                        let _ignored = completion_sender.send(());
                        return CleanPathOutcome::UnmaterializationIneligible(ineligible);
                    }
                    Ok(Err(error)) => return CleanPathOutcome::Failed { size, error },
                    Err(error) => {
                        return CleanPathOutcome::Failed {
                            size,
                            error: error.into(),
                        };
                    }
                }

                let cleaned =
                    clean_artifact(path.clone(), size, cancellations, &io, liveliness_observer)
                        .await;
                let result = match &cleaned {
                    CleanPathOutcome::Failed { error, .. } => {
                        Err(SharedMaterializingError::Error(error.dupe()))
                    }
                    _ => Ok(()),
                };
                let _ignored = command_sender.send_low_priority(
                    LowPriorityMaterializerCommand::CleanupFinished {
                        path,
                        version: cleaning_version,
                        result,
                        dispatcher: get_dispatcher_opt(),
                    },
                );
                let _ignored = completion_sender.send(());
                cleaned
            }
            .boxed()
            .shared();
            clean_futs.push((clean_fut, CleanupOrigin::Uploaded));
        }
    }

    let materializer_stats = materializer_stats.dupe();
    let fut = async move {
        let start_time = Instant::now();
        let results = buck2_util::future::join_all(
            clean_futs
                .into_iter()
                .map(|(clean_fut, origin)| clean_fut.map(move |cleaned| (cleaned, origin)))
                .collect::<Vec<_>>()
                .into_iter(),
        )
        .await;
        stats.record_materialized_sizes_after(materializer_stats.sizes());

        let mut first_error = scan_error;
        for (cleaned, origin) in results {
            let failure = stats.record_clean_outcome(cleaned, origin);
            if first_error.is_none() {
                first_error = failure;
            }
        }
        stats.clean_duration_s = (Instant::now() - start_time).as_secs();
        let kind = if !liveliness_observer.is_alive().await {
            CleanStaleResultKind::Interrupted
        } else {
            CleanStaleResultKind::Finished
        };
        match first_error {
            Some(error) => Err(error.with_stats(stats, adaptive_outcome)),
            None => Ok(CleanResult::new(kind, stats, adaptive_outcome)),
        }
    };
    Ok(fut.boxed())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CleanupOrigin {
    Untracked,
    Stale,
    Unmaterialized,
    Uploaded,
}

struct CleanupPath {
    path: ProjectRelativePathBuf,
    size: u64,
    origin: CleanupOrigin,
}

/// The latest a CAS copy may expire for an artifact to count as re-fetchable: the TTL refresher
/// has until then to extend it. Nothing is re-fetchable when there is no refresher.
pub(crate) fn rematerialization_deadline(ttl: Option<SignedDuration>) -> Timestamp {
    ttl.and_then(|ttl| Timestamp::now().checked_add(ttl).ok())
        .unwrap_or(Timestamp::MAX)
}

#[derive(Clone)]
pub(crate) enum CleanPathOutcome {
    Cleaned(u64),
    Interrupted(u64),
    SkippedPermissionDenied(u64),
    UploadFailed {
        size: u64,
        error: buck2_error::Error,
    },
    UnmaterializationIneligible(UnmaterializationIneligibleArtifact),
    Failed {
        size: u64,
        error: buck2_error::Error,
    },
}

pub(crate) async fn clean_artifact<T: IoHandler>(
    path: ProjectRelativePathBuf,
    size: u64,
    cancellations: &'static CancellationContext,
    io: &Arc<T>,
    liveliness_observer: Arc<dyn LivelinessObserverSync>,
) -> CleanPathOutcome {
    match io
        .clean_invalidated_path(
            CleanInvalidatedPathRequest {
                path,
                buck_out_path: io.buck_out_path().clone(),
                liveliness_observer: liveliness_observer.dupe(),
            },
            cancellations,
        )
        .await
    {
        Ok(()) => CleanPathOutcome::Cleaned(size),
        Err(e) => {
            if e.has_tag(ErrorTag::CleanInterrupt) {
                CleanPathOutcome::Interrupted(size)
            } else if e.has_tag(ErrorTag::IoPermissionDenied) {
                // A skipped path may still hold bytes for an entry the tree/db
                // already recorded as cleaned or unmaterialized (both are
                // committed before deletion) — the same state a failed clean
                // left behind. This is safe: materialization always cleans its
                // destination first, so a later rematerialization surfaces the
                // permission error at the point of use.
                CleanPathOutcome::SkippedPermissionDenied(size)
            } else {
                CleanPathOutcome::Failed { size, error: e }
            }
        }
    }
}

pub struct CleanInvalidatedPathRequest {
    path: ProjectRelativePathBuf,
    /// Ancestors of `path` below this root must be real directories for the deletion to run.
    buck_out_path: ProjectRelativePathBuf,
    pub(crate) liveliness_observer: Arc<dyn LivelinessObserverSync>,
}

impl CleanInvalidatedPathRequest {
    /// The first ancestor of `path` below `buck_out_path` that exists on disk but is not a
    /// directory (a file or a symlink). `cleanup_path` would delete such an entry, and would
    /// delete `path` inside a symlink's target, which may lie outside `buck-out`. Materialization
    /// has checked that nothing overlaps before it cleans a path, but a stale path comes from the
    /// materializer state alone, which can disagree with the disk.
    fn ancestor_that_is_not_a_directory(
        &self,
        project_fs: &ProjectRoot,
    ) -> buck2_error::Result<Option<ProjectRelativePathBuf>> {
        let Some(below_buck_out) = self.path.strip_prefix_opt(&self.buck_out_path) else {
            return Ok(None);
        };
        let mut ancestor = self.buck_out_path.clone();
        for component in below_buck_out.parent().into_iter().flat_map(|p| p.iter()) {
            ancestor.push(component);
            if let Some(metadata) =
                fs_util::symlink_metadata_if_exists(project_fs.resolve(&ancestor))?
                && !metadata.is_dir()
            {
                return Ok(Some(ancestor));
            }
        }
        Ok(None)
    }
}

impl IoRequest for CleanInvalidatedPathRequest {
    fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()> {
        if !self.liveliness_observer.is_alive_sync() {
            return Err(buck2_error!(ErrorTag::CleanInterrupt, "Interrupt"));
        }
        if let Some(ancestor) = self.ancestor_that_is_not_a_directory(project_fs)? {
            return Err(buck2_error!(
                ErrorTag::IoNotADirectory,
                "Not deleting `{}`: `{}` is not a directory on disk",
                self.path,
                ancestor
            ));
        }
        cleanup_path(project_fs, &self.path)?;
        Ok(())
    }
}

/// Deletes everything under the scratch roots (`tmp`, `tmp-anon`, `tmp-bxl`): the local
/// actions' `BUCK_SCRATCH_PATH`s. Scratch liveness is command-scoped — a scratch dir is only
/// in use while its action runs, and the local executor wipes it before the same action's
/// next run — so with no command active, everything under the roots is dead. The daemon
/// sends this when it goes idle after a command.
///
/// The sweep holds the same clean guard as `clean --stale`: the moment any other
/// materializer command arrives, deletion stops between paths. Entries below a root that the
/// scan cannot read are counted, like in the artifact scan, but still deleted: the deletion adds
/// owner permissions to the directories it removes. Entries that stay undeletable (permission
/// denied, e.g. owned by another user) are counted and skipped.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct CleanScratchExtensionCommand {
    #[derivative(Debug = "ignore")]
    pub sender: Sender<BoxFuture<'static, buck2_error::Result<CleanResult>>>,
}

impl<T: IoHandler> ExtensionCommand<T> for CleanScratchExtensionCommand {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        let (liveliness_observer, liveliness_guard) = LivelinessGuard::create_sync();
        *processor.command_sender.clean_guard.write() = Some(liveliness_guard);
        let io = processor.io.dupe();
        let cancellations = processor.cancellations;
        let fut =
            async move { scratch_sweep(&io, cancellations, liveliness_observer).await }.boxed();
        let _ignored = self.sender.send(fut);
    }
}

pub(crate) async fn scratch_sweep<T: IoHandler>(
    io: &Arc<T>,
    cancellations: &'static CancellationContext,
    liveliness_observer: Arc<dyn LivelinessObserverSync>,
) -> buck2_error::Result<CleanResult> {
    let scan_start = Instant::now();
    let mut stats = CleanStaleStats::default();
    let mut found = Vec::new();
    for dir_name in &["tmp", "tmp-anon", "tmp-bxl"] {
        let dir_path = io
            .buck_out_path()
            .join(ProjectRelativePathBuf::unchecked_new(dir_name.to_string()));
        let abs_dir = io.fs().resolve(&dir_path);
        let read_dir = match io.read_dir(&abs_dir) {
            Ok(read_dir) => read_dir,
            // The root may not exist yet, or may vanish between listing and here.
            Err(e) if e.has_tag(ErrorTag::IoNotFound) => continue,
            Err(e) if e.has_tag(ErrorTag::IoPermissionDenied) => {
                stats.record_scan_unreadable();
                continue;
            }
            Err(e) => return Err(e),
        };
        for child in read_dir {
            if !liveliness_observer.is_alive_sync() {
                return Ok(CleanResult::new(
                    CleanStaleResultKind::Interrupted,
                    stats,
                    AdaptiveOutcome::Disabled,
                ));
            }
            let child = match child {
                Ok(child) => child,
                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                    stats.record_scan_unreadable();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let file_name = child.file_name();
            let Some(file_name) = file_name.to_str().and_then(|f| FileName::new(f).ok()) else {
                stats.scan_invalid_filename_count += 1;
                continue;
            };
            // Sizes are for reporting only; `get_size` skips (and counts) what
            // it cannot read rather than failing.
            let size = get_size(&child.path(), &mut stats)?;
            stats.untracked_artifact_count += 1;
            stats.untracked_bytes += size;
            found.push((dir_path.join(file_name), size));
        }
    }
    stats.scan_duration_s = (Instant::now() - scan_start).as_secs();

    let clean_start = Instant::now();
    let mut kind = CleanStaleResultKind::Finished;
    for (path, size) in found {
        match clean_artifact(path, size, cancellations, io, liveliness_observer.dupe()).await {
            CleanPathOutcome::Cleaned(size) => {
                stats.cleaned_artifact_count += 1;
                stats.cleaned_bytes += size;
                stats.cleaned_untracked_artifact_count += 1;
                stats.cleaned_untracked_bytes += size;
            }
            CleanPathOutcome::SkippedPermissionDenied(size) => {
                stats.skipped_unreadable_count += 1;
                stats.delete_permission_denied_artifact_count += 1;
                stats.delete_permission_denied_bytes += size;
            }
            CleanPathOutcome::Interrupted(size) => {
                stats.delete_interrupted_artifact_count += 1;
                stats.delete_interrupted_bytes += size;
                kind = CleanStaleResultKind::Interrupted;
                break;
            }
            CleanPathOutcome::Failed { error, .. }
            | CleanPathOutcome::UploadFailed { error, .. } => return Err(error),
            CleanPathOutcome::UnmaterializationIneligible(_) => {
                unreachable!("upload-backed outcomes are not produced by untracked cleanup")
            }
        }
    }
    stats.clean_duration_s = (Instant::now() - clean_start).as_secs();
    Ok(CleanResult::new(kind, stats, AdaptiveOutcome::Disabled))
}

/// Get file size or directory size, without following symlinks.
/// Entries that cannot be read or statted due to insufficient permissions are
/// skipped (and counted in `stats`) rather than treated as
/// errors, so the result may undercount.
pub fn get_size(path: &AbsNormPath, stats: &mut CleanStaleStats) -> buck2_error::Result<u64> {
    // We check the type with `symlink_metadata` because `Path::is_dir` follows symlinks.
    // With `Path::is_dir`, a symlink in buck-out to a host directory such as `/run` makes
    // the walk read files outside buck-out.
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            stats.record_scan_unreadable();
            return Ok(0);
        }
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    let read_dir = match fs_util::read_dir(path) {
        Ok(read_dir) => read_dir,
        Err(e) if e.io_error_kind() == Some(ErrorKind::PermissionDenied) => {
            stats.record_scan_unreadable();
            return Ok(0);
        }
        Err(e) => return Err(e.categorize_tagged(ErrorTag::CleanStale)),
    };
    let mut result = 0;
    for entry in read_dir {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                stats.record_scan_unreadable();
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        result += get_size(&entry.path(), stats)?;
    }
    Ok(result)
}

struct StaleFinder<'a, T: IoHandler> {
    io: Arc<T>,
    keep_since_time: Timestamp,
    rematerialization_deadline: Timestamp,
    found_paths: &'a mut Vec<FoundPath>,
    unmaterialize_upload_enabled: bool,
    stats: &'a mut CleanStaleStats,
    liveliness_observer: Arc<dyn LivelinessObserverSync>,
}

enum ScanDirectoryOutcome {
    Complete,
    Failed(buck2_error::Error),
}

#[derive(Clone)]
pub(crate) enum FoundPath {
    /// Will be deleted on disk.
    Untracked(ProjectRelativePathBuf, FileType, u64),
    /// Tracked by the materializer. `state` decides what (if anything) we do with it.
    Tracked {
        path: ProjectRelativePathBuf,
        size: u64,
        state: TrackedState,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum TrackedState {
    /// Will be invalidated in the materializer.
    Stale(StaleOrigin),
    /// Materialized, not-stale, and not active. Eligible to be promoted
    /// to `Stale` by `apply_adaptive_low_disk`.
    Retained {
        last_access_time: Timestamp,
        classification: ArtifactClassification,
    },
    /// Materialized and active. Eligible for unmaterialization only when the
    /// stored method is remote-backed and the artifact is intermediate-only.
    ActiveRetained {
        last_access_time: Timestamp,
        classification: ArtifactClassification,
        unmaterialization_eligibility: UnmaterializationEligibility,
    },
    /// Will be returned to the declared state and deleted on disk.
    Unmaterialize,
}

#[derive(Clone, Copy)]
pub(crate) enum StaleOrigin {
    Ttl,
    Adaptive,
}

impl<T: IoHandler> StaleFinder<'_, T> {
    /// Start from `path` and `subtree` and visit everything below.
    fn visit_recursively(
        &mut self,
        path: ProjectRelativePathBuf,
        subtree: &DataTreeChildren<FileNameBuf, ArtifactTree>,
    ) -> ScanDirectoryOutcome {
        let mut queue = vec![(path, subtree)];

        while let Some((path, tree)) = queue.pop() {
            if !self.liveliness_observer.is_alive_sync() {
                break;
            }
            if let Err(error) = self.visit(&path, tree, &mut queue) {
                return ScanDirectoryOutcome::Failed(error);
            }
        }

        ScanDirectoryOutcome::Complete
    }

    /// Visit one directory.
    fn visit<'t>(
        &mut self,
        path: &ProjectRelativePath,
        subtree: &'t DataTreeChildren<FileNameBuf, ArtifactTree>,
        queue: &mut Vec<(
            ProjectRelativePathBuf,
            &'t DataTreeChildren<FileNameBuf, ArtifactTree>,
        )>,
    ) -> buck2_error::Result<()> {
        let abs_path = self.io.fs().resolve(path);

        let read_dir = match self.io.read_dir(&abs_path) {
            Ok(read_dir) => read_dir,
            Err(e) if e.has_tag(ErrorTag::IoPermissionDenied) => {
                // Tracked entries beneath the skipped directory are deliberately
                // left out of `found_paths`: they cannot be deleted through an
                // unreadable parent anyway, and leaving their tree/db state
                // untouched means a later scan (after permissions are repaired)
                // sees them again with metadata intact.
                self.stats.record_scan_unreadable();
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        for child in read_dir {
            let child = match child {
                Ok(child) => child,
                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                    self.stats.record_scan_unreadable();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let file_name = child.file_name();
            let file_name = file_name.to_str().and_then(|f| FileName::new(f).ok());

            let file_name = match file_name {
                Some(file_name) => file_name,
                None => {
                    // If the file name is invalid, then it can't be tracked by the materializer.
                    // We should ideally delete this, but currently we don't support doing that.
                    self.stats.scan_invalid_filename_count += 1;
                    continue;
                }
            };

            let path = path.join(file_name);

            // `file_type()` falls back to an `lstat` on filesystems that don't
            // report `d_type` (NFS, FUSE), which fails without search
            // permission on the parent even though `read_dir` succeeded.
            let file_type = match child.file_type() {
                Ok(file_type) => FileType::from(file_type),
                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                    self.stats.record_scan_unreadable();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let subtree = match subtree.get(file_name) {
                Some(subtree) => subtree,
                None => {
                    // This path is not tracked by the materializer, we can delete it.
                    tracing::trace!(path = %path, file_type = ?file_type, "marking as untracked");
                    self.found_paths.push(FoundPath::Untracked(
                        path,
                        file_type,
                        get_size(&child.path(), self.stats)?,
                    ));
                    continue;
                }
            };

            match subtree {
                ArtifactTree::Tree(subtree) if file_type.is_dir() => {
                    queue.push((path, subtree));
                }
                ArtifactTree::Data(box ArtifactMaterializationData {
                    stage:
                        ArtifactMaterializationStage::Materialized {
                            active: false,
                            last_access_time,
                            metadata,
                            ..
                        },
                    ..
                }) if *last_access_time < self.keep_since_time => {
                    // This is something we can invalidate.
                    tracing::trace!(path = %path, file_type = ?file_type, "marking as stale");
                    self.found_paths.push(FoundPath::Tracked {
                        path,
                        size: artifact_metadata_size(metadata),
                        state: TrackedState::Stale(StaleOrigin::Ttl),
                    });
                }
                ArtifactTree::Data(box ArtifactMaterializationData {
                    classification,
                    stage:
                        ArtifactMaterializationStage::Materialized {
                            active: false,
                            last_access_time,
                            metadata,
                            ..
                        },
                    ..
                }) => {
                    tracing::trace!(path = %path, file_type = ?file_type, "marking as retained");
                    self.found_paths.push(FoundPath::Tracked {
                        path,
                        size: artifact_metadata_size(metadata),
                        state: TrackedState::Retained {
                            last_access_time: *last_access_time,
                            classification: *classification,
                        },
                    });
                }
                ArtifactTree::Data(box ArtifactMaterializationData {
                    classification,
                    stage:
                        ArtifactMaterializationStage::Materialized {
                            metadata,
                            last_access_time,
                            rematerialization_method,
                            ..
                        },
                    ..
                }) => {
                    tracing::trace!(path = %path, file_type = ?file_type, "marking as active retained");
                    let unmaterialization_eligibility =
                        if *classification == ArtifactClassification::FinalOutput {
                            UnmaterializationEligibility::Ineligible(
                                UnmaterializationIneligibilityReason::FinalOutput,
                            )
                        } else {
                            match rematerialization_method {
                                Some(method) => method.unmaterialization_eligibility(
                                    metadata,
                                    self.rematerialization_deadline,
                                ),
                                None if self.unmaterialize_upload_enabled => {
                                    UnmaterializationEligibility::EligibleAfterUpload
                                }
                                None => UnmaterializationEligibility::Ineligible(
                                    UnmaterializationIneligibilityReason::NoRematerializationMethod,
                                ),
                            }
                        };
                    self.found_paths.push(FoundPath::Tracked {
                        path,
                        size: artifact_metadata_size(metadata),
                        state: TrackedState::ActiveRetained {
                            last_access_time: *last_access_time,
                            classification: *classification,
                            unmaterialization_eligibility,
                        },
                    });
                }
                _ => {
                    // What we have on disk does not match what we have in the materializer (which is
                    // not clean stale's problem to fix).
                    tracing::trace!(path = %path, file_type = ?file_type, "skipping");
                    self.stats.scan_tree_state_mismatch_count += 1;
                }
            }
        }

        Ok(())
    }
}

fn find_stale_tracked_only(
    tree: &ArtifactTree,
    keep_since_time: Timestamp,
    found_paths: &mut Vec<FoundPath>,
) {
    for (f_path, v) in tree.iter_with_paths() {
        if let ArtifactMaterializationStage::Materialized {
            last_access_time,
            active,
            ..
        } = &v.stage
        {
            let path = ProjectRelativePathBuf::from(f_path);
            if *last_access_time < keep_since_time && !active {
                tracing::trace!(path = %path, "stale artifact");
                found_paths.push(FoundPath::Tracked {
                    path,
                    size: 0,
                    state: TrackedState::Stale(StaleOrigin::Ttl),
                });
            } else if *active {
                tracing::trace!(path = %path, "retaining artifact (active)");
                found_paths.push(FoundPath::Tracked {
                    path,
                    size: 0,
                    state: TrackedState::ActiveRetained {
                        last_access_time: *last_access_time,
                        classification: v.classification,
                        unmaterialization_eligibility: UnmaterializationEligibility::Ineligible(
                            UnmaterializationIneligibilityReason::NoRematerializationMethod,
                        ),
                    },
                });
            } else {
                tracing::trace!(path = %path, "retaining artifact");
                found_paths.push(FoundPath::Tracked {
                    path,
                    size: 0,
                    state: TrackedState::Retained {
                        last_access_time: *last_access_time,
                        classification: v.classification,
                    },
                });
            }
        }
    }
}

pub(crate) struct AdaptiveCleanupResult {
    pub(crate) outcome: AdaptiveOutcome,
    pub(crate) free_bytes_before: u64,
    pub(crate) total_bytes: u64,
    pub(crate) bytes_needed: u64,
    pub(crate) shortfall_bytes: u64,
    pub(crate) reached_unmaterialization: bool,
}

/// Promotes retained, non-active artifacts to stale, oldest-access-first, until
/// projected free disk space rises above `params.threshold_percent` of `total_space`,
/// or every promotable artifact has been promoted. Artifacts last accessed at
/// or after `params.min_access_time` are excluded unless they are intermediate-only
/// and `params.delete_intermediate_within_min_ttl` is enabled.
pub(crate) fn apply_adaptive_low_disk(
    found_paths: &mut [FoundPath],
    free_space: u64,
    total_space: u64,
    params: &AdaptiveLowDiskParams,
) -> AdaptiveCleanupResult {
    if total_space == 0 {
        return AdaptiveCleanupResult {
            outcome: AdaptiveOutcome::InvalidDiskStats,
            free_bytes_before: free_space,
            total_bytes: total_space,
            bytes_needed: 0,
            shortfall_bytes: 0,
            reached_unmaterialization: false,
        };
    }
    let free_pct = free_space as f64 / total_space as f64 * 100.0;
    if free_pct > params.threshold_percent {
        return AdaptiveCleanupResult {
            outcome: AdaptiveOutcome::AboveThreshold,
            free_bytes_before: free_space,
            total_bytes: total_space,
            bytes_needed: 0,
            shortfall_bytes: 0,
            reached_unmaterialization: false,
        };
    }

    let target_free = (params.threshold_percent / 100.0 * total_space as f64).ceil() as u64;
    let bytes_needed = target_free.saturating_sub(free_space);
    if bytes_needed == 0 {
        return AdaptiveCleanupResult {
            outcome: AdaptiveOutcome::NoBytesNeeded,
            free_bytes_before: free_space,
            total_bytes: total_space,
            bytes_needed,
            shortfall_bytes: 0,
            reached_unmaterialization: false,
        };
    }

    let mut promotable: Vec<(usize, Timestamp, u64)> = found_paths
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match p {
            FoundPath::Tracked {
                state:
                    TrackedState::Retained {
                        last_access_time,
                        classification,
                    },
                size,
                ..
            } if *last_access_time < params.min_access_time
                || (params.delete_intermediate_within_min_ttl
                    && *classification == ArtifactClassification::IntermediateOnly) =>
            {
                Some((i, *last_access_time, *size))
            }
            _ => None,
        })
        .collect();
    promotable.sort_by_key(|(_, last_access_time, _)| *last_access_time);

    let mut accumulated: u64 = 0;
    for (i, _, size) in promotable {
        if accumulated >= bytes_needed {
            break;
        }
        if let FoundPath::Tracked { state, .. } = &mut found_paths[i] {
            *state = TrackedState::Stale(StaleOrigin::Adaptive);
        }
        accumulated = accumulated.saturating_add(size);
    }

    if accumulated >= bytes_needed || !params.unmaterialize_active {
        return AdaptiveCleanupResult {
            outcome: if accumulated >= bytes_needed {
                AdaptiveOutcome::SatisfiedByStaleDeletion
            } else {
                AdaptiveOutcome::InsufficientCandidates
            },
            free_bytes_before: free_space,
            total_bytes: total_space,
            bytes_needed,
            shortfall_bytes: bytes_needed.saturating_sub(accumulated),
            reached_unmaterialization: false,
        };
    }

    let projected_free = found_paths
        .iter()
        .filter_map(|path| match path {
            FoundPath::Untracked(_, _, size)
            | FoundPath::Tracked {
                size,
                state: TrackedState::Stale(_),
                ..
            } => Some(*size),
            _ => None,
        })
        .fold(free_space, u64::saturating_add);
    let unmaterialization_target_free =
        (params.unmaterialization_threshold_percent / 100.0 * total_space as f64).ceil() as u64;
    let unmaterialization_bytes_needed =
        unmaterialization_target_free.saturating_sub(projected_free);
    if unmaterialization_bytes_needed == 0 {
        return AdaptiveCleanupResult {
            outcome: AdaptiveOutcome::InsufficientCandidates,
            free_bytes_before: free_space,
            total_bytes: total_space,
            bytes_needed,
            shortfall_bytes: bytes_needed.saturating_sub(accumulated),
            reached_unmaterialization: false,
        };
    }

    let mut active: Vec<(usize, Timestamp, u64, bool)> = found_paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| match path {
            FoundPath::Tracked {
                size,
                state:
                    TrackedState::ActiveRetained {
                        last_access_time,
                        classification: ArtifactClassification::IntermediateOnly,
                        unmaterialization_eligibility:
                            eligibility @ (UnmaterializationEligibility::Eligible
                            | UnmaterializationEligibility::EligibleAfterUpload),
                    },
                ..
            } => Some((
                index,
                *last_access_time,
                *size,
                matches!(
                    eligibility,
                    UnmaterializationEligibility::EligibleAfterUpload
                ),
            )),
            _ => None,
        })
        .collect();
    // Prefer to clean artifacts that are already remote-backed over ones that need uploads
    active.sort_by_key(|(_, last_access_time, _, upload)| (*upload, *last_access_time));
    let mut unmaterialized_bytes = 0;
    for (index, _, size, _) in active {
        if unmaterialized_bytes >= unmaterialization_bytes_needed {
            break;
        }
        if let FoundPath::Tracked { state, .. } = &mut found_paths[index] {
            *state = TrackedState::Unmaterialize;
        }
        unmaterialized_bytes = unmaterialized_bytes.saturating_add(size);
    }

    let accumulated = accumulated.saturating_add(unmaterialized_bytes);

    AdaptiveCleanupResult {
        outcome: if accumulated >= bytes_needed {
            AdaptiveOutcome::SatisfiedByUnmaterialization
        } else {
            AdaptiveOutcome::InsufficientCandidates
        },
        free_bytes_before: free_space,
        total_bytes: total_space,
        bytes_needed,
        shortfall_bytes: bytes_needed.saturating_sub(accumulated),
        reached_unmaterialization: true,
    }
}

#[derive(Debug, Clone)]
pub struct CleanStaleConfig {
    pub schedule: Option<CleanStaleSchedule>,
    pub artifact_ttl: Duration,
    pub dry_run: bool,
    pub low_disk: Option<LowDiskCleanConfig>,
    pub unmaterialize_upload: Option<UnmaterializationUploadConfig>,
}

#[derive(Debug, Clone)]
pub struct CleanStaleSchedule {
    pub start_offset: Duration,
    pub clean_period: Duration,
}

impl Default for CleanStaleConfig {
    fn default() -> Self {
        Self {
            schedule: None,
            artifact_ttl: Duration::from_secs(DEFAULT_CLEAN_STALE_TTL_DAYS * 24 * 60 * 60),
            dry_run: false,
            low_disk: None,
            unmaterialize_upload: None,
        }
    }
}

/// Configures how clean-stale reacts to low free disk space.
#[derive(Debug, Clone)]
pub struct LowDiskCleanConfig {
    /// Free disk space (as a percentage of total) at or below which the
    /// `mode` engages.
    pub threshold_percent: f64,
    pub mode: LowDiskCleanMode,
}

#[derive(Debug, Clone)]
pub enum LowDiskCleanMode {
    /// Use this smaller TTL when free disk % is at/below the threshold.
    Fixed(Duration),
    /// Run the normal-TTL clean, then keep marking retained, non-active
    /// artifacts (oldest access first) as stale until projected free disk %
    /// rises back above the threshold or every eligible artifact has been
    /// marked. Retained artifacts below `min_ttl` are protected unless
    /// deletion within the minimum TTL is enabled for intermediate artifacts.
    Adaptive {
        min_ttl: Duration,
        delete_intermediate_within_min_ttl: bool,
        unmaterialize_active: bool,
        unmaterialization_threshold_percent: f64,
    },
}

/// Rejects values (negative, NaN, out of range) that the datetime arithmetic these durations
/// feed into cannot represent.
fn duration_from_config_hours(hours: f64, property: &str) -> buck2_error::Result<Duration> {
    Duration::try_from_secs_f64(hours * 3600.0).map_err(|e| {
        buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Invalid value `{}` for `buck2.{}`: {}",
            hours,
            property,
            e
        )
    })
}

fn percentage_from_config(value: f64, property: &str) -> buck2_error::Result<f64> {
    if (0.0..=100.0).contains(&value) {
        Ok(value)
    } else {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Invalid value `{}` for `buck2.{}`: expected a percentage between 0.0 and 100.0",
            value,
            property,
        ))
    }
}

fn unmaterialization_threshold_from_config(
    value: Option<f64>,
    low_disk_threshold: f64,
) -> buck2_error::Result<f64> {
    let value = percentage_from_config(
        value.unwrap_or(low_disk_threshold),
        "clean_stale_low_disk_unmaterialization_threshold",
    )?;
    if value <= low_disk_threshold {
        Ok(value)
    } else {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "`buck2.clean_stale_low_disk_unmaterialization_threshold` ({}) must not exceed `buck2.clean_stale_low_disk_threshold` ({})",
            value,
            low_disk_threshold,
        ))
    }
}

impl CleanStaleConfig {
    pub fn from_buck_config(root_config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
        let clean_stale_enabled = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_enabled",
            })?
            .unwrap_or(false);
        let clean_stale_artifact_ttl_hours = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_artifact_ttl_hours",
            })?
            .unwrap_or(24.0 * DEFAULT_CLEAN_STALE_TTL_DAYS as f64);
        let clean_stale_period_hours = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_period_hours",
            })?
            .unwrap_or(24.0);
        let clean_stale_start_offset_hours = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_start_offset_hours",
            })?
            .unwrap_or(12.0);
        let clean_stale_dry_run = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_dry_run",
            })?
            .unwrap_or(false);
        let adaptive_enabled = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_low_disk_adaptive_enabled",
            })?
            .unwrap_or(false);
        let adaptive_min_ttl_hours: f64 = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_low_disk_adaptive_min_ttl_hours",
            })?
            .unwrap_or(12.0);
        let delete_intermediate_within_min_ttl = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_low_disk_adaptive_delete_intermediate_within_min_ttl",
            })?
            .unwrap_or(false);
        let unmaterialize_active = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_low_disk_adaptive_unmaterialize_active",
            })?
            .unwrap_or(false);
        let unmaterialization_threshold_percent: Option<f64> =
            root_config.parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_low_disk_unmaterialization_threshold",
            })?;
        let unmaterialize_upload_enabled = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_unmaterialize_upload_enabled",
            })?
            .unwrap_or(false);
        let unmaterialize_upload_max_bytes = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "clean_stale_unmaterialize_upload_max_bytes",
            })?
            .unwrap_or(1024 * 1024 * 1024);
        let re_use_case = RemoteExecutorUseCase::new("buck2-local-unmaterialization".to_owned());
        let unmaterialize_upload =
            unmaterialize_upload_enabled.then_some(UnmaterializationUploadConfig {
                re_use_case,
                max_bytes: unmaterialize_upload_max_bytes,
            });
        let low_disk_artifact_ttl_hours: Option<f64> = root_config.parse(BuckconfigKeyRef {
            section: "buck2",
            property: "clean_stale_low_disk_artifact_ttl_hours",
        })?;
        let low_disk_threshold_percent: Option<f64> = root_config.parse(BuckconfigKeyRef {
            section: "buck2",
            property: "clean_stale_low_disk_threshold",
        })?;
        let low_disk = match low_disk_threshold_percent {
            Some(threshold_percent) => {
                let threshold_percent =
                    percentage_from_config(threshold_percent, "clean_stale_low_disk_threshold")?;
                let mode = if adaptive_enabled {
                    let unmaterialization_threshold_percent =
                        unmaterialization_threshold_from_config(
                            unmaterialization_threshold_percent,
                            threshold_percent,
                        )?;
                    LowDiskCleanMode::Adaptive {
                        min_ttl: duration_from_config_hours(
                            adaptive_min_ttl_hours,
                            "clean_stale_low_disk_adaptive_min_ttl_hours",
                        )?,
                        delete_intermediate_within_min_ttl,
                        unmaterialize_active,
                        unmaterialization_threshold_percent,
                    }
                } else {
                    let hours = low_disk_artifact_ttl_hours.unwrap_or(48.0);
                    LowDiskCleanMode::Fixed(duration_from_config_hours(
                        hours,
                        "clean_stale_low_disk_artifact_ttl_hours",
                    )?)
                };
                Some(LowDiskCleanConfig {
                    threshold_percent,
                    mode,
                })
            }
            None => None,
        };
        let schedule = if clean_stale_enabled {
            Some(CleanStaleSchedule {
                clean_period: duration_from_config_hours(
                    clean_stale_period_hours,
                    "clean_stale_period_hours",
                )?,
                start_offset: duration_from_config_hours(
                    clean_stale_start_offset_hours,
                    "clean_stale_start_offset_hours",
                )?,
            })
        } else {
            None
        };
        Ok(Self {
            schedule,
            artifact_ttl: duration_from_config_hours(
                clean_stale_artifact_ttl_hours,
                "clean_stale_artifact_ttl_hours",
            )?,
            low_disk,
            unmaterialize_upload,
            dry_run: clean_stale_dry_run,
        })
    }

    pub fn suppress_unmaterialize_without_ttl_refresh(&mut self, ttl_refresh_enabled: bool) {
        if ttl_refresh_enabled {
            return;
        }
        let Some(LowDiskCleanConfig {
            mode:
                LowDiskCleanMode::Adaptive {
                    unmaterialize_active,
                    ..
                },
            ..
        }) = self.low_disk.as_mut()
        else {
            return;
        };
        if *unmaterialize_active {
            tracing::info!(
                "Disabling adaptive active-artifact unmaterialization because TTL refresh is disabled"
            );
            *unmaterialize_active = false;
        }
    }

    /// Invocation tags describing whether adaptive low-disk clean-stale is
    /// active and, if so, its parameters.
    pub fn adaptive_telemetry_tags(config: Option<&CleanStaleConfig>) -> Vec<String> {
        match config.and_then(|c| c.low_disk.as_ref()) {
            Some(LowDiskCleanConfig {
                threshold_percent,
                mode:
                    LowDiskCleanMode::Adaptive {
                        min_ttl,
                        delete_intermediate_within_min_ttl,
                        unmaterialize_active,
                        unmaterialization_threshold_percent,
                    },
            }) => vec![
                "adaptive-clean-stale:true".to_owned(),
                format!(
                    "adaptive-clean-stale-threshold-percent:{}",
                    threshold_percent
                ),
                format!(
                    "adaptive-clean-stale-min-ttl-hours:{}",
                    min_ttl.as_secs_f64() / 3600.0
                ),
                format!(
                    "adaptive-clean-stale-delete-intermediate-within-min-ttl:{}",
                    delete_intermediate_within_min_ttl
                ),
                format!(
                    "adaptive-clean-stale-unmaterialize-active:{}",
                    unmaterialize_active
                ),
                format!(
                    "adaptive-clean-stale-unmaterialization-threshold-percent:{}",
                    unmaterialization_threshold_percent
                ),
            ],
            _ => vec!["adaptive-clean-stale:false".to_owned()],
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_data::CleanStaleResultKind;
    use buck2_data::CleanStaleStats;
    use buck2_data::clean_stale_result::AdaptiveOutcome;
    use buck2_error::internal_error;
    use buck2_events::daemon_id::DaemonId;
    use buck2_fs::fs_util;
    use jiff::Timestamp;

    use crate::materializers::deferred::artifact_tree::ArtifactClassification;
    use crate::materializers::deferred::artifact_tree::UnmaterializationEligibility;
    use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibilityReason;
    use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibleArtifact;
    use crate::materializers::deferred::artifact_tree::UnmaterializeArtifactsResult;
    use crate::materializers::deferred::clean_stale::AdaptiveLowDiskParams;
    use crate::materializers::deferred::clean_stale::CleanDiskSpace;
    use crate::materializers::deferred::clean_stale::CleanFailure;
    use crate::materializers::deferred::clean_stale::CleanFailurePhase;
    use crate::materializers::deferred::clean_stale::CleanResult;
    use crate::materializers::deferred::clean_stale::CleanStaleStatsExt;
    use crate::materializers::deferred::clean_stale::FoundPath;
    use crate::materializers::deferred::clean_stale::StaleOrigin;
    use crate::materializers::deferred::clean_stale::TrackedState;
    use crate::materializers::deferred::clean_stale::apply_adaptive_low_disk;
    use crate::materializers::deferred::clean_stale::create_result;
    use crate::materializers::deferred::clean_stale::duration_from_config_hours;
    use crate::materializers::deferred::clean_stale::percentage_from_config;
    use crate::materializers::deferred::clean_stale::record_used_disk_space;
    use crate::materializers::deferred::clean_stale::unmaterialization_threshold_from_config;

    #[test]
    fn test_used_disk_space_outcomes() {
        for kind in [
            CleanStaleResultKind::Finished,
            CleanStaleResultKind::Interrupted,
            CleanStaleResultKind::SkippedDryRun,
            CleanStaleResultKind::SkippedNoGenDir,
            CleanStaleResultKind::SkippedDeferWriteDisabled,
            CleanStaleResultKind::SkippedSqliteDisabled,
            CleanStaleResultKind::Failed,
        ] {
            for (start, end) in [
                (Some(101), Some(99)),
                (Some(0), None),
                (None, Some(0)),
                (None, None),
            ] {
                let mut result = if kind == CleanStaleResultKind::Failed {
                    Err(CleanFailure::new(
                        CleanFailurePhase::Scan,
                        CleanStaleStats::default(),
                        AdaptiveOutcome::Disabled,
                        internal_error!("scan failed"),
                    ))
                } else {
                    Ok(CleanResult::new(
                        kind,
                        CleanStaleStats::default(),
                        AdaptiveOutcome::Disabled,
                    ))
                };
                record_used_disk_space(&mut result, start, end);
                let event = create_result(
                    result,
                    None,
                    &DaemonId::new(),
                    3,
                    buck2_data::clean_stale_result::Trigger::ManualExplicit,
                    buck2_data::clean_stale_result::Origin::CleanCommand,
                    buck2_data::clean_stale_result::PolicyMode::ExplicitTtl,
                    false,
                );
                assert_eq!(event.kind, kind as i32);
                assert_eq!(event.error.is_some(), kind == CleanStaleResultKind::Failed);
                let stats = event.stats.expect("clean stats");
                assert_eq!(stats.used_disk_space_bytes_start, start);
                assert_eq!(stats.used_disk_space_bytes_end, end);
                assert_eq!(stats.total_duration_s, 3);
            }
        }
    }

    #[test]
    fn test_used_disk_space_probe_path() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let path = temp
            .path()
            .resolve(ProjectRelativePathBuf::unchecked_new(
                "buck-out/missing".to_owned(),
            ))
            .into_abs_path_buf();
        let sample = CleanDiskSpace::new(Some(&path));
        assert_eq!(
            sample.path,
            Some(
                fs_util::canonicalize(temp.path().root())
                    .expect("canonicalize temp root")
                    .into_abs_path_buf()
            )
        );
        assert!(sample.start.is_some());
        assert!(!fs_util::try_exists(&path)?);
        let mut result = Ok(CleanResult::new(
            CleanStaleResultKind::SkippedNoGenDir,
            CleanStaleStats::default(),
            AdaptiveOutcome::Disabled,
        ));
        sample.record(&mut result);
        let stats = result.ok().expect("clean succeeds").stats;
        assert!(stats.used_disk_space_bytes_start.is_some());
        assert!(stats.used_disk_space_bytes_end.is_some());

        let mut result = Ok(CleanResult::new(
            CleanStaleResultKind::Finished,
            CleanStaleStats::default(),
            AdaptiveOutcome::Disabled,
        ));
        CleanDiskSpace {
            path: Some(path),
            start: Some(0),
        }
        .record(&mut result);
        let stats = result
            .ok()
            .expect("end probe failure does not fail cleaning")
            .stats;
        assert_eq!(stats.used_disk_space_bytes_start, Some(0));
        assert_eq!(stats.used_disk_space_bytes_end, None);

        let mut result = Ok(CleanResult::new(
            CleanStaleResultKind::Finished,
            CleanStaleStats::default(),
            AdaptiveOutcome::Disabled,
        ));
        CleanDiskSpace::new(None).record(&mut result);
        let stats = result
            .ok()
            .expect("probe failure does not fail cleaning")
            .stats;
        assert_eq!(stats.used_disk_space_bytes_start, None);
        assert_eq!(stats.used_disk_space_bytes_end, None);
        Ok(())
    }

    #[test]
    fn test_duration_from_config_hours() {
        assert_eq!(
            duration_from_config_hours(2.0, "prop").unwrap(),
            std::time::Duration::from_secs(7200)
        );
        assert!(duration_from_config_hours(-1.0, "prop").is_err());
        assert!(duration_from_config_hours(f64::NAN, "prop").is_err());
        assert!(duration_from_config_hours(f64::INFINITY, "prop").is_err());
    }

    #[test]
    fn test_percentage_from_config() {
        assert_eq!(percentage_from_config(0.0, "prop").unwrap(), 0.0);
        assert_eq!(percentage_from_config(100.0, "prop").unwrap(), 100.0);
        assert!(percentage_from_config(-1.0, "prop").is_err());
        assert!(percentage_from_config(100.1, "prop").is_err());
        assert!(percentage_from_config(f64::NAN, "prop").is_err());
        assert!(percentage_from_config(f64::INFINITY, "prop").is_err());
    }

    #[test]
    fn test_unmaterialization_threshold_from_config() {
        assert_eq!(
            unmaterialization_threshold_from_config(None, 10.0).unwrap(),
            10.0
        );
        assert_eq!(
            unmaterialization_threshold_from_config(Some(5.0), 10.0).unwrap(),
            5.0
        );
        assert!(unmaterialization_threshold_from_config(Some(11.0), 10.0).is_err());
    }

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_second(secs).unwrap()
    }

    fn retained(name: &str, last_access_secs: i64, size: u64) -> FoundPath {
        retained_with_classification(
            name,
            last_access_secs,
            size,
            ArtifactClassification::FinalOutput,
        )
    }

    fn retained_with_classification(
        name: &str,
        last_access_secs: i64,
        size: u64,
        classification: ArtifactClassification,
    ) -> FoundPath {
        FoundPath::Tracked {
            path: ProjectRelativePathBuf::unchecked_new(name.to_owned()),
            size,
            state: TrackedState::Retained {
                last_access_time: t(last_access_secs),
                classification,
            },
        }
    }

    fn active_retained(size: u64) -> FoundPath {
        active_retained_with_classification(
            "active",
            100,
            size,
            ArtifactClassification::IntermediateOnly,
            true,
        )
    }

    fn active_retained_with_classification(
        name: &str,
        last_access_secs: i64,
        size: u64,
        classification: ArtifactClassification,
        rematerializable: bool,
    ) -> FoundPath {
        let unmaterialization_eligibility = if classification == ArtifactClassification::FinalOutput
        {
            UnmaterializationEligibility::Ineligible(
                UnmaterializationIneligibilityReason::FinalOutput,
            )
        } else if rematerializable {
            UnmaterializationEligibility::Eligible
        } else {
            UnmaterializationEligibility::Ineligible(
                UnmaterializationIneligibilityReason::NoRematerializationMethod,
            )
        };
        FoundPath::Tracked {
            path: ProjectRelativePathBuf::unchecked_new(name.to_owned()),
            size,
            state: TrackedState::ActiveRetained {
                last_access_time: t(last_access_secs),
                classification,
                unmaterialization_eligibility,
            },
        }
    }

    fn is_stale(p: &FoundPath, expected_size: u64) -> bool {
        matches!(
            p,
            FoundPath::Tracked {
                size,
                state: TrackedState::Stale(_),
                ..
            } if *size == expected_size
        )
    }

    fn is_retained(p: &FoundPath) -> bool {
        matches!(
            p,
            FoundPath::Tracked {
                state: TrackedState::Retained { .. },
                ..
            }
        )
    }

    fn is_active_retained(p: &FoundPath) -> bool {
        matches!(
            p,
            FoundPath::Tracked {
                state: TrackedState::ActiveRetained { .. },
                ..
            }
        )
    }

    /// Sentinel `min_access_time` that protects no artifacts — every retained
    /// artifact has `last_access_time` strictly less than this far-future
    /// instant, so the full promotion logic is exercised end-to-end.
    fn no_min_ttl() -> Timestamp {
        Timestamp::MAX
    }

    fn adaptive_params(threshold_percent: f64) -> AdaptiveLowDiskParams {
        AdaptiveLowDiskParams {
            threshold_percent,
            unmaterialization_threshold_percent: threshold_percent,
            min_access_time: no_min_ttl(),
            delete_intermediate_within_min_ttl: false,
            unmaterialize_active: false,
        }
    }

    fn adaptive_params_with_min_access_time(
        threshold_percent: f64,
        min_access_time: Timestamp,
        delete_intermediate_within_min_ttl: bool,
    ) -> AdaptiveLowDiskParams {
        AdaptiveLowDiskParams {
            min_access_time,
            delete_intermediate_within_min_ttl,
            ..adaptive_params(threshold_percent)
        }
    }

    fn adaptive_params_with_unmaterialization(
        threshold_percent: f64,
        unmaterialization_threshold_percent: f64,
    ) -> AdaptiveLowDiskParams {
        AdaptiveLowDiskParams {
            unmaterialization_threshold_percent,
            unmaterialize_active: true,
            ..adaptive_params(threshold_percent)
        }
    }

    #[test]
    fn threshold_already_met_is_noop() {
        let mut paths = vec![retained("a", 100, 50)];
        // 60% free, threshold 50% -> already above, do nothing.
        let result = apply_adaptive_low_disk(&mut paths, 60, 100, &adaptive_params(50.0));
        assert_eq!(result.outcome, AdaptiveOutcome::AboveThreshold);
        assert!(
            is_retained(&paths[0]),
            "free disk (60%) already exceeds threshold (50%); nothing should be promoted",
        );
    }

    #[test]
    fn promotes_oldest_first_until_threshold_crossed() {
        // total=1000, free=100 (10%), threshold=50% -> need 400 bytes promoted.
        let mut paths = vec![
            retained("new", 300, 200),
            retained("oldest", 100, 100),
            retained("old", 200, 350),
        ];
        let result = apply_adaptive_low_disk(&mut paths, 100, 1000, &adaptive_params(50.0));
        assert_eq!(result.outcome, AdaptiveOutcome::SatisfiedByStaleDeletion);
        assert_eq!(result.bytes_needed, 400);
        assert_eq!(result.shortfall_bytes, 0);
        // Oldest (t=100, size=100) + next (t=200, size=350) = 450 >= 400 bytes_needed.
        // Newest (t=300) is left alone — promotion stops as soon as the running total crosses bytes_needed.
        assert!(
            is_stale(&paths[1], 100),
            "oldest retained (size 100) should be promoted to stale first",
        );
        assert!(
            is_stale(&paths[2], 350),
            "second-oldest retained (size 350) should be promoted; running total 100+350=450 >= 400",
        );
        assert!(
            is_retained(&paths[0]),
            "newest retained (t=300) should remain retained — threshold already met after promoting the two older entries",
        );
    }

    #[test]
    fn all_retained_insufficient_promotes_all_non_active() {
        // total=1000, free=0, threshold=100% -> need everything we can spare.
        let mut paths = vec![
            retained("a", 100, 10),
            retained("b", 200, 20),
            active_retained(9999),
        ];
        let result = apply_adaptive_low_disk(&mut paths, 0, 1000, &adaptive_params(100.0));
        assert_eq!(result.outcome, AdaptiveOutcome::InsufficientCandidates);
        assert_eq!(result.shortfall_bytes, 970);
        assert!(
            is_stale(&paths[0], 10),
            "non-active retained `a` should be promoted"
        );
        assert!(
            is_stale(&paths[1], 20),
            "non-active retained `b` should be promoted"
        );
        // Active retained MUST stay put — invariant: adaptive never deletes
        // more than `clean --keep-since-time=<future>` would.
        assert!(
            is_active_retained(&paths[2]),
            "active retained must never be promoted, even when the threshold cannot be met",
        );
    }

    #[test]
    fn mixed_inputs_only_touch_retained() {
        let mut paths = vec![
            FoundPath::Untracked(
                ProjectRelativePathBuf::unchecked_new("u".to_owned()),
                buck2_common::file_ops::metadata::FileType::File,
                42,
            ),
            FoundPath::Tracked {
                path: ProjectRelativePathBuf::unchecked_new("already_stale".to_owned()),
                size: 7,
                state: TrackedState::Stale(StaleOrigin::Ttl),
            },
            retained("r", 100, 100),
            active_retained(50),
        ];
        apply_adaptive_low_disk(&mut paths, 0, 1000, &adaptive_params(100.0));
        assert!(
            matches!(&paths[0], FoundPath::Untracked(_, _, 42)),
            "untracked entries should never be touched",
        );
        assert!(
            is_stale(&paths[1], 7),
            "already-stale entries should remain stale (and unchanged)",
        );
        assert!(
            is_stale(&paths[2], 100),
            "the lone retained entry should be promoted"
        );
        assert!(
            is_active_retained(&paths[3]),
            "active retained must never be promoted",
        );
    }

    #[test]
    fn zero_total_is_noop() {
        let mut paths = vec![retained("a", 100, 50)];
        let result = apply_adaptive_low_disk(&mut paths, 0, 0, &adaptive_params(100.0));
        assert_eq!(result.outcome, AdaptiveOutcome::InvalidDiskStats);
        assert!(
            is_retained(&paths[0]),
            "total_space=0 must short-circuit to a no-op (avoids divide-by-zero)",
        );
    }

    #[test]
    fn min_ttl_protects_recent_retained() {
        // Disk pressure is maximal (free=0, threshold=100%) so adaptive would
        // normally promote every retained, non-active artifact.
        // `min_access_time = t(150)` protects everything accessed at or after t=150.
        let mut paths = vec![
            retained("old", 100, 50),
            retained("on_boundary", 150, 50),
            retained("recent", 200, 50),
        ];
        apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_min_access_time(100.0, t(150), false),
        );
        assert!(
            is_stale(&paths[0], 50),
            "retained accessed before min_access_time is eligible for promotion",
        );
        assert!(
            is_retained(&paths[1]),
            "retained whose last_access_time equals min_access_time is protected (cutoff is strict <)",
        );
        assert!(
            is_retained(&paths[2]),
            "retained accessed after min_access_time must be protected by the adaptive minimum TTL",
        );
    }

    #[test]
    fn min_ttl_can_block_all_promotions() {
        // Even with maximal disk pressure, a min_access_time at or below every
        // retained access time means every artifact is within the min-TTL
        // window — adaptive cleaning never violates the minimum TTL floor.
        let mut paths = vec![retained("a", 100, 100), retained("b", 200, 100)];
        apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_min_access_time(100.0, t(50), false),
        );
        assert!(
            is_retained(&paths[0]) && is_retained(&paths[1]),
            "no retained artifact may be promoted when all are within the adaptive min-TTL window",
        );
    }

    #[test]
    fn enabled_gate_promotes_recent_intermediate_only() {
        let mut paths = vec![
            retained_with_classification(
                "intermediate",
                200,
                50,
                ArtifactClassification::IntermediateOnly,
            ),
            retained("final", 200, 50),
        ];

        apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_min_access_time(100.0, t(150), true),
        );

        assert!(
            is_stale(&paths[0], 50),
            "an intermediate-only artifact within the minimum TTL should be promoted when enabled",
        );
        assert!(
            is_retained(&paths[1]),
            "a final output within the minimum TTL must remain protected",
        );
    }

    #[test]
    fn disabled_gate_protects_recent_intermediate_only() {
        let mut paths = vec![retained_with_classification(
            "intermediate",
            200,
            50,
            ArtifactClassification::IntermediateOnly,
        )];

        apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_min_access_time(100.0, t(150), false),
        );

        assert!(
            is_retained(&paths[0]),
            "the default-disabled gate must preserve minimum-TTL protection",
        );
    }

    #[test]
    fn unmaterializes_active_intermediates_oldest_first_after_deletion_passes() {
        let mut paths = vec![
            retained("retained", 50, 10),
            active_retained_with_classification(
                "newer_active",
                300,
                500,
                ArtifactClassification::IntermediateOnly,
                true,
            ),
            active_retained_with_classification(
                "older_active",
                200,
                100,
                ArtifactClassification::IntermediateOnly,
                true,
            ),
        ];

        let result = apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_unmaterialization(10.0, 10.0),
        );

        assert_eq!(
            result.outcome,
            AdaptiveOutcome::SatisfiedByUnmaterialization
        );
        assert!(result.reached_unmaterialization);
        assert!(is_stale(&paths[0], 10));
        assert!(matches!(
            &paths[2],
            FoundPath::Tracked {
                state: TrackedState::Unmaterialize,
                ..
            }
        ));
        assert!(is_active_retained(&paths[1]));

        let mut stats = CleanStaleStats::default();
        stats.record_paths(&paths, result.reached_unmaterialization);
        assert_eq!(
            (
                stats.retained_artifact_count,
                stats.retained_bytes,
                stats.retained_inactive_artifact_count,
                stats.retained_inactive_bytes,
                stats.retained_active_artifact_count,
                stats.retained_active_bytes,
                stats.adaptive_unmaterialization_selected_artifact_count,
                stats.adaptive_unmaterialization_selected_bytes,
            ),
            (2, 600, 0, 0, 1, 500, 1, 100),
            "retained paths should partition into inactive, active, and selected artifacts",
        );
    }

    #[test]
    fn unmaterialization_stops_at_separate_threshold() {
        let mut paths = vec![
            active_retained_with_classification(
                "newer_active",
                200,
                500,
                ArtifactClassification::IntermediateOnly,
                true,
            ),
            active_retained_with_classification(
                "older_active",
                100,
                100,
                ArtifactClassification::IntermediateOnly,
                true,
            ),
        ];

        let result = apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_unmaterialization(100.0, 10.0),
        );

        assert_eq!(result.outcome, AdaptiveOutcome::InsufficientCandidates);
        assert_eq!(result.shortfall_bytes, 900);
        assert!(result.reached_unmaterialization);
        assert!(is_active_retained(&paths[0]));
        assert!(matches!(
            &paths[1],
            FoundPath::Tracked {
                state: TrackedState::Unmaterialize,
                ..
            }
        ));
    }

    #[test]
    fn inactive_cleanup_can_avoid_unmaterialization() {
        let mut paths = vec![retained("retained", 100, 20), active_retained(100)];

        let result = apply_adaptive_low_disk(
            &mut paths,
            40,
            1000,
            &adaptive_params_with_unmaterialization(10.0, 5.0),
        );

        assert_eq!(result.outcome, AdaptiveOutcome::InsufficientCandidates);
        assert!(!result.reached_unmaterialization);
        assert!(is_stale(&paths[0], 20));
        assert!(is_active_retained(&paths[1]));
    }

    #[test]
    fn final_outputs_are_never_unmaterialized() {
        let mut paths = vec![active_retained_with_classification(
            "final",
            100,
            100,
            ArtifactClassification::FinalOutput,
            true,
        )];

        let result = apply_adaptive_low_disk(
            &mut paths,
            0,
            1000,
            &adaptive_params_with_unmaterialization(100.0, 100.0),
        );

        assert!(is_active_retained(&paths[0]));
        assert_eq!(result.outcome, AdaptiveOutcome::InsufficientCandidates);
        let mut stats = CleanStaleStats::default();
        stats.record_paths(&paths, result.reached_unmaterialization);
        assert_eq!(stats.retained_artifact_count, 1);
        assert_eq!(stats.retained_active_artifact_count, 1);
        assert_eq!(
            (
                stats.adaptive_unmaterialization_statically_ineligible_artifact_count,
                stats.adaptive_unmaterialization_statically_ineligible_bytes,
            ),
            (1, 100),
        );
        assert_eq!(stats.unmaterialization_ineligible_artifact_count, 0);
        assert_eq!(stats.unmaterialization_ineligible_bytes, 0);
        assert_eq!(
            stats.unmaterialization_ineligible_final_output_artifact_count,
            1
        );
        assert_eq!(stats.unmaterialization_ineligible_final_output_bytes, 100);
    }

    #[test]
    fn selected_unmaterializations_include_runtime_ineligible_artifacts() {
        let result = UnmaterializeArtifactsResult {
            uploads: Vec::new(),
            unmaterialized: vec![(
                ProjectRelativePathBuf::unchecked_new("selected".to_owned()),
                10,
            )],
            ineligible: vec![UnmaterializationIneligibleArtifact {
                reason: UnmaterializationIneligibilityReason::StateChanged,
                size: 20,
            }],
        };
        let mut stats = CleanStaleStats {
            adaptive_unmaterialization_selected_artifact_count: 2,
            adaptive_unmaterialization_selected_bytes: 30,
            ..Default::default()
        };

        stats.record_unmaterialization_result(&result);

        assert_eq!(
            (
                stats.adaptive_unmaterialization_selected_artifact_count,
                stats.adaptive_unmaterialization_selected_bytes,
                stats.unmaterialization_ineligible_artifact_count,
                stats.unmaterialization_ineligible_bytes,
                stats.adaptive_unmaterialization_statically_ineligible_artifact_count,
                stats.adaptive_unmaterialization_statically_ineligible_bytes,
                stats.unmaterialization_ineligible_state_changed_artifact_count,
                stats.unmaterialization_ineligible_state_changed_bytes,
            ),
            (2, 30, 1, 20, 0, 0, 1, 20),
            "runtime outcomes must not change adaptive selection counters",
        );
    }

    #[test]
    fn get_size_does_not_follow_symlinks() {
        use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;

        use crate::materializers::deferred::clean_stale::get_size;

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("big"), vec![0u8; 4096]).unwrap();

        let buck_out = tempfile::tempdir().unwrap();
        let dir = buck_out.path().join("dir");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("file"), b"abc").unwrap();
        buck2_fs::fs_util::symlink(
            outside.path(),
            buck2_fs::paths::abs_path::AbsPath::new(&dir.join("link")).unwrap(),
        )
        .unwrap();
        let link_len = std::fs::symlink_metadata(dir.join("link")).unwrap().len();

        let mut stats = CleanStaleStats::default();
        let link = AbsNormPathBuf::new(dir.join("link")).unwrap();
        assert_eq!(get_size(&link, &mut stats).unwrap(), link_len);
        let dir = AbsNormPathBuf::new(dir).unwrap();
        assert_eq!(get_size(&dir, &mut stats).unwrap(), 3 + link_len);
        assert_eq!(stats.scan_unreadable_count, 0);
    }
}
