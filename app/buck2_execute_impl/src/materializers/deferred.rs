/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod clean_stale;
mod data_tree;
mod extension;
pub(crate) mod io_handler;
mod materialize_stack;

pub(crate) mod artifact_tree;
mod command_processor;
pub(crate) mod file_tree;
#[cfg(test)]
pub(crate) mod tests;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use artifact_tree::ArtifactClassification;
use artifact_tree::ArtifactMaterializationMethod;
use artifact_tree::ArtifactMaterializationStage;
use artifact_tree::ProcessingFuture;
use async_trait::async_trait;
use buck2_common::cancellation::CancellationScope;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::liveliness_observer::LivelinessGuard;
use buck2_core::fs::project::ProjectRoot;
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
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::materialize::materializer::ArtifactNotMaterializedReason;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::CasNotFoundError;
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
use buck2_execute::materialize::materializer::MaterializerIterItem;
use buck2_execute::materialize::materializer::ReadLease;
use buck2_execute::materialize::materializer::WriteLease;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::re::manager::ReConnectionManager;
use buck2_hash::BuckMutSet;
use buck2_util::threads::thread_spawn;
use derivative::Derivative;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::StreamExt;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use jiff::SignedDuration;
use jiff::Timestamp;
use parking_lot::Mutex;
use parking_lot::RwLock;
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::materializers::deferred::artifact_tree::ArtifactTree;
use crate::materializers::deferred::artifact_tree::Version;
use crate::materializers::deferred::clean_stale::CleanStaleConfig;
use crate::materializers::deferred::command_processor::DeferredMaterializerCommandProcessor;
use crate::materializers::deferred::command_processor::LowPriorityMaterializerCommand;
use crate::materializers::deferred::command_processor::MaterializerCommand;
use crate::materializers::deferred::file_tree::FileTree;
use crate::materializers::deferred::io_handler::DefaultIoHandler;
use crate::materializers::deferred::io_handler::IoHandler;
use crate::materializers::deferred::io_handler::NoDiskIoHandler;
use crate::sqlite::materializer_db::MaterializerState;
use crate::sqlite::materializer_db::MaterializerStateSqliteDb;

/// Materializer implementation that defers materialization of declared
/// artifacts until they are needed (i.e. `ensure_materialized` is called).
///
/// # Important
///
/// This materializer defers both CAS fetches and local copies. Therefore, one
/// needs to be careful when choosing to call `ensure_materialized`.
/// Between `declare` and `ensure` calls, the local files could have changed.
///
/// This limits us to only "safely" using the materializer within the
/// computation of a build rule, and only to materialize inputs or outputs of
/// the rule, not random artifacts/paths. That's because:
/// - file changes before/after a build are handled by DICE, which invalidates
///   the outputs that depend on it. The materializer ends up having the wrong
///   information about these outputs. But because it's only used within the
///   build rules, the affected rule is recomputed and therefore has its
///   artifacts re-declared. So when `ensure` is called the materializer has
///   up-to-date information about the artifacts.
/// - file changes during a build are not properly supported by Buck and
///   treated as undefined behaviour, so there's no need to worry about them.
#[derive(Allocative)]
pub struct DeferredMaterializerAccessor<T: IoHandler + 'static> {
    /// Sender to emit commands to the command loop. See `MaterializerCommand`.
    #[allocative(skip)]
    command_sender: Arc<MaterializerSender<T>>,
    /// Handle of the command loop thread. Aborted on Drop.
    /// This thread serves as a queue for declare/ensure requests, making
    /// sure only one executes at a time and in the order they came in.
    /// TODO(rafaelc): aim to replace it with a simple mutex.
    #[allocative(skip)]
    #[cfg_attr(not(test), expect(dead_code))]
    command_thread: Option<std::thread::JoinHandle<()>>,
    #[allocative(skip)]
    cancellation: Arc<CancellationScope>,
    #[allocative(skip)]
    runtime: Handle,
    /// Whether final outputs that are not required get materialized at all.
    materialize_final_artifacts: bool,
    defer_write_actions: bool,

    io: Arc<T>,

    /// Tracked for logging purposes.
    materializer_state_info: buck2_data::MaterializerStateInfo,

    stats: Arc<DeferredMaterializerStats>,

    #[allocative(skip)]
    background_cleanup_gate: Arc<BackgroundCleanupGate>,
}

