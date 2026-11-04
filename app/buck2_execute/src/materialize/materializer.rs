/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_data::clean_stale_result::Origin;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::ordered_entry_walk;
use buck2_error::ErrorTag;
use buck2_events::dispatch::EventDispatcher;
use derive_more::Display;
use dice::UserComputationData;
use dupe::Dupe;
use futures::stream::BoxStream;
use futures::stream::TryStreamExt;
use jiff::SignedDuration;
use jiff::Timestamp;
use remote_execution::TCode;

use crate::artifact_value::ArtifactValue;
use crate::directory::ActionDirectoryEntry;
use crate::directory::ActionDirectoryMember;
use crate::directory::ActionImmutableDirectory;
use crate::directory::ActionSharedDirectory;
use crate::execute::action_digest::ActionDigest;
use crate::re::error::RemoteExecutionError;

pub struct WriteRequest {
    pub path: ProjectRelativePathBuf,
    pub content: Vec<u8>,
    pub is_executable: bool,
    pub path_kind: BuckOutPathKind,
}

#[cold]
fn format_directory_entry_leaves(
    directory: &ActionDirectoryEntry<ActionSharedDirectory>,
) -> String {
    let walk = ordered_entry_walk(directory.as_ref());
    let only_files = walk
        .filter_map(|entry| match entry {
            DirectoryEntry::Leaf(ActionDirectoryMember::File(f)) => Some(&f.digest),
            _ => {
                // We only download files from RE, not symlinks or directories.
                // https://fburl.com/code/3o8ht6b6.
                None
            }
        })
        .with_paths();
    const MAX_COUNT: usize = 10;
    const TABULATION: &str = "    ";
    let mut count = 0;
    let mut result = String::new();
    for (path, digest) in only_files {
        count += 1;
        if count > MAX_COUNT {
            continue;
        }
        result.push_str(&format!("{TABULATION}{path}: {digest}\n"));
    }
    if count > MAX_COUNT {
        result.push_str(&format!(
            "{}... and {} more omitted",
            TABULATION,
            count - MAX_COUNT
        ));
    }
    result
}

#[derive(buck2_error::Error, Debug, Clone, Dupe)]
#[error(
    "Your build requires materializing an artifact that has expired in the \
    RE CAS and Buck does not have it. \
    This likely happened because your Buck daemon \
    has been online for a long time. This error is currently unrecoverable. \
    To proceed, you should restart Buck using `buck2 killall`.

Debug information:
  Path: {}
  Digest origin: {}
  Directory:\n{}", .path, .info.origin.as_display_for_not_found(), format_directory_entry_leaves(.directory))]
#[buck2(tag = MaterializationError)]
pub struct CasNotFoundError {
    pub path: Arc<ProjectRelativePathBuf>,
    pub info: Arc<CasDownloadInfo>,
    pub directory: ActionDirectoryEntry<ActionSharedDirectory>,
    #[source]
    pub error: Arc<buck2_error::Error>,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = MaterializationError)]
pub enum MaterializationError {
    #[error("Error materializing artifact at path `{}`", .path)]
    Error {
        path: ProjectRelativePathBuf,

        #[source]
        source: buck2_error::Error,
    },

    /// The artifact wasn't found. This typically means it expired in the CAS.
    #[error(transparent)]
    NotFound { source: CasNotFoundError },

    #[error("Error inserting entry into materializer state sqlite for artifact at `{}`", .path)]
    SqliteDbError {
        path: ProjectRelativePathBuf,

        #[source]
        source: buck2_error::Error,
    },
}

#[derive(Debug)]
pub struct DeclareArtifactPayload {
    pub path: ProjectRelativePathBuf,
    pub artifact: ArtifactValue,
}

/// Why a materialization is requested. Final outputs are accounted separately by clean-stale
/// (they stay on disk longer). A final output that is not `required` may be skipped altogether
/// when the daemon runs with `materializations = deferred_skip_final_artifacts`; a skipped
/// artifact reports success without being put on disk.
#[derive(Clone, Copy, Debug, Dupe, Eq, PartialEq)]
pub enum MaterializationPurpose {
    FinalOutput { required: bool },
    IntermediateOnly,
}

