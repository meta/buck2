/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The materializer whose state is what is on disk.
//!
//! One processor thread owns the disk tree (`DiskTree`): tracked roots, leases and the requests
//! waiting for them, in one trie. It does no IO: it grants leases, records reports and releases,
//! and mirrors tracked state into sqlite. Everything that touches disk happens outside it, under a
//! write lease, and the processor learns about the outcome through a report. That holds for a
//! local action writing its outputs as much as for this materializer's own downloads.
//!
//! A request is split into its roots up front, and each root is then one future, independent of
//! the others: it takes its lease, downloads the content if the lease came in write mode, reports,
//! and ends up holding the root in read mode. The request is the join of its roots, so a root
//! that has to wait for someone else's write holds up nothing but itself. A root's download runs
//! in a `Task` that its holder owns and that, once started, runs to its end: a holder dropped
//! mid-way hands the task back with its lease, and the next holder of the root either takes the
//! download over or cancels it and waits for it to end before writing there itself.
//!
//! Waiting happens in exactly one place, the lease queue. A request for content someone else is
//! writing queues behind that write and, once the writer has reported, is granted in read mode on
//! the same content; there is no in-flight future to join.
//!
//! A request holds the roots it has been granted while it waits for the rest. That cannot
//! deadlock: a request's roots are queued in one processor step, so between any two requests the
//! queue order is the same at every root where they conflict, and the tree only ever makes a
//! request wait for requests queued before it; the wait-for relation follows queue order and has
//! no cycles. The one acquisition made while holding roots from an earlier request is `publish`,
//! for a content-addressed destination: whoever holds that is a downloader or publisher of the
//! same content, which finishes without waiting, or a reader of it, which consumes this output
//! and so cannot be producing the outputs held. Anyone adding an acquisition made while holding
//! has to redo this argument.
//!
//! What is here is the first step of the core: requests whose content is already on disk or in
//! the CAS, the report, output preparation and publishing, and eager versions of the producer
//! calls the transition still needs. Local copies from other tracked paths, clean-stale and the
//! stash come in later steps; what is not supported yet fails with an error that names itself
//! rather than doing something approximate.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use allocative::FlameGraphBuilder;
use async_trait::async_trait;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::current_span;
use buck2_events::dispatch::get_dispatcher;
use buck2_events::dispatch::maybe_proxy_current_span;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::blob_store::BlobStore;
use buck2_execute::blob_store::FileToInsert;
use buck2_execute::blob_store::FileToLink;
use buck2_execute::blob_store::Origin;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionImmutableDirectory;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::directory::INTERNER;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::execute::clean_output_paths::cleanup_path;
use buck2_execute::execute::readonly_outputs::make_output_paths_writable;
use buck2_execute::execute::readonly_outputs::seal_files;
use buck2_execute::materialize::materializer::ArtifactNotMaterializedReason;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::CleanStaleArtifactsArgs;
use buck2_execute::materialize::materializer::CopiedArtifact;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::DeclareMatchOutcome;
use buck2_execute::materialize::materializer::HeldContent;
use buck2_execute::materialize::materializer::MaterializationError;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::MaterializeResponse;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::materialize::materializer::MaterializerBackgroundCleanupGuard;
use buck2_execute::materialize::materializer::MaterializerEntry;
use buck2_execute::materialize::materializer::MaterializerIterItem;
use buck2_execute::materialize::materializer::ReadLease;
use buck2_execute::materialize::materializer::WriteLease;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::re::manager::ReConnectionManager;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::file_name::FileName;
use buck2_hash::BuckMutSet;
use buck2_util::future::join_all;
use buck2_util::threads::thread_spawn;
use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::DisableCancellationGuard;
use dupe::Dupe;
use futures::FutureExt;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use jiff::Timestamp;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::materializers::deferred::AccessTimesUpdates;
use crate::materializers::deferred::DeferredMaterializerConfigs;
use crate::materializers::deferred::MaterializeEntryError;
use crate::materializers::deferred::MaterializerSizeStats;
use crate::materializers::deferred::SharedMaterializingError;
use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::ArtifactMaterializationMethod;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;
use crate::materializers::deferred::artifact_tree::artifact_metadata_matches_entry;
use crate::materializers::deferred::io_handler::DefaultIoHandler;
use crate::materializers::deferred::io_handler::IoHandler;
use crate::materializers::v2::clean_stale::Cleaner;
use crate::materializers::v2::entry::DiskEntry;
use crate::materializers::v2::index::DigestIndex;
use crate::materializers::v2::task::Outcome;
use crate::materializers::v2::task::Task;
use crate::materializers::v2::tree::DiskTree;
use crate::materializers::v2::tree::Effects;
use crate::materializers::v2::tree::Expect;
use crate::materializers::v2::tree::Hold;
use crate::materializers::v2::tree::Lookup;
use crate::materializers::v2::tree::ReadHold;
use crate::materializers::v2::tree::RequestId;
use crate::materializers::v2::tree::Tracked;
use crate::materializers::v2::tree::TreeChange;
use crate::materializers::v2::tree::Want;
use crate::materializers::v2::tree::WriteHold;
use crate::sqlite::materializer_db::MaterializerState;
use crate::sqlite::materializer_db::MaterializerStateSqliteDb;

/// Rolls this implementation out: a daemon runs it when `buck2.materializer_v2_version` is set
/// to at most this value, and the deferred materializer otherwise.
///
/// Bump this with any fix that a rollback waited on. Setting the config above every version in
/// the fleet rolls back; setting it to the fixed version then rolls out again to the binaries
/// that have the fix, and to no others.
pub const ROLLOUT_VERSION: u32 = 2;

/// How often buffered access-time bumps reach sqlite under `AccessTimesUpdates::Full`.
const ACCESS_TIMES_FLUSH_PERIOD: Duration = Duration::from_secs(5);

/// The materializer that tracks disk state only. `T` is the IO it runs through; the daemon uses
/// `DefaultIoHandler`, tests stub it.
#[derive(Allocative)]
pub struct DiskStateMaterializer<T: IoHandler + Allocative + 'static = DefaultIoHandler> {
    #[allocative(skip)]
    sender: Sender,
    #[allocative(skip)]
    thread: Option<std::thread::JoinHandle<()>>,
    io: Arc<T>,
    /// The BlobStore, whose stash the daemon attached: what it holds of a root is linked out of
    /// it before the CAS is asked for the rest.
    store: &'static BlobStore,
    /// Which tracked paths hold which file digests, the processor's to keep and the download
    /// tasks' to read (`index.rs`).
    index: Arc<DigestIndex>,
    #[allocative(skip)]
    blocking_executor: Arc<dyn BlockingExecutor>,
    materialize_final_artifacts: bool,
    /// Whether a reported root's files are made read-only (`buck2.readonly_outputs`).
    readonly_outputs: bool,
    #[allocative(skip)]
    next_request: AtomicU64,
    #[allocative(skip)]
    shared: Arc<Shared>,
    #[allocative(skip)]
    cleaner: Arc<Cleaner<T>>,
    entries_from_sqlite: u64,
}

impl DiskStateMaterializer<DefaultIoHandler> {
    pub fn new(
        fs: ProjectRoot,
        digest_config: DigestConfig,
        buck_out_path: ProjectRelativePathBuf,
        re_client_manager: Arc<ReConnectionManager>,
        blocking_executor: Arc<dyn BlockingExecutor>,
        configs: DeferredMaterializerConfigs,
        sqlite_db: Option<MaterializerStateSqliteDb>,
        sqlite_state: Option<MaterializerState>,
        daemon_dispatcher: EventDispatcher,
    ) -> buck2_error::Result<Self> {
        Self::new_with_io(
            Arc::new(DefaultIoHandler::new(
                fs,
                digest_config,
                buck_out_path,
                re_client_manager,
                blocking_executor.dupe(),
                // The report seals every root, whichever way it came to be on disk.
                false,
            )),
            BlobStore::global(),
            blocking_executor,
            configs,
            sqlite_db,
            sqlite_state,
            daemon_dispatcher,
        )
    }
}

impl<T: IoHandler + Allocative> DiskStateMaterializer<T> {
    /// Starts the processor thread. Must be called from within the daemon's tokio runtime, which
    /// is where the tasks that do IO run.
    pub(crate) fn new_with_io(
        io: Arc<T>,
        store: &'static BlobStore,
        blocking_executor: Arc<dyn BlockingExecutor>,
        configs: DeferredMaterializerConfigs,
        mut sqlite_db: Option<MaterializerStateSqliteDb>,
        sqlite_state: Option<MaterializerState>,
        daemon_dispatcher: EventDispatcher,
    ) -> buck2_error::Result<Self> {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let sender = Sender {
            tx,
            shared: shared.dupe(),
        };

        let mut tree = DiskTree::new();
        let index = Arc::new(DigestIndex::default());
        let mut entries_from_sqlite = 0;
        let mut refused = Vec::new();
        for entry in sqlite_state.into_iter().flatten() {
            // Rows are trusted, as the deferred core trusts its own: what they say is on disk is
            // tracked as such, and never checked against disk.
            let tracked = Tracked {
                entry: DiskEntry::from_action(&entry.metadata),
                last_access_time: entry.last_access_time,
                classification: entry.classification,
            };
            if tree.load(entry.path.clone(), tracked.dupe()) {
                shared.add(&tracked);
                index.added(&entry.path, &tracked.entry);
                entries_from_sqlite += 1;
            } else {
                refused.push(entry.path);
            }
        }
        if !refused.is_empty() {
            // Rows that nest come from a table neither core wrote; they are dropped rather than
            // crash every start, and the paths they named are materialized again when asked.
            buck2_core::soft_error!(
                "disk_state_materializer_nested_rows",
                internal_error!(
                    "{} rows of the materializer state nest with others and are dropped, the first: `{}`",
                    refused.len(),
                    refused[0]
                )
            );
            if let Some(db) = &mut sqlite_db {
                let _ignored = db.materializer_state_table().delete(refused);
            }
        }

        let strict = configs.strict_leases.then(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            let weak = WeakSender {
                tx: sender.tx.downgrade(),
                shared: shared.dupe(),
            };
            tokio::spawn(remove_freed_roots(
                io.dupe(),
                blocking_executor.dupe(),
                weak,
                rx,
            ));
            tx
        });
        let processor = Processor {
            sender: sender.dupe(),
            tree,
            index: index.dupe(),
            sqlite_db,
            touched: (!matches!(configs.update_access_times, AccessTimesUpdates::Disabled))
                .then(BuckMutSet::default),
            shared: shared.dupe(),
            strict,
        };
        let thread = thread_spawn("buck2-dsm", move || processor.run(rx))
            .buck_error_context("Cannot start materializer thread")?;
        if matches!(configs.update_access_times, AccessTimesUpdates::Full) {
            sender.start_access_times_flush(ACCESS_TIMES_FLUSH_PERIOD);
        }

