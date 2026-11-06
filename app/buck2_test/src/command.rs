/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::build::AsyncBuildTargetResultBuilder;
use buck2_build_api::build::BuildConfiguredLabelOptions;
use buck2_build_api::build::BuildEvent;
use buck2_build_api::build::BuildEventConsumer;
use buck2_build_api::build::BuildTargetResult;
use buck2_build_api::build::BuildTargetResultBuilder;
use buck2_build_api::build::ConfiguredBuildEventVariant;
use buck2_build_api::build::ProvidersToBuild;
use buck2_build_api::build::build_configured_label;
use buck2_build_api::build::build_report::build_report_opts;
use buck2_build_api::build::build_report::maybe_stream_build_reports;
use buck2_build_api::build::build_report::streaming_build_result_channel;
use buck2_build_api::build::build_report::write_build_report;
use buck2_build_api::build::build_report_metrics::BuildReportMetricsConfig;
use buck2_build_api::build::build_report_metrics::compute_build_report_supplemental_data;
use buck2_build_api::build::graph_properties::GraphPropertiesOptions;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::InternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::OwnedInternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::run_info::RunInfo;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::interpreter::rule_defs::provider::test_provider::TestProvider;
use buck2_build_api::interpreter::rule_defs::provider::test_provider::build_external_runner_spec;
use buck2_build_api::interpreter::rule_defs::provider::test_provider::test_provider_from_collection;
use buck2_build_api::materialize::MaterializationAndUploadContext;
use buck2_cli_proto::HasClientContext;
use buck2_cli_proto::TestRequest;
use buck2_cli_proto::TestResponse;
use buck2_cli_proto::representative_config_flag;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::events::HasEvents;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::liveliness_observer::LivelinessGuard;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_common::liveliness_observer::LivelinessObserverExt;
use buck2_common::liveliness_observer::TimeoutLivelinessObserver;
use buck2_common::pattern::parse_from_cli::parse_patterns_with_modifiers_from_cli_args;
use buck2_common::pattern::resolve::ResolveTargetPatterns;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_common::tenting::HasTentingAclProvider;
use buck2_common::tenting::TentingStatus;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabelWithModifiers;
use buck2_core::pattern::pattern::Modifiers;
use buck2_core::pattern::pattern::ModifiersError;
use buck2_core::pattern::pattern::PackageSpec;
use buck2_core::pattern::pattern::ProvidersLabelWithModifiers;
use buck2_core::pattern::pattern::TargetLabelWithModifiers;
use buck2_core::pattern::pattern_type::ConfiguredProvidersPatternExtra;
use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::tag_result;
use buck2_core::target::label::label::TargetLabel;
use buck2_data::BuildResult;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_events::dispatch::console_message;
use buck2_events::dispatch::instant_event;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutSet;
use buck2_interpreter::extra::InterpreterHostPlatform;
use buck2_interpreter_for_build::interpreter::context::HasInterpreterContext;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_server_ctx::commands::send_build_phase_end;
use buck2_server_ctx::commands::send_target_cfg_event;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use buck2_server_ctx::test_command::TEST_COMMAND;
use buck2_server_ctx::tpx_experiment_util::get_tpx_experiments;
use buck2_test_api::data::TestResult;
use buck2_test_api::data::TestStatus;
use buck2_test_api::protocol::TestExecutor;
use buck2_test_api::protocol::TestOrchestrator;
use dice::DiceTransaction;
use dice::LinearRecomputeDiceComputations;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::IterDupedExt;
use futures::channel::mpsc;
use futures::future;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use itertools::Itertools;
use tokio::sync::mpsc::UnboundedSender;

use crate::downward_api::BuckTestDownwardApi;
use crate::executor_launcher::ExecutorLaunch;
use crate::executor_launcher::ExecutorLauncher;
use crate::executor_launcher::ExecutorOutput;
use crate::executor_launcher::OutOfProcessTestExecutor;
use crate::executor_launcher::TestExecutorClientWrapper;
use crate::local_resource_registry::HasLocalResourceRegistry;
use crate::orchestrator::BuckTestOrchestrator;
use crate::orchestrator::ExecutorMessage;
use crate::session::TestSession;
use crate::session::TestSessionOptions;
use crate::translations::build_configured_target_handle;

struct TestOutcome {
    build_errors: Vec<buck2_data::ErrorReport>,
    test_errors: Vec<buck2_data::ErrorReport>,
    executor_report: ExecutorReport,
    executor_stdout: String,
    executor_stderr: String,
    build_target_result: BuildTargetResult,
}

impl TestOutcome {
    fn exit_code(&self) -> buck2_error::Result<i32> {
        self.executor_report
            .exit_code()
            .internal_error("Test executor did not provide an exit code")
    }
}

/// Exit code for a run in which tests failed;
/// matches `RunVerdict::Fail` in the OSS test runner
const TESTS_FAILED_EXIT_CODE: i32 = 32;

#[derive(Default)]
struct ExecutorReport {
    /// Exit code reported by the external test executor via
    /// `end_of_test_results`; `None` if the executor exited without
    /// reporting. Not the run's verdict — see `exit_code()`.
    executor_exit_code: Option<i32>,
    /// Whether any target ran in-process. Only then can the executor's
    /// exit code miss results collected here.
    used_internal_runner: bool,
    statuses: TestStatuses,
    info_messages: Vec<String>,
}

impl ExecutorReport {
    fn ingest(&mut self, status: &ExecutorMessage, session: &TestSession) {
        match status {
            ExecutorMessage::TestResult(res) => {
                self.statuses.ingest(res, session);
            }
            ExecutorMessage::ExitCode(exit_code) => {
                self.executor_exit_code = Some(*exit_code);
            }
            ExecutorMessage::InfoMessage(message) => {
                self.info_messages.push(message.clone());
            }
        }
    }

    /// Verdict for the whole run. The executor's exit code is authoritative
    /// unless it reports success while internal-runner tests failed.
    fn exit_code(&self) -> Option<i32> {
        match self.executor_exit_code {
            Some(0) if self.used_internal_runner && self.statuses.has_failures() => {
                Some(TESTS_FAILED_EXIT_CODE)
            }
            code => code,
        }
    }
}

const MAX_EXAMPLE_VALUES: u64 = 10;
struct CounterWithExamples {
    count: u64,
    max: u64,
    example_tests: Vec<String>,
}

impl CounterWithExamples {
    fn add(&mut self, val: &str) {
        self.count += 1;
        if self.count <= self.max {
            self.example_tests.push(val.to_owned());
        }
    }

    fn into_cli_proto_counter(self) -> buck2_cli_proto::CounterWithExamples {
        buck2_cli_proto::CounterWithExamples {
            count: self.count,
            max: self.max,
            example_tests: self.example_tests,
        }
    }
}
impl Default for CounterWithExamples {
    fn default() -> Self {
        Self {
            count: 0,
            max: MAX_EXAMPLE_VALUES,
            example_tests: vec![],
        }
    }
}

#[derive(Default)]
struct TestStatuses {
    passed: CounterWithExamples,
    skipped: CounterWithExamples,
    omitted: CounterWithExamples,
    failed: CounterWithExamples,
    timed_out: CounterWithExamples,
    infra_failure: CounterWithExamples,
    fatals: CounterWithExamples,
    listing_success: CounterWithExamples,
    listing_failed: CounterWithExamples,
}
impl TestStatuses {
    fn ingest(&mut self, result: &TestResult, session: &TestSession) {
        let display_name: std::borrow::Cow<'_, str> = match session.get(result.target) {
            Ok(label) => std::borrow::Cow::Owned(format!("{} ({})", result.name, label.cfg())),
            Err(_) => std::borrow::Cow::Borrowed(result.name.as_str()),
        };
        let name = display_name.as_ref();
        match result.status {
            TestStatus::PASS => self.passed.add(name),
            TestStatus::FAIL => self.failed.add(name),
            TestStatus::SKIP => self.skipped.add(name),
            TestStatus::OMITTED => self.omitted.add(name),
            TestStatus::FATAL => self.fatals.add(name),
            TestStatus::TIMEOUT => self.timed_out.add(name),
            TestStatus::INFRA_FAILURE => self.infra_failure.add(name),
            TestStatus::UNKNOWN => {}
            TestStatus::RERUN => {}
            TestStatus::LISTING_SUCCESS => self.listing_success.add(name),
            TestStatus::LISTING_FAILED => self.listing_failed.add(name),
        }
    }

    /// Whether any result with a failing status was reported. SKIP/OMITTED
    /// deliberately don't count: ignored tests and cancellations must not
    /// fail a run.
    fn has_failures(&self) -> bool {
        self.failed.count > 0
            || self.fatals.count > 0
            || self.timed_out.count > 0
            || self.infra_failure.count > 0
            || self.listing_failed.count > 0
    }
}

#[derive(Debug, buck2_error_derive::Error)]
#[buck2(tag = TestDeadlineExpired)]
#[error("This test run exceeded the deadline that was provided")]
struct DeadlineExpired;

#[derive(Debug, Clone)]
pub(crate) enum InternalRunnerConfig {
    All,
    None,
    Frameworks(BuckMutSet<String>),
}

impl InternalRunnerConfig {
    pub(crate) fn parse(value: Option<&str>) -> Self {
        match value {
            None | Some("true") => Self::All,
            Some("false") => Self::None,
            Some(list) => {
                let frameworks: BuckMutSet<String> = list
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();
                if frameworks.is_empty() {
                    Self::None
                } else {
                    Self::Frameworks(frameworks)
                }
            }
        }
    }

    pub(crate) fn should_use(&self, framework_type: &str) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Frameworks(set) => set.contains(framework_type),
        }
    }
}

