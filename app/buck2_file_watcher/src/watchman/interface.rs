/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
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
use buck2_hash::BuckMutSet;
use buck2_util::process::async_background_command;
use dice::DiceTransactionUpdater;
use dupe::Dupe;
use tracing::info;
use watchman_client::expr::Expr;
use watchman_client::prelude::Connector;
use watchman_client::prelude::FileType;
use watchman_client::prelude::ResolvedRoot;

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
    /// Whether watchman watches the root through EdenFS. What it reports there is what EdenFS's
    /// journal sums the changes of a path up to, which differs from its other watchers:
    ///
    /// - A path that was removed and added again is reported as modified, whatever it was before
    ///   and whatever it is now.
    /// - Nothing is reported for what is in a directory that was renamed. A directory that
    ///   another was renamed onto is reported as modified, and that is all. So is a directory
    ///   whose attributes changed.
    /// - A change of commit reports every directory between the root and a file it touches: as
    ///   created if the commit before had no file below it, as modified otherwise. These
    ///   directories are the ones they were, with what is in them.
    ///
    /// So a directory reported as created or modified is taken for another than the one buck2
    /// read at the path, unless it is reported the way a change of commit reports one: see
    /// `dirs_that_may_be_others`.
    on_eden: bool,
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

        let may_be_new_dirs = if self.on_eden {
            dirs_that_may_be_others(&events)
        } else {
            modified_dirs_in_modified_dirs(&events)
        };
        // On EdenFS a directory that became a file is reported as a modified file. Unless it
        // was empty, what was in it is reported as deleted.
        let was_dir: BuckMutSet<&Path> = if self.on_eden {
            events.iter().filter_map(|ev| ev.path.parent()).collect()
        } else {
            BuckMutSet::default()
        };

        for ev in &events {
            // If the path is invalid, then walk up all the way until you find a valid dir to
            // invalidate listings. We don't need to invalidate the file itself, as we can't
            // read invalid files.

            let (path, event) = match ProjectRelativePath::new(&ev.path) {
                Ok(path) => (path, ChangeEvent::Watchman(ev)),
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

            // Whether what watchman reports as modified may be new at its path.
            let may_be_new = match ev.kind {
                WatchmanKind::Directory => may_be_new_dirs.contains(ev.path.as_path()),
                WatchmanKind::File | WatchmanKind::Symlink => was_dir.contains(ev.path.as_path()),
            };
            self.process_one_change(path, event, may_be_new, &mut handler, &mut stats)?;
        }

        let stats = stats.finish();
        handler.write_to_dice(&mut ctx)?;
        ctx.set_file_system_baseline(self.baseline.dupe())?;

        Ok((stats, ctx))
    }

    /// A directory that may be new at its path. On EdenFS nothing is reported for what is in
    /// it: see `on_eden`.
    fn dir_may_be_new(&self, handler: &mut FileChangeTracker, path: IProjectRelativePath) {
        if self.on_eden {
            handler.dir_tree_added_or_removed(path);
        } else {
            handler.dir_added_or_removed(path);
        }
    }

    fn process_one_change(
        &self,
        path: &ProjectRelativePath,
        ev: ChangeEvent<'_>,
        may_be_new: bool,
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
                                file_modified(handler, path, may_be_new);
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
                                if may_be_new {
                                    self.dir_may_be_new(handler, path);
                                }
                                handler.dir_entries_changed_for_watchman_bug(path);
                                log_event = buck2_data::FileWatcherEventType::Modify;
                            }
                            WatchmanEventType::Create => {
                                if may_be_new {
                                    self.dir_may_be_new(handler, path);
                                } else {
                                    handler.dir_added_or_removed(path);
                                }
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
                                file_modified(handler, path, may_be_new);
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

/// The directories among `events` that watchman reports as modified in a directory it reports as
/// modified too. One of these may be new at its path: watchman reports a path with what it is
/// now, so a file that became a directory is a modified directory, in a directory that has an
/// entry changed.
fn modified_dirs_in_modified_dirs(events: &[WatchmanEvent]) -> BuckMutSet<&Path> {
    let modified: BuckMutSet<&Path> = events
        .iter()
        .filter(|ev| {
            matches!(
                (&ev.kind, &ev.event),
                (WatchmanKind::Directory, WatchmanEventType::Modify)
            )
        })
        .map(|ev| ev.path.as_path())
        .collect();
    modified
        .iter()
        .copied()
        .filter(|dir| dir.parent().is_some_and(|dir| modified.contains(dir)))
        .collect()
}

/// A file or a symlink that watchman reports as modified.
fn file_modified(handler: &mut FileChangeTracker, path: IProjectRelativePath, may_be_new: bool) {
    if may_be_new {
        handler.file_added_or_removed(path);
    } else {
        handler.file_contents_changed(path);
    }
}

/// The directories among `events`, from watchman on EdenFS, that may be others than the ones
/// buck2 read at their paths: those reported as created or modified, but for the ones that are
/// reported the way a change of commit reports the directories above a file it touches.
fn dirs_that_may_be_others(events: &[WatchmanEvent]) -> BuckMutSet<&Path> {
    let dirs: BuckMutSet<&Path> = events
        .iter()
        .filter(|ev| {
            matches!(
                (&ev.kind, &ev.event),
                (
                    WatchmanKind::Directory,
                    WatchmanEventType::Create | WatchmanEventType::Modify
                )
            )
        })
        .map(|ev| ev.path.as_path())
        .collect();
    let with_entry: BuckMutSet<&Path> = events.iter().filter_map(|ev| ev.path.parent()).collect();
    dirs.iter()
        .copied()
        .filter(|dir| !above_commit_change(dir, &dirs, &with_entry))
        .collect()
}

/// Whether a change of commit may be what reports `dir`, one of the directories `dirs` that
/// watchman on EdenFS reports as created or modified. A change of commit reports each
/// directory between the root and a file it touches, so the directory that such a directory is
/// in is reported too. A directory reported for another reason is in one that is not, unless
/// something else changed that one. At the top of the project there is no directory to go by,
/// and an entry of the directory has to be reported.
///
/// Both ways of being wrong are left. A directory that another was renamed onto is taken for
/// one above a commit change when the directory it is in is reported with it, or at the top
/// of the project when an entry of it is, and what is below it is not read again. A directory
/// at the top of the project that a change of commit reports without an entry is taken for
/// another, and what is below it is read again.
fn above_commit_change(
    dir: &Path,
    dirs: &BuckMutSet<&Path>,
    with_entry: &BuckMutSet<&Path>,
) -> bool {
    match dir.parent() {
        Some(parent) if parent != Path::new("") => dirs.contains(parent),
        _ => with_entry.contains(dir),
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

    fn on_connect(&mut self, root: &ResolvedRoot) {
        self.on_eden = root.watcher() == "eden";
    }

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
                on_eden: false,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn event(event: WatchmanEventType, kind: WatchmanKind, path: &str) -> WatchmanEvent {
        WatchmanEvent {
            kind,
            event,
            path: PathBuf::from(path),
        }
    }

    fn modified_dir(path: &str) -> WatchmanEvent {
        event(WatchmanEventType::Modify, WatchmanKind::Directory, path)
    }

    fn paths(dirs: BuckMutSet<&Path>) -> Vec<&Path> {
        let mut dirs: Vec<_> = dirs.into_iter().collect();
        dirs.sort();
        dirs
    }

    /// What watchman reports for `rm d/entry; mkdir d/entry; touch d/entry/inside`.
    #[test]
    fn a_file_that_became_a_directory_is_a_modified_directory_in_a_modified_directory() {
        let events = [
            modified_dir("d"),
            modified_dir("d/entry"),
            event(
                WatchmanEventType::Create,
                WatchmanKind::File,
                "d/entry/inside",
            ),
        ];
        assert_eq!(
            paths(modified_dirs_in_modified_dirs(&events)),
            [Path::new("d/entry")]
        );
    }

    /// A directory that has an entry added or removed is reported as modified, and the directory
    /// it is in is not. Watchman reports nothing for the root.
    #[test]
    fn a_directory_with_an_entry_changed_is_not_in_a_modified_directory() {
        let events = [
            modified_dir("top"),
            event(WatchmanEventType::Create, WatchmanKind::File, "top/created"),
            modified_dir("d/sub"),
            event(
                WatchmanEventType::Delete,
                WatchmanKind::File,
                "d/sub/removed",
            ),
            event(WatchmanEventType::Create, WatchmanKind::Directory, "d/new"),
            modified_dir("d/new/sub"),
        ];
        assert_eq!(
            paths(modified_dirs_in_modified_dirs(&events)),
            Vec::<&Path>::new()
        );
    }

    /// What watchman reports on EdenFS for `mv files/d files/old; mv files/incoming files/d`.
    fn directory_replaced() -> Vec<WatchmanEvent> {
        vec![
            event(
                WatchmanEventType::Create,
                WatchmanKind::Directory,
                "files/old",
            ),
            event(
                WatchmanEventType::Delete,
                WatchmanKind::File,
                "files/incoming",
            ),
            modified_dir("files/d"),
        ]
    }

    /// What watchman reports on EdenFS for a change of commit that adds `files/d/added`,
    /// modifies `gen/out.pyc`, is the first to have a file below `new`, and gives a file below
    /// `same/sub` another id with the same contents.
    fn commit_changed() -> Vec<WatchmanEvent> {
        vec![
            modified_dir("files"),
            modified_dir("files/d"),
            event(
                WatchmanEventType::Create,
                WatchmanKind::File,
                "files/d/added",
            ),
            modified_dir("gen"),
            event(WatchmanEventType::Modify, WatchmanKind::File, "gen/out.pyc"),
            event(WatchmanEventType::Create, WatchmanKind::Directory, "new"),
            event(
                WatchmanEventType::Create,
                WatchmanKind::Directory,
                "new/sub",
            ),
            event(
                WatchmanEventType::Create,
                WatchmanKind::File,
                "new/sub/tracked",
            ),
            modified_dir("same"),
            modified_dir("same/sub"),
        ]
    }

    #[test]
    fn on_eden_a_replaced_directory_may_be_another() {
        assert_eq!(
            paths(dirs_that_may_be_others(&directory_replaced())),
            [Path::new("files/d"), Path::new("files/old")]
        );
    }

    /// A file written below the directory after the rename is reported, and the directory that
    /// the directory is in is not.
    #[test]
    fn on_eden_a_replaced_directory_with_a_file_written_below_it_may_be_another() {
        for written in ["files/d/pkg/written", "files/d/written"] {
            let mut events = directory_replaced();
            events.push(event(
                WatchmanEventType::Create,
                WatchmanKind::File,
                written,
            ));
            assert_eq!(
                paths(dirs_that_may_be_others(&events)),
                [Path::new("files/d"), Path::new("files/old")],
                "{written}"
            );
        }
    }

    #[test]
    fn on_eden_a_directory_above_a_commit_change_is_not_another() {
        assert_eq!(
            paths(dirs_that_may_be_others(&commit_changed())),
            Vec::<&Path>::new()
        );
    }

    /// Nothing is reported with a directory at the top of the project that tells a change of
    /// commit from a rename, unless an entry of it is.
    #[test]
    fn on_eden_a_directory_at_the_top_reported_alone_may_be_another() {
        let events = [modified_dir("top")];
        assert_eq!(paths(dirs_that_may_be_others(&events)), [Path::new("top")]);
    }

    /// A directory at the top of the project that another was renamed onto, and that a file
    /// was then written into, is reported as a directory above a change of commit is.
    #[test]
    fn on_eden_a_directory_at_the_top_reported_with_an_entry_is_not_another() {
        let events = [
            modified_dir("top"),
            event(WatchmanEventType::Create, WatchmanKind::File, "top/written"),
        ];
        assert_eq!(paths(dirs_that_may_be_others(&events)), Vec::<&Path>::new());
    }
}