        let cleaner = Cleaner::new(
            io.dupe(),
            blocking_executor.dupe(),
            sender.dupe(),
            shared.dupe(),
            &configs,
            daemon_dispatcher,
            store,
        );
        cleaner.start_schedule();

        Ok(Self {
            sender,
            thread: Some(thread),
            io,
            store,
            index,
            blocking_executor,
            materialize_final_artifacts: configs.materialize_final_artifacts,
            readonly_outputs: configs.readonly_outputs,
            next_request: AtomicU64::new(0),
            shared,
            cleaner,
            entries_from_sqlite,
        })
    }

    /// Runs `f` on the processor thread with its state, and hands back what it returns.
    async fn on_processor<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Processor) -> R + Send + 'static,
    ) -> buck2_error::Result<R> {
        let answer = self.sender.on_processor(f)?;
        answer.await.map_err(|_| stopped())
    }

    /// Ends the processor thread and waits for it, so that its sqlite connection is closed.
    #[cfg(test)]
    pub(crate) fn abort(mut self) {
        let _ignored = self.sender.send(Command::Abort);
        self.thread
            .take()
            .expect("not aborted twice")
            .join()
            .unwrap();
    }

    /// Asks for every want in one step, as one request. The grants arrive on the returned
    /// channels, in the order of the wants; each is awaited on its own. Dropping the withdrawal
    /// while some are still waiting withdraws them.
    fn acquire(
        &self,
        wants: Vec<(ProjectRelativePathBuf, Want)>,
    ) -> buck2_error::Result<(Vec<oneshot::Receiver<Granted>>, WithdrawOnDrop<'_>)> {
        disjoint(&wants)?;
        let id = RequestId(self.next_request.fetch_add(1, Ordering::Relaxed));
        let mut answers = Vec::with_capacity(wants.len());
        let mut paths = Vec::with_capacity(wants.len());
        let requests = wants
            .into_iter()
            .map(|(path, want)| {
                let (reply, answer) = oneshot::channel();
                answers.push(answer);
                paths.push(path.clone());
                Acquisition { path, want, reply }
            })
            .collect();
        let withdraw = WithdrawOnDrop {
            sender: &self.sender,
            request: Some((id, paths)),
        };
        self.sender.send(Command::Acquire { id, requests })?;
        Ok((answers, withdraw))
    }

    /// Asks for one want and waits for the grant.
    async fn acquire_one(
        &self,
        path: ProjectRelativePathBuf,
        want: Want,
    ) -> buck2_error::Result<Granted> {
        let (mut answers, withdraw) = self.acquire(vec![(path, want)])?;
        let granted = answers
            .pop()
            .expect("one want")
            .await
            .map_err(|_| stopped())?;
        withdraw.disarm();
        Ok(granted)
    }

    /// Records `entry` as what the holder put at the guard's root, which it holds in read mode
    /// from then on. Under `readonly_outputs` the root's files lose their write bits first, so
    /// that nothing writes through a name the tree, the index or the stash vouches for; an
    /// incremental action gets them back through `make_outputs_writable`.
    async fn report(
        &self,
        guard: &RootGuard,
        entry: ArtifactMetadata,
        classification: ArtifactClassification,
    ) -> buck2_error::Result<()> {
        if self.readonly_outputs {
            let (fs, root, files) = (self.io.fs().dupe(), guard.path().to_buf(), entry.dupe());
            self.blocking_executor
                .execute_io_inline(move || seal_files(&fs, &root, &files))
                .await?;
        }
        let hold = guard.take_write()?;
        let (reply, answer) = oneshot::channel();
        self.sender.send(Command::Report {
            hold,
            node: Tracked {
                entry: DiskEntry::from_action(&entry),
                last_access_time: Timestamp::now(),
                classification,
            },
            reply,
        })?;
        match answer.await.map_err(|_| stopped())? {
            Ok(hold) => {
                guard.put(Hold::Read(hold));
                Ok(())
            }
            Err((hold, e)) => {
                guard.put(Hold::Write(hold));
                Err(e)
            }
        }
    }

    /// Puts one content root on disk, or finds it there: the root's lease in read mode on
    /// success, the root's error otherwise.
    async fn materialize_root(
        &self,
        granted: oneshot::Receiver<Granted>,
        root: &Root,
        classification: ArtifactClassification,
        info: &Arc<CasDownloadInfo>,
        dispatcher: &EventDispatcher,
    ) -> Result<RootGuard, MaterializationError> {
        let failed = |source: buck2_error::Error| MaterializationError::Error {
            path: root.path.clone(),
            source,
        };
        let Granted {
            mut held,
            exclusions,
        } = granted.await.map_err(|_| failed(stopped()))?;
        if !held.guard().holds_for_writing() {
            // What is there already counts as a final output from here on, if it was not one.
            if classification == ArtifactClassification::FinalOutput {
                let path = root.path.clone();
                let _ignored = self
                    .on_processor(move |processor| processor.promote(&path))
                    .await;
            }
            return Ok(held.into_guard());
        }
        // A download of this very content that an earlier holder left running is taken over.
        // Anything else running near the root is cancelled and waited for first: nothing may be
        // writing there when this one does.
        let same = held.tasks.iter().position(|task| {
            task.root() == root.path
                && !task.is_cancelled()
                && artifact_metadata_matches_entry(task.entry(), &root.entry)
        });
        held.settle_except(same).await;
        if held.tasks.is_empty() {
            held.tasks.push(Task::spawn(
                root.path.clone(),
                root.entry.dupe(),
                download_task(
                    self.io.dupe(),
                    self.blocking_executor.dupe(),
                    self.store,
                    self.index.dupe(),
                    self.sender.dupe(),
                    root.path.clone(),
                    root.entry.dupe(),
                    exclusions,
                    info.dupe(),
                    dispatcher.dupe(),
                ),
            ));
        }
        match finished(held.finish().await) {
            Ok(()) => {
                self.report(held.guard(), root.entry.dupe(), classification)
                    .await
                    .map_err(failed)?;
                Ok(held.into_guard())
            }
            Err(SharedMaterializingError::Error(e)) => Err(failed(e)),
            Err(SharedMaterializingError::NotFound(source)) => {
                Err(MaterializationError::NotFound { source })
            }
        }
    }

    /// Waits for a root to be granted, in whichever mode, and for whatever was still running
    /// under it to end.
    async fn take(&self, granted: oneshot::Receiver<Granted>) -> buck2_error::Result<RootGuard> {
        let Granted { mut held, .. } = granted.await.map_err(|_| stopped())?;
        held.settle_except(None).await;
        Ok(held.into_guard())
    }

    /// Waits for every root of one request, in the order of its wants.
    async fn take_all(
        &self,
        answers: Vec<oneshot::Receiver<Granted>>,
    ) -> buck2_error::Result<Vec<RootGuard>> {
        join_all(answers.into_iter().map(|answer| self.take(answer)))
            .await
            .into_iter()
            .collect()
    }

    /// Takes every path for writing, as one request.
    async fn take_all_for_writing(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<Roots> {
        let (answers, withdraw) =
            self.acquire(paths.into_iter().map(|path| (path, Want::Write)).collect())?;
        let guards = self.take_all(answers).await?;
        withdraw.disarm();
        Ok(Roots(guards))
    }

    #[cfg(test)]
    pub(crate) fn store(&self) -> &'static BlobStore {
        self.store
    }

    /// How many access-time bumps wait for a flush to sqlite.
    #[cfg(test)]
    pub(crate) async fn pending_access_times(&self) -> buck2_error::Result<usize> {
        self.on_processor(|processor| processor.pending_access_times())
            .await
    }

    /// Puts a root's files into the store: content not known to be in the CAS, entering the
    /// store as this daemon produces it or finds it on disk. The root is held, so nothing
    /// replaces it meanwhile. A failure costs only the store's copy, which is why it is reported
    /// rather than failing the caller.
    async fn stash(&self, root: &Root) {
        let files = files_to_insert(self.io.fs(), &root.path, &root.entry, Origin::BuckOut);
        if let Err(e) = self.store.insert_files(files).await {
            buck2_core::soft_error_or_panic!(
                "blob_store_insert_failed",
                e.context(format!("Putting `{}` into the store", root.path))
            );
        }
    }

    /// Records content this daemon produced at the guard's root, an action's output or a
    /// publish: the stash takes it first, since it is nowhere else, then it is reported as an
    /// intermediate output. A download is reported through `report` alone: the CAS has it.
    async fn report_produced(&self, guard: &RootGuard, root: Root) -> buck2_error::Result<()> {
        self.stash(&root).await;
        self.report(guard, root.entry, ArtifactClassification::IntermediateOnly)
            .await
    }

    /// The roots behind a write lease this materializer handed out.
    fn roots_of<'a>(&self, lease: &'a WriteLease) -> buck2_error::Result<&'a Roots> {
        lease
            .guard()
            .and_then(|guard| guard.downcast_ref::<Roots>())
            .ok_or_else(|| {
                internal_error!(
                    "The disk-state materializer was handed a write lease it did not grant"
                )
            })
    }
}

fn stopped() -> buck2_error::Error {
    internal_error!("The materializer stopped before answering")
}

/// Refuses a request whose roots overlap: granted one, it would wait forever for the other.
fn disjoint(wants: &[(ProjectRelativePathBuf, Want)]) -> buck2_error::Result<()> {
    // Sorted component by component, so that a path nested in another directly follows it.
    let mut paths: Vec<&ProjectRelativePath> = wants.iter().map(|(path, _)| &**path).collect();
    paths.sort_by(|a, b| {
        a.iter()
            .map(FileName::as_str)
            .cmp(b.iter().map(FileName::as_str))
    });
    if let Some(pair) = paths.windows(2).find(|pair| pair[1].starts_with(pair[0])) {
        return Err(internal_error!(
            "One request asks for `{}` and for `{}` inside it",
            pair[0],
            pair[1]
        ));
    }
    Ok(())
}

