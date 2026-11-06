/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_action_metadata_proto::DepFileInputs;
use buck2_action_metadata_proto::RemoteDepFile;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::BaseArtifactKind;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::actions::impls::expanded_command_line::ExpandedCommandLineDigest;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_build_signals::env::WaitingData;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::cas_digest::CasDigestData;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::buck2_env;
use buck2_core::configuration::pair::Configuration;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::soft_error;
use buck2_core::soft_error_or_panic;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_hasher::DirectoryDigester;
use buck2_directory::directory::directory_ref::DirectoryRef;
use buck2_directory::directory::directory_selector::DirectorySelector;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::find::find;
use buck2_directory::directory::fingerprinted_directory::FingerprintedDirectory;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_events::dispatch::span_async;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::dep_file_state::DepFileStore;
use buck2_execute::dep_file_state::StoredDepFileDigests;
use buck2_execute::dep_file_state::StoredDepFileIdentity;
use buck2_execute::dep_file_state::StoredDepFileState;
use buck2_execute::dep_file_state::StoredOutput;
use buck2_execute::dep_file_state::StoredOutputValue;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionFingerprintedDirectoryRef;
use buck2_execute::directory::ActionImmutableDirectory;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::directory::INTERNER;
use buck2_execute::directory::LazyActionDirectoryBuilder;
use buck2_execute::directory::ReDirectorySerializer;
use buck2_execute::directory::expand_selector_for_dependencies;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobsBuilder;
use buck2_execute::execute::cache_uploader::IntoRemoteDepFile;
use buck2_execute::execute::dep_file_digest::DepFileDigest;
use buck2_execute::execute::request::CommandExecutionPaths;
use buck2_execute::execute::request::ExecutorPreference;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::materialize::materializer::MaterializationError;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::materialize::materializer::ReadLease;
use buck2_file_watcher::dep_files::CREATE_DEP_FILE_CACHE;
use buck2_file_watcher::dep_files::DepFileCache;
use buck2_fs::fs_util;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathNormalizer;
use buck2_hash::BuckDashMap;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_util::strong_hasher::Blake3StrongHasher;
use dupe::Dupe;
use either::Either;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use parking_lot::MappedMutexGuard;
use parking_lot::Mutex;
use parking_lot::MutexGuard;
use strong_hash::StrongHash;
use tracing::instrument;

use crate::actions::impls::run::LogicalActionKey;
use crate::actions::impls::run::RunActionKey;

/// Stable, forward-only key: the raw 32-byte blake3 `StrongHash` of `value` (stored as a `BLOB`).
/// `StrongHash` hashes full content, so the digest is stable across daemon restarts and collision-resistant.
/// Never decoded.
fn strong_hash_bytes<T: StrongHash>(value: &T) -> Vec<u8> {
    let mut hasher = Blake3StrongHasher::new();
    value.strong_hash(&mut hasher);
    hasher.finalize().as_bytes().to_vec()
}

/// Stable, forward-only key for a logical action, used as the persisted database key.
fn encode_logical_key(logical: &LogicalActionKey) -> Vec<u8> {
    strong_hash_bytes(logical)
}

/// Stable, forward-only key for a configuration, used as the persisted database `config_key` (which
/// distinguishes per-configuration rows of one logical action). `None` and `Some` hash distinctly.
fn encode_config_key(cfg: Option<Configuration>) -> Vec<u8> {
    strong_hash_bytes(&cfg)
}

/// Groups the per-configuration dep-file states of one logical action, keyed by the action's
/// configuration. The key is `None` for anon-target and BXL actions, which have no
/// configuration (and always occupy their own single-entry slot).
/// Most logical actions are only ever
/// seen under a single configuration, so that common case is stored inline (`One`) to avoid
/// allocating a map for a single entry; a second configuration promotes the slot to `Many`.
#[derive(Allocative)]
enum ConfigActionSlot {
    One(Option<Configuration>, Arc<DepFileState>),
    Many(BuckMutInsertionMap<Option<Configuration>, Arc<DepFileState>>),
}

impl ConfigActionSlot {
    fn get(&self, cfg: Option<Configuration>) -> Option<&Arc<DepFileState>> {
        match self {
            ConfigActionSlot::One(c, s) => (*c == cfg).then_some(s),
            ConfigActionSlot::Many(m) => m.get(&cfg),
        }
    }

    /// The dep-file state of every configuration in this slot.
    fn states(&self) -> impl Iterator<Item = &Arc<DepFileState>> {
        match self {
            ConfigActionSlot::One(_, s) => Either::Left(std::iter::once(s)),
            ConfigActionSlot::Many(m) => Either::Right(m.values()),
        }
    }

    /// Insert (or replace) `cfg`'s entry, promoting `One` -> `Many` when a second configuration
    /// appears.
    fn insert(&mut self, cfg: Option<Configuration>, state: Arc<DepFileState>) {
        match self {
            ConfigActionSlot::Many(m) => {
                m.insert(cfg, state);
            }
            ConfigActionSlot::One(c, s) if *c == cfg => {
                *s = state;
            }
            _ => {
                let ConfigActionSlot::One(c, s) =
                    std::mem::replace(self, ConfigActionSlot::Many(BuckMutInsertionMap::new()))
                else {
                    unreachable!("the arms above cover Many and the matching One");
                };
                let ConfigActionSlot::Many(m) = self else {
                    unreachable!("just replaced with Many");
                };
                m.insert(c, s);
                m.insert(cfg, state);
            }
        }
    }

    /// Remove `cfg`'s entry; returns true if the slot is now empty and should be dropped.
    fn remove(&mut self, cfg: Option<Configuration>) -> bool {
        match self {
            ConfigActionSlot::One(c, _) => *c == cfg,
            ConfigActionSlot::Many(m) => {
                m.shift_remove(&cfg);
                m.is_empty()
            }
        }
    }

    /// Keep only entries whose state satisfies `f`; returns true if the slot is still non-empty.
    fn retain_non_empty(&mut self, f: impl Fn(&Arc<DepFileState>) -> bool) -> bool {
        match self {
            ConfigActionSlot::One(_, s) => f(s),
            ConfigActionSlot::Many(m) => {
                m.retain(|_, s| f(s));
                !m.is_empty()
            }
        }
    }
}

/// Backing store for one repo's dep-file cache: a concurrent map from `(logical action,
/// configuration) -> dep-file state`. A logical action maps to a per-configuration slot, stored
/// inline for the common single-configuration case and promoting to a map on a second
/// configuration; DashMap's sharded locking synchronizes access. `cfg` is `None` for anon-target
/// and BXL actions.
#[derive(Default, Allocative)]
struct ShardedDepFiles {
    map: BuckDashMap<LogicalActionKey, ConfigActionSlot>,
}

impl ShardedDepFiles {
    /// State cached for exactly this `(logical, cfg)`, if any.
    fn get(
        &self,
        logical: &LogicalActionKey,
        cfg: Option<Configuration>,
    ) -> Option<Arc<DepFileState>> {
        self.map.get(logical)?.get(cfg).map(Dupe::dupe)
    }

    /// Insert or replace the state for `(logical, cfg)`.
    fn insert(
        &self,
        logical: LogicalActionKey,
        cfg: Option<Configuration>,
        state: Arc<DepFileState>,
    ) {
        // `entry(..).or_insert_with(..)` holds the shard guard across the insert, so promoting a slot
        // to `Many` when a second configuration populates it concurrently is atomic (no lost entry).
        self.map
            .entry(logical)
            .or_insert_with({
                let cfg = cfg.dupe();
                let state = state.dupe();
                move || ConfigActionSlot::One(cfg, state)
            })
            .insert(cfg, state);
    }

    /// Remove the entry for `(logical, cfg)`, if present.
    fn remove(&self, logical: &LogicalActionKey, cfg: Option<Configuration>) {
        // Remove the entry and, if that empties the slot, drop the slot -- both under one guard.
        self.map.remove_if_mut(logical, |_, slot| slot.remove(cfg));
    }

    /// Every cached state for `logical`, across all configurations built this session -- the
    /// cross-configuration candidates. Returned owned so the caller can drop the shard guard before
    /// awaiting. Entries from previous daemon sessions are not held here; they are loaded on demand
    /// from the persisted store (see `match_if_identical_action`).
    fn states_for_logical(&self, logical: &LogicalActionKey) -> Vec<Arc<DepFileState>> {
        match self.map.get(logical) {
            Some(slot) => slot.states().map(Dupe::dupe).collect(),
            None => Vec::new(),
        }
    }

    /// Drop every entry whose state does not satisfy `keep`.
    fn retain(&self, keep: impl Fn(&DepFileState) -> bool) {
        self.map
            .retain(|_, slot| slot.retain_non_empty(|state| keep(state)));
    }

    /// Drop all entries.
    fn clear(&self) {
        self.map.clear();
    }

    /// Number of cached logical actions (approximate; used only for logging).
    fn len(&self) -> usize {
        self.map.len()
    }
}

impl DepFileCache for ShardedDepFiles {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clear(&self) {
        tracing::info!("Flushing all {} dep files", self.len());
        ShardedDepFiles::clear(self);
    }

    fn clear_non_local(&self) {
        tracing::info!(
            "Flushing non-local dep files, current size is: {}",
            self.len()
        );
        self.retain(|state| state.was_produced_locally);
        tracing::info!(
            "Number of remaining local dep file slots is: {}",
            self.len()
        );
    }
}

fn dep_files(cache: &dyn DepFileCache) -> &ShardedDepFiles {
    cache
        .as_any()
        .downcast_ref()
        .expect("DepFileCache should have been created by buck2_action_impl")
}

/// When this is set, we retain directories after fingerprinting, so that we can output them later
/// for debugging via `buck2 audit dep-files`.
fn keep_directories() -> buck2_error::Result<bool> {
    buck2_env!("BUCK2_KEEP_DEP_FILE_DIRECTORIES", bool)
}

fn new_dep_file_cache() -> Arc<dyn DepFileCache> {
    Arc::new(ShardedDepFiles::default())
}

pub(crate) fn init_dep_file_cache() {
    CREATE_DEP_FILE_CACHE.init(new_dep_file_cache);
}

pub(crate) fn get_dep_files(
    cache: &dyn DepFileCache,
    key: &RunActionKey,
) -> Option<Arc<DepFileState>> {
    dep_files(cache).get(&key.to_logical(), key.configuration())
}

/// Remove a single configuration's entry from the cache (both in-memory and persisted).
fn remove_dep_file_entry(
    cache: &dyn DepFileCache,
    key: &RunActionKey,
    store: Option<&dyn DepFileStore>,
) {
    let logical = key.to_logical();
    dep_files(cache).remove(&logical, key.configuration());
    if let Some(store) = store {
        store.delete(
            encode_logical_key(&logical),
            encode_config_key(key.configuration()),
        );
    }
}

/// The input signatures for a DepFileState. We compute those lazily, so we either have the input
/// directories (no computation done), or the actual signatures (computation was done).
#[derive(Allocative)]
enum DepFileStateInputSignatures {
    /// Deferred means we have the input directories but haven't filtered them using the dep
    /// file yet (which in fact we haven't downloaded yet). Producing the signatures can fail (the
    /// dep file may be invalid), in which case the directories stay here so that the next lookup
    /// reports the same error.
    Deferred(PartitionedInputs<ActionSharedDirectory>),

    /// Computed represents the case where we have produced the input signatures. We only do this
    /// once at most.
    Computed(StoredFingerprints),
}

#[derive(Allocative)]
pub(crate) enum StoredFingerprints {
    /// Store only digests. This is what we use in prod because it is small.
    Digests(PartitionedInputs<TrackedFileDigest>),

    /// Store digests + dirs. We allow this via BUCK2_KEEP_DEP_FILE_DIRECTORIES because it gives
    /// more debuggability.
    Dirs(PartitionedInputs<ActionImmutableDirectory>),
}

impl PartialEq<PartitionedInputs<ActionImmutableDirectory>> for StoredFingerprints {
    fn eq(&self, other: &PartitionedInputs<ActionImmutableDirectory>) -> bool {
        let fingerprints = match self {
            Self::Digests(fingerprints) => Cow::Borrowed(fingerprints),
            Self::Dirs(dirs) => Cow::Owned(dirs.as_fingerprints()),
        };

        *fingerprints == other.as_fingerprints()
    }
}

/// The dep files a cache entry's action declared. This is where the "what survives persistence"
/// invariant lives: a live entry (produced this session) keeps the full configuration-dependent
/// objects plus the input signatures the dep-file-filtered path needs, while a reloaded entry keeps
/// only the configuration-independent identities that survive a round-trip through disk -- which is
/// all the identical-action path ever compares. Making this a single enum keeps illegal combinations
/// (e.g. a reloaded entry claiming to have signatures) unrepresentable.
#[derive(Allocative)]
enum DepFileDeclaration {
    /// The action declared no dep files (live or reloaded -- indistinguishable, and both only ever
    /// serve the identical-action path).
    None,
    /// Produced this session. The configuration-independent identities are derived from
    /// `declared_dep_files` on demand rather than stored redundantly alongside it.
    Live {
        declared_dep_files: DeclaredDepFiles,
        input_signatures: Mutex<DepFileStateInputSignatures>,
    },
    /// Reloaded from disk: only the configuration-independent identities survived.
    Reloaded {
        identities: MiniBoxSlice<DeclaredDepFileIdentity>,
    },
}

