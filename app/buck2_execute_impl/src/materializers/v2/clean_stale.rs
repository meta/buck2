/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `clean --stale` and the scratch sweep for the disk-state core.
//!
//! A run scans buck-out against a snapshot of the tracked roots and classifies what it finds the
//! way the deferred core does; that core's `clean_stale` module owns the policy, which roots are
//! stale, what the low-disk mode promotes, what the stats and the result say, and this one only
//! feeds it. What it decides to delete, it deletes under write holds the tree grants only if the
//! path is still what the scan saw and nobody holds or awaits it. The cleaner never waits for a
//! lease and never gets ahead of a request that does, so it is no party to any wait, and it
//! cannot delete an output an action is producing, which is write-held.
//!
//! "Active" is what this daemon has read or written: a root whose access time is at or after the
//! daemon's start. That is what the deferred core's `active` flag comes to, and it is what keeps
//! the outputs of the running build from a `clean --stale` issued against it. Active roots go
//! only under the adaptive low-disk policy's escalation, and only when their content can come
//! back: from the CAS, when every file's recorded expiry outlasts the refresher's next pass, or
//! after an upload that a presence check has confirmed.

use std::sync::Arc;
use std::time::Instant;

use buck2_common::file_ops::metadata::FileType;
use buck2_common::liveliness_observer::LivelinessGuard;
use buck2_common::liveliness_observer::LivelinessObserverSync;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::soft_error_or_panic;
use buck2_core::tag_error;
use buck2_data::CleanStaleResultKind;
use buck2_data::CleanStaleStats;
use buck2_data::clean_stale_result::AdaptiveOutcome;
use buck2_data::clean_stale_result::Origin;
use buck2_data::clean_stale_result::Trigger;
use buck2_error::ErrorTag;
use buck2_error::internal_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher;
use buck2_execute::blob_store::BlobStore;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::CleanStaleArtifactsArgs;
use buck2_execute::materialize::materializer::MaterializerBackgroundCleanupGuard;
use buck2_execute::re::presence::NegativeCache;
use buck2_fs::fs_util;
use buck2_fs::fs_util::disk_space_stats;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_util::future::join_all;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use jiff::SignedDuration;
use jiff::Timestamp;
use parking_lot::Mutex;

use crate::materializers::deferred::BackgroundCleanupGate;
use crate::materializers::deferred::DeferredMaterializerConfigs;
use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::UnmaterializationEligibility;
use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibilityReason;
use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibleArtifact;
use crate::materializers::deferred::clean_stale::CleanDiskSpace;
use crate::materializers::deferred::clean_stale::CleanFailure;
use crate::materializers::deferred::clean_stale::CleanFailurePhase;
use crate::materializers::deferred::clean_stale::CleanOutcome;
use crate::materializers::deferred::clean_stale::CleanPathOutcome;
use crate::materializers::deferred::clean_stale::CleanResult;
use crate::materializers::deferred::clean_stale::CleanStaleArtifactsCommand;
use crate::materializers::deferred::clean_stale::CleanStaleConfig;
use crate::materializers::deferred::clean_stale::CleanStaleError;
use crate::materializers::deferred::clean_stale::CleanStaleStatsExt;
use crate::materializers::deferred::clean_stale::CleanupOrigin;
use crate::materializers::deferred::clean_stale::FoundPath;
use crate::materializers::deferred::clean_stale::PendingCleanFailure;
use crate::materializers::deferred::clean_stale::StaleOrigin;
use crate::materializers::deferred::clean_stale::TrackedState;
use crate::materializers::deferred::clean_stale::apply_adaptive_low_disk;
use crate::materializers::deferred::clean_stale::clean_artifact;
use crate::materializers::deferred::clean_stale::create_result;
use crate::materializers::deferred::clean_stale::disk_space_probe_path;
use crate::materializers::deferred::clean_stale::get_size;
use crate::materializers::deferred::clean_stale::initial_stats;
use crate::materializers::deferred::clean_stale::rematerialization_deadline;
use crate::materializers::deferred::clean_stale::scratch_sweep;
use crate::materializers::deferred::io_handler::IoHandler;
use crate::materializers::v2::core::Granted;
use crate::materializers::v2::core::Sender;
use crate::materializers::v2::core::Shared;
use crate::materializers::v2::entry::DiskEntry;
use crate::materializers::v2::tree::DiskTree;
use crate::materializers::v2::tree::Expect;
use crate::materializers::v2::tree::Lookup;
use crate::materializers::v2::tree::Tracked;