pub struct MaterializerBackgroundCleanupGuard {
    _guard: Box<dyn Send + Sync>,
}

impl MaterializerBackgroundCleanupGuard {
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self {
            _guard: Box::new(guard),
        }
    }
}

/// What a consumer asks the materializer to put on disk.
pub struct MaterializeRequest {
    /// Each value is materialized at its path. The exhaustiveness markings inside a value are
    /// honored; the paths themselves say nothing about their parents, which are left alone.
    pub artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
    /// Paths the caller is about to produce content at: its outputs and scratch paths. They stop
    /// being tracked, and the response's `outputs` lease holds them for writing. It is granted
    /// together with the read lease over `artifacts`, which is what lets one unit of work hold
    /// everything it needs without ever waiting while holding (problem-path-locking.md).
    pub outputs: Vec<ProjectRelativePathBuf>,
    pub purpose: MaterializationPurpose,
    /// Attributed with any CAS traffic this request causes.
    pub re_use_case: RemoteExecutorUseCase,
}

#[must_use = "check the per-artifact results and hold the lease while reading the paths"]
pub struct MaterializeResponse {
    /// One per requested artifact, in request order.
    pub results: Vec<Result<(), MaterializationError>>,
    pub lease: ReadLease,
    /// The write lease over the request's `outputs`; `None` when it had none.
    pub outputs: Option<WriteLease>,
}

impl MaterializeResponse {
    /// For requests without outputs: the read lease once every artifact materialized, else the
    /// first error. Callers that can make progress with some of the artifacts missing look at
    /// `results` instead.
    pub fn ensure_results_ok(self) -> Result<ReadLease, MaterializationError> {
        debug_assert!(
            self.outputs.is_none(),
            "a request with outputs takes its leases through `into_leases`"
        );
        Ok(self.into_leases()?.0)
    }

    /// Both leases once every artifact materialized, else the first error.
    pub fn into_leases(self) -> Result<(ReadLease, Option<WriteLease>), MaterializationError> {
        for result in self.results {
            result?;
        }
        Ok((self.lease, self.outputs))
    }
}

/// Released on drop; carries whatever an implementation needs to release.
struct Lease {
    guard: Option<Box<dyn Any + Send + Sync>>,
}

/// Held by the caller of [`Materializer::materialize`] for as long as it reads the paths that
/// request put on disk. Holding it tells the materializer those paths are in use, and an
/// implementation may hold off conflicting writes until it is dropped. The deferred
/// materializer does not, so today the guard only fixes the scope callers hold it over.
#[must_use = "hold the lease while reading the materialized paths"]
pub struct ReadLease {
    _lease: Lease,
}

impl ReadLease {
    /// A lease no implementation acts on.
    pub fn noop() -> Self {
        Self {
            _lease: Lease { guard: None },
        }
    }

    /// A lease released by dropping `guard`.
    pub fn new(guard: impl Any + Send + Sync) -> Self {
        Self {
            _lease: Lease {
                guard: Some(Box::new(guard)),
            },
        }
    }

    /// One lease over everything `leases` hold.
    pub fn join(leases: Vec<ReadLease>) -> Self {
        Self::new(leases)
    }
}

impl fmt::Debug for ReadLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReadLease")
    }
}

/// Held by a caller that produces content at paths it asked for through
/// [`Materializer::prepare_outputs`], [`Materializer::hold_or_take`] or the outputs of a
/// [`Materializer::materialize`] request, up to and including the report or publish that makes
/// that content tracked; both take the lease, so that content can only ever be reported by
/// whoever holds its paths. Holding it tells the materializer nothing else may touch those
/// paths; the deferred materializer does not enforce that, so there the guard only fixes the
/// scope. Dropping it releases the paths, whatever was reported under them: content the holder
/// put there and did not report is untracked, and whoever next takes the paths clears it.
#[must_use = "hold the lease while producing the outputs"]
pub struct WriteLease {
    lease: Lease,
}

impl WriteLease {
    /// A lease no implementation acts on.
    pub fn noop() -> Self {
        Self {
            lease: Lease { guard: None },
        }
    }

