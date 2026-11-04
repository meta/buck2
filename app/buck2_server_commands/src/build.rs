/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::sync::Arc;
use std::time::Instant;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::build;
use buck2_build_api::build::AsyncBuildTargetResultBuilder;
use buck2_build_api::build::BuildEvent;
use buck2_build_api::build::BuildEventConsumer;
use buck2_build_api::build::BuildProviderType;
use buck2_build_api::build::BuildTargetResult;
use buck2_build_api::build::ConfiguredBuildEventVariant;
use buck2_build_api::build::HasCreateUnhashedSymlinkLock;
use buck2_build_api::build::ProvidersToBuild;
use buck2_build_api::build::build_report::build_report_opts;
use buck2_build_api::build::build_report::write_build_report;
use buck2_build_api::build::detailed_aggregated_metrics::dice::HasDetailedAggregatedMetrics;
use buck2_build_api::build::detailed_aggregated_metrics::types::ActionGraphSketchResult;
use buck2_build_api::build::detailed_aggregated_metrics::types::ArtifactPathSketchResult;
use buck2_build_api::build::detailed_aggregated_metrics::types::DetailedAggregatedMetrics;
use buck2_build_api::build::graph_properties::GraphPropertiesOptions;
use buck2_build_api::materialize::MaterializationAndUploadContext;
use buck2_cli_proto::CommonBuildOptions;
use buck2_cli_proto::build_request::BuildProviders;
use buck2_cli_proto::build_request::Materializations;
use buck2_cli_proto::build_request::Uploads;
use buck2_cli_proto::build_request::build_providers::Action as BuildProviderAction;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_common::liveliness_observer::TimeoutLivelinessObserver;
use buck2_common::pattern::parse_from_cli::parse_patterns_with_modifiers_from_cli_args;
use buck2_common::pattern::resolve::ResolveTargetPatterns;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_core::deprecation;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::package::PackageLabelWithModifiers;
use buck2_core::pattern::pattern::Modifiers;
use buck2_core::pattern::pattern::ModifiersError;
use buck2_core::pattern::pattern::PackageSpec;
use buck2_core::pattern::pattern::ParsedPatternWithModifiers;
use buck2_core::pattern::pattern::TargetLabelWithModifiers;
use buck2_core::pattern::pattern_type::ConfiguredProvidersPatternExtra;
use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_core::target::label::label::TargetLabel;
use buck2_data::BuildResult;
use buck2_data::ToProtoMessage;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_events::dispatch::console_message;
use buck2_events::dispatch::instant_event;
use buck2_events::dispatch::span_async;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_execute::re::invocation_re_settings::HasInvocationReSettings;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::load_patterns::MissingTargetBehavior;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_server_ctx::commands::send_build_phase_end;
use buck2_server_ctx::commands::send_target_cfg_event;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::target_resolution_config::TargetResolutionConfig;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::DiceTransaction;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;
use futures::future;
use futures::future::FutureExt;
use futures::stream::StreamExt;
use futures::stream::futures_unordered::FuturesUnordered;
use itertools::Either;
use itertools::Itertools;
use tokio::sync::mpsc::UnboundedSender;

use crate::build::agent_advice::BuildAdviceContext;
use crate::build::result_report::ResultReporter;
use crate::build::result_report::ResultReporterOptions;
use crate::build::unhashed_outputs::create_unhashed_outputs_via_materializer;

mod agent_advice;
mod result_report;
mod unhashed_outputs;

pub(crate) async fn build_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
    req: buck2_cli_proto::BuildRequest,
) -> buck2_error::Result<buck2_cli_proto::BuildResponse> {
    run_server_command(BuildServerCommand { req }, ctx, partial_result_dispatcher).await
}

struct BuildServerCommand {
    req: buck2_cli_proto::BuildRequest,
}

#[async_trait]
impl ServerCommandTemplate for BuildServerCommand {
    type StartEvent = buck2_data::BuildCommandStart;
    type EndEvent = buck2_data::BuildCommandEnd;
    type Response = buck2_cli_proto::BuildResponse;
    type PartialResult = NoPartialResult;