async fn test_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
    req: TestRequest,
) -> buck2_error::Result<TestResponse> {
    run_server_command(TestServerCommand { req }, ctx, partial_result_dispatcher).await
}

pub(crate) fn init_test_command() {
    TEST_COMMAND.init(|ctx, partial_result_dispatcher, req| {
        Box::pin(test_command(ctx, partial_result_dispatcher, req))
    })
}

struct TestServerCommand {
    req: buck2_cli_proto::TestRequest,
}

#[async_trait]
impl ServerCommandTemplate for TestServerCommand {
    type StartEvent = buck2_data::TestCommandStart;
    type EndEvent = buck2_data::TestCommandEnd;
    type Response = buck2_cli_proto::TestResponse;
    type PartialResult = NoPartialResult;

    fn build_result(&self, response: &Self::Response) -> Option<BuildResult> {
        Some(BuildResult {
            build_completed: response.build_errors.is_empty(),
        })
    }

    fn end_event(&self, _response: &buck2_error::Result<Self::Response>) -> Self::EndEvent {
        buck2_data::TestCommandEnd {
            unresolved_target_patterns: self
                .req
                .target_patterns
                .iter()
                .map(|p| buck2_data::TargetPattern { value: p.clone() })
                .collect(),
        }
    }

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        _partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        test(server_ctx, ctx, &self.req).await
    }
}

async fn test(
    server_ctx: &dyn ServerCommandContextTrait,
    ctx: DiceTransaction,
    request: &TestRequest,
) -> buck2_error::Result<TestResponse> {
    // TODO (torozco): Should the --fail-fast flag work here?

    let cwd = server_ctx.working_dir();
    let cell_resolver = ctx.ctx().get_cell_resolver().await?;
    let working_dir_cell = cell_resolver.find(cwd);

    let test_builds_targets =
        if request.build_default_info.is_none() || request.build_run_info.is_none() {
            ctx.ctx()
                .parse_legacy_config_property::<bool>(
                    cell_resolver.root_cell(),
                    BuckconfigKeyRef {
                        section: "buck2",
                        property: "test_builds_targets",
                    },
                )
                .await?
                .unwrap_or(false)
        } else {
            false
        };
    let build_default_info = request.build_default_info.unwrap_or(test_builds_targets);
    let build_run_info = request.build_run_info.unwrap_or(test_builds_targets);

    let client_ctx = request.client_context()?;
    let global_cfg_options = global_cfg_options_from_client_context(
        request
            .target_cfg
            .as_ref()
            .internal_error("target_cfg must be set")?,
        server_ctx,
        &mut ctx.ctx(),
    )
    .await?;

    // Get the test runner from the config. Note that we use a different key from v1 since the API
    // is completely different, so there is not expectation that the same binary works for both.
    let test_executor_config = ctx
        .ctx()
        .get_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "test",
                property: "v2_test_executor",
            },
        )
        .await?
        .filter(|s| !s.is_empty());

    let (test_executor, test_executor_args) = match test_executor_config {
        Some(config) => {
            let test_executor = post_process_test_executor(config.as_ref())
                .with_buck_error_context(|| format!("Invalid `test.v2_test_executor`: {config}"))?;
            let mut test_executor_args =
                vec!["--buck-trace-id".to_owned(), client_ctx.trace_id.clone()];
            let platform = match ctx
                .ctx()
                .get_interpreter_configuror()
                .await?
                .host_info()
                .platform
            {
                InterpreterHostPlatform::Linux => "linux",
                InterpreterHostPlatform::MacOS => "mac",
                InterpreterHostPlatform::Windows => "windows",
                _ => "",
            };
            test_executor_args.push("--config-entry".to_owned());
            test_executor_args.push(format!("host={platform}"));

            generate_config_entry_args(
                &mut test_executor_args,
                &client_ctx.representative_config_flags,
            );

            (test_executor, test_executor_args)
        }
        None => {
            // If no v2_test_executor config was set, fall back to the internal test runner.
            let test_executor = std::env::current_exe()?;
            let test_executor_args = vec!["internal-test-runner".to_owned()];
            (test_executor, test_executor_args)
        }
    };

    let parsed_patterns_with_modifiers =
        parse_patterns_with_modifiers_from_cli_args(&mut ctx.ctx(), &request.target_patterns, cwd)
            .await?;
    server_ctx.log_target_pattern_with_modifiers(&parsed_patterns_with_modifiers);

    let has_pattern_modifiers = parsed_patterns_with_modifiers
        .iter()
        .any(|p| p.modifiers.as_slice().is_some());
    if !global_cfg_options.cli_modifiers.is_empty() && has_pattern_modifiers {
        return Err(ModifiersError::PatternModifiersWithGlobalModifiers.into());
    }

    let resolved_pattern = ResolveTargetPatterns::resolve_with_modifiers(
        &mut ctx.ctx(),
        &parsed_patterns_with_modifiers,
    )
    .await?;

    let launcher: Box<dyn ExecutorLauncher> = Box::new(OutOfProcessTestExecutor {
        executable: test_executor,
        args: test_executor_args,
        dispatcher: ctx.per_transaction_data().get_dispatcher().dupe(),
    });

    let options = request
        .session_options
        .as_ref()
        .internal_error("Missing `options`")?;

    let session = TestSession::new(TestSessionOptions {
        allow_re: options.allow_re,
        force_use_project_relative_paths: options.force_use_project_relative_paths,
        force_run_from_project_root: options.force_run_from_project_root,
    });

    let build_opts = request
        .build_opts
        .as_ref()
        .internal_error("should have build options")?;
    let build_report_metrics_config =
        BuildReportMetricsConfig::from_buckconfig(&mut ctx.ctx(), cell_resolver.root_cell())
            .await?;
    let graph_properties = build_report_metrics_config.graph_properties;

    let timeout = request
        .timeout
        .as_ref()
        .map(|t| (*t).try_into())
        .transpose()
        .buck_error_context("Invalid `duration`")?;

    let project_root = server_ctx.project_root();
    let tpx_experiments = get_tpx_experiments(ctx.dupe(), project_root).await?;

    // Forward AI agent identity to TPX as run tags (e.g. ai_agent_id=claude_code).
    #[allow(unused_mut)]
    let mut extra_tpx_args: Vec<String> = Vec::new();
    #[cfg(fbcode_build)]
    extra_tpx_args.extend(ai_agent_tpx_args(&client_ctx.agent_context));

    let mut test_executor_args = request.test_executor_args.clone();
    test_executor_args.extend(extra_tpx_args);

    let (streaming_build_result_tx, streaming_build_result_rx) =
        streaming_build_result_channel(build_opts);

    let build_start = Instant::now();
    let test_future = test_targets(
        ctx.dupe(),
        resolved_pattern,
        global_cfg_options,
        test_executor_args,
        Arc::new(TestLabelFiltering::new(
            request.included_labels.clone(),
            request.excluded_labels.clone(),
            request.always_exclude,
            request.build_filtered_targets,
        )),
        &*launcher,
        session,
        cell_resolver.dupe(),
        working_dir_cell,
        build_opts.skip_incompatible_targets,
        MissingTargetBehavior::from_skip(build_opts.skip_missing_targets),
        timeout,
        request.ignore_tests_attribute,
        build_default_info,
        build_run_info,
        tpx_experiments,
        streaming_build_result_tx,
        build_start,
        graph_properties,
    );

    let test_outcome = maybe_stream_build_reports(
        test_future,
        build_opts,
        ctx.dupe(),
        server_ctx.project_root(),
        server_ctx.working_dir(),
        server_ctx.events().trace_id().dupe(),
        graph_properties,
        streaming_build_result_rx,
    )
    .await?;

    send_target_cfg_event(
        server_ctx.events(),
        test_outcome.build_target_result.configured.keys(),
        &request.target_cfg,
    );

    // TODO(bobyf) remap exit code for buck reserved exit code
    let executor_exit_code = test_outcome.exit_code()?;

    let test_statuses = buck2_cli_proto::test_response::TestStatuses {
        passed: Some(
            test_outcome
                .executor_report
                .statuses
                .passed
                .into_cli_proto_counter(),
        ),
        skipped: Some(
            test_outcome
                .executor_report
                .statuses
                .skipped
                .into_cli_proto_counter(),
        ),
        omitted: Some(
            test_outcome
                .executor_report
                .statuses
                .omitted
                .into_cli_proto_counter(),
        ),
        failed: Some(
            test_outcome
                .executor_report
                .statuses
                .failed
                .into_cli_proto_counter(),
        ),
        fatals: Some(
            test_outcome
                .executor_report
                .statuses
                .fatals
                .into_cli_proto_counter(),
        ),
        infra_failure: Some(
            test_outcome
                .executor_report
                .statuses
                .infra_failure
                .into_cli_proto_counter(),
        ),
        listing_success: Some(
            test_outcome
                .executor_report
                .statuses
                .listing_success
                .into_cli_proto_counter(),
        ),
        listing_failed: Some(
            test_outcome
                .executor_report
                .statuses
                .listing_failed
                .into_cli_proto_counter(),
        ),
        timed_out: Some(
            test_outcome
                .executor_report
                .statuses
                .timed_out
                .into_cli_proto_counter(),
        ),
    };

    let supplemental_data = compute_build_report_supplemental_data(
        &mut ctx.ctx(),
        &test_outcome.build_target_result,
        build_report_metrics_config,
    )
    .await?;

    let serialized_build_report = if build_opts.unstable_print_build_report {
        let artifact_fs = ctx.ctx().get_artifact_fs().await?;
        let build_report_opts =
            build_report_opts(&mut ctx.ctx(), &cell_resolver, build_opts, graph_properties).await?;

        write_build_report(
            build_report_opts,
            artifact_fs,
            &cell_resolver,
            server_ctx.project_root(),
            cwd,
            server_ctx.events().trace_id(),
            &test_outcome.build_target_result.configured,
            &test_outcome
                .build_target_result
                .configured_to_pattern_modifiers,
            &test_outcome.build_target_result.other_errors,
            supplemental_data.detailed_metrics,
            supplemental_data.action_graph_sketch_result,
            supplemental_data.artifact_path_sketch_result,
        )?
    } else {
        None
    };

    Ok(TestResponse {
        executor_exit_code,
        build_errors: test_outcome.build_errors,
        test_errors: test_outcome.test_errors,
        test_statuses: Some(test_statuses),
        executor_stdout: test_outcome.executor_stdout,
        executor_stderr: test_outcome.executor_stderr,
        executor_info_messages: test_outcome.executor_report.info_messages,
        serialized_build_report,
        // Rule types are sourced from `TargetRuleTypeName` instant events
        // emitted in `TestDriver::interpret_targets` and accumulated by the
        // invocation recorder.
        target_rule_type_names: Vec::new(),
    })
}