    /// A lease released by dropping `guard`, which the implementation gets back from
    /// [`WriteLease::guard`] when the lease is passed to it.
    pub fn new(guard: impl Any + Send + Sync) -> Self {
        Self {
            lease: Lease {
                guard: Some(Box::new(guard)),
            },
        }
    }

    /// What the lease was created with; `None` for a no-op lease.
    pub fn guard(&self) -> Option<&(dyn Any + Send + Sync)> {
        self.lease.guard.as_deref()
    }

    /// The lease as the holder's readers hold it once everything under it is reported: the roots
    /// stay held, for reading, until the returned lease is dropped.
    pub fn into_read_lease(self) -> ReadLease {
        ReadLease { _lease: self.lease }
    }
}

impl fmt::Debug for WriteLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WriteLease")
    }
}

/// What `Materializer::hold_or_take` answers.
#[must_use = "hold the lease while reading the path, or make the content and report it"]
pub enum HeldContent {
    /// The path holds the content; the lease holds it for reading.
    Found(ReadLease),
    /// The path does not hold the content, and the lease holds it for writing: the caller puts
    /// the content there, the whole of the path, and reports it with `declare_existing`.
    Missing(WriteLease),
}

/// A trait providing methods to asynchronously materialize artifacts.
///
/// # Invariants
///
/// 0. Before accessing an artifact on disk, `ensure_materialized` must be
///    called with its path. Alternatively, `get_materialized_file_paths` can
///    be used to *try* to find an already materialized path of a file with the
///    same contents as the desired path, if the contents are relevant but the
///    path isn't.
///
/// 1. If an artifact is declared twice followed by `ensure_materialized`, the
///    latest declaration is the one guaranteed to have been materialized.
///    However, if an external source modifies the artifact on disk after it's
///    been declared, there are no guarantees that after `ensure_materialized`
///    the declared artifact will be on disk (due to race conditions).
///
/// 2. If `declare_*` is called concurrently for two artifacts with conflicting
///    paths or subpaths, there are no guarantees on which of the declarations
///    will be used when materializing - possibly a mix of both.
///
/// 3. If `ensure_materialized` is called on a path that wasn't previously
///    declared, that path is not materialized and no errors are raised for it.
///
/// 4. Declare may delete any existing paths that conflict with the path that was
///    declared.
#[async_trait]
pub trait Materializer: Allocative + Send + Sync + 'static {
    /// Whether this materializer tracks disk state only, so that a producer that can make its
    /// content puts it on disk itself, under `hold_or_take`, and reports it, as a local run does
    /// (`materialize::produce`). The deferred core is declared to instead (`declare_write`,
    /// `declare_copy`).
    fn is_disk_state(&self) -> bool;

    /// Holds `path` for reading when it holds `value`'s content already, else for writing, for
    /// the caller to put the content there and report it with `declare_existing`: how a producer
    /// that can make its content asks a disk-state materializer (`is_disk_state`), so that
    /// content a previous build left on disk is found rather than made again. Found content is
    /// backed by the materializer's own means before it is handed out.
    async fn hold_or_take(
        &self,
        path: ProjectRelativePathBuf,
        value: ArtifactValue,
    ) -> buck2_error::Result<HeldContent>;

    /// Prevent background cleanup until the returned guard is dropped.
    async fn prevent_background_cleanup(&self) -> MaterializerBackgroundCleanupGuard {
        MaterializerBackgroundCleanupGuard::new(())
    }

    /// Reports content the caller put on disk at the paths of `artifacts`, which it holds
    /// `lease` for. From then on the content is the artifact at each path.
    async fn declare_existing(
        &self,
        lease: &WriteLease,
        artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()>;

    /// Puts the requested artifacts on disk at their paths and reports one result per artifact.
    /// This is how consumers (local actions reading their inputs, final materialization) ask for
    /// content; the `declare_*` methods remain for producers.
    async fn materialize(
        &self,
        request: MaterializeRequest,
    ) -> buck2_error::Result<MaterializeResponse>;

    async fn declare_copy_impl(
        &self,
        path: ProjectRelativePathBuf,
        value: ArtifactValue,
        srcs: Vec<CopiedArtifact>,
    ) -> buck2_error::Result<()>;

    async fn declare_cas_many_impl<'a, 'b>(
        &self,
        info: Arc<CasDownloadInfo>,
        artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()>;

    /// Write contents to paths. The output is ordered in the same order as the input. Implicitly
    /// cleans up paths that the WriteRequest declares. The deferred core's: a disk-state
    /// materializer refuses it, since its producers make their content under `hold_or_take`.
    async fn declare_write<'a>(
        &self,
        generate: Box<dyn FnOnce() -> buck2_error::Result<Vec<WriteRequest>> + Send + 'a>,
    ) -> buck2_error::Result<Vec<ArtifactValue>>;

    /// Stops tracking `paths` and returns the write lease the caller holds while it produces
    /// content there, up to and including the report or publish that makes that content
    /// tracked. Nothing on disk is touched: deleting what is there is the caller's job, done
    /// under the lease, since only the caller knows whether it wants a clean slate or a
    /// workspace kept across runs. A caller that also reads inputs asks for both in one
    /// [`Materializer::materialize`] request instead.
    async fn prepare_outputs(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<WriteLease>;

    /// Makes every file under `paths` one the caller may write in place, for an incremental
    /// action's previous outputs: an inode of its own, where a materializer keeps buck-out
    /// content linked elsewhere (the disk-state core's BlobStore), and its owner's write
    /// permission, where a materializer keeps tracked files read-only. One that does neither
    /// does nothing. Called under the write lease the caller holds over `paths`.
    async fn make_outputs_writable(
        &self,
        paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()>;

    /// Makes the untracked content an action produced at `src`, which it holds `lease` for, the
    /// artifact at `dest`, described by `value`. The content moves: `src` is the caller's to
    /// produce and not to keep. Returns once `dest` is on disk, with a lease over it for the
    /// readers that follow.
    async fn publish(
        &self,
        lease: &WriteLease,
        src: ProjectRelativePathBuf,
        dest: ProjectRelativePathBuf,
        value: ArtifactValue,
    ) -> buck2_error::Result<ReadLease>;

    /// Ask the materializer if the artifacts at the set of paths match what is on disk or
    /// declared. Returns Ok(Ok) if they do and Ok(Err) if they don't. It's a result not a boolean
    /// so you can't ignore it.
    async fn declare_match(
        &self,
        artifacts: Vec<(ProjectRelativePathBuf, ArtifactValue)>,
    ) -> buck2_error::Result<DeclareMatchOutcome>;

    /// Ask the materializer if there is a "tracked" artifact at the given path.
    ///
    /// While this method provides no information about what that artifact actually is, it can be
    /// used to verify that the artifact has not been eg corrupted by a partial clean-stale since
    /// being materialized.
    ///
    /// Furthermore, if this method returns `true`, the artifact is also treated as having been
    /// declared in the current daemon.
    ///
    /// This method does not guarantee that the artifact was materialized.
    async fn has_artifact_at(&self, path: ProjectRelativePathBuf) -> buck2_error::Result<bool>;

    /// Materialize artifacts paths. Returns a Stream with each element corresponding to one of the
    /// input paths, in the order that they were passed. This method provides access to
    /// individual results, but code that doesn't care about why an individual materialization
    /// failed should probably use `ensure_materialized` instead.
    async fn materialize_many(
        &self,
        artifact_paths: Vec<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<BoxStream<'static, Result<(), MaterializationError>>>;

    /// Given a list of artifact paths, blocks until all previously declared
    /// artifacts on that list are materialized. An [`Err`] is returned if the
    /// materialization fails for one or more of these paths.
    ///
    /// Paths that weren't previously declared are not materialized and no
    /// errors are raised for them.
    async fn ensure_materialized(
        &self,
        artifact_paths: Vec<ProjectRelativePathBuf>,
        _purpose: MaterializationPurpose,
    ) -> buck2_error::Result<()> {
        Ok(self
            .materialize_many(artifact_paths)
            .await?
            .try_collect()
            .await?)
    }

    /// For each file an upload is about to read, given as the path it is wanted at and the
    /// digest it is wanted with: a path holding that content, or why there is none yet.
    /// `RequiresMaterialization` asks the caller to materialize the file at the path first, which
    /// is also the answer for a path that holds other content than asked for;
    /// `RequiresCasDownload` says the content is the CAS's and was never put on disk. A path the
    /// materializer knows nothing about is returned as is, since that is usually a source file.
    ///
    /// TODO(rafaelc): analyze if we can get rid of this method without making
    /// Buck2 significantly slower, after we stop deferring local copies of
    /// artifacts that are already on disk.
    async fn get_materialized_file_paths(
        &self,
        files: Vec<(ProjectRelativePathBuf, FileDigest)>,
    ) -> buck2_error::Result<Vec<Result<ProjectRelativePathBuf, ArtifactNotMaterializedReason>>>;

    fn iterate(&self) -> buck2_error::Result<BoxStream<'static, MaterializerIterItem>>;

    async fn allocative(&self) -> buck2_error::Result<allocative::FlameGraphOutput>;

    /// Obtain a list of files that don't match their in-memory representation. This may not catch
    /// all discrepancies.
    fn fsck(
        &self,
    ) -> buck2_error::Result<BoxStream<'static, (ProjectRelativePathBuf, buck2_error::Error)>>;

    async fn refresh_ttls(&self, min_ttl: i64) -> buck2_error::Result<()>;

    async fn get_ttl_refresh_log(&self) -> buck2_error::Result<String>;

    async fn clean_stale_artifacts(
        &self,
        args: CleanStaleArtifactsArgs,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse>;

    /// Delete all local-action scratch (`buck-out/<iso>/tmp*`). Only sound while no
    /// command is running actions: scratch liveness is command-scoped. Aborts as soon
    /// as any other materializer command arrives.
    async fn clean_scratch(&self) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse>;

    async fn test_iter(&self, count: usize) -> buck2_error::Result<String>;

    async fn flush_all_access_times(&self) -> buck2_error::Result<String>;

    fn log_materializer_state(&self, events: &EventDispatcher);

    /// Inject stats into a snapshot.
    fn add_snapshot_stats(&self, snapshot: &mut buck2_data::Snapshot);

    /// Returns artifact entries for the given `paths`.
    ///
    /// If `fetch_root_artifact_entries_for_subpaths` is false, only returns entries
    /// for paths that exactly match a known artifact root.
    ///
    /// If true, also matches paths that are subpaths of a known artifact root,
    /// returning the root artifact's entry (not the subpath's). The subpath need
    /// not actually exist within the artifact's directory structure.
    ///
    /// Returns `None` for any path that doesn't match a known artifact.
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
    >;
}

#[derive(Copy, Clone, Dupe, Debug)]
#[must_use]
pub enum DeclareMatchOutcome {
    Match,
    NotMatch,
}

impl From<bool> for DeclareMatchOutcome {
    fn from(is_match: bool) -> Self {
        if is_match {
            Self::Match
        } else {
            Self::NotMatch
        }
    }
}

impl DeclareMatchOutcome {
    pub fn is_match(self) -> bool {
        match self {
            Self::Match => true,
            Self::NotMatch => false,
        }
    }
}

impl dyn Materializer {
    /// Declare an artifact at `path` whose files can be materialized by doing
    /// a local copy. Implicitly cleans up `path`.
    ///
    /// The dest of each copied artifact in `srcs` must either be equal to
    /// `path`, or be a subpath of `path`; otherwise, [`Err`] is returned.
    pub async fn declare_copy(
        &self,
        path: ProjectRelativePathBuf,
        value: ArtifactValue,
        srcs: Vec<CopiedArtifact>,
    ) -> buck2_error::Result<()> {
        self.check_declared_external_symlink(&value)?;
        self.declare_copy_impl(path, value, srcs).await
    }

    /// Declares a list of artifacts whose files can be materialized by
    /// downloading from the CAS.
    pub async fn declare_cas_many(
        &self,
        info: Arc<CasDownloadInfo>,
        artifacts: Vec<DeclareArtifactPayload>,
    ) -> buck2_error::Result<()> {
        for DeclareArtifactPayload {
            artifact: value, ..
        } in artifacts.iter()
        {
            self.check_declared_external_symlink(value)?;
        }
        self.declare_cas_many_impl(info, artifacts).await
    }

    /// External symlink is a hack used to resolve the symlink to the correct external hack.
    /// No external symlink should be declared on the materializer with a non-empty remaining
    /// path. This function runs a check on all declared artifacts and returns `Err` if they
    /// are external symlinks with an existing value on `remaining_path` and `Ok` otherwise.
    fn check_declared_external_symlink(&self, value: &ArtifactValue) -> buck2_error::Result<()> {
        if let DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(external_symlink)) =
            value.entry()
        {
            if !external_symlink.remaining_path().is_empty() {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Internal error: external symlink should not be declared on materializer with non-empty remaining path: '{}'",
                    external_symlink.dupe().to_path_buf().display()
                ));
            }
        }
        Ok(())
    }
}

/// Information to perform the copy of an artifact from `src` to `dest`.
#[derive(Debug, Allocative)]
pub struct CopiedArtifact {
    /// Source path of the copied artifact.
    pub src: ProjectRelativePathBuf,
    /// Destination path.
    pub dest: ProjectRelativePathBuf,
    /// Entry of the artifact at `dest`.
    pub dest_entry: ActionDirectoryEntry<ActionImmutableDirectory>,
    // Override the destination executable bit to +x (true) or -x (false)
    pub executable_bit_override: Option<bool>,
}

impl CopiedArtifact {
    pub fn new(
        src: ProjectRelativePathBuf,
        dest: ProjectRelativePathBuf,
        dest_entry: ActionDirectoryEntry<ActionImmutableDirectory>,
        executable_bit_override: Option<bool>,
    ) -> Self {
        Self {
            src,
            dest,
            dest_entry,
            executable_bit_override,
        }
    }
}

#[derive(Debug, Allocative)]
pub enum CasDownloadInfoOrigin {
    /// Declared by an action that executed on RE.
    Execution(ActionExecutionOrigin),

    /// Simply declared by an action.
    Declared,

    /// Declared by an action after buck2 itself confirmed the blob is present in the CAS. A
    /// later miss is therefore a CAS failure rather than a wrong user-supplied digest.
    Probed,

    /// A value the disk-state materializer was handed to put on disk, whose digests' invariant
    /// puts the content in the CAS or in the store; how it got there is not carried along. A miss
    /// is therefore a CAS failure, as for a probed blob.
    Tracked,

    /// Uploaded directly by the client.
    Uploaded,

    /// Produced as a test artifact managed by the test client.
    TestArtifact,
}

#[derive(Debug, Allocative)]
pub struct ActionExecutionOrigin {
    /// Digest of the action that led us to discover this CAS object. Reported, not tracked: what
    /// says whether this origin still guarantees the download is the action's own TTL below.
    action_digest: ActionDigest,

    /// When did we learn of the connection between this digest and the download it allows. This
    /// typically represents how much time has passed since we executed the action or hit in the
    /// action cache.
    ///
    /// NOTE: we do not store this as `std::time::Instant`, because `Instant::elapsed()` does
    /// not necessarily track wall time (which is what we rather care about when dealing with
    /// TTLs received from RE). In particular, `Instant::elapsed()` can be *very* far off if
    /// the system went to sleep (on MacOS, it will not increase during that time!).
    ///
    /// See: <https://github.com/rust-lang/rust/issues/79462>
    action_instant: Timestamp,

    /// The TTL we retrieved from RE for this action.
    ttl: SignedDuration,
}

impl ActionExecutionOrigin {
    fn action_age(&self) -> SignedDuration {
        // NOTE: This might return a negative duration if our time skewed a lot, but that's
        // actually totally fine, since we only care to know if this is < TTL.
        Timestamp::now().duration_since(self.action_instant)
    }
}

impl fmt::Display for CasDownloadInfoOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Execution(execution) => {
                write!(
                    f,
                    "{} retrieved {:.3} seconds ago with ttl = {:.3} seconds",
                    execution.action_digest,
                    execution.action_age().as_secs(),
                    execution.ttl.as_secs()
                )?;
            }
            Self::Declared => {
                write!(f, "declared")?;
            }
            Self::Probed => {
                write!(f, "probed")?;
            }
            Self::Tracked => {
                write!(f, "tracked")?;
            }
            Self::Uploaded => {
                write!(f, "uploaded by client")?;
            }
            Self::TestArtifact => {
                write!(f, "test artifact")?;
            }
        }

        Ok(())
    }
}

