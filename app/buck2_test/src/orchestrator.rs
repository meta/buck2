/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// https://github.com/rust-lang/rust-clippy/issues/12806
#![allow(clippy::unnecessary_to_owned)]

//! Implementation of the `TestOrchestrator` from `buck2_test_api`.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::ffi::OsStr;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::actions::execute::dice_data::CommandExecutorResponse;
use buck2_build_api::actions::execute::dice_data::DiceHasCommandExecutor;
use buck2_build_api::actions::execute::dice_data::GetReClient;
use buck2_build_api::actions::impls::run_action_knobs::HasRunActionKnobs;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::ArtifactGroupValues;
use buck2_build_api::artifact_groups::calculation::ArtifactGroupCalculation;
use buck2_build_api::build_signals::HasBuildSignals;
use buck2_build_api::context::HasBuildContextData;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapperImpl;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::SimpleCommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::SingletonCommandLineSink;
use buck2_build_api::interpreter::rule_defs::command_executor_config::StarlarkCommandExecutorConfig;
use buck2_build_api::interpreter::rule_defs::provider::builtin::external_runner_test_info::ExternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::external_runner_test_info::OwnedExternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestCommandMember;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::InternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::OwnedInternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::local_resource_info::OwnedLocalResourceInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::worker_info::WorkerInfo;
use buck2_build_api::interpreter::rule_defs::required_test_local_resource::StarlarkRequiredTestLocalResource;
use buck2_build_api::keep_going::KeepGoing;
use buck2_build_api::materialize::invocation_re_use_case;
use buck2_build_signals::env::NodeDuration;
use buck2_build_signals::env::WaitingCategory;
use buck2_build_signals::env::WaitingData;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::events::HasEvents;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_common::local_resource_state::LocalResourceState;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::execution_types::executor_config::CommandExecutorConfig;
use buck2_core::execution_types::executor_config::CommandGenerationOptions;
use buck2_core::execution_types::executor_config::Executor;
use buck2_core::execution_types::executor_config::LocalExecutorOptions;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::execution_types::executor_config::RemoteExecutorCustomImage;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::buck_out_path::BuckOutTestPath;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_data::EndOfTestResults;
use buck2_data::SetupLocalResourcesEnd;
use buck2_data::SetupLocalResourcesStart;
use buck2_data::TestDiscovery;
use buck2_data::TestDiscoveryEnd;
use buck2_data::TestDiscoveryStart;
use buck2_data::TestRunEnd;
use buck2_data::TestRunStart;
use buck2_data::TestSessionInfo;
use buck2_data::TestSuite;
use buck2_data::ToProtoMessage;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_execute::execute::blocking::HasBlockingExecutor;
use buck2_execute::execute::cache_uploader::CacheUploadInfo;
use buck2_execute::execute::cache_uploader::NoOpCacheUploader;
use buck2_execute::execute::claim::MutexClaimManager;
use buck2_execute::execute::command_executor::CommandExecutor;
use buck2_execute::execute::environment_inheritance::EnvironmentInheritance;
use buck2_execute::execute::kind::CommandExecutionKind;
use buck2_execute::execute::manager::CommandExecutionManager;
use buck2_execute::execute::prepared::NoOpCommandOptionalExecutor;
use buck2_execute::execute::prepared::PreparedCommand;
use buck2_execute::execute::request::ActionMetadataBlob;
use buck2_execute::execute::request::CommandExecutionInput;
use buck2_execute::execute::request::CommandExecutionOutput;
use buck2_execute::execute::request::CommandExecutionPaths;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::request::ExecutorPreference;
use buck2_execute::execute::request::OutputCreationBehavior;
use buck2_execute::execute::request::WorkerId;
use buck2_execute::execute::request::WorkerSpec;
use buck2_execute::execute::result::CommandCancellationReason;
use buck2_execute::execute::result::CommandExecutionMetadata;
use buck2_execute::execute::result::CommandExecutionReport;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::execute::result::CommandExecutionStatus;
use buck2_execute::execute::target::CommandExecutionTarget;
use buck2_execute::execute::target::request_metadata_target_id;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::ReadLease;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::materialize::produce::write_locally;
use buck2_execute_impl::executors::local::EnvironmentBuilder;
use buck2_execute_impl::executors::local::apply_local_execution_environment;
use buck2_execute_impl::executors::local::create_output_dirs;
use buck2_execute_impl::executors::local::materialize_inputs;
use buck2_execute_impl::executors::local::output_paths;
use buck2_execute_impl::executors::local::prep_scratch_path;
use buck2_fs::async_fs_util::spawn_blocking;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_hash::buck_mut_insertion_set;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_resource_control::HasResourceControl;
use buck2_test_api::data::ArgValue;
use buck2_test_api::data::ArgValueContent;
use buck2_test_api::data::CasDigest;
use buck2_test_api::data::ConfiguredTargetHandle;
use buck2_test_api::data::DeclaredOutput;
use buck2_test_api::data::ExecuteResponse;
use buck2_test_api::data::ExecutionDetails;
use buck2_test_api::data::ExecutionResult2;
use buck2_test_api::data::ExecutionStatus;
use buck2_test_api::data::ExecutionStream;
use buck2_test_api::data::ExecutorConfigOverride;
use buck2_test_api::data::ExternalRunnerSpecValue;
use buck2_test_api::data::LocalExecutionCommand;
use buck2_test_api::data::Output;
use buck2_test_api::data::PrepareForLocalExecutionResult;
use buck2_test_api::data::RequiredLocalResources;
use buck2_test_api::data::TestResult;
use buck2_test_api::data::TestStage;
use buck2_test_api::data::convert::host_sharing_requirements_to_grpc;
use buck2_test_api::protocol::TestOrchestrator;
use derive_more::From;
use dice::DiceComputations;
use dice::DiceTransaction;
use dice::EqualityBehavior;
use dice::Key;
use dice::NoValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use display_container::fmt_container;
use display_container::fmt_keyed_container;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use futures::FutureExt;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use host_sharing::HostSharingRequirements;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::command::InternalRunnerConfig;
use crate::local_resource_api::LocalResourcesSetupResult;
use crate::local_resource_registry::CLEANUP_CAPABILITY_ENV;
use crate::local_resource_registry::CleanupSetup;
use crate::local_resource_registry::HasLocalResourceRegistry;
use crate::local_resource_registry::LocalResourceEntry;
use crate::local_resource_registry::PreparedLocalResource;
use crate::local_resource_setup::TestStageSimple;
use crate::local_resource_setup::required_providers;
use crate::remote_storage;
use crate::session::TestSession;
use crate::session::TestSessionOptions;
use crate::translations;

const MAX_SUFFIX_LEN: usize = 1024;

/// Test info wrapper that works with both `ExternalRunnerTestInfo` and
/// `InternalRunnerTestInfo`. Provider selection is gated on
/// `[test].use_internal_runner` to stay consistent with
/// `command.rs::test_target()`.
pub(crate) enum OwnedTestInfo {
    External(OwnedExternalRunnerTestInfo),
    Internal(OwnedInternalRunnerTestInfo),
}

impl OwnedTestInfo {
    fn supports_test_execution_caching(&self) -> bool {
        match self {
            Self::External(info) => info
                .as_ref()
                .value()
                .as_ref()
                .supports_test_execution_caching(),
            Self::Internal(_) => false,
        }
    }

