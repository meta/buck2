/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::mem;
use std::sync::Arc;
use std::sync::Mutex;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::file_ops::baseline::FileSystemBaseline;
use buck2_common::file_ops::baseline::SetFileSystemBaseline;
use buck2_common::file_ops::io::FileChangeTracker;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project::ProjectRoot;
use buck2_data::FileWatcherEventType;
use buck2_data::FileWatcherKind;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::dispatch::span_async;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_hash::BuckMutInsertionSet;
use dice::DiceTransactionUpdater;
use dupe::Dupe;
use notify::EventKind;
use notify::RecommendedWatcher;
use notify::Watcher;
use notify::event::CreateKind;
use notify::event::MetadataKind;
use notify::event::ModifyKind;
use notify::event::RemoveKind;
use tracing::debug;
use tracing::info;

use crate::file_watcher::FileWatcher;
use crate::file_watcher::SyncOutcome;
use crate::ignores::FileWatcherIgnores;
use crate::mergebase::Mergebase;
use crate::stats::FileWatcherStats;

fn ignore_event_kind(event_kind: EventKind) -> bool {
    match event_kind {
        EventKind::Access(_) => true,
        // Only the access time can change without anything buck2 reads changing. `chmod` arrives
        // as `Metadata(Any)` from every notify backend, never as `Permissions`.
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => true,
        _ => false,
    }
}

/// Buffer containing the events that have happened since we last got a message.
/// Used to dedupe events, since notify sends a notification on every change.
#[derive(Allocative)]
struct NotifyFileData {
    ignored: u64,
    #[allocative(skip)]
    events: BuckMutInsertionSet<(IProjectRelativePath, EventKind)>,
    /// Whether file system changes were missed
    missed_events: bool,
}

impl NotifyFileData {
    fn new() -> Self {
        Self {
            ignored: 0,
            events: BuckMutInsertionSet::new(),
            missed_events: false,
        }
    }