    fn end_event(&self, _response: &buck2_error::Result<Self::Response>) -> Self::EndEvent {
        buck2_data::BuildCommandEnd {
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
        build(server_ctx, ctx, &self.req).await
    }

    fn build_result(&self, response: &Self::Response) -> Option<BuildResult> {
        Some(BuildResult {
            build_completed: response.errors.is_empty(),
        })
    }
}

fn expect_build_opts(req: &buck2_cli_proto::BuildRequest) -> &CommonBuildOptions {
    req.build_opts.as_ref().expect("should have build options")
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
#[error(
    "`buck2 run` will require a `--` separator before target arguments in the future. \
     Please use `buck2 run <target> -- <args>` instead of `buck2 run <target> <args>`"
)]
struct RunArgsMissingSeparator;

async fn build(
    server_ctx: &dyn ServerCommandContextTrait,
    ctx: DiceTransaction,
    request: &buck2_cli_proto::BuildRequest,
) -> buck2_error::Result<buck2_cli_proto::BuildResponse> {
    if request.run_args_missing_separator {
        deprecation!(
            "run_args_without_separator",
            RunArgsMissingSeparator.into(),
            owner: "mattpayne", started: "2026-02-17",
        );
    }

    let cwd = server_ctx.working_dir();

    let build_opts: &CommonBuildOptions = expect_build_opts(request);

    let timeout = request
        .timeout
        .as_ref()
        .map(|t| (*t).try_into())
        .transpose()
        .with_buck_error_context(|| "Invalid `duration`")?;

    let timeout_observer = timeout.map(|timeout| {
        Arc::new(TimeoutLivelinessObserver::new(timeout)) as Arc<dyn LivelinessObserver>
    });

    let cell_resolver = ctx.ctx().get_cell_resolver().await?;

    let parsed_patterns_with_modifiers: Vec<
        ParsedPatternWithModifiers<ConfiguredProvidersPatternExtra>,
    > = parse_patterns_with_modifiers_from_cli_args(&mut ctx.ctx(), &request.target_patterns, cwd)
        .await?;

    let has_pattern_modifiers = parsed_patterns_with_modifiers
        .iter()
        .any(|p| p.modifiers.as_slice().is_some());

    server_ctx.log_target_pattern_with_modifiers(&parsed_patterns_with_modifiers);

    let resolved_pattern: ResolvedPattern<ConfiguredProvidersPatternExtra> =
        ResolveTargetPatterns::resolve_with_modifiers(
            &mut ctx.ctx(),
            &parsed_patterns_with_modifiers,
        )
        .await?;

    let target_resolution_config = TargetResolutionConfig::from_args(
        &mut ctx.ctx(),
        request
            .target_cfg
            .as_ref()
            .internal_error("target_cfg must be set")?,
        server_ctx,
        &request.target_universe,
    )
    .await?;

    match &target_resolution_config {
        TargetResolutionConfig::Default(global_cfg_options) => {
            if !global_cfg_options.cli_modifiers.is_empty() && has_pattern_modifiers {
                return Err(ModifiersError::PatternModifiersWithGlobalModifiers.into());
            }
        }
        TargetResolutionConfig::Universe(_) => {
            if has_pattern_modifiers {
                return Err(ModifiersError::PatternModifiersWithTargetUniverse.into());
            }
        }
    }

    let build_providers = Arc::new(request.build_providers.unwrap());

    let final_artifact_materializations =
        Materializations::try_from(request.final_artifact_materializations)
            .with_buck_error_context(|| "Invalid final_artifact_materializations")?;
    let final_artifact_uploads = Uploads::try_from(request.final_artifact_uploads)
        .with_buck_error_context(|| "Invalid final_artifact_uploads")
        .unwrap();

    let want_configured_graph_size = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_configured_graph_size",
            },
        )
        .await?
        .unwrap_or_default();

    let want_configured_graph_sketch = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_configured_graph_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_total_configured_graph_sketch = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_total_configured_graph_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_retained_analysis_memory_sketch = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_retained_analysis_memory_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_action_graph_sketch = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_action_graph_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_peak_analysis_memory_sketch = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_peak_analysis_memory_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_peak_load_memory_sketch = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_peak_load_memory_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_artifact_count_sketch: bool = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_artifact_count_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_artifact_size_sketch: bool = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_artifact_size_sketch",
            },
        )
        .await?
        .unwrap_or_default();

    let want_log_sketch_cardinalities: bool = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "log_sketch_cardinalities",
            },
        )
        .await?
        .unwrap_or_default();

    let graph_properties = GraphPropertiesOptions {
        configured_graph_size: want_configured_graph_size,
        configured_graph_sketch: want_configured_graph_sketch,
        total_configured_graph_sketch: want_total_configured_graph_sketch,
        retained_analysis_memory_sketch: want_retained_analysis_memory_sketch,
        peak_analysis_memory_sketch: want_peak_analysis_memory_sketch,
        peak_load_memory_sketch: want_peak_load_memory_sketch,
        action_graph_sketch: want_action_graph_sketch,
        artifact_count_sketch: want_artifact_count_sketch,
        artifact_size_sketch: want_artifact_size_sketch,
        log_sketch_cardinalities: want_log_sketch_cardinalities,
    };

    let providers_to_skip_in_artifact_path_sketch: BuckMutSet<BuildProviderType> = ctx
        .ctx()
        .parse_legacy_config_list_property::<SkipProvider>(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "providers_to_skip_in_artifact_path_sketch",
            },
        )
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.into_build_provider_type())
        .collect();

    let (streaming_build_result_tx, streaming_build_result_rx) =
        build::build_report::streaming_build_result_channel(build_opts);

    let return_run_args = request
        .response_options
        .as_ref()
        .is_some_and(|o| o.return_run_args);
    let build_start = Instant::now();
    let build_advice = BuildAdviceContext::new(&server_ctx.buck_settings().invocation, request);
    let cloned_ctx = ctx.clone(); // build_future does a mutable borrow on the context, so we clone it first
    let mut dice = ctx.ctx();
    let build_future = dice.with_linear_recompute(|ctx| {
        async move {
            build_targets(
                ctx,
                resolved_pattern,
                target_resolution_config,
                build_providers,
                (final_artifact_materializations, final_artifact_uploads).into(),
                build_opts.fail_fast,
                MissingTargetBehavior::from_skip(build_opts.skip_missing_targets),
                build_opts.skip_incompatible_targets,
                graph_properties.dupe(),
                return_run_args,
                timeout_observer.as_ref(),
                streaming_build_result_tx,
                build_start,
                build_advice.as_ref(),
            )
            .await
        }
        .boxed()
    });

    let event_dispatcher = server_ctx.events().dupe();
    let build_future = build_future.map(move |result| {
        let build_succeeded = result.as_ref().is_ok_and(|r| !r.build_failed);
        send_build_phase_end(&event_dispatcher, SystemTime::now(), build_succeeded);
        result
    });

    let build_result = build::build_report::maybe_stream_build_reports(
        build_future,
        build_opts,
        cloned_ctx,
        server_ctx.project_root(),
        server_ctx.working_dir(),
        server_ctx.events().trace_id().dupe(),
        graph_properties.dupe(),
        streaming_build_result_rx,
    )
    .await?;

    let want_detailed_metrics = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "detailed_aggregated_metrics",
            },
        )
        .await?
        .unwrap_or_default();

    // We need to take per-build events if we want either detailed metrics or action graph sketch
    // or artifact path sketch
    let need_events = want_detailed_metrics
        || graph_properties.action_graph_sketch
        || graph_properties.artifact_count_sketch
        || graph_properties.artifact_size_sketch;
    let mut events = if need_events {
        Some(ctx.ctx().take_per_build_events()?)
    } else {
        None
    };

    // Compute action graph sketch independently if requested (doesn't require detailed_metrics)
    let action_graph_sketch_result = if graph_properties.action_graph_sketch {
        if let Some(ref events) = events {
            Some(ctx.ctx().compute_action_graph_sketch(events).await?)
        } else {
            None
        }
    } else {
        None
    };

    let artifact_path_sketch_result =
        if graph_properties.artifact_count_sketch || graph_properties.artifact_size_sketch {
            let events = events.as_ref().ok_or_else(|| {
                internal_error!("events should be Some when artifact path sketch is needed")
            })?;
            // Artifact path sketching re-`ensure_artifact_group`s each target's outputs. For a
            // target that hit the `--overall-timeout` deadline that would re-demand (and, via
            // DICE, restart) an action the deadline just cancelled, hanging the command past its
            // timeout. Those targets never finished building, so skip sketching them.
            let timed_out_targets: BuckMutSet<ConfiguredProvidersLabel> = build_result
                .configured
                .iter()
                .filter_map(|(label, result)| {
                    result
                        .as_ref()
                        .filter(|r| r.timed_out())
                        .map(|_| label.clone())
                })
                .collect();
            let artifact_fs = ctx.ctx().get_artifact_fs().await?;
            Some(
                ctx.ctx()
                    .compute_artifact_path_sketch(
                        events,
                        artifact_fs,
                        providers_to_skip_in_artifact_path_sketch,
                        &timed_out_targets,
                        graph_properties.artifact_count_sketch,
                        graph_properties.artifact_size_sketch,
                    )
                    .await?,
            )
        } else {
            None
        };

    let detailed_metrics = if want_detailed_metrics {
        let events = events.take().ok_or_else(|| {
            internal_error!("events should be Some when detailed metrics is needed")
        })?;
        let mut metrics = ctx.ctx().compute_detailed_metrics(events).await?;
        for target_metric in &mut metrics.top_level_target_metrics {
            if let Some(Some(result)) = build_result.configured.get(&target_metric.target) {
                target_metric.wall_clock_completion_ms =
                    result.wall_clock_completion().map(|d| d.as_millis() as u64);
            }
        }
        instant_event(metrics.as_proto());
        Some(metrics)
    } else {
        None
    };

    send_target_cfg_event(
        server_ctx.events(),
        build_result.configured.keys(),
        &request.target_cfg,
    );

    process_build_result(
        server_ctx,
        ctx,
        request,
        build_result,
        detailed_metrics,
        action_graph_sketch_result,
        artifact_path_sketch_result,
        final_artifact_materializations,
        graph_properties,
    )
    .await
}

