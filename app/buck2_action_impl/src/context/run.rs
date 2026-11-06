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
use std::time::Duration;

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::ArtifactErrors;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_build_api::actions::ActionName;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_build_api::interpreter::rule_defs::cmd_args::Requires;
use buck2_build_api::interpreter::rule_defs::cmd_args::SimpleCommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCommandLineValueUnpack;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_build_api::interpreter::rule_defs::command_executor_config::parse_custom_re_image;
use buck2_build_api::interpreter::rule_defs::command_executor_config::parse_meta_internal_extra_params;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::provider::builtin::run_info::RunInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::worker_run_info::WorkerRunInfo;
use buck2_core::category::CategoryRef;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::execution_types::executor_config::ReGangWorker;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use dupe::Dupe;
use either::Either;
use host_sharing::WeightClass;
use host_sharing::WeightPercentage;
use mini_vec::MiniBoxSlice;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::StringValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::list::ListType;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
use starlark::values::typing::StarlarkCallable;
use starlark_map::small_map;

use crate::actions::impls::dep_file_fingerprint::StarlarkDepFileFingerprint;
use crate::actions::impls::run::MetadataParameter;
use crate::actions::impls::run::StarlarkRunActionValues;
use crate::actions::impls::run::UnregisteredRunAction;
use crate::actions::impls::run::dep_files::RunActionDepFiles;
use crate::actions::impls::run::new_executor_preference;
use crate::context::execution_group::action_execution_group;
use crate::context::execution_group::check_default_execution_group_declared;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum RunActionError {
    #[error("expected at least one output artifact, did not get any")]
    NoOutputsSpecified,
    #[error("`weight` must be a positive integer, got `{0}`")]
    InvalidWeight(u32),
    #[error("`timeout_seconds` must be a positive integer, got `{0}`")]
    InvalidTimeout(u32),
    #[error("`weight` and `weight_percentage` cannot both be passed")]
    DuplicateWeightsSpecified,
    #[error("`dep_files` value with key `{}` has an invalid count of associated outputs. Expected 1, got {}.", .key, .count)]
    InvalidDepFileOutputs { key: String, count: usize },
    #[error("`dep_files` with keys `{}` and `{}` are using the same tag", .first, .second)]
    ConflictingDepFiles { first: String, second: String },
    #[error("Dep-files input `{}` is tagged with multiple tags relevant for dep-files: `{}` and `{}`", .input, .tags[0], .tags[1])]
    ConflictingDepFileInputTags {
        input: ArtifactGroup,
        tags: Vec<String>,
    },
    #[error(
        "missing `metadata_path` parameter which is required when `metadata_env_var` parameter is present"
    )]
    MetadataPathMissing,
    #[error(
        "missing `metadata_env_var` parameter which is required when `metadata_path` parameter is present"
    )]
    MetadataEnvVarMissing,
    #[error(
        "Recursion limit exceeded when visiting artifacts: do you have a cycle in your inputs or outputs?"
    )]
    ArtifactVisitRecursionLimitExceeded,
    #[error(
        "`{}` was marked to be materialized on failure but is not declared as an output of the action.", .path
    )]
    FailedActionArtifactNotDeclared { path: String },
    #[error(
        "Action is marked with `incremental_remote_outputs` but output `{}` is content-based, which is not allowed.", .path
    )]
    IncrementalRemoteOutputsWithContentBasedOutputs { path: String },
    #[error(
        "Action is marked with `incremental_remote_outputs` but not `no_outputs_cleanup`, which is not allowed."
    )]
    IncrementalRemoteOutputsWithoutNoOutputsCleanup,
    #[error(
        "Action is marked with `expect_eligible_for_dedupe` but output `{}` is not content-based", .path
    )]
    ExpectEligibleForDedupeWithNonContentBasedOutput { path: String },
    #[error(
        "Action is marked with `expect_eligible_for_dedupe` but input `{}` is not eligible for dedupe", .input
    )]
    ExpectEligibleForDedupeWithIneligibleInput { input: ArtifactGroup },
}