/// A task's outcome as the holder that saw it to its end takes it. A task this holder owns is
/// only ever cancelled by this holder.
fn finished(outcome: Outcome) -> Result<(), SharedMaterializingError> {
    match outcome {
        Outcome::Done(result) => result,
        Outcome::Cancelled => Err(SharedMaterializingError::Error(internal_error!(
            "A task this materializer owns was cancelled under it"
        ))),
    }
}

fn duplicate(error: &MaterializationError) -> MaterializationError {
    match error {
        MaterializationError::Error { path, source } => MaterializationError::Error {
            path: path.clone(),
            source: source.dupe(),
        },
        MaterializationError::NotFound { source } => MaterializationError::NotFound {
            source: source.clone(),
        },
        MaterializationError::SqliteDbError { path, source } => {
            MaterializationError::SqliteDbError {
                path: path.clone(),
                source: source.dupe(),
            }
        }
    }
}

#[async_trait]
impl<T: IoHandler + Allocative> Materializer for DiskStateMaterializer<T> {
    async fn prevent_background_cleanup(&self) -> MaterializerBackgroundCleanupGuard {
        self.cleaner.prevent().await
    }

    async fn declare_existing(
        &self,
        lease: &WriteLease,
        artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()> {
        if artifacts.is_empty() {
            return Ok(());
        }
        let roots = self.roots_of(lease)?;
        let buck_out = self.io.buck_out_path().to_buf();
        for payload in artifacts {
            for root in split(payload.path, payload.artifact.entry().dupe(), &buck_out) {
                let guard = roots.at(&root.path).ok_or_else(|| {
                    internal_error!(
                        "`{}` was reported outside the write roots of its lease",
                        root.path
                    )
                })?;
                self.report_produced(guard, root).await?;
            }
        }
        Ok(())
    }

    async fn materialize(
        &self,
        request: MaterializeRequest,
    ) -> buck2_error::Result<MaterializeResponse> {
        let MaterializeRequest {
            artifacts,
            outputs,
            purpose,
            re_use_case,
        } = request;
        let skip = purpose == (MaterializationPurpose::FinalOutput { required: false })
            && !self.materialize_final_artifacts;
        let plan = if skip {
            RequestPlan::nothing(artifacts.len())
        } else {
            RequestPlan::new(&artifacts, self.io.buck_out_path())
        };
        let classification = match purpose {
            MaterializationPurpose::FinalOutput { .. } => ArtifactClassification::FinalOutput,
            MaterializationPurpose::IntermediateOnly => ArtifactClassification::IntermediateOnly,
        };
        let info = Arc::new(CasDownloadInfo::new_tracked(re_use_case));
        let dispatcher = get_dispatcher();

        // One request for the inputs and the outputs, then one future per root.
        let mut wants: Vec<(ProjectRelativePathBuf, Want)> = plan
            .roots
            .iter()
            .map(|root| (root.path.clone(), Want::Content(root.entry.dupe())))
            .collect();
        let has_outputs = !outputs.is_empty();
        wants.extend(outputs.into_iter().map(|path| (path, Want::Write)));
        let (mut answers, withdraw) = self.acquire(wants)?;
        let outputs = answers.split_off(plan.roots.len());
        let inputs = join_all(plan.roots.iter().zip(answers).map(|(root, answer)| {
            self.materialize_root(answer, root, classification, &info, &dispatcher)
        }));
        let outputs = self.take_all(outputs);
        let (inputs, outputs) = futures::join!(inputs, outputs);
        let outputs = outputs?;
        withdraw.disarm();

        let mut lease = Vec::with_capacity(plan.roots.len());
        let mut failures: Vec<Option<MaterializationError>> = Vec::with_capacity(plan.roots.len());
        for result in inputs {
            match result {
                Ok(guard) => {
                    lease.push(guard);
                    failures.push(None);
                }
                Err(e) => failures.push(Some(e)),
            }
        }
        // A pair is fine when every root it needs is; the first failure among them is its error.
        let results = plan
            .pair_roots
            .iter()
            .map(
                |roots| match roots.iter().find_map(|&root| failures[root].as_ref()) {
                    None => Ok(()),
                    Some(e) => Err(duplicate(e)),
                },
            )
            .collect();
        Ok(MaterializeResponse {
            results,
            lease: ReadLease::new(Roots(lease)),
            outputs: has_outputs.then(|| WriteLease::new(Roots(outputs))),
        })
    }

    /// A copy of buck-out content is nothing to this core: the destination's value names the
    /// same digests as the source's, and whoever needs the destination on disk materializes it
    /// from them, out of the store or the CAS. Nor is a symlink to a source, which copies
    /// nothing. A copy of a source file is the action's to make under `hold_or_take`
    /// (`materialize::produce`), as a local run makes its outputs; it does not come here.
    async fn declare_copy_impl(
        &self,
        _path: ProjectRelativePathBuf,
        _value: ArtifactValue,
        srcs: Vec<CopiedArtifact>,
    ) -> buck2_error::Result<()> {
        let buck_out = self.io.buck_out_path();
        let copies_a_source_file =
            |src: &&CopiedArtifact| !src.src.starts_with(buck_out) && has_files(&src.dest_entry);
        if let Some(src) = srcs.iter().find(copies_a_source_file) {
            return Err(internal_error!(
                "A copy of `{}`, outside buck-out, was declared to the disk-state materializer, which makes no copies: the action produces it under `hold_or_take`",
                src.src
            ));
        }
        Ok(())
    }

    /// Nothing to record: content in the CAS is reached through `materialize`, which carries the
    /// value, and the request's use case is what any download is attributed to.
    async fn declare_cas_many_impl<'a, 'b>(
        &self,
        _info: Arc<CasDownloadInfo>,
        _artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Not this core's: a write is the action's to make under `hold_or_take`
    /// (`materialize::produce`), as a local run makes its outputs.
    async fn declare_write<'a>(
        &self,
        _generate: Box<dyn FnOnce() -> buck2_error::Result<Vec<WriteRequest>> + Send + 'a>,
    ) -> buck2_error::Result<Vec<ArtifactValue>> {
        Err(internal_error!(
            "A write was declared to the disk-state materializer, which makes no writes: the action produces it under `hold_or_take`"
        ))
    }

    fn is_disk_state(&self) -> bool {
        true
    }

    /// Found content is backed from its path before it is handed out: a previous daemon's write
    /// or copy, whose store went with it.
    async fn hold_or_take(
        &self,
        path: ProjectRelativePathBuf,
        value: ArtifactValue,
    ) -> buck2_error::Result<HeldContent> {
        let entry = value.entry().dupe();
        let (answers, withdraw) =
            self.acquire(vec![(path.clone(), Want::Produce(entry.dupe()))])?;
        let mut guards = self.take_all(answers).await?;
        withdraw.disarm();
        let guard = guards.pop().expect("one want, one grant");
        if guard.holds_for_writing() {
            return Ok(HeldContent::Missing(WriteLease::new(Roots(vec![guard]))));
        }
        self.stash(&Root { path, entry }).await;
        Ok(HeldContent::Found(ReadLease::new(Roots(vec![guard]))))
    }

    /// A match is a use of the matched roots, as a read is: what a dep-file hit goes on relying
    /// on stays active for the cleaner, as it does under the deferred core.
    async fn declare_match(
        &self,
        artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
    ) -> buck2_error::Result<DeclareMatchOutcome> {
        let all_match = self
            .on_processor(move |processor| {
                let all_match = artifacts
                    .iter()
                    .all(|(path, value)| processor.tree.matches(path, value.entry()));
                if all_match {
                    let now = Timestamp::now();
                    let mut change = TreeChange::default();
                    for (path, _) in &artifacts {
                        change.touched.extend(processor.tree.touch(path, now));
                    }
                    processor.account(&change);
                    processor.persist(change);
                }
                all_match
            })
            .await?;
        Ok(all_match.into())
    }

    async fn has_artifact_at(&self, path: ProjectRelativePathBuf) -> buck2_error::Result<bool> {
        self.on_processor(move |processor| processor.tracked_entry(&path).is_some())
            .await
    }

    async fn prepare_outputs(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<WriteLease> {
        Ok(WriteLease::new(self.take_all_for_writing(paths).await?))
    }

    async fn make_outputs_writable(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()> {
        let fs = self.io.fs().dupe();
        self.blocking_executor
            .execute_io_inline(move || make_output_paths_writable(&fs, &paths))
            .await
    }

    async fn publish(
        &self,
        lease: &WriteLease,
        src: ProjectRelativePathBuf,
        dest: ProjectRelativePathBuf,
        value: ArtifactValue,
    ) -> buck2_error::Result<ReadLease> {
        if self.roots_of(lease)?.over(&src).is_none() {
            return Err(internal_error!(
                "`{src}` was published from outside the write roots of its lease"
            ));
        }
        let entry = value.entry().dupe();
        let Granted {
            mut held,
            exclusions,
        } = self
            .acquire_one(dest.clone(), Want::Content(entry.dupe()))
            .await?;
        let fs = self.io.fs().dupe();
        let src_abs = fs.resolve(&src);
        let dest_abs = fs.resolve(&dest);
        if !held.guard().holds_for_writing() {
            // Content-addressed destinations only ever hold one thing; the copy the action just
            // produced is redundant.
            self.blocking_executor
                .execute_io_inline(move || fs_util::remove_all(&src_abs).categorize_internal())
                .await?;
            return Ok(ReadLease::new(Roots(vec![held.into_guard()])));
        }
        // Content-addressed paths hold one content forever, so nothing is ever tracked strictly
        // under an untracked one. Were it, the rename would replace what its readers hold.
        if !exclusions.is_empty() {
            return Err(internal_error!(
                "`{dest}` has tracked content under it, which a publish cannot replace: [{}]",
                exclusions
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        held.settle_except(None).await;
        self.blocking_executor
            .execute_io_inline(move || {
                if let Some(parent) = dest_abs.parent() {
                    fs_util::create_dir_all(parent)?;
                }
                fs_util::remove_all(&dest_abs).categorize_internal()?;
                fs_util::rename(&src_abs, &dest_abs).categorize_internal()
            })
            .await?;
        self.report_produced(held.guard(), Root { path: dest, entry })
            .await?;
        Ok(ReadLease::new(Roots(vec![held.into_guard()])))
    }

    /// Paths without values: `ensure_materialized`'s, for the previous outputs of an incremental
    /// action. A root this core tracks is on disk, which is all the caller asks of it; one it
    /// does not track it has no value to make, and the action starts from what is there, which
    /// may be less than the deferred core would have given it (the plan's OQ-VALUE-STORE).
    async fn materialize_many(
        &self,
        _artifact_paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<BoxStream<'static, Result<(), MaterializationError>>> {
        Ok(futures::stream::empty().boxed())
    }

    async fn get_materialized_file_paths(
        &self,
        files: Vec<(ProjectRelativePathBuf, FileDigest)>,
    ) -> buck2_error::Result<Vec<Result<ProjectRelativePathBuf, ArtifactNotMaterializedReason>>>
    {
        self.on_processor(move |processor| {
            files
                .into_iter()
                .map(|(path, digest)| match processor.tracked_entry(&path) {
                    // Tracked with other content: what an earlier build of the same action put
                    // there, say. The caller materializes the content it wants first.
                    Some(DiskEntry::File(file)) if file.digest != digest => {
                        Err(ArtifactNotMaterializedReason::RequiresMaterialization { path })
                    }
                    Some(_) => Ok(path),
                    None => Err(ArtifactNotMaterializedReason::RequiresMaterialization { path }),
                })
                .collect()
        })
        .await
    }

    fn iterate(&self) -> buck2_error::Result<BoxStream<'static, MaterializerIterItem>> {
        let (reply, answer) = oneshot::channel();
        self.sender.send(Command::Sync(Box::new(move |processor| {
            let items: Vec<MaterializerIterItem> = processor
                .tree
                .iter()
                .into_iter()
                .map(|(path, tracked)| MaterializerIterItem {
                    artifact_path: path,
                    artifact_display: Box::new(TrackedDisplay {
                        last_access_time: tracked.last_access_time,
                        size: tracked.size(),
                    }),
                    deps: Vec::new(),
                })
                .collect();
            let _ignored = reply.send(items);
        })))?;
        Ok(
            async move { futures::stream::iter(answer.await.unwrap_or_default()) }
                .flatten_stream()
                .boxed(),
        )
    }

    async fn allocative(&self) -> buck2_error::Result<allocative::FlameGraphOutput> {
        self.on_processor(|processor| {
            let mut graph = FlameGraphBuilder::default();
            graph.visit_root(&processor.tree);
            graph.visit_root(&*processor.index);
            graph.finish()
        })
        .await
    }

    /// Every tracked root that is not on disk, by a stat of each, off the processor.
    fn fsck(
        &self,
    ) -> buck2_error::Result<BoxStream<'static, (ProjectRelativePathBuf, buck2_error::Error)>> {
        let (found, stream) = mpsc::unbounded_channel();
        let (reply, roots) = oneshot::channel();
        self.sender.send(Command::Sync(Box::new(move |processor| {
            let _ignored = reply.send(processor.tree.iter());
        })))?;
        let fs = self.io.fs().dupe();
        let blocking_executor = self.blocking_executor.dupe();
        tokio::spawn(async move {
            let Ok(roots) = roots.await else {
                return;
            };
            let _ignored = blocking_executor
                .execute_io_inline(move || {
                    for (path, _) in roots {
                        if let Err(e) = fs_util::symlink_metadata(fs.resolve(&path)) {
                            let _ignored = found.send((path, e.categorize_internal()));
                        }
                    }
                    Ok(())
                })
                .await;
        });
        Ok(UnboundedReceiverStream::new(stream).boxed())
    }

    /// TTLs are the standalone refresher's, not this core's.
    async fn refresh_ttls(&self, _min_ttl: i64) -> buck2_error::Result<()> {
        Ok(())
    }

    async fn get_ttl_refresh_log(&self) -> buck2_error::Result<String> {
        Ok(
            "The disk-state materializer does not refresh TTLs; the standalone refresher does"
                .to_owned(),
        )
    }

    async fn clean_stale_artifacts(
        &self,
        args: CleanStaleArtifactsArgs,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        self.cleaner.clean_stale(args).await
    }

    async fn clean_scratch(&self) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        self.cleaner.clean_scratch().await
    }

    async fn test_iter(&self, count: usize) -> buck2_error::Result<String> {
        self.on_processor(move |processor| {
            let start = Instant::now();
            let mut roots = 0;
            for _ in 0..count {
                roots = processor.tree.iter().len();
            }
            format!(
                "Elapsed for iter() over {roots} roots ({count} times): {:?}",
                Instant::now() - start
            )
        })
        .await
    }

    async fn flush_all_access_times(&self) -> buck2_error::Result<String> {
        let flushed = self
            .on_processor(|processor| processor.flush_access_times())
            .await??;
        Ok(format!("Flushed access times of {flushed} roots"))
    }

    fn log_materializer_state(&self, events: &EventDispatcher) {
        events.instant_event(buck2_data::MaterializerStateInfo {
            num_entries_from_sqlite: self.entries_from_sqlite,
        });
    }

    fn add_snapshot_stats(&self, snapshot: &mut buck2_data::Snapshot) {
        snapshot.deferred_materializer_queue_size =
            self.shared.queued.load(Ordering::Relaxed) as u64;
        let sizes = self.shared.sizes();
        snapshot.deferred_materializer_final_output_logical_bytes = sizes.final_output;
        snapshot.deferred_materializer_intermediate_only_logical_bytes = sizes.intermediate_only;
        let store = self.store.stats();
        snapshot.blob_store_entries = store.entries;
        snapshot.blob_store_bytes = store.bytes;
        snapshot.blob_store_removed_bytes = store.removed_bytes;
        snapshot.blob_store_served_files = store.served;
        snapshot.blob_store_memory_entries = store.memory_entries;
        snapshot.blob_store_memory_bytes = store.memory_bytes;
        snapshot.blob_store_memory_served_files = store.memory_served;
        snapshot.disk_state_materializer_copied_from_disk =
            self.shared.copied_from_disk.load(Ordering::Relaxed);
    }

    async fn get_artifact_entries_for_materialized_paths(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
        fetch_root_artifact_entries_for_subpaths: bool,
    ) -> buck2_error::Result<
        Vec<
            Option<(
                ProjectRelativePathBuf,
                ActionDirectoryEntry<ActionSharedDirectory>,
            )>,
        >,
    > {
        let digest_config = self.io.digest_config();
        let found: Vec<Option<(ProjectRelativePathBuf, DiskEntry)>> = self
            .on_processor(move |processor| {
                paths
                    .into_iter()
                    .map(|path| match processor.tree.lookup(&path) {
                        Lookup::Exact(tracked) => Some((path, tracked.entry.dupe())),
                        Lookup::Inside { root, .. } if fetch_root_artifact_entries_for_subpaths => {
                            Some((path, root.entry.dupe()))
                        }
                        Lookup::Inside { .. } | Lookup::UnderLeaf { .. } | Lookup::Untracked => {
                            None
                        }
                    })
                    .collect()
            })
            .await?;
        // The caller gets a value's entry, digests tracked, of what the tree holds plain.
        found
            .into_iter()
            .map(|found| {
                found
                    .map(|(path, entry)| Ok((path, entry.to_action(digest_config)?)))
                    .transpose()
            })
            .collect()
    }
}

impl<T: IoHandler + Allocative> Drop for DiskStateMaterializer<T> {
    fn drop(&mut self) {
        // The processor ends once every sender is gone, which is when the last lease has been
        // released; the daemon exits without waiting for it, like the old core's.
        let _ignored = self.thread.take();
    }
}

/// How a tracked root shows up in `audit deferred-materializer list`.
struct TrackedDisplay {
    last_access_time: Timestamp,
    size: u64,
}

impl std::fmt::Display for TrackedDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tracked (ts={}, size={})",
            self.last_access_time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            self.size
        )
    }
}