    fn cli_args_for_stage<'v>(&'v self, stage: &TestStage) -> Vec<&'v dyn CommandLineArgLike<'v>> {
        let filter = |c: TestCommandMember<'v>| -> Option<&'v dyn CommandLineArgLike<'v>> {
            match c {
                TestCommandMember::Literal(..) => None,
                TestCommandMember::Arglike(a) => Some(a),
            }
        };
        match (self, stage) {
            (Self::Internal(info), TestStage::Listing { .. }) => info
                .as_ref()
                .value()
                .as_ref()
                .listing_command()
                .filter_map(filter)
                .collect(),
            (Self::External(info), _) => info
                .as_ref()
                .value()
                .as_ref()
                .command()
                .filter_map(filter)
                .collect(),
            (Self::Internal(info), _) => info
                .as_ref()
                .value()
                .as_ref()
                .command()
                .filter_map(filter)
                .collect(),
        }
    }

    fn env_args<'v>(&'v self) -> BuckMutMap<&'v str, &'v dyn CommandLineArgLike<'v>> {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().env().collect(),
            Self::Internal(info) => info.as_ref().value().as_ref().env().collect(),
        }
    }

    fn local_resources(&self) -> BuckMutInsertionMap<&str, Option<&ConfiguredProvidersLabel>> {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().local_resources(),
            Self::Internal(info) => info.as_ref().value().as_ref().local_resources(),
        }
    }

    fn required_local_resource_names_for_stage(&self, stage: &TestStageSimple) -> Vec<&str> {
        let filter = |r: &StarlarkRequiredTestLocalResource| match stage {
            TestStageSimple::Listing => r.listing,
            TestStageSimple::Testing => r.execution,
        };
        match self {
            Self::External(info) => info
                .as_ref()
                .value()
                .as_ref()
                .required_local_resources()
                .filter(|r| filter(r))
                .map(|r| r.name.as_str())
                .collect(),
            Self::Internal(info) => info
                .as_ref()
                .value()
                .as_ref()
                .required_local_resources()
                .filter(|r| filter(r))
                .map(|r| r.name.as_str())
                .collect(),
        }
    }

    fn execution_required_local_resource_names(&self) -> Vec<&str> {
        self.required_local_resource_names_for_stage(&TestStageSimple::Testing)
    }

    fn executor_override(&self, key: &str) -> Option<&StarlarkCommandExecutorConfig> {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().executor_override(key),
            Self::Internal(info) => info.as_ref().value().as_ref().executor_override(key),
        }
    }

    fn has_executor_overrides(&self) -> bool {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().has_executor_overrides(),
            Self::Internal(info) => info.as_ref().value().as_ref().has_executor_overrides(),
        }
    }

    fn default_executor(&self) -> Option<&StarlarkCommandExecutorConfig> {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().default_executor(),
            Self::Internal(info) => info.as_ref().value().as_ref().default_executor(),
        }
    }

    fn execution_group(&self) -> buck2_error::Result<Option<ExecutionGroup>> {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().execution_group(),
            Self::Internal(info) => info.as_ref().value().as_ref().execution_group(),
        }
    }

    fn run_from_project_root(&self) -> bool {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().run_from_project_root(),
            Self::Internal(info) => info.as_ref().value().as_ref().run_from_project_root(),
        }
    }

    fn use_project_relative_paths(&self) -> bool {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().use_project_relative_paths(),
            Self::Internal(info) => info.as_ref().value().as_ref().use_project_relative_paths(),
        }
    }

    fn worker(&self) -> Option<&WorkerInfo<'_>> {
        match self {
            Self::External(info) => info.as_ref().value().as_ref().worker(),
            Self::Internal(info) => info.as_ref().value().as_ref().worker(),
        }
    }

    fn has_static_listing_label(&self) -> bool {
        match self {
            Self::External(info) => info
                .as_ref()
                .value()
                .as_ref()
                .labels()
                .any(|l| l == "static-listing"),
            Self::Internal(info) => info
                .as_ref()
                .value()
                .as_ref()
                .labels()
                .any(|l| l == "static-listing"),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ExecutorMessage {
    TestResult(TestResult),
    ExitCode(i32),
    InfoMessage(String),
}

pub struct BuckTestOrchestrator<'a: 'static> {
    dice: DiceTransaction,
    session: Arc<TestSession>,
    results_channel: UnboundedSender<buck2_error::Result<ExecutorMessage>>,
    events: EventDispatcher,
    liveliness_observer: Arc<dyn LivelinessObserver>,
    cancellations: &'a CancellationContext,
    re_client: Arc<remote_storage::ReClientWithCache>,
    internal_runner_config: InternalRunnerConfig,
}

impl<'a> BuckTestOrchestrator<'a> {
    pub(crate) async fn new(
        dice: DiceTransaction,
        session: Arc<TestSession>,
        liveliness_observer: Arc<dyn LivelinessObserver>,
        results_channel: UnboundedSender<buck2_error::Result<ExecutorMessage>>,
        cancellations: &'a CancellationContext,
        internal_runner_config: InternalRunnerConfig,
    ) -> buck2_error::Result<BuckTestOrchestrator<'a>> {
        let events = dice.per_transaction_data().get_dispatcher().dupe();
        let re_client = Arc::new(remote_storage::ReClientWithCache::new(
            dice.per_transaction_data().get_re_client().dupe(),
        ));
        Ok(Self::from_parts(
            dice,
            session,
            liveliness_observer,
            results_channel,
            events,
            cancellations,
            re_client,
            internal_runner_config,
        ))
    }

    fn from_parts(
        dice: DiceTransaction,
        session: Arc<TestSession>,
        liveliness_observer: Arc<dyn LivelinessObserver>,
        results_channel: UnboundedSender<buck2_error::Result<ExecutorMessage>>,
        events: EventDispatcher,
        cancellations: &'a CancellationContext,
        re_client: Arc<remote_storage::ReClientWithCache>,
        internal_runner_config: InternalRunnerConfig,
    ) -> BuckTestOrchestrator<'a> {
        Self {
            dice,
            session,
            results_channel,
            events,
            liveliness_observer,
            cancellations,
            re_client,
            internal_runner_config,
        }
    }

    async fn require_alive(
        liveliness_observer: Arc<dyn LivelinessObserver>,
    ) -> Result<(), Cancelled> {
        if !liveliness_observer.is_alive().await {
            return Err(Cancelled {
                ..Default::default()
            });
        }

        Ok(())
    }

    /// Whether to exempt this action from network isolation when it runs locally.
    ///
    /// Static listing's enumeration tool (gtest-list-tests, coral, ...) is a DotSlash
    /// stub that needs the network to resolve. On RE it can still resolve, because RE
    /// has a DotSlash that works under the allowed isolation modes, so the listing
    /// action inherits the normal executor policy and needs no override. Locally the
    /// forkserver would put a restricted policy in a network namespace where the stub
    /// can't resolve, so the local listing action must run without isolation. Has no
    /// effect on remote execution.
    fn disable_local_network_isolation(stage: &TestStage, test_info: &OwnedTestInfo) -> bool {
        matches!(stage, TestStage::Listing { .. }) && test_info.has_static_listing_label()
    }

    async fn execute2(
        &self,
        stage: TestStage,
        test_target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        timeout: Duration,
        host_sharing_requirements: HostSharingRequirements,
        pre_create_dirs: Vec<DeclaredOutput>,
        executor_override: Option<ExecutorConfigOverride>,
        required_local_resources: RequiredLocalResources,
        disable_test_execution_caching: bool,
    ) -> Result<ExecutionResult2, ExecuteError> {
        Self::require_alive(self.liveliness_observer.dupe()).await?;

        let test_target = self.session.get(test_target)?;

        // The runner reads a run's outputs out of process, so their lease lives in the session
        // until the runner comes back to this root; the rerun then takes the root for writing,
        // which the lease it replaces would block.
        let output_root = ProjectRelativePathBuf::from(
            resolve_output_root(&mut self.dice.dupe().ctx(), &test_target, &stage).await?,
        );
        self.session.release(&output_root);

        let fs = self.dice.ctx().get_artifact_fs().await?;
        let pre_create_dirs = Arc::new(pre_create_dirs);

        let ExecuteData {
            stdout,
            stderr,
            status,
            timing,
            execution_kind,
            outputs,
            command_execution,
            ..
        } = prepare_and_execute(
            &mut self.dice.dupe().ctx(),
            self.cancellations,
            TestExecutionKey {
                test_target,
                cmd: Arc::new(cmd),
                env: Arc::new(env),
                executor_override: executor_override.map(Arc::new),
                required_local_resources: Arc::new(required_local_resources),
                pre_create_dirs: pre_create_dirs.dupe(),
                stage: Arc::new(stage),
                options: self.session.options(),
                timeout,
                host_sharing_requirements: host_sharing_requirements.into(),
                disable_test_execution_caching,
            },
            self.liveliness_observer.dupe(),
            &self.internal_runner_config,
        )
        .await?;

        Self::require_alive(self.liveliness_observer.dupe()).await?;

        let mut output_map = BuckMutMap::default();
        let mut outputs_to_materialize = vec![];

        let remote_storage_config_update_futures = FuturesUnordered::new();

        for (test_path, artifact) in outputs {
            let project_relative_path = fs.buck_out_path_resolver().resolve_test(&test_path);
            let output_name = test_path.into_path().into();
            // It's OK to search iteratively here because there will be few entries in `pre_create_dirs`
            let remote_storage_config = pre_create_dirs
                .iter()
                .find(|&x| x.name == output_name)
                .map_or_else(Default::default, |x| x.remote_storage_config.dupe());
            match (
                remote_storage_config.supports_remote,
                execution_kind.as_ref(),
                translations::convert_artifact(output_name.clone().into_string(), &artifact),
            ) {
                // This condition checks that a downstream consumer supports
                // remote outputs AND the output is actually in CAS.
                //
                // TODO(arr): is there a better way to check that the output is
                // in CAS other than checking that the command was executed on
                // RE? Alternatively, when we make buck upload local testing
                // artifacts to CAS, we can remove this condition altogether.
                (true, Some(CommandExecutionKind::Remote { .. }), Some(remote_object)) => {
                    let re_client = self.re_client.clone();
                    let future = async move {
                        let _unused = re_client
                            .apply_config(&artifact, &remote_storage_config)
                            .await;
                        (output_name, remote_object)
                    };
                    remote_storage_config_update_futures.push(future);
                }
                _ => {
                    outputs_to_materialize.push((project_relative_path.clone(), artifact.dupe()));
                    let abs_path = fs.fs().resolve(&project_relative_path);
                    output_map.insert(output_name, Output::LocalPath(abs_path));
                }
            };
        }
        let results: Vec<_> = remote_storage_config_update_futures.collect().await;
        for result in results {
            output_map.insert(result.0, Output::RemoteObject(result.1));
        }

        // Request materialization in case this ran on RE. Eventually Tpx should be able to
        // understand remote outputs but currently we don't have this.
        let re_use_case = invocation_re_use_case(&self.dice.ctx());
        let response = self
            .dice
            .per_transaction_data()
            .get_materializer()
            .materialize(MaterializeRequest {
                artifacts: outputs_to_materialize,
                outputs: Vec::new(),
                purpose: MaterializationPurpose::IntermediateOnly,
                re_use_case,
            })
            .await
            .buck_error_context("Error materializing test outputs")?;
        for result in response.results {
            result.buck_error_context("Error materializing test outputs")?;
        }
        self.session.hold(output_root, response.lease);

        Ok(ExecutionResult2 {
            status,
            stdout,
            stderr,
            outputs: output_map,
            start_time: timing.start_time,
            execution_time: timing.execution_time,
            execution_details: ExecutionDetails {
                execution_kind: execution_kind.map(|k| k.to_proto(false)),
            },
            max_memory_used_bytes: timing.execution_stats.and_then(|s| s.memory_peak),
            command_execution,
        })
    }

    async fn prepare_and_execute_no_dice(
        dice: &mut DiceComputations<'_>,
        key: TestExecutionKey,
        liveliness_observer: Arc<dyn LivelinessObserver>,
        cancellation: &CancellationContext,
        internal_runner_config: &InternalRunnerConfig,
    ) -> Result<ExecuteData, ExecuteError> {
        let TestExecutionKey {
            test_target,
            cmd,
            env,
            executor_override,
            required_local_resources,
            pre_create_dirs,
            stage,
            options,
            timeout,
            host_sharing_requirements,
            disable_test_execution_caching,
        } = key;
        // The test executor decides when to ask buck2 to run a test, so the time from the test's
        // dependencies finishing until now is the executor's scheduling.
        let mut waiting_data =
            WaitingData::with_initial_category(WaitingCategory::TestExecutorScheduling);
        waiting_data.start_waiting_category_now(WaitingCategory::PreparingAction);
        let fs = dice.get_artifact_fs().await?;
        let test_info = Self::get_test_info(dice, &test_target, internal_runner_config).await?;
        let test_labels = test_labels(&test_info);
        let effective_test_execution_caching =
            test_info.supports_test_execution_caching() && !disable_test_execution_caching;
        let disable_local_network_isolation =
            Self::disable_local_network_isolation(stage.as_ref(), &test_info);
        let test_executor = Self::get_test_executor(
            dice,
            &test_target,
            &test_info,
            executor_override,
            fs,
            &stage,
            effective_test_execution_caching,
        )
        .await?;
        let test_executable_expanded = Self::expand_test_executable(
            dice,
            &test_target,
            &test_info,
            Cow::Borrowed(&cmd),
            Cow::Borrowed(&env),
            Cow::Borrowed(&pre_create_dirs),
            &test_executor.executor().executor_fs(),
            &stage,
            options,
        )
        .boxed()
        .await?;
        let ExpandedTestExecutable {
            cwd,
            cmd: expanded_cmd,
            env: expanded_env,
            ensured_inputs,
            supports_re,
            declared_outputs,
            argsfile_inputs,
            worker,
        } = test_executable_expanded;

        let input_deps_action_keys: Vec<_> = ensured_inputs
            .iter()
            .flat_map(|(_, agv)| {
                agv.iter()
                    .filter_map(|(artifact, _)| artifact.action_key().map(|k| k.dupe()))
            })
            .collect::<BuckMutSet<_>>() // dedupe
            .into_iter()
            .collect();

        let executor_preference = Self::executor_preference(options, supports_re)?;
        let required_resources = if test_executor
            .executor()
            .is_local_execution_possible(executor_preference)
        {
            let setup_local_resources_executor = Self::get_local_executor(dice, fs)?;
            let simple_stage = stage.as_ref().into();

            let available_resources: BuckMutMap<_, _> =
                test_info.local_resources().into_iter().collect();
            let rule_required_names =
                test_info.required_local_resource_names_for_stage(&simple_stage);
            let required_providers = {
                required_providers(
                    dice,
                    available_resources,
                    rule_required_names,
                    &required_local_resources,
                )
                .await?
            };
            // If some timeout is neeeded, use the same value as for the test itself which is better than nothing.
            Self::setup_local_resources(
                dice,
                cancellation,
                required_providers,
                setup_local_resources_executor,
                timeout,
                liveliness_observer.dupe(),
            )
            .await?
        } else {
            vec![]
        };
        let execution_request = Self::create_command_execution_request(
            dice,
            cwd,
            expanded_cmd,
            expanded_env,
            ensured_inputs,
            declared_outputs,
            argsfile_inputs,
            fs,
            Some(timeout),
            Some(host_sharing_requirements),
            Some(executor_preference),
            required_resources,
            worker,
            test_executor.re_dynamic_image(),
            test_executor.meta_internal_extra_params(),
            disable_local_network_isolation,
        )
        .boxed()
        .await?;
        let result = Self::execute_request(
            dice,
            cancellation,
            &test_target,
            &stage,
            &test_labels,
            test_executor.executor(),
            execution_request,
            liveliness_observer.dupe(),
            test_executor.re_cache_enabled(),
            effective_test_execution_caching,
            waiting_data,
        )
        .boxed()
        .await?;

        if let Some(signals) = dice.per_transaction_data().get_build_signals() {
            let duration = NodeDuration {
                user: result.timing.execution_time,
                total: result.timing.time_span,
                queue: result.timing.queue_duration,
            };

            match stage.as_ref() {
                TestStage::Listing { suite, .. } => {
                    signals.test_listing(
                        test_target.target().dupe(),
                        suite.to_owned(),
                        duration.to_owned(),
                        &input_deps_action_keys,
                        result.waiting_data.clone(),
                    );
                }
                TestStage::Testing {
                    suite,
                    testcases,
                    variant,
                    ..
                } => {
                    signals.test_execution(
                        test_target.target().dupe(),
                        suite.to_owned(),
                        testcases,
                        variant.to_owned(),
                        duration,
                        &input_deps_action_keys,
                        result.waiting_data.clone(),
                    );
                }
            }
        }

        Ok(result)
    }
}

#[derive(Clone, Dupe, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct TestExecutionKey {
    test_target: ConfiguredProvidersLabel,
    cmd: Arc<Vec<ArgValue>>,
    env: Arc<BuckFixedSortedMap<String, ArgValue>>,
    executor_override: Option<Arc<ExecutorConfigOverride>>,
    required_local_resources: Arc<RequiredLocalResources>,
    pre_create_dirs: Arc<Vec<DeclaredOutput>>,
    stage: Arc<TestStage>,
    options: TestSessionOptions,
    timeout: Duration,
    host_sharing_requirements: Arc<HostSharingRequirements>,
    disable_test_execution_caching: bool,
}

#[async_trait]
impl Key for TestExecutionKey {
    type Value = Result<ExecuteData, ExecuteError>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellations: &CancellationContext,
    ) -> Self::Value {
        let cell_resolver = ctx.get_cell_resolver().await.map_err(ExecuteError::Error)?;
        let config = InternalRunnerConfig::parse(
            ctx.get_legacy_config_property(
                cell_resolver.root_cell(),
                BuckconfigKeyRef {
                    section: "test",
                    property: "use_internal_runner",
                },
            )
            .await
            .map_err(ExecuteError::Error)?
            .as_deref(),
        );
        cancellations
            .with_structured_cancellation(|observer| {
                async move {
                    BuckTestOrchestrator::prepare_and_execute_no_dice(
                        ctx,
                        self.dupe(),
                        Arc::new(observer),
                        cancellations,
                        &config,
                    )
                    .await
                }
                .boxed()
            })
            .await
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::AlwaysUnequal
    }

    fn validity(x: &Self::Value) -> bool {
        // We don't want to cache any failed listings
        x.as_ref()
            .is_ok_and(|f| f.status == ExecutionStatus::Finished { exitcode: 0 })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

async fn prepare_and_execute(
    ctx: &mut DiceComputations<'_>,
    cancellation: &CancellationContext,
    key: TestExecutionKey,
    liveliness_observer: Arc<dyn LivelinessObserver>,
    internal_runner_config: &InternalRunnerConfig,
) -> Result<ExecuteData, ExecuteError> {
    let execute_on_dice = match key.stage.as_ref() {
        TestStage::Listing { cacheable, .. } => *cacheable,
        TestStage::Testing { .. } => false,
    };
    if execute_on_dice {
        let result = tokio::select! {
            _ = liveliness_observer.while_alive() => {
                Err(ExecuteError::Cancelled(Cancelled{..Default::default()}))
            }
            result = prepare_and_execute_dice(ctx, &key) => {
                result
            }
        }?;
        Ok((*result).clone())
    } else {
        Ok(BuckTestOrchestrator::prepare_and_execute_no_dice(
            ctx,
            key,
            liveliness_observer,
            cancellation,
            internal_runner_config,
        )
        .await?)
    }
}

async fn prepare_and_execute_dice<'d>(
    ctx: &mut DiceComputations<'d>,
    key: &TestExecutionKey,
) -> Result<&'d ExecuteData, ExecuteError> {
    ctx.compute(key)
        .await
        .map_err(buck2_error::Error::from)?
        .as_ref()
        .duped_err()
}

