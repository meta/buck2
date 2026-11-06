/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::ops::ControlFlow;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::artifact_path::ArtifactPath;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_build_signals::env::WaitingData;
use buck2_common::dice::data::HasIoProvider;
use buck2_common::events::HasEvents;
use buck2_common::http::HasHttpClient;
use buck2_common::io::IoProvider;
use buck2_common::liveliness_observer::NoopLivelinessObserver;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::execution_types::executor_config::CommandExecutorConfig;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_data::SchedulingMode;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::dispatch::EventDispatcher;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::dep_file_state::DepFileStore;
use buck2_execute::dep_file_state::HasDepFileStore;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::execute::blocking::HasBlockingExecutor;
use buck2_execute::execute::cache_uploader::CacheUploadInfo;
use buck2_execute::execute::cache_uploader::CacheUploadResults;
use buck2_execute::execute::cache_uploader::IntoRemoteDepFile;
use buck2_execute::execute::claim::MutexClaimManager;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::execute::command_executor::CommandExecutor;
use buck2_execute::execute::dep_file_digest::DepFileDigest;
use buck2_execute::execute::kind::CommandExecutionKind;
use buck2_execute::execute::low_pass_filter::LowPassFilter;
use buck2_execute::execute::manager::CommandExecutionManager;
use buck2_execute::execute::prepared::PreparedAction;
use buck2_execute::execute::prepared::PreparedCommand;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::request::ExecutorPreference;
use buck2_execute::execute::result::CommandExecutionReport;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::execute::result::CommandExecutionStatus;
use buck2_execute::execute::write_action_cache::WriteActionCache;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::output_size::OutputCountAndBytes;
use buck2_execute::output_size::OutputSize;
use buck2_execute::re::invocation_re_settings::HasInvocationReSettings;
use buck2_execute::re::invocation_re_settings::InvocationReSettings;
use buck2_execute::re::manager::UnconfiguredRemoteExecutionClient;
use buck2_execute::re::output_trees_download_config::OutputTreesDownloadConfig;
use buck2_file_watcher::dep_files::DepFileCache;
use buck2_file_watcher::dep_files::HasDepFileCache;
use buck2_file_watcher::mergebase::GetMergebase;
use buck2_file_watcher::mergebase::Mergebase;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_hash::buck_mut_insertion_map;
use buck2_http::HttpClient;
use buck2_util::strong_hasher::Blake3StrongHasher;
use derive_more::Display;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use either::Either;
use itertools::Itertools;
use mini_vec::MiniBoxSlice;
use pagable::DeferredValue;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use ref_cast::RefCastCustom;
use ref_cast::ref_cast_custom;
use remote_execution::TActionResult2;
use starlark::pagable::DeferredFieldReadsEnabled;
use strong_hash::StrongHash;

use crate::actions::ActionExecutionCtx;
use crate::actions::RegisteredAction;
use crate::actions::artifact::get_artifact_fs::GetArtifactFs;
use crate::actions::errors::execute_error::ExecuteError;
use crate::actions::execute::action_execution_target::ActionExecutionTarget;
use crate::actions::execute::dice_data::CommandExecutorResponse;
use crate::actions::execute::dice_data::DiceHasCommandExecutor;
use crate::actions::execute::dice_data::GetInvalidationTrackingConfig;
use crate::actions::execute::dice_data::GetReClient;
use crate::actions::execute::dice_data::HasLowPassFilter;
use crate::actions::impls::run_action_knobs::HasRunActionKnobs;
use crate::actions::impls::run_action_knobs::RunActionKnobs;
use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::ArtifactGroupValues;

/// This is the result of the action as exposed to other things in the dice computation.
#[derive(
    Clone,
    Dupe,
    Debug,
    PartialEq,
    Eq,
    Allocative,
    pagable::Pagable,
    RefCastCustom
)]
#[repr(transparent)]
pub struct ActionOutputs(Arc<ActionOutputsData>);

/// The DICE value exposes equality without demanding large output graphs. Actual
/// artifact consumers use `outputs`, which can fail when paged data is unreadable.
#[derive(Debug, Allocative)]
pub struct DiceActionOutputs(DeferredValue<Arc<ActionOutputsData>, [u8; 32]>);

// Computed values must stay as compact as Result<ActionOutputs>; only restored
// values allocate a deferred cell and retain a separate equality fingerprint.
buck2_util::size_assert::words_of_type!(buck2_error::Result<DiceActionOutputs>, 2);

impl From<ActionOutputs> for DiceActionOutputs {
    fn from(outputs: ActionOutputs) -> Self {
        Self(DeferredValue::new(outputs.0))
    }
}

impl DiceActionOutputs {
    pub(crate) fn fingerprint(&self) -> &[u8; 32] {
        self.0
            .metadata_or_else(|outputs| outputs.fingerprint.as_bytes())
    }

