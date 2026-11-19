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

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::file_ops::baseline::FileSystemBaseline;
use buck2_common::file_ops::baseline::SetFileSystemBaseline;
use buck2_common::file_ops::io::FileChangeTracker;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::rollout_percentage::RolloutPercentage;
use buck2_error::internal_error;
use buck2_events::dispatch::span_async;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_util::process::async_background_command;
use dice::DiceTransactionUpdater;
use dupe::Dupe;
use tracing::info;
use watchman_client::expr::Expr;
use watchman_client::prelude::Connector;
use watchman_client::prelude::FileType;

use crate::dep_files::DepFileCache;
use crate::file_watcher::FileWatcher;
use crate::file_watcher::SyncOutcome;
use crate::ignores::FileWatcherIgnores;
use crate::mergebase::Mergebase;
use crate::stats::FileWatcherStats;
use crate::watchman::core::SyncableQuery;
use crate::watchman::core::SyncableQueryProcessor;
use crate::watchman::core::WatchmanEvent;
use crate::watchman::core::WatchmanEventType;
use crate::watchman::core::WatchmanKind;
use crate::watchman::utils::find_first_valid_parent;

struct WatchmanQueryProcessor {
    ignores: FileWatcherIgnores,
    empty_on_fresh_instance: bool,
    report_global_rev: bool,
    last_mergebase: Option<String>,
    last_mergebase_global_rev: Option<u64>,
    last_mergebase_timestamp: Option<u64>,
    dep_file_cache: Arc<dyn DepFileCache>,
    /// What the changes recorded on the updater are relative to.
    baseline: FileSystemBaseline,
}

/// Used in process_one_change
#[derive(Debug)]
enum ChangeEvent<'a> {
    Watchman(&'a WatchmanEvent),
    SyntheticDirectoryChange,
}

impl WatchmanQueryProcessor {
    async fn process_events_impl(
        &self,
        mut ctx: DiceTransactionUpdater,
        events: Vec<WatchmanEvent>,
        base_stats: buck2_data::FileWatcherStats,
    ) -> buck2_error::Result<(buck2_data::FileWatcherStats, DiceTransactionUpdater)> {
        let mut handler = FileChangeTracker::new();

        let mut stats = FileWatcherStats::new(base_stats, events.len());

        for ev in events {
            // If the path is invalid, then walk up all the way until you find a valid dir to
            // invalidate listings. We don't need to invalidate the file itself, as we can't
            // read invalid files.

            let (path, event) = match ProjectRelativePath::new(&ev.path) {
                Ok(path) => (path, ChangeEvent::Watchman(&ev)),
                Err(_) => {
                    // If we error out here then we might miss other changes. This seems like
                    // it shouldn't happen, since the empty path should always be a valid path.
                    let path = find_first_valid_parent(&ev.path)
                        .ok_or_else(|| {
                            internal_error!(
                                "Invalid path had no valid parent: `{}`",
                                ev.path.display()
                            )
                        })
                        .unwrap();

                    (path, ChangeEvent::SyntheticDirectoryChange)
                }
            };

            self.process_one_change(path, event, &mut handler, &mut stats)?;
        }

        let stats = stats.finish();
        handler.write_to_dice(&mut ctx)?;
        ctx.set_file_system_baseline(self.baseline.dupe())?;

        Ok((stats, ctx))
    }

