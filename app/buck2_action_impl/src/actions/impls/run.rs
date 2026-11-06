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
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::BaseArtifactKind;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::box_slice_set::BoxSliceSet;
use buck2_build_api::actions::errors::error_handler::OutputArtifacts;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_execution_target::ActionExecutionTarget;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::actions::impls::expanded_command_line::ExpandedCommandLine;
use buck2_build_api::actions::impls::expanded_command_line::ExpandedCommandLineDigest;
use buck2_build_api::actions::impls::expanded_command_line::ExpandedCommandLineFingerprinter;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::ArtifactGroupValues;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_value::StarlarkArtifactValue;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::FrozenStarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineSink;
use buck2_build_api::interpreter::rule_defs::cmd_args::FrozenStarlarkCmdArgs;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_build_api::interpreter::rule_defs::cmd_args::Requires;
use buck2_build_api::interpreter::rule_defs::cmd_args::SimpleCommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::SingletonCommandLineSink;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use buck2_build_api::interpreter::rule_defs::cmd_args::path_format;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_build_api::interpreter::rule_defs::provider::builtin::worker_info::WorkerInfo;
use buck2_build_signals::env::WaitingCategory;
use buck2_build_signals::env::WaitingData;
use buck2_common::io::trace::TracingIoProvider;
use buck2_core::category::Category;
use buck2_core::category::CategoryRef;
use buck2_core::configuration::pair::Configuration;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::ReGangWorker;
use buck2_core::execution_types::executor_config::RemoteExecutorCustomImage;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_events::dispatch::span_async_simple;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::execute::action_digest::ActionDigest;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use buck2_execute::execute::cache_uploader::IntoRemoteDepFile;
use buck2_execute::execute::cache_uploader::force_cache_upload;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::execute::dep_file_digest::DepFileDigest;
use buck2_execute::execute::environment_inheritance::EnvironmentInheritance;
use buck2_execute::execute::request::ActionMetadataBlob;
use buck2_execute::execute::request::ActionSalt;
use buck2_execute::execute::request::CommandExecutionInput;
use buck2_execute::execute::request::CommandExecutionOutput;
use buck2_execute::execute::request::CommandExecutionPaths;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::request::ExecutorPreference;
use buck2_execute::execute::request::RemoteWorkerSpec;
use buck2_execute::execute::request::WorkerId;
use buck2_execute::execute::request::WorkerSpec;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::re::client::induced_cache_misses_configured;
use buck2_execute::re::client::is_induced_cache_miss;
use buck2_fs::fs_util;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckFixedSortedSet;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_hash::buck_mut_insertion_map;
use derive_more::Display;
use dupe::Dupe;
use host_sharing::HostSharingRequirements;
use host_sharing::WeightClass;
use itertools::Itertools;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use pagable::pagable_typetag;
use serde_json::json;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::OwnedFrozen;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::starlark_value;
use strong_hash::StrongHash;

use self::dep_files::DepFileBundle;
use crate::actions::impls::declare_writes::declare_writes;
use crate::actions::impls::dep_file_fingerprint::StarlarkDepFileFingerprint;
use crate::actions::impls::offline;
use crate::actions::impls::run::dep_files::DepFilesCommandLineVisitor;
use crate::actions::impls::run::dep_files::RunActionDepFiles;
use crate::actions::impls::run::dep_files::make_dep_file_bundle;
use crate::actions::impls::run::dep_files::populate_dep_files;
use crate::actions::impls::run::metadata::metadata_content;
use crate::context::run::RunActionError;

pub(crate) mod audit_dep_files;
pub(crate) mod dep_files;
mod metadata;

#[derive(Debug, Allocative, Pagable)]
pub(crate) struct MetadataParameter {
    /// Name of the environment variable which is set to contain
    /// resolved path of the metadata file when requested by user.
    pub(crate) env_var: String,
    /// User-defined path in the output directory of the metadata file.
    pub(crate) path: ForwardRelativePathBuf,
    /// An artifact that is 'tagged' with any of these tags is ignored
    /// when computing the metadata.
    pub(crate) ignore_tags: BuckFixedSortedSet<ArtifactTag>,
}

impl Display for MetadataParameter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let json = json!({
            "env_var": self.env_var,
            "path": self.path,
            "ignore_tags": self.ignore_tags.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        });
        write!(f, "{json}")
    }
}

/// A key that uniquely identifies a RunAction.
#[derive(Eq, PartialEq, Hash, Clone, Dupe, Display, Allocative, StrongHash)]
#[display(
    "{} {} {}",
    owner,
    category,
    identifier.as_deref().unwrap_or("<no identifier>")
)]
pub(crate) struct RunActionKey {
    owner: BaseDeferredKey,
    category: Category,
    identifier: Option<Arc<str>>,
}

/// The configuration-independent identity of a `RunActionKey`
#[derive(Eq, PartialEq, Hash, Clone, Dupe, Allocative, StrongHash)]
pub(crate) enum LogicalActionKey {
    Configured {
        target: TargetLabel,
        category: Category,
        identifier: Option<Arc<str>>,
    },
    Other(RunActionKey),
}

impl RunActionKey {
    pub(crate) fn new(
        owner: BaseDeferredKey,
        category: Category,
        identifier: Option<Arc<str>>,
    ) -> Self {
        Self {
            owner,
            category,
            identifier,
        }
    }

    pub(crate) fn owner(&self) -> &BaseDeferredKey {
        &self.owner
    }

    /// The target configuration (`cfg` + `exec_cfg`) of this action, if it is owned by a configured
    /// target. `None` for anon-target and BXL actions, which have no such configuration.
    pub(crate) fn configuration(&self) -> Option<Configuration> {
        self.owner()
            .unpack_target_label()
            .map(|t| t.cfg_pair().dupe())
    }

    pub(crate) fn from_action_execution_target(target: ActionExecutionTarget<'_>) -> Self {
        Self {
            owner: target.owner().dupe(),
            category: target.category().to_owned(),
            identifier: target.identifier().map(Arc::from),
        }
    }

    /// The key of this action in the incremental state database. The `Display` of a BXL owner
    /// contains only the BXL function label. Without the owner's hash, BXL invocations with
    /// different arguments share an entry. A content-based incremental action then starts from the
    /// output of a different invocation. The owner's hash includes the arguments.
    pub(crate) fn incremental_state_key(&self) -> String {
        match &self.owner {
            BaseDeferredKey::BxlLabel(bxl) => format!("{} {:016x}", self, bxl.0.strong_hash()),
            _ => self.to_string(),
        }
    }

    pub(crate) fn to_logical(&self) -> LogicalActionKey {
        match &self.owner {
            BaseDeferredKey::TargetLabel(configured) => LogicalActionKey::Configured {
                target: configured.unconfigured().dupe(),
                category: self.category,
                identifier: self.identifier.dupe(),
            },
            BaseDeferredKey::AnonTarget(_) | BaseDeferredKey::BxlLabel(_) => {
                LogicalActionKey::Other(self.dupe())
            }
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum LocalPreferenceError {
    #[error("cannot have `local_only = True` and `prefer_local = True` at the same time")]
    LocalOnlyAndPreferLocal,
    #[error("cannot have `local_only = True` and `prefer_remote = True` at the same time")]
    LocalOnlyAndPreferRemote,
    #[error(
        "cannot have `local_only = True`, `prefer_local = True` and `prefer_remote = True` at the same time"
    )]
    LocalOnlyAndPreferLocalAndPreferRemote,
    #[error("cannot have `prefer_local = True` and `prefer_remote = True` at the same time")]
    PreferLocalAndPreferRemote,
}

pub(crate) fn new_executor_preference(
    local_only: bool,
    prefer_local: bool,
    prefer_remote: bool,
) -> buck2_error::Result<ExecutorPreference> {
    match (local_only, prefer_local, prefer_remote) {
        (true, false, false) => Ok(ExecutorPreference::LocalRequired),
        (true, false, true) => Err(LocalPreferenceError::LocalOnlyAndPreferRemote.into()),
        (false, true, false) => Ok(ExecutorPreference::LocalPreferred),
        (false, true, true) => Err(LocalPreferenceError::PreferLocalAndPreferRemote.into()),
        (false, false, false) => Ok(ExecutorPreference::Default),
        (false, false, true) => Ok(ExecutorPreference::RemotePreferred),
        (true, true, false) => Err(LocalPreferenceError::LocalOnlyAndPreferLocal.into()),
        (true, true, true) => {
            Err(LocalPreferenceError::LocalOnlyAndPreferLocalAndPreferRemote.into())
        }
    }
}

#[derive(Debug, Allocative, Pagable)]
pub(crate) struct UnregisteredRunAction {
    pub(crate) executor_preference: ExecutorPreference,
    pub(crate) always_print_stderr: bool,
    pub(crate) weight: WeightClass,
    pub(crate) low_pass_filter: bool,
    pub(crate) dep_files: RunActionDepFiles,
    // Since this is usually None, use a Box to reduce memory.
    pub(crate) metadata_param: Option<Box<MetadataParameter>>,
    pub(crate) no_outputs_cleanup: bool,
    pub(crate) incremental_remote_outputs: bool,
    pub(crate) allow_cache_upload: Option<bool>,
    pub(crate) allow_dep_file_cache_upload: bool,
    pub(crate) allow_offline_output_cache: bool,
    pub(crate) force_full_hybrid_if_capable: bool,
    pub(crate) unique_input_inodes: bool,
    pub(crate) remote_execution_dependencies: MiniBoxSlice<RemoteExecutorDependency>,
    pub(crate) re_gang_workers: MiniBoxSlice<ReGangWorker>,
    // Since this is usually None, use a Box to avoid using memory that is the size
    // of RemoteExecutorCustomImage.
    pub(crate) remote_execution_custom_image: Option<Box<RemoteExecutorCustomImage>>,
    pub(crate) meta_internal_extra_params: Arc<MetaInternalExtraParams>,
    pub(crate) expected_eligible_for_dedupe: Option<bool>,
    pub(crate) timeout: Option<Duration>,
}

impl UnregisteredAction for UnregisteredRunAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        starlark_data: Option<OwnedFrozen<Value<'static>>>,
        error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        let starlark_values = starlark_data.internal_error("module data to be present")?;
        let run_action = RunAction::new(*self, starlark_values, outputs, error_handler)?;
        Ok(Box::new(run_action))
    }
}