/// Keeps background cleaning and the commands it would get in the way of apart: cleaning starts
/// only while nobody holds a prevention, and a prevention taken while cleaning runs waits for it
/// to stop.
#[derive(Default)]
pub(crate) struct BackgroundCleanupGate {
    state: Mutex<BackgroundCleanupGateState>,
    cleanup_finished: Notify,
}

#[derive(Default)]
struct BackgroundCleanupGateState {
    prevention_guards: usize,
    cleanup_running: bool,
}

pub(crate) struct BackgroundCleanupPreventionGuard {
    gate: Arc<BackgroundCleanupGate>,
}

impl BackgroundCleanupPreventionGuard {
    pub(crate) async fn wait_for_cleanup(&self) {
        loop {
            let notified = self.gate.cleanup_finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.gate.state.lock().cleanup_running {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for BackgroundCleanupPreventionGuard {
    fn drop(&mut self) {
        self.gate.state.lock().prevention_guards -= 1;
    }
}

pub(crate) struct BackgroundCleanupRunGuard {
    gate: Arc<BackgroundCleanupGate>,
}

impl Drop for BackgroundCleanupRunGuard {
    fn drop(&mut self) {
        self.gate.state.lock().cleanup_running = false;
        self.gate.cleanup_finished.notify_waiters();
    }
}

impl BackgroundCleanupGate {
    pub(crate) fn register_prevention(self: &Arc<Self>) -> BackgroundCleanupPreventionGuard {
        self.state.lock().prevention_guards += 1;
        BackgroundCleanupPreventionGuard { gate: self.dupe() }
    }

    pub(crate) fn try_start_cleanup(self: &Arc<Self>) -> Option<BackgroundCleanupRunGuard> {
        let mut state = self.state.lock();
        if state.prevention_guards != 0 || state.cleanup_running {
            return None;
        }
        state.cleanup_running = true;
        Some(BackgroundCleanupRunGuard { gate: self.dupe() })
    }
}

pub type DeferredMaterializer = DeferredMaterializerAccessor<DefaultIoHandler>;
pub type NoDiskDeferredMaterializer = DeferredMaterializerAccessor<NoDiskIoHandler>;

impl<T: IoHandler> Drop for DeferredMaterializerAccessor<T> {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let cancellation = self.cancellation.clone();
        let io = self.io.dupe();
        let _cleanup = self.runtime.spawn(async move {
            cancellation.wait().await;
            drop(io);
        });
        let _ignored = self.command_sender.send(MaterializerCommand::Abort);
    }
}

/// Statistics we collect while operating the Deferred Materializer.
#[derive(Allocative, Default)]
pub struct DeferredMaterializerStats {
    declares: AtomicU64,
    declares_reused: AtomicU64,
    sizes: RwLock<MaterializerSizeStats>,
}

#[derive(Allocative, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MaterializerSizeStats {
    pub(crate) final_output: u64,
    pub(crate) intermediate_only: u64,
}

impl DeferredMaterializerStats {
    pub(crate) fn sizes(&self) -> MaterializerSizeStats {
        *self.sizes.read()
    }

    pub(crate) fn add_materialized(&self, classification: ArtifactClassification, size: u64) {
        let mut sizes = self.sizes.write();
        match classification {
            ArtifactClassification::FinalOutput => sizes.final_output += size,
            ArtifactClassification::IntermediateOnly => sizes.intermediate_only += size,
        }
    }

    pub(crate) fn remove_materialized(&self, classification: ArtifactClassification, size: u64) {
        let mut sizes = self.sizes.write();
        let bucket = match classification {
            ArtifactClassification::FinalOutput => &mut sizes.final_output,
            ArtifactClassification::IntermediateOnly => &mut sizes.intermediate_only,
        };
        *bucket = bucket.saturating_sub(size);
    }

    pub(crate) fn promote_materialized(&self, size: u64) {
        let mut sizes = self.sizes.write();
        sizes.intermediate_only = sizes.intermediate_only.saturating_sub(size);
        sizes.final_output += size;
    }
}

pub struct DeferredMaterializerConfigs {
    pub materialize_final_artifacts: bool,
    pub defer_write_actions: bool,
    pub ttl_refresh: TtlRefreshConfiguration,
    pub update_access_times: AccessTimesUpdates,
    pub verbose_materializer_log: bool,
    pub clean_stale_config: CleanStaleConfig,
    /// Whether the files a materializer tracks are read-only from the moment it tracks them
    /// (`buck2.readonly_outputs`).
    pub readonly_outputs: bool,
    /// Removes a tracked intermediate root from disk the moment its last lease is released, so
    /// that a reader that outlives its lease fails where it would otherwise work by luck. For
    /// buck2's own tests of the disk-state core; the deferred core ignores it.
    pub strict_leases: bool,
}

pub struct TtlRefreshConfiguration {
    pub frequency: std::time::Duration,
    pub min_ttl: SignedDuration,
    pub enabled: bool,
}

impl TtlRefreshConfiguration {
    /// How long a CAS copy has to last for an artifact to count as re-fetchable: the refresher
    /// extends it within that time. `None` without a refresher.
    pub(crate) fn rematerialization_ttl(&self) -> Option<SignedDuration> {
        if self.enabled {
            SignedDuration::try_from(self.frequency).ok()
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Dupe, PartialEq)]
pub enum AccessTimesUpdates {
    /// Flushes when the buffer is full and periodically
    Full,
    ///Flushes only when buffer is full
    Partial,
    /// Does not flush at all
    Disabled,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum AccessTimesUpdatesError {
    #[error(
        "Invalid value for buckconfig `[buck2] update_access_times`. Got `{0}`. Expected one of `full`, `partial`  or `disabled`."
    )]
    InvalidValueForConfig(String),
}

impl AccessTimesUpdates {
    pub fn try_new_from_config_value(config_value: Option<&str>) -> buck2_error::Result<Self> {
        match config_value {
            None | Some("") | Some("full") => Ok(AccessTimesUpdates::Full),
            Some("partial") => Ok(AccessTimesUpdates::Partial),
            Some("disabled") => Ok(AccessTimesUpdates::Disabled),
            Some(v) => Err(AccessTimesUpdatesError::InvalidValueForConfig(v.to_owned()).into()),
        }
    }
}

#[derive(Default)]
struct MaterializerCounters {
    sent: AtomicUsize,
    received: AtomicUsize,
}

impl MaterializerCounters {
    fn ack_received(&self) {
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    fn queue_size(&self) -> usize {
        self.sent
            .load(Ordering::Relaxed)
            .saturating_sub(self.received.load(Ordering::Relaxed))
    }
}

pub struct MaterializerSender<T: 'static> {
    /// High priority commands are processed in order.
    high_priority: mpsc::UnboundedSender<MaterializerCommand<T>>,
    /// Low priority commands are processed in order relative to each other, but high priority
    /// commands can be reordered ahead of them.
    low_priority: mpsc::UnboundedSender<LowPriorityMaterializerCommand>,
    counters: Arc<MaterializerCounters>,
    /// Liveliness guard held while clean stale executes, dropped to interrupt clean.
    clean_guard: RwLock<Option<LivelinessGuard>>,
}

impl<T> MaterializerSender<T> {
    fn interrupt_clean(&self) {
        let read = self.clean_guard.read();
        if read.is_some() {
            drop(read);
            *self.clean_guard.write() = None;
        }
    }

    #[allow(clippy::result_large_err)]
    fn send(
        &self,
        command: MaterializerCommand<T>,
    ) -> Result<(), mpsc::error::SendError<MaterializerCommand<T>>> {
        self.interrupt_clean();
        let res = self.high_priority.send(command);
        self.counters.sent.fetch_add(1, Ordering::Relaxed);
        res
    }

    fn send_low_priority(&self, command: LowPriorityMaterializerCommand) -> bool {
        let sent = self.low_priority.send(command).is_ok();
        if sent {
            self.counters.sent.fetch_add(1, Ordering::Relaxed);
        }
        sent
    }
}

struct MaterializerReceiver<T: 'static> {
    high_priority: mpsc::UnboundedReceiver<MaterializerCommand<T>>,
    low_priority: mpsc::UnboundedReceiver<LowPriorityMaterializerCommand>,
    counters: Arc<MaterializerCounters>,
}

struct TtlRefreshHistoryEntry {
    at: Timestamp,
    outcome: Option<buck2_error::Result<()>>,
}

// NOTE: This doesn't derive `Error` and that's on purpose.  We don't want to make it easy (or
// possible, in fact) to add  `context` to this SharedProcessingError and lose the variant.
#[derive(Debug, Clone, Dupe)]
pub enum SharedMaterializingError {
    Error(buck2_error::Error),
    NotFound(CasNotFoundError),
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
pub enum MaterializeEntryError {
    #[error(transparent)]
    Error(buck2_error::Error),