async fn test_discovery_command_report(
    report: &CommandExecutionReport,
) -> buck2_data::CommandExecution {
    report.to_command_execution_proto(true, true, false).await
}

async fn test_run_command_report(report: &CommandExecutionReport) -> buck2_data::CommandExecution {
    report.to_command_execution_proto(false, false, false).await
}

fn test_timeout_proto(timeout: Option<Duration>) -> Option<prost_types::Duration> {
    timeout.and_then(|timeout| timeout.try_into().ok())
}

impl Display for TestExecutionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "test_target = {}, ", self.test_target)?;
        fmt_container(f, "cmd = [", "], ", self.cmd.as_ref())?;
        fmt_keyed_container(f, "env = {", "}, ", ",", self.env.as_ref())?;
        fmt_container(
            f,
            "executor_override = [",
            "], ",
            self.executor_override.iter(),
        )?;
        write!(
            f,
            "required_local_resources = {}, ",
            self.required_local_resources.as_ref(),
        )?;
        fmt_container(f, "pre_create_dirs = [", "], ", self.pre_create_dirs.iter())?;
        write!(
            f,
            "stage = {}, options = {}, timeout = {}, host_sharing_requirements = {}",
            self.stage,
            self.options,
            self.timeout.as_millis(),
            self.host_sharing_requirements
        )
    }
}

enum LocalResourceSetupEvent {
    Prepared(buck2_error::Result<PreparedLocalResource>),
    Registered(buck2_error::Result<crate::local_resource_registry::SetupOwnership>),
}

struct PreparedLocalResourceSetupContext {
    pub target: ConfiguredTargetLabel,
    pub execution_request: CommandExecutionRequest,
    pub env_var_mapping: BuckMutInsertionMap<String, String>,
    cleanup_setup: Option<CleanupSetup>,
}

#[derive(Clone, Dupe, Allocative)]
enum CancellationReason {
    NotSpecified,
    ReQueueTimeout,
}

#[derive(Default, Clone, Dupe, Allocative)]
struct Cancelled {
    reason: Option<CancellationReason>,
}

// NOTE: This doesn't implement Error so that we can't accidentally lose the Cancelled variant.
#[derive(From, Clone, Dupe, Allocative)]
enum ExecuteError {
    Error(buck2_error::Error),
    Cancelled(Cancelled),
}

#[async_trait]
impl TestOrchestrator for BuckTestOrchestrator<'_> {
    async fn execute2(
        &self,
        stage: TestStage,
        test_target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        timeout: Duration,
        host_sharing_requirements: HostSharingRequirements,
        pre_create_dirs: Vec<DeclaredOutput>,
        executor_override: Option<ExecutorConfigOverride>,
        required_local_resources: RequiredLocalResources,
        disable_test_execution_caching: bool,
    ) -> buck2_error::Result<ExecuteResponse> {
        let res = BuckTestOrchestrator::execute2(
            self,
            stage,
            test_target,
            cmd,
            env,
            timeout,
            host_sharing_requirements,
            pre_create_dirs,
            executor_override,
            required_local_resources,
            disable_test_execution_caching,
        )
        .await;

        match res {
            Ok(r) => Ok(ExecuteResponse::Result(r)),
            Err(ExecuteError::Cancelled(cancelled)) => {
                Ok(ExecuteResponse::Cancelled(match cancelled.reason {
                    Some(CancellationReason::NotSpecified) => {
                        Some(buck2_test_api::data::CancellationReason::NotSpecified)
                    }
                    Some(CancellationReason::ReQueueTimeout) => {
                        Some(buck2_test_api::data::CancellationReason::ReQueueTimeout)
                    }
                    None => None,
                }))
            }
            Err(ExecuteError::Error(e)) => Err(e),
        }
    }

    async fn report_test_result(&self, r: TestResult) -> buck2_error::Result<()> {
        let event = buck2_data::instant_event::Data::TestResult(translations::convert_test_result(
            r.clone(),
            &self.session,
        )?);
        self.events.instant_event(event);
        self.results_channel
            .unbounded_send(Ok(ExecutorMessage::TestResult(r)))
            .map_err(|_| {
                buck2_error::internal_error!("Test result was received after end-of-tests")
            })?;
        Ok(())
    }

    async fn report_tests_discovered(
        &self,
        test_target: ConfiguredTargetHandle,
        suite: String,
        names: Vec<String>,
    ) -> buck2_error::Result<()> {
        let test_target = self.session.get(test_target)?;
        let mut dice = self.dice.ctx();
        let test_info =
            Self::get_test_info(&mut dice, &test_target, &self.internal_runner_config).await?;
        let labels = test_labels(&test_info);

        self.events.instant_event(TestDiscovery {
            data: Some(buck2_data::test_discovery::Data::Tests(TestSuite {
                suite_name: suite,
                test_names: names,
                target_label: Some(test_target.target().as_proto()),
                labels,
            })),
        });

        Ok(())
    }

    async fn report_test_session(
        &self,
        session_info: String,
        test_session_id: Option<String>,
    ) -> buck2_error::Result<()> {
        self.events.instant_event(TestDiscovery {
            data: Some(buck2_data::test_discovery::Data::Session(TestSessionInfo {
                info: session_info,
                test_session_id,
            })),
        });

        Ok(())
    }

    async fn end_of_test_results(&self, exit_code: i32) -> buck2_error::Result<()> {
        self.session.release_all();
        self.events.instant_event(EndOfTestResults { exit_code });
        self.results_channel
            .unbounded_send(Ok(ExecutorMessage::ExitCode(exit_code)))
            .map_err(|_| buck2_error::internal_error!("end_of_tests was received twice"))?;
        self.results_channel.close_channel();
        Ok(())
    }

    async fn prepare_for_local_execution(
        &self,
        stage: TestStage,
        test_target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        pre_create_dirs: Vec<DeclaredOutput>,
        required_local_resources: RequiredLocalResources,
    ) -> buck2_error::Result<PrepareForLocalExecutionResult> {
        let test_target = self.session.get(test_target)?;

        // As in `execute2`: the runner reads the inputs out of process, under a lease the
        // session holds until the runner comes back to this root.
        let output_root = ProjectRelativePathBuf::from(
            resolve_output_root(&mut self.dice.dupe().ctx(), &test_target, &stage).await?,
        );
        self.session.release(&output_root);

        let fs = self.dice.ctx().get_artifact_fs().await?;

        let test_info = Self::get_test_info(
            &mut self.dice.dupe().ctx(),
            &test_target,
            &self.internal_runner_config,
        )
        .await?;
        let disable_local_network_isolation =
            Self::disable_local_network_isolation(&stage, &test_info);

        // In contrast from actual test execution we do not check if local execution is possible.
        // We leave that decision to actual local execution runner that requests local execution preparation.
        let setup_local_resources_executor =
            Self::get_local_executor(&mut self.dice.dupe().ctx(), fs)?;
        let available_resources: BuckMutMap<_, _> =
            test_info.local_resources().into_iter().collect();
        let rule_required_names = test_info.execution_required_local_resource_names();
        let providers = {
            required_providers(
                &mut self.dice.dupe().ctx(),
                available_resources,
                rule_required_names,
                &required_local_resources,
            )
            .await?
        };
        let executor_fs = setup_local_resources_executor.executor_fs();
        let setup_commands: Vec<PreparedLocalResourceSetupContext> = self
            .dice
            .dupe()
            .ctx()
            .try_compute_join(providers, async |dice, provider| {
                Self::prepare_local_resource(dice, provider, fs, &executor_fs, Duration::default())
                    .await
            })
            .await?;

        // Tests are not run, so there is no executor override.
        let test_executor = Self::get_test_executor(
            &mut self.dice.dupe().ctx(),
            &test_target,
            &test_info,
            None,
            fs,
            &stage,
            false,
        )
        .await?;
        let test_executable_expanded = Self::expand_test_executable(
            &mut self.dice.dupe().ctx(),
            &test_target,
            &test_info,
            Cow::Owned(cmd),
            Cow::Owned(env),
            Cow::Owned(pre_create_dirs),
            &test_executor.executor().executor_fs(),
            &stage,
            self.session.options(),
        )
        .await?;

        let ExpandedTestExecutable {
            cwd,
            cmd: expanded_cmd,
            env: expanded_env,
            ensured_inputs,
            supports_re: _,
            declared_outputs,
            argsfile_inputs,
            worker,
        } = test_executable_expanded;

        let execution_request = Self::create_command_execution_request(
            &mut self.dice.dupe().ctx(),
            cwd,
            expanded_cmd,
            expanded_env,
            ensured_inputs,
            declared_outputs,
            argsfile_inputs,
            fs,
            None,
            None,
            None,
            vec![],
            worker,
            test_executor.re_dynamic_image(),
            test_executor.meta_internal_extra_params(),
            disable_local_network_isolation,
        )
        .await?;

        let materializer = self.dice.per_transaction_data().get_materializer();
        let blocking_executor = self.dice.ctx().get_blocking_executor();
        let re_use_case = invocation_re_use_case(&self.dice.ctx());

        // This prepares a command that the test runner executes out of process after this
        // returns. Tests buck2 runs itself go through the local executor, which holds its leases
        // across the run.
        let materialized_inputs = materialize_inputs(
            fs,
            materializer,
            &execution_request,
            self.dice.global_data().get_digest_config(),
            re_use_case,
            output_paths(fs, &execution_request)?,
        )
        .await?;

        prep_scratch_path(&materialized_inputs.scratch, fs).await?;

        create_output_dirs(
            fs,
            &execution_request,
            blocking_executor,
            self.cancellations,
        )
        .await?;
        // FIXME(materializer): The runner writes the outputs after this returns, so nothing
        // here can report them: what it writes is untracked content under buck-out that no
        // core serves, keeps or cleans. Closing that needs the runner to report what it wrote,
        // or buck2 to collect it once the runner is done with the command.
        drop(materialized_inputs.outputs);
        let mut held = vec![materialized_inputs.lease];

        for local_resource_setup_command in setup_commands.iter() {
            let materialized_inputs = materialize_inputs(
                fs,
                materializer,
                &local_resource_setup_command.execution_request,
                self.dice.global_data().get_digest_config(),
                re_use_case,
                output_paths(fs, &local_resource_setup_command.execution_request)?,
            )
            .await?;
            let blocking_executor = self.dice.ctx().get_blocking_executor();

            prep_scratch_path(&materialized_inputs.scratch, fs).await?;

            create_output_dirs(
                fs,
                &local_resource_setup_command.execution_request,
                blocking_executor,
                self.cancellations,
            )
            .await?;
            // FIXME(materializer): As for the test command above.
            drop(materialized_inputs.outputs);
            held.push(materialized_inputs.lease);
        }
        self.session.hold(output_root, ReadLease::join(held));

        Ok(create_prepare_for_local_execution_result(
            fs,
            execution_request,
            setup_commands,
        ))
    }

    async fn attach_info_message(&self, message: String) -> buck2_error::Result<()> {
        self.results_channel
            .unbounded_send(Ok(ExecutorMessage::InfoMessage(message)))
            .map_err(|_| buck2_error::internal_error!("Message received after end-of-tests"))?;
        Ok(())
    }

    async fn upload_to_cas(
        &self,
        local_path: String,
        ttl_seconds: i64,
        use_case: String,
    ) -> buck2_error::Result<CasDigest> {
        let digest_config = self.dice.global_data().get_digest_config();
        let re_digest = self
            .re_client
            .upload_local_file(&local_path, digest_config, ttl_seconds, &use_case)
            .await?;
        Ok(CasDigest {
            hash: re_digest.hash,
            size_bytes: re_digest.size_in_bytes,
        })
    }
}
#[derive(Allocative, Clone)]
struct ExecuteData {
    pub stdout: ExecutionStream,
    pub stderr: ExecutionStream,
    pub status: ExecutionStatus,
    pub timing: CommandExecutionMetadata,
    pub execution_kind: Option<CommandExecutionKind>,
    pub outputs: Vec<(BuckOutTestPath, ArtifactValue)>,
    pub command_execution: Option<buck2_data::CommandExecution>,
    #[allocative(skip)]
    pub waiting_data: WaitingData,
}