/// The directories under buck-out that hold artifacts; the scan covers these and nothing else.
const ARTIFACT_DIRS: &[&str] = &["gen", "gen-anon", "gen-bxl", "art", "art-anon", "art-bxl"];

/// How many paths one message to the processor takes for cleaning.
const TAKE_BATCH: usize = 512;

/// Runs `clean --stale` and the scratch sweep for one materializer: on demand, and on the
/// configured schedule.
pub(crate) struct Cleaner<T> {
    io: Arc<T>,
    blocking_executor: Arc<dyn BlockingExecutor>,
    sender: Sender,
    shared: Arc<Shared>,
    config: CleanStaleConfig,
    rematerialization_ttl: Option<SignedDuration>,
    buck_out_abs_path: Option<Arc<AbsPathBuf>>,
    daemon_dispatcher: EventDispatcher,
    /// When this daemon started; what it has read or written since is active.
    started: Timestamp,
    gate: Arc<BackgroundCleanupGate>,
    /// The liveliness of the run in progress. Dropping it interrupts the run between paths.
    running: Mutex<Option<LivelinessGuard>>,
    /// Where a tracked file's CAS expiration is looked up, the tree holding plain digests.
    store: &'static BlobStore,
}

impl<T: IoHandler> Cleaner<T> {
    pub(crate) fn new(
        io: Arc<T>,
        blocking_executor: Arc<dyn BlockingExecutor>,
        sender: Sender,
        shared: Arc<Shared>,
        configs: &DeferredMaterializerConfigs,
        daemon_dispatcher: EventDispatcher,
        store: &'static BlobStore,
    ) -> Arc<Self> {
        let buck_out_abs_path = disk_space_probe_path(io.fs(), io.buck_out_path());
        Arc::new(Self {
            io,
            blocking_executor,
            sender,
            shared,
            config: configs.clean_stale_config.clone(),
            // The daemon-wide walk refreshes CAS TTLs under this core whatever
            // `buck2.ttl_refresh_enabled` says of the deferred core's own refresher.
            rematerialization_ttl: SignedDuration::try_from(configs.ttl_refresh.frequency).ok(),
            buck_out_abs_path,
            daemon_dispatcher,
            started: Timestamp::now(),
            gate: Arc::new(BackgroundCleanupGate::default()),
            running: Mutex::new(None),
            store,
        })
    }