async fn test_targets(
    ctx: DiceTransaction,
    pattern: ResolvedPattern<ConfiguredProvidersPatternExtra>,
    global_cfg_options: GlobalCfgOptions,
    external_runner_args: Vec<String>,
    label_filtering: Arc<TestLabelFiltering>,
    launcher: &dyn ExecutorLauncher,
    session: TestSession,
    cell_resolver: CellResolver,
    working_dir_cell: CellName,
    skip_incompatible_targets: bool,
    missing_target_behavior: MissingTargetBehavior,
    timeout: Option<Duration>,
    ignore_tests_attribute: bool,
    build_default_info: bool,
    build_run_info: bool,
    tpx_experiments: BuckMutSet<String>,
    streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
    build_start: Instant,
    graph_properties: GraphPropertiesOptions,
) -> buck2_error::Result<TestOutcome> {
    let session = Arc::new(session);

    let (mut liveliness_observer, _guard) = LivelinessGuard::create();
    let timeout_observer = timeout.map(|timeout| {
        Arc::new(TimeoutLivelinessObserver::new(timeout)) as Arc<dyn LivelinessObserver>
    });
    if let Some(timeout_observer) = &timeout_observer {
        liveliness_observer = Arc::new(liveliness_observer.and(timeout_observer.dupe())) as _;
    }

    let tpx_args = {
        let mut args = vec![
            "ignored".to_owned(),
            "--buck-test-info".to_owned(),
            "ignored".to_owned(),
        ];
        args.extend(external_runner_args);

        if cfg!(fbcode_build) {
            // Our OSS test runner does not support `--experiment` flags, so only pass these flags internally.
            args.extend(
                tpx_experiments
                    .iter()
                    .flat_map(|experiment| ["--experiment".to_owned(), experiment.to_owned()]),
            );
        }

        args
    };

    let res = launcher
        .launch(tpx_args)
        .await
        .buck_error_context("Failed to launch executor");

    let res = tag_result!(
        "executor_launch_failed",
        res,
        daemon_in_memory_state_is_corrupted: true,
        task: false
    )?;

    let ExecutorLaunch {
        handle: executor_handle,
        client: test_executor,
        make_server,
    } = res;

    let test_executor = Arc::new(test_executor) as Arc<dyn TestExecutor>;
    let test_executor_wrapper = TestExecutorClientWrapper::new(test_executor.dupe());

    let (test_status_sender, test_status_receiver) = mpsc::unbounded();

    let internal_test_timeout = ctx
        .ctx()
        .get_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "test",
                property: "timeout_default_s",
            },
        )
        .await?
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(600));

    let internal_runner_config = InternalRunnerConfig::parse(
        ctx.ctx()
            .get_legacy_config_property(
                cell_resolver.root_cell(),
                BuckconfigKeyRef {
                    section: "test",
                    property: "use_internal_runner",
                },
            )
            .await?
            .as_deref(),
    );

    let internal_test_status_sender = test_status_sender.clone();

    let test_server = tokio::spawn({
        let test_status_sender = test_status_sender.clone();
        let liveliness_observer = liveliness_observer.dupe();
        with_dispatcher_async(
            ctx.per_transaction_data().get_dispatcher().dupe(),
            // NOTE: This is will cancel if the liveliness guard indicates we should.
            async move {
                // Spawn our server to listen to the test runner's requests for execution.

                // Keep wrapper alive for the lifetime of the executor to ensure it stays registered.
                let _test_executor_wrapper = test_executor_wrapper;

                let orchestrator = BuckTestOrchestrator::new(
                    ctx.dupe(),
                    session.dupe(),
                    liveliness_observer.dupe(),
                    test_status_sender,
                    CancellationContext::never_cancelled(), // sending the orchestrator directly to be spawned by make_server, which never calls it.
                    internal_runner_config.clone(),
                )
                .await
                .buck_error_context("Failed to create a BuckTestOrchestrator")?;

                let server_handle = make_server(orchestrator, BuckTestDownwardApi);

                // Lazily created orchestrator for the in-process internal runner path.
                // Only initialized when a target has InternalRunnerTestInfo.
                let internal_orchestrator = tokio::sync::OnceCell::new();

                let mut driver = TestDriver::new(
                    TestDriverState {
                        ctx: &ctx,
                        label_filtering: &label_filtering,
                        global_cfg_options: &global_cfg_options,
                        session: &session,
                        test_executor: &test_executor,
                        internal_orchestrator: &internal_orchestrator,
                        internal_test_status_sender: &internal_test_status_sender,
                        liveliness_observer: &liveliness_observer,
                        cell_resolver: &cell_resolver,
                        working_dir_cell,
                        internal_test_timeout,
                        internal_runner_config: &internal_runner_config,
                        missing_target_behavior,
                        ignore_tests_attribute,
                        build_default_info,
                        build_run_info,
                        build_start,
                        graph_properties,
                    },
                    streaming_build_result_tx,
                );

                driver.push_pattern(
                    pattern.convert_pattern().buck_error_context(
                        "Test with explicit configuration pattern is not supported yet",
                    )?,
                    skip_incompatible_targets,
                );

                {
                    let drive = driver.drive_to_completion();
                    let alive = liveliness_observer.while_alive();
                    futures::pin_mut!(drive);
                    futures::pin_mut!(alive);
                    match futures::future::select(drive, alive).await {
                        futures::future::Either::Left(..) => {}
                        futures::future::Either::Right(..) => {}
                    }
                }

                // Extract data from driver before dropping it so we can
                // release the internal runner's sender clones.
                let error_target_result = driver.errors.build();
                let first_internal_error = driver.first_internal_error.take();
                let mut build_target_result =
                    std::mem::replace(&mut driver.build_target_result, BuildTargetResult::new());
                // A run cancelled while targets were still being built has no build outcome.
                if driver.pending_build_work == 0 {
                    send_build_phase_end(
                        ctx.per_transaction_data().get_dispatcher(),
                        driver.build_phase_end,
                        convert_error(&build_target_result).is_empty()
                            && convert_error(&error_target_result).is_empty(),
                    );
                }
                drop(driver);

                // Drop the internal runner's sender clone so it doesn't keep
                // the results channel open during try_fold. Orchestrator is not
                // dropped yet -- see below.
                drop(internal_test_status_sender);

                // Whether any target ran in-process deciding whether the exit code accounts for every result collected.
                let used_internal_runner = internal_orchestrator.initialized();

                test_executor
                    .end_of_test_requests()
                    .await
                    .buck_error_context("Failed to notify test executor of end-of-tests")?;

                // Wait for the tests to finish running.
                let mut test_statuses = test_status_receiver
                    .try_fold(ExecutorReport::default(), |mut acc, result| {
                        acc.ingest(&result, &session);
                        future::ready(Ok(acc))
                    })
                    .await
                    .buck_error_context("Did not receive all results from executor")?;

                test_statuses.used_internal_runner = used_internal_runner;

                // The results channel is closed now, so the internal
                // orchestrator's Drop poison lands nowhere.
                drop(internal_orchestrator);

                // Shutdown our server. This is technically not *required* since dropping it would shut it
                // down implicitly, but let's do it anyway so we can collect any errors.
                server_handle
                    .shutdown()
                    .await
                    .buck_error_context("Failed to shutdown orchestrator")?;

                let local_resource_registry = ctx.ctx().get_local_resource_registry()?;

                local_resource_registry
                    .release_all_resources()
                    .await
                    .buck_error_context("Failed to release local resources")?;

                if let Some(e) = first_internal_error {
                    return Err(e);
                }
                build_target_result.extend(error_target_result);

                // And finally return our results;
                buck2_error::Ok((build_target_result, test_statuses))
            },
        )
    });

    let executor_output = executor_handle
        .await
        .buck_error_context("Failed to retrieve executor exit code")?;

    if let Some(error) = executor_termination_error(&executor_output) {
        return Err(error);
    }

    // Now that the executor has exited, we notify our results channel. Two things can happen:
    // - The executor has reported end-of-tests. In this case the channel is closed and nothing
    // happened.
    // - The executor had not reported end-of-tests. Assuming we didn't crash on our end (in which
    // case we're about to get this Err out of test_statuses), then this will ensure we don't wait
    // forever on the executor to notify us!
    let _ignored = test_status_sender.unbounded_send(Err(buck2_error::buck2_error!(
        ErrorTag::TestExecutorNoEndOfTests,
        "Executor exited without reporting end-of-tests",
    )));

    // TODO(bobyf, torozco) we can use cancellation handle here instead of liveliness observer
    let (build_target_result, executor_report) = test_server
        .await
        .buck_error_context("Failed to collect executor report")??;

    let build_errors = convert_error(&build_target_result)
        .iter()
        .map(buck2_data::ErrorReport::from)
        .unique_by(|e| e.message.clone())
        .collect::<Vec<_>>();

    let mut test_errors = Vec::new();
    if let Some(timeout_observer) = timeout_observer {
        if !timeout_observer.is_alive().await {
            test_errors.push(buck2_data::ErrorReport::from(&DeadlineExpired.into()));
        }
    }

    Ok(TestOutcome {
        build_errors,
        test_errors,
        executor_stdout: executor_output.stdout,
        executor_stderr: executor_output.stderr,
        executor_report,
        build_target_result,
    })
}