async fn process_build_result(
    server_ctx: &dyn ServerCommandContextTrait,
    ctx: DiceTransaction,
    request: &buck2_cli_proto::BuildRequest,
    build_result: BuildTargetResult,
    detailed_metrics: Option<DetailedAggregatedMetrics>,
    action_graph_sketch_result: Option<ActionGraphSketchResult>,
    artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
    final_artifact_materializations: Materializations,
    graph_properties_opts: GraphPropertiesOptions,
) -> buck2_error::Result<buck2_cli_proto::BuildResponse> {
    let fs = server_ctx.project_root();
    let cwd = server_ctx.working_dir();

    let build_opts = expect_build_opts(request);
    let response_options = request.response_options.unwrap_or_default();

    let cell_resolver = ctx.ctx().get_cell_resolver().await?;
    let artifact_fs = ctx.ctx().get_artifact_fs().await?;

    let result_reports = ResultReporter::convert(
        artifact_fs,
        server_ctx.cert_state(),
        ResultReporterOptions {
            return_outputs: response_options.return_outputs,
        },
        &build_result,
    )
    .await?;

    let serialized_build_report = if build_opts.unstable_print_build_report {
        let build_report_opts = build_report_opts(
            &mut ctx.ctx(),
            &cell_resolver,
            build_opts,
            graph_properties_opts,
        )
        .await?;

        write_build_report(
            build_report_opts,
            artifact_fs,
            &cell_resolver,
            fs,
            cwd,
            server_ctx.events().trace_id(),
            &build_result.configured,
            &build_result.configured_to_pattern_modifiers,
            &build_result.other_errors,
            detailed_metrics,
            action_graph_sketch_result,
            artifact_path_sketch_result,
        )?
    } else {
        None
    };

    let mut provider_artifacts = Vec::new();
    for v in build_result.configured.into_values() {
        // We omit skipped targets here.
        let Some(v) = v else { continue };
        let mut outputs = v.outputs.into_iter().filter_map(|t| t.inner.ok());
        provider_artifacts.extend(&mut outputs);
    }

    let should_create_unhashed_links = ctx
        .ctx()
        .parse_legacy_config_property(
            cell_resolver.root_cell(),
            BuckconfigKeyRef {
                section: "buck2",
                property: "create_unhashed_links",
            },
        )
        .await?;

    if should_create_unhashed_links.unwrap_or(false) {
        span_async(buck2_data::CreateOutputSymlinksStart {}, async {
            let lock = ctx
                .per_transaction_data()
                .get_create_unhashed_symlink_lock();
            let _guard = lock.lock().await;
            let res = create_unhashed_outputs_via_materializer(
                provider_artifacts,
                artifact_fs,
                ctx.global_data().get_digest_config(),
                ctx.per_transaction_data().get_materializer(),
                final_artifact_materializations,
                ctx.per_transaction_data()
                    .get_invocation_re_settings()
                    .use_case,
            )
            .await;
            (
                res,
                // The materializer creates these links; it does not list them yet.
                buck2_data::CreateOutputSymlinksEnd {
                    symlinks: Vec::new(),
                },
            )
        })
        .await?;
    }

    let build_targets = result_reports.build_targets;
    let errors = result_reports
        .build_errors
        .errors
        .iter()
        .map(buck2_data::ErrorReport::from)
        .unique_by(|e| e.message.clone())
        .collect();

    let project_root = server_ctx.project_root().to_string();

    Ok(buck2_cli_proto::BuildResponse {
        build_targets,
        project_root,
        serialized_build_report,
        errors,
    })
}