impl DepFileDeclaration {
    /// Whether `action_dep_files` (the live action being looked up) declares the same dep files as
    /// this cached entry, compared by configuration-independent identity (set equality). The live
    /// action's identity set is compared against the cached entry's -- derived on the fly for a live
    /// entry, or rebuilt from the persisted identities for a reloaded one.
    fn matches_action(&self, action_dep_files: &DeclaredDepFiles) -> buck2_error::Result<bool> {
        Ok(match self {
            DepFileDeclaration::None => action_dep_files.is_empty(),
            DepFileDeclaration::Live {
                declared_dep_files, ..
            } => action_dep_files.identities()? == declared_dep_files.identities()?,
            DepFileDeclaration::Reloaded { identities } => {
                action_dep_files.identities()? == identities.iter().cloned().collect()
            }
        })
    }

    /// The declared dep files in their persisted, configuration-independent form (empty if none).
    fn stored_identities(&self) -> buck2_error::Result<Vec<StoredDepFileIdentity>> {
        Ok(match self {
            DepFileDeclaration::None => Vec::new(),
            DepFileDeclaration::Live {
                declared_dep_files, ..
            } => declared_dep_files
                .identities()?
                .into_iter()
                .map(|i| i.to_stored())
                .collect(),
            DepFileDeclaration::Reloaded { identities } => identities
                .iter()
                .map(DeclaredDepFileIdentity::to_stored)
                .collect(),
        })
    }
}

/// An entry reloaded from the persisted store, before its outputs have been resolved against the
/// materializer. `promote_reloaded_entry` converts it into a `DepFileState`.
struct LoadedEntry {
    digests: CommandDigests,
    /// Outputs in configuration-independent form, keyed by target-relative path. Leaf values are
    /// held in full; directory values hold only a fingerprint and are rehydrated from the
    /// materializer at reuse time (see `resolve_loaded_outputs`). The real `BuildArtifactPath` is
    /// reconstructed from the live action's declared outputs on reuse.
    outputs: MiniBoxSlice<StoredOutput>,
    was_produced_locally: bool,
    declared: DepFileDeclaration,
}

/// A cache entry being tested against the live action: either one built during this session (under
/// this or another configuration) or one reloaded from disk.
#[derive(Copy, Clone, Dupe)]
enum DepFileCandidate<'a> {
    Live(&'a DepFileState),
    Loaded(&'a LoadedEntry),
}

impl<'a> DepFileCandidate<'a> {
    fn digests(self) -> &'a CommandDigests {
        match self {
            DepFileCandidate::Live(s) => &s.digests,
            DepFileCandidate::Loaded(l) => &l.digests,
        }
    }

    fn declared(self) -> &'a DepFileDeclaration {
        match self {
            DepFileCandidate::Live(s) => &s.declared,
            DepFileCandidate::Loaded(l) => &l.declared,
        }
    }

    /// The target-relative path of every output: the configuration-independent view used by
    /// `outputs_are_reusable` to check the output set matches.
    fn short_paths(self) -> impl Iterator<Item = &'a ForwardRelativePath> {
        match self {
            DepFileCandidate::Live(s) => Either::Left(s.result.iter().map(|(p, _)| p.path())),
            DepFileCandidate::Loaded(l) => Either::Right(l.outputs.iter().map(|o| o.path.as_ref())),
        }
    }
}

/// Configuration-independent identity of a declared dep file, mirroring the tuple compared by
/// `config_independent_key`. Recorded for every entry (live or reloaded) so `check_action` can
/// compare a live action's declared dep files against a cached entry's without needing the cached
/// entry's (configuration-dependent) artifacts.
#[derive(Hash, PartialEq, Eq, Clone, Allocative)]
struct DeclaredDepFileIdentity {
    label: Arc<str>,
    path: ForwardRelativePathBuf,
    projected: ForwardRelativePathBuf,
    is_content_based: bool,
}

impl DeclaredDepFileIdentity {
    fn to_stored(&self) -> StoredDepFileIdentity {
        StoredDepFileIdentity {
            label: self.label.to_string(),
            path: self.path.to_string(),
            projected: self.projected.to_string(),
            is_content_based: self.is_content_based,
        }
    }
}

/// The state that resulted from the previous evaluation of a command that produced dep files. This
/// contains everything we need to determine whether re-evaluation is necessary (and if it isn't,
/// to return the previous value).
#[derive(Allocative)]
pub(crate) struct DepFileState {
    digests: CommandDigests,
    result: ActionOutputs,
    was_produced_locally: bool,
    /// The dep files this action declared. A live entry carries the full objects and the
    /// dep-file-filtered machinery; a reloaded entry carries only configuration-independent
    /// identities. See `DepFileDeclaration`.
    declared: DepFileDeclaration,
}

#[derive(Allocative)]
pub(crate) struct CommandDigests {
    pub(crate) cli: ExpandedCommandLineDigest,
    pub(crate) directory: FileDigest,
    pub(crate) local_worker_directory: Option<TrackedFileDigest>,
}

fn cli_digest_from_bytes(bytes: &[u8]) -> buck2_error::Result<ExpandedCommandLineDigest> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| internal_error!("Persisted cli digest has unexpected length"))?;
    Ok(ExpandedCommandLineDigest::from_bytes(bytes))
}

/// The digests of a persisted candidate, in the form `check_action` compares against.
fn stored_command_digests(digests: &StoredDepFileDigests) -> buck2_error::Result<CommandDigests> {
    Ok(CommandDigests {
        cli: cli_digest_from_bytes(&digests.cli_digest)?,
        directory: digests.directory_digest.dupe(),
        local_worker_directory: digests.local_worker_directory_digest.dupe(),
    })
}

/// The individual digest comparisons `check_action` makes, so that the cheap pre-check the persisted
/// store performs (`matches_all`) cannot drift from the authoritative check: both are built from
/// these, and changing one changes both.
impl CommandDigests {
    fn cli_matches(&self, cli: &ExpandedCommandLineDigest) -> bool {
        self.cli == *cli
    }

    fn local_worker_matches(&self, local_worker: &Option<TrackedFileDigest>) -> bool {
        self.local_worker_directory == *local_worker
    }

    fn directory_matches(&self, directory: &FileDigest) -> bool {
        self.directory == *directory
    }

    /// Whether all three digests are those of this action -- the digest half of `check_action`'s
    /// `Hit` verdict.
    fn matches_all(
        &self,
        cli: &ExpandedCommandLineDigest,
        local_worker: &Option<TrackedFileDigest>,
        directory: &FileDigest,
    ) -> bool {
        self.cli_matches(cli)
            && self.local_worker_matches(local_worker)
            && self.directory_matches(directory)
    }
}

impl DepFileState {
    pub(crate) fn has_signatures(&self) -> bool {
        match &self.declared {
            DepFileDeclaration::Live {
                input_signatures, ..
            } => matches!(
                *input_signatures.lock(),
                DepFileStateInputSignatures::Computed(..)
            ),
            _ => false,
        }
    }

    pub(crate) fn declared_dep_files(&self) -> Option<&DeclaredDepFiles> {
        match &self.declared {
            DepFileDeclaration::Live {
                declared_dep_files, ..
            } => Some(declared_dep_files),
            _ => None,
        }
    }

    pub(crate) fn result(&self) -> &ActionOutputs {
        &self.result
    }
}

impl LoadedEntry {
    /// Reconstruct a reloaded entry from its persisted, configuration-independent form.
    fn from_stored(stored: StoredDepFileState) -> buck2_error::Result<Self> {
        let digests = CommandDigests {
            cli: cli_digest_from_bytes(&stored.cli_digest)?,
            directory: stored.directory_digest,
            local_worker_directory: stored.local_worker_directory_digest,
        };
        // Validate paths loaded from disk rather than trusting them: a corrupt or
        // differently-versioned row surfaces as a `from_stored` error, which the lookup path drops
        // and reports, instead of constructing an invalid path that misbehaves later.
        let identities: MiniBoxSlice<DeclaredDepFileIdentity> = stored
            .declared
            .into_iter()
            .map(|i| {
                buck2_error::Ok(DeclaredDepFileIdentity {
                    label: Arc::from(i.label.as_str()),
                    path: ForwardRelativePathBuf::new(i.path)?,
                    projected: ForwardRelativePathBuf::new(i.projected)?,
                    is_content_based: i.is_content_based,
                })
            })
            .collect::<buck2_error::Result<_>>()?;
        // A reloaded entry carries only identities; with none it is simply `None` (indistinguishable
        // from a live no-dep-file entry, which is fine -- both serve only the identical-action path).
        let declared = if identities.is_empty() {
            DepFileDeclaration::None
        } else {
            DepFileDeclaration::Reloaded { identities }
        };
        Ok(LoadedEntry {
            digests,
            outputs: stored.outputs.into(),
            was_produced_locally: stored.was_produced_locally,
            declared,
        })
    }
}

impl DepFileState {
    /// Convert to the persisted, configuration-independent form, or `None` if the entry must not be
    /// persisted (see the `deps` check below). Leaf outputs are stored in full; directory outputs
    /// store only their fingerprint (the tree is rehydrated from the materializer on reuse).
    fn to_stored(&self) -> buck2_error::Result<Option<StoredDepFileState>> {
        let mut outputs = Vec::new();
        for (path, value) in self.result.iter() {
            // `ArtifactValue::deps` records the artifacts an output's symlinks point at, and there
            // is no column for it here, so a reloaded output would always come back with
            // `deps: None`. `Materializer::declare_match` cannot repair that -- it copies deps *from*
            // the value it is handed -- and the materializer's own state does not persist deps
            // either, so nothing would ever materialize those symlink destinations and the served
            // output could be a dangling symlink. Refuse to persist such an entry: a miss just
            // re-executes the action.
            if value.deps().is_some() {
                return Ok(None);
            }
            let stored_value = match value.entry() {
                DirectoryEntry::Dir(d) => {
                    StoredOutputValue::Directory(d.fingerprint().data().dupe())
                }
                DirectoryEntry::Leaf(_) => StoredOutputValue::Leaf(value.dupe()),
            };
            outputs.push(StoredOutput {
                path: path.path().to_owned(),
                value: stored_value,
            });
        }
        Ok(Some(StoredDepFileState {
            cli_digest: self.digests.cli.as_bytes().to_vec(),
            directory_digest: self.digests.directory.dupe(),
            local_worker_directory_digest: self.digests.local_worker_directory.dupe(),
            was_produced_locally: self.was_produced_locally,
            declared: self.declared.stored_identities()?,
            outputs,
        }))
    }

    /// Compute the signature for this DepFileState, having provided the dep files from
    /// read_dep_files.
    pub(crate) fn locked_compute_fingerprints<'a>(
        &'a self,
        dep_files: Cow<'_, ConcreteDepFiles>,
        keep_directories: bool,
        digest_config: DigestConfig,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<MappedMutexGuard<'a, StoredFingerprints>> {
        let DepFileDeclaration::Live {
            input_signatures, ..
        } = &self.declared
        else {
            return Err(internal_error!(
                "No dep files exist; should not try to compute fingerprints"
            ));
        };

        // Now we need to know the signatures on the original action. Produce them if they're
        // missing. We're either storing input directories or outputs here.
        let mut guard = input_signatures.lock();

        if let DepFileStateInputSignatures::Deferred(ref directories) = *guard {
            // The directories are left in place in case the dep file turns out to be invalid.
            let fingerprints = compute_fingerprints(
                directories.unshare(),
                dep_files.into_owned(),
                digest_config,
                keep_directories,
                fs,
            )?;

            *guard = DepFileStateInputSignatures::Computed(fingerprints);
        }

        Ok(MutexGuard::map(guard, |v| match v {
            DepFileStateInputSignatures::Computed(signatures) => signatures,
            DepFileStateInputSignatures::Deferred(..) => unreachable!(),
        }))
    }
}

/// The set of dep files declared by a RunAction, matching tags to their labels. We enforce at
/// creation time that tags and labels are both unique.
#[derive(Debug, Allocative, Pagable)]
pub(crate) struct RunActionDepFiles {
    pub(crate) labels: BuckFixedInsertionMap<ArtifactTag, Arc<str>>,
}

impl Display for RunActionDepFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let values: Vec<&str> = self
            .labels
            .values()
            .map(|x| x.as_ref())
            .collect::<Vec<&str>>();
        write!(f, "{values:?}")
    }
}

fn get_output_path_digest(
    digest_config: DigestConfig,
    output_paths: &[(ProjectRelativePathBuf, OutputType)],
) -> CasDigestData {
    let mut digester = CasDigestData::digester(digest_config.cas_digest_config());
    digester.update(&output_paths.len().to_le_bytes());
    for (output_path, output_type) in output_paths.iter() {
        digester.update(output_path.as_str().as_bytes());
        digester.update(&[*output_type as u8]);
    }
    digester.finalize()
}