enum TestDriverTask {
    InterpretTarget {
        package_with_modifiers: PackageLabelWithModifiers,
        spec: PackageSpec<ProvidersPatternExtra>,
        skip_incompatible_targets: bool,
    },
    ConfigureTarget {
        label_with_modifiers: ProvidersLabelWithModifiers,
        skippable: bool,
        test_config_unification_rollout: bool,
        tenting_acl_names: TentingStatus,
    },
    BuildTarget {
        label: ConfiguredProvidersLabel,
        modifiers: Modifiers,
        test_config_unification_rollout: bool,
        oncall: Option<String>,
        tenting_acl_names: TentingStatus,
    },
    TestTarget {
        label: ConfiguredProvidersLabel,
        modifiers: Modifiers,
        providers: FrozenProviderCollectionValue,
        build_target_result: BuildTargetResult,
        test_config_unification_rollout: bool,
        oncall: Option<String>,
        tenting_acl_names: TentingStatus,
    },
}

#[derive(Copy, Clone, Dupe)]
struct TestDriverState<'a, 'e> {
    ctx: &'a DiceTransaction,
    label_filtering: &'a Arc<TestLabelFiltering>,
    global_cfg_options: &'a GlobalCfgOptions,
    session: &'a Arc<TestSession>,
    test_executor: &'a Arc<dyn TestExecutor + 'e>,
    internal_orchestrator: &'a tokio::sync::OnceCell<Arc<dyn TestOrchestrator + Send + Sync>>,
    internal_test_status_sender: &'a mpsc::UnboundedSender<buck2_error::Result<ExecutorMessage>>,
    liveliness_observer: &'a Arc<dyn LivelinessObserver>,
    cell_resolver: &'a CellResolver,
    working_dir_cell: CellName,
    internal_test_timeout: Duration,
    internal_runner_config: &'a InternalRunnerConfig,
    missing_target_behavior: MissingTargetBehavior,
    ignore_tests_attribute: bool,
    build_default_info: bool,
    build_run_info: bool,
    build_start: Instant,
    graph_properties: GraphPropertiesOptions,
}

type DriverWorkResult = ControlFlow<Vec<BuildEvent>, Vec<TestDriverTask>>;

#[derive(Clone, Copy, Dupe, PartialEq, Eq)]
enum DriverWorkKind {
    Build,
    Test,
}

/// Maintains the state of an ongoing test execution.
struct TestDriver<'a, 'e> {
    state: TestDriverState<'a, 'e>,
    work: FuturesUnordered<BoxFuture<'a, (DriverWorkKind, DriverWorkResult)>>,
    /// When the last build work item (interpreting, configuring or building a target)
    /// finished. Test work items don't move it.
    build_phase_end: SystemTime,
    pending_build_work: usize,
    labels_configured: BuckMutSet<(ProvidersLabelWithModifiers, bool)>,
    labels_tested: BuckMutSet<ConfiguredProvidersLabel>,
    /// Fed the events of every `ControlFlow::Break`.
    errors: BuildTargetResultBuilder,
    /// The first error `errors` returned, which fails the run once it has been cleaned up.
    first_internal_error: Option<buck2_error::Error>,
    build_target_result: BuildTargetResult,
    streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
}

impl<'a, 'e> TestDriver<'a, 'e> {
    fn new(
        state: TestDriverState<'a, 'e>,
        streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
    ) -> Self {
        Self {
            state,
            work: FuturesUnordered::new(),
            build_phase_end: SystemTime::now(),
            pending_build_work: 0,
            labels_configured: BuckMutSet::default(),
            labels_tested: BuckMutSet::default(),
            errors: BuildTargetResultBuilder::new(
                streaming_build_result_tx.clone(),
                state.build_start,
            ),
            first_internal_error: None,
            build_target_result: BuildTargetResult::new(),
            streaming_build_result_tx,
        }
    }

    fn push_work(&mut self, kind: DriverWorkKind, fut: BoxFuture<'a, DriverWorkResult>) {
        if kind == DriverWorkKind::Build {
            self.pending_build_work += 1;
        }
        self.work.push(fut.map(move |res| (kind, res)).boxed());
    }

    /// Add new patterns for the test driver to process.
    fn push_pattern(
        &mut self,
        pattern: ResolvedPattern<ProvidersPatternExtra>,
        skip_incompatible_targets: bool,
    ) {
        for (package_with_modifiers, spec) in pattern.specs.into_iter() {
            let fut = future::ready(ControlFlow::Continue(vec![
                TestDriverTask::InterpretTarget {
                    package_with_modifiers,
                    spec,
                    skip_incompatible_targets,
                },
            ]))
            .boxed();

            self.push_work(DriverWorkKind::Build, fut);
        }
    }

    /// Drive the test loop until all work is complete.
    async fn drive_to_completion(&mut self) {
        while let Some((kind, tasks)) = self.work.next().await {
            if kind == DriverWorkKind::Build {
                self.pending_build_work -= 1;
                self.build_phase_end = SystemTime::now();
            }
            match tasks {
                ControlFlow::Continue(tasks) => {
                    for task in tasks {
                        match task {
                            TestDriverTask::InterpretTarget {
                                package_with_modifiers,
                                spec,
                                skip_incompatible_targets,
                            } => {
                                self.interpret_targets(
                                    package_with_modifiers,
                                    spec,
                                    skip_incompatible_targets,
                                );
                            }
                            TestDriverTask::ConfigureTarget {
                                label_with_modifiers,
                                skippable,
                                test_config_unification_rollout,
                                tenting_acl_names,
                            } => {
                                self.configure_target(
                                    label_with_modifiers,
                                    skippable,
                                    test_config_unification_rollout,
                                    tenting_acl_names,
                                );
                            }
                            TestDriverTask::BuildTarget {
                                label,
                                modifiers,
                                test_config_unification_rollout,
                                oncall,
                                tenting_acl_names,
                            } => {
                                self.build_target(
                                    label,
                                    modifiers,
                                    test_config_unification_rollout,
                                    oncall,
                                    tenting_acl_names,
                                );
                            }
                            TestDriverTask::TestTarget {
                                label,
                                modifiers,
                                providers,
                                build_target_result,
                                test_config_unification_rollout,
                                oncall,
                                tenting_acl_names,
                            } => {
                                self.test_target(
                                    label,
                                    modifiers,
                                    providers,
                                    build_target_result,
                                    test_config_unification_rollout,
                                    oncall,
                                    tenting_acl_names,
                                );
                            }
                        }
                    }
                }
                ControlFlow::Break(events) => {
                    for event in events {
                        if let Err(e) = self.errors.event(event) {
                            self.first_internal_error.get_or_insert(e);
                        }
                    }
                }
            }
        }
    }

    fn interpret_targets(
        &mut self,
        package_with_modifiers: PackageLabelWithModifiers,
        spec: PackageSpec<ProvidersPatternExtra>,
        skip_incompatible_targets: bool,
    ) {
        let state = self.state;

        let PackageLabelWithModifiers { package, modifiers } = package_with_modifiers;

        self.push_work(
            DriverWorkKind::Build,
            async move {
                let res = match state
                    .ctx
                    .ctx()
                    .get_interpreter_results(package.dupe())
                    .await
                {
                    Ok(res) => res,
                    Err(e) => {
                        let e: buck2_error::Error = e;
                        let mut events = Vec::new();
                        // Try to associate the error to concrete targets, if possible
                        match spec {
                            PackageSpec::Targets(targets) => {
                                for (target, providers) in targets {
                                    let label = Some(ProvidersLabel::new(
                                        TargetLabel::new(package.dupe(), target.as_ref()),
                                        providers.providers,
                                    ));

                                    events.push(BuildEvent::OtherError {
                                        label,
                                        err: e.dupe(),
                                    });
                                }
                            }
                            PackageSpec::All() => events.push(BuildEvent::OtherError {
                                label: None,
                                err: e,
                            }),
                        };

                        return ControlFlow::Break(finish_error_only_targets(events));
                    }
                };

                // Indicates whether this should be skipped if incompatible.
                let skippable = match spec {
                    PackageSpec::Targets(..) => skip_incompatible_targets,
                    PackageSpec::All() => true,
                };

                let (targets, missing) = res.apply_spec(spec);

                if let Some(missing) = missing {
                    match state.missing_target_behavior {
                        MissingTargetBehavior::Fail => {
                            let err = missing.into_first_error();
                            let events = vec![BuildEvent::OtherError {
                                label: Some(ProvidersLabel::new(
                                    TargetLabel::new(err.package.dupe(), err.target.as_ref()),
                                    buck2_core::provider::label::ProvidersName::Default,
                                )),
                                err: err.into(),
                            }];

                            return ControlFlow::Break(finish_error_only_targets(events));
                        }
                        MissingTargetBehavior::Warn => {
                            console_message(missing.missing_targets_warning());
                        }
                    }
                }

                let tenting_acl_names = if let Ok(project_path) =
                    state.cell_resolver.resolve_path(package.as_cell_path())
                {
                    if let Ok(rel_path) = ProjectRelativePathBuf::try_from(project_path.to_string())
                    {
                        match state.ctx.global_data().get_tenting_acl_provider() {
                            Some(provider) => provider
                                .get_tenting_acl_names(&rel_path)
                                .await
                                .unwrap_or(TentingStatus::Unknown),
                            // No provider configured: we cannot determine tenting.
                            None => TentingStatus::Unknown,
                        }
                    } else {
                        TentingStatus::Unknown
                    }
                } else {
                    TentingStatus::Unknown
                };

                let labels =
                    targets
                        .into_iter()
                        .map(|((target_name, providers_pattern), target_node)| {
                            // Emit one rule-type event per CLI-resolved top-level target so the
                            // invocation recorder can populate
                            // `InvocationRecord.target_rule_type_names`. Covers every rule kind,
                            // including non-test rules like `genrule`/`cxx_library` siblings that
                            // tests get bundled with via macros.
                            instant_event(buck2_data::TargetRuleTypeName {
                                rule_type: target_node.rule_type().name().to_owned(),
                            });
                            (
                                providers_pattern.into_providers_label_with_modifiers(
                                    package.dupe(),
                                    target_name.as_ref(),
                                    modifiers.dupe(),
                                ),
                                target_node.test_config_unification_rollout(),
                            )
                        });
                let work = labels
                    .into_iter()
                    .map(|(label_with_modifiers, test_config_unification_rollout)| {
                        TestDriverTask::ConfigureTarget {
                            label_with_modifiers,
                            skippable,
                            test_config_unification_rollout,
                            tenting_acl_names: tenting_acl_names.dupe(),
                        }
                    })
                    .collect();

                ControlFlow::Continue(work)
            }
            .boxed(),
        );
    }