impl MaterializerEntry for TrackedDisplay {}

/// One path with the entry that is, or is to be, there. Never a non-exhaustive directory: those
/// are scaffolding around the roots inside them and are split up front.
#[derive(Debug)]
struct Root {
    path: ProjectRelativePathBuf,
    entry: ArtifactMetadata,
}

/// Splits `entry` at `path` into the roots it stands for: itself, unless it is a non-exhaustive
/// directory, in which case each child in turn. Roots outside buck-out (a dep region rooted in a
/// source tree) are dropped: nothing there is the materializer's to write.
fn split(
    path: ProjectRelativePathBuf,
    entry: ArtifactMetadata,
    buck_out: &ProjectRelativePath,
) -> Vec<Root> {
    fn go(
        path: ProjectRelativePathBuf,
        entry: ArtifactMetadata,
        buck_out: &ProjectRelativePath,
        out: &mut Vec<Root>,
    ) {
        match &entry {
            DirectoryEntry::Dir(dir) if !dir.exhaustiveness_hash().is_exhaustive() => {
                for (name, child) in dir.entries() {
                    go(path.join(name), child.dupe(), buck_out, out);
                }
            }
            _ => {
                if path.starts_with(buck_out) {
                    out.push(Root { path, entry });
                }
            }
        }
    }
    let mut out = Vec::new();
    go(path, entry, buck_out, &mut out);
    out
}

/// A request's pairs as the roots the materializer works on.
struct RequestPlan {
    roots: Vec<Root>,
    /// For each pair of the request, the indices of the roots it needs.
    pair_roots: Vec<Vec<usize>>,
}