    /// Resolve the stored output map once; equality checks do not call this.
    pub fn outputs(&self) -> buck2_error::Result<&ActionOutputs> {
        self.0
            .read()
            .map(ActionOutputs::ref_cast)
            .map_err(|error| from_any_with_tag(error, ErrorTag::Environment))
            .buck_error_context("Failed to restore action outputs")
    }
}

impl PagableSerialize for DiceActionOutputs {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        let outputs = self.0.read()?;
        // Bounded leaf results are cheaper to restore directly: a deferred
        // cell and a second blocking read otherwise cost more than they save.
        // Even one directory or symlink dependency can own an unbounded tree.
        let defer = match &outputs.outputs {
            ActionOutputsMap::Small(entries) => entries
                .iter()
                .any(|(_, value)| value.is_dir() || value.deps().is_some()),
            ActionOutputsMap::Large(_) => true,
        };
        defer.pagable_serialize(serializer)?;
        if defer {
            outputs.fingerprint.pagable_serialize(serializer)?;
        }
        outputs.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for DiceActionOutputs {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        if !bool::pagable_deserialize(deserializer)? {
            return Ok(ActionOutputs::pagable_deserialize(deserializer)?.into());
        }
        let fingerprint = blake3::Hash::pagable_deserialize(deserializer)?;
        let enabled = deserializer
            .storage_context()
            .get::<DeferredFieldReadsEnabled>()
            .is_some();
        Ok(Self(DeferredValue::deserialize_arc_with_metadata(
            deserializer,
            enabled,
            *fingerprint.as_bytes(),
        )?))
    }
}

impl OutputSize for ActionOutputs {
    fn calc_output_count_and_bytes(&self, include_symlinks: bool) -> OutputCountAndBytes {
        let mut total_count = 0;
        let mut total_bytes = 0;
        for v in self.values() {
            let count_and_bytes = v.calc_output_count_and_bytes(include_symlinks);
            total_count += count_and_bytes.count;
            total_bytes += count_and_bytes.bytes;
        }
        OutputCountAndBytes {
            count: total_count,
            bytes: total_bytes,
        }
    }
}

#[derive(Debug, Allocative, pagable::Pagable)]
struct ActionOutputsData {
    outputs: ActionOutputsMap,
    /// Strong hash of `outputs`, computed at construction; see `PartialEq`
    /// below.
    // This is OK to skip because the hash is stored inline.
    #[allocative(skip)]
    fingerprint: blake3::Hash,
}

/// Most actions have a handful of outputs, where an exact-sized slice with
/// linear lookups beats the index map's two allocations plus bucket slack.
/// Big producers (dist-ThinLTO index actions emit tens of thousands) keep
/// the map so `get` stays O(1). Both preserve insertion order, which the
/// fingerprint depends on.
#[derive(Debug, Allocative, pagable::Pagable)]
enum ActionOutputsMap {
    Small(MiniBoxSlice<(BuildArtifactPath, ArtifactValue)>),
    Large(BuckMutInsertionMap<BuildArtifactPath, ArtifactValue>),
}

// One of these is retained per executed action; with the 32-byte fingerprint
// and the Arc header this is the 64-byte jemalloc bin.
buck2_util::size_assert::words_of_type!(ActionOutputsMap, 2);

/// At 8 entries a linear scan is still cheap next to the map's hashing,
/// and beyond that the allocation-shape win stops mattering.
const SMALL_OUTPUTS_MAX: usize = 8;

impl ActionOutputsMap {
    fn new(outputs: BuckMutInsertionMap<BuildArtifactPath, ArtifactValue>) -> Self {
        if outputs.len() <= SMALL_OUTPUTS_MAX {
            ActionOutputsMap::Small(MiniBoxSlice::from_iter(outputs))
        } else {
            ActionOutputsMap::Large(outputs)
        }
    }

    fn get(&self, artifact: &BuildArtifactPath) -> Option<&ArtifactValue> {
        match self {
            ActionOutputsMap::Small(entries) => entries
                .iter()
                .find(|(path, _)| path == artifact)
                .map(|(_, value)| value),
            ActionOutputsMap::Large(map) => map.get(artifact),
        }
    }

    fn contains_key(&self, artifact: &BuildArtifactPath) -> bool {
        self.get(artifact).is_some()
    }

    fn iter(&self) -> impl Iterator<Item = (&BuildArtifactPath, &ArtifactValue)> {
        match self {
            ActionOutputsMap::Small(entries) => {
                Either::Left(entries.iter().map(|(path, value)| (path, value)))
            }
            ActionOutputsMap::Large(map) => Either::Right(map.iter()),
        }
    }

    fn keys(&self) -> impl Iterator<Item = &BuildArtifactPath> {
        self.iter().map(|(path, _)| path)
    }

    fn values(&self) -> impl Iterator<Item = &ArtifactValue> {
        self.iter().map(|(_, value)| value)
    }
}

