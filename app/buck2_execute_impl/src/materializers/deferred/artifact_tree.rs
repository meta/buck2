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
use allocative::Visitor;
use allocative::ident_key;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_ref::DirectoryRef;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::materialize::materializer::ArtifactNotMaterializedReason;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::CopiedArtifact;
use buck2_execute::output_size::OutputSize;
use derive_more::Display;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::Shared;
use jiff::Timestamp;
use tracing::instrument;

use crate::materializers::deferred::DeferredMaterializerStats;
use crate::materializers::deferred::SharedMaterializingError;
use crate::materializers::deferred::WriteFile;
use crate::materializers::deferred::file_tree::FileTree;
use crate::sqlite::materializer_db::MaterializerState;
use crate::sqlite::materializer_db::MaterializerStateEntry;
use crate::sqlite::materializer_db::MaterializerStateSqliteDb;

/// A future that is materializing on a separate task spawned by the materializer
pub(crate) type MaterializingFuture =
    Shared<BoxFuture<'static, Result<(), SharedMaterializingError>>>;
/// A future that is cleaning paths on a separate task spawned by the materializer
pub(crate) type CleaningFuture = Shared<BoxFuture<'static, buck2_error::Result<()>>>;

#[derive(Clone)]
pub(crate) enum ProcessingFuture {
    Materializing(MaterializingFuture),
    Cleaning(CleaningFuture),
}

/// Tree that stores materialization data for each artifact. Used internally by
/// the `DeferredMaterializer` to keep track of artifacts and how to
/// materialize them.
pub(crate) type ArtifactTree = FileTree<Box<ArtifactMaterializationData>>;

/// The Version of a processing future associated with an artifact. We use this to know if we can
/// clear the processing field when a callback is received, or if more work is expected.
#[derive(
    Eq, PartialEq, Copy, Clone, Dupe, Debug, Ord, PartialOrd, Display, Allocative
)]
pub struct Version(pub u64);

#[derive(Allocative)]
pub struct ArtifactMaterializationData {
    /// Taken from `deps` of `ArtifactValue`. Used to materialize deps of the artifact.
    pub(crate) deps: Option<ActionSharedDirectory>,
    pub(crate) classification: ArtifactClassification,
    pub(crate) logical_size_bytes: u64,
    pub(crate) stage: ArtifactMaterializationStage,
    /// An optional future that may be processing something at the current path
    /// (for example, materializing or deleting). Any other future that needs to process
    /// this path would need to wait on the existing future to finish.
    /// TODO(scottcao): Turn this into a queue of pending futures.
    pub(crate) processing: Processing,
}

#[derive(Allocative, Clone, Copy, Debug, Dupe, Eq, PartialEq)]
pub enum ArtifactClassification {
    FinalOutput,
    IntermediateOnly,
}

/// Represents a processing future + the version at which it was issued. When receiving
/// notifications about processing futures that finish, their changes are only applied if their
/// version is greater than the current version.
///
/// The version is an internal counter that is shared between the current processing_fut and
/// this data. When multiple operations are queued on a ArtifactMaterializationData, this
/// allows us to identify which one is current.
#[derive(Allocative)]
pub(crate) enum Processing {
    Done(Version),
    Active(Box<ActiveProcessing>),
}

#[derive(Allocative)]
pub(crate) struct ActiveProcessing {
    #[allocative(skip)]
    pub(crate) future: ProcessingFuture,
    pub(crate) version: Version,
}

impl Processing {
    pub(crate) fn active(future: ProcessingFuture, version: Version) -> Self {
        Self::Active(Box::new(ActiveProcessing { future, version }))
    }

    pub(crate) fn active_ref(&self) -> Option<&ActiveProcessing> {
        match self {
            Self::Done(..) => None,
            Self::Active(active) => Some(active),
        }
    }

    pub(crate) fn current_version(&self) -> Version {
        match self {
            Self::Done(version) => *version,
            Self::Active(active) => active.version,
        }
    }

    fn into_future(self) -> Option<ProcessingFuture> {
        match self {
            Self::Done(..) => None,
            Self::Active(active) => Some(active.future),
        }
    }
}

/// Metadata used to identify an artifact entry and stored for every materialized artifact.
pub type ArtifactMetadata = ActionDirectoryEntry<ActionSharedDirectory>;