async fn build_targets(
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    spec: ResolvedPattern<ConfiguredProvidersPatternExtra>,
    target_resolution_config: TargetResolutionConfig,
    build_providers: Arc<BuildProviders>,
    materialization_and_upload: MaterializationAndUploadContext,
    fail_fast: bool,
    missing_target_behavior: MissingTargetBehavior,
    skip_incompatible_targets: bool,
    graph_properties: GraphPropertiesOptions,
    return_run_args: bool,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
    streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
    build_start: Instant,
    build_advice: Option<&BuildAdviceContext<'_>>,
) -> buck2_error::Result<BuildTargetResult> {
    let (builder, consumer) =
        AsyncBuildTargetResultBuilder::new(streaming_build_result_tx, build_start);
    let fut = match target_resolution_config {
        TargetResolutionConfig::Default(global_cfg_options) => {
            let spec = spec.convert_pattern().buck_error_context(
                "Targets with explicit configuration can only be built when the `--target-universe=` flag is provided",
            )?;
            build_targets_with_global_target_platform(
                &consumer,
                ctx,
                spec,
                global_cfg_options,
                build_providers,
                materialization_and_upload,
                missing_target_behavior,
                skip_incompatible_targets,
                graph_properties,
                return_run_args,
                timeout_observer,
                build_advice,
            )
            .left_future()
        }
        TargetResolutionConfig::Universe(universe) => build_targets_in_universe(
            &consumer,
            ctx,
            spec,
            universe,
            build_providers,
            materialization_and_upload,
            graph_properties,
            return_run_args,
            timeout_observer,
            build_advice,
        )
        .right_future(),
    };

    builder.wait_for(fail_fast, fut).await
}