    /// The artifact wasn't found. This typically means it expired in the CAS.
    #[error(transparent)]
    NotFound(CasNotFoundError),
}

impl From<buck2_error::Error> for MaterializeEntryError {
    fn from(e: buck2_error::Error) -> MaterializeEntryError {
        Self::Error(e)
    }
}

impl From<MaterializeEntryError> for SharedMaterializingError {
    fn from(e: MaterializeEntryError) -> SharedMaterializingError {
        match e {
            MaterializeEntryError::Error(e) => Self::Error(e),
            MaterializeEntryError::NotFound(e) => Self::NotFound(e),
        }
    }
}

#[async_trait]
impl<T: IoHandler + Allocative> Materializer for DeferredMaterializerAccessor<T> {
    async fn prevent_background_cleanup(&self) -> MaterializerBackgroundCleanupGuard {
        let guard = self.background_cleanup_gate.register_prevention();
        self.command_sender.interrupt_clean();
        guard.wait_for_cleanup().await;
        MaterializerBackgroundCleanupGuard::new(guard)
    }

    async fn declare_existing(
        &self,
        _lease: &WriteLease,
        artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()> {
        buck2_util::future::try_join_all(artifacts.iter().map(
            |DeclareArtifactPayload { path, artifact }| {
                self.io.seal(path.clone(), artifact.entry().dupe())
            },
        ))
        .await?;
        let cmd =
            MaterializerCommand::DeclareExisting(artifacts, current_span(), get_dispatcher_opt());
        self.command_sender.send(cmd)?;
        Ok(())
    }

    /// Requires the artifacts to have been declared, as every caller does today; the values are
    /// carried for the materializer that consumes them, this one only ensures the paths.
    async fn materialize(
        &self,
        request: MaterializeRequest,
    ) -> buck2_error::Result<MaterializeResponse> {
        let MaterializeRequest {
            artifacts,
            outputs,
            purpose,
            // Every download this materializer issues runs under the use case its artifact was
            // declared with, so there is nothing to attribute to the requester.
            re_use_case: _,
        } = request;

        // The outputs' preparation and the inputs' materialization are independent here, and
        // the executor used to run them side by side.
        let prepare = async {
            if outputs.is_empty() {
                Ok(None)
            } else {
                self.prepare_outputs(outputs).await.map(Some)
            }
        };
        let ensure = async {
            if purpose == (MaterializationPurpose::FinalOutput { required: false })
                && !self.materialize_final_artifacts
            {
                return buck2_error::Ok(artifacts.iter().map(|_| Ok(())).collect());
            }
            let paths = artifacts.into_iter().map(|(path, _)| path).collect();
            let (sender, recv) = oneshot::channel();
            self.command_sender
                .send(MaterializerCommand::Ensure(
                    paths,
                    purpose,
                    get_dispatcher(),
                    current_span(),
                    sender,
                ))
                .buck_error_context("Sending Ensure() command.")?;
            let materialization_fut = recv
                .await
                .buck_error_context("Receiving materialization future from command thread.")?;
            Ok(materialization_fut.collect::<Vec<_>>().await)
        };
        let (outputs, results) = futures::future::try_join(prepare, ensure).await?;

        Ok(MaterializeResponse {
            results,
            lease: ReadLease::noop(),
            outputs,
        })
    }

    async fn declare_copy_impl(
        &self,
        path: ProjectRelativePathBuf,
        value: ArtifactValue,
        srcs: Vec<CopiedArtifact>,
    ) -> buck2_error::Result<()> {
        // TODO(rafaelc): get rid of this tree; it'd save a lot of memory.
        let mut srcs_tree = FileTree::new();
        for copied_artifact in srcs.iter() {
            let dest = copied_artifact.dest.strip_prefix(&path)?;

            {
                let mut walk = unordered_entry_walk(
                    copied_artifact
                        .dest_entry
                        .as_ref()
                        .map_dir(Directory::as_ref),
                );
                while let Some((path, entry)) = walk.next() {
                    if let DirectoryEntry::Leaf(ActionDirectoryMember::File(..)) = entry {
                        let path = path.get();
                        let dest_iter = dest.iter().chain(path.iter()).map(|f| f.to_owned());
                        let src = copied_artifact.src.join(&path);
                        srcs_tree.insert(dest_iter, src);
                    }
                }
            }
        }
        let cmd = MaterializerCommand::Declare(
            DeclareArtifactPayload {
                path,
                artifact: value,
            },
            Box::new(ArtifactMaterializationMethod::LocalCopy(srcs_tree, srcs)),
            get_dispatcher(),
            current_span(),
        );
        self.command_sender.send(cmd)?;
        Ok(())
    }

    async fn declare_cas_many_impl<'a, 'b>(
        &self,
        info: Arc<CasDownloadInfo>,
        artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()> {
        for a in artifacts {
            let cmd = MaterializerCommand::Declare(
                a,
                Box::new(ArtifactMaterializationMethod::CasDownload { info: info.dupe() }),
                get_dispatcher(),
                current_span(),
            );
            self.command_sender.send(cmd)?;
        }
        Ok(())
    }

    fn is_disk_state(&self) -> bool {
        false
    }

    async fn hold_or_take(
        &self,
        path: ProjectRelativePathBuf,
        _value: ArtifactValue,
    ) -> buck2_error::Result<HeldContent> {
        Err(internal_error!(
            "`{path}` was asked for under `hold_or_take`, which the deferred core does not answer: producers declare to it"
        ))
    }

    async fn declare_write<'a>(
        &self,
        generate: Box<dyn FnOnce() -> buck2_error::Result<Vec<WriteRequest>> + Send + 'a>,
    ) -> buck2_error::Result<Vec<ArtifactValue>> {
        if !self.defer_write_actions {
            return self.io.immediate_write(generate).await;
        }

        let contents = generate()?;

        let mut paths = Vec::with_capacity(contents.len());
        let mut values = Vec::with_capacity(contents.len());
        let mut methods = Vec::with_capacity(contents.len());

        for WriteRequest {
            path,
            content,
            is_executable,
            path_kind: _,
        } in contents
        {
            let digest = TrackedFileDigest::from_content(
                &content,
                self.io.digest_config().cas_digest_config(),
            );

            let meta = FileMetadata {
                digest,
                is_executable,
            };

            paths.push(path);
            values.push(ArtifactValue::file(meta));
            methods.push(ArtifactMaterializationMethod::Write(WriteFile::try_new(
                content,
                is_executable,
            )?));
        }

        for (path, (value, method)) in std::iter::zip(paths, std::iter::zip(values.iter(), methods))
        {
            self.command_sender.send(MaterializerCommand::Declare(
                DeclareArtifactPayload {
                    path,
                    artifact: value.dupe(),
                },
                Box::new(method),
                get_dispatcher(),
                current_span(),
            ))?;
        }

        Ok(values)
    }

    async fn declare_match(
        &self,
        artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
    ) -> buck2_error::Result<DeclareMatchOutcome> {
        let (sender, recv) = oneshot::channel();

        self.command_sender
            .send(MaterializerCommand::MatchArtifacts(artifacts, sender))?;

        let is_match = recv
            .await
            .buck_error_context("Recv'ing match future from command thread.")?;

        Ok(is_match.into())
    }

    async fn has_artifact_at(&self, path: ProjectRelativePathBuf) -> buck2_error::Result<bool> {
        let (sender, recv) = oneshot::channel();

        self.command_sender.send(MaterializerCommand::HasArtifact(
            path,
            sender,
            get_dispatcher_opt(),
        ))?;

        let has_artifact = recv
            .await
            .buck_error_context("Receiving \"has artifact\" future from command thread.")?;

        Ok(has_artifact)
    }

    async fn prepare_outputs(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<WriteLease> {
        self.invalidate_many(paths).await?;
        Ok(WriteLease::noop())
    }

    /// This materializer neither shares an inode with buck-out nor makes a file read-only.
    async fn make_outputs_writable(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()> {
        self.io.make_outputs_writable(paths).await
    }

    async fn publish(
        &self,
        lease: &WriteLease,
        src: ProjectRelativePathBuf,
        dest: ProjectRelativePathBuf,
        value: ArtifactValue,
    ) -> buck2_error::Result<ReadLease> {
        // This materializer keeps `dest` as a copy of `src` rather than moving the content, and
        // makes the copy right away so that it happens while the caller still guarantees `src`
        // is intact. Moving would need writers serialized against readers of `dest`, which
        // nothing here provides; that is a property of the rewritten materializer. Leaving `src`
        // behind is harmless: nothing reads it until the action that owns it runs again, and
        // that run deletes it first.
        self.declare_existing(
            lease,
            vec![DeclareArtifactPayload {
                path: src.clone(),
                artifact: value.dupe(),
            }],
        )
        .await?;
        let materializer: &dyn Materializer = self;
        materializer
            .declare_copy(
                dest.clone(),
                value.dupe(),
                vec![CopiedArtifact::new(
                    src,
                    dest.clone(),
                    value.entry().dupe().map_dir(|d| d.as_immutable()),
                    None,
                )],
            )
            .await?;
        self.ensure_materialized(vec![dest], MaterializationPurpose::IntermediateOnly)
            .await?;
        Ok(ReadLease::noop())
    }

    async fn materialize_many(
        &self,
        artifact_paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<BoxStream<'static, Result<(), MaterializationError>>> {
        // TODO: display [materializing] in superconsole
        let (sender, recv) = oneshot::channel();
        self.command_sender
            .send(MaterializerCommand::Ensure(
                artifact_paths,
                MaterializationPurpose::IntermediateOnly,
                get_dispatcher(),
                current_span(),
                sender,
            ))
            .buck_error_context("Sending Ensure() command.")?;
        let materialization_fut = recv
            .await
            .buck_error_context("Receiving materialization future from command thread.")?;
        Ok(materialization_fut)
    }

    async fn ensure_materialized(
        &self,
        artifact_paths: Vec<ProjectRelativePathBuf>,
        purpose: MaterializationPurpose,
    ) -> buck2_error::Result<()> {
        let (sender, recv) = oneshot::channel();
        self.command_sender
            .send(MaterializerCommand::Ensure(
                artifact_paths,
                purpose,
                get_dispatcher(),
                current_span(),
                sender,
            ))
            .buck_error_context("Sending Ensure() command.")?;
        let materialization_fut = recv
            .await
            .buck_error_context("Receiving materialization future from command thread.")?;
        Ok(materialization_fut.try_collect().await?)
    }

    async fn get_materialized_file_paths(
        &self,
        files: Vec<(ProjectRelativePathBuf, FileDigest)>,
    ) -> buck2_error::Result<Vec<Result<ProjectRelativePathBuf, ArtifactNotMaterializedReason>>>
    {
        // This core answers from what was declared at the path, which is the content the caller
        // asks for or newer; the digest is not needed to tell.
        let paths: Vec<ProjectRelativePathBuf> = files.into_iter().map(|(path, _)| path).collect();
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let (sender, recv) = oneshot::channel();
        self.command_sender
            .send(MaterializerCommand::GetMaterializedFilePaths(paths, sender))?;
        Ok(recv.await?)
    }

    fn iterate(&self) -> buck2_error::Result<BoxStream<'static, MaterializerIterItem>> {
        self.iterate_impl()
    }

    async fn allocative(&self) -> buck2_error::Result<allocative::FlameGraphOutput> {
        self.allocative_impl().await
    }

    fn fsck(
        &self,
    ) -> buck2_error::Result<BoxStream<'static, (ProjectRelativePathBuf, buck2_error::Error)>> {
        self.fsck_impl()
    }

    async fn refresh_ttls(&self, min_ttl: i64) -> buck2_error::Result<()> {
        self.refresh_ttls_impl(min_ttl).await
    }

    async fn get_ttl_refresh_log(&self) -> buck2_error::Result<String> {
        self.get_ttl_refresh_log_impl().await
    }

    async fn clean_stale_artifacts(
        &self,
        args: CleanStaleArtifactsArgs,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        self.clean_stale_artifacts_impl(args).await
    }

    async fn clean_scratch(&self) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        self.clean_scratch_impl().await
    }