    /// Runs the configured clean on the configured schedule, for as long as the materializer
    /// lives, whenever no command holds the gate.
    pub(crate) fn start_schedule(self: &Arc<Self>) {
        let Some(schedule) = self.config.schedule.clone() else {
            return;
        };
        let cleaner = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval_at(
                tokio::time::Instant::now() + schedule.start_offset,
                schedule.clean_period,
            );
            loop {
                ticker.tick().await;
                let Some(cleaner) = cleaner.upgrade() else {
                    return;
                };
                let Some(_run) = cleaner.gate.try_start_cleanup() else {
                    continue;
                };
                let command = CleanStaleArtifactsCommand::configured(
                    &cleaner.config,
                    cleaner.buck_out_abs_path.dupe(),
                    cleaner.daemon_dispatcher.dupe(),
                    false,
                    false,
                    Trigger::Scheduled,
                    Origin::DaemonSchedule,
                );
                let _ignored = cleaner.run(command).await;
            }
        });
    }

    /// Keeps the schedule from running, and stops a run in progress, until the guard is dropped.
    pub(crate) async fn prevent(&self) -> MaterializerBackgroundCleanupGuard {
        let prevention = self.gate.register_prevention();
        self.interrupt();
        prevention.wait_for_cleanup().await;
        MaterializerBackgroundCleanupGuard::new(prevention)
    }

    /// Stops the run in progress, if any, at the next path.
    fn interrupt(&self) {
        drop(self.running.lock().take());
    }

    pub(crate) async fn clean_stale(
        self: &Arc<Self>,
        args: CleanStaleArtifactsArgs,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        let command = CleanStaleArtifactsCommand::from_args(
            args,
            &self.config,
            self.buck_out_abs_path.dupe(),
            get_dispatcher(),
        );
        self.run(command)
            .await
            .map(Into::into)
            .map_err(|failure| failure.into_error())
    }

    pub(crate) async fn clean_scratch(
        &self,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        let observer = self.begin();
        let result = scratch_sweep(&self.io, CancellationContext::never_cancelled(), observer)
            .await
            .map(Into::into);
        self.interrupt();
        result
    }

    /// Makes a run the one in progress and hands out what tells it to stop.
    fn begin(&self) -> Arc<dyn LivelinessObserverSync> {
        let (observer, guard) = LivelinessGuard::create_sync();
        *self.running.lock() = Some(guard);
        observer
    }

    /// One run, reported the way the deferred core reports its runs. The run owns everything it
    /// touches, the cleaner included, which is what lets it be spawned or awaited from anywhere.
    fn run(
        self: &Arc<Self>,
        command: CleanStaleArtifactsCommand,
    ) -> BoxFuture<'static, CleanOutcome> {
        let cleaner = self.dupe();
        async move {
            let start = Instant::now();
            let disk_space = CleanDiskSpace::new(cleaner.buck_out_abs_path.as_deref());
            let observer = cleaner.begin();
            let mut result = cleaner.dupe().run_inner(command.clone(), observer).await;
            cleaner.interrupt();
            disk_space.record(&mut result);
            let trace_id = match command.trigger {
                Trigger::Scheduled => None,
                _ => Some(command.dispatcher.trace_id().dupe()),
            };
            command.dispatcher.instant_event(create_result(
                result.clone(),
                trace_id,
                command.dispatcher.daemon_id(),
                (Instant::now() - start).as_secs(),
                command.trigger,
                command.origin,
                command.policy_mode,
                command.tracked_only,
            ));
            result
        }
        .boxed()
    }

    async fn run_inner(
        self: Arc<Self>,
        command: CleanStaleArtifactsCommand,
        observer: Arc<dyn LivelinessObserverSync>,
    ) -> CleanOutcome {
        let command = &command;
        let scan_start = Instant::now();
        let mut stats = initial_stats(self.shared.sizes());
        let mut adaptive_outcome = if command.adaptive_low_disk.is_some() {
            AdaptiveOutcome::Unspecified
        } else {
            AdaptiveOutcome::Disabled
        };
        // The tracked roots as they are now. The scan runs against this copy, off the processor;
        // whatever changes meanwhile is caught when a path is taken.
        let roots = match self.sender.on_processor(|processor| processor.tree.iter()) {
            Ok(answer) => answer.await.map_err(|_| {
                internal_error!("The materializer stopped before answering the cleaner")
            }),
            Err(error) => Err(error),
        };
        let mut index = match roots {
            Ok(roots) => {
                let mut index = DiskTree::<()>::new();
                for (path, tracked) in roots {
                    let loaded = index.load(path, tracked);
                    debug_assert!(loaded, "the processor's roots do not nest");
                }
                index
            }
            Err(error) => {
                return Err(CleanFailure::new(
                    CleanFailurePhase::Scan,
                    stats,
                    adaptive_outcome,
                    error,
                ));
            }
        };

        let mut artifact_dirs = Vec::new();
        let mut scan_error: Option<PendingCleanFailure> = None;
        for dir_name in ARTIFACT_DIRS {
            let dir_path = self
                .io
                .buck_out_path()
                .join(ProjectRelativePathBuf::unchecked_new(
                    (*dir_name).to_owned(),
                ));
            let dir_abs = self.io.fs().resolve(&dir_path);
            match fs_util::try_exists(&dir_abs) {
                Ok(true) => artifact_dirs.push(dir_path),
                Ok(false) => {}
                Err(error) => {
                    let error = error.context(format!(
                        "Error checking clean-stale artifact directory `{dir_abs}`"
                    ));
                    scan_error.get_or_insert(PendingCleanFailure {
                        phase: CleanFailurePhase::RootDiscovery,
                        error,
                    });
                }
            }
        }
        if artifact_dirs.is_empty() {
            return match scan_error {
                Some(error) => {
                    stats.scan_duration_s = (Instant::now() - scan_start).as_secs();
                    Err(error.with_stats(stats, adaptive_outcome))
                }
                None => Ok(CleanResult::new(
                    CleanStaleResultKind::SkippedNoGenDir,
                    stats,
                    adaptive_outcome,
                )),
            };
        }

        let rules = Rules {
            started: self.started,
            keep_since_time: command.keep_since_time,
            rematerialization_deadline: rematerialization_deadline(self.rematerialization_ttl),
            upload_enabled: command.unmaterialize_upload.is_some(),
            store: self.store,
        };
        let mut found = Vec::new();
        if command.tracked_only {
            for (path, tracked) in index.iter() {
                found.push(rules.classify(path, &tracked, 0, false));
            }
        } else {
            // The index goes to the scanning thread and comes back with what was found.
            let scan = Scan {
                io: self.io.dupe(),
                index,
                rules,
                observer: observer.dupe(),
            };
            let (scanned, scanned_stats, error, returned) = match self
                .blocking_executor
                .execute_io_inline(move || Ok(scan.run(&artifact_dirs)))
                .boxed()
                .await
            {
                Ok(scanned) => scanned,
                Err(error) => {
                    return Err(CleanFailure::new(
                        CleanFailurePhase::Scan,
                        stats,
                        adaptive_outcome,
                        error,
                    ));
                }
            };
            found = scanned;
            index = returned;
            stats.add_scan(&scanned_stats);
            if scan_error.is_none() {
                scan_error = error;
            }
        }

        let mut record_unmaterialization_ineligibility = false;
        if let Some(params) = command.adaptive_low_disk.as_ref() {
            adaptive_outcome = match self.buck_out_abs_path.as_ref() {
                Some(buck_out_abs_path) => match disk_space_stats(&**buck_out_abs_path) {
                    Ok(disk_stats) => {
                        let mut params = params.clone();
                        params.unmaterialize_active &= self.rematerialization_ttl.is_some();
                        let adaptive = apply_adaptive_low_disk(
                            &mut found,
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

        stats.record_paths(&found, record_unmaterialization_ineligibility);
        stats.scan_duration_s = (Instant::now() - scan_start).as_secs();

        // Log limited number of untracked artifacts to avoid logging spikes if schema changes.
        for (path, file_type) in found
            .iter()
            .filter_map(|x| match x {
                FoundPath::Untracked(path, file_type, _) => Some((path, file_type)),
                _ => None,
            })
            .take(2000)
        {
            command.dispatcher.instant_event(buck2_data::UntrackedFile {
                path: path.to_string(),
                file_type: format!("{file_type:?}"),
            });
        }

        if !observer.is_alive_sync() {
            return Ok(CleanResult::new(
                CleanStaleResultKind::Interrupted,
                stats,
                adaptive_outcome,
            ));
        }

        // Every tracked root lies under one of the scanned directories, so a scan that found
        // none of them while the tree has some did not look where it should have; untracked
        // content is then not deleted on its say-so either.
        if scan_error.is_none()
            && stats.stale_artifact_count + stats.retained_artifact_count == 0
            && !index.iter().is_empty()
        {
            let error = tag_error!(
                "clean_stale_error",
                CleanStaleError {
                    db_size: index.iter().len(),
                    stats,
                }
                .into()
            );
            return Err(CleanFailure::new(
                CleanFailurePhase::StateValidation,
                stats,
                adaptive_outcome,
                error,
            ));
        }

        if command.dry_run {
            return match scan_error {
                Some(error) => Err(error.with_stats(stats, adaptive_outcome)),
                None => Ok(CleanResult::new(
                    CleanStaleResultKind::SkippedDryRun,
                    stats,
                    adaptive_outcome,
                )),
            };
        }

        let clean_start = Instant::now();
        let plan = plan(command, &index, &rules, found, &mut stats);
        let outcomes = self.dupe().sweep(plan, observer.dupe()).await;
        stats.record_materialized_sizes_after(self.shared.sizes());
        let mut first_error = scan_error;
        for (outcome, origin) in outcomes {
            let failure = stats.record_clean_outcome(outcome, origin);
            if first_error.is_none() {
                first_error = failure;
            }
        }
        stats.clean_duration_s = (Instant::now() - clean_start).as_secs();
        let kind = if observer.is_alive_sync() {
            CleanStaleResultKind::Finished
        } else {
            CleanStaleResultKind::Interrupted
        };
        match first_error {
            Some(error) => Err(error.with_stats(stats, adaptive_outcome)),
            None => Ok(CleanResult::new(kind, stats, adaptive_outcome)),
        }
    }

    /// Deletes what the plan says: the paths to take, then the active roots to upload first.
    async fn sweep(
        self: Arc<Self>,
        plan: Plan,
        observer: Arc<dyn LivelinessObserverSync>,
    ) -> Vec<(CleanPathOutcome, CleanupOrigin)> {
        let Plan {
            takes,
            uploads,
            mut outcomes,
        } = plan;
        let mut deletions = Vec::new();
        for batch in takes.chunks(TAKE_BATCH) {
            let requests = batch
                .iter()
                .map(|take| (take.path.clone(), take.expect))
                .collect();
            let taken = match self.sender.take_for_cleaning(requests).await {
                Ok(taken) => taken,
                Err(error) => {
                    for take in batch {
                        outcomes.push((
                            CleanPathOutcome::Failed {
                                size: take.size,
                                error: error.dupe(),
                            },
                            take.origin,
                        ));
                    }
                    continue;
                }
            };
            for (take, granted) in batch.iter().zip(taken) {
                match granted {
                    // Used, changed, held or waited for since the scan: not ours to delete.
                    None => tracing::trace!(path = %take.path, "skipping, in use"),
                    Some(granted) => deletions.push(self.dupe().delete(
                        granted,
                        take.path.clone(),
                        take.size,
                        take.origin,
                        observer.dupe(),
                    )),
                }
            }
        }
        outcomes.append(&mut join_all(deletions).await);
        outcomes.append(
            &mut join_all(uploads.into_iter().map(|upload| {
                self.dupe()
                    .unmaterialize_after_upload(upload, observer.dupe())
            }))
            .await,
        );
        outcomes
    }

    /// Deletes one path taken for cleaning, once nothing runs under it any more.
    async fn delete(
        self: Arc<Self>,
        granted: Granted,
        path: ProjectRelativePathBuf,
        size: u64,
        origin: CleanupOrigin,
        observer: Arc<dyn LivelinessObserverSync>,
    ) -> (CleanPathOutcome, CleanupOrigin) {
        let Granted { mut held, .. } = granted;
        held.settle_except(None).await;
        let outcome = clean_artifact(
            path,
            size,
            CancellationContext::never_cancelled(),
            &self.io,
            observer,
        )
        .await;
        drop(held);
        (outcome, origin)
    }

    /// Uploads an active root whose content is nowhere but on this disk, and deletes it once the
    /// CAS has confirmed having it. Held for reading while it uploads, so that readers go on
    /// meanwhile; any use of it in between, seen by its access time, calls the deletion off.
    async fn unmaterialize_after_upload(
        self: Arc<Self>,
        upload: Upload,
        observer: Arc<dyn LivelinessObserverSync>,
    ) -> (CleanPathOutcome, CleanupOrigin) {
        let Upload {
            path,
            size,
            last_access_time,
            entry,
            info,
        } = upload;
        let reading = match self
            .sender
            .hold_for_reading(vec![(path.clone(), Some(last_access_time))])
            .await
        {
            Ok(mut held) => match held.pop().flatten() {
                Some(guard) => guard,
                None => return state_changed(size),
            },
            Err(error) => {
                return (
                    CleanPathOutcome::Failed { size, error },
                    CleanupOrigin::Uploaded,
                );
            }
        };
        if !observer.is_alive_sync() {
            return (CleanPathOutcome::Interrupted(size), CleanupOrigin::Uploaded);
        }
        let action_entry = match entry.to_action(self.io.digest_config()) {
            Ok(action_entry) => action_entry,
            Err(error) => {
                return (
                    CleanPathOutcome::UploadFailed { size, error },
                    CleanupOrigin::Uploaded,
                );
            }
        };
        if let Err(error) = self
            .io
            .upload_materialized_artifact(path.clone(), action_entry, info.dupe())
            .await
        {
            return (
                CleanPathOutcome::UploadFailed { size, error },
                CleanupOrigin::Uploaded,
            );
        }
        if let Err(error) = self.dupe().confirm_in_cas(entry, info).await {
            return (
                CleanPathOutcome::UploadFailed { size, error },
                CleanupOrigin::Uploaded,
            );
        }
        drop(reading);

        let expect = Expect::Tracked { last_access_time };
        match self
            .sender
            .take_for_cleaning(vec![(path.clone(), expect)])
            .await
        {
            Ok(mut taken) => match taken.pop().flatten() {
                Some(granted) => {
                    self.delete(granted, path, size, CleanupOrigin::Uploaded, observer)
                        .await
                }
                None => state_changed(size),
            },
            Err(error) => (
                CleanPathOutcome::Failed { size, error },
                CleanupOrigin::Uploaded,
            ),
        }
    }

    /// Asks the CAS whether it holds every file of `entry`, which records their expirations for
    /// the refresher to keep alive; an upload only counts once this says yes.
    async fn confirm_in_cas(
        self: Arc<Self>,
        entry: DiskEntry,
        info: Arc<CasDownloadInfo>,
    ) -> buck2_error::Result<()> {
        let digests = entry.tracked_digests(self.io.digest_config().cas_digest_config());
        if digests.is_empty() {
            return Ok(());
        }
        let connection = self.io.re_client_manager().get_re_connection();
        let client = connection.get_client().with_use_case(info.re_use_case);
        let presence = client
            .check_presence(digests.clone(), NegativeCache::Bypassed, &info)
            .await?;
        match digests
            .iter()
            .zip(presence)
            .find(|(_, present)| present.is_none())
        {
            None => Ok(()),
            Some((digest, _)) => Err(internal_error!(
                "`{digest}` is not in the CAS right after its upload"
            )),
        }
    }
}

/// What the sweep does, decided over the scan's index before anything is awaited.
struct Plan {
    takes: Vec<Take>,
    uploads: Vec<Upload>,
    /// Outcomes decided without touching disk.
    outcomes: Vec<(CleanPathOutcome, CleanupOrigin)>,
}

/// A path to take for writing and delete.
struct Take {
    path: ProjectRelativePathBuf,
    expect: Expect,
    size: u64,
    origin: CleanupOrigin,
}

/// Sorts the found paths into what the sweep does with them: untracked paths and stale roots are
/// deleted; of the active roots the low-disk policy selected, those the CAS holds are deleted
/// and the rest uploaded first, within the upload size the policy allows.
fn plan(
    command: &CleanStaleArtifactsCommand,
    index: &DiskTree<()>,
    rules: &Rules,
    found: Vec<FoundPath>,
    stats: &mut CleanStaleStats,
) -> Plan {
    let mut plan = Plan {
        takes: Vec::new(),
        uploads: Vec::new(),
        outcomes: Vec::new(),
    };
    for path in found {
        match path {
            FoundPath::Untracked(path, _, size) => plan.takes.push(Take {
                path,
                expect: Expect::Untracked,
                size,
                origin: CleanupOrigin::Untracked,
            }),
            FoundPath::Tracked {
                path,
                size,
                state: TrackedState::Stale(_),
            } => {
                let Lookup::Exact(tracked) = index.lookup(&path) else {
                    continue;
                };
                plan.takes.push(Take {
                    path,
                    expect: Expect::Tracked {
                        last_access_time: tracked.last_access_time,
                    },
                    size,
                    origin: CleanupOrigin::Stale,
                });
            }
            FoundPath::Tracked {
                path,
                size,
                state: TrackedState::Unmaterialize,
            } => {
                let Lookup::Exact(tracked) = index.lookup(&path) else {
                    continue;
                };
                match rules.backing(&tracked.entry) {
                    Backing::Cas => plan.takes.push(Take {
                        path,
                        expect: Expect::Tracked {
                            last_access_time: tracked.last_access_time,
                        },
                        size,
                        origin: CleanupOrigin::Unmaterialized,
                    }),
                    Backing::LocalOnly | Backing::Expiring => {
                        let Some(upload) = command.unmaterialize_upload.as_ref() else {
                            continue;
                        };
                        if size > upload.max_bytes {
                            plan.outcomes.push((
                                CleanPathOutcome::UnmaterializationIneligible(
                                    UnmaterializationIneligibleArtifact {
                                        reason:
                                            UnmaterializationIneligibilityReason::UploadTooLarge,
                                        size,
                                    },
                                ),
                                CleanupOrigin::Uploaded,
                            ));
                            continue;
                        }
                        stats.unmaterialization_upload_attempted_artifact_count += 1;
                        stats.unmaterialization_upload_attempted_bytes += size;
                        plan.uploads.push(Upload {
                            path,
                            size,
                            last_access_time: tracked.last_access_time,
                            entry: tracked.entry.dupe(),
                            info: Arc::new(CasDownloadInfo::new_uploaded(upload.re_use_case)),
                        });
                    }
                }
            }
            FoundPath::Tracked { .. } => {}
        }
    }
    plan
}

/// The outcome of an unmaterialization called off because the root was used in the meantime.
fn state_changed(size: u64) -> (CleanPathOutcome, CleanupOrigin) {
    (
        CleanPathOutcome::UnmaterializationIneligible(UnmaterializationIneligibleArtifact {
            reason: UnmaterializationIneligibilityReason::StateChanged,
            size,
        }),
        CleanupOrigin::Uploaded,
    )
}

/// An active root the low-disk policy selected whose content is only on this disk.
struct Upload {
    path: ProjectRelativePathBuf,
    size: u64,
    last_access_time: Timestamp,
    entry: DiskEntry,
    info: Arc<CasDownloadInfo>,
}

/// What decides a tracked root's fate.
#[derive(Clone, Copy)]
struct Rules {
    started: Timestamp,
    keep_since_time: Timestamp,
    rematerialization_deadline: Timestamp,
    upload_enabled: bool,
    store: &'static BlobStore,
}

impl Rules {
    /// A tracked root as the policy sees it. `sized` is false for the tracked-only scan, which
    /// reports no sizes, like the deferred core's.
    fn classify(
        &self,
        path: ProjectRelativePathBuf,
        tracked: &Tracked,
        size: u64,
        sized: bool,
    ) -> FoundPath {
        let active = tracked.last_access_time >= self.started;
        let state = if !active && tracked.last_access_time < self.keep_since_time {
            TrackedState::Stale(StaleOrigin::Ttl)
        } else if !active {
            TrackedState::Retained {
                last_access_time: tracked.last_access_time,
                classification: tracked.classification,
            }
        } else {
            let unmaterialization_eligibility = if !sized {
                UnmaterializationEligibility::Ineligible(
                    UnmaterializationIneligibilityReason::NoRematerializationMethod,
                )
            } else if tracked.classification == ArtifactClassification::FinalOutput {
                UnmaterializationEligibility::Ineligible(
                    UnmaterializationIneligibilityReason::FinalOutput,
                )
            } else {
                match self.backing(&tracked.entry) {
                    Backing::Cas => UnmaterializationEligibility::Eligible,
                    Backing::Expiring => UnmaterializationEligibility::Ineligible(
                        UnmaterializationIneligibilityReason::RemoteTtlTooShort,
                    ),
                    Backing::LocalOnly if self.upload_enabled => {
                        UnmaterializationEligibility::EligibleAfterUpload
                    }
                    Backing::LocalOnly => UnmaterializationEligibility::Ineligible(
                        UnmaterializationIneligibilityReason::NoRematerializationMethod,
                    ),
                }
            };
            TrackedState::ActiveRetained {
                last_access_time: tracked.last_access_time,
                classification: tracked.classification,
                unmaterialization_eligibility,
            }
        };
        FoundPath::Tracked { path, size, state }
    }
}

/// Where a root's content can come back from once its files are gone from disk.
enum Backing {
    /// Every file has a CAS expiration that outlasts the deadline.
    Cas,
    /// Every file has a CAS expiration, but one of them not until the deadline.
    Expiring,
    /// A file the CAS was never seen to hold.
    LocalOnly,
}

impl Rules {
    /// Where `entry`'s content can come back from, by what the CAS has said about each file.
    fn backing(&self, entry: &DiskEntry) -> Backing {
        let mut backing = Backing::Cas;
        let mut local_only = false;
        entry.files(&mut |_, file| match self.store.cas_expiry(&file.digest) {
            None => local_only = true,
            Some(expires) if expires < self.rematerialization_deadline => {
                backing = Backing::Expiring;
            }
            Some(_) => {}
        });
        if local_only {
            Backing::LocalOnly
        } else {
            backing
        }
    }
}

/// What the scan counts, apart from the paths it finds.
#[derive(Default)]
struct ScanStats {
    unreadable: u64,
    invalid_filename: u64,
    tree_state_mismatch: u64,
    failed_directory: u64,
}

trait AddScan {
    fn add_scan(&mut self, scan: &ScanStats);
}

impl AddScan for CleanStaleStats {
    fn add_scan(&mut self, scan: &ScanStats) {
        self.scan_unreadable_count += scan.unreadable;
        self.skipped_unreadable_count += scan.unreadable;
        self.scan_invalid_filename_count += scan.invalid_filename;
        self.scan_tree_state_mismatch_count += scan.tree_state_mismatch;
        self.scan_failed_directory_count += scan.failed_directory;
    }
}

/// The walk of the artifact directories on disk against the index of tracked roots.
struct Scan<T> {
    io: Arc<T>,
    index: DiskTree<()>,
    rules: Rules,
    observer: Arc<dyn LivelinessObserverSync>,
}

impl<T: IoHandler> Scan<T> {
    /// Walks `dirs`; hands the index back with what it found.
    fn run(
        self,
        dirs: &[ProjectRelativePathBuf],
    ) -> (
        Vec<FoundPath>,
        ScanStats,
        Option<PendingCleanFailure>,
        DiskTree<()>,
    ) {
        let mut found = Vec::new();
        let mut stats = ScanStats::default();
        // Sizes are counted with the deferred core's helper, which keeps its own counters.
        let mut size_stats = CleanStaleStats::default();
        let mut error = None;
        for dir in dirs {
            tracing::trace!(dir = %self.io.fs().resolve(dir), "Scanning");
            let mut queue = vec![dir.clone()];
            while let Some(dir) = queue.pop() {
                if !self.observer.is_alive_sync() {
                    break;
                }
                if let Err(e) =
                    self.visit(&dir, &mut queue, &mut found, &mut stats, &mut size_stats)
                {
                    let e = e.context(format!(
                        "Error scanning clean-stale artifact directory `{}`",
                        self.io.fs().resolve(&dir)
                    ));
                    stats.failed_directory += 1;
                    error.get_or_insert(PendingCleanFailure {
                        phase: CleanFailurePhase::Scan,
                        error: e,
                    });
                    break;
                }
            }
        }
        stats.unreadable += size_stats.scan_unreadable_count;
        (found, stats, error, self.index)
    }

    /// One directory: untracked entries are found, tracked roots classified, and directories on
    /// the way to tracked roots queued.
    fn visit(
        &self,
        dir: &ProjectRelativePath,
        queue: &mut Vec<ProjectRelativePathBuf>,
        found: &mut Vec<FoundPath>,
        stats: &mut ScanStats,
        size_stats: &mut CleanStaleStats,
    ) -> buck2_error::Result<()> {
        let read_dir = match self.io.read_dir(&self.io.fs().resolve(dir)) {
            Ok(read_dir) => read_dir,
            Err(e) if e.has_tag(ErrorTag::IoPermissionDenied) => {
                // Tracked roots beneath an unreadable directory stay as they are: they cannot
                // be deleted through it anyway, and a later scan sees them again.
                stats.unreadable += 1;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        for child in read_dir {
            let child = match child {
                Ok(child) => child,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    stats.unreadable += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let file_name = child.file_name();
            let Some(file_name) = file_name.to_str().and_then(|f| FileName::new(f).ok()) else {
                // Nothing tracked has a name like this; deleting it is not supported yet.
                stats.invalid_filename += 1;
                continue;
            };
            let path = dir.join(file_name);
            // `file_type()` falls back to an `lstat` on filesystems that don't report `d_type`
            // (NFS, FUSE), which fails without search permission on the parent even though
            // `read_dir` succeeded.
            let file_type = match child.file_type() {
                Ok(file_type) => FileType::from(file_type),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    stats.unreadable += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            match self.index.lookup(&path) {
                Lookup::Untracked => {
                    if file_type.is_dir() && self.index.contains_roots_below(&path) {
                        queue.push(path);
                    } else {
                        tracing::trace!(path = %path, file_type = ?file_type, "marking as untracked");
                        let size = get_size(&child.path(), size_stats)?;
                        found.push(FoundPath::Untracked(path, file_type, size));
                    }
                }
                Lookup::Exact(tracked) => {
                    let is_dir = matches!(tracked.entry, DiskEntry::Dir(_));
                    if is_dir != file_type.is_dir() {
                        // What is on disk is not what the tree says, which is not clean stale's
                        // to fix.
                        tracing::trace!(path = %path, file_type = ?file_type, "skipping");
                        stats.tree_state_mismatch += 1;
                        continue;
                    }
                    let tracked = tracked.dupe();
                    let size = tracked.entry.size();
                    found.push(self.rules.classify(path, &tracked, size, true));
                }
                Lookup::Inside { .. } | Lookup::UnderLeaf { .. } => {
                    // The walk never descends into a root, so this is a root the walk did not
                    // come through: a directory on disk where the tree has a file, or the like.
                    stats.tree_state_mismatch += 1;
                }
            }
        }
        Ok(())
    }
}