/// Equality compares the fingerprint, making it O(1). That matters because
/// DICE compares `BuildKey` values on its serialized core state thread, and
/// these values can be both huge and widely shared: an action can have tens
/// of thousands of outputs (dist-ThinLTO index actions), and every artifact
/// re-bound through a `dynamic_output` gets an action key whose dice value is
/// the producing action's entire `ActionOutputs`.
///
/// The fingerprint hashes entries in iteration order, so two maps that are
/// equal as unordered maps may compare unequal here. `Key::equality` permits
/// that direction (it only costs extra invalidation); the opposite direction
/// rests on blake3 collision resistance.
impl PartialEq for ActionOutputsData {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
    }
}

impl Eq for ActionOutputsData {}

/// Metadata associated with the execution of this action.
#[derive(Debug)]
pub struct ActionExecutionMetadata {
    pub execution_kind: ActionExecutionKind,
    pub timing: ActionExecutionTimingData,
    pub input_files_bytes: Option<u64>,
    pub waiting_data: WaitingData,
    /// Dep-file cache entries this action queued for persisting.
    pub dep_file_db_writes_queued: u64,
}

/// The *way* that a particular action was executed.
#[derive(Debug, Display, Clone)]
pub enum ActionExecutionKind {
    #[display("command({})", kind)]
    Command {
        kind: Box<CommandExecutionKind>,
        prefers_local: bool,
        requires_local: bool,
        allows_cache_upload: bool,
        cache_upload_result: buck2_data::UploadResult,
        allows_dep_file_cache_upload: bool,
        dep_file_cache_upload_result: buck2_data::UploadResult,
        eligible_for_full_hybrid: bool,
        dep_file_key: Option<DepFileDigest>,
        scheduling_mode: Option<SchedulingMode>,
        incremental_kind: buck2_data::IncrementalKind,
    },
    /// This action is simple and executed inline within buck2 (e.g. write, symlink_dir)
    #[display("simple")]
    Simple,
    /// This action logically executed, but didn't do all the work.
    #[display("deferred")]
    Deferred,
    /// This action was served by the local dep file cache and not executed.
    #[display("local_dep_files")]
    LocalDepFile,

    /// This action was served by the local action cache and not executed.
    #[display("local_action_cache")]
    LocalActionCache,
}

pub struct CommandExecutionRef<'a> {
    pub kind: &'a CommandExecutionKind,
    pub prefers_local: bool,
    pub requires_local: bool,
    pub allows_cache_upload: bool,
    pub cache_upload_result: buck2_data::UploadResult,
    pub allows_dep_file_cache_upload: bool,
    pub dep_file_cache_upload_result: buck2_data::UploadResult,
    pub eligible_for_full_hybrid: bool,
    pub scheduling_mode: Option<SchedulingMode>,
    pub dep_file_key: &'a Option<DepFileDigest>,
    pub incremental_kind: buck2_data::IncrementalKind,
}

pub(crate) const fn did_upload_from_upload_result(upload_result: buck2_data::UploadResult) -> bool {
    matches!(upload_result, buck2_data::UploadResult::Uploaded)
}

impl CommandExecutionRef<'_> {
    pub fn did_cache_upload(&self) -> bool {
        did_upload_from_upload_result(self.cache_upload_result)
    }

    pub fn did_dep_file_cache_upload(&self) -> bool {
        did_upload_from_upload_result(self.dep_file_cache_upload_result)
    }
}

impl ActionExecutionKind {
    pub fn as_enum(&self) -> buck2_data::ActionExecutionKind {
        match self {
            ActionExecutionKind::Command { kind, .. } => kind.as_enum(),
            ActionExecutionKind::Simple => buck2_data::ActionExecutionKind::Simple,
            ActionExecutionKind::Deferred => buck2_data::ActionExecutionKind::Deferred,
            ActionExecutionKind::LocalDepFile => buck2_data::ActionExecutionKind::LocalDepFile,
            ActionExecutionKind::LocalActionCache => {
                buck2_data::ActionExecutionKind::LocalActionCache
            }
        }
    }

    pub fn command(&self) -> Option<CommandExecutionRef<'_>> {
        match self {
            Self::Command {
                kind,
                prefers_local,
                requires_local,
                allows_cache_upload,
                cache_upload_result,
                allows_dep_file_cache_upload,
                dep_file_cache_upload_result,
                dep_file_key,
                eligible_for_full_hybrid,
                scheduling_mode,
                incremental_kind,
                ..
            } => Some(CommandExecutionRef {
                kind,
                prefers_local: *prefers_local,
                requires_local: *requires_local,
                allows_cache_upload: *allows_cache_upload,
                cache_upload_result: *cache_upload_result,
                allows_dep_file_cache_upload: *allows_dep_file_cache_upload,
                dep_file_cache_upload_result: *dep_file_cache_upload_result,
                dep_file_key,
                eligible_for_full_hybrid: *eligible_for_full_hybrid,
                scheduling_mode: *scheduling_mode.dupe(),
                incremental_kind: *incremental_kind,
            }),
            Self::Simple | Self::Deferred | Self::LocalDepFile | Self::LocalActionCache => None,
        }
    }
}

impl ActionOutputs {
    #[ref_cast_custom]
    fn ref_cast(outputs: &Arc<ActionOutputsData>) -> &Self;