async fn build_targets_in_universe(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    spec: ResolvedPattern<ConfiguredProvidersPatternExtra>,
    universe: CqueryUniverse,
    build_providers: Arc<BuildProviders>,
    materialization_and_upload: MaterializationAndUploadContext,
    graph_properties: GraphPropertiesOptions,
    return_run_args: bool,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
    build_advice: Option<&BuildAdviceContext<'_>>,
) {
    let providers_to_build = build_providers_to_providers_to_build(&build_providers);
    let provider_labels = universe.get_provider_labels(&spec);
    if provider_labels.is_empty() {
        console_message(
            "\nNo targets found inside the specified universe, nothing will be built\n\n"
                .to_owned(),
        );
    }
    // No `MapModifiers` is emitted on this path, so the report files every entry without
    // modifiers.
    group_by_key(provider_labels, |p| p.target().unconfigured().dupe())
        .into_iter()
        .map(|(target, labels)| {
            buck2_util::async_move_clone!(providers_to_build, {
                let providers_to_build = &providers_to_build;
                group_by_key(labels, |label| label.target().dupe())
                    .into_iter()
                    .map(|(configured_target, labels)| async move {
                        labels
                            .into_iter()
                            .map(|label| {
                                agent_advice::build_configured_label(
                                    event_consumer,
                                    ctx,
                                    materialization_and_upload,
                                    label,
                                    providers_to_build,
                                    build::BuildConfiguredLabelOptions {
                                        skippable: false,
                                        graph_properties,
                                        return_run_args,
                                    },
                                    timeout_observer,
                                    build_advice,
                                )
                            })
                            .collect::<FuturesUnordered<_>>()
                            .collect::<()>()
                            .await;
                        event_consumer.consume(BuildEvent::ConfiguredTargetBuildFinished {
                            target: configured_target,
                            modifiers: Modifiers::new(None),
                        });
                    })
                    .collect::<FuturesUnordered<_>>()
                    .collect::<()>()
                    .await;
                event_consumer.consume(BuildEvent::TargetBuildFinished {
                    target: without_modifiers(target),
                });
            })
        })
        .collect::<FuturesUnordered<_>>()
        .collect()
        .await
}