    async fn test_iter(&self, count: usize) -> buck2_error::Result<String> {
        self.test_iter_impl(count).await
    }

    async fn flush_all_access_times(&self) -> buck2_error::Result<String> {
        self.flush_all_access_times_impl().await
    }

    fn log_materializer_state(&self, events: &EventDispatcher) {
        events.instant_event(self.materializer_state_info)
    }

    fn add_snapshot_stats(&self, snapshot: &mut buck2_data::Snapshot) {
        snapshot.deferred_materializer_declares = self.stats.declares.load(Ordering::Relaxed);
        snapshot.deferred_materializer_declares_reused =
            self.stats.declares_reused.load(Ordering::Relaxed);
        snapshot.deferred_materializer_queue_size = self.command_sender.counters.queue_size() as _;
        let sizes = self.stats.sizes();
        snapshot.deferred_materializer_final_output_logical_bytes = sizes.final_output;
        snapshot.deferred_materializer_intermediate_only_logical_bytes = sizes.intermediate_only;
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
        let (sender, recv) = oneshot::channel();

        self.command_sender.send(
            MaterializerCommand::GetArtifactEntriesForMaterializedPaths {
                paths,
                fetch_root_artifact_entries_for_subpaths,
                sender,
            },
        )?;

        let result = recv.await.buck_error_context(
            "Receiving \"artifact entries for materialized paths\" future from command thread.",
        )?;

        Ok(result)
    }
}

impl<T: IoHandler + Allocative> DeferredMaterializerAccessor<T> {
    /// Stops tracking `paths`. Whatever is on disk there is left alone.
    pub(crate) async fn invalidate_many(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()> {
        let (sender, recv) = oneshot::channel();

        self.command_sender
            .send(MaterializerCommand::InvalidateFilePaths(
                paths,
                sender,
                get_dispatcher(),
                current_span(),
            ))?;

        // Wait on future to finish before invalidation can continue.
        let invalidate_fut = recv.await?;
        invalidate_fut.await
    }

    /// Spawns the command loop. Dropping the accessor stops the loop; outstanding I/O retains
    /// its handler until it finishes.
    fn new_with_io(
        io: Arc<T>,
        configs: DeferredMaterializerConfigs,
        sqlite_db: Option<MaterializerStateSqliteDb>,
        sqlite_state: Option<MaterializerState>,
        daemon_dispatcher: EventDispatcher,
    ) -> buck2_error::Result<Self> {
        let (high_priority_sender, high_priority_receiver) = mpsc::unbounded_channel();
        let (low_priority_sender, low_priority_receiver) = mpsc::unbounded_channel();

        let counters = Arc::new(MaterializerCounters::default());

        let command_sender = Arc::new(MaterializerSender {
            high_priority: high_priority_sender,
            low_priority: low_priority_sender,
            counters: counters.dupe(),
            clean_guard: RwLock::new(None),
        });

        let command_receiver = MaterializerReceiver {
            high_priority: high_priority_receiver,
            low_priority: low_priority_receiver,
            counters,
        };

        let stats = Arc::new(DeferredMaterializerStats::default());
        if let Some(sqlite_state) = &sqlite_state {
            for entry in sqlite_state {
                stats.add_materialized(
                    entry.classification,
                    artifact_tree::artifact_metadata_size(&entry.metadata),
                );
            }
        }

        let num_entries_from_sqlite = sqlite_state.as_ref().map_or(0, |s| s.len()) as u64;
        let materializer_state_info = buck2_data::MaterializerStateInfo {
            num_entries_from_sqlite,
        };
        let access_times_buffer =
            (!matches!(configs.update_access_times, AccessTimesUpdates::Disabled))
                .then(BuckMutSet::default);

        let rematerialization_ttl = configs.ttl_refresh.rematerialization_ttl();

        let tree = ArtifactTree::initialize(sqlite_state);

        let background_cleanup_gate = Arc::new(BackgroundCleanupGate::default());
        let cancellation = Arc::new(CancellationScope::new());
        let command_processor = {
            let command_sender = command_sender.dupe();
            let rt = Handle::current();
            let stats = stats.dupe();
            let io = io.dupe();
            let background_cleanup_gate = background_cleanup_gate.dupe();
            let cancellation = cancellation.clone();
            move |cancellations| {
                DeferredMaterializerCommandProcessor::new(
                    io,
                    sqlite_db,
                    rt,
                    configs.defer_write_actions,
                    command_sender,
                    tree,
                    cancellations,
                    stats,
                    access_times_buffer,
                    configs.verbose_materializer_log,
                    daemon_dispatcher,
                    configs.clean_stale_config.clone(),
                    rematerialization_ttl,
                    background_cleanup_gate,
                    cancellation,
                )
            }
        };

        let command_thread = thread_spawn("buck2-dm", {
            move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();

                let cancellations = CancellationContext::never_cancelled();

                rt.block_on(command_processor(cancellations).run(
                    command_receiver,
                    configs.ttl_refresh,
                    configs.update_access_times,
                ));
            }
        })
        .buck_error_context("Cannot start materializer thread")?;

        Ok(Self {
            command_thread: Some(command_thread),
            cancellation,
            runtime: Handle::current(),
            command_sender,
            materialize_final_artifacts: configs.materialize_final_artifacts,
            defer_write_actions: configs.defer_write_actions,
            io,
            materializer_state_info,
            stats,
            background_cleanup_gate,
        })
    }
}

impl DeferredMaterializerAccessor<DefaultIoHandler> {
    pub fn new(
        fs: ProjectRoot,
        digest_config: DigestConfig,
        buck_out_path: ProjectRelativePathBuf,
        re_client_manager: Arc<ReConnectionManager>,
        io_executor: Arc<dyn BlockingExecutor>,
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
                io_executor,
                configs.readonly_outputs,
            )),
            configs,
            sqlite_db,
            sqlite_state,
            daemon_dispatcher,
        )
    }
}