    fn process(
        &mut self,
        event: notify::Result<notify::Event>,
        root: &ProjectRoot,
        ignores: &FileWatcherIgnores,
    ) -> buck2_error::Result<()> {
        let event =
            event.map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::NotifyWatcher))?;

        // Checked before the path loop: the kernel-overflow rescan signal can arrive with no
        // paths attached (platform dependent), and a missed rescan means the daemon keeps
        // answering from a stale graph.
        if event.need_rescan() {
            self.missed_events = true;
            debug!("FileWatcher: File change events were missed");
        }

        for path in &event.paths {
            // Testing shows that we get absolute paths back from the `notify` library.
            // It's not documented though.
            let path = root.relativize(AbsNormPath::new(&path)?)?;
            let path = IProjectRelativePath::intern_directly(path.as_ref());

            // We ignore the buck-out prefix, as those are uninteresting events caused by us.
            // We also ignore other buck-out directories, as if you have two isolation dirs running at once, they are not interesting.
            // We do this in the notify-watcher, rather than a generic layer, as watchman users should configure
            // to ignore buck-out, to reduce the number of events, rather than hiding them later.
            if path.starts_with(InvocationPaths::buck_out_dir_prefix()) {
                // We don't want to event add them as ignored events, since they are super common
                // and very boring
                continue;
            }

            let ignore = ignores.is_ignored(path);

            info!(
                "FileWatcher: {:?} {:?} (ignore = {})",
                path, &event.kind, ignore
            );

            if ignore || ignore_event_kind(event.kind) {
                self.ignored += 1;
            } else {
                self.events.insert((path, event.kind));
            }
        }
        Ok(())
    }

    fn sync(self) -> (buck2_data::FileWatcherStats, Option<FileChangeTracker>) {
        // The changes that go into the DICE transaction
        let mut changed = FileChangeTracker::new();
        // If we missed events, the sync reports a fresh instance and the daemon starts over.
        // Surface that to telemetry/UI the same way the watchman path does.
        let base = if self.missed_events {
            buck2_data::FileWatcherStats {
                fresh_instance: true,
                fresh_instance_data: Some(buck2_data::FreshInstance {
                    new_mergebase: false,
                    cleared_dice: true,
                    cleared_dep_files: false,
                }),
                incomplete_events_reason: Some(
                    "notify dropped events (kernel queue overflow)".to_owned(),
                ),
                ..Default::default()
            }
        } else {
            Default::default()
        };
        let mut stats = FileWatcherStats::new(base, self.events.len());
        stats.add_ignored(self.ignored);

        for (path, event_kind) in self.events {
            match event_kind {
                EventKind::Create(create_kind) => match create_kind {
                    CreateKind::File => {
                        stats.add(path, FileWatcherEventType::Create, FileWatcherKind::File);
                        changed.file_added_or_removed(path);
                    }
                    CreateKind::Folder => {
                        stats.add(
                            path,
                            FileWatcherEventType::Create,
                            FileWatcherKind::Directory,
                        );
                        changed.dir_added_or_removed(path);
                    }
                    CreateKind::Any | CreateKind::Other => {
                        stats.add(path, FileWatcherEventType::Create, FileWatcherKind::File);
                        changed.file_added_or_removed(path);

                        stats.add(
                            path,
                            FileWatcherEventType::Create,
                            FileWatcherKind::Directory,
                        );
                        changed.dir_added_or_removed(path);
                    }
                },
                EventKind::Modify(modify_kind) => match modify_kind {
                    ModifyKind::Data(_) | ModifyKind::Metadata(_) => {
                        stats.add(path, FileWatcherEventType::Modify, FileWatcherKind::File);
                        changed.file_contents_changed(path);
                    }
                    ModifyKind::Name(_) | ModifyKind::Any | ModifyKind::Other => {
                        stats.add(path, FileWatcherEventType::Create, FileWatcherKind::File);
                        stats.add(path, FileWatcherEventType::Delete, FileWatcherKind::File);
                        changed.file_added_or_removed(path);
                        stats.add(
                            path,
                            FileWatcherEventType::Create,
                            FileWatcherKind::Directory,
                        );
                        stats.add(
                            path,
                            FileWatcherEventType::Delete,
                            FileWatcherKind::Directory,
                        );
                        changed.dir_added_or_removed(path);
                    }
                },
                EventKind::Remove(remove_kind) => match remove_kind {
                    RemoveKind::File => {
                        stats.add(path, FileWatcherEventType::Delete, FileWatcherKind::File);
                        changed.file_added_or_removed(path);
                    }
                    RemoveKind::Folder => {
                        stats.add(
                            path,
                            FileWatcherEventType::Delete,
                            FileWatcherKind::Directory,
                        );
                        changed.dir_added_or_removed(path);
                    }
                    RemoveKind::Any | RemoveKind::Other => {
                        stats.add(path, FileWatcherEventType::Delete, FileWatcherKind::File);
                        changed.file_added_or_removed(path);
                        stats.add(
                            path,
                            FileWatcherEventType::Delete,
                            FileWatcherKind::Directory,
                        );
                        changed.dir_added_or_removed(path);
                    }
                },
                _ => {}
            }
        }

        let stats = stats.finish();
        let changed = if self.missed_events {
            None
        } else {
            Some(changed)
        };

        (stats, changed)
    }
}

#[derive(Allocative)]
pub struct NotifyFileWatcher {
    #[allocative(skip)]
    #[expect(unused)]
    // FIXME(JakobDegen): Clarify if this just needs to be kept alive or can be removed?
    watcher: RecommendedWatcher,
    data: Arc<Mutex<buck2_error::Result<NotifyFileData>>>,
    /// What the changes recorded on the updater are relative to.
    baseline: Mutex<FileSystemBaseline>,
}

impl NotifyFileWatcher {
    pub fn new(root: &ProjectRoot, ignores: FileWatcherIgnores) -> buck2_error::Result<Self> {
        let data = Arc::new(Mutex::new(Ok(NotifyFileData::new())));
        let data2 = data.dupe();
        let root2 = root.dupe();
        let mut watcher = notify::recommended_watcher(move |event| {
            let mut guard = data2.lock().unwrap();
            if let Ok(state) = &mut *guard {
                if let Err(e) = state.process(event, &root2, &ignores) {
                    *guard = Err(e);
                }
            }
        })
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::NotifyWatcher))?;
        watcher
            .watch(root.root().as_path(), notify::RecursiveMode::Recursive)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::NotifyWatcher))?;
        Ok(Self {
            watcher,
            data,
            baseline: Mutex::new(FileSystemBaseline::unique()),
        })
    }

    fn sync2(
        &self,
        mut dice: DiceTransactionUpdater,
    ) -> buck2_error::Result<(buck2_data::FileWatcherStats, DiceTransactionUpdater)> {
        let old = {
            let mut guard = self.data.lock().unwrap();
            mem::replace(&mut *guard, Ok(NotifyFileData::new()))
        };
        let (stats, changes) = old?.sync();
        let mut baseline = self.baseline.lock().unwrap();
        if let Some(changes) = changes {
            changes.write_to_dice(&mut dice)?;
        } else {
            // Events were missed; the fresh instance reported in `stats` has the daemon start
            // over.
            *baseline = FileSystemBaseline::unique();
        }
        dice.set_file_system_baseline(baseline.dupe())?;
        Ok((stats, dice))
    }
}