#[derive(Debug, Display, Trace, ProvidesStaticType, NoSerialize, Allocative)]
#[display("RunActionValues")]
pub(crate) struct StarlarkRunActionValues<'v> {
    pub(crate) exe: ValueTyped<'v, StarlarkCmdArgs<'v>>,
    pub(crate) args: ValueTyped<'v, StarlarkCmdArgs<'v>>,
    pub(crate) dep_file_fingerprints: MiniBoxSlice<ValueTyped<'v, StarlarkDepFileFingerprint<'v>>>,
    pub(crate) env: Option<ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>>,
    pub(crate) worker: Option<ValueTyped<'v, WorkerInfo<'v>>>,
    pub(crate) remote_worker: Option<ValueTyped<'v, WorkerInfo<'v>>>,
    pub(crate) category: StringValue<'v>,
    pub(crate) identifier: Option<StringValue<'v>>,
    pub(crate) outputs_for_error_handler: MiniBoxSlice<ValueTyped<'v, StarlarkOutputArtifact<'v>>>,
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("RunActionValues")]
pub(crate) struct FrozenStarlarkRunActionValues<'v> {
    pub(crate) exe: ValueTyped<'v, FrozenStarlarkCmdArgs<'v>>,
    pub(crate) args: ValueTyped<'v, FrozenStarlarkCmdArgs<'v>>,
    pub(crate) dep_file_fingerprints: MiniBoxSlice<ValueTyped<'v, StarlarkDepFileFingerprint<'v>>>,
    pub(crate) env: Option<ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>>,
    pub(crate) worker: Option<ValueTyped<'v, WorkerInfo<'v>>>,
    pub(crate) remote_worker: Option<ValueTyped<'v, WorkerInfo<'v>>>,
    pub(crate) category: StringValue<'v>,
    pub(crate) identifier: Option<StringValue<'v>>,
    pub(crate) outputs_for_error_handler:
        MiniBoxSlice<ValueTyped<'v, FrozenStarlarkOutputArtifact<'v>>>,
}

/// A `RunActionValues` in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum StarlarkRunActionValuesView<'v> {
    Unfrozen(ValueTyped<'v, StarlarkRunActionValues<'v>>),
    Frozen(ValueTyped<'v, FrozenStarlarkRunActionValues<'v>>),
}

#[starlark_value(type = "RunActionValues")]
impl<'v> StarlarkType<'v> for StarlarkRunActionValuesView<'v> {}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkRunActionValues<'v> {
    type Type = StarlarkRunActionValuesView<'v>;
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenStarlarkRunActionValues<'v> {
    type Type = StarlarkRunActionValuesView<'v>;
}

impl<'v> AllocValue<'v> for StarlarkRunActionValues<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

impl<'v> Freeze<'v> for StarlarkRunActionValues<'v> {
    type Frozen<'fv> = FrozenStarlarkRunActionValues<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let StarlarkRunActionValues {
            exe,
            args,
            dep_file_fingerprints,
            env,
            worker,
            remote_worker,
            category,
            identifier,
            outputs_for_error_handler,
        } = self;

        Ok(FrozenStarlarkRunActionValues {
            exe: Freeze::freeze(exe, freezer)?,
            args: Freeze::freeze(args, freezer)?,
            dep_file_fingerprints: Freeze::freeze(dep_file_fingerprints, freezer)?,
            env: Freeze::freeze(env, freezer)?,
            worker: Freeze::freeze(worker, freezer)?,
            remote_worker: Freeze::freeze(remote_worker, freezer)?,
            category: category.freeze(freezer)?,
            identifier: identifier.map(|i| i.freeze(freezer)).transpose()?,
            outputs_for_error_handler: Freeze::freeze(outputs_for_error_handler, freezer)?,
        })
    }
}

impl<'v> FrozenStarlarkRunActionValues<'v> {
    pub(crate) fn worker(&self) -> buck2_error::Result<Option<ValueTyped<'v, WorkerInfo<'v>>>> {
        let Some(worker) = self.worker else {
            return Ok(None);
        };
        ValueTyped::new_err(worker.to_value())
            .map_err(buck2_error::Error::from)
            .map(Some)
    }

    pub(crate) fn remote_worker(
        &self,
    ) -> buck2_error::Result<Option<ValueTyped<'v, WorkerInfo<'v>>>> {
        let Some(remote_worker) = self.remote_worker else {
            return Ok(None);
        };
        ValueTyped::new_err(remote_worker.to_value())
            .map_err(buck2_error::Error::from)
            .map(Some)
    }
}

struct UnpackedWorkerValues<'v> {
    exe: &'v dyn CommandLineArgLike<'v>,
    env: Vec<(&'v str, &'v dyn CommandLineArgLike<'v>)>,
    id: WorkerId,
    concurrency: Option<usize>,
    streaming: bool,
    supports_bazel_remote_persistent_worker_protocol: bool,
}

struct UnpackedRunActionValues<'v> {
    exe: &'v dyn CommandLineArgLike<'v>,
    args: &'v dyn CommandLineArgLike<'v>,
    dep_file_fingerprints: &'v [ValueTyped<'v, StarlarkDepFileFingerprint<'v>>],
    env: Vec<(&'v str, &'v dyn CommandLineArgLike<'v>)>,
    worker: Option<UnpackedWorkerValues<'v>>,
    remote_worker: Option<UnpackedWorkerValues<'v>>,
}

#[derive(Debug, Allocative, Pagable)]
pub(crate) struct RunAction {
    inner: UnregisteredRunAction,
    starlark_values: OwnedFrozen<ValueTyped<'static, FrozenStarlarkRunActionValues<'static>>>,
    outputs: BoxSliceSet<BuildArtifact>,
    error_handler: Option<OwnedFrozen<Value<'static>>>,
}

#[allow(clippy::large_enum_variant)]
enum ExecuteResult {
    LocalDepFileHit(ActionOutputs, ActionExecutionMetadata),
    ExecutedOrReHit {
        result: CommandExecutionResult,
        dep_file_bundle: DepFileBundle,
        executor_preference: ExecutorPreference,
        action_and_blobs: ActionDigestAndBlobs,
        input_files_bytes: u64,
    },
}

pub struct DepFilesPlaceholderArtifactPathMapper {}

impl ArtifactPathMapper for DepFilesPlaceholderArtifactPathMapper {
    fn get(&self, _artifact: &Artifact) -> Option<&ContentBasedPathHash> {
        Some(&ContentBasedPathHash::DepFilesPlaceholder)
    }
}

type ExpandedCommandLineDigestForDepFiles = ExpandedCommandLineDigest;

/// A CommandLineArtifactVisitor that gathers non-hidden inputs.
pub struct SkipHiddenCommandLineArtifactVisitor {
    pub inputs: BuckMutInsertionSet<ArtifactGroup>,
    pub path_only_inputs: BuckMutInsertionSet<ArtifactGroup>,
}