// A utility struct to hold digests that are included in both remote depfile key and value
pub(crate) struct CommonDigests {
    commandline_cli_digest: ExpandedCommandLineDigest,
    // A digest of all output paths for this action
    output_paths_digest: CasDigestData,
    // A digest of inputs that are untagged (not tied to a dep file)
    untagged_inputs_digest: TrackedFileDigest,
    // A digest of local worker inputs
    local_worker_inputs_digest: Option<TrackedFileDigest>,
}
impl CommonDigests {
    // Take the digest of everythig in the structure
    fn fingerprint(&self, digest_config: DigestConfig) -> CasDigestData {
        let mut digester = CasDigestData::digester(digest_config.cas_digest_config());

        digester.update(self.output_paths_digest.raw_digest().as_bytes());
        digester.update(self.commandline_cli_digest.as_bytes());
        digester.update(self.untagged_inputs_digest.raw_digest().as_bytes());
        if let Some(local_worker_inputs_digest) = &self.local_worker_inputs_digest {
            digester.update(local_worker_inputs_digest.raw_digest().as_bytes());
        }

        digester.finalize()
    }

    // Construct an action so that the action digest can be used as a remote dep file key,
    // which is the key to an action result with dep file metadata.
    // The action itself is unused but we need to upload some action to avoid permission
    // errors when uploading the action result, and the digest needs to match that action.
    pub(crate) fn make_remote_dep_file_action(
        &self,
        digest_config: DigestConfig,
        mergebase: &Option<String>,
        re_platform: &remote_execution::Platform,
    ) -> ActionDigestAndBlobs {
        let mut digester = DepFileDigest::digester(digest_config.cas_digest_config());
        digester.update(self.fingerprint(digest_config).raw_digest().as_bytes());

        // Take the digest of the mergebase to get the closest hit.
        if let Some(m) = mergebase {
            digester.update(m.as_bytes());
        }
        let inner_remote_dep_file_key = digester.finalize().to_string();

        let mut blobs = ActionDigestAndBlobsBuilder::new(digest_config);
        let command = blobs.add_command(&remote_execution::Command {
            // Instead of using the digest directly, we could use the constituent digests, or constituent paths
            // which might be useful for debugging.
            arguments: vec![inner_remote_dep_file_key],
            #[allow(deprecated)]
            platform: Some(re_platform.clone()),
            ..Default::default()
        });

        blobs.build(&remote_execution::Action {
            command_digest: Some(command.to_grpc()),
            ..Default::default()
        })
    }

    /// Take a list of declared dep files (label, artifact) and filtered inputs (StoredFingerprints)
    /// and create a list of dep file path and filtered inputs for that input
    fn get_dep_file_inputs(
        &self,
        declared_dep_files: &DeclaredDepFiles,
        filtered_input_fingerprints: &StoredFingerprints,
    ) -> Vec<DepFileInputs> {
        let filtered_input_fingerprints = match filtered_input_fingerprints {
            StoredFingerprints::Digests(digests) => Cow::Borrowed(digests),
            StoredFingerprints::Dirs(dirs) => Cow::Owned(dirs.as_fingerprints()),
        };
        declared_dep_files
            .tagged
            .iter()
            // Both declared inputs and filtered fingerprints are constructed with preserved input ordering
            .zip(filtered_input_fingerprints.tagged.iter())
            .map(|((_, dep_file), (_, filtered_fingerprint))| DepFileInputs {
                dep_file_path: dep_file.output.get_path().to_string(),
                filtered_fingerprint: filtered_fingerprint.raw_digest().as_bytes().to_vec(),
            })
            .collect()
    }

    fn make_dep_file_entry_proto(
        &self,
        declared_dep_files: &DeclaredDepFiles,
        filtered_input_fingerprints: &StoredFingerprints,
    ) -> RemoteDepFile {
        let output_paths_digest = self.output_paths_digest.raw_digest().as_bytes().to_vec();
        let commandline_cli_digest = self.commandline_cli_digest.as_bytes().to_vec();
        let untagged_inputs_digest = self.untagged_inputs_digest.raw_digest().as_bytes().to_vec();
        let dep_file_inputs =
            self.get_dep_file_inputs(declared_dep_files, filtered_input_fingerprints);

        RemoteDepFile {
            commandline_cli_digest,
            output_paths_digest,
            untagged_inputs_digest,
            dep_file_inputs,
        }
    }
}

pub(crate) struct DepFileBundle {
    dep_files_key: RunActionKey,
    input_directory_digest: FileDigest,
    shared_declared_inputs: Option<PartitionedInputs<ActionSharedDirectory>>,
    declared_dep_files: DeclaredDepFiles,
    filtered_input_fingerprints: Option<StoredFingerprints>,
    common_digests: CommonDigests,
    /// The command line digest the local action cache compares; see `local_cache_cli_digest`.
    local_cli_digest: ExpandedCommandLineDigest,
}

/// The command line digest the local action cache keys on: `cli` plus what in the executor
/// configuration the action runs under can change its result, and its executor preference.
///
/// The local cache shares hits across configurations of a target. With content-based paths, two
/// configurations can have the same command line and inputs while running under different
/// executors (another RE platform or image), or under one hybrid executor with one preferring
/// local and the other remote execution, and a tool the image or host provides then differs while
/// nothing the other digests cover does. The remote dep-file key keeps `cli` alone: the RE
/// platform is part of its action.
fn local_cache_cli_digest(
    cli: &ExpandedCommandLineDigest,
    executor_result_fingerprint: u64,
    executor_preference: ExecutorPreference,
) -> ExpandedCommandLineDigest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(cli.as_bytes());
    hasher.update(&executor_result_fingerprint.to_le_bytes());
    hasher.update(executor_preference.to_string().as_bytes());
    ExpandedCommandLineDigest::from_bytes(*hasher.finalize().as_bytes())
}

#[async_trait]
impl IntoRemoteDepFile for DepFileBundle {
    fn remote_dep_file_action(
        &self,
        digest_config: DigestConfig,
        mergebase: &Option<String>,
        re_platform: &remote_execution::Platform,
    ) -> ActionDigestAndBlobs {
        self.common_digests
            .make_remote_dep_file_action(digest_config, mergebase, re_platform)
    }

    async fn make_remote_dep_file(
        &mut self,
        digest_config: DigestConfig,
        fs: &ArtifactFs,
        materializer: &dyn Materializer,
        re_use_case: RemoteExecutorUseCase,
        result: &CommandExecutionResult,
    ) -> buck2_error::Result<Option<RemoteDepFile>> {
        let shared_declared_inputs = match &self.shared_declared_inputs {
            Some(shared_declared_inputs) => shared_declared_inputs,
            None => {
                return Ok(None);
            }
        };

        // Compute the input fingerprint digest if it hasn't been computed already.
        if self.filtered_input_fingerprints.is_none() {
            let action_outputs = ActionOutputs::new(
                result
                    .outputs
                    .iter()
                    .filter_map(|(o, v)| Some((o.build_artifact_path()?.dupe(), v.dupe())))
                    .collect(),
            );

            self.filtered_input_fingerprints = Some(
                eagerly_compute_fingerprints(
                    digest_config,
                    fs,
                    materializer,
                    re_use_case,
                    shared_declared_inputs,
                    &self.declared_dep_files,
                    &action_outputs,
                )
                .await?,
            );
        }

        Ok(Some(self.common_digests.make_dep_file_entry_proto(
            &self.declared_dep_files,
            self.filtered_input_fingerprints.as_ref().unwrap(),
        )))
    }
}

impl DepFileBundle {
    pub(crate) fn has_dep_files(&self) -> bool {
        !self.declared_dep_files.is_empty()
    }

    pub(crate) async fn check_local_dep_file_cache_for_identical_action(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        declared_outputs: &[BuildArtifact],
    ) -> buck2_error::Result<(Option<(ActionOutputs, ActionExecutionMetadata)>, bool)> {
        // Get the action outputs (if cache hit) and an indicator on whether a full lookup operation should be performed
        let (outputs, check_filtered_inputs) = span_async(
            buck2_data::MatchDepFilesStart {
                checking_filtered_inputs: false,
                remote_cache: false,
            },
            async {
                let mut stats = DepFileLookupStats::default();
                let result = match_if_identical_action(
                    ctx,
                    &self.dep_files_key,
                    &self.input_directory_digest,
                    &self.common_digests.local_worker_inputs_digest,
                    &self.local_cli_digest,
                    declared_outputs,
                    &self.declared_dep_files,
                    &mut stats,
                )
                .await;
                if result.is_err() {
                    stats.error();
                }
                (result, stats.into_end_event())
            },
        )
        .await?;
        let outputs = outputs.map(|o| {
            (
                o,
                ActionExecutionMetadata {
                    dep_file_db_writes_queued: 0,
                    execution_kind: ActionExecutionKind::LocalActionCache,
                    timing: Default::default(),
                    input_files_bytes: None,
                    waiting_data: WaitingData::new(),
                },
            )
        });
        Ok((outputs, check_filtered_inputs))
    }

    pub(crate) async fn check_local_dep_file_cache(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        declared_outputs: &[BuildArtifact],
    ) -> buck2_error::Result<Option<(ActionOutputs, ActionExecutionMetadata)>> {
        // This path never consults the persisted store: it needs input signatures, which only an
        // entry produced this session carries. So the timings stay absent and the outcome is only
        // ever live-or-miss.
        let matching_result = span_async(
            buck2_data::MatchDepFilesStart {
                checking_filtered_inputs: true,
                remote_cache: false,
            },
            async {
                let result = match_or_clear_dep_file(
                    ctx,
                    &self.dep_files_key,
                    &self.input_directory_digest,
                    &self.common_digests.local_worker_inputs_digest,
                    &self.local_cli_digest,
                    &self.shared_declared_inputs,
                    declared_outputs,
                    &self.declared_dep_files,
                )
                .await;
                let outcome = match &result {
                    Ok(Some(_)) => buck2_data::DepFileLookupOutcome::Live,
                    Ok(None) => buck2_data::DepFileLookupOutcome::Miss,
                    Err(_) => buck2_data::DepFileLookupOutcome::Error,
                };
                let end = buck2_data::MatchDepFilesEnd {
                    outcome: outcome as i32,
                    persisted_probe_us: None,
                    persisted_fetch_us: None,
                    persisted_fetches: None,
                };
                (result, end)
            },
        )
        .await?;

        let matching_result = matching_result.map(|o| {
            (
                o,
                ActionExecutionMetadata {
                    dep_file_db_writes_queued: 0,
                    execution_kind: ActionExecutionKind::LocalDepFile,
                    timing: Default::default(),
                    input_files_bytes: None,
                    waiting_data: WaitingData::new(),
                },
            )
        });
        Ok(matching_result)
    }

    pub(crate) async fn check_remote_dep_file_entry(
        &self,
        digest_config: DigestConfig,
        fs: &ArtifactFs,
        materializer: &dyn Materializer,
        re_use_case: RemoteExecutorUseCase,
        found: &RemoteDepFile,
        result: &CommandExecutionResult,
    ) -> buck2_error::Result<bool> {
        // Everything in the common digest structure is included in the remote dep file key,
        // so they should be the same but it's good to double check.
        let common = &self.common_digests;
        if common.commandline_cli_digest.as_bytes() != found.commandline_cli_digest.as_slice() {
            tracing::debug!("Remote dep files miss: command cli digests are different");
            return Ok(false);
        }
        if common.output_paths_digest.raw_digest().as_bytes()
            != found.output_paths_digest.as_slice()
        {
            tracing::debug!("Remote dep files miss: output paths digest are different");
            return Ok(false);
        }
        if common.untagged_inputs_digest.raw_digest().as_bytes()
            != found.untagged_inputs_digest.as_slice()
        {
            tracing::debug!("Remote dep files miss: untagged inputs digest are different");
            return Ok(false);
        }

        // Ensure the declared dep files are the same
        if self.declared_dep_files.tagged.len() != found.dep_file_inputs.len() {
            tracing::debug!("Remote dep files miss: declared dep file counts are different");
            return Ok(false);
        }
        let different_dep_files_count = self
            .declared_dep_files
            .tagged
            .iter()
            .zip(found.dep_file_inputs.iter())
            .filter(|((_, d1), d2)| d1.output.get_path().to_string() != d2.dep_file_path)
            .count();
        if different_dep_files_count != 0 {
            tracing::debug!("Remote dep files miss: declared dep files are different");
            return Ok(false);
        }

        let action_outputs = ActionOutputs::new(
            result
                .outputs
                .iter()
                .filter_map(|(o, v)| Some((o.build_artifact_path()?.dupe(), v.dupe())))
                .collect(),
        );

        // Now we compare the filtered input digests.
        // To do this,
        // 1. Read the dep file contents, this should have been downloaded as part of the action cache lookup
        // 2. Filter our inputs so that only the inputs mentioned in the dep files are retained
        // 3. Compare that against the one we retrieved from the remote dep file cache: RemoteDepFile.dep_file_inputs
        let dep_files = read_dep_files(
            false,
            // The declared dep files of this and the found action are the same
            &self.declared_dep_files,
            &action_outputs,
            fs,
            materializer,
            re_use_case,
        )
        .await
        .buck_error_context("Error reading dep files")?;

        let dep_files = match dep_files {
            Some(dep_files) => dep_files,
            None => {
                tracing::debug!("Remote dep files miss: Dep files cannot be materialized");
                return Ok(false);
            }
        };

        let computed_filtered_fingerprints = self
            .shared_declared_inputs
            .as_ref()
            .internal_error("Dep files should have been declared")?
            .unshare()
            .filter(dep_files, fs)?
            .fingerprint(digest_config);

        let different_digest_count = computed_filtered_fingerprints
            .tagged
            .iter()
            .zip(found.dep_file_inputs.iter())
            .filter(|((_, f1), found)| {
                f1.fingerprint().raw_digest().as_bytes() != found.filtered_fingerprint.as_slice()
            })
            .count();

        if different_digest_count != 0 {
            tracing::debug!("Remote dep files miss: Filtered input digests are different");
            return Ok(false);
        }

        // Everything matches! The action from cache can be used.
        Ok(true)
    }
}