impl CasDownloadInfoOrigin {
    pub fn as_display_for_not_found(&self) -> CasDownloadInfoOriginNotFound<'_> {
        CasDownloadInfoOriginNotFound { inner: self }
    }

    /// Does the action cache guarantee this result exist? We expect the action cache to always
    /// return a TTL that is lower than the TTL of any of the outputs referenced by the action.
    pub fn guaranteed_by_action_cache(&self) -> bool {
        match self {
            Self::Execution(execution) => execution.action_age() < execution.ttl,
            Self::Declared | Self::Probed | Self::Tracked | Self::Uploaded | Self::TestArtifact => {
                // These origins do not carry an action cache result.
                false
            }
        }
    }
}

/// A Display wrapper for CasDownloadInfoOrigin in cases where this origin was not found (in those
/// cases we want to report potential action cache corruption).
pub struct CasDownloadInfoOriginNotFound<'a> {
    inner: &'a CasDownloadInfoOrigin,
}

impl fmt::Display for CasDownloadInfoOriginNotFound<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)?;
        if self.inner.guaranteed_by_action_cache() {
            write!(f, " (not expired: action cache corruption)")?;
        }

        Ok(())
    }
}

/// Information about a CAS download we might require when an artifact is not materialized.
#[derive(Debug, Display, Allocative)]
#[display("{}, re_use_case = {}", self.origin, self.re_use_case)]
pub struct CasDownloadInfo {
    pub origin: CasDownloadInfoOrigin,
    /// RE Use case to use when downloading this
    pub re_use_case: RemoteExecutorUseCase,
}