impl BuckTestOrchestrator<'_> {
    fn executor_preference(
        opts: TestSessionOptions,
        test_supports_re: bool,
    ) -> buck2_error::Result<ExecutorPreference> {
        let mut executor_preference = ExecutorPreference::Default;

        if !opts.allow_re {
            // We don't ban RE (we only prefer not to use it) if the session doesn't allow it, so
            // that executor overrides or default executor can still route executions to RE.
            executor_preference = executor_preference.and(ExecutorPreference::LocalPreferred)?;
        }

        if !test_supports_re {
            // But if the test doesn't support RE at all, then we ban it.
            executor_preference = executor_preference.and(ExecutorPreference::LocalRequired)?;
        }

        Ok(executor_preference)
    }

    /// Core request execution logic.
    async fn execute_request(
        dice: &mut DiceComputations<'_>,
        cancellation: &CancellationContext,
        test_target_label: &ConfiguredProvidersLabel,
        stage: &TestStage,
        test_labels: &[String],
        executor: &CommandExecutor,
        request: CommandExecutionRequest,
        liveliness_observer: Arc<dyn LivelinessObserver>,
        re_cache_enabled: bool,
        supports_test_execution_caching: bool,
        waiting_data: WaitingData,
    ) -> Result<ExecuteData, ExecuteError> {
        let events = dice.per_transaction_data().get_dispatcher().dupe();
        let manager = CommandExecutionManager::new(
            Box::new(MutexClaimManager::new()),
            events.dupe(),
            liveliness_observer.dupe(),
            waiting_data,
        );
        let digest_config = dice.global_data().get_digest_config();

        let test_target = TestTarget {
            target: test_target_label.target(),
            action_key_suffix: create_action_key_suffix(stage),
        };

        let prepared_action = match executor.prepare_action(&request, digest_config, false) {
            Ok(prepared_action) => prepared_action,
            Err(e) => return Err(ExecuteError::Error(e)),
        };
        let prepared_command = PreparedCommand {
            target: &test_target as _,
            request: &request,
            prepared_action: &prepared_action,
            digest_config,
        };

        // instrument execution with a span.
        // TODO(brasselsprouts): migrate this into the executor to get better accuracy.
        let command_exec_result = match stage {
            TestStage::Listing { suite, cacheable } => {
                let start = TestDiscoveryStart {
                    target_label: Some(test_target.target.as_proto()),
                    suite_name: suite.clone(),
                    labels: test_labels.to_vec(),
                };
                let (result, cached) = events
                    .span_async(start, async move {
                        let (result, cached) = if *cacheable {
                            match executor
                                .action_cache(manager, &prepared_command, cancellation)
                                .await
                            {
                                ControlFlow::Continue(manager) => {
                                    let result = executor
                                        .exec_cmd(manager, &prepared_command, cancellation)
                                        .await;
                                    (result, false)
                                }
                                ControlFlow::Break(result) => (result, true),
                            }
                        } else {
                            let result = executor
                                .exec_cmd(manager, &prepared_command, cancellation)
                                .await;
                            (result, false)
                        };
                        let end = TestDiscoveryEnd {
                            suite_name: suite.clone(),
                            target_label: Some(test_target.target.as_proto()),
                            labels: test_labels.to_vec(),
                            command_report: Some(
                                test_discovery_command_report(&result.report).await,
                            ),
                            command_host_sharing_requirements: host_sharing_requirements_to_grpc(
                                prepared_command.request.host_sharing_requirements().clone(),
                            )
                            .ok(),
                            re_cache_enabled: *cacheable && re_cache_enabled,
                        };
                        ((result, cached), end)
                    })
                    .await;
                if !cached && *cacheable {
                    let info = CacheUploadInfo {
                        target: &test_target as _,
                        digest_config,
                        mergebase: &None,
                        re_platform: executor.re_platform(),
                        allow_cache_upload: true,
                        paths: request.paths(),
                    };
                    let _result = match executor
                        .cache_upload(
                            &info,
                            &result,
                            None,
                            None,
                            &prepared_action.action_and_blobs,
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(e) => return Err(ExecuteError::Error(e)),
                    };
                }
                result
            }
            TestStage::Testing {
                suite, testcases, ..
            } => {
                let test_suite = Some(TestSuite {
                    suite_name: suite.clone(),
                    test_names: testcases.clone(),
                    target_label: Some(test_target.target.as_proto()),
                    labels: test_labels.to_vec(),
                });
                let start = TestRunStart {
                    suite: test_suite.clone(),
                };
                events
                    .span_async(start, async move {
                        let result = if supports_test_execution_caching {
                            match executor
                                .action_cache(manager, &prepared_command, cancellation)
                                .await
                            {
                                ControlFlow::Continue(manager) => {
                                    executor
                                        .exec_cmd(manager, &prepared_command, cancellation)
                                        .await
                                }
                                ControlFlow::Break(result) => result,
                            }
                        } else {
                            executor
                                .exec_cmd(manager, &prepared_command, cancellation)
                                .await
                        };
                        let end = TestRunEnd {
                            suite: test_suite,
                            command_report: Some(test_run_command_report(&result.report).await),
                            command_host_sharing_requirements: host_sharing_requirements_to_grpc(
                                prepared_command.request.host_sharing_requirements().clone(),
                            )
                            .ok(),
                            timeout: test_timeout_proto(prepared_command.request.timeout()),
                        };
                        (result, end)
                    })
                    .await
            }
        };
        let command_execution = Some(
            command_exec_result
                .report
                .to_command_execution_proto(false, false, false)
                .await,
        );

        let CommandExecutionResult {
            outputs,
            report:
                CommandExecutionReport {
                    std_streams,
                    exit_code,
                    status,
                    timing,
                    ..
                },
            waiting_data,
            ..
        } = command_exec_result;

        let outputs = outputs
            .into_iter()
            .filter_map(|(output, artifact)| Some((output.into_test_path()?, artifact)))
            .collect();

        let std_streams = std_streams
            .into_bytes()
            .await
            .buck_error_context("Error accessing test output")?;
        let stdout = ExecutionStream::Inline(std_streams.stdout);
        let stderr = ExecutionStream::Inline(std_streams.stderr);

        // If we are shutting down, we may have terminated executions and caused
        // the outcomes we are reporting (typically w/ a worker failure).
        Self::require_alive(liveliness_observer.dupe()).await?;

        Ok(match status {
            CommandExecutionStatus::Success { execution_kind } => ExecuteData {
                stdout,
                stderr,
                status: ExecutionStatus::Finished {
                    exitcode: exit_code.unwrap_or(0),
                },
                timing,
                execution_kind: Some(execution_kind),
                outputs,
                command_execution,
                waiting_data,
            },
            CommandExecutionStatus::Failure { execution_kind, .. }
            | CommandExecutionStatus::WorkerFailure { execution_kind } => ExecuteData {
                stdout,
                stderr,
                status: ExecutionStatus::Finished {
                    exitcode: failure_exit_code(exit_code),
                },
                timing,
                execution_kind: Some(execution_kind),
                outputs,
                command_execution,
                waiting_data,
            },
            CommandExecutionStatus::TimedOut {
                duration,
                execution_kind,
            } => ExecuteData {
                stdout,
                stderr,
                status: ExecutionStatus::TimedOut { duration },
                timing,
                execution_kind: Some(execution_kind),
                outputs,
                command_execution,
                waiting_data,
            },
            CommandExecutionStatus::Error {
                error,
                execution_kind,
                ..
            } => ExecuteData {
                stdout: ExecutionStream::Inline(Default::default()),
                stderr: ExecutionStream::Inline(format!("{error:?}").into_bytes()),
                status: ExecutionStatus::Finished {
                    exitcode: failure_exit_code(exit_code),
                },
                timing,
                execution_kind,
                outputs,
                command_execution,
                waiting_data,
            },
            CommandExecutionStatus::Cancelled {
                execution_kind: _,
                reason,
            } => {
                let reason = reason.map(|reason| match reason {
                    CommandCancellationReason::NotSpecified => CancellationReason::NotSpecified,
                    CommandCancellationReason::ReQueueTimeout => CancellationReason::ReQueueTimeout,
                });
                return Err(ExecuteError::Cancelled(Cancelled { reason }));
            }
        })
    }

    fn executor_config_with_remote_cache_override<'a>(
        test_target_node: &'a ConfiguredTargetNode,
        executor_override: Option<&'a CommandExecutorConfig>,
        stage: &TestStage,
        supports_test_execution_caching: bool,
    ) -> buck2_error::Result<Cow<'a, CommandExecutorConfig>> {
        let executor_config = match executor_override {
            Some(o) => o,
            None => test_target_node
                .execution_platform_resolution()
                .executor_config()
                .buck_error_context("Error accessing executor config")?,
        };

        if let TestStage::Listing { .. } = &stage {
            return Ok(Cow::Borrowed(executor_config));
        }

        if supports_test_execution_caching {
            return Ok(Cow::Borrowed(executor_config));
        }

        match &executor_config.executor {
            Executor::RemoteEnabled(options) if options.remote_cache_enabled => {
                let mut exec_options = options.clone();
                exec_options.remote_cache_enabled = false;
                let executor_config = CommandExecutorConfig {
                    executor: Executor::RemoteEnabled(exec_options),
                    options: executor_config.options.dupe(),
                };
                Ok(Cow::Owned(executor_config))
            }
            Executor::Local(_) | Executor::RemoteEnabled(_) | Executor::None => {
                Ok(Cow::Borrowed(executor_config))
            }
        }
    }

    fn get_command_executor(
        dice: &mut DiceComputations<'_>,
        fs: &ArtifactFs,
        executor_config: &CommandExecutorConfig,
        stage: &TestStage,
        supports_test_execution_caching: bool,
    ) -> buck2_error::Result<CommandExecutor> {
        let CommandExecutorResponse {
            executor,
            platform,
            action_cache_checker,
            remote_dep_file_cache_checker: _,
            cache_uploader,
            output_trees_download_config: _,
            write_action_cache: _,
        } = dice.get_command_executor_from_dice(fs, executor_config)?;

        let (cache_uploader, action_cache_checker) = match stage {
            TestStage::Listing { .. } => (cache_uploader, action_cache_checker),
            TestStage::Testing { .. } => {
                (
                    // We never upload local test executions
                    Arc::new(NoOpCacheUploader {}) as _,
                    if supports_test_execution_caching {
                        action_cache_checker
                    } else {
                        Arc::new(NoOpCommandOptionalExecutor {}) as _
                    },
                )
            }
        };

        let executor = CommandExecutor::new(
            executor,
            action_cache_checker,
            Arc::new(NoOpCommandOptionalExecutor {}),
            cache_uploader,
            None,
            fs.dupe(),
            executor_config.options,
            platform,
        );
        Ok(executor)
    }

    fn get_local_executor(
        dice: &mut DiceComputations<'_>,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<CommandExecutor> {
        let executor_config = CommandExecutorConfig {
            executor: Executor::Local(LocalExecutorOptions::default()),
            options: CommandGenerationOptions {
                path_separator: PathSeparatorKind::system_default(),
                output_paths_behavior: Default::default(),
                use_bazel_protocol_remote_persistent_workers: false,
                network_access: None,
            },
        };
        let CommandExecutorResponse {
            executor,
            platform,
            action_cache_checker: _,
            remote_dep_file_cache_checker: _,
            cache_uploader: _,
            output_trees_download_config: _,
            write_action_cache: _,
        } = dice.get_command_executor_from_dice(fs, &executor_config)?;
        let executor = CommandExecutor::new(
            executor,
            Arc::new(NoOpCommandOptionalExecutor {}),
            Arc::new(NoOpCommandOptionalExecutor {}),
            Arc::new(NoOpCacheUploader {}),
            None,
            fs.dupe(),
            executor_config.options,
            platform,
        );
        Ok(executor)
    }

    async fn get_test_info(
        dice: &mut DiceComputations<'_>,
        test_target: &ConfiguredProvidersLabel,
        internal_runner_config: &InternalRunnerConfig,
    ) -> buck2_error::Result<OwnedTestInfo> {
        let providers = dice
            .get_providers(test_target)
            .await?
            .require_compatible()?;

        // Gate on [test].use_internal_runner, matching the runner
        // selection in command.rs::test_target(). Without this check
        // the orchestrator could resolve fields from the Internal
        // provider while TPX was set up with the External one.
        let internal: Option<OwnedInternalRunnerTestInfo> =
            providers.builtin_provider_value::<InternalRunnerTestInfo>()?;
        if let Some(internal) = internal {
            if internal_runner_config.should_use(internal.as_ref().value().as_ref().test_type()) {
                return Ok(OwnedTestInfo::Internal(internal));
            }
        }

        let external: Option<OwnedExternalRunnerTestInfo> =
            providers.builtin_provider_value::<ExternalRunnerTestInfo>()?;
        if let Some(external) = external {
            return Ok(OwnedTestInfo::External(external));
        }

        Err(internal_error!(
            "Test executable requires ExternalRunnerTestInfo or InternalRunnerTestInfo"
        ))
    }

    async fn get_test_executor(
        dice: &mut DiceComputations<'_>,
        test_target: &ConfiguredProvidersLabel,
        test_info: &OwnedTestInfo,
        executor_override: Option<Arc<ExecutorConfigOverride>>,
        fs: &ArtifactFs,
        stage: &TestStage,
        supports_test_execution_caching: bool,
    ) -> buck2_error::Result<TestExecutor> {
        // NOTE: get_providers() implicitly calls this already but it's not the end of the world
        // since this will get cached in DICE.
        let node = dice
            .get_configured_target_node(test_target.target())
            .await
            .require_compatible()?;

        let resolved_executor_override = match executor_override {
            Some(executor_override) => Some(
                &test_info
                    .executor_override(&executor_override.name)
                    .ok_or_else(|| {
                        internal_error!("The `executor_override` provided does not exist")
                    })
                    .with_buck_error_context(|| {
                        format!(
                            "Error processing `executor_override`: `{}`",
                            executor_override.name
                        )
                    })?
                    .0,
            ),
            None => match stage {
                TestStage::Listing { .. } if test_info.has_executor_overrides() => test_info
                    .executor_override("listing")
                    .or(test_info.default_executor())
                    .map(|o| &o.0),
                _ => test_info.default_executor().map(|o| &o.0),
            },
        };

        let execution_group_executor_config = match (
            &resolved_executor_override,
            test_info.execution_group()?,
        ) {
            (None, None | Some(ExecutionGroup::Default))
                if !node
                    .execution_platform_resolution()
                    .default_execution_group_declared() =>
            {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "The test uses the default execution group, which the rule of `{test_target}` does not declare (the test info may come from another rule that created it). Set `exec_group` or `default_executor` where the test info is created, or add `DefaultExecGroup` to `exec_groups` of the rule of `{test_target}`"
                ));
            }
            (None, Some(group)) => Some(
                node.execution_platform_resolution()
                    .executor_config_for_group(&group)
                    .with_buck_error_context(|| {
                        format!(
                            "The test info of `{test_target}` names execution group `{group}`, but no executor was recorded for it when the test info was created, so it is resolved against `{test_target}`"
                        )
                    })?
                    .dupe(),
            ),
            _ => None,
        };

        let executor_config = Self::executor_config_with_remote_cache_override(
            node,
            resolved_executor_override
                .as_ref()
                .map(|a| &***a)
                .or(execution_group_executor_config.as_deref()),
            stage,
            supports_test_execution_caching,
        )?;

        let executor = Self::get_command_executor(
            dice,
            fs,
            &executor_config,
            stage,
            supports_test_execution_caching,
        )
        .buck_error_context("Error constructing CommandExecutor")?;

        Ok(TestExecutor {
            test_executor: executor,
            executor_config: executor_config.into_owned(),
        })
    }

    /// Replaces each argument carrying argsfile contents with `@<path>`, and returns the files as
    /// action inputs.
    ///
    /// Writing them here, before an executor is chosen, makes each file an input of the action,
    /// so it is hashed into the action digest and reaches remote execution, local execution and
    /// workers by the same path as any other input.
    async fn materialize_argsfiles<'a>(
        dice: &mut DiceComputations<'_>,
        test_target: &ConfiguredProvidersLabel,
        cmd: Cow<'a, [ArgValue]>,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<(Vec<ExpandableArg>, Vec<CommandExecutionInput>)> {
        let digest_config = dice.global_data().get_digest_config();
        let materializer = dice.per_transaction_data().get_materializer();

        let mut inputs = Vec::new();
        let mut expanded = Vec::with_capacity(cmd.len());
        for arg in cmd.into_owned() {
            let ArgValueContent::ArgsfileContents(contents) = arg.content else {
                expanded.push(ExpandableArg::Value(arg));
                continue;
            };
            if arg.format.is_some() {
                return Err(internal_error!(
                    "Argsfile contents do not support a format string"
                ));
            }

            let digest =
                TrackedFileDigest::from_content(&contents, digest_config.cas_digest_config());
            let content_hash = ContentBasedPathHash::new(digest.raw_digest().as_bytes())?;
            let path = BuildArtifactPath::new(
                BaseDeferredKey::TargetLabel(test_target.target().dupe()),
                ForwardRelativePathBuf::unchecked_new(format!(
                    "__test_argsfile_{}__",
                    inputs.len()
                )),
                BuckOutPathKind::ContentHash,
            );
            let project_rel_path = fs
                .buck_out_path_resolver()
                .resolve_gen(&path, Some(&content_hash))?;

            let request = WriteRequest {
                path: project_rel_path.clone(),
                content: contents,
                is_executable: false,
                path_kind: path.path_resolution_method(),
            };
            let written = if materializer.is_disk_state() {
                write_locally(
                    materializer,
                    dice.get_blocking_executor(),
                    fs.fs(),
                    digest_config,
                    request,
                )
                .await
                .map(|_| ())
            } else {
                materializer
                    .declare_write(Box::new(move || Ok(vec![request])))
                    .await
                    .map(|_| ())
            };
            written.buck_error_context("Failed to write test argsfile")?;

            expanded.push(ExpandableArg::Argsfile(project_rel_path));
            inputs.push(CommandExecutionInput::ActionMetadata(ActionMetadataBlob {
                digest,
                path,
                content_hash,
            }));
        }

        Ok((expanded, inputs))
    }

    async fn expand_test_executable<'a>(
        dice: &mut DiceComputations<'_>,
        test_target: &ConfiguredProvidersLabel,
        test_info: &OwnedTestInfo,
        cmd: Cow<'a, [ArgValue]>,
        env: Cow<'a, BuckFixedSortedMap<String, ArgValue>>,
        pre_create_dirs: Cow<'a, [DeclaredOutput]>,
        executor_fs: &ExecutorFs<'_>,
        stage: &TestStage,
        opts: TestSessionOptions,
    ) -> buck2_error::Result<ExpandedTestExecutable> {
        let (cmd, argsfile_inputs) =
            Self::materialize_argsfiles(dice, test_target, cmd, executor_fs.fs()).await?;

        let output_root = resolve_output_root(dice, test_target, stage).await?;

        let mut declared_outputs =
            BuckMutInsertionMap::<BuckOutTestPath, OutputCreationBehavior>::new();

        let mut supports_re = true;

        let cwd;
        let (expanded_cmd, expanded_env, ensured_inputs, expanded_worker) = {
            cwd = if test_info.run_from_project_root() || opts.force_run_from_project_root {
                CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("".to_owned()))
            } else {
                supports_re = false;
                // For compatibility with v1,
                let cell_resolver = dice.get_cell_resolver().await?;
                let cell = cell_resolver.get(test_target.target().pkg().cell_name())?;
                cell.path().to_buf()
            };

            let expander = Execute2RequestExpander {
                test_info,
                stage,
                output_root: &output_root,
                declared_outputs: &mut declared_outputs,
                fs: executor_fs,
                cmd,
                env,
                digest_config: dice.global_data().get_digest_config(),
            };

            let inputs = expander.get_inputs()?;
            // We already built these before reaching out to tpx, so these should already be ready.
            let ensured_inputs =
                KeepGoing::try_compute_join_all(dice, inputs, async |dice, input| {
                    let artifact_group_value = dice.ensure_artifact_group(&input).await?;
                    buck2_error::Ok((input, artifact_group_value))
                })
                .await?;

            let (expanded_cmd, expanded_env, expanded_worker) = if test_info
                .use_project_relative_paths()
                || opts.force_use_project_relative_paths
            {
                expander.expand(&ensured_inputs, false)
            } else {
                supports_re = false;
                expander.expand(&ensured_inputs, true)
            }?;
            (expanded_cmd, expanded_env, ensured_inputs, expanded_worker)
        };

        for output in pre_create_dirs.into_owned() {
            let test_path = BuckOutTestPath::new(output_root.clone(), output.name.into());
            declared_outputs.insert(test_path, OutputCreationBehavior::Create);
        }

        Ok(ExpandedTestExecutable {
            cwd: cwd.as_project_relative_path().to_buf(),
            cmd: expanded_cmd,
            env: expanded_env,
            ensured_inputs,
            declared_outputs,
            argsfile_inputs,
            supports_re,
            worker: expanded_worker,
        })
    }

    async fn create_command_execution_request(
        dice: &mut DiceComputations<'_>,
        cwd: ProjectRelativePathBuf,
        cmd: Vec<String>,
        env: BuckFixedSortedMap<String, String>,
        ensured_inputs: Vec<(ArtifactGroup, ArtifactGroupValues)>,
        declared_outputs: BuckMutInsertionMap<BuckOutTestPath, OutputCreationBehavior>,
        argsfile_inputs: Vec<CommandExecutionInput>,
        fs: &ArtifactFs,
        timeout: Option<Duration>,
        host_sharing_requirements: Option<Arc<HostSharingRequirements>>,
        executor_preference: Option<ExecutorPreference>,
        required_local_resources: Vec<LocalResourceState>,
        worker: Option<WorkerSpec>,
        re_dynamic_image: Option<RemoteExecutorCustomImage>,
        meta_internal_extra_params: Arc<MetaInternalExtraParams>,
        disable_local_network_isolation: bool,
    ) -> buck2_error::Result<CommandExecutionRequest> {
        let inputs = ensured_inputs
            .into_iter()
            .map(|(_, v)| CommandExecutionInput::Artifact(Box::new(v)))
            .chain(argsfile_inputs)
            .collect_vec();

        // NOTE: This looks a bit awkward, that's because fbcode's rustfmt and ours slightly
        // disagree about format here...
        let outputs = declared_outputs
            .into_iter()
            .map(|(path, create)| CommandExecutionOutput::TestPath { path, create })
            .collect();
        let digest_config = dice.global_data().get_digest_config();
        let mut request = CommandExecutionRequest::new(
            vec![],
            cmd,
            CommandExecutionPaths::new(
                inputs,
                outputs,
                fs,
                digest_config,
                dice.per_transaction_data()
                    .get_run_action_knobs()
                    .action_paths_interner
                    .as_ref(),
            )?,
            env,
        );
        let has_resource_control = dice
            .per_transaction_data()
            .data
            .get::<HasResourceControl>()
            .unwrap()
            .0;
        request = request
            .with_working_directory(cwd)
            .with_local_environment_inheritance(EnvironmentInheritance::test_allowlist())
            .with_disable_miniperf(!has_resource_control)
            .with_worker(worker)
            .with_remote_execution_custom_image(re_dynamic_image)
            .with_meta_internal_extra_params(meta_internal_extra_params)
            .with_required_local_resources(required_local_resources)?
            .with_disable_local_network_isolation(disable_local_network_isolation)
            .with_is_test();
        if let Some(timeout) = timeout {
            request = request.with_timeout(timeout)
        }
        if let Some(host_sharing_requirements) = host_sharing_requirements {
            request = request.with_host_sharing_requirements(host_sharing_requirements.dupe());
        }
        if let Some(executor_preference) = executor_preference {
            request = request.with_executor_preference(executor_preference);
        }
        Ok(request)
    }

    async fn setup_local_resources(
        dice: &mut DiceComputations<'_>,
        cancellation: &CancellationContext,
        required_providers: Vec<(&'_ ConfiguredTargetLabel, OwnedLocalResourceInfo)>,
        executor: CommandExecutor,
        default_timeout: Duration,
        liveliness_observer: Arc<dyn LivelinessObserver>,
    ) -> Result<Vec<LocalResourceState>, ExecuteError> {
        if required_providers.is_empty() {
            return Ok(vec![]);
        }
        // The registry owns one pool per configured target, including its subtargets.
        let required_providers = required_providers
            .into_iter()
            .unique_by(|(target, _)| (*target).dupe())
            .collect::<Vec<_>>();
        let setup_commands = dice
            .try_compute_join(required_providers, async |dice, provider| {
                Self::prepare_local_resource(
                    dice,
                    provider,
                    &executor.fs(),
                    &executor.executor_fs(),
                    default_timeout,
                )
                .await
            })
            .await?;

        Self::require_alive(liveliness_observer.dupe()).await?;
        let events = dice.per_transaction_data().get_dispatcher().dupe();
        let digest_config = dice.global_data().get_digest_config();

        // TODO(romanp): The code below is not optimal. We are locking the entire registry here, but we could have better concurrency.
        // For example, if different suites require different local resources and can execute in parallel, this code runs sequentially but should run in parallel.
        // An easy fix would be to introduce an RwLock instead of a mutex. In this case, suites that have the necessary resources and do not require write access
        // can be executed in parallel.
        let local_resource_state_registry = dice.get_local_resource_registry()?;
        let required_targets = setup_commands
            .iter()
            .map(|ctx| ctx.target.dupe())
            .collect::<Vec<_>>();
        let mut lock = local_resource_state_registry.0.lock().await;

        let resource_futs = setup_commands
            .into_iter()
            .filter(|ctx| !lock.contains_key(&ctx.target))
            .map(|ctx| {
                let missing_target = ctx.target.dupe();
                let setup = Self::start_local_resource(
                    events.dupe(),
                    liveliness_observer.dupe(),
                    digest_config.dupe(),
                    executor.dupe(),
                    ctx,
                    cancellation,
                );
                async move {
                    (
                        missing_target.dupe(),
                        LocalResourceSetupEvent::Prepared(setup.await.with_buck_error_context(
                            || {
                                format!(
                                    "Error setting up local resource declared in `{missing_target}`"
                                )
                            },
                        )),
                    )
                }
                .boxed()
            });
        let mut completed = futures::stream::FuturesUnordered::from_iter(resource_futs);
        while let Some((target, event)) = completed.next().await {
            match event {
                LocalResourceSetupEvent::Prepared(Ok(PreparedLocalResource {
                    state,
                    ownership: Some(mut owner),
                })) => {
                    lock.insert(
                        target.dupe(),
                        Ok(LocalResourceEntry::Pending {
                            state,
                            lease: owner.lease.dupe(),
                        }),
                    );
                    let liveliness = liveliness_observer.dupe();
                    completed.push(async move {
                        let result = cancellation.with_structured_cancellation(|observer| async {
                            futures::pin_mut!(observer);
                            let registered = tokio::select! {
                                result = owner.register() => result,
                                _ = &mut observer => Err(buck2_error::buck2_error!(ErrorTag::LocalResourceSetup, "Local resource registration cancelled")),
                                _ = liveliness.while_alive() => Err(buck2_error::buck2_error!(ErrorTag::LocalResourceSetup, "Local resource request ended during registration")),
                            };
                            registered?;
                            if observer.is_cancelled() { return Err(buck2_error::buck2_error!(ErrorTag::LocalResourceSetup, "Local resource registration cancelled")); }
                            Self::require_alive(liveliness.dupe()).await.map_err(|_| buck2_error::buck2_error!(ErrorTag::LocalResourceSetup, "Local resource request ended before registration commit"))?;
                            Ok(())
                        }).await;
                        (target, LocalResourceSetupEvent::Registered(result.map(|()| owner)))
                    }.boxed());
                }
                LocalResourceSetupEvent::Registered(Ok(owner)) => {
                    if !liveliness_observer.is_alive().await {
                        lock.insert(
                            target,
                            Err(buck2_error::buck2_error!(
                                ErrorTag::LocalResourceSetup,
                                "Local resource request ended before registration commit"
                            )),
                        );
                        continue;
                    }
                    let pending = lock
                        .remove(&target)
                        .expect("Pending entry inserted before registration")?;
                    let LocalResourceEntry::Pending { state, .. } = pending else {
                        unreachable!("Entry remains pending under registry lock")
                    };
                    lock.insert(
                        target,
                        Ok(LocalResourceEntry::Registered {
                            state,
                            lease: owner.commit(),
                        }),
                    );
                }
                LocalResourceSetupEvent::Prepared(Ok(PreparedLocalResource {
                    state,
                    ownership: None,
                })) => {
                    lock.insert(target, Ok(LocalResourceEntry::Legacy(state)));
                }
                LocalResourceSetupEvent::Prepared(Err(error))
                | LocalResourceSetupEvent::Registered(Err(error)) => {
                    lock.insert(target, Err(error));
                }
            }
        }

        let result: buck2_error::Result<Vec<_>> = required_targets
            .iter()
            .map(|t| lock.get(t).unwrap().as_ref().map_err(Clone::clone)?.state())
            .collect();
        Ok(result?)
    }

    async fn prepare_local_resource(
        dice: &mut DiceComputations<'_>,
        provider: (&ConfiguredTargetLabel, OwnedLocalResourceInfo),
        fs: &ArtifactFs,
        executor_fs: &ExecutorFs<'_>,
        default_timeout: Duration,
    ) -> buck2_error::Result<PreparedLocalResourceSetupContext> {
        let digest_config = dice.global_data().get_digest_config();
        let root = dice.get_cell_resolver().await?.root_cell();
        let enabled = dice
            .get_legacy_config_property(
                root,
                BuckconfigKeyRef {
                    section: "buck2",
                    property: "local_resource_cleanup_registration",
                },
            )
            .await?;
        let enabled = enabled
            .as_deref()
            .unwrap_or("false")
            .parse::<bool>()
            .map_err(|_| {
                buck2_error::buck2_error!(
                    ErrorTag::LocalResourceSetup,
                    "Invalid cleanup registration buckconfig value"
                )
            })?
            && cfg!(target_os = "linux");
        let cleanup_setup = if enabled {
            Some(spawn_blocking(CleanupSetup::create).await??)
        } else {
            None
        };
        let env = [(
            CLEANUP_CAPABILITY_ENV.to_owned(),
            cleanup_setup
                .as_ref()
                .map_or_else(String::new, |s| s.capability.to_owned()),
        )]
        .into_iter()
        .collect();

        let (target, provider) = provider;
        // The `'v`-branded view of the provider must not be held across an await (only the
        // `OwnedFrozen` may be), so this is scoped and re-derived below.
        let visited_inputs = {
            let info = provider.as_ref().value().as_ref();
            let mut artifact_visitor = SimpleCommandLineArtifactVisitor::new();
            info.setup_command_line()
                .visit_artifacts(&mut artifact_visitor)?;
            artifact_visitor.inputs
        };

        let inputs = dice
            .try_compute_join(visited_inputs, async |dice, group| {
                dice.ensure_artifact_group(&group).await
            })
            .await?;

        let info = provider.as_ref().value().as_ref();
        let artifact_path_mapping: BuckMutMap<_, _> = inputs
            .iter()
            .flat_map(|v| v.iter())
            .map(|(a, v)| (a, v.content_based_path_hash()))
            .collect();
        let mut cmd: Vec<String> = vec![];
        info.setup_command_line()
            .add_to_command_line(&mut CommandLineBuilder::new(
                &mut cmd,
                &artifact_path_mapping,
                executor_fs,
            ))?;

        let inputs = inputs
            .into_iter()
            .map(|group_values| CommandExecutionInput::Artifact(Box::new(group_values)))
            .collect();
        let paths = CommandExecutionPaths::new(
            inputs,
            buck_mut_insertion_set![],
            fs,
            digest_config,
            dice.per_transaction_data()
                .get_run_action_knobs()
                .action_paths_interner
                .as_ref(),
        )?;
        let mut execution_request = CommandExecutionRequest::new(vec![], cmd, paths, env);
        execution_request =
            execution_request.with_timeout(info.setup_timeout().unwrap_or(default_timeout));
        execution_request = execution_request.with_skip_resource_control();
        Ok(PreparedLocalResourceSetupContext {
            target: target.dupe(),
            execution_request,
            env_var_mapping: info.env_var_mapping(),
            cleanup_setup,
        })
    }

    async fn start_local_resource(
        events: EventDispatcher,
        liveliness_observer: Arc<dyn LivelinessObserver>,
        digest_config: DigestConfig,
        executor: CommandExecutor,
        mut context: PreparedLocalResourceSetupContext,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<PreparedLocalResource> {
        let mut cleanup_setup = context.cleanup_setup.take();
        let manager = CommandExecutionManager::new(
            Box::new(MutexClaimManager::new()),
            events.dupe(),
            liveliness_observer,
            WaitingData::new(),
        );

        let local_resource_target = LocalResourceTarget {
            target: &context.target,
        };
        let prepared_action =
            executor.prepare_action(&context.execution_request, digest_config, false)?;
        let prepared_command = PreparedCommand {
            target: &local_resource_target as _,
            request: &context.execution_request,
            prepared_action: &prepared_action,
            digest_config,
        };
        let registration_deadline = context
            .execution_request
            .timeout()
            .map(|timeout| Instant::now() + timeout);
        let command = executor.exec_cmd(manager, &prepared_command, cancellation);

        let start = SetupLocalResourcesStart {
            target_label: Some(context.target.as_proto()),
        };
        let end = SetupLocalResourcesEnd {};
        let execution_result = events
            .span_async(start, async {
                let execution = async {
                    futures::pin_mut!(command);
                    if let Some(setup) = cleanup_setup.as_mut() {
                        tokio::select! {
                            result = &mut command => return Ok::<_, buck2_error::Error>(result),
                            result = setup.accept() => result?,
                        }
                    }
                    Ok(command.await)
                };
                (execution.await, end)
            })
            .await?;

        let CommandExecutionResult {
            outputs: _,
            report:
                CommandExecutionReport {
                    std_streams,
                    exit_code,
                    status,
                    ..
                },
            ..
        } = execution_result;

        let std_streams = std_streams
            .into_bytes()
            .await
            .buck_error_context("Error accessing setup local resource output")?;

        match status {
            CommandExecutionStatus::Success { .. } => {}
            CommandExecutionStatus::Failure { .. }
            | CommandExecutionStatus::WorkerFailure { .. } => {
                return Err(buck2_error::buck2_error!(
                    ErrorTag::LocalResourceSetup,
                    "Local resource setup command failed with `{}` exit code, stdout:\n{}\nstderr:\n{}\n",
                    exit_code.unwrap_or(1),
                    String::from_utf8_lossy(&std_streams.stdout),
                    String::from_utf8_lossy(&std_streams.stderr),
                ));
            }
            CommandExecutionStatus::TimedOut { duration, .. } => {
                return Err(buck2_error::buck2_error!(
                    ErrorTag::LocalResourceSetup,
                    "Local resource setup command timed out after `{}s`, stdout:\n{}\nstderr:\n{}\n",
                    duration.as_secs(),
                    String::from_utf8_lossy(&std_streams.stdout),
                    String::from_utf8_lossy(&std_streams.stderr),
                ));
            }
            CommandExecutionStatus::Error { error, .. } => {
                return Err(error);
            }
            CommandExecutionStatus::Cancelled { .. } => {
                return Err(buck2_error::buck2_error!(
                    ErrorTag::LocalResourceSetup,
                    "Local resource setup command cancelled"
                ));
            }
        };

        let string_content = String::from_utf8_lossy(&std_streams.stdout);
        let mut data: LocalResourcesSetupResult = serde_json::from_str(&string_content)
            // .buck_error_context("Error parsing local resource setup command output")
            .map_err(|e| from_any_with_tag(e, ErrorTag::LocalResourceSetup))?;
        let descriptor = data.cleanup_registration.take();
        let ownership = match cleanup_setup {
            Some(setup) => setup.finish(descriptor, data.pid, registration_deadline)?,
            None if descriptor.is_none() => None,
            None => {
                return Err(buck2_error::buck2_error!(
                    ErrorTag::LocalResourceSetup,
                    "Cleanup registration is not enabled"
                ));
            }
        };
        let state = data.into_state(context.target.dupe(), &context.env_var_mapping)?;

        Ok(PreparedLocalResource { state, ownership })
    }
}

impl Drop for BuckTestOrchestrator<'_> {
    fn drop(&mut self) {
        // If we didn't close the sender yet, then notify the receiver that our stream is
        // incomplete.
        let _ignored = self
            .results_channel
            .unbounded_send(Err(buck2_error::internal_error!(
                "BuckTestOrchestrator exited before end-of-tests was received",
            )));
    }
}