#[async_trait]
impl FileWatcher for NotifyFileWatcher {
    async fn sync(
        &self,
        dice: DiceTransactionUpdater,
    ) -> buck2_error::Result<(DiceTransactionUpdater, Mergebase, SyncOutcome)> {
        span_async(
            buck2_data::FileWatcherStart {
                provider: buck2_data::FileWatcherProvider::RustNotify as i32,
            },
            async {
                let (stats, res) = match self.sync2(dice) {
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
    use buck2_common::ignores::ignore_set::IgnoreSet;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_fs::fs_util::uncategorized as fs_util;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use notify::event::CreateKind;
    use notify::event::Flag;

    use super::*;

    fn fixture() -> (ProjectRoot, FileWatcherIgnores, tempfile::TempDir) {
        let cells = CellResolver::testing_with_name_and_path(
            CellName::testing_new("root"),
            CellRootPathBuf::testing_new(""),
        );
        let tempdir = tempfile::tempdir().unwrap();
        let root_path =
            fs_util::canonicalize(AbsNormPathBuf::new(tempdir.path().to_owned()).unwrap()).unwrap();
        let root = ProjectRoot::new(root_path).unwrap();
        let ignores = Arc::new(
            IgnoreSet::testing_new_single_cell(cells, CellName::testing_new("root"), "").unwrap(),
        );
        (root, FileWatcherIgnores::new(ignores), tempdir)
    }

    /// The kernel-overflow signal can arrive as a rescan-flagged event with no paths attached
    /// (platform dependent). The flag must still be recorded, otherwise lost notifications go
    /// undetected and the daemon keeps answering from a stale graph, which is the cache
    /// inconsistency the missed-events handling exists to close.
    #[test]
    fn rescan_event_with_no_paths_sets_missed_events() {
        let (root, ignores, _t) = fixture();
        let mut state = NotifyFileData::new();
        let event = notify::Event::new(EventKind::Any).set_flag(Flag::Rescan);
        state.process(Ok(event), &root, &ignores).unwrap();
        assert!(
            state.missed_events,
            "a pathless rescan event must set missed_events"
        );
    }

    #[test]
    fn rescan_event_with_paths_sets_missed_events() {
        let (root, ignores, _t) = fixture();
        let mut state = NotifyFileData::new();
        let path = root.resolve(ProjectRelativePath::new("f").unwrap());
        let event = notify::Event::new(EventKind::Create(CreateKind::File))
            .add_path(path.into_abs_path_buf().into_path_buf())
            .set_flag(Flag::Rescan);
        state.process(Ok(event), &root, &ignores).unwrap();
        assert!(state.missed_events);
    }

    /// Missed events: the sync result carries no tracker (the daemon starts over) and the stats
    /// surface the fresh instance the same way watchman's path does.
    #[test]
    fn sync_with_missed_events_reports_fresh_instance_and_drops_changes() {
        let mut state = NotifyFileData::new();
        state.missed_events = true;
        let (stats, changes) = state.sync();
        assert!(changes.is_none(), "missed events must drop the tracker");
        assert!(stats.fresh_instance);
        let fresh = stats
            .fresh_instance_data
            .expect("fresh instance data populated");
        assert!(fresh.cleared_dice);
        assert!(stats.incomplete_events_reason.is_some());
    }

    #[test]
    fn sync_without_missed_events_returns_changes() {
        let (stats, changes) = NotifyFileData::new().sync();
        assert!(
            changes.is_some(),
            "no missed events: incremental tracker must survive"
        );
        assert!(!stats.fresh_instance);
        assert!(stats.incomplete_events_reason.is_none());
    }

    /// `chmod` arrives from every notify backend as `Modify(Metadata(Any))` (inotify
    /// `IN_ATTRIB`, FSEvents `INODE_META_MOD`, kqueue `NOTE_ATTRIB`); no backend emits
    /// `Permissions`. Only access events and access-time changes can be dropped.
    #[test]
    fn test_metadata_any_is_not_ignored() {
        assert!(!ignore_event_kind(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Any
        ))));
        assert!(!ignore_event_kind(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Permissions
        ))));
        assert!(ignore_event_kind(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));
        assert!(ignore_event_kind(EventKind::Access(
            notify::event::AccessKind::Read
        )));
    }
}