impl CasDownloadInfo {
    pub fn new_execution(
        action_digest: ActionDigest,
        re_use_case: RemoteExecutorUseCase,
        action_instant: Timestamp,
        ttl: SignedDuration,
    ) -> Self {
        Self {
            origin: CasDownloadInfoOrigin::Execution(ActionExecutionOrigin {
                action_digest,
                action_instant,
                ttl,
            }),
            re_use_case,
        }
    }

    pub fn new_declared(re_use_case: RemoteExecutorUseCase) -> Self {
        Self {
            origin: CasDownloadInfoOrigin::Declared,
            re_use_case,
        }
    }

    pub fn new_probed(re_use_case: RemoteExecutorUseCase) -> Self {
        Self {
            origin: CasDownloadInfoOrigin::Probed,
            re_use_case,
        }
    }

    pub fn new_tracked(re_use_case: RemoteExecutorUseCase) -> Self {
        Self {
            origin: CasDownloadInfoOrigin::Tracked,
            re_use_case,
        }
    }

    pub fn new_uploaded(re_use_case: RemoteExecutorUseCase) -> Self {
        Self {
            origin: CasDownloadInfoOrigin::Uploaded,
            re_use_case,
        }
    }

    pub fn new_test_artifact(re_use_case: RemoteExecutorUseCase) -> Self {
        Self {
            origin: CasDownloadInfoOrigin::TestArtifact,
            re_use_case,
        }
    }