    fn configure_target(
        &mut self,
        label_with_modifiers: ProvidersLabelWithModifiers,
        skippable: bool,
        test_config_unification_rollout: bool,
        tenting_acl_names: TentingStatus,
    ) {
        if !self
            .labels_configured
            .insert((label_with_modifiers.dupe(), skippable))
        {
            return;
        }

        let ProvidersLabelWithModifiers {
            providers_label,
            modifiers,
        } = label_with_modifiers;

        let state = self.state;

        let local_cfg_options = match modifiers.as_slice() {
            Some(modifiers) => GlobalCfgOptions {
                target_platform: state.global_cfg_options.target_platform.dupe(),
                cli_modifiers: modifiers.to_vec().into(),
            },
            None => state.global_cfg_options.dupe(),
        };

        let fut = async move {
            let label = match state
                .ctx
                .ctx()
                .get_configured_provider_label(&providers_label, &local_cfg_options)
                .await
            {
                Ok(label) => label,
                Err(e) => {
                    return ControlFlow::Break(finish_error_only_targets(vec![
                        BuildEvent::OtherError {
                            label: Some(providers_label),
                            err: e,
                        },
                    ]));
                }
            };

            let node = match state
                .ctx
                .ctx()
                .get_configured_target_node(label.target())
                .await
            {
                ResultMaybeCompatible::Compatible(node) => node.dupe(),
                ResultMaybeCompatible::Incompatible(reason) => {
                    if skippable {
                        //TODO: add aggregated error message
                        tracing::debug!("{}", reason.skipping_message(label.target()));
                        return ControlFlow::Continue(vec![]);
                    } else {
                        return ControlFlow::Break(create_and_map_configured_build_error(
                            label,
                            reason.to_err(),
                            modifiers,
                        ));
                    }
                }
                ResultMaybeCompatible::Err(e) => {
                    return ControlFlow::Break(create_and_map_configured_build_error(
                        label, e, modifiers,
                    ));
                }
            };

            let oncall = node.oncall().map(|s| s.to_owned());

            // Build and then test this: it's compatible.
            let mut work = vec![TestDriverTask::BuildTarget {
                label,
                modifiers: modifiers.dupe(),
                test_config_unification_rollout,
                oncall,
                tenting_acl_names: tenting_acl_names.dupe(),
            }];

            // If this node is a forward, it'll get flattened when we do analysis and run the
            // test later, but its `tests` attribute here will not be, and that means we'll
            // just ignore it (since we don't traverse `tests` recursively). So ... just
            // flatten it?
            let node = node.forward_target().unwrap_or(&node);

            // Look up `tests` in the target we're testing, and if we find any tests, add them to the test backlog.
            if !state.ignore_tests_attribute {
                for test in node.tests() {
                    work.push(TestDriverTask::ConfigureTarget {
                        label_with_modifiers: ProvidersLabelWithModifiers {
                            providers_label: test.unconfigured(),
                            modifiers: modifiers.dupe(),
                        },
                        // Historically `skippable: false` is what we enforced here, perhaps that
                        // should change.
                        skippable: false,
                        test_config_unification_rollout,
                        // Tenting is resolved per top-level target, not for targets
                        // pulled in via the `tests` attribute; treat as undetermined.
                        tenting_acl_names: TentingStatus::Unknown,
                    });
                }
            }

            ControlFlow::Continue(work)
        }
        .boxed();

        self.push_work(DriverWorkKind::Build, fut);
    }

    fn build_target(
        &mut self,
        label: ConfiguredProvidersLabel,
        modifiers: Modifiers,
        test_config_unification_rollout: bool,
        oncall: Option<String>,
        tenting_acl_names: TentingStatus,
    ) {
        if !self.labels_tested.insert(label.dupe()) {
            self.push_work(
                DriverWorkKind::Build,
                async move {
                    ControlFlow::Break(vec![BuildEvent::new_configured(
                        label,
                        ConfiguredBuildEventVariant::MapModifiers { modifiers },
                    )])
                }
                .boxed(),
            );

            return;
        }

        let state = self.state;
        let build_label = label.dupe();
        let streaming_build_result_tx = self.streaming_build_result_tx.clone();
        let fut = async move {
            let ctx = state.ctx.clone();

            let modifiers_dupe = modifiers.dupe();

            let result = match ctx
                .ctx()
                .with_linear_recompute(|ctx| {
                    async move {
                        build_target_result(
                            ctx,
                            state.label_filtering,
                            build_label,
                            modifiers_dupe,
                            state.build_default_info,
                            state.build_run_info,
                            streaming_build_result_tx,
                            state.build_start,
                            state.graph_properties,
                        )
                        .await
                    }
                    .boxed()
                })
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    return ControlFlow::Break(create_and_map_configured_build_error(
                        label, e, modifiers,
                    ));
                }
            };

            ControlFlow::Continue(vec![TestDriverTask::TestTarget {
                label,
                build_target_result: result.0,
                providers: result.1,
                modifiers,
                test_config_unification_rollout,
                oncall,
                tenting_acl_names,
            }])
        }
        .boxed();

        self.push_work(DriverWorkKind::Build, fut);
    }

    fn test_target(
        &mut self,
        label: ConfiguredProvidersLabel,
        modifiers: Modifiers,
        providers: FrozenProviderCollectionValue,
        build_target_result: BuildTargetResult,
        test_config_unification_rollout: bool,
        oncall: Option<String>,
        tenting_acl_names: TentingStatus,
    ) {
        let should_test = !build_target_result.build_failed && !build_target_result.is_empty();
        self.build_target_result.extend(build_target_result);

        // Build has failed, no need to continue with test.
        if !should_test {
            return;
        }

        let state = self.state;
        let fut = async move {
            if let Err(e) = test_target(
                label.dupe(),
                providers,
                state.test_executor.dupe(),
                state,
                state.label_filtering.dupe(),
                state.cell_resolver,
                state.working_dir_cell,
                state.internal_test_timeout,
                test_config_unification_rollout,
                oncall,
                tenting_acl_names,
            )
            .await
            {
                return ControlFlow::Break(create_and_map_configured_build_error(
                    label, e, modifiers,
                ));
            }

            ControlFlow::Continue(vec![])
        }
        .boxed();

        self.push_work(DriverWorkKind::Test, fut);
    }
}