impl RequestPlan {
    fn new(
        artifacts: &[(ProjectRelativePathBuf, ArtifactValue)],
        buck_out: &ProjectRelativePath,
    ) -> Self {
        // Every root every pair wants, with the pairs that want it.
        let mut wanted: Vec<(Root, Vec<usize>)> = Vec::new();
        for (pair, (path, value)) in artifacts.iter().enumerate() {
            let mut roots = split(path.clone(), value.entry().dupe(), buck_out);
            if let Some(deps) = value.deps() {
                // A value's deps are a non-exhaustive scaffold from the project root down to the
                // regions it captured; those regions are roots at their own project paths.
                roots.extend(split(
                    ProjectRelativePathBuf::unchecked_new(String::new()),
                    DirectoryEntry::Dir(deps.dupe()),
                    buck_out,
                ));
            }
            for root in roots {
                wanted.push((root, vec![pair]));
            }
        }
        // Nested roots normalize to the outermost: a request that names a directory and a path
        // inside it wants both to hold the same content, and the directory covers it. Sorted
        // component by component (not as strings, where `a-b` would sort between `a` and `a/c`),
        // every path under a root directly follows it.
        wanted.sort_by(|a, b| {
            a.0.path
                .iter()
                .map(FileName::as_str)
                .cmp(b.0.path.iter().map(FileName::as_str))
        });
        let mut roots: Vec<Root> = Vec::new();
        let mut root_pairs: Vec<Vec<usize>> = Vec::new();
        for (root, pairs) in wanted {
            match roots.last() {
                Some(kept) if root.path.starts_with(&kept.path) => {
                    root_pairs.last_mut().expect("kept").extend(pairs);
                }
                _ => {
                    roots.push(root);
                    root_pairs.push(pairs);
                }
            }
        }
        let mut pair_roots = vec![Vec::new(); artifacts.len()];
        for (root, pairs) in root_pairs.iter().enumerate() {
            for &pair in pairs {
                pair_roots[pair].push(root);
            }
        }
        Self { roots, pair_roots }
    }

    /// A plan that asks for nothing, for `pairs` pairs that all succeed.
    fn nothing(pairs: usize) -> Self {
        Self {
            roots: Vec::new(),
            pair_roots: vec![Vec::new(); pairs],
        }
    }
}

/// The IO of a content root this request has to write: whatever is there and not wanted goes,
/// entry by entry where something under the root is excluded, and the rest is put there, file by
/// file from the stash, then from a tracked path holding the same content, then from the CAS.
fn download_task<T: IoHandler>(
    io: Arc<T>,
    blocking_executor: Arc<dyn BlockingExecutor>,
    store: &'static BlobStore,
    index: Arc<DigestIndex>,
    sender: Sender,
    root: ProjectRelativePathBuf,
    entry: ArtifactMetadata,
    exclusions: Vec<ProjectRelativePathBuf>,
    info: Arc<CasDownloadInfo>,
    dispatcher: EventDispatcher,
) -> impl for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, Result<(), SharedMaterializingError>>
+ Send {
    // The requester's span is the materialization span's parent, as under the deferred core;
    // the task runs under the requester's dispatcher with that span proxied in.
    let parent_span = current_span();
    move |cancellations| {
        let scope = dispatcher.dupe();
        let download = async move {
            let _running = started(cancellations)?;
            let fs = io.fs().dupe();
            let (prune_root, prune_entry, prune_exclusions) =
                (root.clone(), entry.dupe(), exclusions.clone());
            blocking_executor
                .execute_io_inline(move || prune(&fs, &prune_root, &prune_entry, &prune_exclusions))
                .await
                .map_err(SharedMaterializingError::Error)?;

            let mut served: Vec<ProjectRelativePathBuf> = Vec::new();
            let (stashed, mut rest): (Vec<WantedFile>, Vec<WantedFile>) =
                wanted_files(&root, &entry, &exclusions)
                    .into_iter()
                    .partition(|wanted| wanted.digest.stash().is_some());
            if !stashed.is_empty() {
                let answers = store
                    .materialize(
                        stashed
                            .iter()
                            .map(|wanted| FileToLink {
                                digest: wanted.digest.dupe(),
                                executable: wanted.executable,
                                dest: io.fs().resolve(&wanted.path),
                            })
                            .collect(),
                    )
                    .await
                    .map_err(SharedMaterializingError::Error)?;
                for (wanted, answered) in stashed.into_iter().zip(answers) {
                    if answered {
                        served.push(wanted.path);
                    } else {
                        rest.push(wanted);
                    }
                }
            }
            // Only what the index has a path for is worth a round trip to the processor: a
            // path holding the content, held for reading for the length of the copy. A hold
            // that never waits cannot close a cycle with the write lease this task runs under.
            let indexed: Vec<WantedFile> = rest
                .extract_if(.., |wanted| index.holds(wanted.digest.data()))
                .collect();
            if !indexed.is_empty() {
                let sources = sender
                    .hold_sources(
                        indexed
                            .iter()
                            .map(|wanted| wanted.digest.data().dupe())
                            .collect(),
                    )
                    .await
                    .map_err(SharedMaterializingError::Error)?;
                let copies: Vec<(WantedFile, ProjectRelativePathBuf, RootGuard)> = indexed
                    .into_iter()
                    .zip(sources)
                    .filter_map(|(wanted, source)| {
                        source.map(|(path, guard)| (wanted, path, guard))
                    })
                    .collect();
                if !copies.is_empty() {
                    let fs = io.fs().dupe();
                    let copied = blocking_executor
                        .execute_io_inline(move || {
                            let mut copied = Vec::new();
                            for (wanted, source, guard) in copies {
                                match copy_from_disk(&fs, &source, &wanted.path, wanted.executable)
                                {
                                    Ok(()) => copied.push(wanted.path),
                                    // A miss: the CAS is asked.
                                    Err(e) => {
                                        buck2_core::soft_error_or_panic!(
                                            "disk_state_materializer_copy_failed",
                                            e.context(format!(
                                                "Copying `{source}` to `{}`",
                                                wanted.path
                                            ))
                                        );
                                    }
                                }
                                drop(guard);
                            }
                            Ok(copied)
                        })
                        .await
                        .map_err(SharedMaterializingError::Error)?;
                    sender
                        .shared
                        .copied_from_disk
                        .fetch_add(copied.len() as u64, Ordering::Relaxed);
                    served.extend(copied);
                }
            }
            if matches!(entry, DirectoryEntry::Leaf(_)) && !served.is_empty() {
                return Ok(());
            }
            let mut exclusions = exclusions;
            exclusions.extend(served);
            let delta = delta_entry(&entry, &root, &exclusions, io.digest_config())
                .map_err(SharedMaterializingError::Error)?;
            io.materialize_entry(
                root,
                Arc::new(ArtifactMaterializationMethod::CasDownload { info }),
                delta,
                dispatcher,
                cancellations,
            )
            .await
            .map_err(shared)
        };
        maybe_proxy_current_span(parent_span, move || with_dispatcher_async(scope, download))
            .boxed()
    }
}

/// A file a root needs on disk.
struct WantedFile {
    path: ProjectRelativePathBuf,
    digest: TrackedFileDigest,
    executable: bool,
}

/// The files of `entry` under `root` that are not excluded.
fn wanted_files(
    root: &ProjectRelativePath,
    entry: &ArtifactMetadata,
    exclusions: &[ProjectRelativePathBuf],
) -> Vec<WantedFile> {
    let mut files = Vec::new();
    let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
    while let Some((path, entry)) = walk.next() {
        let DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) = entry else {
            continue;
        };
        let path = root.join(path.get());
        if exclusions.iter().any(|excluded| path.starts_with(excluded)) {
            continue;
        }
        files.push(WantedFile {
            path,
            digest: file.digest.dupe(),
            executable: file.is_executable,
        });
    }
    files
}

/// Puts a copy of the tracked file at `source` at `dest`, with the mode asked for: a clone where
/// the filesystem has them, never a hardlink, so that nothing later written in place at one path
/// reaches the other. Leaves nothing behind on failure.
fn copy_from_disk(
    fs: &ProjectRoot,
    source: &ProjectRelativePath,
    dest: &ProjectRelativePath,
    executable: bool,
) -> buck2_error::Result<()> {
    let (source, dest) = (fs.resolve(source), fs.resolve(dest));
    let copied = (|| {
        if let Some(parent) = dest.parent() {
            fs_util::create_dir_all(parent)?;
        }
        fs_util::reflink_or_copy(&source, &dest).categorize_internal()?;
        fs_util::set_executable(&dest, executable).categorize_internal()?;
        buck2_error::Ok(())
    })();
    if copied.is_err() {
        let _ignored = fs_util::remove_all(&dest);
    }
    copied
}

/// Whether `entry` has a file anywhere in it.
fn has_files(entry: &ActionDirectoryEntry<ActionImmutableDirectory>) -> bool {
    let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
    while let Some((_, entry)) = walk.next() {
        if let DirectoryEntry::Leaf(ActionDirectoryMember::File(_)) = entry {
            return true;
        }
    }
    false
}

/// The files of `entry`, on disk under `root`, as the store wants them for insertion.
fn files_to_insert(
    fs: &ProjectRoot,
    root: &ProjectRelativePath,
    entry: &ArtifactMetadata,
    origin: Origin,
) -> Vec<FileToInsert> {
    let mut files = Vec::new();
    let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
    while let Some((path, entry)) = walk.next() {
        let DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) = entry else {
            continue;
        };
        files.push(FileToInsert {
            path: fs.resolve(root.join(path.get())),
            digest: file.digest.dupe(),
            executable: file.is_executable,
            origin,
        });
    }
    files
}

/// Marks a task as started: from here on it runs to its end whatever cancellation asks, so that
/// what it puts on disk is never half-written and its outcome is its real one. A cancellation that
/// came first stops it before it touches anything.
fn started(
    cancellations: &CancellationContext,
) -> Result<DisableCancellationGuard, SharedMaterializingError> {
    cancellations.try_disable_cancellation().ok_or_else(|| {
        SharedMaterializingError::Error(internal_error!(
            "A materialization task was cancelled before it started"
        ))
    })
}

fn shared(e: MaterializeEntryError) -> SharedMaterializingError {
    match e {
        MaterializeEntryError::Error(e) => SharedMaterializingError::Error(e),
        MaterializeEntryError::NotFound(e) => SharedMaterializingError::NotFound(e),
    }
}

/// Removes what is at `root` and not in `entry`, leaving the excluded paths untouched. Without
/// exclusions the root is simply cleared, which is also what makes room for it when a parent is
/// a file.
fn prune(
    fs: &ProjectRoot,
    root: &ProjectRelativePath,
    entry: &ArtifactMetadata,
    exclusions: &[ProjectRelativePathBuf],
) -> buck2_error::Result<()> {
    if exclusions.is_empty() {
        return cleanup_path(fs, root);
    }
    prune_at(&fs.resolve(root), root, entry, exclusions)
}