    pub fn new(outputs: BuckMutInsertionMap<BuildArtifactPath, ArtifactValue>) -> Self {
        let mut hasher = Blake3StrongHasher::new();
        outputs.len().strong_hash(&mut hasher);
        for (path, value) in &outputs {
            path.strong_hash(&mut hasher);
            value.strong_hash(&mut hasher);
        }
        let fingerprint = hasher.finalize();
        Self(Arc::new(ActionOutputsData {
            outputs: ActionOutputsMap::new(outputs),
            fingerprint,
        }))
    }

    pub fn from_single(artifact: BuildArtifactPath, value: ArtifactValue) -> Self {
        Self::new(buck_mut_insertion_map! {artifact => value})
    }

    pub fn get(&self, artifact: &BuildArtifactPath) -> Option<&ArtifactValue> {
        self.0.outputs.get(artifact)
    }

    pub fn get_from_artifact_path(&self, path: &ArtifactPath) -> Option<&ArtifactValue> {
        match path.base_path.as_ref() {
            Either::Left(base) => self.get(base),
            Either::Right(_) => None,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BuildArtifactPath, &ArtifactValue)> {
        self.0.outputs.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &ArtifactValue> {
        self.0.outputs.values()
    }
}

#[async_trait]
pub trait HasActionExecutor<'d> {
    async fn get_action_executor(
        &mut self,
        config: &CommandExecutorConfig,
    ) -> buck2_error::Result<BuckActionExecutor<'d>>;
}

#[async_trait]
impl<'d> HasActionExecutor<'d> for DiceComputations<'d> {
    async fn get_action_executor(
        &mut self,
        executor_config: &CommandExecutorConfig,
    ) -> buck2_error::Result<BuckActionExecutor<'d>> {
        let artifact_fs = self.get_artifact_fs().await?;
        let digest_config = self.global_data().get_digest_config();

        let CommandExecutorResponse {
            executor,
            platform,
            action_cache_checker,
            remote_dep_file_cache_checker,
            cache_uploader,
            output_trees_download_config,
            write_action_cache,
        } = self.get_command_executor_from_dice(artifact_fs, executor_config)?;
        let blocking_executor = self.get_blocking_executor();
        let materializer = self.per_transaction_data().get_materializer();
        let events = self.per_transaction_data().get_dispatcher();
        let re_client = self.per_transaction_data().get_re_client();
        let run_action_knobs = self.per_transaction_data().get_run_action_knobs();
        let low_pass_filter = self.per_transaction_data().get_low_pass_filter();
        let dep_file_cache = self.per_transaction_data().get_dep_file_cache();
        let dep_file_store = self.per_transaction_data().get_dep_file_store();
        let io_provider = self.global_data().get_io_provider();
        let http_client = self.per_transaction_data().get_http_client();
        let mergebase = self.per_transaction_data().get_mergebase();
        let invalidation_tracking_enabled = self.get_invalidation_tracking_config().enabled;
        let invocation_re_settings = self.per_transaction_data().get_invocation_re_settings();

        Ok(BuckActionExecutor::new(
            CommandExecutor::new(
                executor,
                action_cache_checker,
                remote_dep_file_cache_checker,
                cache_uploader,
                write_action_cache,
                artifact_fs.dupe(),
                executor_config.options,
                platform,
            ),
            blocking_executor,
            materializer,
            events,
            re_client,
            digest_config,
            run_action_knobs,
            low_pass_filter,
            dep_file_cache,
            dep_file_store,
            io_provider,
            http_client,
            mergebase,
            invalidation_tracking_enabled,
            output_trees_download_config,
            invocation_re_settings,
        )
        .with_executor_config(executor_config, self.get_command_options_fingerprint()?))
    }
}

pub struct BuckActionExecutor<'d> {
    command_executor: CommandExecutor,
    blocking_executor: &'d dyn BlockingExecutor,
    materializer: &'d dyn Materializer,
    events: &'d EventDispatcher,
    re_client: &'d UnconfiguredRemoteExecutionClient,
    digest_config: DigestConfig,
    run_action_knobs: &'d RunActionKnobs,
    low_pass_filter: &'d LowPassFilter,
    dep_file_cache: &'d dyn DepFileCache,
    dep_file_store: Option<&'d dyn DepFileStore>,
    io_provider: &'d dyn IoProvider,
    http_client: &'d HttpClient,
    mergebase: &'d Mergebase,
    invalidation_tracking_enabled: bool,
    output_trees_download_config: OutputTreesDownloadConfig,
    invocation_re_settings: InvocationReSettings,
    /// Hash of the `CommandExecutorConfig` this executor was built from. Zero when unknown.
    executor_config_fingerprint: u64,
    /// `HasCommandExecutor::command_options_fingerprint` of the command this executor runs for.
    command_options_fingerprint: u64,
}