async fn build_target_result(
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    label_filtering: &TestLabelFiltering,
    label: ConfiguredProvidersLabel,
    modifiers: Modifiers,
    build_default_info: bool,
    build_run_info: bool,
    streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
    build_start: Instant,
    graph_properties: GraphPropertiesOptions,
) -> buck2_error::Result<(BuildTargetResult, FrozenProviderCollectionValue)> {
    // NOTE: We fail if we hit an incompatible target here. This can happen if we reach an
    // incompatible target via `tests = [...]`. This should perhaps change, but that's how it works
    // in v1: https://fb.workplace.com/groups/buckeng/posts/8520953297953210
    let providers = ctx
        .get()
        .get_providers(&label)
        .await?
        .require_compatible()?;
    let collections = providers.provider_collection();

    // We build target if any of the following is true
    // 1. It's a test (aka it produces a TestInfo provider) and it is not skipped by label filtering
    // 2. --build-default-info is requested
    // 3. --build-run-info is requested and the target produces a RunInfo
    if let Some(test_info) = test_provider_from_collection(collections)? {
        let skip_build_based_on_labels = !label_filtering.build_filtered_targets
            && label_filtering.is_excluded(test_info.labels()?);
        if skip_build_based_on_labels {
            return Ok((BuildTargetResult::new(), providers));
        }
    } else if !(build_default_info
        || build_run_info
            && providers
                .provider_collection()
                .builtin_provider::<RunInfo>()?
                .is_some())
    {
        return Ok((BuildTargetResult::new(), providers));
    }

    let materialization_and_upload = MaterializationAndUploadContext::skip();
    let (result_builder, consumer) =
        AsyncBuildTargetResultBuilder::new(streaming_build_result_tx, build_start);
    let configured_target = label.target().dupe();
    let target = TargetLabelWithModifiers {
        target_label: configured_target.unconfigured().dupe(),
        modifiers: modifiers.dupe(),
    };
    consumer.consume(BuildEvent::new_configured(
        label.dupe(),
        ConfiguredBuildEventVariant::MapModifiers { modifiers },
    ));
    let build_target_result = result_builder
        .wait_for(false, async {
            build_configured_label(
                &consumer,
                ctx,
                materialization_and_upload,
                label,
                &ProvidersToBuild {
                    default: build_default_info,
                    default_other: build_default_info,
                    run: build_run_info,
                    tests: true,
                },
                BuildConfiguredLabelOptions {
                    skippable: false,
                    graph_properties,
                    return_run_args: false,
                },
                None, // TODO: is this right?
            )
            .await;
            // Each builder holds one provider label, so a target tested under several provider
            // labels is reported finished once per label.
            consumer.consume(BuildEvent::ConfiguredTargetBuildFinished {
                target: configured_target,
                modifiers: target.modifiers.dupe(),
            });
            consumer.consume(BuildEvent::TargetBuildFinished { target });
        })
        .await?;

    Ok((build_target_result, providers))
}

async fn test_target<'a, 'e>(
    target: ConfiguredProvidersLabel,
    providers: FrozenProviderCollectionValue,
    test_executor: Arc<dyn TestExecutor + 'e>,
    driver_state: TestDriverState<'a, 'e>,
    label_filtering: Arc<TestLabelFiltering>,
    cell_resolver: &CellResolver,
    working_dir_cell: CellName,
    internal_test_timeout: Duration,
    test_config_unification_rollout: bool,
    oncall: Option<String>,
    tenting_acl_names: TentingStatus,
) -> buck2_error::Result<Option<ConfiguredProvidersLabel>> {
    let collection = providers.provider_collection();

    // Check for InternalRunnerTestInfo first — run in-process.
    // Gated by [test].use_internal_runner (default true, comma-separated framework types,
    // or false to force TPX fallback).
    let internal_provider: Option<OwnedInternalRunnerTestInfo> =
        providers.builtin_provider_value::<InternalRunnerTestInfo>()?;
    if let Some(internal_provider) = internal_provider {
        // `'v`-branded views of the provider must not be held across awaits (only the
        // `OwnedFrozen` may be), so views are derived in scopes that end before the next await.
        let use_internal_runner = {
            let provider = internal_provider.as_ref().value().as_ref();
            driver_state
                .internal_runner_config
                .should_use(provider.test_type())
        };
        if use_internal_runner {
            let excluded = {
                let provider = internal_provider.as_ref().value().as_ref();
                label_filtering
                    .is_excluded(provider.labels().collect::<buck2_error::Result<Vec<_>>>()?)
            };
            if excluded {
                return Ok(None);
            }

            let handle = build_configured_target_handle(
                target.dupe(),
                driver_state.session,
                cell_resolver,
                test_config_unification_rollout,
                oncall,
            )?;

            let orchestrator = driver_state
                .internal_orchestrator
                .get_or_try_init(|| async {
                    let orchestrator = BuckTestOrchestrator::new(
                        driver_state.ctx.dupe(),
                        driver_state.session.dupe(),
                        driver_state.liveliness_observer.dupe(),
                        driver_state.internal_test_status_sender.clone(),
                        CancellationContext::never_cancelled(),
                        driver_state.internal_runner_config.clone(),
                    )
                    .await
                    .buck_error_context("Failed to create internal BuckTestOrchestrator")?;
                    Ok::<_, buck2_error::Error>(
                        Arc::new(orchestrator) as Arc<dyn TestOrchestrator + Send + Sync>
                    )
                })
                .await?;

            let (spec, listing_spec) = {
                let provider = internal_provider.as_ref().value().as_ref();
                let listing_spec = build_external_runner_spec(
                    provider.listing_command(),
                    provider.env().map(|entry| Ok(entry?.0)),
                    provider.test_type(),
                    provider.labels(),
                    provider.contacts(),
                    handle.clone(),
                    working_dir_cell,
                    &tenting_acl_names,
                )?;
                let spec = build_external_runner_spec(
                    provider.command(),
                    provider.env().map(|entry| Ok(entry?.0)),
                    provider.test_type(),
                    provider.labels(),
                    provider.contacts(),
                    handle,
                    working_dir_cell,
                    &tenting_acl_names,
                )?;
                (spec, listing_spec)
            };
            crate::internal_runner::run_internal_test(
                orchestrator.as_ref(),
                spec,
                listing_spec,
                &internal_provider,
                internal_test_timeout,
            )
            .await?;
            return Ok(Some(target));
        }
    }

    let fut = match test_provider_from_collection(collection)? {
        Some(test_info) => {
            if label_filtering.is_excluded(test_info.labels()?) {
                return Ok(None);
            }
            run_tests(
                test_executor,
                target,
                test_info,
                driver_state.session,
                cell_resolver,
                working_dir_cell,
                test_config_unification_rollout,
                oncall,
                tenting_acl_names,
            )
            .map(|l| Some(l).transpose())
            .left_future()
        }
        None => {
            // not a test
            future::ready(Ok(None)).right_future()
        }
    };

    // NOTE: We produce the future above, then await it here because Module & TestInfo aren't Sync.
    // That's OK, because we don't need them across an await point, but we do need to structure the
    // code this way for the compiler to see it.
    fut.await
}

fn convert_error(build_result: &BuildTargetResult) -> Vec<buck2_error::Error> {
    let mut errors = Vec::new();
    errors.extend(build_result.other_errors.values().flatten().duped());

    for v in build_result.configured.values().flatten() {
        errors.extend(v.errors.iter().map(|t| t.inner.dupe()));
        errors.extend(
            v.outputs
                .iter()
                .filter_map(|x| x.inner.as_ref().err())
                .duped(),
        );
    }

    errors
}

fn run_tests<'a, 'b>(
    test_executor: Arc<dyn TestExecutor + 'a>,
    providers_label: ConfiguredProvidersLabel,
    test_info: &'b dyn TestProvider<'b>,
    session: &'b TestSession,
    cell_resolver: &'b CellResolver,
    working_dir_cell: CellName,
    test_config_unification_rollout: bool,
    oncall: Option<String>,
    tenting_acl_names: TentingStatus,
) -> BoxFuture<'a, buck2_error::Result<ConfiguredProvidersLabel>> {
    let maybe_handle = build_configured_target_handle(
        providers_label.dupe(),
        session,
        cell_resolver,
        test_config_unification_rollout,
        oncall,
    );

    match maybe_handle {
        Ok(handle) => {
            let fut =
                test_info.dispatch(handle, test_executor, working_dir_cell, tenting_acl_names);

            (async move {
                fut.await
                    .buck_error_context("Failed to notify test executor of a new test")?;
                Ok(providers_label)
            })
            .boxed()
        }
        Err(err) => future::ready(Err(err)).boxed(),
    }
}

// TODO(azhang2542): Ideally we would only have to map the `Modifier`` to a `ConfiguredProvidersLabel`
// just once when we first get the `ConfiguredProvidersLabel`. Refactor the code so that we have to do
// a call to mapping minimally, potentially by adding the mappings to the `TestDriverState`.
/// Also finishes the entry that `err` is filed under, since nothing more is built for the label.
fn create_and_map_configured_build_error(
    label: ConfiguredProvidersLabel,
    err: buck2_error::Error,
    modifiers: Modifiers,
) -> Vec<BuildEvent> {
    let target = TargetLabelWithModifiers {
        target_label: label.target().unconfigured().dupe(),
        modifiers: modifiers.dupe(),
    };
    vec![
        BuildEvent::new_configured(label.dupe(), ConfiguredBuildEventVariant::Error { err }),
        BuildEvent::new_configured(
            label,
            ConfiguredBuildEventVariant::MapModifiers { modifiers },
        ),
        BuildEvent::TargetBuildFinished { target },
    ]
}

/// Appends `TargetBuildFinished` for the targets of the `OtherError`s in `events`. These targets
/// failed before configuring, and the report files their errors under no modifiers.
fn finish_error_only_targets(mut events: Vec<BuildEvent>) -> Vec<BuildEvent> {
    let finished: Vec<BuildEvent> = events
        .iter()
        .filter_map(|event| match event {
            BuildEvent::OtherError {
                label: Some(label), ..
            } => Some(label.target().dupe()),
            _ => None,
        })
        .unique()
        .map(|target_label| BuildEvent::TargetBuildFinished {
            target: TargetLabelWithModifiers {
                target_label,
                modifiers: Modifiers::new(None),
            },
        })
        .collect();
    events.extend(finished);
    events
}

struct TestLabelFiltering {
    /// These have the highest order of precedence. Order of precedence within the label is the
    /// iteration order.
    /// A single string label will mean match that label. A `!` prefix turns that label into an
    /// exclusion match.
    /// If positive include label filters are present, then this filter will ONLY match sets of
    /// labels that contains the label filter. Otherwise, if only exclusion filters are present, or
    /// no label filters are present, this will match any set of labels as long as its not excluded.
    included_labels: BuckMutInsertionSet<String>,
    /// Additional excluded labels. These have order of precedence after `included_labels`.
    excluded_labels: BuckMutInsertionSet<String>,
    /// If true, ignores order of precedence such that as long as an exclusion filter matches, we
    /// don't match the set of labels.
    always_exclude: bool,
    /// Whether to build targets that are filtered out, but don't run it.
    build_filtered_targets: bool,
}