    fn process_one_change(
        &self,
        path: &ProjectRelativePath,
        ev: ChangeEvent<'_>,
        handler: &mut FileChangeTracker,
        stats: &mut FileWatcherStats,
    ) -> buck2_error::Result<()> {
        // Watchman cookie files are synchronization markers, not real source changes.
        if crate::is_watchman_cookie(path) {
            stats.add_ignored(1);
            return Ok(());
        }

        let path = IProjectRelativePath::intern_directly(&path);

        let ignore = self.ignores.is_ignored(path);

        info!("Watchman: {:?} (ignore = {})", ev, ignore);

        if ignore {
            stats.add_ignored(1);
        } else {
            let log_kind;
            let log_event;

            match ev {
                ChangeEvent::Watchman(ev) => match (&ev.kind, &ev.event) {
                    (WatchmanKind::File, typ) => {
                        log_kind = buck2_data::FileWatcherKind::File;
                        match typ {
                            WatchmanEventType::Modify => {
                                handler.file_contents_changed(path);
                                log_event = buck2_data::FileWatcherEventType::Modify;
                            }
                            WatchmanEventType::Create => {
                                handler.file_added_or_removed(path);
                                log_event = buck2_data::FileWatcherEventType::Create;
                            }
                            WatchmanEventType::Delete => {
                                handler.file_added_or_removed(path);
                                log_event = buck2_data::FileWatcherEventType::Delete;
                            }
                        }
                    }
                    (WatchmanKind::Directory, typ) => {
                        log_kind = buck2_data::FileWatcherKind::Directory;
                        match typ {
                            WatchmanEventType::Modify => {
                                handler.dir_entries_changed_for_watchman_bug(path);
                                log_event = buck2_data::FileWatcherEventType::Modify;
                            }
                            WatchmanEventType::Create => {
                                handler.dir_added_or_removed(path);
                                log_event = buck2_data::FileWatcherEventType::Create;
                            }
                            WatchmanEventType::Delete => {
                                handler.dir_added_or_removed(path);
                                log_event = buck2_data::FileWatcherEventType::Delete;
                            }
                        }
                    }
                    (WatchmanKind::Symlink, typ) => {
                        log_kind = buck2_data::FileWatcherKind::Symlink;
                        match typ {
                            WatchmanEventType::Modify => {
                                handler.file_contents_changed(path);
                                log_event = buck2_data::FileWatcherEventType::Modify;
                            }
                            WatchmanEventType::Create => {
                                handler.file_added_or_removed(path);
                                log_event = buck2_data::FileWatcherEventType::Create;
                            }
                            WatchmanEventType::Delete => {
                                handler.file_added_or_removed(path);
                                log_event = buck2_data::FileWatcherEventType::Delete;
                            }
                        }
                    }
                },
                ChangeEvent::SyntheticDirectoryChange => {
                    log_kind = buck2_data::FileWatcherKind::Directory;
                    log_event = buck2_data::FileWatcherEventType::Modify;
                    // FIXME(JakobDegen): Add comment explaining why this is needed.
                    handler.dir_entries_changed_force_invalidate(path);
                }
            };

            stats.add(path, log_event, log_kind);
        }

        Ok(())
    }
}

struct RevisionDetails {
    global_rev: u64,
    timestamp: u64,
}

async fn try_fetch_revision_details(hash: &str) -> Option<RevisionDetails> {
    // There's a variety of ways in which this might go wrong: `PATH` is messed up, this somehow got
    // turned on in a non-`hg` repo, etc. To make sure we don't fail any builds from this, ignore
    // all errors.
    let command = async_background_command("hg")
        .args([
            "log",
            "-r",
            hash,
            "-T",
            "{get(extras, \"global_rev\")}\n{date}",
        ])
        .env("HPGPLAIN", "1")
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_millis(500), command)
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let (global_rev_string, time_string) = stdout.trim().split_once('\n')?;
    Some(RevisionDetails {
        global_rev: global_rev_string.parse::<u64>().ok()?,
        timestamp: time_string.parse::<f64>().ok()? as u64, // hg returns the fractional seconds
    })
}

#[async_trait]
impl SyncableQueryProcessor for WatchmanQueryProcessor {
    type Output = buck2_data::FileWatcherStats;
    type Payload = DiceTransactionUpdater;

    async fn process_events(
        &mut self,
        dice: DiceTransactionUpdater,
        events: Vec<WatchmanEvent>,
        mergebase: &Option<String>,
        watchman_version: Option<String>,
    ) -> buck2_error::Result<(Self::Output, DiceTransactionUpdater)> {
        self.last_mergebase = mergebase.clone();
        self.process_events_impl(
            dice,
            events,
            buck2_data::FileWatcherStats {
                branched_from_revision: self.last_mergebase.clone(),
                branched_from_global_rev: self.last_mergebase_global_rev,
                branched_from_revision_timestamp: self.last_mergebase_timestamp,
                watchman_version,
                ..Default::default()
            },
        )
        .await
    }

    async fn on_fresh_instance(
        &mut self,
        mut ctx: DiceTransactionUpdater,
        events: Vec<WatchmanEvent>,
        mergebase: &Option<String>,
        watchman_version: Option<String>,
        events_since_mergebase: bool,
    ) -> buck2_error::Result<(Self::Output, DiceTransactionUpdater)> {
        let has_new_mergebase = self.last_mergebase.as_ref() != mergebase.as_ref();

        let clear_dep_files = has_new_mergebase;

        // We'll clear dep files if the mergebase has changed, which means our dep files are likely
        // irrelevant.
        //
        // This is imperfect. If the user rebased from yesterday's stable to today's stable, then
        // flushing is the right thing to do. In contrast, if they rebased from X to X's parent,
        // then it probably isn't. The consequences of flushing in the latter case aren't as bad
        // (where we'll rebuild things our dep files *could* have avoided) as not flushing in the
        // former (where we'll fetch loads of dep files that all miss), so we err on the side of
        // being safe and drop them when the mergebase changes.
        //
        // We do retain dep files that were produced locally, since we don't need to fetch them as
        // they are already on disk.
        if clear_dep_files {
            self.dep_file_cache.clear_non_local();
        }

        self.last_mergebase = mergebase.clone();

        if let Some(hash) = self.last_mergebase.as_ref() {
            if self.report_global_rev {
                if let Some(revision_details) = try_fetch_revision_details(hash).await {
                    self.last_mergebase_global_rev = Some(revision_details.global_rev);
                    self.last_mergebase_timestamp = Some(revision_details.timestamp);
                } else {
                    self.last_mergebase_global_rev = None;
                    self.last_mergebase_timestamp = None;
                }
            }
        }

        self.baseline = match mergebase {
            Some(mergebase) if events_since_mergebase => {
                FileSystemBaseline::Revision(Arc::from(mergebase.as_str()))
            }
            _ => FileSystemBaseline::unique(),
        };

        let mut base_stats = buck2_data::FileWatcherStats {
            fresh_instance: true,
            branched_from_revision: mergebase.clone(),
            branched_from_global_rev: self.last_mergebase_global_rev,
            branched_from_revision_timestamp: self.last_mergebase_timestamp,
            watchman_version,
            fresh_instance_data: Some(buck2_data::FreshInstance {
                new_mergebase: has_new_mergebase,
                cleared_dice: true,
                cleared_dep_files: clear_dep_files,
            }),
            ..Default::default()
        };

        if self.empty_on_fresh_instance {
            base_stats.incomplete_events_reason = Some("Fresh instance".to_owned());
            ctx.set_file_system_baseline(self.baseline.dupe())?;
            Ok((base_stats, ctx))
        } else {
            self.process_events_impl(ctx, events, base_stats).await
        }
    }
}