async fn build_targets_with_global_target_platform(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    spec: ResolvedPattern<ProvidersPatternExtra>,
    global_cfg_options: GlobalCfgOptions,
    build_providers: Arc<BuildProviders>,
    materialization_and_upload: MaterializationAndUploadContext,
    missing_target_behavior: MissingTargetBehavior,
    skip_incompatible_targets: bool,
    graph_properties: GraphPropertiesOptions,
    return_run_args: bool,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
    build_advice: Option<&BuildAdviceContext<'_>>,
) {
    let global_cfg_options = &global_cfg_options;
    let build_providers = &build_providers;
    spec.specs
        .into_iter()
        .map(move |(package_with_modifiers, spec)| async move {
            build_targets_for_spec(
                event_consumer,
                ctx,
                spec,
                package_with_modifiers,
                global_cfg_options.dupe(),
                build_providers.dupe(),
                materialization_and_upload,
                missing_target_behavior,
                skip_incompatible_targets,
                graph_properties,
                return_run_args,
                timeout_observer,
                build_advice,
            )
            .await
        })
        .collect::<FuturesUnordered<_>>()
        .collect()
        .await
}

struct TargetBuildSpec {
    target: ProvidersLabel,
    global_cfg_options: GlobalCfgOptions,
    modifiers: Modifiers,
    // Indicates whether this target was explicitly requested or not. If it's the result
    // of something like `//foo/...` we can skip it (for example if it's incompatible with
    // the target platform).
    skippable: bool,
    graph_properties: GraphPropertiesOptions,
    return_run_args: bool,
}

fn build_providers_to_providers_to_build(build_providers: &BuildProviders) -> ProvidersToBuild {
    let mut providers_to_build = ProvidersToBuild::default();

    if build_providers.default_info != BuildProviderAction::Skip as i32 {
        providers_to_build.default = true;
        providers_to_build.default_other = true;
    }

    if build_providers.test_info != BuildProviderAction::Skip as i32 {
        providers_to_build.tests = true;
    }

    if build_providers.run_info != BuildProviderAction::Skip as i32 {
        providers_to_build.run = true;
    }

    providers_to_build
}