impl DeferredMaterializerAccessor<NoDiskIoHandler> {
    pub fn new_no_disk(
        fs: ProjectRoot,
        digest_config: DigestConfig,
        buck_out_path: ProjectRelativePathBuf,
        configs: DeferredMaterializerConfigs,
        daemon_dispatcher: EventDispatcher,
    ) -> buck2_error::Result<Self> {
        Self::new_with_io(
            Arc::new(NoDiskIoHandler::new(fs, digest_config, buck_out_path)),
            configs,
            None,
            None,
            daemon_dispatcher,
        )
    }

    pub fn testing_new_no_disk(fs: ProjectRoot) -> buck2_error::Result<Self> {
        Self::new_no_disk(
            fs,
            DigestConfig::testing_default(),
            ProjectRelativePathBuf::unchecked_new("buck-out/v2".to_owned()),
            DeferredMaterializerConfigs {
                materialize_final_artifacts: true,
                defer_write_actions: true,
                ttl_refresh: TtlRefreshConfiguration {
                    frequency: std::time::Duration::default(),
                    min_ttl: SignedDuration::ZERO,
                    enabled: false,
                },
                update_access_times: AccessTimesUpdates::Disabled,
                verbose_materializer_log: false,
                clean_stale_config: CleanStaleConfig::default(),
                readonly_outputs: false,
                strict_leases: false,
            },
            EventDispatcher::null(),
        )
    }
}