pub(crate) fn artifact_metadata_matches_entry(
    metadata: &ArtifactMetadata,
    entry: &ArtifactMetadata,
) -> bool {
    match (metadata, entry) {
        (DirectoryEntry::Dir(d1), DirectoryEntry::Dir(d2)) => d1.fingerprint() == d2.fingerprint(),
        (DirectoryEntry::Leaf(l1), DirectoryEntry::Leaf(l2)) => {
            // In Windows, the 'executable bit' absence can cause Buck2 to re-download identical artifacts.
            // To avoid this, we exclude the executable bit from the comparison.
            if cfg!(windows) {
                if let (ActionDirectoryMember::File(meta1), ActionDirectoryMember::File(meta2)) =
                    (l1, l2)
                {
                    return meta1.digest == meta2.digest;
                }
            }
            l1 == l2
        }
        _ => false,
    }
}

pub(crate) fn artifact_metadata_size(metadata: &ArtifactMetadata) -> u64 {
    match metadata {
        DirectoryEntry::Dir(_) => metadata.calc_output_count_and_bytes(false).bytes,
        DirectoryEntry::Leaf(ActionDirectoryMember::File(file_metadata)) => {
            file_metadata.digest.size()
        }
        DirectoryEntry::Leaf(_) => 0,
    }
}

#[derive(Allocative)]
pub enum ArtifactMaterializationStage {
    /// The artifact was declared, but the materialization hasn't started yet.
    /// If it did start but end with an error, it returns to this stage.
    /// When the artifact is declared, we spawn a deletion future to delete
    /// all existing paths that conflict with the output paths.
    Declared {
        /// Taken from `entry` of `ArtifactValue`. Used to materialize the actual artifact.
        entry: ActionDirectoryEntry<ActionSharedDirectory>,
        method: Arc<ArtifactMaterializationMethod>,
    },
    /// This artifact was materialized
    Materialized {
        /// Once the artifact is materialized, we don't need the full entry anymore.
        /// We can throw away most of the entry and just keep some metadata used to
        /// check if materialized artifact matches declared artifact.
        metadata: ArtifactMetadata,
        /// Used to clean older artifacts from buck-out.
        last_access_time: Timestamp,
        /// How to recreate this artifact after discarding its local contents.
        rematerialization_method: Option<Arc<ArtifactRematerializationMethod>>,
        /// Artifact declared by running daemon.
        /// Should not be deleted without invalidating DICE nodes, which currently
        /// means killing the daemon.
        active: bool,
    },
}

/// Different ways to materialize the files of an artifact. Some artifacts need
/// to be fetched from the CAS, others copied locally.
#[derive(Debug, Display)]
pub enum ArtifactMaterializationMethod {
    /// The files must be copied from a local path.
    #[display("local copy")]
    LocalCopy(
        /// A map `[dest => src]`, meaning that a file at
        /// `{artifact_path}/{dest}/{p}` needs to be copied from `{src}/{p}`.
        FileTree<ProjectRelativePathBuf>,
        /// Raw list of copied artifacts, as received in `declare_copy`.
        Vec<CopiedArtifact>,
    ),

    #[display("write")]
    Write(Arc<WriteFile>),

    /// The files must be fetched from the CAS.
    #[display("cas download (action: {})", info.origin)]
    CasDownload {
        /// The digest of the action that produced this output
        info: Arc<CasDownloadInfo>,
    },

    #[cfg(test)]
    Test,
}