#[starlark_module]
pub(crate) fn analysis_actions_methods_run(methods: &mut MethodsBuilder) {
    /// Run a command to produce one or more artifacts.
    ///
    /// * `arguments`: must be of type `cmd_args`, or a type convertible to such (such as a list of
    ///   strings and artifacts). See below for detailed description of artifact arguments.
    /// * `env`: environment variables to set when the command is executed.
    /// * `category`: category and identifier - when used together, identify the action in Buck2's
    ///   event stream, and must be unique for a given target
    /// * `weight`: used to note how heavy the command is and will typically be set to a higher
    ///   value to indicate that less such commands should be run in parallel (if running locally)
    /// * `no_outputs_cleanup`: if this flag is set then Buck2 won't clean the outputs of a previous
    ///   build that might be present on a disk; in which case, command from arguments should be
    ///   responsible for the cleanup (that is useful, for example, when an action is supporting
    ///   incremental mode and its outputs are based on result from a previous build)
    /// * `metadata_env_var` and `meadata_path` should be used together: both set or both unset
    ///     * `metadata_path`: defines a path relative to the result directory for a file with
    ///       action metadata, which will be created right before the command will be run.
    ///     * Metadata contains the path relative to the Buck2 project root and hash digest for
    ///       every action input (this excludes symlinks as they could be resolved by a user script
    ///       if needed). The resolved path relative to the Buck2 project for the metadata file will
    ///       be passed to command from arguments, via the environment variable, with its name set
    ///       by `metadata_env_var`
    ///     * Both `metadata_env_var` and `metadata_path` are useful when making actions behave in
    ///       an incremental manner (for details, see [Incremental
    ///       Actions](https://buck2.build/docs/rule_authors/incremental_actions/))
    /// * `dep_files`: a dictionary mapping labels to `ArtifactTag` instances for tracking actual
    ///   dependencies via dependency files (depfiles). This enables precise incremental builds by
    ///   allowing the build tool to report which inputs it actually used.
    ///     * Each entry maps a string label (e.g., `"headers"`) to an `ArtifactTag` created via
    ///       `ctx.actions.artifact_tag()`
    ///     * The tag should be used to mark both the potential inputs (via `tag.tag_artifacts()`)
    ///       and the depfile output that will list the actual inputs used
    ///     * After execution, Buck2 reads the depfile and only tracks changes to inputs listed in it,
    ///       rather than all tagged inputs
    ///     * Depfiles must use Makefile syntax: `output: input1 input2 input3`
    ///     * For complete documentation and examples, see [`ctx.actions.artifact_tag()`](../AnalysisActions#analysisactionsartifact_tag)
    /// * `dep_file_fingerprints`: fingerprints returned alongside artifacts by `write` or
    ///   `write_json` with `dep_files_fingerprint_using_canonical_paths = True`. Their canonical contents contribute to
    ///   dep-file matching, and their referenced inputs are visited with their original tags.
    ///   Supply each corresponding artifact to the command separately, tagged for dep-file
    ///   filtering. Include fingerprints for nested files explicitly; they are not
    ///   discovered through command arguments.
    /// * `allow_offline_output_cache`: enables caching of this action's outputs for offline builds (default: `false`)
    ///     * When `true`, action outputs are cached during trace builds (via `buck2 debug trace-io`)
    ///       and restored during offline builds without re-executing the action
    ///     * Intended for actions that read from the network (e.g., downloads, remote artifact fetches)
    ///       which cannot execute in offline build environments where network access is restricted
    ///     * During trace builds: outputs are copied to `buck-out/offline-cache/` after successful execution
    ///     * During offline builds: if all outputs exist in offline cache, they are restored without
    ///       running the action; otherwise the action executes normally (graceful fallback)
    ///     * Requires `buck2.use_network_action_output_cache=true` config to take effect
    ///     * Example use case: caching network downloads in containerized offline build environments
    /// * The `prefer_local`, `prefer_remote` and `local_only` options allow selecting where the
    /// action should run if the executor selected for this target is a hybrid executor.
    ///     * All those options disable concurrent execution: the action will run on the preferred
    ///     platform first (concurrent execution only happens with a "full" hybrid executor).
    ///     * Execution may be retried on the "non-preferred" platform if it fails due to a
    ///     transient error, except for `local_only`, which does not allow this.
    ///     * If the executor selected is a remote-only executor and you use `local_only`, that's an
    ///     error. The other options will not raise errors.
    ///     * Setting more than one of those options is an error.
    ///     * Those flags behave the same way as the equivalent `--prefer-remote`, `--prefer-local`
    ///     and `--local-only` CLI flags. The CLI flags take precedence.
    ///     * The `force_full_hybrid_if_capable` option overrides the `use_limited_hybrid` hybrid.
    ///     The options listed above take precedence if set.
    /// * `remote_execution_dependencies`: list of dependencies which is passed to Remote Execution.
    ///   Each dependency is dictionary with the following keys:
    ///     * `smc_tier`: name of the SMC tier to call by RE Scheduler.
    ///     * `id`: name of the dependency.
    /// * `remote_execution_dynamic_image`: a custom Tupperware image which is passed to Remote Execution.
    ///   It takes a dictionary with the following keys:
    ///     * `identifier`: name of the SMC tier to call by RE Scheduler.
    ///         * `name`: name of the image.
    ///         * `uuid`: uuid of the image.
    ///     * `drop_host_mount_globs`: list of strings containing file
    ///     globs. Any mounts globs specified will not be bind mounted
    ///     from the host.
    /// * `timeout_seconds`: an optional timeout for the action, in seconds. If
    ///   the action takes longer than this, it will be cancelled and behave as if
    ///   it has failed. Must be a positive number. The default is no timeout.
    ///     * Use this to abort misbehaving actions (e.g. if an action sometimes
    ///     deadlock). In other words, use this when the build will fail either
    ///     way, and you want to just make it fail faster.
    ///     * Do NOT use  this to attempt to enforce e.g. a runtime policy on a
    ///     specific set of actions: action runtime is often variable so setting
    ///     a timeout to try and enforce a specific runtime goal will inevitably
    ///     result in flaky failures for end users running builds.
    ///  * `meta_internal_extra_params`: a dictionary to pass extra parameters to RE, can add more keys in the future:
    ///     * `remote_execution_policy`: refer to TExecutionPolicy. Keys: `priority`,
    ///       `region_preference`, `setup_preference_key` and `queue_timeout_s` (seconds the action
    ///       may wait in the RE queue; RE caps it at the platform's queue timeout).
    ///  * `error_handler`: an optional function that analyzes action failures and produces structured error information.
    ///     * Type signature: `def error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]`
    ///     * The function receives an [`ActionErrorCtx`](../ActionErrorCtx) parameter and should return a list of [`ActionSubError`](../ActionSubError) objects
    ///     * Error handlers enable better error diagnostics and language-specific error categorization
    ///  * `outputs_for_error_handler`: Output files to be provided to the action error handler and read by
    /// [error handler](https://buck2.build/docs/api/build/ActionErrorCtx/#actionerrorctxoutput_artifacts) in the event of a failure..
    ///     * The output must also be declared as an output of the action
    ///     * The output artifact must be created if the action fails
    ///     * Nothing will be provided if left empty (Which is the default)
    ///
    /// When actions execute, they'll do so from the root of the repository. As they execute,
    /// actions have exclusive access to their output directory.
    ///
    /// Actions also get exclusive access to a "scratch" path that is exposed via the environment
    /// variable `BUCK_SCRATCH_PATH`. This path is expressed as a path relative to the working
    /// directory (i.e. relative to the project). This path is guaranteed to exist when the action
    /// executes.
    ///
    /// When actions run locally, the scratch path is also used as the `TMPDIR`.
    ///
    /// ### Input and output artifacts
    ///
    /// Run action consumes arbitrary number of input artifacts
    /// and produces at least one output artifact.
    ///
    /// Both input and output artifacts can be passed in:
    /// - positional `arguments` parameters
    /// - `env` dict
    ///
    /// Input artifacts must be already bound prior to this call,
    /// meaning these artifacts must be either:
    /// - source artifacts
    /// - coming from dependencies
    /// - declared locally and bound to another action (passed to `.as_output()`)
    ///   *before* this `run()` call
    /// - or created already bound with some simple action like `write()`
    ///
    /// Output artifacts must be declared locally (within the same analysis),
    /// and must not be already bound. Output artifacts become "bound" after this call.
    fn run<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] arguments: StarlarkCommandLineValueUnpack<'v>,
        #[starlark(require = named)] category: StringValue<'v>,
        #[starlark(require = named, default = NoneOr::None)] identifier: NoneOr<StringValue<'v>>,
        #[starlark(require = named)] env: Option<
            ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>,
        >,
        #[starlark(require = named, default = false)] local_only: bool,
        #[starlark(require = named, default = false)] prefer_local: bool,
        #[starlark(require = named, default = false)] prefer_remote: bool,
        #[starlark(require = named, default = true)] low_pass_filter: bool,
        #[starlark(require = named, default = false)] always_print_stderr: bool,
        #[starlark(require = named)] weight: Option<u32>,
        #[starlark(require = named)] weight_percentage: Option<u32>,
        #[starlark(require = named)] dep_files: Option<
            ValueOf<'v, DictType<&'v str, &'v ArtifactTag>>,
        >,
        #[starlark(require = named, default = ValueOf::empty_list())]
        dep_file_fingerprints: ValueOf<
            'v,
            ListOrTupleType<ValueTyped<'v, StarlarkDepFileFingerprint<'v>>>,
        >,
        #[starlark(require = named)] metadata_env_var: Option<String>,
        #[starlark(require = named)] metadata_path: Option<String>,
        #[starlark(require = named, default = ValueOf::empty_list())]
        incremental_metadata_ignore_tags: ValueOf<'v, ListOrTupleType<&'v ArtifactTag>>,
        // TODO(scottcao): Refactor `no_outputs_cleanup` to `outputs_cleanup`
        #[starlark(require = named, default = false)] no_outputs_cleanup: bool,
        #[starlark(require = named, default = false)] incremental_remote_outputs: bool,
        #[starlark(require = named, default = NoneOr::None)] allow_cache_upload: NoneOr<bool>,
        #[starlark(require = named, default = false)] allow_dep_file_cache_upload: bool,
        #[starlark(require = named, default = false)] allow_offline_output_cache: bool,
        #[starlark(require = named, default = false)] force_full_hybrid_if_capable: bool,
        #[starlark(require = named)] exe: Option<
            Either<ValueTyped<'v, WorkerRunInfo<'v>>, ValueTyped<'v, RunInfo<'v>>>,
        >,
        #[starlark(require = named, default = false)] unique_input_inodes: bool,
        #[starlark(require = named, default = NoneOr::None)] error_handler: NoneOr<
            StarlarkCallable<'v>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
        #[starlark(require = named, default = ValueOf::empty_list())]
        remote_execution_dependencies: ValueOf<
            'v,
            ListType<ValueOf<'v, DictType<&'v str, &'v str>>>,
        >,
        #[starlark(require = named, default = ValueOf::empty_list())] re_gang_workers: ValueOf<
            'v,
            ListType<ValueOf<'v, DictType<&'v str, &'v str>>>,
        >,
        #[starlark(default = NoneType, require = named)] remote_execution_dynamic_image: Value<'v>,
        #[starlark(require = named, default = NoneOr::None)] timeout_seconds: NoneOr<u32>,
        #[starlark(require = named, default = NoneOr::None)] meta_internal_extra_params: NoneOr<
            DictRef<'v>,
        >,
        // Note: Intentionally don't support frozen output artifacts
        #[starlark(require = named, default = ValueOf::empty_list())]
        outputs_for_error_handler: ValueOf<
            'v,
            ListOrTupleType<ValueTyped<'v, StarlarkOutputArtifact<'v>>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] expect_eligible_for_dedupe: NoneOr<
            bool,
        >,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
    ) -> starlark::Result<NoneType> {
        let exec_group = action_execution_group(exec_group);
        if incremental_remote_outputs && !no_outputs_cleanup {
            // Precaution to make sure content-based paths are not involved.
            return Err(buck2_error::Error::from(
                RunActionError::IncrementalRemoteOutputsWithoutNoOutputsCleanup,
            )
            .into());
        }

        struct RunCommandArtifactVisitor<'v> {
            inner: SimpleCommandLineArtifactVisitor<'v>,
            tagged_outputs: BuckMutMap<ArtifactTag, Vec<OutputArtifact<'v>>>,
            depth: u64,
            dep_file_artifact_tags: Option<BuckMutInsertionSet<&'v ArtifactTag>>,
            inputs_with_multiple_tags_for_dep_files: Vec<(ArtifactGroup, Vec<ArtifactTag>)>,
        }

        impl<'v> RunCommandArtifactVisitor<'v> {
            fn new(
                dep_files: Option<ValueOf<'v, DictType<&'v str, &'v ArtifactTag>>>,
            ) -> buck2_error::Result<Self> {
                let dep_file_artifact_tags = if let Some(dep_files) = dep_files {
                    let mut tags = BuckMutInsertionSet::with_capacity(dep_files.dict().len());
                    for entry in dep_files.try_items() {
                        let (_key, tag) = entry?;
                        tags.insert(tag);
                    }
                    Some(tags)
                } else {
                    None
                };
                Ok(Self {
                    inner: SimpleCommandLineArtifactVisitor::new(),
                    tagged_outputs: BuckMutMap::default(),
                    depth: 0,
                    dep_file_artifact_tags,
                    inputs_with_multiple_tags_for_dep_files: Vec::new(),
                })
            }
        }

        impl<'v> CommandLineArtifactVisitor<'v> for RunCommandArtifactVisitor<'v> {
            fn visit_input(
                &mut self,
                input: ArtifactGroup,
                props: InputProperties,
                tags: Vec<&ArtifactTag>,
            ) {
                let InputProperties { hidden, requires } = props;
                match requires {
                    Requires::Contents => {}
                    Requires::Path if hidden => return,
                    Requires::Path => {}
                    Requires::Digest => {}
                }
                if let Some(ref dep_file_artifact_tags) = self.dep_file_artifact_tags {
                    let dep_file_tags: Vec<&ArtifactTag> = tags
                        .iter()
                        .filter_map(|t| {
                            if dep_file_artifact_tags.contains(*t) {
                                Some(*t)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if dep_file_tags.len() > 1 {
                        self.inputs_with_multiple_tags_for_dep_files.push((
                            input.dupe(),
                            dep_file_tags.into_iter().map(|t| t.dupe()).collect(),
                        ));
                    }
                }
                self.inner.visit_input(input, props, tags);
            }

            fn visit_declared_output(
                &mut self,
                artifact: OutputArtifact<'v>,
                tags: Vec<&ArtifactTag>,
            ) {
                for tag in tags.iter() {
                    self.tagged_outputs
                        .entry((*tag).dupe())
                        .or_default()
                        .push(artifact.dupe());
                }

                self.inner.visit_declared_output(artifact, tags);
            }

            fn visit_frozen_output(&mut self, artifact: Artifact, tags: Vec<&ArtifactTag>) {
                self.inner.visit_frozen_output(artifact, tags)
            }

            fn push_frame(&mut self) -> buck2_error::Result<()> {
                self.depth += 1;
                if self.depth > 1000 {
                    return Err(RunActionError::ArtifactVisitRecursionLimitExceeded.into());
                }
                Ok(())
            }

            fn pop_frame(&mut self) {
                self.depth = self.depth.saturating_sub(1);
            }
        }

        let executor_preference = new_executor_preference(local_only, prefer_local, prefer_remote)?;

        let mut artifact_visitor = RunCommandArtifactVisitor::new(dep_files)?;

        let starlark_args = StarlarkCmdArgs::try_from_value_typed(arguments)?;
        starlark_args.visit_artifacts(&mut artifact_visitor)?;
        let dep_file_fingerprints = dep_file_fingerprints.try_to_vec()?;
        for fingerprint in &dep_file_fingerprints {
            fingerprint.as_ref().visit_inputs(&mut artifact_visitor)?;
        }

        // TODO(nga): we should not accept output artifacts in worker.
        let (starlark_exe, starlark_worker, starlark_remote_worker) = match exe {
            Some(Either::Left(worker_run)) => {
                let worker = worker_run.worker();
                let remote_worker = worker_run.remote_worker();
                let starlark_exe = StarlarkCmdArgs::try_from_value(worker_run.exe().to_value())?;
                starlark_exe.visit_artifacts(&mut artifact_visitor)?;
                (starlark_exe, worker, remote_worker)
            }
            Some(Either::Right(exe)) => {
                let starlark_exe = StarlarkCmdArgs::try_from_value(exe.to_value())?;
                starlark_exe.visit_artifacts(&mut artifact_visitor)?;
                (starlark_exe, None, None)
            }
            None => (StarlarkCmdArgs::default(), None, None),
        };

        let weight = match (weight, weight_percentage) {
            (None, None) => WeightClass::Permits(1),
            (Some(v), None) => {
                if v == 0 {
                    return Err(buck2_error::Error::from(RunActionError::InvalidWeight(v)).into());
                }
                WeightClass::Permits(v.try_into().unwrap()) // We don't support < 32 bit platforms.
            }
            (None, Some(v)) => WeightClass::Percentage(
                WeightPercentage::try_new(v)
                    .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
                    .buck_error_context("Invalid `weight_percentage`")?,
            ),
            (Some(..), Some(..)) => {
                return Err(
                    buck2_error::Error::from(RunActionError::DuplicateWeightsSpecified).into(),
                );
            }
        };

        let starlark_env = match env {
            None => None,
            Some(env) => {
                for entry in env.iter() {
                    entry?.1.unpack().0.visit_artifacts(&mut artifact_visitor)?;
                }
                Some(env)
            }
        };

        let RunCommandArtifactVisitor {
            inner: artifacts,
            tagged_outputs,
            inputs_with_multiple_tags_for_dep_files,
            ..
        } = artifact_visitor;

        if let Some(frozen) = { artifacts.frozen_outputs }.pop() {
            return Err(buck2_error::Error::from(ArtifactErrors::DuplicateBind(frozen)).into());
        }

        let mut dep_file_labels: BuckMutInsertionMap<ArtifactTag, Arc<str>> =
            BuckMutInsertionMap::new();

        if let Some(dep_files) = dep_files {
            for entry in dep_files.try_items() {
                let (key, tag) = entry?;
                let tagged = tagged_outputs.get(tag);
                let count = tagged.map_or(0, |t| t.len());

                if count != 1 {
                    return Err(
                        buck2_error::Error::from(RunActionError::InvalidDepFileOutputs {
                            key: (*key).to_owned(),
                            count,
                        })
                        .into(),
                    );
                }

                match dep_file_labels.entry(tag.dupe()) {
                    small_map::Entry::Vacant(v) => {
                        v.insert(Arc::from(key));
                    }
                    small_map::Entry::Occupied(o) => {
                        return Err(buck2_error::Error::from(
                            RunActionError::ConflictingDepFiles {
                                first: (**o.get()).to_owned(),
                                second: (*key).to_owned(),
                            },
                        )
                        .into());
                    }
                }
            }
        }

        if let Some((input, conflicting_tags)) = inputs_with_multiple_tags_for_dep_files.first() {
            return Err(
                buck2_error::Error::from(RunActionError::ConflictingDepFileInputTags {
                    input: input.dupe(),
                    tags: conflicting_tags
                        .iter()
                        .map(|t| (**dep_file_labels.get(t).unwrap()).to_owned())
                        .collect(),
                })
                .into(),
            );
        }

        let metadata_param = match (metadata_env_var, metadata_path) {
            (Some(env_var), Some(path)) => {
                let path: ForwardRelativePathBuf = path.try_into()?;
                this.state()?.claim_output_path(eval, &path)?;
                buck2_error::Ok(Some(Box::new(MetadataParameter {
                    env_var,
                    path,
                    ignore_tags: incremental_metadata_ignore_tags
                        .try_items()
                        .map(|x| buck2_error::Ok(x?.dupe()))
                        .collect::<buck2_error::Result<_>>()?,
                })))
            }
            (Some(_), None) => Err(RunActionError::MetadataPathMissing.into()),
            (None, Some(_)) => Err(RunActionError::MetadataEnvVarMissing.into()),
            (None, None) => Ok(None),
        }?;

        if artifacts.declared_outputs.is_empty() {
            return Err(buck2_error::Error::from(RunActionError::NoOutputsSpecified).into());
        }
        let heap = eval.heap();

        let outputs_for_error_handler = outputs_for_error_handler.try_to_vec()?;
        for o in outputs_for_error_handler.iter() {
            let to_materialize = o.artifact();
            if !artifacts.declared_outputs.contains(&to_materialize) {
                return Err(buck2_error::Error::from(
                    RunActionError::FailedActionArtifactNotDeclared {
                        path: o.to_string(),
                    },
                )
                .into());
            }
        }

        let starlark_values = heap.alloc(StarlarkRunActionValues {
            exe: heap.alloc_typed(starlark_exe),
            args: heap.alloc_typed(starlark_args),
            dep_file_fingerprints: MiniBoxSlice::from(dep_file_fingerprints),
            env: starlark_env,
            worker: starlark_worker,
            remote_worker: starlark_remote_worker,
            category: {
                CategoryRef::new(category.as_str())?;
                category
            },
            identifier: identifier.into_option(),
            outputs_for_error_handler: MiniBoxSlice::from(outputs_for_error_handler),
        });

        let re_dependencies = remote_execution_dependencies
            .try_items()
            .map(|d| {
                RemoteExecutorDependency::parse(d?.try_items().collect::<starlark::Result<_>>()?)
            })
            .collect::<buck2_error::Result<MiniBoxSlice<RemoteExecutorDependency>>>()?;

        let re_gang_workers = re_gang_workers
            .try_items()
            .map(|w| ReGangWorker::parse(w?.try_items().collect::<starlark::Result<_>>()?))
            .collect::<buck2_error::Result<MiniBoxSlice<ReGangWorker>>>()?;

        let re_custom_image = parse_custom_re_image(
            "remote_execution_dynamic_image",
            remote_execution_dynamic_image,
        )?;

        let extra_params =
            parse_meta_internal_extra_params(meta_internal_extra_params.into_option())?;

        let timeout = match timeout_seconds.into_option() {
            Some(t) => {
                if t == 0 {
                    return Err(buck2_error::Error::from(RunActionError::InvalidTimeout(t)).into());
                }
                Some(Duration::from_secs(t.into()))
            }
            None => None,
        };

        if incremental_remote_outputs {
            for o in artifacts.declared_outputs.iter() {
                if o.has_content_based_path() {
                    return Err(buck2_error::Error::from(
                        RunActionError::IncrementalRemoteOutputsWithContentBasedOutputs {
                            path: o.get_path().to_string(),
                        },
                    )
                    .into());
                }
            }
        }

        let action = UnregisteredRunAction {
            executor_preference,
            always_print_stderr,
            weight,
            low_pass_filter,
            dep_files: RunActionDepFiles {
                labels: dep_file_labels.freeze(),
            },
            metadata_param,
            no_outputs_cleanup,
            incremental_remote_outputs,
            allow_cache_upload: allow_cache_upload.into_option(),
            allow_dep_file_cache_upload,
            allow_offline_output_cache,
            force_full_hybrid_if_capable,
            unique_input_inodes,
            remote_execution_dependencies: re_dependencies,
            re_gang_workers,
            remote_execution_custom_image: re_custom_image,
            meta_internal_extra_params: extra_params,
            expected_eligible_for_dedupe: expect_eligible_for_dedupe.into_option(),
            timeout,
        };

        let expect_eligible_for_dedupe = expect_eligible_for_dedupe.into_option().unwrap_or(false);
        if expect_eligible_for_dedupe {
            let deferred_holder_key = &this.state()?.analysis_value_storage.self_key;
            let target_platform = if let BaseDeferredKey::TargetLabel(configured_label) =
                deferred_holder_key.owner()
            {
                Some(configured_label.cfg())
            } else {
                None
            };
            for o in artifacts.declared_outputs.iter() {
                if !o.has_content_based_path() {
                    return Err(buck2_error::Error::from(
                        RunActionError::ExpectEligibleForDedupeWithNonContentBasedOutput {
                            path: o.get_path().to_string(),
                        },
                    )
                    .into());
                }
            }

            for i in artifacts.inputs.iter() {
                if i.is_eligible_for_dedupe(target_platform)
                    == buck2_data::EligibleForDedupe::IneligibleInput
                {
                    return Err(buck2_error::Error::from(
                        RunActionError::ExpectEligibleForDedupeWithIneligibleInput {
                            input: i.dupe(),
                        },
                    )
                    .into());
                }
            }
        }

        check_default_execution_group_declared(this.state()?.execution_platform(), &exec_group)?;

        this.state()?.register_action_in_group(
            artifacts.declared_outputs,
            action,
            Some(starlark_values),
            error_handler.into_option(),
            exec_group,
            || ActionName {
                category: category.as_str().to_owned(),
                identifier: identifier.into_option().map(|i| i.as_str().to_owned()),
            },
        )?;
        Ok(NoneType)
    }
}