pub(crate) fn make_dep_file_bundle<'a>(
    ctx: &mut dyn ActionExecutionCtx,
    visitor: DepFilesCommandLineVisitor<'_>,
    expanded_command_line_digest: ExpandedCommandLineDigest,
    execution_paths: &'a CommandExecutionPaths,
    local_worker_execution_paths: Option<&'a CommandExecutionPaths>,
    executor_preference: ExecutorPreference,
) -> buck2_error::Result<DepFileBundle> {
    let input_directory_digest = execution_paths.input_directory().fingerprint();
    let dep_files_key = RunActionKey::from_action_execution_target(ctx.target());

    let DepFilesCommandLineVisitor {
        inputs: declared_inputs,
        tagged_outputs,
        ..
    } = visitor;

    // Filter out tags with no dep file associated with it
    let tagged_outputs: BuckFixedInsertionMap<ArtifactTag, DeclaredDepFile> = tagged_outputs
        .into_iter()
        .filter_map(|(tag, (label, output))| {
            let output = output?;
            Some((tag, DeclaredDepFile { label, output }))
        })
        .collect();

    let declared_dep_files = DeclaredDepFiles {
        tagged: tagged_outputs,
    };

    let (shared_declared_inputs, untagged_inputs_digest) = if !declared_dep_files.is_empty() {
        let shared_declared_inputs = declared_inputs
            .to_directories(ctx)?
            .share(ctx.digest_config());
        let untagged_inputs_digest = shared_declared_inputs.untagged.fingerprint().dupe();
        (Some(shared_declared_inputs), untagged_inputs_digest)
    } else {
        (None, input_directory_digest.dupe())
    };

    let local_cli_digest = local_cache_cli_digest(
        &expanded_command_line_digest,
        ctx.executor_result_fingerprint(),
        executor_preference,
    );
    // Construct digests needed to construct a remote dep file key and remote dep file entry (if needed)
    let common_digests = CommonDigests {
        commandline_cli_digest: expanded_command_line_digest,
        untagged_inputs_digest,
        output_paths_digest: get_output_path_digest(
            ctx.digest_config(),
            execution_paths.output_paths(),
        ),
        local_worker_inputs_digest: local_worker_execution_paths
            .map(|p| p.input_directory().fingerprint().dupe()),
    };

    Ok(DepFileBundle {
        dep_files_key,
        input_directory_digest: input_directory_digest.data().dupe(),
        shared_declared_inputs,
        declared_dep_files,
        common_digests,
        local_cli_digest,
        // We will lazily compute this when we need it.
        filtered_input_fingerprints: None,
    })
}

/// See if there is an identical action that matches in the cache.
/// If there is, return the outputs. Otherwise, additionally return a boolean to indicate if the lookup was a miss.
#[instrument(level = "debug", skip(input_directory_digest, cli_digest, ctx), fields(key = %key))]
pub(crate) async fn match_if_identical_action(
    ctx: &dyn ActionExecutionCtx,
    key: &RunActionKey,
    input_directory_digest: &FileDigest,
    local_worker_digest: &Option<TrackedFileDigest>,
    cli_digest: &ExpandedCommandLineDigest,
    declared_outputs: &[BuildArtifact],
    declared_dep_files: &DeclaredDepFiles,
    stats: &mut DepFileLookupStats,
) -> buck2_error::Result<(Option<ActionOutputs>, bool)> {
    // Every exit below is a miss unless it says otherwise, so the hit paths are the only ones that
    // have to remember to record anything. A failure is not one of them: the caller overwrites this
    // with `error` when the lookup returns `Err`.
    stats.outcome = buck2_data::DepFileLookupOutcome::Miss as i32;

    // First, this configuration's own cached action.
    if let Some(previous_state) = get_dep_files(ctx.dep_file_cache(), key) {
        let actions_match = check_action(
            Some(key),
            ctx.dep_file_cache(),
            ctx.dep_file_store(),
            DepFileCandidate::Live(&previous_state),
            input_directory_digest,
            local_worker_digest,
            cli_digest,
            declared_outputs,
            declared_dep_files,
        )?;

        if actions_match == InitialDepFileLookupResult::Hit {
            // Same-configuration hit: a live entry's outputs are already keyed by exactly this
            // configuration's `BuildArtifactPath`s. Return them directly (an `Arc` clone) and skip
            // the remap -- this is the common cache-hit hot path.
            let live = previous_state.result();
            if outputs_are_still_present_in_materializer(ctx, live).await? {
                tracing::trace!("Dep files are a hit");
                stats.hit_live();
                return Ok((Some(live.dupe()), false));
            }
            // Outputs no longer present; not a hit. Don't evict -- we didn't fully check dep files.
            return Ok((None, false));
        }

        // Don't remove the key from cache in this case as we did not fully check with the dep file content
        tracing::trace!("Local dep file cache does not have an identical action cached");
        return Ok((
            None,
            actions_match == InitialDepFileLookupResult::CheckFilteredInputs,
        ));
    }

    // No entry for this configuration. First reuse an identical action built this session under a
    // different configuration; if none match, fall back to the persisted store (previous daemon
    // sessions). Either way we honor only `check_action`'s exact-match `Hit` verdict (identical input
    // directory), never reading another configuration's dep file. The first candidate `check_action`
    // matches decides the outcome: if its outputs are still materialized we serve them, otherwise it
    // is a miss (every candidate reuses the same configuration-independent outputs, so a later one
    // would not help). `states_for_logical` clones the candidates out, so we hold no shard guard
    // across the async work.
    let logical = key.to_logical();
    for candidate in dep_files(ctx.dep_file_cache()).states_for_logical(&logical) {
        match probe_cross_config_candidate(
            ctx,
            declared_outputs,
            DepFileCandidate::Live(&candidate),
            input_directory_digest,
            local_worker_digest,
            cli_digest,
            declared_dep_files,
        )
        .await?
        {
            CrossConfigProbe::NotHit => {}
            CrossConfigProbe::Hit(outputs) => {
                tracing::trace!("Cross-configuration local action cache hit");
                stats.hit_live();
                return Ok((Some(outputs), false));
            }
            CrossConfigProbe::OutputsGone => return Ok((None, false)),
        }
    }

    // Nothing built this session matched. Consult the persisted store, loading just this logical
    // action's rows on demand rather than holding the whole db in memory. On a hit we promote the
    // entry into the live `map` so subsequent same-configuration lookups take the fast path.
    //
    // Promotion does not write to the store, so a row's `last_write_time` tracks when the action
    // last *executed*, not when it was last served. An action that keeps hitting this path without
    // re-executing is therefore pruned once it passes `sqlite_dep_file_state_ttl_days`.
    if let Some(store) = ctx.dep_file_store() {
        let logical_key = encode_logical_key(&logical);
        // Two phases: reject on the scalar row alone, and only fetch a candidate's outputs and
        // declared dep files once its digests match. `probe_cross_config_candidate` honors nothing
        // but `Hit`, whose digest half is exactly `matches_all`, so a candidate rejected here could
        // never have been served -- but reading it costs two more queries and the deserialization of
        // every output row, on a path taken once per run action by a freshly started daemon.
        //
        // The probe is advisory. Writes are applied asynchronously, so one can land between the two
        // reads, and the row handle does not pin what it addresses: rewriting an entry deletes and
        // re-inserts its row, and sqlite recycles the id, so the fetch can return the replacement or
        // even an unrelated entry that inherited the id. `check_action` re-validates whatever is
        // served, so a stale probe costs at most a wasted fetch.
        // A row that cannot be decoded never will be, so drop it instead of skipping it: leaving it
        // would re-read and re-fail on every lookup of this action until its TTL expires. Reported
        // rather than logged: `DEP_FILE_DB_SCHEMA_VERSION` recreates the db whenever the encoding
        // changes, so a row that fails to decode means that version was not bumped, and the drift
        // would otherwise disable the cache silently.
        let drop_malformed = |config_key: Vec<u8>, e: buck2_error::Error| {
            soft_error_or_panic!(
                "malformed_dep_file_db_entry",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Dropping malformed persisted dep-file entry. {}",
                    e
                )
            );
            store.delete(logical_key.clone(), config_key);
        };
        let probe_started = Instant::now();
        let candidates = store.get_digests(&logical_key);
        stats.add_probe(Instant::now() - probe_started);
        for digests in candidates {
            let candidate_digests = match stored_command_digests(&digests) {
                Ok(candidate_digests) => candidate_digests,
                Err(e) => {
                    drop_malformed(digests.config_key, e);
                    continue;
                }
            };
            if !candidate_digests.matches_all(
                cli_digest,
                local_worker_digest,
                input_directory_digest,
            ) {
                continue;
            }
            let fetch_started = Instant::now();
            let stored = store.get_entry(digests.id);
            stats.add_fetch(Instant::now() - fetch_started);
            let Some(stored) = stored else {
                continue;
            };
            let loaded = match LoadedEntry::from_stored(stored) {
                Ok(loaded) => loaded,
                Err(e) => {
                    drop_malformed(digests.config_key, e);
                    continue;
                }
            };
            match probe_cross_config_candidate(
                ctx,
                declared_outputs,
                DepFileCandidate::Loaded(&loaded),
                input_directory_digest,
                local_worker_digest,
                cli_digest,
                declared_dep_files,
            )
            .await?
            {
                CrossConfigProbe::NotHit => {}
                CrossConfigProbe::Hit(outputs) => {
                    promote_reloaded_entry(
                        ctx.dep_file_cache(),
                        &logical,
                        key.configuration(),
                        loaded,
                        outputs.dupe(),
                    );
                    tracing::trace!("Persisted local action cache hit");
                    store.note_persisted_hit();
                    stats.hit_persisted();
                    return Ok((Some(outputs), false));
                }
                CrossConfigProbe::OutputsGone => return Ok((None, false)),
            }
        }
    }

    Ok((None, false))
}

/// Outcome of testing one cross-configuration (or persisted) candidate against the live action.
enum CrossConfigProbe {
    /// `check_action` did not consider this an identical action.
    NotHit,
    /// Identical action and its outputs are still materialized: reuse them.
    Hit(ActionOutputs),
    /// Identical action, but its outputs are no longer materialized -- a safe miss.
    OutputsGone,
}

/// Test one candidate (another configuration built this session, or an entry reloaded from the
/// persisted store) against the live action. Honors only the exact-match `Hit` verdict, then
/// re-keys the cached outputs onto the live declared outputs and confirms they are still
/// materialized. `None` is passed as the evict key: the candidate belongs to another configuration
/// (or a prior session), so evicting the live `key` here would be meaningless.
async fn probe_cross_config_candidate(
    ctx: &dyn ActionExecutionCtx,
    declared_outputs: &[BuildArtifact],
    candidate: DepFileCandidate<'_>,
    input_directory_digest: &FileDigest,
    local_worker_digest: &Option<TrackedFileDigest>,
    cli_digest: &ExpandedCommandLineDigest,
    declared_dep_files: &DeclaredDepFiles,
) -> buck2_error::Result<CrossConfigProbe> {
    if check_action(
        None,
        ctx.dep_file_cache(),
        ctx.dep_file_store(),
        candidate,
        input_directory_digest,
        local_worker_digest,
        cli_digest,
        declared_outputs,
        declared_dep_files,
    )? != InitialDepFileLookupResult::Hit
    {
        return Ok(CrossConfigProbe::NotHit);
    }
    match hit_outputs_if_present(ctx, declared_outputs, candidate).await? {
        Some(outputs) => Ok(CrossConfigProbe::Hit(outputs)),
        None => Ok(CrossConfigProbe::OutputsGone),
    }
}

/// Promote an entry reloaded from the persisted store into the live `map` under `cfg`, so later
/// same-configuration lookups take the fast path without re-querying the store.
///
/// The reloaded `declared` (its config-independent identities) is carried over unchanged so
/// `check_action` still matches the identical action on the fast path. A reloaded entry has no live
/// signature machinery, so if it declared dep files it cannot serve the dep-file-filtered path;
/// `match_or_clear_dep_file` recognizes such an entry and leaves it (and its persisted row) intact
/// instead of clearing it, so the action just re-executes and re-populates a live entry.
fn promote_reloaded_entry(
    cache: &dyn DepFileCache,
    logical: &LogicalActionKey,
    cfg: Option<Configuration>,
    loaded: LoadedEntry,
    outputs: ActionOutputs,
) {
    let promoted = DepFileState {
        // `check_action` confirmed these equal the live action's digests.
        digests: loaded.digests,
        result: outputs,
        was_produced_locally: loaded.was_produced_locally,
        declared: loaded.declared,
    };
    // The insert is unconditional: DICE evaluates an action key once and shares the result, so no
    // concurrent execution of this `(logical, cfg)` can have filled the slot with a `Live` entry
    // whose signature machinery this would discard.
    dep_files(cache).insert(logical.dupe(), cfg, Arc::new(promoted));
}