impl SkipHiddenCommandLineArtifactVisitor {
    pub fn new() -> Self {
        Self {
            inputs: BuckMutInsertionSet::default(),
            path_only_inputs: BuckMutInsertionSet::default(),
        }
    }
}

impl CommandLineArtifactVisitor<'_> for SkipHiddenCommandLineArtifactVisitor {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        props: InputProperties,
        _tags: Vec<&ArtifactTag>,
    ) {
        let InputProperties {
            hidden: _,
            requires,
        } = props;
        match requires {
            Requires::Contents => {
                self.inputs.insert(input);
            }
            Requires::Path => {
                self.path_only_inputs.insert(input);
            }
            Requires::Digest => {}
        }
    }

    fn visit_declared_output(&mut self, _artifact: OutputArtifact<'_>, _tags: Vec<&ArtifactTag>) {}

    fn visit_frozen_output(&mut self, _artifact: Artifact, _tags: Vec<&ArtifactTag>) {}

    fn skip_hidden(&self) -> bool {
        true
    }
}

/// Dep-file command line digests normally render every content-based path as a
/// placeholder: the content of a real input already participates in dep-file
/// matching, and using the real hash would defeat it. A path-only input
/// participates in nothing else — its content affects the action solely through
/// its rendered path — so its real hash must reach the digest, or a change to it
/// could be missed by a dep-file match.
struct PathOnlyAwareDepFilesPlaceholderMapper<'a> {
    mapping: &'a BuckMutMap<&'a Artifact, ContentBasedPathHash>,
    path_only: &'a BuckMutSet<&'a Artifact>,
}

impl ArtifactPathMapper for PathOnlyAwareDepFilesPlaceholderMapper<'_> {
    fn get(&self, artifact: &Artifact) -> Option<&ContentBasedPathHash> {
        if self.path_only.contains(artifact) {
            self.mapping.get(artifact)
        } else {
            Some(&ContentBasedPathHash::DepFilesPlaceholder)
        }
    }
}