impl<'d> BuckActionExecutor<'d> {
    pub fn new(
        command_executor: CommandExecutor,
        blocking_executor: &'d dyn BlockingExecutor,
        materializer: &'d dyn Materializer,
        events: &'d EventDispatcher,
        re_client: &'d UnconfiguredRemoteExecutionClient,
        digest_config: DigestConfig,
        run_action_knobs: &'d RunActionKnobs,
        low_pass_filter: &'d LowPassFilter,
        dep_file_cache: &'d dyn DepFileCache,
        dep_file_store: Option<&'d dyn DepFileStore>,
        io_provider: &'d dyn IoProvider,
        http_client: &'d HttpClient,
        mergebase: &'d Mergebase,
        invalidation_tracking_enabled: bool,
        output_trees_download_config: OutputTreesDownloadConfig,
        invocation_re_settings: InvocationReSettings,
    ) -> Self {
        BuckActionExecutor {
            command_executor,
            blocking_executor,
            materializer,
            events,
            re_client,
            digest_config,
            run_action_knobs,
            low_pass_filter,
            dep_file_cache,
            dep_file_store,
            io_provider,
            http_client,
            mergebase,
            invalidation_tracking_enabled,
            output_trees_download_config,
            invocation_re_settings,
            executor_config_fingerprint: 0,
            command_options_fingerprint: 0,
        }
    }

    /// Records which `CommandExecutorConfig` this executor runs, and under which command options
    /// (`HasCommandExecutor::command_options_fingerprint`), so that actions can tell executions
    /// under different executors or commands apart.
    pub fn with_executor_config(
        mut self,
        config: &CommandExecutorConfig,
        command_options_fingerprint: u64,
    ) -> Self {
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(config, &mut hasher);
        self.executor_config_fingerprint = std::hash::Hasher::finish(&hasher);
        self.command_options_fingerprint = command_options_fingerprint;
        self
    }
}

struct BuckActionExecutionContext<'a, 'd> {
    executor: &'a BuckActionExecutor<'d>,
    action: &'a RegisteredAction,
    inputs: BuckMutInsertionMap<ArtifactGroup, ArtifactGroupValues>,
    command_reports: &'a mut Vec<CommandExecutionReport>,
    cancellations: &'a CancellationContext,
}