    pub(crate) fn classify_result<T>(
        &self,
        result: buck2_error::Result<T>,
    ) -> buck2_error::Result<T> {
        result.map_err(|error| {
            if matches!(self.origin, CasDownloadInfoOrigin::Declared)
                && error
                    .find_typed_context::<RemoteExecutionError>()
                    .is_some_and(|error| error.code == TCode::NOT_FOUND)
            {
                error.tag([ErrorTag::DeclaredArtifactNotFound])
            } else {
                error
            }
        })
    }

    pub fn action_age(&self) -> Option<SignedDuration> {
        match &self.origin {
            CasDownloadInfoOrigin::Execution(execution) => Some(execution.action_age()),
            CasDownloadInfoOrigin::Declared
            | CasDownloadInfoOrigin::Probed
            | CasDownloadInfoOrigin::Tracked
            | CasDownloadInfoOrigin::Uploaded
            | CasDownloadInfoOrigin::TestArtifact => None,
        }
    }

    pub fn action_digest(&self) -> Option<&ActionDigest> {
        match &self.origin {
            CasDownloadInfoOrigin::Execution(execution) => Some(&execution.action_digest),
            CasDownloadInfoOrigin::Declared
            | CasDownloadInfoOrigin::Probed
            | CasDownloadInfoOrigin::Tracked
            | CasDownloadInfoOrigin::Uploaded
            | CasDownloadInfoOrigin::TestArtifact => None,
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum ArtifactNotMaterializedReason {
    #[error(
        "The artifact at path '{}' ({}) was produced by a RE action ({}), \
        but has not been downloaded",
        .path,
        .entry,
        .info.origin.as_display_for_not_found()
    )]
    RequiresCasDownload {
        path: ProjectRelativePathBuf,
        entry: ActionDirectoryEntry<ActionImmutableDirectory>,
        info: Arc<CasDownloadInfo>,
    },

    #[error("The artifact at path '{}' has not been downloaded yet", .path)]
    RequiresMaterialization { path: ProjectRelativePathBuf },

    #[error(
        "The artifact at path '{}' points into an entry ({}) \
        produced by a RE action ({}), but that entry does not contain \
        this exact path.",
        .path,
        .entry,
        .info.origin
    )]
    DeferredMaterializerCorruption {
        path: ProjectRelativePathBuf,
        entry: ActionDirectoryEntry<ActionSharedDirectory>,
        info: Arc<CasDownloadInfo>,
    },
}