impl RunAction {
    fn visit_artifacts<'a>(
        &'a self,
        artifact_visitor: &mut dyn CommandLineArtifactVisitor<'a>,
    ) -> buck2_error::Result<()> {
        let values = Self::unpack(self.values())?;
        values.args.visit_artifacts(artifact_visitor)?;
        for fingerprint in values.dep_file_fingerprints {
            fingerprint.as_ref().visit_inputs(artifact_visitor)?;
        }
        values.exe.visit_artifacts(artifact_visitor)?;
        if let Some(worker) = values.worker {
            worker.exe.visit_artifacts(artifact_visitor)?;
        }
        if let Some(remote_worker) = values.remote_worker {
            remote_worker.exe.visit_artifacts(artifact_visitor)?;
            for (_, v) in remote_worker.env.iter() {
                v.visit_artifacts(artifact_visitor)?;
            }
        }
        for (_, v) in values.env.iter() {
            v.visit_artifacts(artifact_visitor)?;
        }
        Ok(())
    }

    /// The branded view of `starlark_values`; confine to sync scopes.
    fn values<'v>(&'v self) -> &'v FrozenStarlarkRunActionValues<'v> {
        self.starlark_values.as_ref().value().as_ref()
    }

    fn unpack<'v>(
        values: &'v FrozenStarlarkRunActionValues<'v>,
    ) -> buck2_error::Result<UnpackedRunActionValues<'v>> {
        let exe: &dyn CommandLineArgLike = values.exe.as_ref();
        let args: &dyn CommandLineArgLike = values.args.as_ref();
        let env = match values.env {
            None => Vec::new(),
            Some(env) => env
                .iter()
                .map(|entry| {
                    let (k, v) = entry?;
                    Ok((k.unpack().as_str(), v.unpack().0))
                })
                .collect::<buck2_error::Result<Vec<_>>>()?,
        };
        let worker: Option<&WorkerInfo> = values.worker()?.map(|v| v.as_ref());

        let worker = worker
            .map(|worker| {
                buck2_error::Ok(UnpackedWorkerValues {
                    exe: worker.exe_command_line(),
                    env: worker.env()?,
                    id: WorkerId(worker.id),
                    concurrency: worker.concurrency(),
                    streaming: worker.streaming(),
                    supports_bazel_remote_persistent_worker_protocol: worker
                        .supports_bazel_remote_persistent_worker_protocol(),
                })
            })
            .transpose()?;

        let remote_worker: Option<&WorkerInfo> = values.remote_worker()?.map(|v| v.as_ref());

        let remote_worker = remote_worker
            .map(|remote_worker| {
                buck2_error::Ok(UnpackedWorkerValues {
                    exe: remote_worker.exe_command_line(),
                    env: remote_worker.env()?,
                    id: WorkerId(remote_worker.id),
                    concurrency: remote_worker.concurrency(),
                    streaming: false,
                    supports_bazel_remote_persistent_worker_protocol: false,
                })
            })
            .transpose()?;

        Ok(UnpackedRunActionValues {
            exe,
            args,
            dep_file_fingerprints: &values.dep_file_fingerprints,
            env,
            worker,
            remote_worker,
        })
    }

    /// Get the command line expansion for this RunAction.
    fn expand_command_line_and_worker<'v>(
        &'v self,
        action_execution_ctx: &dyn ActionExecutionCtx,
        artifact_visitor: &mut RunActionVisitor<'v>,
    ) -> buck2_error::Result<(
        ExpandedCommandLine,
        ExpandedCommandLineDigestForDepFiles,
        Option<WorkerSpec>,
        Option<RemoteWorkerSpec>,
        Option<ActionSalt>,
    )> {
        let fs = &action_execution_ctx.executor_fs();
        let values = Self::unpack(self.values())?;

        // Creating the artifact_path_mapping isn't free, because we have to iterate TSets.
        // Therefore, only create a mapping if we're going to use it - i.e. if the input
        // is not hidden.
        let mut skip_hidden_visitor = SkipHiddenCommandLineArtifactVisitor::new();
        self.visit_artifacts(&mut skip_hidden_visitor)?;
        let (artifact_path_mapping, path_only_artifacts) = action_execution_ctx
            .artifact_path_mapping_with_path_only(
                &skip_hidden_visitor.inputs,
                &skip_hidden_visitor.path_only_inputs,
            );

        let mut exe_rendered = Vec::<String>::new();
        values
            .exe
            .add_to_command_line(&mut CommandLineBuilder::new(
                &mut exe_rendered,
                &artifact_path_mapping,
                fs,
            ))?;

        let mut command_line_digest_for_dep_files = ExpandedCommandLineFingerprinter::new();
        let artifact_path_mapping_for_dep_files = PathOnlyAwareDepFilesPlaceholderMapper {
            mapping: &artifact_path_mapping,
            path_only: &path_only_artifacts,
        };
        values
            .exe
            .add_to_command_line(&mut CommandLineBuilder::new(
                &mut command_line_digest_for_dep_files,
                &artifact_path_mapping_for_dep_files,
                fs,
            ))?;
        values.exe.visit_artifacts(artifact_visitor)?;
        command_line_digest_for_dep_files.push_count();

        let worker = if let Some(worker) = values.worker {
            let mut worker_rendered = Vec::<String>::new();
            worker
                .exe
                .add_to_command_line(&mut CommandLineBuilder::new(
                    &mut worker_rendered,
                    &artifact_path_mapping,
                    fs,
                ))?;
            worker
                .exe
                .add_to_command_line(&mut CommandLineBuilder::new(
                    &mut command_line_digest_for_dep_files,
                    &artifact_path_mapping_for_dep_files,
                    fs,
                ))?;
            let mut local_worker_visitor = SimpleCommandLineArtifactVisitor::new();
            worker.exe.visit_artifacts(&mut local_worker_visitor)?;
            let worker_env: buck2_error::Result<BuckFixedSortedMap<_, _>> = worker
                .env
                .into_iter()
                .map(|(k, v)| {
                    v.visit_artifacts(&mut local_worker_visitor)?;

                    let mut env = SingletonCommandLineSink::new();
                    let mut env_fmt = CommandLineBuilder::new(&mut env, &artifact_path_mapping, fs);
                    env_fmt.push_scope_delimiter(" ");
                    v.add_to_command_line(&mut env_fmt)?;
                    env_fmt.pop_scope();

                    command_line_digest_for_dep_files.push_arg(Cow::Borrowed(k));
                    v.add_to_command_line(&mut CommandLineBuilder::new(
                        &mut command_line_digest_for_dep_files,
                        &artifact_path_mapping_for_dep_files,
                        fs,
                    ))?;
                    command_line_digest_for_dep_files.push_count();
                    Ok((k.to_owned(), env.finalize()?))
                })
                .collect();

            let local_worker_inputs: Vec<&ArtifactGroupValues> = local_worker_visitor
                .inputs()
                .map(|group| action_execution_ctx.artifact_values(group))
                .collect();

            let inputs: Vec<CommandExecutionInput> = local_worker_inputs[..]
                .iter()
                .map(|&i| CommandExecutionInput::Artifact(Box::new(i.dupe())))
                .collect::<Vec<_>>();

            let input_paths = CommandExecutionPaths::new(
                inputs,
                BuckMutInsertionSet::default(),
                action_execution_ctx.fs(),
                action_execution_ctx.digest_config(),
                action_execution_ctx
                    .run_action_knobs()
                    .action_paths_interner
                    .as_ref(),
            )?;

            let worker_key = if worker.supports_bazel_remote_persistent_worker_protocol {
                let mut worker_visitor = SimpleCommandLineArtifactVisitor::new();
                worker.exe.visit_artifacts(&mut worker_visitor)?;
                if !worker_visitor.declared_outputs.is_empty()
                    && !worker_visitor.frozen_outputs.is_empty()
                {
                    // TODO[AH] create appropriate error enum value.
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::ActionMismatchedOutputs,
                        "Remote persistent worker command should not produce outputs."
                    ));
                }
                let worker_inputs: Vec<&ArtifactGroupValues> = worker_visitor
                    .inputs()
                    .map(|group| action_execution_ctx.artifact_values(group))
                    .collect();
                let (_, worker_digest) = metadata_content(
                    fs.fs(),
                    &worker_inputs,
                    action_execution_ctx.digest_config(),
                )?;
                Some(worker_digest)
            } else {
                None
            };

            Some(WorkerSpec {
                exe: worker_rendered,
                id: worker.id,
                env: worker_env?,
                concurrency: worker.concurrency,
                streaming: worker.streaming,
                remote_key: worker_key,
                input_paths,
            })
        } else {
            None
        };

        let remote_worker = if let Some(remote_worker) = values.remote_worker {
            let mut remote_worker_init_visitor = SimpleCommandLineArtifactVisitor::new();
            let mut remote_worker_init_rendered = Vec::<String>::new();
            remote_worker
                .exe
                .add_to_command_line(&mut CommandLineBuilder::new(
                    &mut remote_worker_init_rendered,
                    &artifact_path_mapping,
                    fs,
                ))?;
            remote_worker
                .exe
                .add_to_command_line(&mut CommandLineBuilder::new(
                    &mut command_line_digest_for_dep_files,
                    &artifact_path_mapping_for_dep_files,
                    fs,
                ))?;
            remote_worker
                .exe
                .visit_artifacts(&mut remote_worker_init_visitor)?;

            let remote_worker_env: buck2_error::Result<BuckFixedSortedMap<_, _>> = remote_worker
                .env
                .into_iter()
                .map(|(k, v)| {
                    v.visit_artifacts(&mut remote_worker_init_visitor)?;

                    let mut env = SingletonCommandLineSink::new();
                    let mut env_fmt = CommandLineBuilder::new(&mut env, &artifact_path_mapping, fs);
                    env_fmt.push_scope_delimiter(" ");
                    v.add_to_command_line(&mut env_fmt)?;
                    env_fmt.pop_scope();

                    command_line_digest_for_dep_files.push_arg(Cow::Borrowed(k));
                    v.add_to_command_line(&mut CommandLineBuilder::new(
                        &mut command_line_digest_for_dep_files,
                        &artifact_path_mapping_for_dep_files,
                        fs,
                    ))?;
                    command_line_digest_for_dep_files.push_count();
                    Ok((k.to_owned(), env.finalize()?))
                })
                .collect();

            let artifact_inputs: Vec<&ArtifactGroupValues> = remote_worker_init_visitor
                .inputs()
                .map(|group| action_execution_ctx.artifact_values(group))
                .collect();

            let inputs: Vec<CommandExecutionInput> = artifact_inputs[..]
                .iter()
                .map(|&i| CommandExecutionInput::Artifact(Box::new(i.dupe())))
                .collect::<Vec<_>>();

            let input_paths = CommandExecutionPaths::new(
                inputs,
                BuckMutInsertionSet::default(),
                action_execution_ctx.fs(),
                action_execution_ctx.digest_config(),
                action_execution_ctx
                    .run_action_knobs()
                    .action_paths_interner
                    .as_ref(),
            )?;
            Some(RemoteWorkerSpec {
                id: remote_worker.id,
                init: remote_worker_init_rendered,
                env: remote_worker_env?,
                input_paths,
                concurrency: remote_worker.concurrency,
            })
        } else {
            None
        };

        let mut args_rendered = Vec::<String>::new();
        values
            .args
            .add_to_command_line(&mut CommandLineBuilder::new(
                &mut args_rendered,
                &artifact_path_mapping,
                fs,
            ))?;
        values
            .args
            .add_to_command_line(&mut CommandLineBuilder::new(
                &mut command_line_digest_for_dep_files,
                &artifact_path_mapping_for_dep_files,
                fs,
            ))?;
        values.args.visit_artifacts(artifact_visitor)?;
        command_line_digest_for_dep_files.push_count();

        let env_len = values.env.len();
        let cli_env: buck2_error::Result<BuckFixedSortedMap<_, _>> = values
            .env
            .into_iter()
            .map(|(k, v)| {
                v.visit_artifacts(artifact_visitor)?;

                let mut env = SingletonCommandLineSink::new();
                let mut env_fmt = CommandLineBuilder::new(&mut env, &artifact_path_mapping, fs);
                env_fmt.push_scope_delimiter(" ");
                v.add_to_command_line(&mut env_fmt)?;
                env_fmt.pop_scope();

                command_line_digest_for_dep_files.push_arg(Cow::Borrowed(k));
                v.add_to_command_line(&mut CommandLineBuilder::new(
                    &mut command_line_digest_for_dep_files,
                    &artifact_path_mapping_for_dep_files,
                    fs,
                ))?;
                command_line_digest_for_dep_files.push_count();
                Ok((k.to_owned(), env.finalize()?))
            })
            .collect();

        command_line_digest_for_dep_files.push_arg(Cow::Owned(env_len.to_string()));
        command_line_digest_for_dep_files.push_count();

        if !values.dep_file_fingerprints.is_empty() {
            command_line_digest_for_dep_files
                .push_arg(Cow::Borrowed("buck2.structured-dep-file-inputs.v1"));
            for fingerprint in values.dep_file_fingerprints {
                let fingerprint = fingerprint.as_ref();
                fingerprint.visit_inputs(artifact_visitor)?;
                let (path, digest) = fingerprint.fingerprint(fs)?;
                command_line_digest_for_dep_files.push_bytes(path.as_str().as_bytes());
                command_line_digest_for_dep_files.push_bytes(&digest);
            }
            command_line_digest_for_dep_files.push_count();
        }

        // Digest inputs are in neither the command line nor the input tree; one digest
        // over their paths and contents stands in for the input-tree entries they would
        // otherwise have contributed.
        let salt = if artifact_visitor.digest_inputs.is_empty() {
            None
        } else {
            let groups: Vec<&ArtifactGroupValues> = artifact_visitor
                .digest_inputs
                .iter()
                .map(|group| action_execution_ctx.artifact_values(group))
                .collect();
            let (_, digest) =
                metadata_content(fs.fs(), &groups, action_execution_ctx.digest_config())?;
            let salt = ActionSalt::from_digest(&digest);
            command_line_digest_for_dep_files.push_bytes(salt.as_bytes());
            command_line_digest_for_dep_files.push_count();
            Some(salt)
        };

        Ok((
            ExpandedCommandLine {
                exe: exe_rendered,
                args: args_rendered,
                env: cli_env?,
            },
            command_line_digest_for_dep_files.finalize(),
            worker,
            remote_worker,
            salt,
        ))
    }

    pub(crate) fn new(
        inner: UnregisteredRunAction,
        starlark_values: OwnedFrozen<Value<'static>>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Self> {
        let starlark_values: OwnedFrozen<
            ValueTyped<'static, FrozenStarlarkRunActionValues<'static>>,
        > = starlark_values
            // The eta-expansion is load-bearing: a closure gets the
            // higher-ranked signature `try_map` needs, the fn item does not.
            .try_map(
                #[allow(clippy::redundant_closure)]
                |v| ValueTyped::new_err(v),
            )
            .map_err(buck2_error::Error::from)
            .internal_error("Must be `RunActionValues`")?;

        Self::unpack(starlark_values.as_ref().value().as_ref())?;

        // This is checked when declared, but we depend on it so make it clear that it's enforced.
        if outputs.is_empty() {
            return Err(RunActionError::NoOutputsSpecified.into());
        }

        Ok(RunAction {
            inner,
            starlark_values,
            outputs: BoxSliceSet::from(outputs),
            error_handler,
        })
    }

    async fn prepare<'v>(
        &'v self,
        visitor: &mut RunActionVisitor<'v>,
        ctx: &mut dyn ActionExecutionCtx,
    ) -> buck2_error::Result<(
        PreparedRunAction,
        ExpandedCommandLineDigestForDepFiles,
        HostSharingRequirements,
    )> {
        let (expanded, expanded_command_line_digest_for_dep_files, worker, remote_worker, salt) =
            self.expand_command_line_and_worker(ctx, visitor)?;

        let executor_fs = ctx.executor_fs();
        let fs = executor_fs.fs();

        // TODO (@torozco): At this point, might as well just receive the list already. Finding
        // those things in a HashMap is just not very useful.
        let artifact_inputs: Vec<&ArtifactGroupValues> = visitor
            .inputs()
            .map(|group| ctx.artifact_values(group))
            .collect();

        let mut inputs: Vec<CommandExecutionInput> = artifact_inputs[..]
            .iter()
            .map(|&i| CommandExecutionInput::Artifact(Box::new(i.dupe())))
            .collect::<Vec<_>>();

        let mut extra_env = Vec::new();
        self.prepare_action_metadata(ctx, &executor_fs, visitor, &mut inputs, &mut extra_env)
            .await?;

        let mut shared_content_based_paths = Vec::new();
        self.prepare_scratch_path(
            ctx,
            &executor_fs,
            &mut inputs,
            &mut shared_content_based_paths,
            &mut extra_env,
        )?;

        for output in self.outputs.iter() {
            if output.get_path().is_content_based_path() {
                let full_path = fs.buck_out_path_resolver().resolve_gen(
                    output.get_path(),
                    Some(&ContentBasedPathHash::for_output_artifact()),
                )?;
                shared_content_based_paths.push(full_path);
            }
        }

        // TODO(ianc) Only do this if we're actually going to run the action?
        let host_sharing_requirements = if !shared_content_based_paths.is_empty() {
            HostSharingRequirements::OnePerTokens(
                shared_content_based_paths
                    .into_iter()
                    .map(|p| p.to_string())
                    .collect(),
                self.inner.weight,
            )
        } else {
            HostSharingRequirements::Shared(self.inner.weight)
        };

        let paths = CommandExecutionPaths::new(
            inputs,
            self.outputs
                .iter()
                .map(|b| CommandExecutionOutput::BuildArtifact {
                    path: b.get_path().dupe(),
                    output_type: b.output_type(),
                })
                .collect(),
            ctx.fs(),
            ctx.digest_config(),
            ctx.run_action_knobs().action_paths_interner.as_ref(),
        )?;

        Ok((
            PreparedRunAction {
                expanded,
                extra_env,
                paths,
                worker,
                remote_worker,
                salt,
            },
            expanded_command_line_digest_for_dep_files,
            host_sharing_requirements,
        ))
    }

    /// Handle case when user requested file with action metadata to be generated.
    /// Generate content and output path for the file. It will be either passed
    /// to RE as a blob or written to disk in local executor.
    /// Path to this file is passed to user in environment variable which is selected by user.
    async fn prepare_action_metadata(
        &self,
        ctx: &dyn ActionExecutionCtx,
        fs: &ExecutorFs<'_>,
        visitor: &mut RunActionVisitor<'_>,
        inputs: &mut Vec<CommandExecutionInput>,
        extra_env: &mut Vec<(String, String)>,
    ) -> buck2_error::Result<()> {
        if let Some(metadata_param) = &self.inner.metadata_param {
            let path = BuildArtifactPath::new(
                ctx.target().owner().dupe(),
                metadata_param.path.clone(),
                if self.all_outputs_are_content_based() {
                    BuckOutPathKind::ContentHash
                } else {
                    BuckOutPathKind::Configuration
                },
            );

            let artifact_inputs: Vec<&ArtifactGroupValues> = visitor
                .incremental_metadata_inputs
                .iter()
                .map(|group| ctx.artifact_values(group))
                .collect();
            let (data, digest) = metadata_content(fs.fs(), &artifact_inputs, ctx.digest_config())?;
            let content_hash = ContentBasedPathHash::new(digest.raw_digest().as_bytes())?;
            let project_rel_path = fs
                .fs()
                .buck_out_path_resolver()
                .resolve_gen(&path, Some(&content_hash))?;

            declare_writes(
                ctx,
                Box::new(|| {
                    Ok(vec![WriteRequest {
                        path: project_rel_path.clone(),
                        content: data.0.0.clone(),
                        is_executable: false,
                        path_kind: path.path_resolution_method(),
                    }])
                }),
            )
            .await
            .buck_error_context("Failed to write action metadata!")?;

            inputs.push(CommandExecutionInput::ActionMetadata(ActionMetadataBlob {
                digest,
                path,
                content_hash,
            }));

            extra_env.push((
                metadata_param.env_var.to_owned(),
                path_format(project_rel_path.as_ref(), fs.path_separator()).into_owned(),
            ));
        }
        Ok(())
    }

    fn prepare_scratch_path(
        &self,
        ctx: &dyn ActionExecutionCtx,
        fs: &ExecutorFs,
        inputs: &mut Vec<CommandExecutionInput>,
        shared_content_based_paths: &mut Vec<ProjectRelativePathBuf>,
        extra_env: &mut Vec<(String, String)>,
    ) -> buck2_error::Result<()> {
        let scratch = ctx.target().scratch_path();
        let scratch_path = fs.fs().buck_out_path_resolver().resolve_scratch(&scratch)?;

        if scratch.uses_content_hash() {
            shared_content_based_paths.push(scratch_path.clone());
        }

        extra_env.push((
            "BUCK_SCRATCH_PATH".to_owned(),
            path_format(scratch_path.as_ref(), fs.path_separator()).into_owned(),
        ));
        inputs.push(CommandExecutionInput::ScratchPath(scratch));

        Ok(())
    }

    pub(crate) async fn check_cache_result_is_useable(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        request: &CommandExecutionRequest,
        action_digest: &ActionDigest,
        result: CommandExecutionResult,
        dep_file_bundle: &DepFileBundle,
        remote_dep_file_key: &DepFileDigest,
    ) -> buck2_error::Result<ControlFlow<CommandExecutionResult, ()>> {
        // If it's served by the regular action cache no need to verify anything here.
        if !result.was_served_by_remote_dep_file_cache() {
            return Ok(ControlFlow::Break(result));
        }

        if let Some(found_dep_file_entry) = &result.dep_file_metadata {
            let can_use = span_async_simple(
                buck2_data::MatchDepFilesStart {
                    checking_filtered_inputs: true,
                    remote_cache: true,
                },
                dep_file_bundle.check_remote_dep_file_entry(
                    ctx.digest_config(),
                    ctx.fs(),
                    ctx.materializer(),
                    ctx.invocation_re_use_case(),
                    found_dep_file_entry,
                    &result,
                ),
                // The remote dep-file cache, not the persisted local store.
                buck2_data::MatchDepFilesEnd {
                    outcome: buck2_data::DepFileLookupOutcome::NotSet as i32,
                    persisted_probe_us: None,
                    persisted_fetch_us: None,
                    persisted_fetches: None,
                },
            )
            .await?;

            if can_use {
                tracing::info!(
                    "Action result is cached via remote dep file cache, skipping execution of :\n```\n$ {}\n```\n for action `{}` with remote dep file key `{}`",
                    request.all_args_str(),
                    action_digest,
                    &remote_dep_file_key,
                );
                return Ok(ControlFlow::Break(result));
            }
        }
        // Continue through other options below
        Ok(ControlFlow::Continue(()))
    }

    async fn execute_inner(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        mut waiting_data: WaitingData,
    ) -> Result<ExecuteResult, ExecuteError> {
        let incremental_action_ignore_tags = self
            .inner
            .metadata_param
            .as_ref()
            .map(|metadata_param| &metadata_param.ignore_tags);
        let mut run_action_visitor =
            RunActionVisitor::new(&self.inner.dep_files, incremental_action_ignore_tags);
        waiting_data.start_waiting_category_now(WaitingCategory::PreparingAction);
        let (prepared_run_action, cmdline_digest_for_dep_files, host_sharing_requirements) =
            self.prepare(&mut run_action_visitor, ctx).await?;

        let dep_file_bundle = make_dep_file_bundle(
            ctx,
            run_action_visitor.dep_files_visitor,
            cmdline_digest_for_dep_files,
            &prepared_run_action.paths,
            prepared_run_action.worker.as_ref().map(|w| &w.input_paths),
        )?;

        // First, check in the local dep file cache if an identical action can be found there.
        // Do this before checking the action cache as we can avoid a potentially large download.
        // Once the action cache lookup misses, we will do the full dep file cache look up.
        let (identical_action, should_fully_check_dep_file_cache) = dep_file_bundle
            .check_local_dep_file_cache_for_identical_action(ctx, self.outputs.as_slice())
            .await?;
        // We can avoid preparing the action for a local cache hit as long as we're not forcing certain digests to cache-miss.
        let forced_misses = induced_cache_misses_configured();
        if !forced_misses && let Some((outputs, metadata)) = identical_action {
            return Ok(ExecuteResult::LocalDepFileHit(outputs, metadata));
        }

        let req =
            self.command_execution_request(ctx, prepared_run_action, host_sharing_requirements)?;
        let prepared_action = ctx.prepare_action(&req, true)?;
        let induced_cache_miss = is_induced_cache_miss(&prepared_action.action_and_blobs.action);
        let req = req.with_induced_cache_miss(induced_cache_miss);
        if !induced_cache_miss && let Some((outputs, metadata)) = identical_action {
            return Ok(ExecuteResult::LocalDepFileHit(outputs, metadata));
        }
        let should_fully_check_dep_file_cache =
            should_fully_check_dep_file_cache && !induced_cache_miss;

        // Check the action cache, fully check the local dep file cache if needed, then execute the command
        waiting_data.start_waiting_category_now(WaitingCategory::CheckingCaches);
        let manager = ctx.command_execution_manager(waiting_data);

        let action_cache_result = ctx.action_cache(manager, &req, &prepared_action).await;

        let (req, result) = match action_cache_result {
            ControlFlow::Break(_) => (req, action_cache_result),
            ControlFlow::Continue(manager) => {
                // If we didn't find anything in the action cache, first do a local dep file cache lookup, and if that fails,
                // try to find a remote dep file cache hit.
                if should_fully_check_dep_file_cache {
                    let lookup = dep_file_bundle
                        .check_local_dep_file_cache(ctx, self.outputs.as_slice())
                        .await?;
                    if let Some((outputs, metadata)) = lookup {
                        return Ok(ExecuteResult::LocalDepFileHit(outputs, metadata));
                    }
                }

                let supports_remote_dep_files =
                    self.inner.allow_dep_file_cache_upload && dep_file_bundle.has_dep_files();

                // Enable remote dep file cache lookup for actions that have remote depfile uploads enabled.
                if supports_remote_dep_files {
                    let remote_dep_file_key = dep_file_bundle
                        .remote_dep_file_action(
                            ctx.digest_config(),
                            ctx.mergebase().0.as_ref(),
                            ctx.re_platform(),
                        )
                        .action
                        .coerce();
                    let req = req.with_remote_dep_file_key(&remote_dep_file_key);
                    let remote_dep_file_result = ctx
                        .remote_dep_file_cache(manager, &req, &prepared_action)
                        .await;
                    if let ControlFlow::Break(res) = remote_dep_file_result {
                        // If the result was served by the remote dep file cache, we can't use the result just yet. We need to verify that
                        // the inputs tracked by a depfile that was actually used in the cache hit are identical to the inputs we have for this action.
                        let res = self
                            .check_cache_result_is_useable(
                                ctx,
                                &req,
                                &prepared_action.action_and_blobs.action,
                                res,
                                &dep_file_bundle,
                                &remote_dep_file_key,
                            )
                            .await?;
                        (
                            req,
                            res.map_continue(|_| ctx.command_execution_manager(WaitingData::new())),
                        )
                    } else {
                        (req, remote_dep_file_result)
                    }
                } else {
                    (req, ControlFlow::Continue(manager))
                }
            }
        };

        // If the cache queries did not yield to a result, then we need to execute the action.
        let (result, req, action_and_blobs) = match result {
            ControlFlow::Break(res) => (res, req, prepared_action.action_and_blobs),
            ControlFlow::Continue(mut manager) => {
                manager
                    .inner
                    .waiting_data
                    .start_waiting_category_now(WaitingCategory::Unknown);
                let (req, prepared_action) = if self.inner.incremental_remote_outputs {
                    // For the case of incremental remote outputs, we checked the caches using the action which
                    // does not include the outputs as inputs.
                    // To execute such action we first prepare a different action with the outputs added as inputs.
                    let output_paths_as_inputs = self.output_paths_as_inputs(ctx).await?;
                    if !output_paths_as_inputs.is_empty() {
                        let executor_fs = ctx.executor_fs();
                        let fs = executor_fs.fs();
                        let digest_config = ctx.digest_config();
                        let override_req = req.with_outputs_paths_added_as_inputs(
                            output_paths_as_inputs,
                            fs,
                            digest_config,
                            ctx.run_action_knobs().action_paths_interner.as_ref(),
                        )?;
                        let override_prepared_action = ctx.prepare_action(&override_req, true)?;
                        (override_req, override_prepared_action)
                    } else {
                        (req, prepared_action)
                    }
                } else {
                    (req, prepared_action)
                };
                let execution_result = ctx.exec_cmd(manager, &req, &prepared_action).await;
                (execution_result, req, prepared_action.action_and_blobs)
            }
        };

        Ok(ExecuteResult::ExecutedOrReHit {
            result,
            dep_file_bundle,
            executor_preference: req.executor_preference,
            action_and_blobs,
            input_files_bytes: req.paths().input_files_bytes(),
        })
    }

    async fn output_paths_as_inputs(
        &self,
        ctx: &dyn ActionExecutionCtx,
    ) -> buck2_error::Result<Vec<CommandExecutionInput>> {
        let executor_fs = ctx.executor_fs();
        let fs = executor_fs.fs();
        let output_paths = {
            let mut output_paths = Vec::new();
            for output in &self.outputs {
                // TODO(T219919866): support content based paths
                let path = fs.resolve_build(output.get_path(), None)?;
                output_paths.push(path);
            }
            output_paths
        };
        let entries = ctx
            .materializer()
            .get_artifact_entries_for_materialized_paths(output_paths, false)
            .await?;
        // Only proceed with incremental outputs if every output is present
        Ok(entries
            .into_iter()
            .map(|entry| entry.map(|(p, e)| CommandExecutionInput::IncrementalRemoteOutput(p, e)))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default())
    }

    fn command_execution_request(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        prepared_run_action: PreparedRunAction,
        host_sharing_requirements: HostSharingRequirements,
    ) -> buck2_error::Result<CommandExecutionRequest> {
        let outputs_for_error_handler = self.outputs_for_error_handler()?;
        let mut req = prepared_run_action
            .into_command_execution_request()
            .with_prefetch_lossy_stderr(true)
            .with_executor_preference(self.inner.executor_preference)
            .with_host_sharing_requirements(host_sharing_requirements.into())
            .with_low_pass_filter(self.inner.low_pass_filter)
            .with_outputs_cleanup(!self.inner.no_outputs_cleanup)
            .with_local_environment_inheritance(EnvironmentInheritance::local_command_exclusions())
            .with_force_full_hybrid_if_capable(self.inner.force_full_hybrid_if_capable)
            .with_unique_input_inodes(self.inner.unique_input_inodes)
            .with_remote_execution_dependencies(self.inner.remote_execution_dependencies.to_vec())
            .with_re_gang_workers(self.inner.re_gang_workers.to_vec())
            .with_remote_execution_custom_image(
                self.inner.remote_execution_custom_image.clone().map(|s| *s),
            )
            .with_meta_internal_extra_params(self.inner.meta_internal_extra_params.clone())
            .with_outputs_for_error_handler(outputs_for_error_handler);

        if let Some(timeout) = self.inner.timeout {
            req = req.with_timeout(timeout);
        }

        if self.inner.no_outputs_cleanup {
            if self
                .outputs
                .iter()
                .any(|o| o.get_path().is_content_based_path())
            {
                req = req.with_run_action_key(Some(
                    // Using string representation as it is going to be stored in db which requires it to be a string
                    // doing it early here prevents us from exposing RunActionKey type
                    RunActionKey::from_action_execution_target(ctx.target())
                        .incremental_state_key(),
                ));
            }
        }

        Ok(req)
    }
}