fn prune_at(
    abs: &AbsNormPath,
    rel: &ProjectRelativePath,
    entry: &ArtifactMetadata,
    exclusions: &[ProjectRelativePathBuf],
) -> buck2_error::Result<()> {
    let dir = match entry {
        DirectoryEntry::Dir(dir) => dir,
        // Files and symlinks are rewritten whole.
        DirectoryEntry::Leaf(_) => {
            return fs_util::remove_all(abs).categorize_internal();
        }
    };
    match fs_util::symlink_metadata_if_exists(abs)? {
        None => return Ok(()),
        Some(metadata) if !metadata.is_dir() => {
            return fs_util::remove_all(abs).categorize_internal();
        }
        Some(_) => {}
    }
    if dir.exhaustiveness_hash().is_exhaustive() {
        for child in fs_util::read_dir(abs).categorize_internal()? {
            let child = child.buck_error_context("Listing a directory being pruned")?;
            let wanted = child
                .file_name()
                .to_str()
                .and_then(|name| FileName::new(name).ok())
                .is_some_and(|name| dir.get(name).is_some());
            if !wanted {
                fs_util::remove_all(child.path()).categorize_internal()?;
            }
        }
    }
    for (name, child) in dir.entries() {
        let child_rel = rel.join(name);
        if exclusions.iter().any(|e| child_rel.starts_with(e)) {
            continue;
        }
        prune_at(&abs.join(name), &child_rel, &child, exclusions)?;
    }
    Ok(())
}

/// `entry` without the excluded subtrees: what the download has to fetch.
fn delta_entry(
    entry: &ArtifactMetadata,
    root: &ProjectRelativePath,
    exclusions: &[ProjectRelativePathBuf],
    digest_config: DigestConfig,
) -> buck2_error::Result<ArtifactMetadata> {
    let dir = match entry {
        DirectoryEntry::Dir(dir) if !exclusions.is_empty() => dir,
        _ => return Ok(entry.dupe()),
    };
    let mut builder = dir.dupe().into_builder();
    for excluded in exclusions {
        let relative = excluded
            .strip_prefix(root)
            .internal_error("an exclusion lies under its root")?;
        builder
            .remove_prefix(relative)
            .internal_error("an exclusion is a path inside the wanted directory")?;
    }
    Ok(DirectoryEntry::Dir(
        builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER),
    ))
}

/// Holds one root of a lease; dropping it releases the root.
pub(crate) struct RootGuard {
    sender: Sender,
    path: ProjectRelativePathBuf,
    /// Empty while a report is in flight, and once released.
    hold: Mutex<Option<Hold>>,
}

impl RootGuard {
    fn path(&self) -> &ProjectRelativePath {
        &self.path
    }

    fn holds_for_writing(&self) -> bool {
        matches!(&*self.hold.lock(), Some(Hold::Write(_)))
    }

    /// Takes the write hold out, for a report.
    fn take_write(&self) -> buck2_error::Result<WriteHold> {
        let mut hold = self.hold.lock();
        match hold.take() {
            Some(Hold::Write(write)) => Ok(write),
            other => {
                *hold = other;
                Err(internal_error!("`{}` is not held for writing", self.path))
            }
        }
    }

    fn put(&self, hold: Hold) {
        let previous = self.hold.lock().replace(hold);
        debug_assert!(previous.is_none(), "a guard holds one root once");
    }

    /// Releases the root, handing `tasks` back to the processor with it.
    fn release_with(self, tasks: Vec<Task>) {
        if let Some(hold) = self.hold.lock().take() {
            // If the processor is gone there is nothing left to release to.
            let _ignored = self.sender.send(Command::Release {
                holds: vec![hold],
                tasks,
            });
        }
    }
}

impl Drop for RootGuard {
    fn drop(&mut self) {
        if let Some(hold) = self.hold.lock().take() {
            let _ignored = self.sender.send(Command::Release {
                holds: vec![hold],
                tasks: Vec::new(),
            });
        }
    }
}

/// The roots of one lease. This is what [`ReadLease`] and [`WriteLease`] wrap; dropping it
/// releases them all in one message.
pub(crate) struct Roots(Vec<RootGuard>);

impl Roots {
    /// The guard of the root at exactly `path`.
    fn at(&self, path: &ProjectRelativePath) -> Option<&RootGuard> {
        self.0.iter().find(|guard| guard.path() == path)
    }

    /// The guard of the root at or above `path`.
    fn over(&self, path: &ProjectRelativePath) -> Option<&RootGuard> {
        self.0.iter().find(|guard| path.starts_with(guard.path()))
    }
}

impl Drop for Roots {
    fn drop(&mut self) {
        let holds: Vec<Hold> = self
            .0
            .iter()
            .filter_map(|guard| guard.hold.lock().take())
            .collect();
        if let (Some(guard), false) = (self.0.first(), holds.is_empty()) {
            let _ignored = guard.sender.send(Command::Release {
                holds,
                tasks: Vec::new(),
            });
        }
    }
}

/// A granted root while its holder works on it: the guard, and the tasks running under the root
/// that the holder has to see to their end. Dropping it hands the tasks back with the root, so
/// that a holder dropped mid-way leaves nothing running under nobody's lease.
pub(crate) struct Held {
    /// Taken by `into_guard`, so that the drop after it sends nothing.
    guard: Option<RootGuard>,
    pub(crate) tasks: Vec<Task>,
}

impl Held {
    pub(crate) fn guard(&self) -> &RootGuard {
        self.guard.as_ref().expect("taken only by `into_guard`")
    }

    /// Cancels every task but the one at `keep`, and waits for them to end. The kept task, if
    /// any, is the only one left afterwards.
    pub(crate) async fn settle_except(&mut self, keep: Option<usize>) {
        if let Some(keep) = keep {
            self.tasks.swap(0, keep);
        }
        let kept = keep.is_some() as usize;
        for task in &mut self.tasks[kept..] {
            task.cancel();
        }
        while self.tasks.len() > kept {
            self.tasks
                .last_mut()
                .expect("checked non-empty")
                .settle()
                .await;
            self.tasks.pop();
        }
    }

    /// Waits for the one task left to end, and lets go of it.
    async fn finish(&mut self) -> Outcome {
        let outcome = self
            .tasks
            .first_mut()
            .expect("a task to finish")
            .settle()
            .await;
        self.tasks.clear();
        outcome
    }

    /// The guard alone, once no task is left under it.
    fn into_guard(mut self) -> RootGuard {
        debug_assert!(self.tasks.is_empty(), "tasks would be lost");
        self.guard.take().expect("taken once")
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            guard.release_with(std::mem::take(&mut self.tasks));
        }
    }
}

/// The processor's answer to one want of a request.
pub(crate) struct Granted {
    pub(crate) held: Held,
    /// For a write: the tracked roots below it that the holder must leave alone.
    pub(crate) exclusions: Vec<ProjectRelativePathBuf>,
}

/// Withdraws whatever of a request is still queued if its requester stops waiting for it.
struct WithdrawOnDrop<'a> {
    sender: &'a Sender,
    request: Option<(RequestId, Vec<ProjectRelativePathBuf>)>,
}

impl WithdrawOnDrop<'_> {
    /// Every want of the request was granted; there is nothing left to withdraw.
    fn disarm(mut self) {
        self.request = None;
    }
}

impl Drop for WithdrawOnDrop<'_> {
    fn drop(&mut self) {
        if let Some((id, paths)) = self.request.take() {
            let _ignored = self.sender.send(Command::Cancel { id, paths });
        }
    }
}

/// One want of an `Acquire`, with where its grant goes.
struct Acquisition {
    path: ProjectRelativePathBuf,
    want: Want,
    reply: oneshot::Sender<Granted>,
}

enum Command {
    /// One request, all of its wants: queued in one step, so that between any two requests the
    /// queue order is the same at every root (see the module doc).
    Acquire {
        id: RequestId,
        requests: Vec<Acquisition>,
    },
    /// The root held holds `node` now; the hold comes back as a read hold, or as it was when the
    /// tree refuses the report.
    Report {
        hold: WriteHold,
        node: Tracked,
        reply: oneshot::Sender<Result<ReadHold, (WriteHold, buck2_error::Error)>>,
    },
    /// From a guard's drop. The tasks ran under one of the write holds.
    Release { holds: Vec<Hold>, tasks: Vec<Task> },
    /// From a requester that stopped waiting for a request still queued at some of `paths`.
    Cancel {
        id: RequestId,
        paths: Vec<ProjectRelativePathBuf>,
    },
    /// From the cleaner: each path taken for writing if it is as expected and free, in one
    /// step (see `DiskTree::take_for_cleaning`).
    TakeForCleaning {
        requests: Vec<(ProjectRelativePathBuf, Expect)>,
        reply: oneshot::Sender<Vec<Option<Granted>>>,
    },
    /// Each path held for reading if content is tracked there and free, never waiting (see
    /// `DiskTree::hold_for_reading`).
    HoldForReading {
        requests: Vec<(ProjectRelativePathBuf, Option<Timestamp>)>,
        reply: oneshot::Sender<Vec<Option<RootGuard>>>,
    },
    /// For each digest, a tracked path holding a file of it, held for reading, where the index
    /// has one that is free; never waiting.
    HoldSources {
        digests: Vec<FileDigest>,
        reply: oneshot::Sender<Vec<Option<(ProjectRelativePathBuf, RootGuard)>>>,
    },
    /// Anything that needs the state and no IO.
    Sync(Box<dyn FnOnce(&mut Processor) + Send>),
    #[cfg(test)]
    Abort,
}

/// What the requesters read without asking the processor: the queue's depth, and the tracked
/// bytes by classification, kept by the processor as roots come and go.
#[derive(Default)]
pub(crate) struct Shared {
    queued: AtomicUsize,
    final_output_bytes: AtomicU64,
    intermediate_only_bytes: AtomicU64,
    /// Files put on disk by a copy from a tracked path rather than a download, since the start.
    copied_from_disk: AtomicU64,
}

impl Shared {
    pub(crate) fn sizes(&self) -> MaterializerSizeStats {
        MaterializerSizeStats {
            final_output: self.final_output_bytes.load(Ordering::Relaxed),
            intermediate_only: self.intermediate_only_bytes.load(Ordering::Relaxed),
        }
    }

    fn bytes_of(&self, classification: ArtifactClassification) -> &AtomicU64 {
        match classification {
            ArtifactClassification::FinalOutput => &self.final_output_bytes,
            ArtifactClassification::IntermediateOnly => &self.intermediate_only_bytes,
        }
    }