/// A command argument on its way to an executor, once argsfiles have been written.
enum ExpandableArg {
    Value(ArgValue),
    /// An argsfile the runner supplied, already written and part of the action's inputs.
    Argsfile(ProjectRelativePathBuf),
}

/// Renders an argsfile argument as `@<path>`.
fn push_argsfile_arg(
    fmt: &mut CommandLineBuilder<'_, '_>,
    path: ProjectRelativePathBuf,
) -> buck2_error::Result<()> {
    fmt.push_scope_format("@{}");
    fmt.push_project_path(path)?;
    fmt.pop_scope();
    Ok(())
}

struct Execute2RequestExpander<'a> {
    test_info: &'a OwnedTestInfo,
    stage: &'a TestStage,
    output_root: &'a ForwardRelativePath,
    declared_outputs: &'a mut BuckMutInsertionMap<BuckOutTestPath, OutputCreationBehavior>,
    fs: &'a ExecutorFs<'a>,
    cmd: Vec<ExpandableArg>,
    env: Cow<'a, BuckFixedSortedMap<String, ArgValue>>,
    digest_config: DigestConfig,
}

fn make_visit_arg_artifacts<'v>(
    cli_args_for_interpolation: Vec<&'v dyn CommandLineArgLike<'v>>,
    env_for_interpolation: BuckMutMap<&'v str, &'v dyn CommandLineArgLike<'v>>,
) -> impl for<'a> Fn(&'a mut dyn CommandLineArtifactVisitor<'v>, &'a ArgValue) -> buck2_error::Result<()>
{
    move |artifact_visitor: &mut dyn CommandLineArtifactVisitor<'_>, value: &ArgValue| {
        match &value.content {
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::ArgHandle(h)) => {
                let arg = cli_args_for_interpolation
                    .get(h.0)
                    .with_internal_error(|| format!("Invalid ArgHandle: {h:?}"))?;
                arg.visit_artifacts(artifact_visitor)?;
            }
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::EnvHandle(h)) => {
                let arg = env_for_interpolation
                    .get(h.0.as_str())
                    .with_internal_error(|| format!("Invalid EnvHandle: {h:?}"))?;
                arg.visit_artifacts(artifact_visitor)?;
            }
            ArgValueContent::DeclaredOutput(_)
            | ArgValueContent::ExternalRunnerSpecValue(_)
            | ArgValueContent::ArgsfileContents(_) => {}
        };

        buck2_error::Ok(())
    }
}