async fn build_targets_for_spec(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    spec: PackageSpec<ProvidersPatternExtra>,
    package_with_modifiers: PackageLabelWithModifiers,
    global_cfg_options: GlobalCfgOptions,
    build_providers: Arc<BuildProviders>,
    materialization_and_upload: MaterializationAndUploadContext,
    missing_target_behavior: MissingTargetBehavior,
    skip_incompatible_targets: bool,
    graph_properties: GraphPropertiesOptions,
    return_run_args: bool,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
    build_advice: Option<&BuildAdviceContext<'_>>,
) {
    let skippable = match spec {
        PackageSpec::Targets(..) => skip_incompatible_targets,
        PackageSpec::All() => true,
    };

    let PackageLabelWithModifiers { package, modifiers } = package_with_modifiers;

    let res = match ctx.get().get_interpreter_results(package.dupe()).await {
        Ok(res) => res,
        Err(e) => {
            let e: buck2_error::Error = e;
            // Try to associate the error to concrete targets, if possible
            let targets: Vec<_> = match spec {
                PackageSpec::Targets(targets) => Either::Left(
                    targets
                        .into_iter()
                        .map(move |(t, providers)| {
                            ProvidersLabel::new(
                                TargetLabel::new(package.dupe(), t.as_ref()),
                                providers.providers,
                            )
                        })
                        .map(Some),
                ),
                PackageSpec::All() => Either::Right(std::iter::once(None)),
            }
            .collect();
            for t in &targets {
                event_consumer.consume(BuildEvent::OtherError {
                    label: t.dupe(),
                    err: e.dupe(),
                });
            }
            finish_error_only_targets(
                event_consumer,
                targets.into_iter().flatten().map(|t| t.target().dupe()),
            );
            return;
        }
    };
    let (targets, missing) = res.apply_spec(spec);
    if let Some(missing) = missing {
        match missing_target_behavior {
            MissingTargetBehavior::Fail => {
                let missing_targets: Vec<TargetLabel> = missing
                    .into_all_errors()
                    .map(|err| {
                        let target = TargetLabel::new(err.package.dupe(), err.target.as_ref());
                        event_consumer.consume(BuildEvent::OtherError {
                            label: Some(ProvidersLabel::new(target.dupe(), ProvidersName::Default)),
                            err: err.into(),
                        });
                        target
                    })
                    .collect();
                finish_error_only_targets(event_consumer, missing_targets);
            }
            MissingTargetBehavior::Warn => {
                // TODO: This should be reported in the build report eventually.
                console_message(missing.missing_targets_warning());
            }
        }
    }
    let todo_targets: Vec<TargetBuildSpec> = targets
        .into_iter()
        .map(|((_target_name, extra), target)| TargetBuildSpec {
            target: ProvidersLabel::new(target.label().dupe(), extra.providers),
            global_cfg_options: global_cfg_options.dupe(),
            modifiers: modifiers.dupe(),
            skippable,
            graph_properties,
            return_run_args,
        })
        .collect();

    let providers_to_build = build_providers_to_providers_to_build(&build_providers);

    group_by_key(todo_targets, |build_spec| build_spec.target.target().dupe())
        .into_iter()
        .map(|(target, build_specs)| {
            buck2_util::async_move_clone!(providers_to_build, modifiers, {
                let configured: Vec<ConfiguredProvidersLabel> = build_specs
                    .into_iter()
                    .map(|build_spec| {
                        build_target(
                            event_consumer,
                            ctx,
                            build_spec,
                            &providers_to_build,
                            materialization_and_upload,
                            timeout_observer,
                            build_advice,
                        )
                    })
                    .collect::<FuturesUnordered<_>>()
                    .filter_map(future::ready)
                    .collect()
                    .await;

                // A request's labels for one target all configure it the same way, so its
                // configured target finishes with the last of them, not the first.
                configured
                    .iter()
                    .map(|label| label.target().dupe())
                    .unique()
                    .for_each(|target| {
                        event_consumer.consume(BuildEvent::ConfiguredTargetBuildFinished {
                            target,
                            modifiers: modifiers.dupe(),
                        })
                    });
                // If every label failed to configure, the key is that of a request without
                // modifiers, which another spec of this target can also emit. The configure
                // error is filed under every entry of the target, so that entry reads FAIL
                // whichever spec finishes it.
                let target = if configured.is_empty() {
                    without_modifiers(target)
                } else {
                    TargetLabelWithModifiers {
                        target_label: target,
                        modifiers,
                    }
                };
                event_consumer.consume(BuildEvent::TargetBuildFinished { target });
            })
        })
        .collect::<FuturesUnordered<_>>()
        .collect()
        .await
}