/// Reuse a cached entry's outputs for `declared_outputs` if they are still present in the
/// materializer. Re-keys the cached (possibly configuration-independent or reloaded) values onto the
/// live declared outputs, then confirms the materializer still holds them. Returns `None` if they
/// are gone (a safe miss).
async fn hit_outputs_if_present(
    ctx: &dyn ActionExecutionCtx,
    declared_outputs: &[BuildArtifact],
    candidate: DepFileCandidate<'_>,
) -> buck2_error::Result<Option<ActionOutputs>> {
    let remapped = match candidate {
        DepFileCandidate::Live(state) => remap_live_outputs(declared_outputs, &state.result)?,
        DepFileCandidate::Loaded(loaded) => {
            match resolve_loaded_outputs(ctx, declared_outputs, &loaded.outputs).await? {
                Some(remapped) => remapped,
                None => return Ok(None),
            }
        }
    };
    if outputs_are_still_present_in_materializer(ctx, &remapped).await? {
        Ok(Some(remapped))
    } else {
        Ok(None)
    }
}

/// Re-key a live entry's `ActionOutputs` onto this configuration's `declared_outputs`, matched by
/// configuration-independent output path.
fn remap_live_outputs(
    declared_outputs: &[BuildArtifact],
    cached: &ActionOutputs,
) -> buck2_error::Result<ActionOutputs> {
    let by_short_path: BuckMutMap<&ForwardRelativePath, &ArtifactValue> = cached
        .iter()
        .map(|(path, value)| (path.path(), value))
        .collect();
    let outputs = declared_outputs
        .iter()
        .map(|declared| {
            let path = declared.get_path();
            let value = by_short_path.get(&(path.path())).ok_or_else(|| {
                internal_error!(
                    "Dep file cache hit but cached result is missing output `{}`",
                    path.path()
                )
            })?;
            buck2_error::Ok((path.dupe(), (*value).dupe()))
        })
        .collect::<buck2_error::Result<_>>()?;
    Ok(ActionOutputs::new(outputs))
}

/// Build an `ActionOutputs` for `declared_outputs` from a reloaded entry, rehydrating directory
/// values from the materializer. Leaf values are used directly; directory values (stored as only a
/// fingerprint) are fetched from the materializer -- which already persists+reloads the full tree --
/// and verified against the stored fingerprint. Returns `None` (a safe miss) if any output is
/// missing from the materializer or its fingerprint no longer matches (e.g. a different action
/// overwrote that path).
async fn resolve_loaded_outputs(
    ctx: &dyn ActionExecutionCtx,
    declared_outputs: &[BuildArtifact],
    outputs: &[StoredOutput],
) -> buck2_error::Result<Option<ActionOutputs>> {
    let fs = ctx.fs();
    let by_short_path: BuckMutMap<&ForwardRelativePath, &StoredOutputValue> = outputs
        .iter()
        .map(|o| (o.path.as_ref(), &o.value))
        .collect();

    // Directory value pending a materializer fetch: which declared output it belongs to, its on-disk
    // path, and the fingerprint to verify the fetched tree against.
    enum Plan<'a> {
        Ready(ArtifactValue),
        Directory {
            project_path: ProjectRelativePathBuf,
            fingerprint: &'a FileDigest,
        },
    }

    let mut plan: Vec<(BuildArtifactPath, Plan)> = Vec::with_capacity(declared_outputs.len());
    for declared in declared_outputs {
        let path = declared.get_path();
        let value = by_short_path.get(&(path.path())).ok_or_else(|| {
            internal_error!(
                "Dep file cache hit but cached result is missing output `{}`",
                path.path()
            )
        })?;
        let entry = match value {
            StoredOutputValue::Leaf(v) => Plan::Ready((*v).dupe()),
            StoredOutputValue::Directory(fingerprint) => {
                // A content-based directory path embeds its fingerprint, so we must supply it to
                // resolve the on-disk path we will query. Deriving the hash from the fingerprint
                // reproduces the path only because an action's own output values always carry an
                // inferred hash: an explicit one is set solely on projected artifacts, which are
                // views onto a base artifact rather than outputs an action declares. The same
                // assumption lets `StoredOutputValue::Leaf` round-trip without persisting the hash.
                let content_hash = if path.is_content_based_path() {
                    Some(ContentBasedPathHash::new(
                        fingerprint.raw_digest().as_bytes(),
                    )?)
                } else {
                    None
                };
                let project_path = fs
                    .buck_out_path_resolver()
                    .resolve_gen(path, content_hash.as_ref())?;
                Plan::Directory {
                    project_path,
                    fingerprint,
                }
            }
        };
        plan.push((path.dupe(), entry));
    }

    let dir_paths: Vec<ProjectRelativePathBuf> = plan
        .iter()
        .filter_map(|(_, p)| match p {
            Plan::Directory { project_path, .. } => Some(project_path.clone()),
            Plan::Ready(_) => None,
        })
        .collect();
    let mut fetched: BuckMutMap<ProjectRelativePathBuf, _> = if dir_paths.is_empty() {
        BuckMutMap::default()
    } else {
        ctx.materializer()
            .get_artifact_entries_for_materialized_paths(dir_paths, false)
            .await?
            .into_iter()
            .flatten()
            .collect()
    };

    let mut result = Vec::with_capacity(plan.len());
    for (decl_path, p) in plan {
        let value = match p {
            Plan::Ready(v) => v,
            Plan::Directory {
                project_path,
                fingerprint,
            } => {
                let Some(entry) = fetched.remove(&project_path) else {
                    tracing::trace!(
                        "Reloaded directory output `{}` is not materialized",
                        project_path
                    );
                    return Ok(None);
                };
                match entry {
                    DirectoryEntry::Dir(dir) => {
                        if dir.fingerprint().data() != fingerprint {
                            tracing::trace!(
                                "Reloaded directory output `{}` fingerprint changed",
                                project_path
                            );
                            return Ok(None);
                        }
                        ArtifactValue::dir(dir)
                    }
                    DirectoryEntry::Leaf(_) => {
                        tracing::trace!(
                            "Reloaded directory output `{}` is now a leaf",
                            project_path
                        );
                        return Ok(None);
                    }
                }
            }
        };
        result.push((decl_path, value));
    }
    Ok(Some(ActionOutputs::new(result.into_iter().collect())))
}

/// Match the dep file recorded for key, or clear it from the map (if it exists).
#[instrument(level = "debug", skip(input_directory_digest,cli_digest,declared_inputs,ctx), fields(key = %key))]
pub(crate) async fn match_or_clear_dep_file(
    ctx: &dyn ActionExecutionCtx,
    key: &RunActionKey,
    input_directory_digest: &FileDigest,
    local_worker_digest: &Option<TrackedFileDigest>,
    cli_digest: &ExpandedCommandLineDigest,
    declared_inputs: &Option<PartitionedInputs<ActionSharedDirectory>>,
    declared_outputs: &[BuildArtifact],
    declared_dep_files: &DeclaredDepFiles,
) -> buck2_error::Result<Option<ActionOutputs>> {
    let previous_state = match get_dep_files(ctx.dep_file_cache(), key) {
        Some(d) => d.dupe(),
        None => return Ok(None),
    };

    let filtered_match = dep_files_match(
        key,
        &previous_state,
        input_directory_digest,
        local_worker_digest,
        cli_digest,
        declared_inputs,
        declared_outputs,
        declared_dep_files,
        ctx,
    )
    .await?;
    if filtered_match == DepFileFilteredMatch::Match
        && let Some(outputs) = hit_outputs_if_present(
            ctx,
            declared_outputs,
            DepFileCandidate::Live(&previous_state),
        )
        .await?
    {
        tracing::trace!("Dep files are a hit");
        return Ok(Some(outputs));
    }

    // `CannotEvaluate` is not staleness, so it is the one outcome that keeps the entry: clearing it
    // would also delete its persisted row, throwing away a still-valid identical-action entry.
    // A `Match` whose outputs are gone falls through to here and is cleared like a `Miss`.
    if filtered_match != DepFileFilteredMatch::CannotEvaluate {
        tracing::trace!("Dep files are a miss, removing the key from cache");
        remove_dep_file_entry(ctx.dep_file_cache(), key, ctx.dep_file_store());
    }

    Ok(None)
}

/// What a lookup did, reported on `MatchDepFilesEnd`. The persisted timings stay `None` when the
/// store was not consulted, which is the common case once the in-memory cache is warm -- an absent
/// value and a zero mean different things here.
#[derive(Debug, Default)]
pub(crate) struct DepFileLookupStats {
    outcome: i32,
    probe_us: Option<u64>,
    fetch_us: Option<u64>,
    fetches: Option<u64>,
}

impl DepFileLookupStats {
    fn hit_live(&mut self) {
        self.outcome = buck2_data::DepFileLookupOutcome::Live as i32;
    }

    fn hit_persisted(&mut self) {
        self.outcome = buck2_data::DepFileLookupOutcome::Persisted as i32;
    }

    /// Overrides whatever the lookup had recorded: it did not finish, so what it reached before
    /// failing does not describe the outcome.
    fn error(&mut self) {
        self.outcome = buck2_data::DepFileLookupOutcome::Error as i32;
    }

    fn add_probe(&mut self, d: Duration) {
        *self.probe_us.get_or_insert(0) += d.as_micros() as u64;
    }

    fn add_fetch(&mut self, d: Duration) {
        *self.fetch_us.get_or_insert(0) += d.as_micros() as u64;
        *self.fetches.get_or_insert(0) += 1;
    }

    pub(crate) fn into_end_event(self) -> buck2_data::MatchDepFilesEnd {
        buck2_data::MatchDepFilesEnd {
            outcome: self.outcome,
            persisted_probe_us: self.probe_us,
            persisted_fetch_us: self.fetch_us,
            persisted_fetches: self.fetches,
        }
    }
}

#[derive(PartialEq)]
enum InitialDepFileLookupResult {
    Hit,
    Miss,
    CheckFilteredInputs,
}

/// Outcome of comparing an action against a cached entry on the dep-file-filtered path. `Miss` and
/// `CannotEvaluate` are both "not a hit", but only `Miss` says the entry is stale: `CannotEvaluate`
/// means the comparison never ran, so the entry is kept.
#[derive(PartialEq)]
enum DepFileFilteredMatch {
    Match,
    /// The filtered inputs differ, the dep files cannot be materialized, or there are no dep files
    /// to filter on and the action is not identical.
    Miss,
    /// The entry carries no live signature machinery, so the filtered inputs cannot be compared
    /// against it.
    CannotEvaluate,
}

async fn outputs_are_still_present_in_materializer(
    ctx: &dyn ActionExecutionCtx,
    result: &ActionOutputs,
) -> buck2_error::Result<bool> {
    let fs = ctx.fs();

    // Finally, we need to make sure that the artifacts in the materializer actually
    // match. This is necessary in case a different action wrote to those artifacts and
    // didn't use the same cache key.
    let output_matches = result
        .iter()
        .map(|(path, value)| {
            Ok((
                fs.buck_out_path_resolver()
                    .resolve_gen(path, Some(&value.content_based_path_hash()))?,
                value.dupe(),
            ))
        })
        .collect::<buck2_error::Result<Vec<(ProjectRelativePathBuf, ArtifactValue)>>>()?;

    let materializer_accepts = ctx
        .materializer()
        .declare_match(output_matches)
        .await?
        .is_match();
    Ok(materializer_accepts)
}

/// Check whether a cached entry matches the current action. `evict_key`, when `Some`, is the key to
/// evict on a genuine miss (used for this configuration's own entry); pass `None` when checking
/// another configuration's or a reloaded candidate, where eviction would be meaningless.
fn check_action(
    evict_key: Option<&RunActionKey>,
    dep_file_cache: &dyn DepFileCache,
    dep_file_store: Option<&dyn DepFileStore>,
    candidate: DepFileCandidate<'_>,
    input_directory_digest: &FileDigest,
    local_worker_digest: &Option<TrackedFileDigest>,
    cli_digest: &ExpandedCommandLineDigest,
    declared_outputs: &[BuildArtifact],
    declared_dep_files: &DeclaredDepFiles,
) -> buck2_error::Result<InitialDepFileLookupResult> {
    let evict = || {
        if let Some(key) = evict_key {
            remove_dep_file_entry(dep_file_cache, key, dep_file_store);
        }
    };

    if !candidate.declared().matches_action(declared_dep_files)? {
        // We first need to check if the same dep files existed before or not. If not, then we
        // can't assume they'll still be on disk, and we have to bail.
        tracing::trace!("Dep files miss: Dep files declaration has changed");
        evict();
        return Ok(InitialDepFileLookupResult::Miss);
    }

    if !outputs_are_reusable(declared_outputs, candidate) {
        tracing::trace!("Dep files miss: Output declaration has changed");
        evict();
        return Ok(InitialDepFileLookupResult::Miss);
    }

    if !candidate.digests().cli_matches(cli_digest) {
        tracing::trace!("Dep files miss: Command line has changed");
        evict();
        return Ok(InitialDepFileLookupResult::Miss);
    }

    if !candidate
        .digests()
        .local_worker_matches(local_worker_digest)
    {
        tracing::trace!("Dep files miss: Local worker directory has changed");
        evict();
        return Ok(InitialDepFileLookupResult::Miss);
    }

    if candidate
        .digests()
        .directory_matches(input_directory_digest)
    {
        // The actions are identical
        tracing::trace!("Dep files hit: Command line and directory have not changed");
        return Ok(InitialDepFileLookupResult::Hit);
    }
    Ok(InitialDepFileLookupResult::CheckFilteredInputs)
}