// ==== dice ====

pub trait SetMaterializer {
    fn set_materializer(&mut self, materializer: Arc<dyn Materializer>);
}

pub trait HasMaterializer {
    fn get_materializer(&self) -> &dyn Materializer;

    fn get_materializer_handle(&self) -> Arc<dyn Materializer>;
}

impl SetMaterializer for UserComputationData {
    fn set_materializer(&mut self, materializer: Arc<dyn Materializer>) {
        self.data.set(materializer);
    }
}

impl HasMaterializer for UserComputationData {
    fn get_materializer(&self) -> &dyn Materializer {
        &**self
            .data
            .get::<Arc<dyn Materializer>>()
            .expect("Materializer should be set")
    }

    fn get_materializer_handle(&self) -> Arc<dyn Materializer> {
        self.data
            .get::<Arc<dyn Materializer>>()
            .expect("Materializer should be set")
            .dupe()
    }
}

#[derive(Clone, Copy, Debug, Dupe)]
pub enum FinalArtifactMaterialization {
    Enabled,
    Skipped,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum FinalArtifactMaterializationError {
    #[error(
        "Invalid value for buckconfig `[buck2] materializations`. Got `{0}`. Expected one of `all`, `deferred`, or `deferred_skip_final_artifacts`."
    )]
    InvalidValueForConfig(String),
}