/// Wait on all futures in `futs` to finish. Return Error for first future that failed
/// in the Vec.
async fn join_all_existing_futs(
    existing_futs: Vec<(ProjectRelativePathBuf, ProcessingFuture)>,
) -> buck2_error::Result<()> {
    // We can await inside a loop here because all ProcessingFuture's are spawned.
    for (path, fut) in existing_futs.into_iter() {
        match fut {
            ProcessingFuture::Materializing(f) => {
                // We don't care about errors from previous materializations.
                // We are trying to delete anything that has been materialized,
                // so these errors can be ignored.
                f.await.ok();
            }
            ProcessingFuture::Cleaning(f) => {
                f.await.with_buck_error_context(|| {
                    format!(
                        "Error waiting for a previous future to finish cleaning output path {path}"
                    )
                })?;
            }
        };
    }

    Ok(())
}

#[derive(Allocative, Derivative)]
#[derivative(Debug)]
pub struct WriteFile {
    #[derivative(Debug = "ignore")]
    compressed_data: Box<[u8]>,
    decompressed_size: usize,
    is_executable: bool,
}

impl WriteFile {
    pub(crate) fn try_new(content: Vec<u8>, is_executable: bool) -> buck2_error::Result<Arc<Self>> {
        // NOTE: The zstd crate doesn't release extra capacity of its encoding buffer so it's
        // important to do so here (or the compressed Vec is the same capacity as the input!).
        let compressed_data = zstd::bulk::compress(&content, 0)
            .with_buck_error_context(|| format!("Error compressing {} bytes", content.len()))?
            .into_boxed_slice();

        Ok(Arc::new(Self {
            compressed_data,
            decompressed_size: content.len(),
            is_executable,
        }))
    }
}