#[async_trait]
impl ActionExecutionCtx for BuckActionExecutionContext<'_, '_> {
    fn target(&self) -> ActionExecutionTarget<'_> {
        ActionExecutionTarget::new(self.action)
    }

    fn fs(&self) -> &ArtifactFs {
        self.executor.command_executor.fs()
    }

    fn executor_fs(&self) -> ExecutorFs<'_> {
        self.executor.command_executor.executor_fs()
    }

    fn materializer(&self) -> &dyn Materializer {
        self.executor.materializer
    }

    fn events(&self) -> &EventDispatcher {
        &self.executor.events
    }

    fn command_execution_manager(&self, waiting_data: WaitingData) -> CommandExecutionManager {
        CommandExecutionManager::new(
            Box::new(MutexClaimManager::new()),
            self.executor.events.dupe(),
            NoopLivelinessObserver::create(),
            waiting_data,
        )
    }

    fn artifact_values(&self, artifact: &ArtifactGroup) -> &ArtifactGroupValues {
        self.inputs.get(artifact).unwrap_or_else(|| panic!("Internal error: action {} tried to grab the artifact {} even though it was not an input.", self.action.owner(), artifact))
    }

    fn artifact_path_mapping(
        &self,
        filter: Option<&BuckMutInsertionSet<ArtifactGroup>>,
    ) -> BuckMutMap<&Artifact, ContentBasedPathHash> {
        self.inputs
            .iter()
            .filter(|(ag, _)| {
                ag.path_resolution_may_require_artifact_value()
                    && filter.is_none_or(|filter| filter.contains(*ag))
            })
            .flat_map(|(_, v)| v.iter())
            .map(|(a, v)| (a, v.content_based_path_hash()))
            .collect()
    }

    fn artifact_path_mapping_with_path_only(
        &self,
        rendered: &BuckMutInsertionSet<ArtifactGroup>,
        path_only: &BuckMutInsertionSet<ArtifactGroup>,
    ) -> (
        BuckMutMap<&Artifact, ContentBasedPathHash>,
        BuckMutSet<&Artifact>,
    ) {
        let mut mapping = BuckMutMap::default();
        let mut path_only_artifacts = BuckMutSet::default();
        for (ag, values) in self.inputs.iter() {
            if !ag.path_resolution_may_require_artifact_value() {
                continue;
            }
            let is_path_only = path_only.contains(ag);
            if !is_path_only && !rendered.contains(ag) {
                continue;
            }
            for (artifact, value) in values.iter() {
                mapping.insert(artifact, value.content_based_path_hash());
                if is_path_only {
                    path_only_artifacts.insert(artifact);
                }
            }
        }
        (mapping, path_only_artifacts)
    }

    fn blocking_executor(&self) -> &dyn BlockingExecutor {
        self.executor.blocking_executor
    }

    fn re_client(&self) -> UnconfiguredRemoteExecutionClient {
        self.executor.re_client.dupe()
    }

    fn invocation_re_use_case(&self) -> RemoteExecutorUseCase {
        self.executor.invocation_re_settings.use_case
    }

    fn cas_configured(&self) -> bool {
        self.executor.invocation_re_settings.cas_configured
    }

    fn may_probe_cas(&self) -> bool {
        self.executor.invocation_re_settings.may_probe_cas
    }

    fn write_action_cache(&self) -> Option<&dyn WriteActionCache> {
        self.executor.command_executor.write_action_cache()
    }

    fn low_pass_filter(&self) -> &LowPassFilter {
        self.executor.low_pass_filter
    }

    fn re_platform(&self) -> &remote_execution::Platform {
        self.executor.command_executor.re_platform()
    }

    fn digest_config(&self) -> DigestConfig {
        self.executor.digest_config
    }

    fn run_action_knobs(&self) -> &RunActionKnobs {
        &self.executor.run_action_knobs
    }

    fn executor_config_fingerprint(&self) -> u64 {
        self.executor.executor_config_fingerprint
    }

    fn command_options_fingerprint(&self) -> u64 {
        self.executor.command_options_fingerprint
    }

    fn dep_file_store(&self) -> Option<&dyn DepFileStore> {
        self.executor.dep_file_store
    }

    fn dep_file_cache(&self) -> &dyn DepFileCache {
        self.executor.dep_file_cache
    }

    fn cancellation_context(&self) -> &CancellationContext {
        self.cancellations
    }

    fn mergebase(&self) -> &Mergebase {
        &self.executor.mergebase
    }

    fn prepare_action(
        &mut self,
        request: &CommandExecutionRequest,
        re_outputs_required: bool,
    ) -> buck2_error::Result<PreparedAction> {
        self.executor.command_executor.prepare_action(
            request,
            self.digest_config(),
            re_outputs_required,
        )
    }

    async fn action_cache(
        &mut self,
        manager: CommandExecutionManager,
        request: &CommandExecutionRequest,
        prepared_action: &PreparedAction,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManager> {
        let action = self.target();
        self.executor
            .command_executor
            .action_cache(
                manager,
                &PreparedCommand {
                    target: &action as _,
                    request,
                    prepared_action,
                    digest_config: self.digest_config(),
                },
                self.cancellations,
            )
            .await
    }

    async fn remote_dep_file_cache(
        &mut self,
        manager: CommandExecutionManager,
        request: &CommandExecutionRequest,
        prepared_action: &PreparedAction,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManager> {
        let action = self.target();
        self.executor
            .command_executor
            .remote_dep_file_cache(
                manager,
                &PreparedCommand {
                    target: &action as _,
                    request,
                    prepared_action,
                    digest_config: self.digest_config(),
                },
                self.cancellations,
            )
            .await
    }

    fn unpack_command_execution_result(
        &mut self,
        executor_preference: ExecutorPreference,
        result: CommandExecutionResult,
        allows_cache_upload: bool,
        allows_dep_file_cache_upload: bool,
        input_files_bytes: Option<u64>,
        incremental_kind: buck2_data::IncrementalKind,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let CommandExecutionResult {
            outputs,
            report,
            rejected_execution,
            cache_upload_result,
            dep_file_cache_upload_result,
            dep_file_key,
            eligible_for_full_hybrid,
            scheduling_mode,
            waiting_data,
            ..
        } = result;

        // TODO(T156483516): We should also validate that the outputs match the expected outputs
        let action_outputs = ActionOutputs::new(
            outputs
                .into_iter()
                .filter_map(|(output, value)| Some((output.build_artifact_path()?.dupe(), value)))
                .collect(),
        );

        // TODO (@torozco): The execution kind should be made to come via the command reports too.
        let res = match &report.status {
            CommandExecutionStatus::Success { execution_kind } => {
                let result = (
                    action_outputs,
                    ActionExecutionMetadata {
                        dep_file_db_writes_queued: 0,
                        execution_kind: ActionExecutionKind::Command {
                            kind: Box::new(execution_kind.clone()),
                            prefers_local: executor_preference.prefers_local(),
                            requires_local: executor_preference.requires_local(),
                            allows_cache_upload,
                            cache_upload_result,
                            allows_dep_file_cache_upload,
                            dep_file_cache_upload_result,
                            dep_file_key,
                            eligible_for_full_hybrid,
                            scheduling_mode,
                            incremental_kind,
                        },
                        timing: report.timing.into(),
                        input_files_bytes,
                        waiting_data,
                    },
                );
                Ok(result)
            }
            CommandExecutionStatus::Error { error, .. } => {
                Err(ExecuteError::CommandExecutionError {
                    action_outputs,
                    error: Some(error.clone()),
                })
            }
            _ => Err(ExecuteError::CommandExecutionError {
                action_outputs,
                error: None,
            }),
        };
        self.command_reports.extend(rejected_execution);
        self.command_reports.push(report);
        res
    }

    async fn exec_cmd(
        &mut self,
        manager: CommandExecutionManager,
        request: &CommandExecutionRequest,
        prepared_action: &PreparedAction,
    ) -> CommandExecutionResult {
        let action = self.target();
        self.executor
            .command_executor
            .exec_cmd(
                manager,
                &PreparedCommand {
                    target: &action as _,
                    request,
                    prepared_action,
                    digest_config: self.digest_config(),
                },
                self.cancellations,
            )
            .await
    }

    async fn cache_upload(
        &mut self,
        action_digest_and_blobs: &ActionDigestAndBlobs,
        execution_result: &CommandExecutionResult,
        re_result: Option<&mut TActionResult2>,
        dep_file_bundle: Option<&mut dyn IntoRemoteDepFile>,
        allow_cache_upload: bool,
    ) -> buck2_error::Result<CacheUploadResults> {
        let action = self.target();
        Ok(self
            .executor
            .command_executor
            .cache_upload(
                &CacheUploadInfo {
                    target: &action as _,
                    digest_config: self.digest_config(),
                    mergebase: self.mergebase().0.as_ref(),
                    re_platform: self.re_platform(),
                    allow_cache_upload,
                },
                execution_result,
                re_result,
                dep_file_bundle,
                action_digest_and_blobs,
            )
            .await?)
    }

    fn io_provider(&self) -> &dyn IoProvider {
        self.executor.io_provider
    }

    fn http_client(&self) -> &HttpClient {
        self.executor.http_client
    }

    fn output_trees_download_config(&self) -> &OutputTreesDownloadConfig {
        &self.executor.output_trees_download_config
    }
}

impl<'d> BuckActionExecutor<'d> {
    pub(crate) async fn execute(
        &self,
        waiting_data: WaitingData,
        inputs: BuckMutInsertionMap<ArtifactGroup, ArtifactGroupValues>,
        action: &RegisteredAction,
        cancellations: &CancellationContext,
    ) -> (
        Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError>,
        Vec<CommandExecutionReport>,
    ) {
        let mut command_reports = Vec::new();

        let res = async {
            let outputs = action.outputs();

            let mut ctx = BuckActionExecutionContext {
                executor: self,
                action,
                inputs,
                command_reports: &mut command_reports,
                cancellations,
            };

            let (result, metadata) = action.execute(&mut ctx, waiting_data).await?;

            // Check that all the outputs are the right output_type
            for x in outputs.iter() {
                let declared = x.output_type();
                // FIXME: One day we should treat FileOrDirectory as a File, and soft_error if it is a directory
                if declared != OutputType::FileOrDirectory {
                    if let Some(t) = result.0.outputs.get(x.get_path()) {
                        let real = if t.is_dir() {
                            OutputType::Directory
                        } else {
                            OutputType::File
                        };
                        if real != declared {
                            return Err(ExecuteError::WrongOutputType {
                                path: self.command_executor.fs().resolve_build(
                                    x.get_path(),
                                    Some(&t.content_based_path_hash()),
                                )?,
                                declared,
                                real,
                            });
                        }
                    }
                }
            }

            fn check_all_requested_outputs_returned_without_extra<'a>(
                outputs: &[BuildArtifact],
                result_outputs: impl IntoIterator<Item = &'a BuildArtifactPath>,
            ) -> bool {
                // Ignore ordering as outputs in original action might be ordered differently from
                // output paths in action result (they are sorted there).
                let result_output_paths: BuckMutSet<&BuildArtifactPath> =
                    result_outputs.into_iter().collect();
                let mut outputs_count = 0;
                for output in outputs.iter() {
                    outputs_count += 1;
                    let output_path = output.get_path();
                    if !result_output_paths.contains(output_path) {
                        return false;
                    }
                }
                outputs_count == result_output_paths.len()
            }

            // TODO (T122966509): Check projections here as well
            if !check_all_requested_outputs_returned_without_extra(
                &outputs,
                result.0.outputs.keys(),
            ) {
                let declared = outputs
                    .iter()
                    .filter(|x| !result.0.outputs.contains_key(x.get_path()))
                    .map(|x| {
                        self.command_executor.fs().resolve_build(
                            x.get_path(),
                            Some(&ContentBasedPathHash::for_output_artifact()),
                        )
                    })
                    .collect::<buck2_error::Result<_>>()?;
                let real = result
                    .0
                    .outputs
                    .keys()
                    .filter(|x| {
                        // This is error message, linear search is fine.
                        !outputs.iter().map(|b| b.get_path()).contains(x)
                    })
                    .map(|x| {
                        self.command_executor
                            .fs()
                            .resolve_build(x, Some(&ContentBasedPathHash::for_output_artifact()))
                    })
                    .collect::<buck2_error::Result<Vec<_>>>()?;
                if real.is_empty() {
                    Err(ExecuteError::MissingOutputs { declared })
                } else {
                    Err(ExecuteError::MismatchedOutputs { declared, real })
                }
            } else {
                Ok((result, metadata))
            }
        }
        .await;

        (res, command_reports)
    }

    pub fn invalidation_tracking_enabled(&self) -> bool {
        self.invalidation_tracking_enabled
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_execute::execute::clean_output_paths::cleanup_path;
    use buck2_fs::fs_util::uncategorized as fs_util;
    use dupe::Dupe;

    use super::*;

    #[test]
    fn test_action_outputs_map_small_large_boundary() {
        use buck2_common::cas_digest::CasDigestConfig;
        use buck2_common::file_ops::metadata::FileMetadata;
        use buck2_common::file_ops::metadata::TrackedFileDigest;
        use buck2_core::configuration::data::ConfigurationData;
        use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
        use buck2_core::fs::buck_out_path::BuckOutPathKind;
        use buck2_core::fs::buck_out_path::BuildArtifactPath;
        use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
        use buck2_execute::artifact_value::ArtifactValue;
        use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

        let owner = BaseDeferredKey::TargetLabel(ConfiguredTargetLabel::testing_parse(
            "cell//pkg:target",
            ConfigurationData::testing_new(),
        ));
        let config = CasDigestConfig::testing_default();
        let entry = |i: usize| {
            let path = BuildArtifactPath::new(
                owner.dupe(),
                ForwardRelativePathBuf::unchecked_new(format!("out/{i}")),
                BuckOutPathKind::Configuration,
            );
            let value = ArtifactValue::file(FileMetadata {
                digest: TrackedFileDigest::from_content(format!("{i}").as_bytes(), config),
                is_executable: false,
            });
            (path, value)
        };

        for (n, expect_small) in [
            (0, true),
            (1, true),
            (SMALL_OUTPUTS_MAX, true),
            (SMALL_OUTPUTS_MAX + 1, false),
        ] {
            let outputs: BuckMutInsertionMap<_, _> = (0..n).rev().map(entry).collect();
            let map = ActionOutputsMap::new(outputs.clone());
            assert_eq!(
                matches!(map, ActionOutputsMap::Small(_)),
                expect_small,
                "n = {n}"
            );
            for (key, value) in &outputs {
                assert_eq!(map.get(key), Some(value));
                assert!(map.contains_key(key));
            }
            let (missing, _) = entry(n);
            assert!(map.get(&missing).is_none());
            assert!(!map.contains_key(&missing));
            // Use reverse insertion order to catch accidental sorting.
            assert_eq!(
                map.iter().collect::<Vec<_>>(),
                outputs.iter().collect::<Vec<_>>()
            );
            assert_eq!(
                map.keys().collect::<Vec<_>>(),
                outputs.keys().collect::<Vec<_>>()
            );
            assert_eq!(
                map.values().collect::<Vec<_>>(),
                outputs.values().collect::<Vec<_>>()
            );

            let mut serializer = pagable::testing::TestingSerializer::new();
            pagable::PagableSerialize::pagable_serialize(&map, &mut serializer).unwrap();
            let bytes = serializer.finish();
            let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
            let restored: ActionOutputsMap =
                pagable::PagableDeserialize::pagable_deserialize(&mut deserializer).unwrap();
            assert_eq!(
                restored.iter().collect::<Vec<_>>(),
                map.iter().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_cleanup_path_missing() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let fs = fs.path();
        fs_util::create_dir_all(fs.resolve(ProjectRelativePath::unchecked_new("foo/bar/qux")))?;
        cleanup_path(fs, ProjectRelativePath::unchecked_new("foo/bar/qux/xx"))?;
        assert!(
            fs.resolve(ProjectRelativePath::unchecked_new("foo/bar/qux"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn test_cleanup_path_present() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let fs = fs.path();
        fs_util::create_dir_all(fs.resolve(ProjectRelativePath::unchecked_new("foo/bar/qux")))?;
        cleanup_path(fs, ProjectRelativePath::unchecked_new("foo/bar/qux"))?;
        assert!(
            !fs.resolve(ProjectRelativePath::unchecked_new("foo/bar/qux"))
                .exists()
        );
        assert!(
            fs.resolve(ProjectRelativePath::unchecked_new("foo/bar"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn test_cleanup_path_overlap() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let fs = fs.path();
        fs.write_file(ProjectRelativePath::unchecked_new("foo/bar"), "xx", false)?;
        cleanup_path(fs, ProjectRelativePath::unchecked_new("foo/bar/qux"))?;
        assert!(
            !fs.resolve(ProjectRelativePath::unchecked_new("foo/bar"))
                .exists()
        );
        assert!(
            fs.resolve(ProjectRelativePath::unchecked_new("foo"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn test_cleanup_path_overlap_deep() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let fs = fs.path();
        fs.write_file(ProjectRelativePath::unchecked_new("foo/bar"), "xx", false)?;
        cleanup_path(
            fs,
            ProjectRelativePath::unchecked_new("foo/bar/qux/1/2/3/4"),
        )?;
        assert!(
            !fs.resolve(ProjectRelativePath::unchecked_new("foo/bar"))
                .exists()
        );
        assert!(
            fs.resolve(ProjectRelativePath::unchecked_new("foo"))
                .exists()
        );
        Ok(())
    }
}