impl FinalArtifactMaterialization {
    pub fn try_new_from_config_value(config_value: Option<&str>) -> buck2_error::Result<Self> {
        match config_value {
            None | Some("") | Some("deferred") | Some("all") => Ok(Self::Enabled),
            Some("deferred_skip_final_artifacts") => Ok(Self::Skipped),
            Some(v) => {
                Err(FinalArtifactMaterializationError::InvalidValueForConfig(v.to_owned()).into())
            }
        }
    }
}

/// This trait provides a level of indirection since the concrete implementation of
/// `MaterializerEntry` lives in a crate that depends on this one.
pub trait MaterializerEntry: Send + Sync + std::fmt::Display {}

pub struct MaterializerIterItem {
    pub artifact_path: ProjectRelativePathBuf,
    pub artifact_display: Box<dyn MaterializerEntry>,
    pub deps: Vec<(ProjectRelativePathBuf, &'static str)>,
}

#[derive(Debug, Clone)]
pub struct CleanStaleArtifactsArgs {
    pub policy: CleanStaleArtifactsPolicy,
    pub dry_run: bool,
    pub tracked_only: bool,
    pub origin: Origin,
}

#[derive(Debug, Clone)]
pub enum CleanStaleArtifactsPolicy {
    Configured,
    Explicit {
        keep_since_time: Timestamp,
        adaptive_low_disk_threshold: Option<f64>,
        adaptive_min_ttl: Option<std::time::Duration>,
        adaptive_unmaterialize_active: bool,
    },
}
