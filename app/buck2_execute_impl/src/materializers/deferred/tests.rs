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

use buck2_common::file_ops::metadata::FileMetadata;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::directory::insert_file;
use buck2_execute::materialize::materializer::CleanStaleArtifactsArgs;
use buck2_execute::materialize::materializer::CleanStaleArtifactsPolicy;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use parking_lot::Mutex;

use super::*;

#[test]
fn test_rematerialization_ttl_tracks_refresh_frequency() {
    let config = TtlRefreshConfiguration {
        frequency: std::time::Duration::from_secs(123),
        min_ttl: SignedDuration::from_hours(1),
        enabled: true,
    };
    assert_eq!(
        config.rematerialization_ttl(),
        Some(SignedDuration::from_secs(123))
    );

    let disabled = TtlRefreshConfiguration {
        enabled: false,
        ..config
    };
    assert_eq!(disabled.rematerialization_ttl(), None);
}

#[tokio::test]
async fn test_background_cleanup_gate() {
    let gate = Arc::new(BackgroundCleanupGate::default());
    let command_guard = gate.register_prevention();
    assert!(gate.try_start_cleanup().is_none());
    drop(command_guard);

    let clean_guard = gate.try_start_cleanup().unwrap();
    let waiter = tokio::spawn({
        let gate = gate.dupe();
        async move {
            let command_guard = gate.register_prevention();
            command_guard.wait_for_cleanup().await;
            command_guard
        }
    });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());

    drop(clean_guard);
    let command_guard = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(gate.try_start_cleanup().is_none());
    drop(command_guard);
    assert!(gate.try_start_cleanup().is_some());
}

#[tokio::test]
async fn test_background_cleanup_wait_is_cancel_safe() {
    let gate = Arc::new(BackgroundCleanupGate::default());
    let clean_guard = gate.try_start_cleanup().unwrap();
    let waiter = tokio::spawn({
        let gate = gate.dupe();
        async move {
            let command_guard = gate.register_prevention();
            command_guard.wait_for_cleanup().await;
        }
    });
    tokio::task::yield_now().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());

    drop(clean_guard);
    assert!(gate.try_start_cleanup().is_some());
}

#[test]
fn test_find_artifacts() -> buck2_error::Result<()> {
    let artifact1 = ProjectRelativePathBuf::unchecked_new("foo/bar/baz".to_owned());
    let artifact2 = ProjectRelativePathBuf::unchecked_new("foo/bar/bar/qux".to_owned());
    let artifact3 = ProjectRelativePathBuf::unchecked_new("foo/bar/bar/quux".to_owned());
    let artifact4 = ProjectRelativePathBuf::unchecked_new("foo/bar/qux/quuz".to_owned());
    let non_artifact1 = ProjectRelativePathBuf::unchecked_new("foo/bar/qux".to_owned());
    let non_artifact2 = ProjectRelativePathBuf::unchecked_new("foo/bar/bar/corge".to_owned());

    let file = FileMetadata::empty(DigestConfig::testing_default().cas_digest_config());

    // Build deps with artifacts 1-3, and non-artifacts 1-2
    let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
    insert_file(
        &mut builder,
        artifact1.join(ForwardRelativePath::new("f1").unwrap()),
        file.dupe(),
    )?;
    insert_file(
        &mut builder,
        artifact2.join(ForwardRelativePath::new("d/f1").unwrap()),
        file.dupe(),
    )?;
    insert_file(&mut builder, artifact3.clone(), file.dupe())?;
    insert_file(&mut builder, non_artifact2, file.dupe())?;
    builder.mkdir(&non_artifact1)?;

    // Build tree with artifacts 1-4
    let mut tree: FileTree<()> = FileTree::new();
    tree.insert(artifact1.iter().map(|f| f.to_owned()), ());
    tree.insert(artifact2.iter().map(|f| f.to_owned()), ());
    tree.insert(artifact3.iter().map(|f| f.to_owned()), ());
    tree.insert(artifact4.iter().map(|f| f.to_owned()), ());

    let expected_artifacts: BuckMutSet<_> =
        vec![artifact1, artifact2, artifact3].into_iter().collect();
    let found_artifacts: BuckMutSet<_> = tree.find_artifacts(&builder).into_iter().collect();
    assert_eq!(found_artifacts, expected_artifacts);
    Ok(())
}

#[test]
fn test_remove_path() {
    fn insert(tree: &mut FileTree<String>, path: &str) {
        tree.insert(
            ProjectRelativePath::unchecked_new(path)
                .iter()
                .map(|f| f.to_owned()),
            path.to_owned(),
        );
    }

    let mut tree: FileTree<String> = FileTree::new();
    insert(&mut tree, "a/b/c/d");
    insert(&mut tree, "a/b/c/e");
    insert(&mut tree, "a/c");

    let removed_subtree = tree.remove_path(ProjectRelativePath::unchecked_new("a/b"));
    // Convert to BuckMutMap<String, String> so it's easier to test
    let removed_subtree: BuckMutMap<String, String> = removed_subtree
        .map(|(k, v)| (k.as_str().to_owned(), v))
        .collect();

    assert_eq!(removed_subtree.len(), 2);
    assert_eq!(removed_subtree.get("a/b/c/d"), Some(&"a/b/c/d".to_owned()));
    assert_eq!(removed_subtree.get("a/b/c/e"), Some(&"a/b/c/e".to_owned()));
}

#[cfg(test)]
pub(crate) mod state_machine {
    #[cfg(all(unix, not(target_os = "macos")))]
    use std::ffi::OsString;
    #[cfg(all(unix, not(target_os = "macos")))]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Barrier;
    use std::thread;

    use assert_matches::assert_matches;
    use buck2_common::file_ops::metadata::Symlink;
    use buck2_core::error::SoftErrorContext;
    use buck2_core::error::with_soft_error_context;
    use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_error::BuckErrorContext;
    use buck2_error::buck2_error;
    use buck2_events::daemon_id::DaemonId;
    use buck2_events::dispatch::with_dispatcher_async;
    use buck2_events::source::ChannelEventSource;
    use buck2_execute::directory::ActionDirectoryEntry;
    use buck2_execute::directory::ActionSharedDirectory;
    use buck2_execute::directory::INTERNER;
    use buck2_execute::execute::blocking::IoRequest;
    use buck2_fs::fs_util::ReadDir;
    use buck2_fs::fs_util::uncategorized as fs_util;
    use buck2_fs::paths::RelativePathBuf;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    use buck2_hash::IntentionallyStdHashMap;
    use buck2_util::threads::ignore_stack_overflow_checks_for_current_thread;
    use buck2_util::threads::ignore_stack_overflow_checks_for_future;
    use buck2_wrapper_common::invocation_id::TraceId;
    use futures::StreamExt;
    use futures::future::BoxFuture;
    use futures::future::FutureExt;
    use tokio::time::Duration as TokioDuration;
    use tokio::time::sleep;

    use super::*;
    use crate::materializers::deferred::artifact_tree::Processing;
    use crate::materializers::deferred::artifact_tree::UnmaterializationIneligibilityReason;
    use crate::materializers::deferred::artifact_tree::artifact_metadata_size;
    use crate::materializers::deferred::clean_stale::CleanInvalidatedPathRequest;
    use crate::materializers::deferred::clean_stale::CleanStaleSchedule;
    use crate::materializers::deferred::command_processor::TestingDeferredMaterializerCommandProcessor;
    use crate::materializers::deferred::extension::ExtensionCommand;
    use crate::sqlite::materializer_db::testing_materializer_state_sqlite_db;

    #[derive(Debug, Eq, PartialEq, Allocative)]
    pub(crate) enum Op {
        Clean,
        Materialize,
        MaterializeError,
    }

    #[derive(Allocative)]
    pub(crate) struct StubIoHandler {
        log: Mutex<Vec<(Op, ProjectRelativePathBuf)>>,
        fail: Mutex<bool>,
        fail_paths: Mutex<Vec<ProjectRelativePathBuf>>,
        fail_read_dirs: Mutex<Vec<AbsNormPathBuf>>,
        fail_next_invalidated_cleans: Mutex<usize>,
        invalidated_clean_attempts: Mutex<usize>,
        // If set, add a sleep when materializing to simulate a long materialization period
        materialization_config: BuckMutMap<ProjectRelativePathBuf, TokioDuration>,
        #[allocative(skip)]
        pub(crate) block_materialization: Option<Arc<tokio::sync::Semaphore>>,
        #[allocative(skip)]
        read_dir_barriers: Option<Arc<(Barrier, Barrier)>>,
        #[allocative(skip)]
        clean_barriers: Option<Arc<(Barrier, Barrier)>>,
        digest_config: DigestConfig,
        buck_out_path: ProjectRelativePathBuf,
        fs: ProjectRoot,
    }

    #[derive(Debug)]
    struct CaptureSoftErrorContext {
        sender: oneshot::Sender<bool>,
    }