impl TestLabelFiltering {
    fn is_excluded(&self, labels: Vec<&str>) -> bool {
        let mut matched = !self
            .included_labels
            .iter()
            .any(|label| !label.starts_with('!'));
        for include_label in &self.included_labels {
            if let Some(include) = include_label.strip_prefix('!') {
                // exclusion filters
                if labels.contains(&include) {
                    return true;
                }
            } else {
                // inclusion filters
                if labels.contains(&include_label.as_str()) {
                    if !self.always_exclude {
                        return false;
                    } else {
                        matched = true;
                    }
                }
            }
        }
        for exclude_label in &self.excluded_labels {
            if labels.contains(&exclude_label.as_str()) {
                return true;
            }
        }

        !matched
    }

    fn new(
        included_labels: Vec<String>,
        excluded_labels: Vec<String>,
        always_exclude: bool,
        build_filtered_targets: bool,
    ) -> Self {
        Self {
            included_labels: included_labels.into_iter().collect(),
            excluded_labels: excluded_labels.into_iter().collect(),
            always_exclude,
            build_filtered_targets,
        }
    }
}

fn generate_config_entry_args(
    test_executor_args: &mut Vec<String>,
    representative_config_flags: &[buck2_cli_proto::RepresentativeConfigFlag],
) {
    let mut config_flags = String::new();
    let mut config_files = String::new();
    let mut flagfiles = String::new();
    let mut modifiers = String::new();
    let mut target_platforms = String::new();

    for s in representative_config_flags {
        if let Some(source) = &s.source {
            match source {
                representative_config_flag::Source::ConfigFlag(s) => {
                    if config_flags.is_empty() {
                        config_flags.push_str(s);
                    } else {
                        config_flags.push(';');
                        config_flags.push_str(s);
                    }
                }
                representative_config_flag::Source::ConfigFile(s) => {
                    if config_files.is_empty() {
                        config_files.push_str(s);
                    } else {
                        config_files.push(';');
                        config_files.push_str(s);
                    }
                }
                representative_config_flag::Source::ModeFile(s) => {
                    if flagfiles.is_empty() {
                        flagfiles.push_str(s);
                    } else {
                        flagfiles.push(';');
                        flagfiles.push_str(s);
                    }
                }
                representative_config_flag::Source::Modifier(s) => {
                    if modifiers.is_empty() {
                        modifiers.push_str(s);
                    } else {
                        modifiers.push(';');
                        modifiers.push_str(s);
                    }
                }
                representative_config_flag::Source::TargetPlatforms(s) => {
                    if target_platforms.is_empty() {
                        target_platforms.push_str(s);
                    } else {
                        target_platforms.push(';');
                        target_platforms.push_str(s);
                    }
                }
                representative_config_flag::Source::TargetUniverse(_) => {
                    // Target universe is not passed to TPX
                }
            }
        }
    }

    if !config_flags.is_empty() {
        test_executor_args.push("--config-entry".to_owned());
        test_executor_args.push(format!("config={config_flags}"));
    }
    if !config_files.is_empty() {
        test_executor_args.push("--config-entry".to_owned());
        test_executor_args.push(format!("config_file={config_files}"));
    }
    if !flagfiles.is_empty() {
        test_executor_args.push("--config-entry".to_owned());
        test_executor_args.push(format!("mode={flagfiles}"));
    }
    if !modifiers.is_empty() {
        test_executor_args.push("--config-entry".to_owned());
        test_executor_args.push(format!("modifier={modifiers}"));
    }
    if !target_platforms.is_empty() {
        test_executor_args.push("--config-entry".to_owned());
        test_executor_args.push(format!("target_platforms={target_platforms}"));
    }
}

fn post_process_test_executor(s: &str) -> buck2_error::Result<PathBuf> {
    match s.split_once("$BUCK2_BINARY_DIR/") {
        Some(("", rest)) => {
            let exe = AbsPathBuf::new(
                std::env::current_exe().buck_error_context("Cannot get Buck2 executable")?,
            )?;
            // On Linux, /proc/self/exe appends " (deleted)" to the path when the
            // binary has been removed from disk (e.g. after a buck2 upgrade).
            if exe.as_path().to_string_lossy().ends_with(" (deleted)") {
                return Err(buck2_error::buck2_error!(
                    ErrorTag::BuckdExeDeleted,
                    "The buck2 daemon's binary has been deleted from disk. \
                     Run `buck2 kill` to restart the daemon with the current binary."
                ));
            }
            let exe = fs_util::canonicalize(&exe)
                .categorize_tagged(ErrorTag::BuckdExeDeleted)
                .buck_error_context(
                    "Failed to canonicalize path to Buck2 executable. Try running `buck2 kill`.",
                )?;

            let exe = exe.as_abs_path();
            let exe_dir = exe
                .parent()
                .internal_error("Buck2 executable directory has no parent")?;

            Ok(exe_dir.join(rest).to_path_buf())
        }
        Some(..) => Err(buck2_error::buck2_error!(
            ErrorTag::Environment,
            "Invalid value: {}",
            s
        )),
        None => Ok(s.into()),
    }
}

/// Generates `--tags` TPX args from agent context entries.
/// Tags exceeding 80 chars are silently dropped to avoid TPX failures
/// (testinfra MAXIMUM_TAG_LENGTH = 80).
#[cfg(any(fbcode_build, test))]
fn ai_agent_tpx_args(agent_context: &[buck2_data::AgentContextEntry]) -> Vec<String> {
    use buck2_data::AgentContextEntry;

    const MAX_TAG_LEN: usize = 80;

    let find = |key: &str| agent_context.iter().find(|e| e.key == key);

    let Some(id_entry) = find(AgentContextEntry::KEY_ID) else {
        return Vec::new();
    };

    let mut args = Vec::new();
    let mut push_tag = |tag: String| {
        if tag.len() <= MAX_TAG_LEN {
            args.extend(["--tags".to_owned(), tag]);
        }
    };

    push_tag("ai-agent".to_owned());
    push_tag(format!("ai_agent_id={}", id_entry.value));

    if let Some(inv) = find(AgentContextEntry::KEY_INVOCATION_ID) {
        let inv_id = inv
            .value
            .rsplit_once("_invocation_")
            .map_or(inv.value.as_str(), |(_, uuid)| uuid);
        push_tag(format!("ai_inv_id={}", inv_id));
    }

    args
}