impl<'a> Execute2RequestExpander<'a> {
    fn get_inputs(&self) -> buck2_error::Result<BuckMutInsertionSet<ArtifactGroup>> {
        let Execute2RequestExpander {
            test_info,
            stage,
            cmd,
            env,
            ..
        } = self;
        let cli_args_for_interpolation = test_info.cli_args_for_stage(stage);
        let env_for_interpolation = test_info.env_args();

        let visit_arg_artifacts =
            make_visit_arg_artifacts(cli_args_for_interpolation, env_for_interpolation);

        let mut artifact_visitor = SimpleCommandLineArtifactVisitor::new();
        for var in cmd.iter() {
            if let ExpandableArg::Value(var) = var {
                visit_arg_artifacts(&mut artifact_visitor, var)?;
            }
        }

        for (_, var) in env.iter() {
            visit_arg_artifacts(&mut artifact_visitor, var)?;
        }
        let worker_exe = test_info.worker().map(|worker| worker.exe_command_line());
        if let Some(worker_exe) = worker_exe {
            worker_exe.visit_artifacts(&mut artifact_visitor)?;
        }

        Ok(artifact_visitor.inputs)
    }

    fn expand_arg_value<'v>(
        fmt: &mut CommandLineBuilder<'v, '_>,
        declared_outputs: &mut BuckMutInsertionMap<BuckOutTestPath, OutputCreationBehavior>,
        value: &'v ArgValue,
        cli_args_for_interpolation: &[&dyn CommandLineArgLike<'v>],
        env_for_interpolation: &BuckMutMap<&str, &dyn CommandLineArgLike<'v>>,
        output_root: &ForwardRelativePath,
        fs: &ExecutorFs<'_>,
    ) -> buck2_error::Result<()> {
        let ArgValue { content, format } = value;

        if let Some(format) = format {
            fmt.push_scope_format(format);
        }

        match content {
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::Verbatim(v)) => {
                v.as_str().add_to_command_line(fmt)?;
            }
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::ArgHandle(h)) => {
                let arg = cli_args_for_interpolation
                    .get(h.0)
                    .with_internal_error(|| format!("Invalid ArgHandle: {h:?}"))?;
                arg.add_to_command_line(fmt)?;
            }
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::EnvHandle(h)) => {
                let arg = env_for_interpolation
                    .get(h.0.as_str())
                    .with_internal_error(|| format!("Invalid EnvHandle: {h:?}"))?;
                arg.add_to_command_line(fmt)?;
            }
            ArgValueContent::ArgsfileContents(_) => {
                return Err(internal_error!(
                    "Argsfile contents are only valid as command arguments"
                ));
            }
            ArgValueContent::DeclaredOutput(output) => {
                let test_path = BuckOutTestPath::new(output_root.to_owned(), output.name.clone());
                let path = fs.fs().buck_out_path_resolver().resolve_test(&test_path);
                fmt.push_project_path(path)?;
                declared_outputs.insert(test_path, OutputCreationBehavior::Parent);
            }
        };

        if format.is_some() {
            fmt.pop_scope();
        }

        buck2_error::Ok(())
    }

    /// Expand a command and env.
    fn expand(
        self,
        ensured_inputs: &Vec<(ArtifactGroup, ArtifactGroupValues)>,
        absolute: bool,
    ) -> buck2_error::Result<(
        Vec<String>,
        BuckFixedSortedMap<String, String>,
        Option<WorkerSpec>,
    )> {
        let Execute2RequestExpander {
            test_info,
            stage,
            output_root,
            declared_outputs,
            fs,
            cmd,
            env,
            digest_config,
        } = self;
        let cli_args_for_interpolation = test_info.cli_args_for_stage(stage);
        let env_for_interpolation = test_info.env_args();

        let artifact_path_mapping = ArtifactPathMapperImpl::from(ensured_inputs);

        let mut expanded_cmd = Vec::<String>::new();
        let mut cmd_fmt = CommandLineBuilder::new_with_options(
            &mut expanded_cmd,
            &artifact_path_mapping,
            self.fs,
            absolute,
            None,
        );
        for var in cmd.iter() {
            match var {
                ExpandableArg::Argsfile(path) => push_argsfile_arg(&mut cmd_fmt, path.clone())?,
                ExpandableArg::Value(var) => Self::expand_arg_value(
                    &mut cmd_fmt,
                    declared_outputs,
                    var,
                    &cli_args_for_interpolation,
                    &env_for_interpolation,
                    output_root,
                    fs,
                )?,
            }
        }

        let expanded_env = env
            .as_ref()
            .into_iter()
            .map(|(k, v)| {
                let mut curr_env = SingletonCommandLineSink::new();
                let mut fmt = CommandLineBuilder::new_with_options(
                    &mut curr_env,
                    &artifact_path_mapping,
                    self.fs,
                    absolute,
                    None,
                );
                fmt.push_scope_delimiter(" ");
                Self::expand_arg_value(
                    &mut fmt,
                    declared_outputs,
                    &v,
                    &cli_args_for_interpolation,
                    &env_for_interpolation,
                    output_root,
                    fs,
                )?;
                fmt.pop_scope();
                buck2_error::Ok((k.to_owned(), curr_env.finalize()?))
            })
            .collect::<Result<BuckFixedSortedMap<_, _>, _>>()?;

        let expanded_worker = match test_info.worker() {
            Some(worker) => {
                let mut worker_rendered = Vec::<String>::new();
                let worker_exe = worker.exe_command_line();
                let mut fmt =
                    CommandLineBuilder::new(&mut worker_rendered, &artifact_path_mapping, self.fs);
                worker_exe.add_to_command_line(&mut fmt)?;
                let worker_env: buck2_error::Result<BuckFixedSortedMap<_, _>> = worker
                    .env()
                    .into_iter()
                    .map(|(k, v)| {
                        let mut env = SingletonCommandLineSink::new();
                        let mut fmt =
                            CommandLineBuilder::new(&mut env, &artifact_path_mapping, self.fs);
                        fmt.push_scope_delimiter(" ");
                        v.add_to_command_line(&mut fmt)?;
                        fmt.pop_scope();
                        Ok((k.to_owned(), env.finalize()?))
                    })
                    .collect();

                Some(WorkerSpec {
                    exe: worker_rendered,
                    id: WorkerId(worker.id),
                    env: worker_env?,
                    concurrency: worker.concurrency(),
                    streaming: worker.streaming(),
                    remote_key: None,
                    // TODO(ianc): Support input_paths on test workers
                    input_paths: CommandExecutionPaths::new(
                        vec![],
                        buck_mut_insertion_set![],
                        fs.fs(),
                        digest_config,
                        None,
                    )?,
                })
            }
            _ => None,
        };

        Ok((expanded_cmd, expanded_env, expanded_worker))
    }
}