pub(crate) struct PreparedRunAction {
    expanded: ExpandedCommandLine,
    /// Environment which is added on top of the one coming from `ExpandedCommandLine::env`
    extra_env: Vec<(String, String)>,
    paths: CommandExecutionPaths,
    worker: Option<WorkerSpec>,
    remote_worker: Option<RemoteWorkerSpec>,
    salt: Option<ActionSalt>,
}

impl PreparedRunAction {
    fn into_command_execution_request(self) -> CommandExecutionRequest {
        let Self {
            expanded: ExpandedCommandLine { exe, args, env },
            extra_env,
            paths,
            worker,
            remote_worker,
            salt,
        } = self;

        let env = if extra_env.is_empty() {
            env
        } else {
            env.into_iter().chain(extra_env).collect()
        };

        CommandExecutionRequest::new(exe, args, paths, env)
            .with_worker(worker)
            .with_remote_worker(remote_worker)
            .with_salt(salt)
    }
}

pub struct RunActionVisitor<'a> {
    pub(crate) dep_files_visitor: DepFilesCommandLineVisitor<'a>,
    pub(crate) incremental_metadata_inputs: Vec<ArtifactGroup>,
    incremental_metadata_ignore_tags: Option<&'a BuckFixedSortedSet<ArtifactTag>>,
    /// Built but never materialized; folded into the action key as a salt.
    digest_inputs: BuckMutInsertionSet<ArtifactGroup>,
}