async fn dep_files_match(
    key: &RunActionKey,
    previous_state: &DepFileState,
    input_directory_digest: &FileDigest,
    local_worker_digest: &Option<TrackedFileDigest>,
    cli_digest: &ExpandedCommandLineDigest,
    declared_inputs: &Option<PartitionedInputs<ActionSharedDirectory>>,
    declared_outputs: &[BuildArtifact],
    declared_dep_files: &DeclaredDepFiles,
    ctx: &dyn ActionExecutionCtx,
) -> buck2_error::Result<DepFileFilteredMatch> {
    let initial_check = check_action(
        Some(key),
        ctx.dep_file_cache(),
        ctx.dep_file_store(),
        DepFileCandidate::Live(previous_state),
        input_directory_digest,
        local_worker_digest,
        cli_digest,
        declared_outputs,
        declared_dep_files,
    )?;
    if initial_check == InitialDepFileLookupResult::Hit {
        return Ok(DepFileFilteredMatch::Match);
    }

    // We didn't get an exact match, and we don't have any dep files, so we're done.
    if declared_dep_files.is_empty() {
        return Ok(DepFileFilteredMatch::Miss);
    }

    // The dep-file-filtered path needs the cached entry's declared dep files, which only entries
    // produced this session carry. A promoted entry has only config-independent identities and no
    // live signature machinery, so the filtered inputs cannot be evaluated against it at all. Only a
    // reloaded entry reaches this arm: a live entry that declared no dep files would have failed
    // `check_action`'s declaration comparison against this dep-file-declaring action.
    let previous_declared_dep_files = match previous_state.declared_dep_files() {
        Some(declared_dep_files) => declared_dep_files,
        None => {
            return Ok(DepFileFilteredMatch::CannotEvaluate);
        }
    };
    let previous_result = previous_state.result();

    let dep_files = read_dep_files(
        previous_state.has_signatures(),
        previous_declared_dep_files,
        previous_result,
        ctx.fs(),
        ctx.materializer(),
        ctx.invocation_re_use_case(),
    )
    .await
    .buck_error_context(
        "Error reading persisted dep files. \
            Fix the command that produced an invalid dep file. \
            You may also use `buck2 debug flush-dep-files` to drop all dep file state.",
    )?;

    let dep_files = match dep_files {
        Some(dep_files) => dep_files,
        None => {
            tracing::trace!("Dep files miss: Dep files cannot be materialized");
            return Ok(DepFileFilteredMatch::Miss);
        }
    };

    // Now we need to know the fingerprints on the original action. Produce them if they're
    // missing. We're either storing input directories or outputs here.

    let digest_config = ctx.digest_config();

    let fingerprints_match = {
        // NOTE: We don't bother releasing the guard here (we'd have to clone the fingerprints to do
        // so), because this Mutex won't be contended: only one action will look at its value.
        let previous_fingerprints = previous_state.locked_compute_fingerprints(
            Cow::Borrowed(&dep_files),
            keep_directories()?,
            digest_config,
            ctx.fs(),
        )?;

        // NOTE: We use the new directory to e.g. resolve symlinks referenced in the dep file. This
        // makes sense: if a path in the depfile is still a symlink, then we'll compare the new
        // destination and the old. If it's not, then we can assume that the tool wouldn't traverse
        // the symlink anymore.
        let new_fingerprints = declared_inputs
            .as_ref()
            .expect("Must have declared inputs when we have dep-files!")
            .unshare()
            .filter(dep_files, ctx.fs())?
            .fingerprint(digest_config);
        *previous_fingerprints == new_fingerprints
    };

    Ok(if fingerprints_match {
        DepFileFilteredMatch::Match
    } else {
        DepFileFilteredMatch::Miss
    })
}

/// If an action is unchanged but now requires a different set of outputs, that's not a cache hit
/// because we need to rehash the outputs. Having to re-run the action isn't the best, but it's
/// probably infrequent enough that we seem unlikely to care.
fn outputs_are_reusable(
    declared_outputs: &[BuildArtifact],
    candidate: DepFileCandidate<'_>,
) -> bool {
    // Match by configuration-independent output path so an identical action built under a different
    // configuration (or reloaded from disk) is still recognized as reusable.
    let cached: BuckMutSet<&ForwardRelativePath> = candidate.short_paths().collect();
    declared_outputs.iter().all(|out| {
        let path = out.get_path();
        cached.contains(&(path.path()))
    })
}

/// Read the dep files for this DepFileState. This will return None if the dep files cannot be
/// materialized (because they e.g. expired).
pub(crate) async fn read_dep_files(
    has_signatures: bool,
    declared_dep_files: &DeclaredDepFiles,
    result: &ActionOutputs,
    fs: &ArtifactFs,
    materializer: &dyn Materializer,
    re_use_case: RemoteExecutorUseCase,
) -> buck2_error::Result<Option<ConcreteDepFiles>> {
    // NOTE: We only materialize if we haven't computed our signatures yet, since we know we
    // can't have computed our signatures without having read the dep file already. In an ideal
    // world this wouldn't be necessary, but in practice contention on the materializer makes
    // this slower.
    let _lease = if !has_signatures {
        match declared_dep_files
            .materialize(fs, materializer, result, re_use_case)
            .await
        {
            Ok(lease) => Some(lease),
            Err(MaterializeDepFilesError::NotFound) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    } else {
        None
    };

    let dep_files = declared_dep_files.read(fs, result).buck_error_context(
        "Error reading dep files, verify that the action produced valid output",
    )?;

    Ok(dep_files)
}

fn compute_fingerprints(
    directories: PartitionedInputsBuilder<ActionDirectoryBuilder>,
    dep_files: ConcreteDepFiles,
    digest_config: DigestConfig,
    keep_directories: bool,
    fs: &ArtifactFs,
) -> buck2_error::Result<StoredFingerprints> {
    let filtered_directories = directories
        .filter(dep_files, fs)?
        .fingerprint(digest_config);
    if keep_directories {
        Ok(StoredFingerprints::Dirs(filtered_directories))
    } else {
        Ok(StoredFingerprints::Digests(
            filtered_directories.as_fingerprints(),
        ))
    }
}

async fn eagerly_compute_fingerprints(
    digest_config: DigestConfig,
    artifact_fs: &ArtifactFs,
    materializer: &dyn Materializer,
    re_use_case: RemoteExecutorUseCase,
    shared_declared_inputs: &PartitionedInputs<ActionSharedDirectory>,
    declared_dep_files: &DeclaredDepFiles,
    result: &ActionOutputs,
) -> buck2_error::Result<StoredFingerprints> {
    let dep_files = read_dep_files(
        false,
        declared_dep_files,
        result,
        artifact_fs,
        materializer,
        re_use_case,
    )
    .await?
    .internal_error("Dep file not found")?;

    let fingerprints = compute_fingerprints(
        shared_declared_inputs.unshare(),
        dep_files,
        digest_config,
        keep_directories()?,
        artifact_fs,
    )?;
    Ok(fingerprints)
}

/// Post-process the dep files produced by an action.
/// Returns whether an entry was queued for persisting.
pub(crate) async fn populate_dep_files(
    ctx: &dyn ActionExecutionCtx,
    dep_file_bundle: DepFileBundle,
    result: &ActionOutputs,
    was_produced_locally: bool,
) -> buck2_error::Result<bool> {
    let DepFileBundle {
        declared_dep_files,
        dep_files_key,
        input_directory_digest,
        shared_declared_inputs,
        filtered_input_fingerprints,
        common_digests,
        local_cli_digest,
    } = dep_file_bundle;
    let should_compute_fingerprints = ctx.run_action_knobs().eager_dep_files;
    let digests = CommandDigests {
        cli: local_cli_digest,
        directory: input_directory_digest,
        local_worker_directory: common_digests.local_worker_inputs_digest,
    };

    let result = result.dupe();

    let state = if declared_dep_files.is_empty() {
        DepFileState {
            digests,
            result,
            was_produced_locally,
            declared: DepFileDeclaration::None,
        }
    } else {
        let input_signatures = match filtered_input_fingerprints {
            Some(fingerprints) => DepFileStateInputSignatures::Computed(fingerprints),
            None => {
                let shared_declared_inputs = shared_declared_inputs.ok_or_else(|| {
                    internal_error!("Must have inputs when dep-files are present!")
                })?;
                if should_compute_fingerprints {
                    let fingerprints = eagerly_compute_fingerprints(
                        ctx.digest_config(),
                        ctx.fs(),
                        ctx.materializer(),
                        ctx.invocation_re_use_case(),
                        &shared_declared_inputs,
                        &declared_dep_files,
                        &result,
                    )
                    .await?;
                    DepFileStateInputSignatures::Computed(fingerprints)
                } else {
                    DepFileStateInputSignatures::Deferred(shared_declared_inputs)
                }
            }
        };

        DepFileState {
            digests,
            result,
            was_produced_locally,
            declared: DepFileDeclaration::Live {
                declared_dep_files,
                input_signatures: Mutex::new(input_signatures),
            },
        }
    };

    // Persist the entry (best-effort) before installing it in memory. We only persist
    // locally-produced entries: those are the ones worth reloading (their outputs are already on
    // disk), and it keeps the on-disk cache consistent with `DepFileCache::clear_non_local`, which
    // evicts non-local entries from memory. `to_stored` returns `None` for an entry that is not safe
    // to persist (an output's symlink destinations are *not* covered by "already on disk").
    let logical = dep_files_key.to_logical();
    let cfg = dep_files_key.configuration();
    let mut queued_write = false;
    if was_produced_locally && let Some(store) = ctx.dep_file_store() {
        // Persisting is best-effort, per the `DepFileStore` contract: a failure to serialize costs a
        // cache miss in a later session and must not fail this build.
        match state.to_stored() {
            Ok(Some(stored)) => {
                store.insert(
                    encode_logical_key(&logical),
                    encode_config_key(cfg.dupe()),
                    stored,
                );
                queued_write = true;
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("Not persisting dep-file entry: {}", e),
        }
    }

    // Install the live entry. It shadows any persisted row for this `(logical, cfg)` on subsequent
    // lookups, since the live `map` is consulted before the persisted store. When we declined to
    // persist above, the previous row is left in place; a lookup re-validates it against the
    // action's digests and the materializer before serving it. Rows this version writes are
    // deps-free, but nothing in the schema enforces that, so the re-validation is what makes
    // leaving the row safe.
    dep_files(ctx.dep_file_cache()).insert(logical.dupe(), cfg, Arc::new(state));
    Ok(queued_write)
}

/// Inputs partitioned by dep-file label: the inputs no dep file covers, then one partition per
/// declared label, in declaration order. `D` is the representation of one partition.
///
/// Once built, the partitions are only read; this is the form the dep-file cache keeps and
/// compares. The stages that still write to their partitions are [`PartitionedInputsBuilder`]s.
#[derive(Clone, PartialEq, Eq, Allocative)]
pub(crate) struct PartitionedInputs<D> {
    pub(crate) untagged: D,
    pub(crate) tagged: BuckFixedInsertionMap<Arc<str>, D>,
}

/// [`PartitionedInputs`] whose partitions are still being written to: the artifacts gathered off
/// the command line, and the directory builders a dep file is applied to.
pub(crate) struct PartitionedInputsBuilder<D> {
    untagged: D,
    tagged: BuckMutInsertionMap<Arc<str>, D>,
}

impl<D> PartitionedInputsBuilder<D> {
    pub(crate) fn iter(&self) -> impl Iterator<Item = &D> {
        std::iter::once(&self.untagged).chain(self.tagged.values())
    }
}

impl PartitionedInputsBuilder<Vec<ArtifactGroup>> {
    /// Add an input to this set of PartitionedInputs.
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        tags: Vec<&ArtifactTag>,
        dep_files: &RunActionDepFiles,
    ) {
        for tag in tags {
            match dep_files.labels.get(tag) {
                None => {}
                Some(label) => {
                    self.tagged.get_mut(label).unwrap().push(input);
                    // We already verified that there is only one relevant dep-file tag per input, so we are done.
                    return;
                }
            }
        }

        self.untagged.push(input);
    }

    /// Produce Directories from this set of PartitionedInputs. One directory will be produced for
    /// each tag (and one for untagged). This will actually allocate directories (whereas until now
    /// we only held references to artifacts).
    fn to_directories(
        &self,
        ctx: &dyn ActionExecutionCtx,
    ) -> buck2_error::Result<PartitionedInputsBuilder<ActionDirectoryBuilder>> {
        fn reduce(
            ctx: &dyn ActionExecutionCtx,
            inputs: &[ArtifactGroup],
        ) -> buck2_error::Result<ActionDirectoryBuilder> {
            let mut builder = LazyActionDirectoryBuilder::empty();

            for input in inputs {
                let input = ctx.artifact_values(input);
                input.add_to_directory(&mut builder, ctx.fs())?;
            }

            builder.finalize()
        }

        Ok(PartitionedInputsBuilder {
            untagged: reduce(ctx, &self.untagged)?,
            tagged: self
                .tagged
                .iter()
                .map(|(tag, inputs)| buck2_error::Ok((tag.dupe(), reduce(ctx, inputs)?)))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl PartitionedInputs<ActionSharedDirectory> {
    fn unshare(&self) -> PartitionedInputsBuilder<ActionDirectoryBuilder> {
        PartitionedInputsBuilder {
            untagged: self.untagged.dupe().into_builder(),
            tagged: self
                .tagged
                .iter()
                .map(|(k, v)| (k.dupe(), v.dupe().into_builder()))
                .collect(),
        }
    }
}

impl PartitionedInputsBuilder<ActionDirectoryBuilder> {
    fn share(self, digest_config: DigestConfig) -> PartitionedInputs<ActionSharedDirectory> {
        PartitionedInputs {
            untagged: self
                .untagged
                .fingerprint(digest_config.as_directory_serializer())
                .shared(&*INTERNER),
            tagged: self
                .tagged
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        v.fingerprint(digest_config.as_directory_serializer())
                            .shared(&*INTERNER),
                    )
                })
                .collect(),
        }
    }

    /// Filter this set of PartitionedInputs using dep files that were produced by the command (or
    /// a previous invocation thereof). For each partition of inputs (i.e. untagged, or specific
    /// tags), only the inputs listed in the dep file will be retained.
    fn filter(
        mut self,
        mut concrete_dep_files: ConcreteDepFiles,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<Self> {
        fn filter(
            label: &Arc<str>,
            builder: &mut ActionDirectoryBuilder,
            concrete_dep_files: &mut ConcreteDepFiles,
            fs: &ArtifactFs,
        ) -> buck2_error::Result<()> {
            let mut matching_selector = match concrete_dep_files.get_selector(label, fs, builder)? {
                Some(s) => s,
                None => return Ok(()),
            };

            expand_selector_for_dependencies(builder, &mut matching_selector);

            // NOTE: We ignore the filtering if it produces an invalid directory. If we can't
            // filter, that means we're selecting into a leaf, which means one of two things:
            //
            // - We hit an external symlink, and we can't traverse through those.
            // - The directory structure changed and was a dir is now a leaf, in which case we'll
            // select the leaf but when comparing this to the dir it'll conflict.
            let _ignored = matching_selector.filter(builder);

            Ok(())
        }

        for (label, dir) in self.tagged.iter_mut() {
            filter(label, dir, &mut concrete_dep_files, fs)?;
        }

        Ok(self)
    }

    /// Compute fingerprints for all the PartitionedInputs in here. This lets us do comparisons.
    fn fingerprint(
        self,
        digest_config: DigestConfig,
    ) -> PartitionedInputs<ActionImmutableDirectory> {
        PartitionedInputs {
            untagged: self
                .untagged
                .fingerprint(digest_config.as_directory_serializer()),
            tagged: self
                .tagged
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        v.fingerprint(&TaggedInputsDirectorySerializer {
                            cas_digest_config: digest_config.cas_digest_config(),
                        }),
                    )
                })
                .collect(),
        }
    }
}

