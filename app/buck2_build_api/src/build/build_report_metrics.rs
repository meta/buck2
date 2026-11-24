/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_data::ToProtoMessage;
use buck2_error::internal_error;
use buck2_events::dispatch::instant_event;
use buck2_hash::BuckMutSet;
use buck2_settings::BuckconfigSource;
use buck2_settings::SketchedProvider;
use buck2_settings::dice::HasInvocationBuckSettings;
use dice::DiceComputations;
use dupe::Dupe;

use crate::actions::artifact::get_artifact_fs::GetArtifactFs;
use crate::build::BuildProviderType;
use crate::build::BuildTargetResult;
use crate::build::detailed_aggregated_metrics::dice::HasDetailedAggregatedMetrics;
use crate::build::detailed_aggregated_metrics::types::ActionGraphSketchResult;
use crate::build::detailed_aggregated_metrics::types::ArtifactPathSketchResult;
use crate::build::detailed_aggregated_metrics::types::DetailedAggregatedMetrics;
use crate::build::graph_properties::GraphPropertiesOptions;

pub struct BuildReportMetricsConfig {
    pub graph_properties: GraphPropertiesOptions,
    detailed_aggregated_metrics: bool,
    providers_to_skip_in_artifact_path_sketch: BuckMutSet<BuildProviderType>,
}

impl BuildReportMetricsConfig {
    /// What the command's `build_metrics` settings ask for.
    pub async fn new(ctx: &mut DiceComputations<'_>) -> buck2_error::Result<Self> {
        let invocation_settings = ctx.per_transaction_data().get_invocation_buck_settings();
        let settings = &invocation_settings.build_metrics;
        let root_config = ctx.get_legacy_root_config_on_dice().await?;
        let buckconfig: &mut dyn BuckconfigSource = &mut root_config.view(ctx);
        let graph_properties = GraphPropertiesOptions {
            configured_graph_size: settings.configured_graph_size(buckconfig)?,
            configured_graph_sketch: settings.configured_graph_sketch(buckconfig)?,
            total_configured_graph_sketch: settings.total_configured_graph_sketch(buckconfig)?,
            retained_analysis_memory_sketch: settings
                .retained_analysis_memory_sketch(buckconfig)?,
            peak_analysis_memory_sketch: settings.peak_analysis_memory_sketch(buckconfig)?,
            peak_load_memory_sketch: settings.peak_load_memory_sketch(buckconfig)?,
            action_graph_sketch: settings.action_graph_sketch(buckconfig)?,
            artifact_count_sketch: settings.artifact_count_sketch(buckconfig)?,
            artifact_size_sketch: settings.artifact_size_sketch(buckconfig)?,
            log_sketch_cardinalities: settings.sketch_cardinalities(buckconfig)?,
        };

        let providers_to_skip_in_artifact_path_sketch = settings
            .artifact_sketch_skip_providers(buckconfig)?
            .unwrap_or_default()
            .into_iter()
            .map(|provider| match provider {
                SketchedProvider::Build => BuildProviderType::Default,
                SketchedProvider::Run => BuildProviderType::Run,
                SketchedProvider::Test => BuildProviderType::Test,
            })
            .collect();

        Ok(Self {
            graph_properties,
            detailed_aggregated_metrics: settings.detailed_aggregated(buckconfig)?,
            providers_to_skip_in_artifact_path_sketch,
        })
    }
}

pub struct BuildReportSupplementalData {
    pub detailed_metrics: Option<DetailedAggregatedMetrics>,
    pub action_graph_sketch_result: Option<ActionGraphSketchResult>,
    pub artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
}

pub async fn compute_build_report_supplemental_data(
    ctx: &mut DiceComputations<'_>,
    build_result: &BuildTargetResult,
    config: BuildReportMetricsConfig,
) -> buck2_error::Result<BuildReportSupplementalData> {
    let BuildReportMetricsConfig {
        graph_properties,
        detailed_aggregated_metrics,
        providers_to_skip_in_artifact_path_sketch,
    } = config;

    let need_events = detailed_aggregated_metrics
        || graph_properties.action_graph_sketch
        || graph_properties.artifact_count_sketch
        || graph_properties.artifact_size_sketch;
    let mut events = if need_events {
        Some(ctx.take_per_build_events()?)
    } else {
        None
    };

    // Detailed metrics consume the event collection, so compute the sketches that borrow it first.
    let action_graph_sketch_result = if graph_properties.action_graph_sketch {
        let events = events.as_ref().ok_or_else(|| {
            internal_error!("events should be Some when action graph sketch is needed")
        })?;
        Some(ctx.compute_action_graph_sketch(events).await?)
    } else {
        None
    };

    let artifact_path_sketch_result =
        if graph_properties.artifact_count_sketch || graph_properties.artifact_size_sketch {
            let events = events.as_ref().ok_or_else(|| {
                internal_error!("events should be Some when artifact path sketch is needed")
            })?;
            // Re-ensuring artifacts for a timed-out target can restart the action cancelled by the
            // overall timeout and hang the command past its deadline.
            let timed_out_targets: BuckMutSet<_> = build_result
                .configured
                .iter()
                .filter_map(|(label, result)| {
                    result
                        .as_ref()
                        .filter(|result| result.timed_out())
                        .map(|_| label.dupe())
                })
                .collect();
            let artifact_fs = ctx.get_artifact_fs().await?;
            Some(
                ctx.compute_artifact_path_sketch(
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

    let detailed_metrics = if detailed_aggregated_metrics {
        let events = events.take().ok_or_else(|| {
            internal_error!("events should be Some when detailed metrics are needed")
        })?;
        let mut metrics = ctx.compute_detailed_metrics(events).await?;
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

    Ok(BuildReportSupplementalData {
        detailed_metrics,
        action_graph_sketch_result,
        artifact_path_sketch_result,
    })
}