/// A materialization method that can recreate an artifact from a remote source, so the
/// artifact's local contents can be discarded and fetched again later.
#[derive(Allocative, Debug, Display)]
pub struct ArtifactRematerializationMethod(Arc<ArtifactMaterializationMethod>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnmaterializationEligibility {
    Eligible,
    EligibleAfterUpload,
    Ineligible(UnmaterializationIneligibilityReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnmaterializationIneligibilityReason {
    FinalOutput,
    NoRematerializationMethod,
    UploadTooLarge,
    RemoteTtlTooShort,
    Processing,
    StateChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UnmaterializationIneligibleArtifact {
    pub(crate) reason: UnmaterializationIneligibilityReason,
    pub(crate) size: u64,
}

#[derive(Debug)]
pub(crate) struct UnmaterializationUpload {
    pub(crate) path: ProjectRelativePathBuf,
    pub(crate) size: u64,
    pub(crate) entry: ActionDirectoryEntry<ActionSharedDirectory>,
    pub(crate) version: Version,
}

#[derive(Default)]
pub(crate) struct UnmaterializeArtifactsResult {
    pub(crate) unmaterialized: Vec<(ProjectRelativePathBuf, u64)>,
    pub(crate) uploads: Vec<UnmaterializationUpload>,
    pub(crate) ineligible: Vec<UnmaterializationIneligibleArtifact>,
}

impl UnmaterializeArtifactsResult {
    fn record_ineligible(&mut self, reason: UnmaterializationIneligibilityReason, size: u64) {
        self.ineligible
            .push(UnmaterializationIneligibleArtifact { reason, size });
    }
}

impl ArtifactRematerializationMethod {
    pub(crate) fn from_materialization_method(
        method: &Arc<ArtifactMaterializationMethod>,
    ) -> Option<Arc<Self>> {
        match method.as_ref() {
            ArtifactMaterializationMethod::CasDownload { .. } => {
                Some(Arc::new(Self(method.dupe())))
            }
            _ => None,
        }
    }

    pub(crate) fn materialization_method(&self) -> &Arc<ArtifactMaterializationMethod> {
        &self.0
    }

    /// Whether every blob backing this artifact is expected to still be fetchable, so its local
    /// contents can be discarded and downloaded again later.
    pub(crate) fn unmaterialization_eligibility(
        &self,
        entry: &ActionDirectoryEntry<ActionSharedDirectory>,
        deadline: Timestamp,
    ) -> UnmaterializationEligibility {
        match self.materialization_method().as_ref() {
            ArtifactMaterializationMethod::CasDownload { .. } => {
                let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
                while let Some((_, entry)) = walk.next() {
                    if let DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) = entry
                        && file.digest.expires().unwrap_or_default() < deadline
                    {
                        return UnmaterializationEligibility::Ineligible(
                            UnmaterializationIneligibilityReason::RemoteTtlTooShort,
                        );
                    }
                }
                UnmaterializationEligibility::Eligible
            }
            _ => UnmaterializationEligibility::Ineligible(
                UnmaterializationIneligibilityReason::NoRematerializationMethod,
            ),
        }
    }
}

impl Allocative for ArtifactMaterializationMethod {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        match self {
            Self::LocalCopy(srcs, copied) => {
                let mut visitor = visitor.enter(ident_key!(LocalCopy), 0);
                visitor.visit_field(ident_key!(srcs), &srcs.allocative_dfs());
                visitor.visit_field(ident_key!(copied), copied);
                visitor.exit();
            }
            Self::Write(write_file) => {
                visitor.visit_field(ident_key!(Write), write_file);
            }
            Self::CasDownload { info } => {
                visitor.visit_field(ident_key!(CasDownload), info);
            }
            #[cfg(test)]
            Self::Test => {
                visitor.visit_simple(allocative::Key::new("Test"), 0);
            }
        }
        visitor.exit();
    }
}

pub(crate) trait MaterializationMethodToProto {
    fn to_proto(&self) -> buck2_data::MaterializationMethod;
}

impl MaterializationMethodToProto for ArtifactMaterializationMethod {
    fn to_proto(&self) -> buck2_data::MaterializationMethod {
        match self {
            ArtifactMaterializationMethod::LocalCopy { .. } => {
                buck2_data::MaterializationMethod::LocalCopy
            }
            ArtifactMaterializationMethod::CasDownload { .. } => {
                buck2_data::MaterializationMethod::CasDownload
            }
            ArtifactMaterializationMethod::Write { .. } => buck2_data::MaterializationMethod::Write,
            #[cfg(test)]
            ArtifactMaterializationMethod::Test => unimplemented!(),
        }
    }
}

impl ArtifactTree {
    pub(crate) fn initialize(sqlite_state: Option<MaterializerState>) -> Self {
        let mut tree = ArtifactTree::new();
        if let Some(sqlite_state) = sqlite_state {
            for entry in sqlite_state.into_iter() {
                let MaterializerStateEntry {
                    path,
                    metadata,
                    last_access_time,
                    classification,
                } = entry;
                let logical_size_bytes = artifact_metadata_size(&metadata);
                tree.insert(
                    path.iter().map(|f| f.to_owned()),
                    Box::new(ArtifactMaterializationData {
                        deps: None,
                        classification,
                        logical_size_bytes,
                        stage: ArtifactMaterializationStage::Materialized {
                            metadata,
                            last_access_time,
                            rematerialization_method: None,
                            active: false,
                        },
                        processing: Processing::Done(Version(0)),
                    }),
                );
            }
        }
        tree
    }

    /// Given a path that's (possibly) not yet materialized, returns the path
    /// `contents_path` where its contents can be found. Returns Err if the
    /// contents cannot be found (ex. if it requires a CAS download)
    ///
    /// Note that the returned `contents_path` could be the same as `path`.
    #[instrument(level = "trace", skip(self), fields(path = %path))]
    pub(crate) fn file_contents_path(
        &self,
        path: ProjectRelativePathBuf,
        digest_config: DigestConfig,
    ) -> Result<ProjectRelativePathBuf, ArtifactNotMaterializedReason> {
        let mut path_iter = path.iter();
        let materialization_data = match self.prefix_get(&mut path_iter) {
            // Not in tree. Assume it's a source file that doesn't require materialization from materializer.
            None => return Ok(path),
            Some(data) => data,
        };
        let (entry, method) = match &materialization_data.stage {
            ArtifactMaterializationStage::Materialized { .. } => {
                return Ok(path);
            }
            ArtifactMaterializationStage::Declared { entry, method } => {
                (entry.dupe(), method.dupe())
            }
        };
        match method.as_ref() {
            ArtifactMaterializationMethod::CasDownload { info } => {
                let path_iter = path_iter.peekable();

                let root_entry: ActionDirectoryEntry<ActionSharedDirectory> = entry.dupe();
                let mut entry = Some(entry.as_ref());

                // Check if the path we are asking for exists in this entry.
                for name in path_iter {
                    entry = match entry {
                        Some(DirectoryEntry::Dir(d)) => d.get(name),
                        _ => break,
                    }
                }

                match entry {
                    Some(entry) => Err(ArtifactNotMaterializedReason::RequiresCasDownload {
                        path,
                        // TODO (@torozco): A nicer API to get an Immutable directory here.
                        entry: entry
                            .map_dir(|d| {
                                d.as_dyn()
                                    .to_builder()
                                    .fingerprint(digest_config.as_directory_serializer())
                            })
                            .map_leaf(|l| l.dupe()),
                        info: info.dupe(),
                    }),
                    None => Err(
                        ArtifactNotMaterializedReason::DeferredMaterializerCorruption {
                            path,
                            entry: root_entry,
                            info: info.dupe(),
                        },
                    ),
                }
            }
            ArtifactMaterializationMethod::Write { .. } => {
                // TODO: Do the write directly to RE instead of materializing locally?
                Err(ArtifactNotMaterializedReason::RequiresMaterialization { path })
            }
            // TODO: also record and check materialized_files for LocalCopy
            ArtifactMaterializationMethod::LocalCopy(srcs, _) => {
                match srcs.prefix_get(&mut path_iter) {
                    None => Ok(path),
                    Some(src_path) => match path_iter.next() {
                        None => self.file_contents_path(src_path.clone(), digest_config),
                        // This is not supposed to be reachable, and if it's, there
                        // is a bug somewhere else. Panic to prevent the bug from
                        // propagating.
                        Some(part) => panic!(
                            "While getting materialized path of {path:?}: path {src_path:?} is a file, so subpath {part:?} doesn't exist within.",
                        ),
                    },
                }
            }
            #[cfg(test)]
            ArtifactMaterializationMethod::Test => unimplemented!(),
        }
    }

    #[instrument(level = "debug", skip(self, result), fields(path = %artifact_path))]
    pub(crate) fn cleanup_finished(
        &mut self,
        artifact_path: ProjectRelativePathBuf,
        version: Version,
        result: Result<(), SharedMaterializingError>,
    ) {
        let Some(info) = self.prefix_get_mut(&mut artifact_path.iter()) else {
            tracing::debug!("cleanup_finished after path invalidation");
            return;
        };
        if info.processing.current_version() > version {
            // We can only unset the future if version matches.
            // Otherwise, we may be unsetting a different future from a newer version.
            tracing::debug!("version conflict");
            return;
        }

        if result.is_ok() {
            info.processing = Processing::Done(version);
        }
    }

    /// Removes paths from tree and returns a pair of two vecs.
    /// First vec is a list of paths removed. Second vec is a list of
    /// pairs of removed paths to futures that haven't finished.
    pub(crate) fn invalidate_paths_and_collect_futures(
        &mut self,
        paths: Vec<ProjectRelativePathBuf>,
        sqlite_db: Option<&mut MaterializerStateSqliteDb>,
        stats: &DeferredMaterializerStats,
    ) -> buck2_error::Result<Vec<(ProjectRelativePathBuf, ProcessingFuture)>> {
        let mut invalidated_paths = Vec::new();
        let mut futs = Vec::new();

        for path in paths {
            for (path, data) in self.remove_path(&path) {
                if matches!(
                    data.stage,
                    ArtifactMaterializationStage::Materialized { .. }
                ) {
                    stats.remove_materialized(data.classification, data.logical_size_bytes);
                }
                if let Some(processing_fut) = data.processing.into_future() {
                    futs.push((path.clone(), processing_fut));
                }
                invalidated_paths.push(path);
            }
        }

        #[cfg(test)]
        {
            use buck2_error::buck2_error;
            for path in &invalidated_paths {
                if path.as_str() == "test/invalidate/failure" {
                    return Err(buck2_error!(buck2_error::ErrorTag::Tier0, "Injected error"));
                }
            }
        }

        // We can invalidate the paths here even if materializations are currently running on
        // the underlying nodes, because when materialization finishes we'll check the version
        // number.
        if let Some(sqlite_db) = sqlite_db {
            sqlite_db
                .materializer_state_table()
                .delete(invalidated_paths)
                .buck_error_context("Error invalidating paths in materializer state")?;
        }

        Ok(futs)
    }

    pub(crate) fn unmaterialize_artifacts(
        &mut self,
        paths: Vec<(ProjectRelativePathBuf, u64)>,
        deadline: Timestamp,
        upload_max_bytes: Option<u64>,
        sqlite_db: &mut MaterializerStateSqliteDb,
        stats: &DeferredMaterializerStats,
    ) -> buck2_error::Result<UnmaterializeArtifactsResult> {
        let mut result = UnmaterializeArtifactsResult::default();
        let mut eligible = Vec::new();

        for (path, size) in paths {
            let mut path_iter = path.iter();
            let Some(data) = self.prefix_get(&mut path_iter) else {
                result.record_ineligible(UnmaterializationIneligibilityReason::StateChanged, size);
                continue;
            };
            if path_iter.next().is_some() {
                result.record_ineligible(UnmaterializationIneligibilityReason::StateChanged, size);
                continue;
            }
            if data.classification == ArtifactClassification::FinalOutput {
                result.record_ineligible(UnmaterializationIneligibilityReason::FinalOutput, size);
                continue;
            }
            if data.processing.active_ref().is_some() {
                result.record_ineligible(UnmaterializationIneligibilityReason::Processing, size);
                continue;
            }

            match &data.stage {
                ArtifactMaterializationStage::Materialized {
                    metadata,
                    rematerialization_method: Some(method),
                    ..
                } => match method.unmaterialization_eligibility(metadata, deadline) {
                    UnmaterializationEligibility::Eligible => {
                        eligible.push((
                            path,
                            size,
                            ArtifactMaterializationStage::Declared {
                                entry: metadata.dupe(),
                                method: method.materialization_method().dupe(),
                            },
                        ));
                    }
                    UnmaterializationEligibility::EligibleAfterUpload => unreachable!(
                        "artifacts with a rematerialization method cannot require upload"
                    ),
                    UnmaterializationEligibility::Ineligible(reason) => {
                        result.record_ineligible(reason, size);
                    }
                },
                ArtifactMaterializationStage::Materialized {
                    metadata,
                    rematerialization_method: None,
                    ..
                } => match upload_max_bytes {
                    Some(max_bytes) if size <= max_bytes => {
                        result.uploads.push(UnmaterializationUpload {
                            path,
                            size,
                            entry: metadata.dupe(),
                            version: data.processing.current_version(),
                        });
                    }
                    Some(_) => result.record_ineligible(
                        UnmaterializationIneligibilityReason::UploadTooLarge,
                        size,
                    ),
                    None => result.record_ineligible(
                        UnmaterializationIneligibilityReason::NoRematerializationMethod,
                        size,
                    ),
                },
                _ => result
                    .record_ineligible(UnmaterializationIneligibilityReason::StateChanged, size),
            }
        }

        #[cfg(test)]
        {
            use buck2_error::buck2_error;
            for (path, _, _) in &eligible {
                if path.as_str() == "test/unmaterialize/failure" {
                    return Err(buck2_error!(buck2_error::ErrorTag::Tier0, "Injected error"));
                }
            }
        }

        // Delete before demoting the tree entries. On failure the caller aborts without cleaning
        // these paths or attaching cleaning futures, so the tree must still agree with what is on
        // disk; only sqlite is left stale, in the direction that forces a rematerialization.
        sqlite_db
            .materializer_state_table()
            .delete(eligible.iter().map(|(path, _, _)| path.clone()).collect())
            .buck_error_context("Error unmaterializing paths in materializer state")?;

        result.unmaterialized = eligible
            .into_iter()
            .map(|(path, size, replacement)| {
                let data = self.prefix_get_mut(&mut path.iter()).expect(
                    "should be present, nothing can mutate the tree between the two passes",
                );
                data.stage = replacement;
                stats.remove_materialized(data.classification, data.logical_size_bytes);
                (path, size)
            })
            .collect();

        Ok(result)
    }

    pub(crate) fn finish_unmaterialization_upload(
        &mut self,
        upload: UnmaterializationUpload,
        info: Arc<CasDownloadInfo>,
        cleaning_fut: CleaningFuture,
        sqlite_db: &mut MaterializerStateSqliteDb,
        stats: &DeferredMaterializerStats,
    ) -> buck2_error::Result<Option<UnmaterializationIneligibleArtifact>> {
        let mut path_iter = upload.path.iter();
        let Some(data) = self.prefix_get_mut(&mut path_iter) else {
            return Ok(Some(UnmaterializationIneligibleArtifact {
                reason: UnmaterializationIneligibilityReason::StateChanged,
                size: upload.size,
            }));
        };
        if path_iter.next().is_some()
            || data.classification == ArtifactClassification::FinalOutput
            || data.processing.active_ref().is_some()
            || data.processing.current_version() != upload.version
            || !matches!(
                &data.stage,
                ArtifactMaterializationStage::Materialized { metadata, .. }
                    if artifact_metadata_matches_entry(metadata, &upload.entry)
            )
        {
            return Ok(Some(UnmaterializationIneligibleArtifact {
                reason: UnmaterializationIneligibilityReason::StateChanged,
                size: upload.size,
            }));
        }

        sqlite_db
            .materializer_state_table()
            .delete(vec![upload.path])
            .buck_error_context("Error unmaterializing uploaded path in materializer state")?;
        data.stage = ArtifactMaterializationStage::Declared {
            entry: upload.entry,
            method: Arc::new(ArtifactMaterializationMethod::CasDownload { info }),
        };
        data.processing =
            Processing::active(ProcessingFuture::Cleaning(cleaning_fut), upload.version);
        stats.remove_materialized(data.classification, data.logical_size_bytes);
        Ok(None)
    }

    pub(crate) fn attach_unmaterialization_future(
        &mut self,
        path: &ProjectRelativePath,
        cleaning_fut: CleaningFuture,
        version: Version,
    ) -> buck2_error::Result<()> {
        let mut path_iter = path.iter();
        let Some(data) = self.prefix_get_mut(&mut path_iter) else {
            return Err(internal_error!(
                "Unmaterialized artifact `{path}` disappeared before its cleaning future was attached"
            ));
        };
        if path_iter.next().is_some()
            || !matches!(data.stage, ArtifactMaterializationStage::Declared { .. })
        {
            return Err(internal_error!(
                "Unmaterialized artifact `{path}` was not declared when its cleaning future was attached"
            ));
        }
        data.processing = Processing::active(ProcessingFuture::Cleaning(cleaning_fut), version);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use super::ActiveProcessing;
    use super::Processing;

    #[test]
    fn processing_done_layout_does_not_include_active_state() {
        assert!(mem::size_of::<Processing>() < mem::size_of::<ActiveProcessing>());
    }
}