/// The error for a test executor that did not exit cleanly, or `None` if it did.
///
/// The string tags become part of the error's category key, which is the identity
/// of the Logview MIDs tracking executor deaths: signal deaths are split by signal
/// and core-dumped bit on purpose; non-zero exits are not split by status, since
/// nearly all of them are status 1 and the status is already in the message.
fn executor_termination_error(output: &ExecutorOutput) -> Option<buck2_error::Error> {
    if let Some(signal) = output.signal {
        // A crash (e.g. SIGSEGV) or an OOM kill, as opposed to an orderly non-zero exit.
        let mut error = buck2_error::buck2_error!(
            ErrorTag::TestExecutorSignaled,
            "{}",
            output.termination_message()
        );
        error = error.string_tag(&format!("signal({signal})"));
        if output.core_dumped {
            error = error.string_tag("core_dumped");
        }
        return Some(error);
    }

    if output.exit_code != Some(0) {
        return Some(buck2_error::buck2_error!(
            ErrorTag::TestExecutorNonZeroExit,
            "{}",
            output.termination_message()
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use buck2_cli_proto::RepresentativeConfigFlag;
    use buck2_cli_proto::representative_config_flag;
    use buck2_data::AgentContextEntry;

    use crate::command::InternalRunnerConfig;
    use crate::command::TestLabelFiltering;
    use crate::command::ai_agent_tpx_args;
    use crate::command::executor_termination_error;
    use crate::command::generate_config_entry_args;
    use crate::executor_launcher::ExecutorOutput;

    fn executor_output(
        exit_code: Option<i32>,
        signal: Option<i32>,
        core_dumped: bool,
    ) -> ExecutorOutput {
        ExecutorOutput {
            exit_code,
            signal,
            core_dumped,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    // These category keys identify the Logview MIDs that track executor deaths;
    // a changed key opens a new regression task.
    #[test]
    fn non_zero_exit_category_key_is_not_split_by_status() {
        let error = executor_termination_error(&executor_output(Some(1), None, false)).unwrap();
        assert_eq!(
            error.category_key(),
            "buck2_test/src/command.rs:TEST_EXECUTOR_NON_ZERO_EXIT"
        );
    }

    #[test]
    fn signal_death_category_key_includes_signal_and_core_dump() {
        let error = executor_termination_error(&executor_output(None, Some(11), true)).unwrap();
        assert_eq!(
            error.category_key(),
            "buck2_test/src/command.rs:TEST_EXECUTOR_SIGNALED:core_dumped:signal(11)"
        );
    }

    #[test]
    fn signal_death_without_core_dump_omits_core_tag() {
        let error = executor_termination_error(&executor_output(None, Some(9), false)).unwrap();
        assert_eq!(
            error.category_key(),
            "buck2_test/src/command.rs:TEST_EXECUTOR_SIGNALED:signal(9)"
        );
    }

    #[test]
    fn clean_exit_is_not_an_error() {
        assert!(executor_termination_error(&executor_output(Some(0), None, false)).is_none());
    }

    #[test]
    fn only_include_labels_in_includes() {
        let filter = TestLabelFiltering::new(
            vec!["include_me".to_owned(), "!not_me".to_owned()],
            vec!["this_doesnt_affect_anything".to_owned()],
            false,
            false,
        );

        assert!(!filter.is_excluded(vec!["include_me"]));
        assert!(filter.is_excluded(vec!["not_me"]));
        assert!(filter.is_excluded(vec!["blah"]));
    }

    /// An include list made only of `!` exclusions (`--include '!slow'`) keeps every target that
    /// is not excluded: without a positive entry there is no allow list.
    #[test]
    fn only_negative_includes_keep_unexcluded_targets() {
        let filter = TestLabelFiltering::new(vec!["!slow".to_owned()], vec![], false, false);
        assert!(filter.is_excluded(vec!["slow"]));
        assert!(!filter.is_excluded(vec!["fast"]));
        assert!(!filter.is_excluded(vec![]));

        let filter = TestLabelFiltering::new(
            vec!["!slow".to_owned(), "!flaky".to_owned()],
            vec!["broken".to_owned()],
            true,
            false,
        );
        assert!(!filter.is_excluded(vec!["fast"]));
        assert!(filter.is_excluded(vec!["flaky"]));
        assert!(filter.is_excluded(vec!["broken"]));
    }

    #[test]
    fn order_of_precedence() {
        let conflicting_filter = TestLabelFiltering::new(
            vec!["!not_me1".to_owned(), "not_me1".to_owned()],
            vec!["not_me2".to_owned()],
            false,
            false,
        );

        assert!(conflicting_filter.is_excluded(vec!["not_me1"]));

        let conflicting_filter = TestLabelFiltering::new(
            vec!["include_me".to_owned(), "!include_me".to_owned()],
            vec!["include_me".to_owned()],
            false,
            false,
        );

        assert!(!conflicting_filter.is_excluded(vec!["include_me"]));

        let conflicting_filter = TestLabelFiltering::new(
            vec!["include_me".to_owned()],
            vec!["!include_me".to_owned()],
            false,
            false,
        );

        assert!(!conflicting_filter.is_excluded(vec!["include_me"]));
    }

    #[test]
    fn always_excludes_overrides_precedence() {
        let filter = TestLabelFiltering::new(
            vec!["include_me".to_owned(), "!not_me1".to_owned()],
            vec!["not_me2".to_owned()],
            true,
            false,
        );

        assert!(!filter.is_excluded(vec!["include_me", "blah"]));
        assert!(filter.is_excluded(vec!["include_me", "not_me1"]));
        assert!(filter.is_excluded(vec!["include_me", "not_me2"]));

        let conflicting_filter = TestLabelFiltering::new(
            vec!["include_me".to_owned(), "!include_me".to_owned()],
            vec!["include_me".to_owned()],
            true,
            false,
        );

        assert!(conflicting_filter.is_excluded(vec!["include_me"]));
    }

    #[test]
    fn test_generate_config_entry_args_empty() {
        let mut args = Vec::new();
        let config_flags = Vec::new();

        generate_config_entry_args(&mut args, &config_flags);

        assert!(args.is_empty());
    }

    #[test]
    fn test_generate_config_entry_args_single_config_flag() {
        let mut args = Vec::new();
        let config_flags = vec![RepresentativeConfigFlag {
            source: Some(representative_config_flag::Source::ConfigFlag(
                "key=value".to_owned(),
            )),
        }];

        generate_config_entry_args(&mut args, &config_flags);

        assert_eq!(args, vec!["--config-entry", "config=key=value"]);
    }

    #[test]
    fn test_generate_config_entry_args_multiple_config_flags() {
        let mut args = Vec::new();
        let config_flags = vec![
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFlag(
                    "key1=value1".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFlag(
                    "key2=value2".to_owned(),
                )),
            },
        ];

        generate_config_entry_args(&mut args, &config_flags);

        assert_eq!(
            args,
            vec!["--config-entry", "config=key1=value1;key2=value2"]
        );
    }

    #[test]
    fn test_generate_config_entry_args_all_types() {
        let mut args = Vec::new();
        let config_flags = vec![
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFlag(
                    "config_key=config_value".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFile(
                    "config_file_path".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ModeFile(
                    "mode_file_path".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::Modifier(
                    "modifier_value".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::TargetPlatforms(
                    "platform1".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::TargetUniverse(
                    "//uni:target".to_owned(),
                )),
            },
        ];

        generate_config_entry_args(&mut args, &config_flags);

        // TargetUniverse is intentionally not passed to TPX, so the output
        // should contain all other types but no entry for target_universe.
        let expected = vec![
            "--config-entry",
            "config=config_key=config_value",
            "--config-entry",
            "config_file=config_file_path",
            "--config-entry",
            "mode=mode_file_path",
            "--config-entry",
            "modifier=modifier_value",
            "--config-entry",
            "target_platforms=platform1",
        ];
        assert_eq!(args, expected);
    }

    #[test]
    fn test_generate_config_entry_args_multiple_same_type() {
        let mut args = Vec::new();
        let config_flags = vec![
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFile(
                    "file1.cfg".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFile(
                    "file2.cfg".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::Modifier(
                    "mod1".to_owned(),
                )),
            },
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::Modifier(
                    "mod2".to_owned(),
                )),
            },
        ];

        generate_config_entry_args(&mut args, &config_flags);

        let expected = vec![
            "--config-entry",
            "config_file=file1.cfg;file2.cfg",
            "--config-entry",
            "modifier=mod1;mod2",
        ];
        assert_eq!(args, expected);
    }

    #[test]
    fn test_generate_config_entry_args_with_none_source() {
        let mut args = Vec::new();
        let config_flags = vec![
            RepresentativeConfigFlag {
                source: Some(representative_config_flag::Source::ConfigFlag(
                    "key=value".to_owned(),
                )),
            },
            RepresentativeConfigFlag { source: None },
        ];

        generate_config_entry_args(&mut args, &config_flags);

        assert_eq!(args, vec!["--config-entry", "config=key=value"]);
    }

    fn agent_entry(key: &str, value: &str) -> AgentContextEntry {
        AgentContextEntry {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn test_ai_agent_tpx_args_claude_code() {
        let ctx = vec![
            agent_entry("id", "claude_code"),
            agent_entry(
                "invocation_id",
                "claude_code_invocation_c9e892fa-148a-4494-8466-c1f44f1647d3",
            ),
        ];
        assert_eq!(
            ai_agent_tpx_args(&ctx),
            vec![
                "--tags",
                "ai-agent",
                "--tags",
                "ai_agent_id=claude_code",
                "--tags",
                "ai_inv_id=c9e892fa-148a-4494-8466-c1f44f1647d3",
            ]
        );
    }

    #[test]
    fn test_ai_agent_tpx_args_devmate() {
        let ctx = vec![
            agent_entry("id", "devmate_vscode"),
            agent_entry(
                "invocation_id",
                "agent--38920caa-4558-45dd-bc90-b6abff501f8a",
            ),
        ];
        let args = ai_agent_tpx_args(&ctx);
        assert_eq!(
            args,
            vec![
                "--tags",
                "ai-agent",
                "--tags",
                "ai_agent_id=devmate_vscode",
                "--tags",
                "ai_inv_id=agent--38920caa-4558-45dd-bc90-b6abff501f8a",
            ]
        );
    }

    #[test]
    fn test_ai_agent_tpx_args_no_invocation_id() {
        let ctx = vec![agent_entry("id", "some_agent")];
        assert_eq!(
            ai_agent_tpx_args(&ctx),
            vec!["--tags", "ai-agent", "--tags", "ai_agent_id=some_agent"]
        );
    }

    #[test]
    fn test_ai_agent_tpx_args_no_agent() {
        let ctx = vec![agent_entry("intent", "build")];
        let args = ai_agent_tpx_args(&ctx);
        assert!(args.is_empty());
    }

    #[test]
    fn test_ai_agent_tpx_args_tag_too_long() {
        let long_name = "a".repeat(80); // ai_agent_id= is 14 chars + 80 = 94 > 80
        let ctx = vec![agent_entry("id", &long_name)];
        let args = ai_agent_tpx_args(&ctx);
        // ai-agent (8 chars) is kept, ai_agent_id=aaa... (94 chars) is dropped
        assert_eq!(args, vec!["--tags", "ai-agent"]);
    }

    #[test]
    fn internal_runner_config_defaults_to_all() {
        let config = InternalRunnerConfig::parse(None);
        assert!(config.should_use("rust"));
        assert!(config.should_use("gtest"));
    }

    #[test]
    fn internal_runner_config_true_enables_all() {
        let config = InternalRunnerConfig::parse(Some("true"));
        assert!(config.should_use("rust"));
        assert!(config.should_use("gtest"));
    }

    #[test]
    fn internal_runner_config_false_disables_all() {
        let config = InternalRunnerConfig::parse(Some("false"));
        assert!(!config.should_use("rust"));
        assert!(!config.should_use("gtest"));
    }

    #[test]
    fn internal_runner_config_comma_separated_frameworks() {
        let config = InternalRunnerConfig::parse(Some("rust,gtest"));
        assert!(config.should_use("rust"));
        assert!(config.should_use("gtest"));
        assert!(!config.should_use("python"));
    }

    #[test]
    fn internal_runner_config_trims_whitespace() {
        let config = InternalRunnerConfig::parse(Some(" rust , gtest "));
        assert!(config.should_use("rust"));
        assert!(config.should_use("gtest"));
    }

    #[test]
    fn internal_runner_config_empty_string_disables() {
        let config = InternalRunnerConfig::parse(Some(""));
        assert!(!config.should_use("rust"));
    }
}