impl<'a> RunActionVisitor<'a> {
    pub(crate) fn new(
        dep_files: &'a RunActionDepFiles,
        incremental_metadata_ignore_tags: Option<&'a BuckFixedSortedSet<ArtifactTag>>,
    ) -> Self {
        Self {
            dep_files_visitor: DepFilesCommandLineVisitor::new(dep_files),
            incremental_metadata_inputs: Vec::new(),
            incremental_metadata_ignore_tags,
            digest_inputs: BuckMutInsertionSet::default(),
        }
    }

    pub(crate) fn inputs(&self) -> impl Iterator<Item = &ArtifactGroup> {
        self.dep_files_visitor.inputs()
    }
}

impl<'v> CommandLineArtifactVisitor<'v> for RunActionVisitor<'v> {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        props: InputProperties,
        tags: Vec<&ArtifactTag>,
    ) {
        let InputProperties {
            hidden: _,
            requires,
        } = props;
        match requires {
            Requires::Contents => {}
            // In no dep file and no incremental metadata; the rendered path carries its content.
            Requires::Path => return,
            Requires::Digest => {
                self.digest_inputs.insert(input);
                return;
            }
        }

        // If incremental_metadata_ignore_tags is None, then we're not going to produce
        // incremental metadata at all, so there's nothing to do here.
        if let Some(ignore_tags) = self.incremental_metadata_ignore_tags {
            if !tags.iter().any(|t| ignore_tags.contains(*t)) {
                self.incremental_metadata_inputs.push(input.dupe());
            }
        }

        self.dep_files_visitor.visit_input(input, props, tags);
    }

    fn visit_declared_output(&mut self, _artifact: OutputArtifact<'v>, tags: Vec<&ArtifactTag>) {
        self.dep_files_visitor
            .visit_declared_output(_artifact, tags);
    }

    fn visit_frozen_output(&mut self, artifact: Artifact, tags: Vec<&ArtifactTag>) {
        self.dep_files_visitor.visit_frozen_output(artifact, tags);
    }
}