fn test_labels(test_info: &OwnedTestInfo) -> Vec<String> {
    match test_info {
        OwnedTestInfo::External(info) => info
            .as_ref()
            .value()
            .as_ref()
            .labels()
            .map(str::to_owned)
            .collect(),
        OwnedTestInfo::Internal(info) => info
            .as_ref()
            .value()
            .as_ref()
            .labels()
            .map(str::to_owned)
            .collect(),
    }
}

async fn resolve_output_root(
    dice: &mut DiceComputations<'_>,
    test_target: &ConfiguredProvidersLabel,
    stage: &TestStage,
) -> Result<ForwardRelativePathBuf, buck2_error::Error> {
    let resolver = dice.get_buck_out_path().await?;

    let output_root = match stage {
        TestStage::Listing { .. } => resolver
            .resolve_test_discovery(test_target)?
            .into_forward_relative_path_buf(),
        TestStage::Testing {
            testcases,
            variant,
            repeat_count,
            ..
        } => {
            let mut hasher = DefaultHasher::new();
            variant.hash(&mut hasher);
            repeat_count.hash(&mut hasher);
            testcases.hash(&mut hasher);
            let extra_info_hashed = format!("{:016x}", hasher.finish());

            resolver
                .resolve_test_execution(
                    test_target,
                    ForwardRelativePath::unchecked_new(&extra_info_hashed),
                )?
                .into_forward_relative_path_buf()
        }
    };
    Ok(output_root)
}

struct ExpandedTestExecutable {
    cwd: ProjectRelativePathBuf,
    cmd: Vec<String>,
    env: BuckFixedSortedMap<String, String>,
    ensured_inputs: Vec<(ArtifactGroup, ArtifactGroupValues)>,
    supports_re: bool,
    declared_outputs: BuckMutInsertionMap<BuckOutTestPath, OutputCreationBehavior>,
    /// Argsfiles the runner supplied, already written; these become inputs of the action.
    argsfile_inputs: Vec<CommandExecutionInput>,
    worker: Option<WorkerSpec>,
}

fn create_prepare_for_local_execution_result(
    fs: &ArtifactFs,
    request: CommandExecutionRequest,
    local_resource_setup_commands: Vec<PreparedLocalResourceSetupContext>,
) -> PrepareForLocalExecutionResult {
    let relative_cwd = request.working_directory();
    let cwd = fs.fs().resolve(relative_cwd);
    let cmd = request.all_args_vec();

    let mut env = LossyEnvironment::new();
    apply_local_execution_environment(
        &mut env,
        &cwd,
        request.env(),
        request.local_environment_inheritance(),
    );

    let local_resource_setup_commands = local_resource_setup_commands
        .into_iter()
        .map(|r| local_resource_setup_command_prepared_for_local_execution(fs, r))
        .collect::<Vec<_>>();

    PrepareForLocalExecutionResult {
        command: LocalExecutionCommand {
            cmd,
            env: env.into_inner(),
            cwd,
        },
        local_resource_setup_commands,
    }
}

fn local_resource_setup_command_prepared_for_local_execution(
    fs: &ArtifactFs,
    resource_setup_command: PreparedLocalResourceSetupContext,
) -> LocalExecutionCommand {
    let relative_cwd = resource_setup_command.execution_request.working_directory();
    let cwd = fs.fs().resolve(relative_cwd);
    let cmd = resource_setup_command.execution_request.all_args_vec();

    let mut env = LossyEnvironment::new();
    apply_local_execution_environment(
        &mut env,
        &cwd,
        resource_setup_command.execution_request.env(),
        resource_setup_command
            .execution_request
            .local_environment_inheritance(),
    );

    LocalExecutionCommand {
        cmd,
        env: env.into_inner(),
        cwd,
    }
}

struct LossyEnvironment {
    inner: BuckMutInsertionMap<String, String>,
}

impl LossyEnvironment {
    fn new() -> Self {
        Self {
            inner: BuckMutInsertionMap::new(),
        }
    }

    fn into_inner(self) -> BuckFixedSortedMap<String, String> {
        self.inner.into()
    }
}

impl EnvironmentBuilder for LossyEnvironment {
    fn clear(&mut self) {
        self.inner.clear();
    }

    fn set<K, V>(&mut self, key: K, val: V)
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.insert(
            key.as_ref().to_string_lossy().into_owned(),
            val.as_ref().to_string_lossy().into_owned(),
        );
    }

    fn remove<K>(&mut self, key: K)
    where
        K: AsRef<OsStr>,
    {
        self.inner.shift_remove(&*key.as_ref().to_string_lossy());
    }
}

#[derive(Debug)]
struct TestTarget<'a> {
    target: &'a ConfiguredTargetLabel,
    action_key_suffix: String,
}

impl CommandExecutionTarget for TestTarget<'_> {
    fn re_action_key(&self) -> String {
        format!("{} test {}", self.target, self.action_key_suffix)
    }

    fn re_affinity_key(&self) -> String {
        self.target.to_string()
    }

    fn as_proto_action_key(&self) -> buck2_data::ActionKey {
        buck2_data::ActionKey {
            id: Default::default(),
            owner: Some(buck2_data::action_key::Owner::TestTargetLabel(
                self.target.as_proto(),
            )),
            key: Default::default(),
        }
    }

    fn as_proto_action_name(&self) -> buck2_data::ActionName {
        buck2_data::ActionName {
            category: "test".to_owned(),
            identifier: "".to_owned(),
        }
    }

    fn action_mnemonic(&self) -> Option<String> {
        Some("test".to_owned())
    }

    fn target_label(&self) -> Option<String> {
        Some(request_metadata_target_id(self.target))
    }

    fn configuration_hash(&self) -> Option<String> {
        Some(self.target.cfg().output_hash().as_str().to_owned())
    }
}

fn create_action_key_suffix(stage: &TestStage) -> String {
    let mut action_key_suffix = match &stage {
        TestStage::Listing { .. } => "listing".to_owned(),
        TestStage::Testing {
            testcases, variant, ..
        } => {
            if let Some(variant) = variant {
                format!("{} {}", variant, testcases.join(" "),)
            } else {
                testcases.join(" ")
            }
        }
    };
    if action_key_suffix.len() > MAX_SUFFIX_LEN {
        let truncated = "(truncated)";
        let max_len = MAX_SUFFIX_LEN - truncated.len();
        let truncate_at = action_key_suffix.floor_char_boundary(max_len);
        action_key_suffix.truncate(truncate_at);
        action_key_suffix += truncated;
    }
    action_key_suffix
}

#[derive(Debug)]
struct LocalResourceTarget<'a> {
    target: &'a ConfiguredTargetLabel,
}

impl CommandExecutionTarget for LocalResourceTarget<'_> {
    fn re_action_key(&self) -> String {
        String::new()
    }

    fn re_affinity_key(&self) -> String {
        String::new()
    }

    fn as_proto_action_key(&self) -> buck2_data::ActionKey {
        buck2_data::ActionKey {
            id: Default::default(),
            owner: Some(buck2_data::action_key::Owner::LocalResourceSetup(
                self.target.as_proto(),
            )),
            key: Default::default(),
        }
    }

    fn as_proto_action_name(&self) -> buck2_data::ActionName {
        buck2_data::ActionName {
            category: "setup_local_resource".to_owned(),
            identifier: "".to_owned(),
        }
    }

    fn action_mnemonic(&self) -> Option<String> {
        Some("setup_local_resource".to_owned())
    }

    fn target_label(&self) -> Option<String> {
        Some(request_metadata_target_id(self.target))
    }

    fn configuration_hash(&self) -> Option<String> {
        Some(self.target.cfg().output_hash().as_str().to_owned())
    }
}

struct TestExecutor {
    test_executor: CommandExecutor,
    executor_config: CommandExecutorConfig,
}

impl TestExecutor {
    pub fn re_cache_enabled(&self) -> bool {
        self.executor_config.re_cache_enabled()
    }

    pub fn executor(&self) -> &CommandExecutor {
        &self.test_executor
    }

    pub fn re_dynamic_image(&self) -> Option<RemoteExecutorCustomImage> {
        if let Executor::RemoteEnabled(options) = &self.executor_config.executor {
            options.custom_image.clone().map(|image| *image)
        } else {
            None
        }
    }