impl PartitionedInputs<ActionImmutableDirectory> {
    fn as_fingerprints(&self) -> PartitionedInputs<TrackedFileDigest> {
        PartitionedInputs {
            untagged: self.untagged.fingerprint().dupe(),
            tagged: self
                .tagged
                .iter()
                .map(|(k, v)| (k.dupe(), v.fingerprint().dupe()))
                .collect(),
        }
    }
}

/// A dep file declared by a command. This includes its label (which we use for stability across
/// command executions), as well as the artifact that the command will write the dep file to.
#[derive(Hash, PartialEq, Eq, Debug, Dupe, Clone, Allocative)]
struct DeclaredDepFile {
    label: Arc<str>,
    output: Artifact,
}

impl DeclaredDepFile {
    /// The configuration-independent identity of this declared dep file.
    fn identity(&self) -> buck2_error::Result<DeclaredDepFileIdentity> {
        let (base, projected) = self.output.as_parts();
        match base {
            BaseArtifactKind::Build(b) => {
                let path = b.get_path();
                Ok(DeclaredDepFileIdentity {
                    label: self.label.dupe(),
                    path: path.path().to_owned(),
                    projected: projected.to_owned(),
                    is_content_based: path.is_content_based_path(),
                })
            }
            BaseArtifactKind::Source(_) => Err(internal_error!(
                "dep-file output must be a build artifact, not a source artifact"
            )),
        }
    }
}

/// All the dep files declared by a command;
#[derive(Default, Debug, Allocative)]
pub(crate) struct DeclaredDepFiles {
    tagged: BuckFixedInsertionMap<ArtifactTag, DeclaredDepFile>,
}

impl DeclaredDepFiles {
    pub(crate) fn is_empty(&self) -> bool {
        self.tagged.is_empty()
    }

    /// Given an ActionOutputs, materialize this set of dep files, so that we may read them later.
    /// The returned lease covers that read.
    async fn materialize(
        &self,
        fs: &ArtifactFs,
        materializer: &dyn Materializer,
        result: &ActionOutputs,
        re_use_case: RemoteExecutorUseCase,
    ) -> Result<ReadLease, MaterializeDepFilesError> {
        let mut artifacts = Vec::with_capacity(self.tagged.len());

        for declared_dep_file in self.tagged.values() {
            let dep_file = &declared_dep_file.output;
            let value = result
                .get_from_artifact_path(&dep_file.get_path())
                .internal_error("declared dep file must be one of the ActionOutputs")
                .map_err(|e| MaterializeDepFilesError::MaterializationFailed { source: e })?;
            let content_hash = if dep_file.path_resolution_requires_artifact_value() {
                Some(value.content_based_path_hash())
            } else {
                None
            };
            let path = dep_file
                .resolve_path(fs, content_hash.as_ref())
                .map_err(|e| MaterializeDepFilesError::MaterializationFailed { source: e })?;
            artifacts.push((path, value.dupe()));
        }

        if artifacts.is_empty() {
            return Ok(ReadLease::noop());
        }

        let response = materializer
            .materialize(MaterializeRequest {
                artifacts,
                outputs: Vec::new(),
                purpose: MaterializationPurpose::IntermediateOnly,
                re_use_case,
            })
            .await
            .map_err(|e| MaterializeDepFilesError::MaterializationFailed { source: e })?;

        let mut has_not_found = false;
        for dep_file in response.results {
            match dep_file {
                Ok(()) => {}
                Err(MaterializationError::NotFound { .. }) => {
                    has_not_found = true;
                }
                Err(e) => {
                    return Err(MaterializeDepFilesError::MaterializationFailed {
                        source: e.into(),
                    });
                }
            }
        }

        if has_not_found {
            Err(MaterializeDepFilesError::NotFound)
        } else {
            Ok(response.lease)
        }
    }

    /// Read this set of dep files, producing ConcreteDepFiles. This can then be used to compute
    /// signatures for the input set that was used, and for future input sets.
    fn read(
        &self,
        fs: &ArtifactFs,
        result: &ActionOutputs,
    ) -> buck2_error::Result<Option<ConcreteDepFiles>> {
        let mut contents =
            BuckMutMap::with_capacity_and_hasher(self.tagged.len(), Default::default());

        for declared_dep_file in self.tagged.values() {
            let content_hash = if declared_dep_file
                .output
                .path_resolution_requires_artifact_value()
            {
                Some(
                    result
                        .get_from_artifact_path(&declared_dep_file.output.get_path())
                        .expect("declared dep file must be one of the ActionOutputs!")
                        .content_based_path_hash(),
                )
            } else {
                None
            };
            let dep_file = declared_dep_file
                .output
                .resolve_path(fs, content_hash.as_ref())?;

            let read_dep_file: buck2_error::Result<()> = try {
                let dep_file_path = fs.fs().resolve(&dep_file);
                let dep_file = fs_util::read_to_string_if_exists(&dep_file_path)?;

                let dep_file = match dep_file {
                    Some(dep_file) => dep_file,
                    None => {
                        soft_error!(
                            "missing_dep_file",
                            buck2_error::buck2_error!(
                                buck2_error::ErrorTag::Input,
                                "Dep file is missing at {}",
                                dep_file_path
                            )
                        );
                        return Ok(None);
                    }
                };

                contents.insert(declared_dep_file.label.dupe(), dep_file);
            };

            read_dep_file.with_buck_error_context(|| {
                format!(
                    "Action execution produced an invalid `{}` dep file at `{}`",
                    declared_dep_file.label, dep_file,
                )
            })?;
        }

        Ok(Some(ConcreteDepFiles { contents }))
    }

    /// The set of configuration-independent identities of the dep files this action declares.
    ///
    /// Two actions declare the same dep files iff their identity sets are equal (`==`). An identity
    /// is the dep-file label, target-relative output path, projected path, and whether the path is
    /// content-based; the tag identity and the configuration are intentionally ignored. Because
    /// cached state is looked up by a key that already fixes the (unconfigured) target, comparing
    /// these identities is sufficient to reuse dep files (or the whole action) from a previous
    /// invocation, possibly under a different configuration or from a previous daemon session.
    fn identities(&self) -> buck2_error::Result<BuckMutSet<DeclaredDepFileIdentity>> {
        self.tagged
            .values()
            .map(DeclaredDepFile::identity)
            .collect()
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum MaterializeDepFilesError {
    #[error("Error materializing dep file")]
    MaterializationFailed {
        #[source]
        source: buck2_error::Error,
    },

    #[error("A dep file was not found")]
    NotFound,
}

/// A set of concrete dep files. That is, given a label, a string that represents the
/// content of the corresponding dep file.
#[derive(Clone)]
pub(crate) struct ConcreteDepFiles {
    contents: BuckMutMap<Arc<str>, String>,
}

impl ConcreteDepFiles {
    fn get_selector(
        &self,
        label: &Arc<str>,
        fs: &ArtifactFs,
        builder: &ActionDirectoryBuilder,
    ) -> buck2_error::Result<Option<DirectorySelector>> {
        let dep_file = match self.contents.get(label) {
            Some(dep_file) => dep_file,
            None => return Ok(None),
        };

        let mut selector = DirectorySelector::empty();
        for line in dep_file.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let path = ForwardRelativePathNormalizer::normalize_path(line)
                .buck_error_context("Invalid line encountered in dep file")?;

            if let Err(e) = Self::add_path_to_selector(path, &mut selector, fs, builder) {
                soft_error!("failed_to_add_dep_file_path_to_selector", e);
                return Ok(None);
            }
        }

        Ok(Some(selector))
    }

    /// If dep-files contain content-based paths, then the paths in our input directory might not
    /// match the paths that were written to the dep-file in the previous invocation of the action.
    ///
    /// Therefore, if we have a path with a content hash, we look in the input directory for all
    /// possible content hashes, and construct paths based on those hashes instead.
    ///
    /// This does mean that we might over-approximate on the tagged input files, since multiple paths
    /// can be added in place of a single entry in the dep-file, but (1) it's unlikely to happen
    /// anyway and (2) the multiple files with conflicting paths are likely to change together anyway,
    /// so this doesn't seem like a big issue.
    fn add_path_to_selector(
        path: Cow<'_, ForwardRelativePath>,
        selector: &mut DirectorySelector,
        fs: &ArtifactFs,
        builder: &ActionDirectoryBuilder,
    ) -> buck2_error::Result<()> {
        if !path.starts_with(fs.buck_out_path_resolver().root()) {
            // This path isn't in buck-out, no content-based hash to replace.
            selector.select(path.as_ref());
            return Ok(());
        }

        let mut before_content_hash_parts = vec![];
        let mut path_iter = path.as_ref().iter();
        // Paths always begin with "buck-out/<ISOLATION_DIR>/<gen or art, etc.>/<CELL>", so
        // we can skip the first 4 segments.
        for _ in 0..4 {
            if let Some(segment) = path_iter.next() {
                before_content_hash_parts.push(segment);
            }
        }

        // If the path uses a configuration-based hash, it will come next.
        let potential_configuration_hash = path_iter.next();
        let should_look_for_content_based_hash = match potential_configuration_hash {
            Some(s) => {
                before_content_hash_parts.push(s);
                !is_hash(s.as_str())
            }
            None => false,
        };

        if !should_look_for_content_based_hash {
            selector.select(path.as_ref());
            return Ok(());
        }

        for segment in &mut path_iter {
            if !is_hash(segment.as_str()) {
                before_content_hash_parts.push(segment);
            } else {
                // We found a content-based hash.
                let dir_in_builder = find(builder.as_ref(), before_content_hash_parts.clone())?;
                let after = path_iter.as_path();
                for after_segment in after.iter() {
                    if is_hash(after_segment.as_str()) {
                        return Err(buck2_error::internal_error!(
                            "Path {} cannot be normalized for dep-files because it has two path segments that look like a content-based hash!",
                            path,
                        ));
                    }
                }
                if let Some(dir_in_builder) = dir_in_builder {
                    match dir_in_builder {
                        DirectoryEntry::Dir(d) => {
                            for (name, _entry) in d.entries() {
                                if is_hash(name.as_str()) {
                                    let full_path: ForwardRelativePathBuf =
                                        before_content_hash_parts
                                            .iter()
                                            .copied()
                                            .chain(std::iter::once(name))
                                            .chain(after.iter())
                                            .collect();
                                    selector.select(&full_path);
                                }
                            }
                        }
                        DirectoryEntry::Leaf(..) => {
                            return Err(buck2_error::internal_error!(
                                "Found content-based hash {} in path {} that was a leaf in the input directory!",
                                segment,
                                path,
                            ));
                        }
                    }
                }
                return Ok(());
            }
        }

        selector.select(path.as_ref());
        Ok(())
    }
}

/// A command line visitor to collect inputs and outputs in a form relevant for dep files
/// computations.
pub(crate) struct DepFilesCommandLineVisitor<'a> {
    pub(crate) inputs: PartitionedInputsBuilder<Vec<ArtifactGroup>>,
    pub(crate) tagged_outputs: BuckMutInsertionMap<ArtifactTag, (Arc<str>, Option<Artifact>)>,
    dep_files: &'a RunActionDepFiles,
}