    fn add(&self, tracked: &Tracked) {
        self.bytes_of(tracked.classification)
            .fetch_add(tracked.size(), Ordering::Relaxed);
    }

    fn remove(&self, tracked: &Tracked) {
        let _ignored = self.bytes_of(tracked.classification).try_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_sub(tracked.size())),
        );
    }
}

#[derive(Clone)]
pub(crate) struct Sender {
    tx: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
}

impl Dupe for Sender {}

/// A `Sender` that does not keep the processor running: it upgrades while a `Sender` exists.
struct WeakSender {
    tx: mpsc::WeakUnboundedSender<Command>,
    shared: Arc<Shared>,
}

impl WeakSender {
    fn upgrade(&self) -> Option<Sender> {
        Some(Sender {
            tx: self.tx.upgrade()?,
            shared: self.shared.dupe(),
        })
    }
}

impl Sender {
    /// Flushes the processor's buffered access times to sqlite every `period`, for as long as
    /// the processor runs. Must be called from within a tokio runtime.
    fn start_access_times_flush(&self, period: Duration) {
        let weak = WeakSender {
            tx: self.tx.downgrade(),
            shared: self.shared.dupe(),
        };
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(sender) = weak.upgrade() else {
                    return;
                };
                let flush = Command::Sync(Box::new(|processor| {
                    processor.flush_access_times_or_abandon()
                }));
                if sender.send(flush).is_err() {
                    return;
                }
            }
        });
    }

    fn send(&self, command: Command) -> buck2_error::Result<()> {
        self.shared.queued.fetch_add(1, Ordering::Relaxed);
        self.tx.send(command).map_err(|_| {
            self.shared.queued.fetch_sub(1, Ordering::Relaxed);
            internal_error!("The materializer has stopped")
        })
    }

    /// Runs `f` on the processor thread with its state; what it returns arrives on the channel.
    /// Synchronous, so that the closure's type stays out of the caller's future.
    pub(crate) fn on_processor<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Processor) -> R + Send + 'static,
    ) -> buck2_error::Result<oneshot::Receiver<R>> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::Sync(Box::new(move |processor| {
            let _ignored = reply.send(f(processor));
        })))?;
        Ok(answer)
    }

    /// Takes each path for writing on the cleaner's behalf, where it is as expected and free.
    pub(crate) async fn take_for_cleaning(
        &self,
        requests: Vec<(ProjectRelativePathBuf, Expect)>,
    ) -> buck2_error::Result<Vec<Option<Granted>>> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::TakeForCleaning { requests, reply })?;
        answer.await.map_err(|_| stopped())
    }

    /// Holds each path for reading where content is tracked there and free, never waiting.
    pub(crate) async fn hold_for_reading(
        &self,
        requests: Vec<(ProjectRelativePathBuf, Option<Timestamp>)>,
    ) -> buck2_error::Result<Vec<Option<RootGuard>>> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::HoldForReading { requests, reply })?;
        answer.await.map_err(|_| stopped())
    }

    /// For each digest, a tracked path holding a file of it, held for reading, where there is
    /// one that is free; never waiting.
    async fn hold_sources(
        &self,
        digests: Vec<FileDigest>,
    ) -> buck2_error::Result<Vec<Option<(ProjectRelativePathBuf, RootGuard)>>> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::HoldSources { digests, reply })?;
        answer.await.map_err(|_| stopped())
    }
}

/// The owner of the tree. Lives on its own thread; see the module doc.
pub(crate) struct Processor {
    sender: Sender,
    pub(crate) tree: DiskTree<oneshot::Sender<Granted>>,
    /// Which tracked paths hold which file digests, kept in step with the tree.
    index: Arc<DigestIndex>,
    sqlite_db: Option<MaterializerStateSqliteDb>,
    /// Roots whose access time changed in memory and not yet in sqlite; `None` when access
    /// times are not persisted (`buck2.update_access_times = disabled`).
    touched: Option<BuckMutSet<ProjectRelativePathBuf>>,
    shared: Arc<Shared>,
    /// Where intermediate roots nothing holds any more go to be removed from disk, under
    /// `strict_leases`.
    strict: Option<mpsc::UnboundedSender<Freed>>,
}

/// A tracked root nothing holds any more, for `remove_freed_roots`.
struct Freed {
    path: ProjectRelativePathBuf,
    last_access_time: Timestamp,
}

/// Directories under buck-out whose roots are read after the command that made them, by the
/// test runner and by whoever expands an external cell; `strict_leases` leaves those alone.
const READ_AFTER_THE_COMMAND: &[&str] = &["test", "external_cells"];

/// Under `strict_leases`: removes each root it is handed from disk, so that nothing reads it
/// without a lease. Whether the content could be put back is not checked: that depends on who
/// holds its digest, which has nothing to do with its leases, so a check here would be a
/// snapshot, and a root kept on its account is one an unleased reader gets away with. A request
/// for the content after this goes to the stash or the CAS and fails at the read when neither
/// has it.
async fn remove_freed_roots<T: IoHandler>(
    io: Arc<T>,
    blocking_executor: Arc<dyn BlockingExecutor>,
    sender: WeakSender,
    mut freed: mpsc::UnboundedReceiver<Freed>,
) {
    while let Some(root) = freed.recv().await {
        let Some(sender) = sender.upgrade() else {
            return;
        };
        remove_freed_root(&io, &blocking_executor, &sender, root).await;
    }
}

async fn remove_freed_root<T: IoHandler>(
    io: &Arc<T>,
    blocking_executor: &Arc<dyn BlockingExecutor>,
    sender: &Sender,
    freed: Freed,
) {
    let Freed {
        path,
        last_access_time,
    } = freed;
    let buck_out = io.buck_out_path();
    if READ_AFTER_THE_COMMAND.iter().any(|dir| {
        path.starts_with(buck_out.join(ProjectRelativePathBuf::unchecked_new(dir.to_string())))
    }) {
        return;
    }
    // Taken as the cleaner takes a stale root: refused when anything has used, held or waited
    // for it since it was freed.
    let taken = match sender
        .take_for_cleaning(vec![(path.clone(), Expect::Tracked { last_access_time })])
        .await
    {
        Ok(mut taken) => taken.pop().flatten(),
        Err(_) => return,
    };
    let Some(Granted { mut held, .. }) = taken else {
        return;
    };
    held.settle_except(None).await;
    let fs = io.fs().dupe();
    let removed = path.clone();
    if let Err(e) = blocking_executor
        .execute_io_inline(move || cleanup_path(&fs, &removed))
        .await
    {
        buck2_core::soft_error_or_panic!(
            "disk_state_materializer_strict_remove_failed",
            e.context(format!("Removing `{path}` on its release"))
        );
    }
    drop(held);
}