/// `Itertools::into_group_map_by`, but into a `BuckMutMap`.
fn group_by_key<K: Eq + Hash, V>(
    items: impl IntoIterator<Item = V>,
    key: impl Fn(&V) -> K,
) -> BuckMutMap<K, Vec<V>> {
    items
        .into_iter()
        .fold(BuckMutMap::default(), |mut groups, item| {
            groups.entry(key(&item)).or_default().push(item);
            groups
        })
}

/// The report key for an entry that has no `MapModifiers`, whatever it was requested with.
fn without_modifiers(target_label: TargetLabel) -> TargetLabelWithModifiers {
    TargetLabelWithModifiers {
        target_label,
        modifiers: Modifiers::new(None),
    }
}

/// Emits `TargetBuildFinished` for targets that failed before configuring, whose entries hold
/// only errors.
fn finish_error_only_targets(
    event_consumer: &dyn BuildEventConsumer,
    targets: impl IntoIterator<Item = TargetLabel>,
) {
    targets.into_iter().unique().for_each(|target| {
        event_consumer.consume(BuildEvent::TargetBuildFinished {
            target: without_modifiers(target),
        })
    });
}

/// Returns the configured label, or `None` if the target failed to configure.
async fn build_target(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    spec: TargetBuildSpec,
    providers_to_build: &ProvidersToBuild,
    materialization_and_upload: MaterializationAndUploadContext,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
    build_advice: Option<&BuildAdviceContext<'_>>,
) -> Option<ConfiguredProvidersLabel> {
    let local_cfg_options = match spec.modifiers.as_slice() {
        None => spec.global_cfg_options.dupe(),
        Some(modifiers) => GlobalCfgOptions {
            target_platform: spec.global_cfg_options.target_platform.dupe(),
            cli_modifiers: modifiers.to_vec().into(),
        },
    };
    let providers_label = match ctx
        .get()
        .get_configured_provider_label(&spec.target, &local_cfg_options)
        .await
    {
        Ok(configured_label) => {
            event_consumer.consume(BuildEvent::new_configured(
                configured_label.dupe(),
                ConfiguredBuildEventVariant::MapModifiers {
                    modifiers: spec.modifiers,
                },
            ));
            configured_label
        }
        Err(e) => {
            event_consumer.consume(BuildEvent::OtherError {
                label: Some(spec.target.dupe()),
                err: e,
            });
            return None;
        }
    };

    agent_advice::build_configured_label(
        event_consumer,
        ctx,
        materialization_and_upload,
        providers_label.dupe(),
        providers_to_build,
        build::BuildConfiguredLabelOptions {
            skippable: spec.skippable,
            graph_properties: spec.graph_properties,
            return_run_args: spec.return_run_args,
        },
        timeout_observer,
        build_advice,
    )
    .await;
    Some(providers_label)
}

/// Provider types that can be skipped in artifact path sketch computation.
/// Parsed from `buck2.providers_to_skip_in_artifact_path_sketch` buckconfig.
enum SkipProvider {
    Build,
    Run,
    Test,
}

impl SkipProvider {
    fn into_build_provider_type(self) -> BuildProviderType {
        match self {
            SkipProvider::Build => BuildProviderType::Default,
            SkipProvider::Run => BuildProviderType::Run,
            SkipProvider::Test => BuildProviderType::Test,
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[error("Invalid skip provider: `{0}`. Valid values are: [`build`, `run`, `test`]")]
#[buck2(input)]
struct SkipProviderParseError(String);

impl std::str::FromStr for SkipProvider {
    type Err = SkipProviderParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "build" => Ok(SkipProvider::Build),
            "run" => Ok(SkipProvider::Run),
            "test" => Ok(SkipProvider::Test),
            other => Err(SkipProviderParseError(other.to_owned())),
        }
    }
}