impl RunAction {
    /// Execute for offline builds by restoring from cache.
    /// Returns None if cache miss, Some if hit.
    async fn execute_for_offline(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
    ) -> buck2_error::Result<Option<(ActionOutputs, ActionExecutionMetadata)>> {
        // Collect references to all outputs
        let output_refs: Vec<&BuildArtifact> = self.outputs.iter().collect();

        // Try to restore ALL outputs - any miss = total miss
        match offline::declare_copy_from_offline_cache(ctx, &output_refs).await {
            Ok(outputs) => Ok(Some((
                outputs,
                ActionExecutionMetadata {
                    dep_file_db_writes_queued: 0,
                    execution_kind: ActionExecutionKind::Deferred,
                    timing: ActionExecutionTimingData::default(),
                    input_files_bytes: None,
                    waiting_data: WaitingData::new(),
                },
            ))),
            Err(_) => {
                // Cache miss - return None to fall through to normal execution
                Ok(None)
            }
        }
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for RunAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::Run
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        let mut artifact_visitor = SimpleCommandLineArtifactVisitor::new();
        self.visit_artifacts(&mut artifact_visitor)?;
        Ok(Cow::Owned(artifact_visitor.inputs.into_iter().collect()))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(self.outputs.as_slice())
    }

    fn first_output(&self) -> &BuildArtifact {
        // Required to have outputs on construction
        &self.outputs.as_slice()[0]
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new(self.values().category.as_str())
    }

    fn identifier(&self) -> Option<&str> {
        self.values().identifier.map(|x| x.as_str())
    }

    fn always_print_stderr(&self) -> bool {
        self.inner.always_print_stderr
    }

    fn is_expected_eligible_for_dedupe(&self) -> Option<bool> {
        self.inner.expected_eligible_for_dedupe
    }

    fn aquery_attributes(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        let values = Self::unpack(self.values())?;
        let mut cli_rendered = Vec::<String>::new();
        let mut fmt = CommandLineBuilder::new(&mut cli_rendered, artifact_path_mapping, fs);
        values.exe.add_to_command_line(&mut fmt)?;
        values.args.add_to_command_line(&mut fmt)?;
        let cmd = format!("[{}]", cli_rendered.iter().join(", "));
        Ok(buck_mut_insertion_map! {
            "cmd".to_owned() => cmd,
            "executor_preference".to_owned() => self.inner.executor_preference.to_string(),
            "always_print_stderr".to_owned() => self.inner.always_print_stderr.to_string(),
            "weight".to_owned() => self.inner.weight.to_string(),
            "dep_files".to_owned() => self.inner.dep_files.to_string(),
            "metadata_param".to_owned() => match &self.inner.metadata_param {
                None => "None".to_owned(),
                Some(x) => x.to_string(),
            },
            "no_outputs_cleanup".to_owned() => self.inner.no_outputs_cleanup.to_string(),
            "allow_cache_upload".to_owned() => match &self.inner.allow_cache_upload {
                None => "None".to_owned(),
                Some(x) => x.to_string(),
            },
            "allow_dep_file_cache_upload".to_owned() => self.inner.allow_dep_file_cache_upload.to_string(),
        })
    }