impl<'a> DepFilesCommandLineVisitor<'a> {
    pub(crate) fn new(dep_files: &'a RunActionDepFiles) -> Self {
        // Prepopulate inputs & outputs to maintain the ordering of the labels declaration.

        let tagged_inputs: BuckMutInsertionMap<Arc<str>, Vec<ArtifactGroup>> = dep_files
            .labels
            .iter()
            .map(|(_tag, label)| (label.dupe(), Default::default()))
            .collect();

        let tagged_outputs: BuckMutInsertionMap<ArtifactTag, (Arc<str>, Option<Artifact>)> =
            dep_files
                .labels
                .iter()
                .map(|(tag, label)| (tag.dupe(), (label.dupe(), None)))
                .collect();
        Self {
            inputs: PartitionedInputsBuilder {
                untagged: Default::default(),
                tagged: tagged_inputs,
            },
            tagged_outputs,
            dep_files,
        }
    }

    pub fn inputs(&self) -> impl Iterator<Item = &ArtifactGroup> {
        self.inputs.iter().flat_map(|g| g.iter())
    }
}

impl<'v> CommandLineArtifactVisitor<'v> for DepFilesCommandLineVisitor<'_> {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        _props: InputProperties,
        tags: Vec<&ArtifactTag>,
    ) {
        self.inputs.visit_input(input, tags, self.dep_files);
    }

    fn visit_declared_output(&mut self, _artifact: OutputArtifact<'v>, _tags: Vec<&ArtifactTag>) {
        unreachable!("Dep files visitors are run after freezing")
    }

    fn visit_frozen_output(&mut self, artifact: Artifact, tags: Vec<&ArtifactTag>) {
        for tag in tags {
            // NOTE: We have validated tags earlier, so we know that if a tag does not point to
            // a dep file here, it's safe to ignore it. We also know that we'll have exactly 1
            // dep file per tag, but one output may carry several dep-file tags: record it for
            // each of them so that every label has its dep file.
            if let Some((_label, output)) = self.tagged_outputs.get_mut(tag) {
                // NOTE: analysis has been done so we know inputs are bound now.
                *output = Some(artifact.dupe());
            }
        }
    }
}

fn is_hash(s: &str) -> bool {
    if s.len() != 16 {
        return false;
    }

    for c in s.chars() {
        if !c.is_ascii_hexdigit() {
            return false;
        }
    }

    true
}

fn rename_hash_dirs(name: &FileName) -> Option<FileNameBuf> {
    if is_hash(name.as_str()) {
        Some(FileNameBuf::unchecked_new(
            ContentBasedPathHash::DepFilesPlaceholder.as_str(),
        ))
    } else {
        None
    }
}

#[derive(Allocative)]
#[repr(transparent)]
pub struct TaggedInputsDirectorySerializer {
    pub cas_digest_config: CasDigestConfig,
}

impl DirectoryDigester<ActionDirectoryMember, TrackedFileDigest>
    for TaggedInputsDirectorySerializer
{
    fn hash_entries<'a, D, I>(&self, entries: I) -> TrackedFileDigest
    where
        I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a ActionDirectoryMember>)>,
        D: ActionFingerprintedDirectoryRef<'a>,
    {
        TrackedFileDigest::from_content(
            &ReDirectorySerializer::rename_and_serialize_entries(entries, &rename_hash_dirs),
            self.cas_digest_config,
        )
    }

    fn leaf_size(&self, leaf: &ActionDirectoryMember) -> u64 {
        match leaf {
            ActionDirectoryMember::File(f) => f.digest.size(),
            ActionDirectoryMember::Symlink(_) => 0,
            ActionDirectoryMember::ExternalSymlink(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_artifact::actions::key::ActionIndex;
    use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
    use buck2_build_api::actions::impls::expanded_command_line::ExpandedCommandLineFingerprinter;
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::Symlink;
    use buck2_core::category::Category;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_execute::directory::extract_artifact_value;
    use buck2_execute::directory::insert_entry;
    use buck2_execute::directory::insert_file;

    use super::*;

    #[test]
    fn dep_file_caches_are_independent() {
        let first = new_dep_file_cache();
        let second = new_dep_file_cache();
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());
        let key = LogicalActionKey::Configured {
            target: target.unconfigured().dupe(),
            category: Category::new("test".to_owned()).expect("test category should be valid"),
            identifier: None,
        };
        let state = Arc::new(dep_file_state_with(
            ActionOutputs::new(BuckMutInsertionMap::new()),
            DigestConfig::testing_default(),
        ));

        dep_files(first.as_ref()).insert(key.dupe(), None, state);

        assert!(dep_files(first.as_ref()).get(&key, None).is_some());
        assert!(dep_files(second.as_ref()).get(&key, None).is_none());
    }

    #[test]
    fn test_dep_files_visitor_output_collection() {
        let tag1 = ArtifactTag::testing_new();
        let tag2 = ArtifactTag::testing_new();
        let tag3 = ArtifactTag::testing_new();
        let tag4 = ArtifactTag::testing_new();

        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());
        let artifact1 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar1.h",
            ActionIndex::new(0),
        ));
        let artifact2 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar2.h",
            ActionIndex::new(0),
        ));
        let artifact3 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar3.h",
            ActionIndex::new(0),
        ));
        let artifact4 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar4.h",
            ActionIndex::new(0),
        ));
        let artifact5 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar5.h",
            ActionIndex::new(0),
        ));

        let dep_files = RunActionDepFiles {
            labels: BuckFixedInsertionMap::from_iter([
                (tag1.dupe(), Arc::from("l1")),
                (tag2.dupe(), Arc::from("l2")),
                (tag3.dupe(), Arc::from("l3")),
            ]),
        };

        let mut visitor = DepFilesCommandLineVisitor::new(&dep_files);
        visitor.visit_frozen_output(artifact3, vec![&tag3]);
        visitor.visit_frozen_output(artifact2, vec![&tag2]);
        visitor.visit_frozen_output(artifact1, vec![&tag1]);
        // This should be ignored as it's not included in RunActionDepFiles
        visitor.visit_frozen_output(artifact4, vec![&tag4]);
        // This should be ignored as it does not have a tag
        visitor.visit_frozen_output(artifact5, vec![]);

        // Assert that the order is preserved between the two maps
        let x: Vec<_> = visitor.tagged_outputs.keys().collect();
        let y: Vec<_> = dep_files.labels.keys().collect();
        assert_eq!(x, y);
    }

    #[test]
    fn test_declares_same_dep_files() -> buck2_error::Result<()> {
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());

        let artifact1 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar1.h",
            ActionIndex::new(0),
        ));

        let artifact2 = Artifact::from(BuildArtifact::testing_new(
            target.dupe(),
            "foo/bar2.h",
            ActionIndex::new(0),
        ));

        let depfile1 = DeclaredDepFile {
            label: Arc::from("foo"),
            output: artifact1,
        };

        let depfile2 = DeclaredDepFile {
            label: Arc::from("foo"),
            output: artifact2.dupe(),
        };

        let depfile3 = DeclaredDepFile {
            label: Arc::from("bar"),
            output: artifact2,
        };

        let tag1 = ArtifactTag::testing_new();

        let tag2 = ArtifactTag::testing_new();

        let decl1 = DeclaredDepFiles {
            tagged: BuckFixedInsertionMap::from_iter([(tag1.dupe(), depfile1.dupe())]),
        };

        let decl2 = DeclaredDepFiles {
            tagged: BuckFixedInsertionMap::from_iter([(tag2.dupe(), depfile1.dupe())]),
        };

        let decl3 = DeclaredDepFiles {
            tagged: BuckFixedInsertionMap::from_iter([(tag2.dupe(), depfile2.dupe())]),
        };

        let decl4 = DeclaredDepFiles {
            tagged: BuckFixedInsertionMap::from_iter([(tag2.dupe(), depfile3.dupe())]),
        };

        assert!(decl1.identities()? == decl1.identities()?);
        assert!(decl1.identities()? == decl2.identities()?);
        assert!(decl2.identities()? != decl3.identities()?);
        assert!(decl3.identities()? != decl4.identities()?);
        Ok(())
    }

    #[test]
    fn test_declares_same_dep_files_across_configurations() -> buck2_error::Result<()> {
        // The same dep file declared under two different configurations compares equal: this is the
        // loosening that lets dep-file state be reused across target configurations.
        let make = |cfg: ConfigurationData, path: &str| DeclaredDepFiles {
            tagged: BuckFixedInsertionMap::from_iter([(
                ArtifactTag::testing_new(),
                DeclaredDepFile {
                    label: Arc::from("foo"),
                    output: Artifact::from(BuildArtifact::testing_new(
                        ConfiguredTargetLabel::testing_parse("cell//pkg:foo", cfg),
                        path,
                        ActionIndex::new(0),
                    )),
                },
            )]),
        };

        let cfg_a = make(ConfigurationData::testing_new(), "foo/bar.h");
        let cfg_b = make(ConfigurationData::unbound(), "foo/bar.h");
        assert!(cfg_a.identities()? == cfg_b.identities()?);

        // A different output path is still not a match, even across configurations.
        let cfg_b_other = make(ConfigurationData::unbound(), "foo/other.h");
        assert!(cfg_a.identities()? != cfg_b_other.identities()?);
        Ok(())
    }

    /// Build an `ActionOutputs` holding a single output named `out` with `value`.
    fn outputs_with(value: ArtifactValue) -> ActionOutputs {
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());
        let artifact = BuildArtifact::testing_new(target, "out", ActionIndex::new(0));
        ActionOutputs::new(BuckMutInsertionMap::from_iter([(
            artifact.get_path().dupe(),
            value,
        )]))
    }

    fn dep_file_state_with(result: ActionOutputs, digest_config: DigestConfig) -> DepFileState {
        DepFileState {
            digests: CommandDigests {
                cli: ExpandedCommandLineFingerprinter::new().finalize(),
                directory: TrackedFileDigest::from_content(
                    b"dir",
                    digest_config.cas_digest_config(),
                )
                .data()
                .dupe(),
                local_worker_directory: None,
            },
            result,
            was_produced_locally: true,
            declared: DepFileDeclaration::None,
        }
    }

    #[test]
    fn test_to_stored_skips_outputs_with_deps() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        // An output that is a symlink pointing at another artifact: `extract_artifact_value`
        // records that artifact in `deps`.
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_file(
            &mut builder,
            ProjectRelativePathBuf::unchecked_new("target".to_owned()),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        insert_entry(
            &mut builder,
            ProjectRelativePathBuf::unchecked_new("out".to_owned()),
            DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(Arc::new(Symlink::new(
                "target".into(),
            )))),
        )?;
        let with_deps = extract_artifact_value(
            &builder,
            ProjectRelativePath::unchecked_new("out"),
            digest_config,
        )?
        .expect("`out` is in the builder");
        assert!(
            with_deps.deps().is_some(),
            "test setup: the symlink output should have deps"
        );

        // `deps` has nowhere to go in the database, so the whole entry is not persistable.
        let state = dep_file_state_with(outputs_with(with_deps), digest_config);
        assert!(state.to_stored()?.is_none());

        // The same action without a deps-bearing output is persisted as usual.
        let plain = ArtifactValue::file(FileMetadata::empty(digest_config.cas_digest_config()));
        assert!(plain.deps().is_none());
        let state = dep_file_state_with(outputs_with(plain), digest_config);
        assert!(state.to_stored()?.is_some());
        Ok(())
    }

    /// Analysis accepts one output tagged with two dep-file tags (each tag then has exactly one
    /// output). `visit_frozen_output` records the output for every matching tag, so both labels
    /// get their dep file.
    #[test]
    fn output_tagged_with_two_dep_file_tags_is_recorded_for_both() {
        let tag1 = ArtifactTag::testing_new();
        let tag2 = ArtifactTag::testing_new();
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());
        let out = Artifact::from(BuildArtifact::testing_new(
            target,
            "dep_file",
            ActionIndex::new(0),
        ));
        let dep_files = RunActionDepFiles {
            labels: BuckFixedInsertionMap::from_iter([
                (tag1.dupe(), Arc::from("l1")),
                (tag2.dupe(), Arc::from("l2")),
            ]),
        };
        let mut visitor = DepFilesCommandLineVisitor::new(&dep_files);
        visitor.visit_frozen_output(out.dupe(), vec![&tag1, &tag2]);

        assert_eq!(visitor.inputs.tagged.len(), 2);
        assert_eq!(
            visitor.tagged_outputs.get(&tag1).unwrap().1,
            Some(out.dupe())
        );
        assert_eq!(visitor.tagged_outputs.get(&tag2).unwrap().1, Some(out));
    }
}