#[derive(Allocative)]
pub(crate) struct WatchmanFileWatcher {
    #[allocative(skip)]
    query: SyncableQuery<buck2_data::FileWatcherStats, DiceTransactionUpdater>,
}

/// The watchman query is constructed once on daemon startup. It is an unfiltered watchman query
/// over the entire project (though, like all watchman queries, still filtered by the
/// .watchmanconfig itself). Before any new computation request is started, it will be synced to
/// ensure that any recent changes are flushed and visible to the computation.
impl WatchmanFileWatcher {
    pub(crate) fn new(
        project_root: &AbsNormPath,
        root_config: &LegacyBuckConfig,
        ignores: FileWatcherIgnores,
        dep_file_cache: Arc<dyn DepFileCache>,
    ) -> buck2_error::Result<Self> {
        let watchman_merge_base = root_config
            .get(BuckconfigKeyRef {
                section: "project",
                property: "watchman_merge_base",
            })
            .map(|s| s.to_owned());

        let empty_on_fresh_instance = if watchman_merge_base.is_some() {
            // double negative here because we'd prefer that rollout changes config value from false->true.
            !root_config
                .parse::<RolloutPercentage>(BuckconfigKeyRef {
                    section: "buck2",
                    property: "disable_watchman_empty_on_fresh_instance",
                })?
                .unwrap_or_else(RolloutPercentage::never)
                .roll()
        } else {
            // When not using scm-aware queries, fresh instances would list every file in
            // the repo. That's a lot and not very useful.
            // TODO(cjhopman): If we find we get a lot of value from the invalidation tracking that
            // getting changed files since branch point gives us, we could try to get a better
            // approach here for the non scm-aware case.
            true
        };

        let report_global_rev = root_config
            .parse::<bool>(BuckconfigKeyRef {
                section: "buck2",
                property: "watchman_report_global_rev",
            })?
            .unwrap_or(false);

        let dice_clear_on_mergebase_change =
            crate::file_watcher::dice_clear_on_mergebase_change(root_config)?;

        let query = SyncableQuery::new(
            Connector::new(),
            project_root,
            Expr::Any(vec![
                Expr::FileType(FileType::Regular),
                Expr::FileType(FileType::Directory),
                Expr::FileType(FileType::Symlink),
            ]),
            Box::new(WatchmanQueryProcessor {
                ignores,
                empty_on_fresh_instance,
                report_global_rev,
                last_mergebase: None,
                last_mergebase_global_rev: None,
                last_mergebase_timestamp: None,
                dep_file_cache,
                baseline: FileSystemBaseline::unique(),
            }),
            watchman_merge_base,
            empty_on_fresh_instance,
            dice_clear_on_mergebase_change,
        )?;

        Ok(Self { query })
    }
}

#[async_trait]
impl FileWatcher for WatchmanFileWatcher {
    async fn sync(
        &self,
        dice: DiceTransactionUpdater,
    ) -> buck2_error::Result<(DiceTransactionUpdater, Mergebase, SyncOutcome)> {
        span_async(
            buck2_data::FileWatcherStart {
                provider: buck2_data::FileWatcherProvider::Watchman as i32,
            },
            async {
                let (stats, res) = match self.query.sync(dice).await {
                    Ok((stats, dice)) => {
                        let mergebase = Mergebase(Arc::new(stats.branched_from_revision.clone()));
                        let outcome = SyncOutcome::reported_by(&stats);
                        ((Some(stats)), Ok((dice, mergebase, outcome)))
                    }
                    Err(e) => (None, Err(e)),
                };
                (res, buck2_data::FileWatcherEnd { stats })
            },
        )
        .await
    }
}