    impl<T: 'static> ExtensionCommand<T> for CaptureSoftErrorContext {
        fn execute(self: Box<Self>, _processor: &mut DeferredMaterializerCommandProcessor<T>) {
            let has_context = get_dispatcher_opt()
                .and_then(|dispatcher| dispatcher.soft_error_context())
                .is_some();
            let _ignored = self.sender.send(has_context);
        }
    }

    impl DeferredMaterializerAccessor<StubIoHandler> {
        // Ensure that the command thread ends so that the command processor is dropped,
        // and the sqlite connection is flushed and closed.
        fn abort(mut self) {
            self.command_sender
                .send(MaterializerCommand::Abort)
                .unwrap();
            self.command_thread.take().unwrap().join().unwrap();
        }
    }

    impl StubIoHandler {
        pub(crate) fn take_log(&self) -> Vec<(Op, ProjectRelativePathBuf)> {
            std::mem::take(&mut *self.log.lock())
        }

        fn set_fail(&self, fail: bool) {
            *self.fail.lock() = fail;
        }

        pub(crate) fn set_fail_on(&self, paths: Vec<ProjectRelativePathBuf>) {
            *self.fail_paths.lock() = paths;
        }

        fn set_fail_read_dirs(&self, paths: Vec<AbsNormPathBuf>) {
            *self.fail_read_dirs.lock() = paths;
        }

        fn set_fail_next_invalidated_cleans(&self, count: usize) {
            *self.fail_next_invalidated_cleans.lock() = count;
        }

        fn invalidated_clean_attempts(&self) -> usize {
            *self.invalidated_clean_attempts.lock()
        }

        pub(crate) fn new(fs: ProjectRoot) -> Self {
            Self {
                log: Default::default(),
                fail: Default::default(),
                fail_paths: Default::default(),
                fail_read_dirs: Default::default(),
                fail_next_invalidated_cleans: Default::default(),
                invalidated_clean_attempts: Default::default(),
                materialization_config: BuckMutMap::default(),
                block_materialization: None,
                read_dir_barriers: None,
                clean_barriers: None,
                digest_config: DigestConfig::testing_default(),
                buck_out_path: make_path("buck-out/v2"),
                fs,
            }
        }

        pub fn with_materialization_config(
            mut self,
            materialization_config: BuckMutMap<ProjectRelativePathBuf, TokioDuration>,
        ) -> Self {
            self.materialization_config = materialization_config;
            self
        }

        pub fn with_read_dir_barriers(
            mut self,
            read_dir_barriers: Arc<(Barrier, Barrier)>,
        ) -> Self {
            self.read_dir_barriers = Some(read_dir_barriers);
            self
        }

        pub fn with_clean_barriers(mut self, clean_barriers: Arc<(Barrier, Barrier)>) -> Self {
            self.clean_barriers = Some(clean_barriers);
            self
        }
    }

    impl StubIoHandler {
        fn actually_write(self: &Arc<Self>, path: &ProjectRelativePathBuf, write: &Arc<WriteFile>) {
            let data = zstd::bulk::decompress(&write.compressed_data, write.decompressed_size)
                .buck_error_context("Error decompressing data")
                .unwrap();
            self.fs.write_file(path, data, write.is_executable).unwrap();
        }
    }

    #[async_trait]
    impl IoHandler for StubIoHandler {
        async fn immediate_write<'a>(
            self: &Arc<Self>,
            _gen: Box<dyn FnOnce() -> buck2_error::Result<Vec<WriteRequest>> + Send + 'a>,
        ) -> buck2_error::Result<Vec<ArtifactValue>> {
            unimplemented!()
        }

        fn clean_path<'a>(
            self: &Arc<Self>,
            path: ProjectRelativePathBuf,
            version: Version,
            command_sender: Arc<MaterializerSender<Self>>,
            _cancellations: &'a CancellationContext,
        ) -> BoxFuture<'a, Result<(), buck2_error::Error>> {
            self.log.lock().push((Op::Clean, path.clone()));

            async move {
                let _ignored = command_sender.send_low_priority(
                    LowPriorityMaterializerCommand::CleanupFinished {
                        path,
                        version,
                        result: Ok(()),
                        dispatcher: None,
                    },
                );
                Ok(())
            }
            .boxed()
        }

        async fn clean_invalidated_path<'a>(
            self: &Arc<Self>,
            request: CleanInvalidatedPathRequest,
            _cancellations: &'a CancellationContext,
        ) -> buck2_error::Result<()> {
            *self.invalidated_clean_attempts.lock() += 1;
            let should_fail = {
                let mut remaining = self.fail_next_invalidated_cleans.lock();
                if *remaining == 0 {
                    false
                } else {
                    *remaining -= 1;
                    true
                }
            };
            if should_fail {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CleanStale,
                    "Injected clean-stale deletion error"
                ));
            }
            if let Some(barriers) = self.clean_barriers.as_ref() {
                // Allow tests to advance here, execute something and then continue
                barriers.as_ref().0.wait();
                barriers.as_ref().1.wait();
            }
            Box::new(request).execute(&self.fs)
        }

        async fn materialize_entry(
            self: &Arc<Self>,
            path: ProjectRelativePathBuf,
            _method: Arc<ArtifactMaterializationMethod>,
            _entry: ActionDirectoryEntry<ActionSharedDirectory>,
            _event_dispatcher: EventDispatcher,
            _cancellations: &CancellationContext,
        ) -> Result<(), MaterializeEntryError> {
            if let Some(started) = &self.block_materialization {
                started.add_permits(1);
                futures::future::pending::<()>().await;
            }
            // Simulate a non-immediate materialization if configured
            if let Some(duration) = self.materialization_config.get(&path) {
                sleep(*duration).await;
            }

            if (*self.fail_paths.lock()).contains(&path) || *self.fail.lock() {
                self.log.lock().push((Op::MaterializeError, path));
                Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::MaterializationError,
                    "Injected error"
                )
                .into())
            } else {
                if let ArtifactMaterializationMethod::Write(write) = _method.as_ref() {
                    self.actually_write(&path, write);
                }
                self.log.lock().push((Op::Materialize, path));
                Ok(())
            }
        }

        fn create_ttl_refresh(
            self: &Arc<Self>,
            _tree: &ArtifactTree,
            _min_ttl: SignedDuration,
        ) -> Option<BoxFuture<'static, buck2_error::Result<()>>> {
            unimplemented!()
        }

        /// The stub has no CAS to upload to; a caller sees the failure it would see offline.
        async fn upload_materialized_artifact(
            self: &Arc<Self>,
            path: ProjectRelativePathBuf,
            _entry: ActionDirectoryEntry<ActionSharedDirectory>,
            _info: Arc<CasDownloadInfo>,
        ) -> buck2_error::Result<()> {
            Err(buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "The IO stub cannot upload `{path}`"
            ))
        }

        fn read_dir(&self, path: &AbsNormPathBuf) -> buck2_error::Result<ReadDir> {
            if self.fail_read_dirs.lock().contains(path) {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CleanStale,
                    "Injected clean-stale scan error"
                ));
            }
            if let Some(barriers) = self.read_dir_barriers.as_ref() {
                // Allow tests to advance here, execute something and then continue
                barriers.as_ref().0.wait();
                barriers.as_ref().1.wait();
            }
            fs_util::read_dir(path)
        }

        fn buck_out_path(&self) -> &ProjectRelativePathBuf {
            &self.buck_out_path
        }

        fn re_client_manager(&self) -> &Arc<ReConnectionManager> {
            unimplemented!()
        }

        fn fs(&self) -> &ProjectRoot {
            &self.fs
        }

        fn digest_config(&self) -> DigestConfig {
            self.digest_config
        }

        async fn seal(
            self: &Arc<Self>,
            _path: ProjectRelativePathBuf,
            _entry: ActionDirectoryEntry<ActionSharedDirectory>,
        ) -> buck2_error::Result<()> {
            Ok(())
        }

        async fn make_outputs_writable(
            self: &Arc<Self>,
            _paths: Vec<ProjectRelativePathBuf>,
        ) -> buck2_error::Result<()> {
            Ok(())
        }
    }

    /// A stub command sender. We are calling materializer methods directly so that's all we need.
    fn channel() -> (
        Arc<MaterializerSender<StubIoHandler>>,
        MaterializerReceiver<StubIoHandler>,
    ) {
        let (hi_send, hi_recv) = mpsc::unbounded_channel();
        let (lo_send, lo_recv) = mpsc::unbounded_channel();
        let counters = Arc::new(MaterializerCounters::default());

        (
            Arc::new(MaterializerSender {
                high_priority: hi_send,
                low_priority: lo_send,
                counters: counters.dupe(),
                clean_guard: Default::default(),
            }),
            MaterializerReceiver {
                high_priority: hi_recv,
                low_priority: lo_recv,
                counters,
            },
        )
    }

    fn make_path(p: &str) -> ProjectRelativePathBuf {
        ProjectRelativePath::new(p).unwrap().to_owned()
    }

    fn temp_root() -> ProjectRoot {
        ProjectRootTemp::new().unwrap().path().clone()
    }

    /// Queries whether the materializer considers `path` materialized, which it only does once it
    /// has processed the completion of the materialization.
    #[derive(Debug)]
    struct IsPathMaterialized {
        path: ProjectRelativePathBuf,
        sender: oneshot::Sender<bool>,
    }

    impl<T: IoHandler> ExtensionCommand<T> for IsPathMaterialized {
        fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
            let _ignored = self
                .sender
                .send(processor.testing_is_path_materialized(&self.path));
        }
    }

    async fn materialize_write(
        path: &ProjectRelativePathBuf,
        contents: &'static [u8],
        dm: &DeferredMaterializerAccessor<StubIoHandler>,
    ) -> buck2_error::Result<()> {
        dm.declare_write(Box::new(|| {
            Ok(vec![WriteRequest {
                path: path.clone(),
                content: contents.to_vec(),
                is_executable: false,
                path_kind: BuckOutPathKind::Configuration,
            }])
        }))
        .await?;

        dm.materialize_many(vec![path.clone()])
            .await?
            .next()
            .await
            .unwrap()?;

        // Materialization resolves once the file is on disk, but callers need the state the
        // materializer records when it processes the completion (notably the sqlite entry).
        loop {
            let (sender, receiver) = oneshot::channel();
            dm.command_sender.send(MaterializerCommand::Extension(
                Box::new(IsPathMaterialized {
                    path: path.clone(),
                    sender,
                }) as _,
                None,
            ))?;
            if receiver
                .await
                .buck_error_context("No response from materializer")?
            {
                return Ok(());
            }
            sleep(TokioDuration::from_millis(1)).await;
        }
    }

    fn make_db(fs: &ProjectRoot) -> (MaterializerStateSqliteDb, Option<MaterializerState>) {
        let (db, state) = testing_materializer_state_sqlite_db(
            fs,
            IntentionallyStdHashMap::from([("version".to_owned(), "0".to_owned())]),
            IntentionallyStdHashMap::new(),
            None,
        )
        .unwrap();
        (db, state.ok())
    }

    fn make_processor_for_io(
        io: Arc<StubIoHandler>,
    ) -> (
        DeferredMaterializerCommandProcessor<StubIoHandler>,
        Arc<MaterializerSender<StubIoHandler>>,
        MaterializerReceiver<StubIoHandler>,
        ChannelEventSource,
        Arc<BackgroundCleanupGate>,
    ) {
        let (db, sqlite_state) = make_db(io.fs());
        let stats = Arc::new(DeferredMaterializerStats::default());
        if let Some(sqlite_state) = &sqlite_state {
            for entry in sqlite_state {
                stats.add_materialized(
                    entry.classification,
                    artifact_metadata_size(&entry.metadata),
                );
            }
        }
        let tree = ArtifactTree::initialize(sqlite_state);

        let (daemon_dispatcher_events, daemon_dispatcher_sink) =
            buck2_events::create_source_sink_pair();
        let daemon_dispatcher =
            EventDispatcher::new(TraceId::null(), DaemonId::new(), daemon_dispatcher_sink);

        let (command_sender, command_receiver) = channel();
        let background_cleanup_gate = Arc::new(BackgroundCleanupGate::default());
        (
            DeferredMaterializerCommandProcessor::new(
                io,
                Some(db),
                Handle::current(),
                true,
                command_sender.dupe(),
                tree,
                CancellationContext::testing(),
                stats,
                Default::default(),
                true,
                daemon_dispatcher,
                CleanStaleConfig::default(),
                None,
                background_cleanup_gate.dupe(),
                Arc::new(CancellationScope::new()),
            ),
            command_sender,
            command_receiver,
            daemon_dispatcher_events,
            background_cleanup_gate,
        )
    }

    fn make_processor(
        materialization_config: BuckMutMap<ProjectRelativePathBuf, TokioDuration>,
    ) -> (
        DeferredMaterializerCommandProcessor<StubIoHandler>,
        MaterializerReceiver<StubIoHandler>,
    ) {
        let (dm, _, receiver, _, _) = make_processor_for_io(Arc::new(
            StubIoHandler::new(temp_root()).with_materialization_config(materialization_config),
        ));
        (dm, receiver)
    }

    async fn make_materializer(
        io: Arc<StubIoHandler>,
        clean_stale_config: Option<CleanStaleConfig>,
    ) -> (
        DeferredMaterializerAccessor<StubIoHandler>,
        ChannelEventSource,
    ) {
        let (
            mut processor,
            command_sender,
            command_receiver,
            daemon_dispatcher_events,
            background_cleanup_gate,
        ) = make_processor_for_io(io.dupe());
        processor.clean_stale_config = clean_stale_config.unwrap_or_default();
        let stats = processor.stats.dupe();
        let cancellation = processor.cancellation.clone();

        let command_thread = thread_spawn("buck2-dm", {
            move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();

                rt.block_on(processor.run(
                    command_receiver,
                    TtlRefreshConfiguration {
                        frequency: std::time::Duration::default(),
                        min_ttl: jiff::SignedDuration::ZERO,
                        enabled: false,
                    },
                    AccessTimesUpdates::Disabled,
                ));
            }
        })
        .buck_error_context("Cannot start materializer thread")
        .unwrap();

        (
            DeferredMaterializerAccessor {
                command_thread: Some(command_thread),
                cancellation,
                runtime: Handle::current(),
                command_sender,
                materialize_final_artifacts: true,
                defer_write_actions: true,
                io,
                materializer_state_info: buck2_data::MaterializerStateInfo {
                    num_entries_from_sqlite: 0,
                },
                stats,
                background_cleanup_gate,
            },
            daemon_dispatcher_events,
        )
    }

    #[tokio::test]
    async fn dropping_materializer_stops_worker_and_releases_io() {
        let io = Arc::new(StubIoHandler::new(temp_root()));
        let weak_io = Arc::downgrade(&io);
        let (mut materializer, _events) = make_materializer(io, None).await;
        let sender = materializer.command_sender.dupe();
        let worker = materializer.command_thread.take().unwrap();
        drop(materializer);
        let mut stopped = tokio::task::spawn_blocking(move || worker.join());
        let outcome = tokio::time::timeout(TokioDuration::from_secs(5), &mut stopped).await;
        if outcome.is_err() {
            let _ignored = sender.send(MaterializerCommand::Abort);
            stopped.await.unwrap().unwrap();
            panic!("dropping the materializer did not stop its worker");
        }
        outcome.unwrap().unwrap().unwrap();
        tokio::time::timeout(TokioDuration::from_secs(5), async {
            while weak_io.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            weak_io.upgrade().is_none(),
            "worker must release its I/O handler"
        );
    }

    #[tokio::test]
    async fn dropping_materializer_cancels_background_materialization() -> buck2_error::Result<()> {
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let mut io = StubIoHandler::new(temp_root());
        io.block_materialization = Some(started.clone());
        let io = Arc::new(io);
        let weak_io = Arc::downgrade(&io);
        let (mut materializer, _events) = make_materializer(io, None).await;
        let path = make_path("cancelled/output");
        materializer
            .declare_write(Box::new(|| {
                Ok(vec![WriteRequest {
                    path: path.clone(),
                    content: b"old".to_vec(),
                    is_executable: false,
                    path_kind: BuckOutPathKind::Configuration,
                }])
            }))
            .await?;
        let mut result = materializer.materialize_many(vec![path]).await?;
        started.acquire().await.unwrap().forget();
        let cancellation = materializer.cancellation.clone();
        let worker = materializer.command_thread.take().unwrap();
        drop(materializer);
        tokio::time::timeout(TokioDuration::from_secs(5), cancellation.wait())
            .await
            .unwrap();
        assert!(result.next().await.unwrap().is_err());
        tokio::task::spawn_blocking(move || worker.join())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(TokioDuration::from_secs(5), async {
            while weak_io.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            weak_io.upgrade().is_none(),
            "cancelled materialization must release its I/O handler"
        );
        Ok(())
    }

    fn receive_clean_result(events: &mut ChannelEventSource) -> buck2_data::CleanStaleResult {
        loop {
            let event = events.receive().expect("clean-stale event should be sent");
            if let buck2_data::buck_event::Data::Instant(instant) = event
                .unpack_buck()
                .expect("event should be a Buck event")
                .data()
                && let Some(buck2_data::instant_event::Data::CleanStaleResult(result)) =
                    instant.data.as_ref()
            {
                return (**result).clone();
            }
        }
    }

    async fn clean_stale_with_events(
        dm: &DeferredMaterializerAccessor<StubIoHandler>,
        args: CleanStaleArtifactsArgs,
    ) -> (
        buck2_error::Result<buck2_cli_proto::CleanStaleResponse>,
        ChannelEventSource,
    ) {
        let (events, sink) = buck2_events::create_source_sink_pair();
        let dispatcher = EventDispatcher::new(TraceId::null(), DaemonId::new(), sink);
        let result = with_dispatcher_async(dispatcher, dm.clean_stale_artifacts(args)).await;
        (result, events)
    }

    #[tokio::test]
    async fn command_thread_propagates_soft_error_context() -> buck2_error::Result<()> {
        let io = Arc::new(StubIoHandler::new(temp_root()));
        let (dm, _daemon_dispatcher_events) = make_materializer(io, None).await;
        let context = Arc::new(SoftErrorContext::new("", "")?);
        let dispatcher = EventDispatcher::null().with_soft_error_context(context);
        let (sender, receiver) = oneshot::channel();

        with_dispatcher_async(dispatcher, async {
            dm.command_sender.send(MaterializerCommand::Extension(
                Box::new(CaptureSoftErrorContext { sender }),
                get_dispatcher_opt(),
            ))?;
            assert!(receiver.await?);
            buck2_error::Ok(())
        })
        .await?;

        dm.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_allocative_profiles_artifact_tree() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let io = Arc::new(StubIoHandler::new(temp_root()));
            let path = make_path("foo/bar");
            let artifact = ArtifactValue::file(io.digest_config().empty_file());
            let (dm, _daemon_dispatcher_events) = make_materializer(io, None).await;

            let lease = dm.prepare_outputs(vec![path.clone()]).await?;
            dm.declare_existing(&lease, vec![DeclareArtifactPayload { path, artifact }])
                .await?;

            let source = dm.allocative().await?.flamegraph().write();
            assert!(
                source.contains("artifact_tree"),
                "flamegraph source should contain artifact_tree: {source}"
            );

            dm.abort();
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_declare_reuse() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, _) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            let path = make_path("foo/bar");
            let value = ArtifactValue::file(digest_config.empty_file());

            dm.testing_declare(&path, value.dupe());
            assert_eq!(dm.io.take_log(), &[(Op::Clean, path.clone())]);

            // When redeclaring the same artifact nothing happens.
            dm.testing_declare(&path, value.dupe());
            assert_eq!(dm.io.take_log(), &[]);

            let res = dm
                .materialize_artifact(&path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;
            assert_eq!(dm.io.take_log(), &[(Op::Materialize, path.clone())]);

            dm.testing_materialization_finished(path.clone(), jiff::Timestamp::now(), res);
            assert_eq!(dm.io.take_log(), &[]);

            // When redeclaring the same artifact nothing happens.
            dm.testing_declare(&path, value.dupe());
            assert_eq!(dm.io.take_log(), &[]);

            // When declaring the same artifact but under it, we clean it and it's a new artifact.
            let path2 = make_path("foo/bar/baz");
            dm.testing_declare(&path2, value.dupe());
            assert_eq!(dm.io.take_log(), &[(Op::Clean, path2.clone())]);

            let _ignore = dm
                .materialize_artifact(&path2, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;
            assert_eq!(dm.io.take_log(), &[(Op::Materialize, path2.clone())]);

            Ok(())
        })
        .await
    }

    /// Tracked file digests are interned by value, so two values built from the same content
    /// share one expiration; callers that need distinct expirations pass distinct content.
    fn cas_value(digest_config: DigestConfig, content: &[u8], expiry: Timestamp) -> ArtifactValue {
        let digest = TrackedFileDigest::from_content(content, digest_config.cas_digest_config());
        digest.update_expires(expiry);
        ArtifactValue::file(FileMetadata {
            digest,
            is_executable: false,
        })
    }

    fn cas_method() -> Box<ArtifactMaterializationMethod> {
        Box::new(ArtifactMaterializationMethod::CasDownload {
            info: Arc::new(CasDownloadInfo::new_declared(
                RemoteExecutorUseCase::buck2_default(),
            )),
        })
    }

    fn declare_and_materialize(
        dm: &mut DeferredMaterializerCommandProcessor<StubIoHandler>,
        path: &ProjectRelativePathBuf,
        value: ArtifactValue,
        method: Box<ArtifactMaterializationMethod>,
    ) {
        dm.testing_process_one_command(MaterializerCommand::Declare(
            DeclareArtifactPayload {
                path: path.clone(),
                artifact: value,
            },
            method,
            EventDispatcher::null(),
            None,
        ));
        dm.testing_materialization_finished(path.clone(), Timestamp::now(), Ok(()));
    }

    #[tokio::test]
    async fn cas_rematerialization_method_survives_redeclaration() -> buck2_error::Result<()> {
        let (mut dm, _) = make_processor(Default::default());
        let path = make_path("foo/cas");
        let expiry = Timestamp::now()
            .checked_add(SignedDuration::from_hours(1))
            .expect("one hour should fit in a timestamp");
        let value = cas_value(
            dm.io.digest_config(),
            b"cas_rematerialization_method_survives_redeclaration",
            expiry,
        );

        declare_and_materialize(&mut dm, &path, value.dupe(), cas_method());
        let data = dm
            .tree
            .prefix_get(&mut path.iter())
            .expect("materialized CAS artifact should be present");
        assert!(matches!(
            &data.stage,
            ArtifactMaterializationStage::Materialized {
                rematerialization_method: Some(_),
                ..
            }
        ));

        dm.testing_process_one_command(MaterializerCommand::Declare(
            DeclareArtifactPayload {
                path: path.clone(),
                artifact: value,
            },
            cas_method(),
            EventDispatcher::null(),
            None,
        ));
        let data = dm
            .tree
            .prefix_get(&mut path.iter())
            .expect("redeclared CAS artifact should be present");
        assert!(matches!(
            &data.stage,
            ArtifactMaterializationStage::Materialized {
                rematerialization_method: Some(_),
                ..
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn unmaterialization_blocks_rematerialization_until_cleaning_finishes()
    -> buck2_error::Result<()> {
        let (mut dm, _) = make_processor(Default::default());
        let digest_config = dm.io.digest_config();
        let now = Timestamp::now();
        let deadline = now
            .checked_add(SignedDuration::from_mins(10))
            .expect("ten minutes should fit in a timestamp");
        let valid_expiry = now
            .checked_add(SignedDuration::from_hours(1))
            .expect("one hour should fit in a timestamp");
        let paths = [
            make_path("foo/happy"),
            make_path("foo/expired"),
            make_path("foo/processing"),
            make_path("foo/final"),
            make_path("foo/local"),
        ];

        for path in &paths[..4] {
            let expiry = if path == &paths[1] { now } else { valid_expiry };
            declare_and_materialize(
                &mut dm,
                path,
                cas_value(digest_config, path.as_str().as_bytes(), expiry),
                cas_method(),
            );
        }
        declare_and_materialize(
            &mut dm,
            &paths[4],
            cas_value(digest_config, paths[4].as_str().as_bytes(), valid_expiry),
            Box::new(ArtifactMaterializationMethod::LocalCopy(
                FileTree::new(),
                Vec::new(),
            )),
        );
        assert_eq!(
            dm.io.take_log(),
            paths
                .iter()
                .cloned()
                .map(|path| (Op::Clean, path))
                .collect::<Vec<_>>()
        );

        dm.tree
            .prefix_get_mut(&mut paths[2].iter())
            .expect("processing artifact should be present")
            .processing = Processing::active(
            ProcessingFuture::Materializing(
                futures::future::pending::<Result<(), SharedMaterializingError>>()
                    .boxed()
                    .shared(),
            ),
            Version(100),
        );
        dm.tree
            .prefix_get_mut(&mut paths[3].iter())
            .expect("final artifact should be present")
            .classification = ArtifactClassification::FinalOutput;

        let requested = paths.iter().cloned().map(|path| (path, 1)).collect();
        let result = dm.tree.unmaterialize_artifacts(
            requested,
            deadline,
            None,
            dm.sqlite_db
                .as_mut()
                .expect("test processor should have sqlite state"),
            &dm.stats,
        )?;

        assert_eq!(result.unmaterialized, vec![(paths[0].clone(), 1)]);
        assert_eq!(result.ineligible.len(), 4);
        assert_eq!(
            result
                .ineligible
                .iter()
                .map(|artifact| artifact.size)
                .sum::<u64>(),
            4
        );
        for reason in [
            UnmaterializationIneligibilityReason::RemoteTtlTooShort,
            UnmaterializationIneligibilityReason::Processing,
            UnmaterializationIneligibilityReason::FinalOutput,
            UnmaterializationIneligibilityReason::NoRematerializationMethod,
        ] {
            assert!(
                result
                    .ineligible
                    .iter()
                    .any(|artifact| artifact.reason == reason),
                "expected ineligibility reason {reason:?}"
            );
        }
        assert!(matches!(
            dm.tree
                .prefix_get(&mut paths[0].iter())
                .expect("unmaterialized artifact should remain in the tree")
                .stage,
            ArtifactMaterializationStage::Declared { .. }
        ));

        let (cleaning_started_sender, cleaning_started_receiver) = oneshot::channel();
        let (cleaning_sender, cleaning_receiver) = oneshot::channel();
        dm.tree.attach_unmaterialization_future(
            &paths[0],
            async move {
                cleaning_started_sender
                    .send(())
                    .map_err(|_| internal_error!("cleaning-start receiver should remain alive"))?;
                cleaning_receiver
                    .await
                    .map_err(|_| internal_error!("cleaning sender should remain alive"))
            }
            .boxed()
            .shared(),
            Version(101),
        )?;
        let data = dm
            .tree
            .prefix_get(&mut paths[0].iter())
            .expect("unmaterialized artifact should retain its cleaning future");
        assert!(matches!(
            data.processing.active_ref().map(|active| &active.future),
            Some(ProcessingFuture::Cleaning(_))
        ));

        let materialization = {
            let _ignore = ignore_stack_overflow_checks_for_current_thread();
            dm.materialize_artifact(&paths[0], EventDispatcher::null())
                .expect("declared artifact should require rematerialization")
        };
        cleaning_started_receiver
            .await
            .expect("cleaning future should be polled");
        assert_eq!(dm.io.take_log(), &[]);

        cleaning_sender
            .send(())
            .expect("cleaning receiver should remain alive");
        materialization
            .await
            .expect("rematerialization should succeed after cleaning");
        assert_eq!(dm.io.take_log(), &[(Op::Materialize, paths[0].clone())]);
        Ok(())
    }

    #[tokio::test]
    async fn unmaterialization_failure_keeps_artifact_materialized() -> buck2_error::Result<()> {
        let (mut dm, _) = make_processor(Default::default());
        let path = make_path("test/unmaterialize/failure");
        let now = Timestamp::now();
        let deadline = now
            .checked_add(SignedDuration::from_mins(10))
            .expect("ten minutes should fit in a timestamp");
        let expiry = now
            .checked_add(SignedDuration::from_hours(1))
            .expect("one hour should fit in a timestamp");
        let value = cas_value(
            dm.io.digest_config(),
            b"unmaterialization_failure_keeps_artifact_materialized",
            expiry,
        );

        declare_and_materialize(&mut dm, &path, value, cas_method());
        let sizes_before = *dm.stats.sizes.read();

        let result = dm.tree.unmaterialize_artifacts(
            vec![(path.clone(), 1)],
            deadline,
            None,
            dm.sqlite_db
                .as_mut()
                .expect("test processor should have sqlite state"),
            &dm.stats,
        );
        assert!(result.is_err(), "injected unmaterialization should fail");
        assert!(matches!(
            dm.tree
                .prefix_get(&mut path.iter())
                .expect("failed unmaterialization should leave the artifact in the tree")
                .stage,
            ArtifactMaterializationStage::Materialized { .. }
        ));
        assert_eq!(*dm.stats.sizes.read(), sizes_before);
        Ok(())
    }

    fn make_artifact_value_with_symlink_dep(
        target_path: &ProjectRelativePathBuf,
        target_from_symlink: &RelativePathBuf,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<ArtifactValue> {
        let mut deps = ActionDirectoryBuilder::empty_non_exhaustive();
        let target = ActionDirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata::empty(
            digest_config.cas_digest_config(),
        )));
        deps.insert(target_path.as_forward_relative_path(), target)?;
        let symlink_value = ArtifactValue::new(
            ActionDirectoryEntry::Leaf(ActionDirectoryMember::Symlink(Arc::new(Symlink::new(
                target_from_symlink.clone(),
            )))),
            Some(
                deps.fingerprint(digest_config.as_directory_serializer())
                    .shared(&*INTERNER),
            ),
        );
        Ok(symlink_value)
    }

    #[tokio::test]
    async fn test_final_output_accounting_includes_symlink_deps() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, _) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();
            let target_path = make_path("foo/target");
            let symlink_path = make_path("foo/link");
            let target_from_symlink = RelativePathBuf::from_system_path(Path::new("target"))?;
            let content = b"target contents";
            let target_value = ArtifactValue::file(FileMetadata {
                digest: TrackedFileDigest::from_content(content, digest_config.cas_digest_config()),
                is_executable: false,
            });
            let symlink_value = make_artifact_value_with_symlink_dep(
                &target_path,
                &target_from_symlink,
                digest_config,
            )?;

            dm.testing_declare_existing(&target_path, target_value.dupe());
            dm.testing_declare_existing(&symlink_path, symlink_value);
            assert_eq!(
                *dm.stats.sizes.read(),
                MaterializerSizeStats {
                    final_output: 0,
                    intermediate_only: content.len() as u64,
                }
            );
            let persisted = dm
                .sqlite_db
                .as_mut()
                .expect("test processor should have sqlite state")
                .materializer_state_table()
                .read_materializer_state(digest_config)?;
            assert_eq!(persisted.len(), 2);
            assert!(
                persisted
                    .iter()
                    .all(|entry| entry.classification == ArtifactClassification::IntermediateOnly)
            );

            let (sender, receiver) = oneshot::channel();
            dm.testing_process_one_command(MaterializerCommand::Ensure(
                vec![symlink_path.clone()],
                MaterializationPurpose::FinalOutput { required: true },
                EventDispatcher::null(),
                None,
                sender,
            ));
            let _materializations = receiver.await?;

            for path in [&target_path, &symlink_path] {
                let data = dm
                    .tree
                    .prefix_get(&mut path.iter())
                    .expect("declared artifact should be present");
                assert_eq!(data.classification, ArtifactClassification::FinalOutput);
            }
            assert_eq!(
                *dm.stats.sizes.read(),
                MaterializerSizeStats {
                    final_output: content.len() as u64,
                    intermediate_only: 0,
                }
            );
            let persisted = dm
                .sqlite_db
                .as_mut()
                .expect("test processor should have sqlite state")
                .materializer_state_table()
                .read_materializer_state(digest_config)?;
            assert!(
                persisted
                    .iter()
                    .all(|entry| entry.classification == ArtifactClassification::FinalOutput)
            );

            dm.testing_declare_existing(&target_path, target_value);
            let data = dm
                .tree
                .prefix_get(&mut target_path.iter())
                .expect("redeclared artifact should be present");
            assert_eq!(data.classification, ArtifactClassification::FinalOutput);
            assert_eq!(
                *dm.stats.sizes.read(),
                MaterializerSizeStats {
                    final_output: content.len() as u64,
                    intermediate_only: 0,
                }
            );

            let (sender, receiver) = oneshot::channel();
            dm.testing_process_one_command(MaterializerCommand::InvalidateFilePaths(
                vec![target_path, symlink_path],
                sender,
                EventDispatcher::null(),
                None,
            ));
            receiver.await?.await?;
            assert_eq!(*dm.stats.sizes.read(), MaterializerSizeStats::default());

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_skipped_final_output_stays_intermediate_only() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let io = Arc::new(StubIoHandler::new(temp_root()));
            let digest_config = io.digest_config();
            let path = make_path("foo/skipped");
            let content = b"skipped contents";
            let value = ArtifactValue::file(FileMetadata {
                digest: TrackedFileDigest::from_content(content, digest_config.cas_digest_config()),
                is_executable: false,
            });
            let (mut dm, _events) = make_materializer(io, None).await;
            dm.materialize_final_artifacts = false;
            let lease = dm.prepare_outputs(vec![path.clone()]).await?;
            dm.declare_existing(
                &lease,
                vec![DeclareArtifactPayload {
                    path: path.clone(),
                    artifact: value.dupe(),
                }],
            )
            .await?;
            assert!(dm.has_artifact_at(path.clone()).await?);

            let response = dm
                .materialize(request(
                    vec![(path.clone(), value.dupe())],
                    MaterializationPurpose::FinalOutput { required: false },
                ))
                .await?;
            drop(response.ensure_results_ok()?);
            assert_eq!(
                *dm.stats.sizes.read(),
                MaterializerSizeStats {
                    final_output: 0,
                    intermediate_only: content.len() as u64,
                }
            );

            let mut snapshot = buck2_data::Snapshot::default();
            dm.add_snapshot_stats(&mut snapshot);
            assert_eq!(snapshot.deferred_materializer_final_output_logical_bytes, 0);
            assert_eq!(
                snapshot.deferred_materializer_intermediate_only_logical_bytes,
                content.len() as u64
            );

            dm.materialize_final_artifacts = true;
            let response = dm
                .materialize(request(
                    vec![(path, value)],
                    MaterializationPurpose::FinalOutput { required: true },
                ))
                .await?;
            drop(response.ensure_results_ok()?);
            let mut snapshot = buck2_data::Snapshot::default();
            dm.add_snapshot_stats(&mut snapshot);
            assert_eq!(
                snapshot.deferred_materializer_final_output_logical_bytes,
                content.len() as u64
            );
            assert_eq!(
                snapshot.deferred_materializer_intermediate_only_logical_bytes,
                0
            );
            dm.abort();
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_materialize_symlink_and_target() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            // Construct a tree with a symlink and its target, materialize both at once
            let symlink_path = make_path("foo/bar_symlink");
            let target_path = make_path("foo/bar_target");
            let target_from_symlink = RelativePathBuf::from_system_path(Path::new("bar_target"))?;

            let mut materialization_config = BuckMutMap::default();
            // Materialize the symlink target slowly so that we actually hit the logic point where we
            // await for symlink targets and the entry materialization
            materialization_config.insert(target_path.clone(), TokioDuration::from_millis(100));

            let (mut dm, _) = make_processor(materialization_config);
            let digest_config = dm.io.digest_config();

            // Declare symlink target
            dm.testing_declare(
                &target_path,
                ArtifactValue::file(digest_config.empty_file()),
            );
            assert_eq!(dm.io.take_log(), &[(Op::Clean, target_path.clone())]);

            // Declare symlink
            let symlink_value = make_artifact_value_with_symlink_dep(
                &target_path,
                &target_from_symlink,
                digest_config,
            )?;
            dm.testing_declare(&symlink_path, symlink_value);
            assert_eq!(dm.io.take_log(), &[(Op::Clean, symlink_path.clone())]);

            dm.materialize_artifact(&symlink_path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await
                .map_err(|_| {
                    buck2_error!(
                        buck2_error::ErrorTag::MaterializationError,
                        "error materializing"
                    )
                })?;

            let logs = dm.io.take_log();
            if cfg!(unix) {
                assert_eq!(
                    logs,
                    &[
                        (Op::Materialize, symlink_path.clone()),
                        (Op::Materialize, target_path.clone())
                    ]
                );
            } else {
                assert_eq!(
                    logs,
                    &[
                        (Op::Materialize, target_path.clone()),
                        (Op::Materialize, symlink_path.clone())
                    ]
                );
            }
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_materialize_symlink_first_then_target() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            // Materialize a symlink, then materialize the target. Test that we still
            // materialize deps if the main artifact has already been materialized.
            let symlink_path = make_path("foo/bar_symlink");
            let target_path = make_path("foo/bar_target");
            let target_from_symlink = RelativePathBuf::from_system_path(Path::new("bar_target"))?;

            let mut materialization_config = BuckMutMap::default();
            // Materialize the symlink target slowly so that we actually hit the logic point where we
            // await for symlink targets and the entry materialization
            materialization_config.insert(target_path.clone(), TokioDuration::from_millis(100));

            let (mut dm, _) = make_processor(materialization_config);
            let digest_config = dm.io.digest_config();

            // Declare symlink
            let symlink_value = make_artifact_value_with_symlink_dep(
                &target_path,
                &target_from_symlink,
                digest_config,
            )?;
            dm.testing_declare(&symlink_path, symlink_value);
            assert_eq!(dm.io.take_log(), &[(Op::Clean, symlink_path.clone())]);

            // Materialize the symlink, at this point the target is not in the tree so it's ignored
            let res = dm
                .materialize_artifact(&symlink_path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;

            let logs = dm.io.take_log();
            assert_eq!(logs, &[(Op::Materialize, symlink_path.clone())]);

            // Mark the symlink as materialized
            dm.testing_materialization_finished(symlink_path.clone(), jiff::Timestamp::now(), res);
            assert_eq!(dm.io.take_log(), &[]);

            // Declare symlink target
            dm.testing_declare(
                &target_path,
                ArtifactValue::file(digest_config.empty_file()),
            );
            assert_eq!(dm.io.take_log(), &[(Op::Clean, target_path.clone())]);

            // Materialize the symlink again.
            // This time, we don't re-materialize the symlink as that's already been done.
            // But we still materialize the target as that has not been materialized yet.
            dm.materialize_artifact(&symlink_path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await
                .map_err(|_| {
                    buck2_error!(
                        buck2_error::ErrorTag::MaterializationError,
                        "error materializing"
                    )
                })?;

            let logs = dm.io.take_log();
            assert_eq!(logs, &[(Op::Materialize, target_path.clone())]);

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_invalidate_error() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async{
            let (mut dm, _) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            let path = make_path("test/invalidate/failure");
            let value1 = ArtifactValue::file(digest_config.empty_file());
            let value2 = ArtifactValue::dir(digest_config.empty_directory());

            // Start from having something.
            dm.testing_declare_existing(&path, value1);

            // This will collect the existing future and invalidate, and then fail in doing so.
            dm.testing_declare(&path, value2);

            // Now we check that materialization fails. This needs to wait on the previous clean.
            let res = dm
                .materialize_artifact(&path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;

            assert_matches!(
            res,
            Err(SharedMaterializingError::Error(e)) if format!("{e:#}").contains("Injected error")
        );

            // We do not actually get to materializing or cleaning.
            assert_eq!(dm.io.take_log(), &[]);

            Ok(())
        }).await
    }

    #[tokio::test]
    async fn test_materialize_dep_error() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            // Construct a tree with a symlink and its target, materialize both at once
            let symlink_path = make_path("foo/bar_symlink");
            let target_path = make_path("foo/bar_target");
            let target_from_symlink =
                RelativePathBuf::from_system_path(Path::new("bar_target"))?;

            let (mut dm, mut channel) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            let target_value = ArtifactValue::file(digest_config.empty_file());
            let symlink_value = make_artifact_value_with_symlink_dep(
                &target_path,
                &target_from_symlink,
                digest_config,
            )?;
            // Declare and materialize symlink and target
            dm.testing_declare(
                &target_path,
                target_value.clone(),
            );
            dm.testing_declare(
                &symlink_path,
                symlink_value.clone(),
            );
            dm.materialize_artifact(&symlink_path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await
                .map_err(|err| buck2_error!(buck2_error::ErrorTag::MaterializationError, "error materializing {:?}", err))?;
            assert_eq!(
                dm.io.take_log(),
                &[
                    (Op::Clean, target_path.clone()),
                    (Op::Clean, symlink_path.clone()),
                    (Op::Materialize, target_path.clone()),
                    (Op::Materialize, symlink_path.clone()),
                ]
            );

            // Process materialization_finished, change symlink stage to materialized
            while let Ok(cmd) = channel.low_priority.try_recv() {
                dm.testing_process_one_low_priority_command(cmd);
            }

            // Change symlink target value and re-declare
            let content = b"not empty";
            let meta = FileMetadata {
                digest: TrackedFileDigest::from_content(content, digest_config.cas_digest_config()),
                is_executable: false,
            };
            let target_value = ArtifactValue::file(meta);
            dm.testing_declare(
                &target_path,
                target_value,
            );
            assert_eq!(dm.io.take_log(), &[(Op::Clean, target_path.clone())]);

            // Request to materialize symlink, fail to materialize target
            dm.io.set_fail_on(vec![target_path.clone()]);
            let res = dm
                .materialize_artifact(&symlink_path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;
            assert_matches!(
            res,
            Err(SharedMaterializingError::Error(e)) if format!("{e:#}").contains("Injected error")
        );
            assert_eq!(
                dm.io.take_log(),
                &[(Op::MaterializeError, target_path.clone())]
            );
            // Process materialization_finished, _only_ target is cleaned, not symlink
            while let Ok(cmd) = channel.low_priority.try_recv() {
                dm.testing_process_one_low_priority_command(cmd);
            }
            assert_eq!(dm.io.take_log(), &[(Op::Clean, target_path.clone())]);

            // Request symlink again, target is materialized and symlink materialization succeeds
            dm.io.set_fail_on(vec![]);
            dm.materialize_artifact(&symlink_path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await
                .map_err(|err| buck2_error!(buck2_error::ErrorTag::MaterializationError, "error materializing 2 {:?}", err))?;
            assert_eq!(dm.io.take_log(), &[(Op::Materialize, target_path.clone()), ]);
            Ok(())
        }).await
    }

    #[tokio::test]
    async fn test_cleanup_finished_after_invalidation() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, mut channel) = make_processor(Default::default());
            let path = make_path("foo/bar");
            let value = ArtifactValue::file(dm.io.digest_config().empty_file());
            dm.testing_declare(&path, value);

            let pending = dm.tree.invalidate_paths_and_collect_futures(
                vec![path.clone()],
                dm.sqlite_db.as_mut(),
                &dm.stats,
            )?;
            assert_eq!(pending.len(), 1);
            join_all_existing_futs(pending).await?;
            assert!(dm.tree.prefix_get(&mut path.iter()).is_none());

            let completion = channel
                .low_priority
                .try_recv()
                .expect("cleanup should have queued its completion");
            assert_matches!(
                completion,
                LowPriorityMaterializerCommand::CleanupFinished { .. }
            );
            let context = Arc::new(SoftErrorContext::new("true", "")?);
            with_soft_error_context(Some(context), || {
                dm.testing_process_one_low_priority_command(completion);
            });
            assert!(dm.tree.prefix_get(&mut path.iter()).is_none());
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_retry() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, mut channel) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            let path = make_path("test");
            let value1 = ArtifactValue::file(digest_config.empty_file());

            // Declare a value.
            dm.testing_declare(&path, value1);

            // Make materializations fail
            dm.io.set_fail(true);

            // Materializing it fails.
            let res = dm
                .materialize_artifact(&path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;

            assert_matches!(
                res,
                Err(SharedMaterializingError::Error(e)) if format!("{e:#}").contains("Injected error")
            );

            // Unset fail, but we haven't processed materialization_finished yet so this does nothing.
            dm.io.set_fail(false);

            // Rejoining the existing future fails.
            let res = dm
                .materialize_artifact(&path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;

            assert_matches!(
                res,
                Err(SharedMaterializingError::Error(e)) if format!("{e:#}").contains("Injected error")
            );

            // Now process cleanup_finished_vacant and materialization_finished.
            let mut processed = 0;

            while let Ok(cmd) = channel.low_priority.try_recv() {
                eprintln!("got cmd = {cmd:?}");
                dm.testing_process_one_low_priority_command(cmd);
                processed += 1;
            }

            assert_eq!(processed, 2);

            // Materializing works now:
            let res = dm
                .materialize_artifact(&path, EventDispatcher::null())
                .internal_error("Expected a future")?
                .await;

            assert_matches!(res, Ok(()));

            Ok(())
        }).await
    }

    const SAMPLE_BUCK_OUT_PATH: &str = "buck-out/v2/art/foo/bar";

    #[tokio::test]
    async fn test_clean_stale() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            materialize_write(&path, b"contents", &dm).await?;
            // Drop dm and flush sqlite connection.
            dm.abort();
            // Create new materializer from db state so that artifacts are not active
            let (dm, _) = make_materializer(io, None).await;

            let (res, mut events) = clean_stale_with_events(
                &dm,
                CleanStaleArtifactsArgs {
                    policy: CleanStaleArtifactsPolicy::Explicit {
                        keep_since_time: jiff::Timestamp::MAX,
                        adaptive_low_disk_threshold: None,
                        adaptive_min_ttl: None,
                        adaptive_unmaterialize_active: false,
                    },
                    dry_run: false,
                    tracked_only: false,
                    origin: buck2_data::clean_stale_result::Origin::CleanallCommand,
                },
            )
            .await;
            let res = res?;

            let stats = res
                .stats
                .as_ref()
                .unwrap_or_else(|| panic!("{}", res.message.unwrap()));
            let &buck2_data::CleanStaleStats {
                stale_artifact_count,
                stale_bytes,
                cleaned_artifact_count,
                cleaned_bytes,
                ..
            } = stats;
            assert_eq!(
                (
                    stale_artifact_count,
                    stale_bytes,
                    cleaned_artifact_count,
                    cleaned_bytes
                ),
                (1, 8, 1, 8)
            );
            assert_eq!(stats.ttl_stale_artifact_count, 1);
            assert_eq!(stats.ttl_stale_bytes, 8);
            assert_eq!(stats.adaptive_stale_artifact_count, 0);
            assert_eq!(stats.adaptive_stale_bytes, 0);
            assert_eq!(stats.cleaned_stale_artifact_count, 1);
            assert_eq!(stats.cleaned_stale_bytes, 8);
            assert_eq!(
                stats.materialized_final_output_bytes_before
                    + stats.materialized_intermediate_only_bytes_before,
                8
            );
            assert_eq!(
                stats.materialized_final_output_bytes_after
                    + stats.materialized_intermediate_only_bytes_after,
                0
            );

            let event = receive_clean_result(&mut events);
            assert_eq!(
                event.trigger,
                buck2_data::clean_stale_result::Trigger::ManualExplicit as i32
            );
            assert_eq!(
                event.origin,
                buck2_data::clean_stale_result::Origin::CleanallCommand as i32
            );
            assert_eq!(
                event.policy_mode,
                buck2_data::clean_stale_result::PolicyMode::ExplicitTtl as i32
            );
            assert_eq!(
                event.failure_phase,
                buck2_data::clean_stale_result::FailurePhase::None as i32
            );
            assert_eq!(
                event.adaptive_outcome,
                buck2_data::clean_stale_result::AdaptiveOutcome::Disabled as i32
            );
            assert!(!event.tracked_only);
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_clean_stale_records_sizes_after_cleanup_finishes() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io, None).await;
            materialize_write(&path, b"contents", &dm).await?;
            dm.abort();

            let clean_barriers = Arc::new((Barrier::new(2), Barrier::new(2)));
            let io = Arc::new(
                StubIoHandler::new(project_root).with_clean_barriers(clean_barriers.dupe()),
            );
            let (dm, _) = make_materializer(io, None).await;
            let materializer_stats = dm.stats.dupe();
            let stats_update = thread::spawn(move || {
                clean_barriers.0.wait();
                materializer_stats.add_materialized(ArtifactClassification::IntermediateOnly, 4);
                clean_barriers.1.wait();
            });

            let res = dm
                .clean_stale_artifacts(CleanStaleArtifactsArgs {
                    policy: CleanStaleArtifactsPolicy::Explicit {
                        keep_since_time: jiff::Timestamp::MAX,
                        adaptive_low_disk_threshold: None,
                        adaptive_min_ttl: None,
                        adaptive_unmaterialize_active: false,
                    },
                    dry_run: false,
                    tracked_only: false,
                    origin: buck2_data::clean_stale_result::Origin::CleanCommand,
                })
                .await?;
            stats_update
                .join()
                .expect("materializer stats update should finish");

            let stats = res.stats.expect("clean-stale should return stats");
            assert!(stats.used_disk_space_bytes_start.is_some());
            assert!(stats.used_disk_space_bytes_end.is_some());
            assert_eq!(
                (
                    stats.materialized_intermediate_only_bytes_before,
                    stats.materialized_intermediate_only_bytes_after,
                ),
                (8, 4),
            );
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_clean_stale_keeps_going_after_scan_error() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let project_root = temp_root();
            let failed_dir = project_root.resolve(make_path("buck-out/v2/gen"));
            let cleanable_dir = project_root.resolve(make_path("buck-out/v2/art/cleanable"));
            fs_util::create_dir_all(&failed_dir)?;
            fs_util::create_dir_all(&cleanable_dir)?;

            let io = Arc::new(StubIoHandler::new(project_root));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            io.set_fail_read_dirs(vec![failed_dir]);

            let (result, mut events) = clean_stale_with_events(
                &dm,
                CleanStaleArtifactsArgs {
                    policy: CleanStaleArtifactsPolicy::Explicit {
                        keep_since_time: jiff::Timestamp::MAX,
                        adaptive_low_disk_threshold: None,
                        adaptive_min_ttl: None,
                        adaptive_unmaterialize_active: false,
                    },
                    dry_run: false,
                    tracked_only: false,
                    origin: buck2_data::clean_stale_result::Origin::CleanCommand,
                },
            )
            .await;

            let Err(error) = result else {
                panic!("clean-stale should report the injected scan error");
            };
            assert!(
                format!("{error:#}").contains("Injected clean-stale scan error"),
                "clean-stale should return the first scan error"
            );
            assert!(
                !fs_util::try_exists(&cleanable_dir)?,
                "clean-stale should scan and delete artifacts from later directories"
            );
            let event = receive_clean_result(&mut events);
            assert_eq!(
                event.failure_phase,
                buck2_data::clean_stale_result::FailurePhase::Scan as i32
            );
            let stats = event.stats.expect("failed event should preserve stats");
            assert_eq!(stats.scan_failed_directory_count, 1);
            assert_eq!(stats.cleaned_untracked_artifact_count, 1);
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_clean_stale_keeps_going_after_delete_error() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let project_root = temp_root();
            let first_dir = project_root.resolve(make_path("buck-out/v2/art/first"));
            let second_dir = project_root.resolve(make_path("buck-out/v2/art/second"));
            fs_util::create_dir_all(&first_dir)?;
            fs_util::create_dir_all(&second_dir)?;

            let io = Arc::new(StubIoHandler::new(project_root));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            io.set_fail_next_invalidated_cleans(1);

            let (result, mut events) = clean_stale_with_events(
                &dm,
                CleanStaleArtifactsArgs {
                    policy: CleanStaleArtifactsPolicy::Explicit {
                        keep_since_time: jiff::Timestamp::MAX,
                        adaptive_low_disk_threshold: None,
                        adaptive_min_ttl: None,
                        adaptive_unmaterialize_active: false,
                    },
                    dry_run: false,
                    tracked_only: false,
                    origin: buck2_data::clean_stale_result::Origin::CleanCommand,
                },
            )
            .await;

            let Err(error) = result else {
                panic!("clean-stale should report the injected deletion error");
            };
            assert!(
                format!("{error:#}").contains("Injected clean-stale deletion error"),
                "clean-stale should return the first deletion error"
            );
            assert_eq!(
                io.invalidated_clean_attempts(),
                2,
                "clean-stale should attempt every deletion"
            );
            assert_ne!(
                fs_util::try_exists(&first_dir)?,
                fs_util::try_exists(&second_dir)?,
                "exactly one path should remain after one injected deletion failure"
            );
            let event = receive_clean_result(&mut events);
            assert_eq!(
                event.failure_phase,
                buck2_data::clean_stale_result::FailurePhase::Clean as i32
            );
            let stats = event.stats.expect("failed event should preserve stats");
            assert_eq!(stats.cleaned_untracked_artifact_count, 1);
            assert_eq!(stats.delete_failed_artifact_count, 1);
            Ok(())
        })
        .await
    }

    fn explicit_clean_stale_args(tracked_only: bool) -> CleanStaleArtifactsArgs {
        CleanStaleArtifactsArgs {
            policy: CleanStaleArtifactsPolicy::Explicit {
                keep_since_time: jiff::Timestamp::MAX,
                adaptive_low_disk_threshold: None,
                adaptive_min_ttl: None,
                adaptive_unmaterialize_active: false,
            },
            dry_run: false,
            tracked_only,
            origin: buck2_data::clean_stale_result::Origin::CleanCommand,
        }
    }

    #[tokio::test]
    async fn test_clean_stale_tracked_only_with_file_at_tracked_dir() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            materialize_write(&path, b"contents", &dm).await?;
            // Drop dm and flush sqlite connection.
            dm.abort();

            // The state still lists `foo/bar`, but on disk `foo` is now an untracked file.
            let foo = project_root.resolve(make_path("buck-out/v2/art/foo"));
            fs_util::remove_dir_all(&foo)?;
            fs_util::write(&foo, b"untracked")?;

            let (dm, _) = make_materializer(io, None).await;
            let error = dm
                .clean_stale_artifacts(explicit_clean_stale_args(true))
                .await
                .expect_err("the stale path is reported, not deleted");
            assert!(format!("{error:#}").contains("is not a directory on disk"));
            // `foo` is not a directory, so nothing below it is ours to delete.
            assert!(
                fs_util::symlink_metadata_if_exists(&foo)?
                    .expect("`foo` still exists")
                    .is_file()
            );
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_clean_stale_tracked_only_with_symlink_at_tracked_dir() -> buck2_error::Result<()>
    {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            materialize_write(&path, b"contents", &dm).await?;
            dm.abort();

            // The state still lists `foo/bar`, but on disk `foo` now points outside `buck-out`.
            let outside = project_root.resolve(make_path("outside"));
            let precious = project_root.resolve(make_path("outside/bar"));
            fs_util::create_dir_all(&outside)?;
            fs_util::write(&precious, b"precious")?;
            let foo = project_root.resolve(make_path("buck-out/v2/art/foo"));
            fs_util::remove_dir_all(&foo)?;
            fs_util::symlink(&outside, &foo)?;

            let (dm, _) = make_materializer(io, None).await;
            let error = dm
                .clean_stale_artifacts(explicit_clean_stale_args(true))
                .await
                .expect_err("the stale path is reported, not deleted");
            assert!(format!("{error:#}").contains("is not a directory on disk"));
            // Nothing is deleted through the symlink, and the symlink itself stays.
            assert!(fs_util::try_exists(&precious)?);
            assert!(
                fs_util::symlink_metadata_if_exists(&foo)?
                    .expect("`foo` still exists")
                    .is_symlink()
            );
            Ok(())
        })
        .await
    }

    /// The tests remove the owner permissions of `path`. If the clean under test fails to
    /// delete `path`, the temp dir can't be deleted until the permissions are restored.
    #[cfg(unix)]
    fn restore_permissions_if_exists(path: &AbsNormPathBuf) -> buck2_error::Result<()> {
        if fs_util::try_exists(path)? {
            fs_util::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_clean_stale_deletes_unreadable() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            materialize_write(&path, b"contents", &dm).await?;
            // Drop dm and flush sqlite connection.
            dm.abort();
            // Create new materializer from db state so that artifacts are not active
            let (dm, _) = make_materializer(io, None).await;

            // An untracked artifact containing a directory the scan cannot read.
            let untracked_dir = project_root.resolve(make_path("buck-out/v2/art/foo/untracked"));
            let unreadable_dir =
                project_root.resolve(make_path("buck-out/v2/art/foo/untracked/unreadable"));
            fs_util::create_dir(&untracked_dir)?;
            fs_util::create_dir(&unreadable_dir)?;
            fs_util::set_permissions(&unreadable_dir, std::fs::Permissions::from_mode(0o000))?;

            // A second untracked artifact whose directory can be listed but not
            // traversed (readable, not executable), so its entries cannot be
            // statted.
            let listable_dir = project_root.resolve(make_path("buck-out/v2/art/foo/untracked2"));
            let listable_file =
                project_root.resolve(make_path("buck-out/v2/art/foo/untracked2/file"));
            fs_util::create_dir(&listable_dir)?;
            fs_util::write(&listable_file, b"x")?;
            fs_util::set_permissions(&listable_dir, std::fs::Permissions::from_mode(0o444))?;

            let res = dm
                .clean_stale_artifacts(CleanStaleArtifactsArgs {
                    policy: CleanStaleArtifactsPolicy::Explicit {
                        keep_since_time: jiff::Timestamp::MAX,
                        adaptive_low_disk_threshold: None,
                        adaptive_min_ttl: None,
                        adaptive_unmaterialize_active: false,
                    },
                    dry_run: false,
                    tracked_only: false,
                    origin: buck2_data::clean_stale_result::Origin::CleanCommand,
                })
                .await;

            restore_permissions_if_exists(&unreadable_dir)?;
            restore_permissions_if_exists(&listable_dir)?;

            let res = res?;
            let &buck2_data::CleanStaleStats {
                stale_artifact_count,
                cleaned_artifact_count,
                untracked_artifact_count,
                skipped_unreadable_count,
                scan_unreadable_count,
                delete_permission_denied_artifact_count,
                ..
            } = res
                .stats
                .as_ref()
                .unwrap_or_else(|| panic!("{}", res.message.unwrap()));
            assert!(!fs_util::try_exists(&untracked_dir)?);
            assert!(!fs_util::try_exists(&listable_dir)?);
            assert_eq!(
                (
                    stale_artifact_count,
                    cleaned_artifact_count,
                    untracked_artifact_count,
                    skipped_unreadable_count,
                    scan_unreadable_count,
                    delete_permission_denied_artifact_count,
                ),
                (1, 3, 2, 2, 2, 0),
                "the scan skips reading the unreadable dir and statting the file in the \
                 non-traversable dir, and the clean still deletes both untracked roots \
                 because it adds owner permissions to the directories it deletes"
            );
            Ok(())
        })
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_clean_scratch_sweep() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io.dupe(), None).await;

            // Dead scratch is deleted regardless of age.
            let dead = project_root.resolve(make_path("buck-out/v2/tmp/dead"));
            let dead_file = project_root.resolve(make_path("buck-out/v2/tmp/dead/junk"));
            fs_util::create_dir_all(&dead)?;
            fs_util::write(&dead_file, b"xxxx")?;

            // The scan can't read `unreadable` or traverse `listable`. The sweep still deletes
            // `unreadable` and `listable`.
            let unreadable = project_root.resolve(make_path("buck-out/v2/tmp/unreadable"));
            let unreadable_file =
                project_root.resolve(make_path("buck-out/v2/tmp/unreadable/junk"));
            fs_util::create_dir(&unreadable)?;
            fs_util::write(&unreadable_file, b"x")?;
            fs_util::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))?;

            let listable = project_root.resolve(make_path("buck-out/v2/tmp/listable"));
            let listable_file = project_root.resolve(make_path("buck-out/v2/tmp/listable/junk"));
            fs_util::create_dir(&listable)?;
            fs_util::write(&listable_file, b"x")?;
            fs_util::set_permissions(&listable, std::fs::Permissions::from_mode(0o444))?;

            #[cfg(not(target_os = "macos"))]
            let invalid_name = {
                // macOS filesystems reject invalid UTF-8 file names.
                let scratch_root = project_root.resolve(make_path("buck-out/v2/tmp"));
                let invalid_name = scratch_root
                    .as_abs_path()
                    .join(OsString::from_vec(vec![0xff]));
                fs_util::create_dir(&invalid_name)?;
                invalid_name
            };

            let (mut events, sink) = buck2_events::create_source_sink_pair();
            let dispatcher = EventDispatcher::new(TraceId::null(), DaemonId::new(), sink);
            let res = with_dispatcher_async(dispatcher, dm.clean_scratch()).await;

            restore_permissions_if_exists(&unreadable)?;
            restore_permissions_if_exists(&listable)?;

            let res = res?;
            let &buck2_data::CleanStaleStats {
                untracked_artifact_count,
                cleaned_artifact_count,
                cleaned_bytes,
                skipped_unreadable_count,
                scan_invalid_filename_count,
                ..
            } = res
                .stats
                .as_ref()
                .unwrap_or_else(|| panic!("{}", res.message.unwrap()));
            assert!(!fs_util::try_exists(&dead)?);
            assert!(!fs_util::try_exists(&unreadable)?);
            assert!(!fs_util::try_exists(&listable)?);
            #[cfg(not(target_os = "macos"))]
            assert!(fs_util::try_exists(&invalid_name)?);
            let expected_invalid_filename_count = if cfg!(target_os = "macos") { 0 } else { 1 };
            assert_eq!(
                (
                    untracked_artifact_count,
                    cleaned_artifact_count,
                    cleaned_bytes,
                    skipped_unreadable_count,
                    scan_invalid_filename_count,
                ),
                (3, 3, 4, 2, expected_invalid_filename_count,),
                "the sweep deletes all scratch with a valid name and records the entries the \
                 scan can't read and the invalid names separately"
            );
            assert!(
                events.try_receive().is_none(),
                "scratch sweeps should not emit clean-stale telemetry",
            );
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_clean_stale_interrupt() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            let io = Arc::new(StubIoHandler::new(project_root.clone()));
            let (dm, _) = make_materializer(io.dupe(), None).await;
            materialize_write(&path, b"contents", &dm).await?;

            let read_dir_barriers =
                Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
            let io = Arc::new(
                StubIoHandler::new(project_root.dupe())
                    .with_read_dir_barriers(read_dir_barriers.dupe()),
            );
            let (dm, _) = make_materializer(io, None).await;

            // Interrupt while scanning buck-out
            let dm = Arc::new(dm);
            let dm_dup = dm.dupe();
            let fut = dm_dup.clean_stale_artifacts(CleanStaleArtifactsArgs {
                policy: CleanStaleArtifactsPolicy::Explicit {
                    keep_since_time: jiff::Timestamp::MAX,
                    adaptive_low_disk_threshold: None,
                    adaptive_min_ttl: None,
                    adaptive_unmaterialize_active: false,
                },
                dry_run: false,
                tracked_only: false,
                origin: buck2_data::clean_stale_result::Origin::CleanCommand,
            });
            thread::spawn(move || {
                // Wait until a read_dir request is about to execute
                read_dir_barriers.0.wait();
                // Sending a high_priority command will interrupt the processor
                let noop_command = MaterializerCommand::DeclareExisting(vec![], None, None);
                let _unused = dm.command_sender.send(noop_command);
                // Wait after sending so that a second request doesn't start
                read_dir_barriers.1.wait();
            });
            let res = fut.await?;
            let &buck2_data::CleanStaleStats {
                stale_artifact_count,
                stale_bytes,
                cleaned_artifact_count,
                cleaned_bytes,
                ..
            } = res.stats.as_ref().unwrap();
            assert_eq!(
                (
                    stale_artifact_count,
                    stale_bytes,
                    cleaned_artifact_count,
                    cleaned_bytes
                ),
                (0, 0, 0, 0)
            );

            let clean_barriers = Arc::new((Barrier::new(2), Barrier::new(2)));
            let io = Arc::new(
                StubIoHandler::new(project_root.dupe()).with_clean_barriers(clean_barriers.dupe()),
            );
            let (dm, _) = make_materializer(io, None).await;

            // Interrupt while deleting files
            let dm = Arc::new(dm);
            let dm_dup = dm.dupe();
            let fut = dm_dup.clean_stale_artifacts(CleanStaleArtifactsArgs {
                policy: CleanStaleArtifactsPolicy::Explicit {
                    keep_since_time: jiff::Timestamp::MAX,
                    adaptive_low_disk_threshold: None,
                    adaptive_min_ttl: None,
                    adaptive_unmaterialize_active: false,
                },
                dry_run: false,
                tracked_only: false,
                origin: buck2_data::clean_stale_result::Origin::CleanCommand,
            });
            thread::spawn(move || {
                // Wait until a single clean request is about to execute
                clean_barriers.0.wait();
                // Sending a high_priority command will drop the clean guard immediately (from this thread)
                let noop_command = MaterializerCommand::DeclareExisting(vec![], None, None);
                let _unused = dm.command_sender.send(noop_command);
                // Wait after sending, executing clean request will complete but a second request doesn't start because
                // the single io thread is blocked
                clean_barriers.1.wait();
            });
            let res = fut.await?;
            let &buck2_data::CleanStaleStats {
                stale_artifact_count,
                stale_bytes,
                cleaned_artifact_count,
                cleaned_bytes,
                ..
            } = res.stats.as_ref().unwrap();
            assert_eq!(
                (
                    stale_artifact_count,
                    stale_bytes,
                    cleaned_artifact_count,
                    cleaned_bytes
                ),
                (1, 8, 0, 0)
            );

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_clean_stale_schedule() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let path = make_path(SAMPLE_BUCK_OUT_PATH);
            let project_root = temp_root();
            // dry run because it's easier and since this is only testing that cleans are triggered by the materializer
            let clean_stale_config = CleanStaleConfig {
                schedule: Some(CleanStaleSchedule {
                    clean_period: std::time::Duration::from_secs(1),
                    start_offset: std::time::Duration::from_secs(0),
                }),
                artifact_ttl: std::time::Duration::from_secs(0),
                low_disk: None,
                unmaterialize_upload: None,
                dry_run: true,
            };
            let io = Arc::new(StubIoHandler::new(project_root.dupe()));
            let (dm, mut daemon_dispatcher_events) =
                make_materializer(io.dupe(), Some(clean_stale_config)).await;
            materialize_write(&path, b"contents", &dm).await?;

            // The first clean stale request is scheduled at roughly the same time as materialize_write so we may receive an initial clean event
            // before anything is materialized, if so ignore events until an artifact is found (retained != 0).
            // It should only be necessary to wait for a single clean (1 second) but wait for up to 5 just in case.
            let mut i = 0;
            while i < 5 {
                let res = receive_clean_result(&mut daemon_dispatcher_events);
                let stats = res.stats.unwrap();
                if let buck2_data::CleanStaleStats {
                    retained_artifact_count: 0,
                    ..
                } = stats
                {
                    i += 1;
                } else {
                    break;
                }
            }
            let res = receive_clean_result(&mut daemon_dispatcher_events);
            assert_eq!(
                res.trigger,
                buck2_data::clean_stale_result::Trigger::Scheduled as i32
            );
            assert_eq!(
                res.origin,
                buck2_data::clean_stale_result::Origin::DaemonSchedule as i32
            );
            assert_eq!(
                res.policy_mode,
                buck2_data::clean_stale_result::PolicyMode::ConfiguredTtl as i32
            );
            let buck2_data::CleanStaleStats {
                retained_artifact_count,
                ..
            } = res.stats.unwrap();
            assert_eq!(retained_artifact_count, 1);
            // check it's scheduled more than once
            let res = receive_clean_result(&mut daemon_dispatcher_events);
            let buck2_data::CleanStaleStats {
                retained_artifact_count,
                ..
            } = res.stats.unwrap();
            assert_eq!(retained_artifact_count, 1);
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_has_artifact_at() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, _) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            let path = make_path("test/dir/path");
            let value1 = ArtifactValue::dir(digest_config.empty_directory());
            dm.testing_declare_existing(&path, value1);

            assert!(dm.testing_has_artifact(path.clone()));
            assert!(!dm.testing_has_artifact(path.join(ForwardRelativePath::new("foo").unwrap())));
            assert!(!dm.testing_has_artifact(path.parent().unwrap().to_owned()));

            dm.materialize_artifact(&path, EventDispatcher::null());
            assert!(dm.testing_has_artifact(path.clone()));
            assert!(!dm.testing_has_artifact(path.join(ForwardRelativePath::new("foo").unwrap())));
            assert!(!dm.testing_has_artifact(path.parent().unwrap().to_owned()));

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_get_artifact_entries_for_materialized_paths() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, _) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            // Path not in tree
            let unknown_path = make_path("not/in/tree");
            let result =
                dm.testing_get_artifact_entries_for_materialized_paths(vec![unknown_path.clone()]);
            assert_eq!(result.len(), 1);
            assert!(result[0].is_none());

            let declared_file_path = make_path("declared/file");
            let file_value = ArtifactValue::file(digest_config.empty_file());
            dm.testing_declare(&declared_file_path, file_value.dupe());
            let result = dm.testing_get_artifact_entries_for_materialized_paths(vec![
                declared_file_path.clone(),
            ]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, declared_file_path);
            assert_eq!(&returned_entry, file_value.entry());

            let declared_dir_path = make_path("declared/dir");
            let dir_value = ArtifactValue::dir(digest_config.empty_directory());
            dm.testing_declare(&declared_dir_path, dir_value.dupe());
            let result = dm.testing_get_artifact_entries_for_materialized_paths(vec![
                declared_dir_path.clone(),
            ]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, declared_dir_path);
            assert_eq!(&returned_entry, dir_value.entry());

            let materialized_file_path = make_path("materialized/file");
            let file_value = ArtifactValue::file(digest_config.empty_file());
            dm.testing_declare_existing(&materialized_file_path, file_value.dupe());
            let result = dm.testing_get_artifact_entries_for_materialized_paths(vec![
                materialized_file_path.clone(),
            ]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, materialized_file_path);
            assert_eq!(&returned_entry, file_value.entry());

            let materialized_dir_path = make_path("materialized/dir");
            let dir_value = ArtifactValue::dir(digest_config.empty_directory());
            dm.testing_declare_existing(&materialized_dir_path, dir_value.dupe());
            let result = dm.testing_get_artifact_entries_for_materialized_paths(vec![
                materialized_dir_path.clone(),
            ]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, materialized_dir_path);
            assert!(matches!(returned_entry, ActionDirectoryEntry::Dir(_)));

            // Subpath of an artifact via projected artifact; returns None
            let parent_path = make_path("parent/artifact");
            let parent_value = ArtifactValue::dir(digest_config.empty_directory());
            dm.testing_declare(&parent_path, parent_value);
            let subpath = make_path("parent/artifact/child");
            let result =
                dm.testing_get_artifact_entries_for_materialized_paths(vec![subpath.clone()]);
            assert_eq!(result.len(), 1);
            assert!(result[0].is_none());

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn test_get_artifact_entries_for_projected_paths() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let (mut dm, _) = make_processor(Default::default());
            let digest_config = dm.io.digest_config();

            // Build a non-empty directory artifact with internal structure:
            //   child/file.txt
            //   child/subdir/nested.txt
            //   top_file.txt
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
            insert_file(
                &mut builder,
                ProjectRelativePathBuf::unchecked_new("child/file.txt".to_owned()),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
            insert_file(
                &mut builder,
                ProjectRelativePathBuf::unchecked_new("child/subdir/nested.txt".to_owned()),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
            insert_file(
                &mut builder,
                ProjectRelativePathBuf::unchecked_new("top_file.txt".to_owned()),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
            builder.mark_uniformly_exhaustive();
            let shared_dir = builder
                .fingerprint(digest_config.as_directory_serializer())
                .shared(&*INTERNER);
            let dir_value = ArtifactValue::dir(shared_dir);

            let artifact_root = make_path("parent/artifact");
            dm.testing_declare(&artifact_root, dir_value);

            // Subpath of a file within artifact - returns the base artifact entry (Dir), not the projected file
            let file_subpath = make_path("parent/artifact/child/file.txt");
            let result =
                dm.testing_get_root_artifact_entries_for_subpaths(vec![file_subpath.clone()]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, file_subpath);
            assert!(matches!(returned_entry, ActionDirectoryEntry::Dir(_)));

            // Subpath of a subdirectory within artifact - returns the base artifact entry (Dir)
            let subdir_path = make_path("parent/artifact/child/subdir");
            let result =
                dm.testing_get_root_artifact_entries_for_subpaths(vec![subdir_path.clone()]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, subdir_path);
            assert!(matches!(returned_entry, ActionDirectoryEntry::Dir(_)));

            // Nonexistent subpath within artifact - still returns the base artifact entry (Dir)
            let nonexistent = make_path("parent/artifact/does_not_exist.txt");
            let result =
                dm.testing_get_root_artifact_entries_for_subpaths(vec![nonexistent.clone()]);
            assert_eq!(result.len(), 1);
            let (returned_path, returned_entry) = result[0].clone().unwrap();
            assert_eq!(returned_path, nonexistent);
            assert!(matches!(returned_entry, ActionDirectoryEntry::Dir(_)));

            Ok(())
        })
        .await
    }

    fn file_value(digest_config: DigestConfig, content: &[u8]) -> ArtifactValue {
        ArtifactValue::file(FileMetadata {
            digest: TrackedFileDigest::from_content(content, digest_config.cas_digest_config()),
            is_executable: false,
        })
    }

    fn request(
        artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
        purpose: MaterializationPurpose,
    ) -> MaterializeRequest {
        MaterializeRequest {
            artifacts,
            outputs: Vec::new(),
            purpose,
            re_use_case: RemoteExecutorUseCase::buck2_default(),
        }
    }

    #[tokio::test]
    async fn materialize_request_yields_one_result_per_path_in_order() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let io = Arc::new(StubIoHandler::new(temp_root()));
            let digest_config = io.digest_config();
            let (dm, _events) = make_materializer(io.dupe(), None).await;

            let existing = make_path("foo/existing");
            let existing_value = file_value(digest_config, b"existing");
            let lease = dm.prepare_outputs(vec![existing.clone()]).await?;
            dm.declare_existing(
                &lease,
                vec![DeclareArtifactPayload {
                    path: existing.clone(),
                    artifact: existing_value.dupe(),
                }],
            )
            .await?;

            let remote = make_path("foo/remote");
            let remote_value = file_value(digest_config, b"remote");
            let materializer: &dyn Materializer = &dm;
            materializer
                .declare_cas_many(
                    Arc::new(CasDownloadInfo::new_declared(
                        RemoteExecutorUseCase::buck2_default(),
                    )),
                    vec![DeclareArtifactPayload {
                        path: remote.clone(),
                        artifact: remote_value.dupe(),
                    }],
                )
                .await?;

            // Nothing declared this one; this materializer takes it to be on disk already.
            let undeclared = make_path("foo/undeclared");

            let response = dm
                .materialize(request(
                    vec![
                        (remote.clone(), remote_value),
                        (existing.clone(), existing_value),
                        (undeclared, file_value(digest_config, b"undeclared")),
                    ],
                    MaterializationPurpose::IntermediateOnly,
                ))
                .await?;
            assert_eq!(response.results.len(), 3);
            drop(response.ensure_results_ok()?);

            // Only the CAS-declared path had anything to do; the request declared nothing.
            assert_eq!(
                io.take_log(),
                vec![(Op::Clean, remote.clone()), (Op::Materialize, remote)]
            );
            dm.abort();
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn prepare_outputs_untracks_without_touching_disk() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let io = Arc::new(StubIoHandler::new(temp_root()));
            let digest_config = io.digest_config();
            let (dm, _events) = make_materializer(io.dupe(), None).await;

            let path = make_path("foo/output");
            io.fs().write_file(&path, "previous", false)?;
            let lease = dm.prepare_outputs(vec![path.clone()]).await?;
            dm.declare_existing(
                &lease,
                vec![DeclareArtifactPayload {
                    path: path.clone(),
                    artifact: file_value(digest_config, b"previous"),
                }],
            )
            .await?;
            assert!(dm.has_artifact_at(path.clone()).await?);

            let _lease = dm.prepare_outputs(vec![path.clone()]).await?;
            assert!(!dm.has_artifact_at(path.clone()).await?);
            assert!(
                io.fs().resolve(&path).exists(),
                "deleting what is there is the caller's job"
            );
            assert_eq!(io.take_log(), vec![]);
            dm.abort();
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn publish_materializes_the_destination_eagerly() -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let io = Arc::new(StubIoHandler::new(temp_root()));
            let digest_config = io.digest_config();
            let (dm, _events) = make_materializer(io.dupe(), None).await;

            let src = make_path("foo/placeholder");
            let dest = make_path("foo/content");
            let lease = dm.prepare_outputs(vec![src.clone()]).await?;
            drop(
                dm.publish(
                    &lease,
                    src.clone(),
                    dest.clone(),
                    file_value(digest_config, b"published"),
                )
                .await?,
            );

            // `dest` is on disk before `publish` returns, and `src` stays tracked because it is
            // what `dest` copies from.
            assert_eq!(
                io.take_log(),
                vec![(Op::Clean, dest.clone()), (Op::Materialize, dest.clone())]
            );
            assert!(dm.has_artifact_at(src).await?);
            assert!(dm.has_artifact_at(dest).await?);
            dm.abort();
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn materialize_request_skips_unrequired_final_outputs_when_configured()
    -> buck2_error::Result<()> {
        ignore_stack_overflow_checks_for_future(async {
            let io = Arc::new(StubIoHandler::new(temp_root()));
            let digest_config = io.digest_config();
            let (mut dm, _events) = make_materializer(io.dupe(), None).await;
            dm.materialize_final_artifacts = false;

            let path = make_path("foo/final");
            let value = file_value(digest_config, b"final");
            let materializer: &dyn Materializer = &dm;
            materializer
                .declare_cas_many(
                    Arc::new(CasDownloadInfo::new_declared(
                        RemoteExecutorUseCase::buck2_default(),
                    )),
                    vec![DeclareArtifactPayload {
                        path: path.clone(),
                        artifact: value.dupe(),
                    }],
                )
                .await?;

            let response = dm
                .materialize(request(
                    vec![(path.clone(), value.dupe())],
                    MaterializationPurpose::FinalOutput { required: false },
                ))
                .await?;
            assert_eq!(response.results.len(), 1);

            let response = dm
                .materialize(request(
                    vec![(path.clone(), value)],
                    MaterializationPurpose::FinalOutput { required: true },
                ))
                .await?;
            drop(response.ensure_results_ok()?);
            // One clean from the declare and one materialization from the required request: the
            // skipped request touched nothing.
            assert_eq!(
                io.take_log(),
                vec![(Op::Clean, path.clone()), (Op::Materialize, path)]
            );
            dm.abort();
            Ok(())
        })
        .await
    }
}