    pub fn meta_internal_extra_params(&self) -> Arc<MetaInternalExtraParams> {
        if let Executor::RemoteEnabled(options) = &self.executor_config.executor {
            options.meta_internal_extra_params.clone()
        } else {
            MetaInternalExtraParams::default_arc()
        }
    }
}

/// The exit code reported for a failed execution. Runners read exit code 0 as a pass, and an
/// executor can fail with exit code 0 (a worker that exits 0 before connecting), so a failure
/// always reports a non-zero code.
fn failure_exit_code(exit_code: Option<i32>) -> i32 {
    match exit_code {
        Some(0) | None => 1,
        Some(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use buck2_build_api::context::SetBuildContextData;
    use buck2_common::dice::cells::SetCellResolver;
    use buck2_common::dice::data::SetIsolationDir;
    use buck2_common::dice::data::testing::SetTestingIoProvider;
    use buck2_common::file_ops::baseline::FileSystemBaseline;
    use buck2_common::file_ops::baseline::SetFileSystemBaseline;
    use buck2_common::liveliness_observer::NoopLivelinessObserver;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::name::CellName;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::fs::buck_out_path::BuckOutPathResolver;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_execute::digest_config::SetDigestConfig;
    use buck2_execute::execute::action_digest::ActionDigest;
    use buck2_execute::execute::manager::CommandExecutionManagerExt;
    use buck2_execute::execute::output::CommandStdStreams;
    use buck2_execute::execute::prepared::PreparedCommandExecutor;
    use buck2_execute::re::manager::UnconfiguredRemoteExecutionClient;
    use buck2_test_api::data::TestStage;
    use buck2_test_api::data::TestStatus;
    use buck2_util::time_span::TimeSpan;
    use dice::UserComputationData;
    use dice::testing::DiceBuilder;
    use futures::channel::mpsc;
    use futures::channel::mpsc::UnboundedReceiver;
    use futures::future;
    use futures::stream::TryStreamExt;

    use super::*;

    fn make_test_command_report() -> CommandExecutionReport {
        let digest_config = DigestConfig::testing_default();
        CommandExecutionReport {
            claim: None,
            status: CommandExecutionStatus::Success {
                execution_kind: CommandExecutionKind::Local {
                    digest: ActionDigest::empty(digest_config.cas_digest_config()),
                    command: vec!["test-binary".to_owned()],
                    env: BuckFixedSortedMap::default(),
                },
            },
            timing: CommandExecutionMetadata::empty(buck2_util::time_span::TimeSpan::empty_now()),
            std_streams: CommandStdStreams::Local {
                stdout: b"test stdout".to_vec(),
                stderr: b"test stderr".to_vec(),
            },
            exit_code: Some(0),
            additional_message: None,
            inline_environment_metadata: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_run_command_report_preserves_streams() {
        let report = make_test_command_report();
        let proto = test_run_command_report(&report).await;
        let details = proto.details.as_ref().expect("command details");

        assert_eq!(details.cmd_stdout, "test stdout");
        assert_eq!(details.cmd_stderr, "test stderr");
    }

    #[test]
    fn test_timeout_proto_preserves_seconds() {
        let timeout = test_timeout_proto(Some(Duration::from_secs(42))).expect("timeout");

        assert_eq!(timeout.seconds, 42);
        assert_eq!(timeout.nanos, 0);
    }

    async fn make() -> buck2_error::Result<(
        BuckTestOrchestrator<'static>,
        UnboundedReceiver<buck2_error::Result<ExecutorMessage>>,
    )> {
        let fs = ProjectRootTemp::new().unwrap();

        let cell_resolver = CellResolver::testing_with_name_and_path(
            CellName::testing_new("cell"),
            CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("cell".to_owned())),
        );
        let buckout_path = ProjectRelativePathBuf::unchecked_new("buck_out/v2".into());
        let mut dice = DiceBuilder::new()
            .set_data(|d| d.set_testing_io_provider(&fs))
            .build(UserComputationData::new())
            .await
            .unwrap();
        dice.set_isolation_dir(fs.path().resolve(&buckout_path))?;
        dice.set_buck_out_path(Some(buckout_path))?;
        dice.set_cell_resolver(cell_resolver)?;
        dice.set_file_system_baseline(FileSystemBaseline::unique())?;

        let dice = dice.commit().await;

        let (sender, receiver) = mpsc::unbounded();

        let re_client = Arc::new(remote_storage::ReClientWithCache::new(
            UnconfiguredRemoteExecutionClient::testing_new_dummy(),
        ));

        Ok((
            BuckTestOrchestrator::from_parts(
                dice,
                Arc::new(TestSession::new(Default::default())),
                NoopLivelinessObserver::create(),
                sender,
                EventDispatcher::null(),
                CancellationContext::testing(),
                re_client,
                InternalRunnerConfig::parse(None),
            ),
            receiver,
        ))
    }

    /// An executor that reports a failure with exit code 0, as a test worker that exits 0 before
    /// connecting does (`WorkerInitError::EarlyExit { exit_code: Some(0) }`).
    struct FailureWithExitCodeZero;

    #[async_trait]
    impl PreparedCommandExecutor for FailureWithExitCodeZero {
        async fn exec_cmd(
            &self,
            command: &PreparedCommand<'_, '_>,
            manager: CommandExecutionManager,
            _cancellations: &CancellationContext,
        ) -> CommandExecutionResult {
            let kind = CommandExecutionKind::Local {
                digest: command.prepared_action.digest(),
                command: vec![],
                env: BuckFixedSortedMap::new(),
            };
            manager.claim().await.failure(
                kind,
                Default::default(),
                CommandStdStreams::Local {
                    stdout: Vec::new(),
                    stderr: b"worker exited before connecting".to_vec(),
                },
                Some(0),
                CommandExecutionMetadata::empty(TimeSpan::empty_now()),
                None,
                false,
            )
        }

        fn is_local_execution_possible(&self, _executor_preference: ExecutorPreference) -> bool {
            true
        }
    }

    /// A failed execution is never reported with exit code 0, which the runners take as a pass
    /// and `TestExecutionKey::validity` caches as a successful listing.
    #[tokio::test]
    async fn failure_with_exit_code_zero_is_reported_as_a_failure() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new().unwrap();
        let digest_config = DigestConfig::testing_default();
        let mut data = UserComputationData::new();
        data.data.set(EventDispatcher::null());
        let dice = DiceBuilder::new()
            .set_data(|d| {
                d.set_testing_io_provider(&temp);
                d.set_digest_config(digest_config);
            })
            .build(data)
            .await
            .unwrap()
            .commit()
            .await;

        let fs = ArtifactFs::new(
            CellResolver::testing_with_name_and_path(
                CellName::testing_new("cell"),
                CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("cell".to_owned())),
            ),
            BuckOutPathResolver::new(ProjectRelativePathBuf::unchecked_new("buck_out/v2".into())),
            temp.path().dupe(),
        );
        let executor = CommandExecutor::new(
            Arc::new(FailureWithExitCodeZero),
            Arc::new(NoOpCommandOptionalExecutor {}),
            Arc::new(NoOpCommandOptionalExecutor {}),
            Arc::new(NoOpCacheUploader {}),
            None,
            fs.dupe(),
            CommandGenerationOptions {
                path_separator: PathSeparatorKind::system_default(),
                output_paths_behavior: Default::default(),
                use_bazel_protocol_remote_persistent_workers: false,
                network_access: None,
            },
            remote_execution::Platform { properties: vec![] },
        );
        let paths = CommandExecutionPaths::new(
            vec![],
            BuckMutInsertionSet::default(),
            &fs,
            digest_config,
            None,
        )?;
        let request = CommandExecutionRequest::new(
            vec![],
            vec!["false".to_owned()],
            paths,
            BuckFixedSortedMap::new(),
        );
        let label = ConfiguredProvidersLabel::new(
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new()),
            Default::default(),
        );
        let stage = TestStage::Testing {
            suite: "foo".to_owned(),
            testcases: vec![],
            variant: None,
            repeat_count: None,
        };

        let mut ctx = dice.ctx();
        let data = buck2_events::dispatch::with_dispatcher_async(
            EventDispatcher::null(),
            BuckTestOrchestrator::execute_request(
                &mut ctx,
                CancellationContext::testing(),
                &label,
                &stage,
                &[],
                &executor,
                request,
                NoopLivelinessObserver::create(),
                false,
                false,
                WaitingData::new(),
            ),
        )
        .await
        .map_err(|_| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "execute_request returned an error or a cancellation"
            )
        })?;
        assert_eq!(ExecutionStatus::Finished { exitcode: 1 }, data.status);
        Ok(())
    }

    #[tokio::test]
    async fn orchestrator_results() -> buck2_error::Result<()> {
        let (orchestrator, channel) = make().await?;

        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());

        let target = ConfiguredProvidersLabel::new(target, Default::default());
        let target = orchestrator.session.register(target);

        let jobs = async {
            orchestrator
                .report_test_result(TestResult {
                    target,
                    status: TestStatus::PASS,
                    msg: None,
                    name: "First - test".to_owned(),
                    duration: Some(Duration::from_micros(1)),
                    details: "1".to_owned(),
                    max_memory_used_bytes: None,
                })
                .await?;

            orchestrator
                .report_test_result(TestResult {
                    target,
                    status: TestStatus::FAIL,
                    msg: None,
                    name: "Second - test".to_owned(),
                    duration: Some(Duration::from_micros(2)),
                    details: "2".to_owned(),
                    max_memory_used_bytes: None,
                })
                .await?;

            orchestrator.end_of_test_results(0).await?;

            buck2_error::Ok(())
        };

        let ((), results) = future::try_join(jobs, channel.try_collect::<Vec<_>>()).await?;

        assert_eq!(
            results,
            vec![
                ExecutorMessage::TestResult(TestResult {
                    target,

                    status: TestStatus::PASS,
                    msg: None,
                    name: "First - test".to_owned(),
                    duration: Some(Duration::from_micros(1)),
                    details: "1".to_owned(),
                    max_memory_used_bytes: None,
                }),
                ExecutorMessage::TestResult(TestResult {
                    target,

                    status: TestStatus::FAIL,
                    msg: None,
                    name: "Second - test".to_owned(),
                    duration: Some(Duration::from_micros(2)),
                    details: "2".to_owned(),
                    max_memory_used_bytes: None,
                }),
                ExecutorMessage::ExitCode(0),
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn orchestrator_attach_info_messages() -> buck2_error::Result<()> {
        let (orchestrator, channel) = make().await?;

        let jobs = async {
            orchestrator.attach_info_message("yolo".to_owned()).await?;

            orchestrator.end_of_test_results(0).await?;

            buck2_error::Ok(())
        };

        let ((), results) = future::try_join(jobs, channel.try_collect::<Vec<_>>()).await?;

        assert_eq!(
            results,
            vec![
                ExecutorMessage::InfoMessage("yolo".to_owned(),),
                ExecutorMessage::ExitCode(0),
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_orchestrator_channel_drop() -> buck2_error::Result<()> {
        let (orchestrator, channel) = make().await?;
        drop(orchestrator);

        let res = channel.try_collect::<Vec<_>>().await;
        assert!(res.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_orchestrator_closes_channel() -> buck2_error::Result<()> {
        let (orchestrator, channel) = make().await?;
        let sender = orchestrator.results_channel.clone();
        orchestrator.end_of_test_results(1).await?;

        assert!(sender.is_closed());
        drop(channel);

        Ok(())
    }

    #[test]
    fn test_create_action_key_suffix_listing() {
        let stage = TestStage::Listing {
            suite: "test_suite".to_owned(),
            cacheable: true,
        };
        assert_eq!(create_action_key_suffix(&stage), "listing");
    }

    #[test]
    fn test_create_action_key_suffix_testing_no_variant() {
        let stage = TestStage::Testing {
            suite: "test_suite".to_owned(),
            testcases: vec!["test1".to_owned(), "test2".to_owned()],
            variant: None,
            repeat_count: None,
        };
        assert_eq!(create_action_key_suffix(&stage), "test1 test2");
    }

    #[test]
    fn test_create_action_key_suffix_testing_with_variant() {
        let stage = TestStage::Testing {
            suite: "test_suite".to_owned(),
            testcases: vec!["test1".to_owned(), "test2".to_owned()],
            variant: Some("variant1".to_owned()),
            repeat_count: None,
        };
        assert_eq!(create_action_key_suffix(&stage), "variant1 test1 test2");
    }

    #[test]
    fn test_create_action_key_suffix_truncation() {
        let long_testcase = "a".repeat(MAX_SUFFIX_LEN + 100);
        let stage = TestStage::Testing {
            suite: "test_suite".to_owned(),
            testcases: vec![long_testcase],
            variant: None,
            repeat_count: None,
        };
        let result = create_action_key_suffix(&stage);
        assert_eq!(result.len(), MAX_SUFFIX_LEN);
        assert!(result.ends_with("(truncated)"));
    }

    #[test]
    fn test_create_action_key_suffix_truncation_multibyte_utf8() {
        // 3-byte char (中 = 0xE4 0xB8 0xAD); byte 1013 lands mid-character.
        let long_testcase = "中".repeat(MAX_SUFFIX_LEN);
        let stage = TestStage::Testing {
            suite: "test_suite".to_owned(),
            testcases: vec![long_testcase],
            variant: None,
            repeat_count: None,
        };
        let result = create_action_key_suffix(&stage);
        assert!(result.len() <= MAX_SUFFIX_LEN);
        assert!(result.ends_with("(truncated)"));
    }
}