    fn error_handler(&self) -> Option<&OwnedFrozen<Value<'static>>> {
        self.error_handler.as_ref()
    }

    fn outputs_for_error_handler(&self) -> buck2_error::Result<Vec<BuildArtifactPath>> {
        self.values()
            .outputs_for_error_handler
            .iter()
            .map(|artifact| {
                let a = artifact.inner().artifact();

                match a.as_parts().0 {
                    BaseArtifactKind::Source(s) => Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Cannot use source artifact `{}` as output for error handler",
                        s.get_path()
                    )),
                    BaseArtifactKind::Build(b) => Ok(b.get_path().dupe()),
                }
            })
            .collect()
    }

    fn failed_action_output_artifacts<'v>(
        &self,
        artifact_fs: &ArtifactFs,
        heap: Heap<'v>,
        outputs: Option<&ActionOutputs>,
    ) -> buck2_error::Result<Option<OutputArtifacts<'v>>> {
        let values = self.values();
        let mut artifact_value_dict = Vec::with_capacity(values.outputs_for_error_handler.len());

        for x in values.outputs_for_error_handler.iter() {
            let artifact = x.inner().artifact();

            let content_based_path_hash = if artifact.path_resolution_requires_artifact_value() {
                let outputs = outputs.ok_or_else(|| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Action failed with no outputs available"
                    )
                })?;
                let artifact_value = outputs
                    .get_from_artifact_path(&artifact.get_path())
                    .ok_or_else(|| {
                        buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "ArtifactValue for artifact `{}` was not found in action outputs",
                            artifact.get_path()
                        )
                    })?;
                Some(artifact_value.content_based_path_hash())
            } else {
                None
            };

            let path = artifact
                .get_path()
                .resolve(artifact_fs, content_based_path_hash.as_ref())?;

            let abs = artifact_fs.fs().resolve(&path);
            // Check if the output file specified exists. We will return an error if it doesn't
            if !fs_util::try_exists(&abs)? {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Output '{}' defined for error handler does not exist. This is likely due to file not being created, please ensure the action would produce an output",
                    &path
                ));
            }

            let artifact_value = StarlarkArtifactValue::new(
                artifact.dupe(),
                path.to_owned(),
                artifact_fs.fs().dupe(),
            );
            let artifact = StarlarkArtifact::new(artifact);

            artifact_value_dict.push((artifact, artifact_value));
        }

        Ok(Some(ValueOf::alloc_dict(
            heap,
            artifact_value_dict.into_iter().map(|(artifact, value)| {
                (
                    ValueOf::from(heap.alloc_typed(artifact)),
                    ValueOf::from(heap.alloc_typed(value)),
                )
            }),
        )))
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        // Check offline cache first if parameter enabled
        if self.inner.allow_offline_output_cache
            && ctx.run_action_knobs().use_network_action_output_cache
        {
            if let Some((outputs, metadata)) = self.execute_for_offline(ctx).await? {
                return Ok((outputs, metadata));
            }
            // Cache miss - fall through to normal execution
        }

        let (
            mut result,
            mut dep_file_bundle,
            executor_preference,
            action_and_blobs,
            input_files_bytes,
        ) = match self.execute_inner(ctx, waiting_data).await? {
            ExecuteResult::LocalDepFileHit(outputs, metadata) => {
                return Ok((outputs, metadata));
            }
            ExecuteResult::ExecutedOrReHit {
                result,
                dep_file_bundle,
                executor_preference,
                action_and_blobs,
                input_files_bytes,
            } => (
                result,
                dep_file_bundle,
                executor_preference,
                action_and_blobs,
                input_files_bytes,
            ),
        };

        let allow_cache_upload = self
            .inner
            .allow_cache_upload
            .unwrap_or_else(|| ctx.run_action_knobs().default_allow_cache_upload);
        let supports_remote_dep_files =
            self.inner.allow_dep_file_cache_upload && dep_file_bundle.has_dep_files();
        let incremental_kind = match (
            self.inner.no_outputs_cleanup,
            self.inner.incremental_remote_outputs,
        ) {
            (true, true) => buck2_data::IncrementalKind::IncrementalLocalAndRemote,
            (false, true) => buck2_data::IncrementalKind::IncrementalRemote,
            (true, false) => buck2_data::IncrementalKind::IncrementalLocal,
            (false, false) => buck2_data::IncrementalKind::NonIncremental,
        };

        let allow_cache_upload = allow_cache_upload || force_cache_upload()?;
        // If there is a dep file entry AND if dep file cache upload is enabled, upload it
        if result.was_success()
            && !result.was_served_by_remote_dep_file_cache()
            && (allow_cache_upload || supports_remote_dep_files)
        {
            let mut re_result = result.action_result.take();
            // TODO(jtbraun): D121882458 moves this upload off the action's critical path. Worth
            // adopting if missing entries, e.g. after a cache bust, make these uploads visible in
            // build wall time; its in-flight limit has to rise first or it drops most seeding uploads.
            let upload_result = ctx
                .cache_upload(
                    &action_and_blobs,
                    &result,
                    re_result.as_mut(),
                    // match needed for coercion, https://github.com/rust-lang/rust/issues/108999
                    if supports_remote_dep_files {
                        Some(&mut dep_file_bundle)
                    } else {
                        None
                    },
                    allow_cache_upload,
                )
                .await?;

            result.cache_upload_result = upload_result.cache_upload_outcome.to_proto();
            result.dep_file_cache_upload_result = if supports_remote_dep_files {
                upload_result.dep_file_cache_upload_outcome.to_proto()
            } else if dep_file_bundle.has_dep_files() {
                buck2_data::UploadResult::DepFileUploadNotAllowed
            } else {
                buck2_data::UploadResult::NotAttempted
            };
            result.dep_file_key = upload_result.dep_file_cache_upload_key;
        } else if !result.was_success() {
            result.cache_upload_result = buck2_data::UploadResult::ActionNotSuccessful;
        } else if result.was_served_by_remote_dep_file_cache() {
            result.cache_upload_result = buck2_data::UploadResult::RemoteDepFileCacheHit;
        } else if !allow_cache_upload {
            result.cache_upload_result = buck2_data::UploadResult::ActionUploadNotAllowed;
            if dep_file_bundle.has_dep_files() {
                result.dep_file_cache_upload_result =
                    buck2_data::UploadResult::DepFileUploadNotAllowed;
            }
        }

        let was_locally_executed = result.was_locally_executed();
        let (outputs, mut metadata) = ctx.unpack_command_execution_result(
            executor_preference,
            result,
            allow_cache_upload,
            self.inner.allow_dep_file_cache_upload,
            Some(input_files_bytes),
            incremental_kind,
        )?;

        // Cache outputs if tracing and parameter enabled
        if self.inner.allow_offline_output_cache {
            let io_provider = ctx.io_provider();
            if let Some(tracer) = TracingIoProvider::from_io(io_provider) {
                for output in self.outputs.iter() {
                    if let Some(value) = outputs.get(output.get_path()) {
                        offline::declare_copy_to_offline_output_cache(
                            ctx,
                            tracer,
                            output,
                            value.dupe(),
                        )
                        .await?;
                    }
                }
            }
        }

        let queued_write =
            populate_dep_files(ctx, dep_file_bundle, &outputs, was_locally_executed).await?;
        metadata.dep_file_db_writes_queued = u64::from(queued_write);

        Ok((outputs, metadata))
    }
}

#[cfg(test)]
mod tests {
    use buck2_artifact::actions::key::ActionIndex;
    use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
    use buck2_build_api::interpreter::rule_defs::cmd_args::register_cmd_args;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
    use buck2_interpreter::factory::BuckStarlarkModule;
    use buck2_interpreter::factory::StarlarkEvaluatorProvider;
    use dice_futures::cancellation::CancellationContext;
    use starlark::environment::GlobalsBuilder;
    use starlark::syntax::AstModule;
    use starlark::syntax::Dialect;
    use starlark::values::UnpackValue;

    use super::*;

    fn check_skip_hidden_visitor(tag_method: &str) -> buck2_error::Result<()> {
        BuckStarlarkModule::with_profiling(|module| {
            let target = ConfiguredTargetLabel::testing_parse(
                "cell//pkg:target",
                ConfigurationData::testing_new(),
            );
            let [visible, hidden] = ["visible", "hidden"].map(|path| {
                Artifact::from(BuildArtifact::testing_new(
                    target.dupe(),
                    path,
                    ActionIndex::new(0),
                ))
            });
            module.set(
                "visible",
                module.heap().alloc(StarlarkArtifact::new(visible.dupe())),
            );
            module.set(
                "hidden",
                module.heap().alloc(StarlarkArtifact::new(hidden.dupe())),
            );
            module.set("tag", module.heap().alloc(ArtifactTag::testing_new()));
            let globals = GlobalsBuilder::standard().with(register_cmd_args).build();
            let ast = AstModule::parse(
                "test.bzl",
                format!("args = tag.{tag_method}(cmd_args(visible, hidden = hidden))"),
                &Dialect::Standard,
            )?;
            let provider = StarlarkEvaluatorProvider::passthrough(StarlarkEvalKind::Unknown(
                tag_method.into(),
            ));
            let (finished_eval, ()) = provider.with_evaluator(
                &module,
                CancellationContext::testing().into(),
                |eval, _| {
                    eval.eval_module(ast, &globals)?;
                    Ok(())
                },
            )?;
            let (profiling, module, _) = finished_eval.freeze_and_finish(module)?;
            let args = module.get("args").expect("args should be defined");
            let args = ValueAsCommandLineLike::unpack_value_err(args.as_ref().value())?;

            let mut path_visitor = SkipHiddenCommandLineArtifactVisitor::new();
            args.0.visit_artifacts(&mut path_visitor)?;
            assert_eq!(
                path_visitor.inputs.into_iter().collect::<Vec<_>>(),
                vec![ArtifactGroup::Artifact(visible.dupe())],
            );

            let mut dependency_visitor = SimpleCommandLineArtifactVisitor::new();
            args.0.visit_artifacts(&mut dependency_visitor)?;
            assert_eq!(
                dependency_visitor.inputs.into_iter().collect::<Vec<_>>(),
                vec![
                    ArtifactGroup::Artifact(visible),
                    ArtifactGroup::Artifact(hidden)
                ],
            );
            Ok((profiling, ()))
        })
    }

    #[test]
    fn test_skip_hidden_visitor_tag_artifacts() -> buck2_error::Result<()> {
        check_skip_hidden_visitor("tag_artifacts")
    }

    #[test]
    fn test_skip_hidden_visitor_tag_inputs() -> buck2_error::Result<()> {
        check_skip_hidden_visitor("tag_inputs")
    }
}