impl Processor {
    fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = commands.blocking_recv() {
            self.shared.queued.fetch_sub(1, Ordering::Relaxed);
            if !self.handle(command) {
                return;
            }
        }
    }

    /// Returns whether to keep running.
    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Acquire { id, requests } => {
                let mut effects = Effects::default();
                for Acquisition { path, want, reply } in requests {
                    effects.extend(self.tree.acquire(path, want, id, reply));
                }
                self.apply(effects);
            }
            Command::Report { hold, node, reply } => {
                let outcome = match self.tree.report(hold, node) {
                    Ok((hold, effects)) => {
                        self.apply(effects);
                        Ok(hold)
                    }
                    Err((hold, disagreement)) => {
                        let e = internal_error!(
                            "The report of `{}` disagrees with `{}`, tracked under it and excluded from its lease",
                            hold.path(),
                            disagreement.root
                        );
                        Err((hold, e))
                    }
                };
                if let Err(unreceived) = reply.send(outcome) {
                    // The reporter is gone; what it held is released here.
                    let effects = match unreceived {
                        Ok(hold) => self.tree.release_read(hold),
                        Err((hold, _)) => self.tree.release_write(hold, Vec::new()),
                    };
                    self.apply(effects);
                }
            }
            Command::Release { holds, tasks } => {
                let mut tasks = Some(tasks);
                let mut effects = Effects::default();
                for hold in holds {
                    effects.extend(match hold {
                        Hold::Read(hold) => self.tree.release_read(hold),
                        Hold::Write(hold) => self
                            .tree
                            .release_write(hold, tasks.take().unwrap_or_default()),
                    });
                }
                debug_assert!(
                    tasks.is_none_or(|tasks| tasks.is_empty()),
                    "tasks handed back without a write hold"
                );
                self.apply(effects);
            }
            Command::Cancel { id, paths } => {
                let mut effects = Effects::default();
                for path in paths {
                    effects.extend(self.tree.cancel(&path, id));
                }
                self.apply(effects);
            }
            Command::TakeForCleaning { requests, reply } => {
                let mut change = TreeChange::default();
                let taken: Vec<Option<Granted>> = requests
                    .into_iter()
                    .map(|(path, expect)| {
                        let (grant, one) = self.tree.take_for_cleaning(path, expect)?;
                        change.extend(one);
                        Some(self.granted(grant))
                    })
                    .collect();
                self.account(&change);
                self.persist(change);
                // A cleaner gone already drops the grants, whose guards release the roots.
                let _ignored = reply.send(taken);
            }
            Command::HoldForReading { requests, reply } => {
                let held: Vec<Option<RootGuard>> = requests
                    .into_iter()
                    .map(|(path, as_of)| {
                        let hold = self.tree.hold_for_reading(path, as_of)?;
                        Some(self.read_guard(hold))
                    })
                    .collect();
                let _ignored = reply.send(held);
            }
            Command::HoldSources { digests, reply } => {
                let held: Vec<Option<(ProjectRelativePathBuf, RootGuard)>> = digests
                    .into_iter()
                    .map(|digest| {
                        let candidates = self.index.holders(&digest);
                        candidates.into_iter().find_map(|path| {
                            let hold = self.tree.hold_for_reading(path.clone(), None)?;
                            Some((path, self.read_guard(hold)))
                        })
                    })
                    .collect();
                let _ignored = reply.send(held);
            }
            Command::Sync(f) => f(self),
            #[cfg(test)]
            Command::Abort => return false,
        }
        true
    }

    /// A guard for a read hold the tree handed out.
    fn read_guard(&self, hold: ReadHold) -> RootGuard {
        RootGuard {
            sender: self.sender.dupe(),
            path: hold.path().to_buf(),
            hold: Mutex::new(Some(Hold::Read(hold))),
        }
    }

    /// Makes the root at `path` a final output, which clean-stale keeps longer; sqlite follows.
    fn promote(&mut self, path: &ProjectRelativePath) {
        if let Some(change) = self.tree.promote(path, ArtifactClassification::FinalOutput) {
            self.apply(Effects {
                grants: Vec::new(),
                change,
                freed: Vec::new(),
            });
        }
    }

    /// Mirrors what an operation changed and hands its grants to their requesters.
    fn apply(&mut self, effects: Effects<oneshot::Sender<Granted>>) {
        let Effects {
            grants,
            change,
            freed,
        } = effects;
        self.account(&change);
        self.persist(change);
        for (reply, grant) in grants {
            // A requester gone already drops the grant, whose guard releases the root through
            // this same channel.
            let _ignored = reply.send(self.granted(grant));
        }
        if let Some(strict) = &self.strict {
            for path in freed {
                if let Lookup::Exact(tracked) = self.tree.lookup(&path)
                    && tracked.classification == ArtifactClassification::IntermediateOnly
                {
                    // The remover gone takes the mode with it.
                    let _ignored = strict.send(Freed {
                        last_access_time: tracked.last_access_time,
                        path,
                    });
                }
            }
        }
    }

    /// A grant as the requester gets it.
    fn granted(&self, grant: crate::materializers::v2::tree::Grant) -> Granted {
        Granted {
            held: Held {
                guard: Some(RootGuard {
                    sender: self.sender.dupe(),
                    path: grant.hold.path().to_buf(),
                    hold: Mutex::new(Some(grant.hold)),
                }),
                tasks: grant.tasks,
            },
            exclusions: grant.exclusions,
        }
    }

    /// Keeps the shared totals and the index in step with the tracked roots.
    fn account(&mut self, change: &TreeChange) {
        for (_, tracked) in &change.removed {
            self.shared.remove(tracked);
        }
        for (_, tracked) in &change.added {
            self.shared.add(tracked);
        }
        self.index.apply(change);
    }

    /// The entry tracked at `path`: the root's when `path` is a root, the directory's entry at the
    /// remaining path when it is inside a tracked directory.
    fn tracked_entry(&self, path: &ProjectRelativePath) -> Option<DiskEntry> {
        match self.tree.lookup(path) {
            Lookup::Exact(tracked) => Some(tracked.entry.dupe()),
            Lookup::Inside { entry, .. } => entry,
            Lookup::UnderLeaf { .. } | Lookup::Untracked => None,
        }
    }

    /// Mirrors a tree change into sqlite. The mirror may say less than the tree, never more: a
    /// row the next daemon would trust for content that is gone is a wrong build. So a failure
    /// to follow ends the mirroring, and the rows go as far as they can be made to.
    fn persist(&mut self, change: TreeChange) {
        let TreeChange {
            removed,
            added,
            touched,
        } = change;
        let Some(db) = &mut self.sqlite_db else {
            return;
        };
        let table = db.materializer_state_table();
        let result = (|| {
            if !removed.is_empty() {
                table.delete(removed.into_iter().map(|(path, _)| path).collect())?;
            }
            for (path, tracked) in &added {
                table.insert_disk_entry(
                    path,
                    &tracked.entry,
                    tracked.last_access_time,
                    tracked.classification,
                )?;
            }
            buck2_error::Ok(())
        })();
        match result {
            Ok(()) => {
                if let Some(pending) = &mut self.touched {
                    pending.extend(touched);
                    for (path, _) in &added {
                        pending.remove(path);
                    }
                }
            }
            Err(e) => self.abandon_mirror(e.context("Mirroring tracked state into sqlite")),
        }
    }

    /// Ends the mirroring after a failure: the rows go as far as they can be made to, so that
    /// no later start trusts them.
    fn abandon_mirror(&mut self, e: buck2_error::Error) {
        buck2_core::soft_error_or_panic!(
            "disk_state_materializer_persist",
            e.context("the mirror is abandoned")
        );
        if let Some(db) = &mut self.sqlite_db {
            let all: Vec<ProjectRelativePathBuf> =
                self.tree.iter().into_iter().map(|(path, _)| path).collect();
            let _ignored = db.materializer_state_table().delete(all);
        }
        self.sqlite_db = None;
        if let Some(pending) = &mut self.touched {
            pending.clear();
        }
    }

    /// Writes buffered access-time bumps to sqlite; returns how many.
    fn flush_access_times(&mut self) -> buck2_error::Result<usize> {
        let Some(pending) = &mut self.touched else {
            return Ok(0);
        };
        let touched = std::mem::take(pending);
        let n = touched.len();
        if let Some(db) = &mut self.sqlite_db
            && n > 0
        {
            db.materializer_state_table()
                .update_access_times(touched.iter().collect())?;
        }
        Ok(n)
    }

    /// The periodic flush: a failure ends the mirroring, as a failed insertion does.
    fn flush_access_times_or_abandon(&mut self) {
        if let Err(e) = self.flush_access_times() {
            self.abandon_mirror(e.context("Writing access times into sqlite"));
        }
    }

    /// How many access-time bumps wait for a flush.
    #[cfg(test)]
    pub(crate) fn pending_access_times(&self) -> usize {
        self.touched.as_ref().map_or(0, |pending| pending.len())
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_execute::directory::ActionDirectoryBuilder;
    use buck2_execute::directory::ActionDirectoryMember;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;

    use super::*;

    fn path(s: &str) -> ProjectRelativePathBuf {
        ProjectRelativePathBuf::unchecked_new(s.to_owned())
    }

    fn file(content: &str) -> ArtifactMetadata {
        DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
            digest: TrackedFileDigest::from_content(
                content.as_bytes(),
                DigestConfig::testing_default().cas_digest_config(),
            ),
            is_executable: false,
        }))
    }

    /// An exhaustive directory of `(relative path, content)` files.
    fn dir(files: &[(&str, &str)]) -> ArtifactMetadata {
        let mut builder = ActionDirectoryBuilder::empty_exhaustive();
        for (p, content) in files {
            builder
                .insert(
                    ForwardRelativePath::unchecked_new(p),
                    file(content).map_dir(|d| d.into_builder()),
                )
                .unwrap();
        }
        builder.mark_uniformly_exhaustive();
        DirectoryEntry::Dir(
            builder
                .fingerprint(DigestConfig::testing_default().as_directory_serializer())
                .shared(&*INTERNER),
        )
    }

    #[test]
    fn test_prune_keeps_the_excluded_paths_and_clears_the_rest() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        let root = path("buck-out/d");
        let exists = |p: &str| fs.resolve(path(p)).exists();
        // Wanted and rewritten whole, excluded and kept, unwanted, and wanted inside a
        // subdirectory that stays a directory.
        fs.write_file(path("buck-out/d/a"), "old a", false)?;
        fs.write_file(path("buck-out/d/b"), "B", false)?;
        fs.write_file(path("buck-out/d/x"), "X", false)?;
        fs.write_file(path("buck-out/d/sub/y"), "old y", false)?;
        fs.write_file(path("buck-out/d/sub/z"), "Z", false)?;
        let want = dir(&[("a", "A"), ("b", "B"), ("sub/y", "Y")]);

        prune(fs, &root, &want, &[path("buck-out/d/b")])?;
        assert!(!exists("buck-out/d/a"));
        assert_eq!(
            std::fs::read_to_string(fs.resolve(path("buck-out/d/b")))?,
            "B"
        );
        assert!(!exists("buck-out/d/x"));
        assert!(!exists("buck-out/d/sub/y"));
        assert!(!exists("buck-out/d/sub/z"));
        assert!(fs.resolve(path("buck-out/d/sub")).is_dir());

        // A file where a directory is wanted goes; without exclusions the whole root goes.
        fs.write_file(path("buck-out/e"), "E", false)?;
        prune(
            fs,
            &path("buck-out/e"),
            &dir(&[("a", "A")]),
            &[path("buck-out/e/never")],
        )?;
        assert!(!exists("buck-out/e"));
        prune(fs, &root, &want, &[])?;
        assert!(!exists("buck-out/d"));
        Ok(())
    }

    #[test]
    fn test_the_delta_leaves_out_the_excluded_subtrees() -> buck2_error::Result<()> {
        let want = dir(&[("a", "A"), ("b", "B"), ("sub/y", "Y")]);
        let root = path("buck-out/d");
        let digest_config = DigestConfig::testing_default();

        let delta = delta_entry(
            &want,
            &root,
            &[path("buck-out/d/b"), path("buck-out/d/sub")],
            digest_config,
        )?;
        let DirectoryEntry::Dir(delta) = delta else {
            panic!("a directory's delta is a directory");
        };
        let names: Vec<String> = delta
            .entries()
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(names, vec!["a"]);

        let whole = delta_entry(&want, &root, &[], digest_config)?;
        assert!(artifact_metadata_matches_entry(&whole, &want));
        Ok(())
    }

    #[test]
    fn test_the_plan_collapses_nested_roots_and_keeps_pairs_apart() {
        let buck_out = path("buck-out");
        let d = dir(&[("a", "A"), ("b", "B")]);
        let plan = RequestPlan::new(
            &[
                (path("buck-out/d/a"), ArtifactValue::new(file("A"), None)),
                (path("buck-out/d"), ArtifactValue::new(d, None)),
                (path("buck-out/e"), ArtifactValue::new(file("E"), None)),
                (path("source/f"), ArtifactValue::new(file("F"), None)),
            ],
            &buck_out,
        );
        let roots: Vec<&str> = plan.roots.iter().map(|root| root.path.as_str()).collect();
        assert_eq!(roots, vec!["buck-out/d", "buck-out/e"]);
        assert_eq!(plan.pair_roots, vec![vec![0], vec![0], vec![1], vec![]]);
    }

    #[test]
    fn test_a_request_may_not_overlap_itself() {
        assert!(disjoint(&[(path("a/b"), Want::Write), (path("a-b"), Want::Write)]).is_ok());
        assert!(disjoint(&[(path("a"), Want::Write), (path("a/b"), Want::Write)]).is_err());
        assert!(disjoint(&[(path("a"), Want::Write), (path("a"), Want::Write)]).is_err());
    }
}
