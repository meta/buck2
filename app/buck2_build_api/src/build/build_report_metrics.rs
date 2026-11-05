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
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::cells::name::CellName;
use buck2_data::ToProtoMessage;
use buck2_error::internal_error;
use buck2_events::dispatch::instant_event;
use buck2_hash::BuckMutSet;
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
    pub async fn from_buckconfig(
        ctx: &mut DiceComputations<'_>,
        root_cell: CellName,
    ) -> buck2_error::Result<Self> {
        let graph_properties = GraphPropertiesOptions {
            configured_graph_size: bool_property(ctx, root_cell, "log_configured_graph_size")
                .await?,
            configured_graph_sketch: bool_property(ctx, root_cell, "log_configured_graph_sketch")
                .await?,
            total_configured_graph_sketch: bool_property(
                ctx,
                root_cell,
                "log_total_configured_graph_sketch",
            )
            .await?,
            retained_analysis_memory_sketch: bool_property(
                ctx,
                root_cell,
                "log_retained_analysis_memory_sketch",
            )
            .await?,
            peak_analysis_memory_sketch: bool_property(
                ctx,
                root_cell,
                "log_peak_analysis_memory_sketch",
            )
            .await?,
            peak_load_memory_sketch: bool_property(ctx, root_cell, "log_peak_load_memory_sketch")
                .await?,
            action_graph_sketch: bool_property(ctx, root_cell, "log_action_graph_sketch").await?,
            artifact_count_sketch: bool_property(ctx, root_cell, "log_artifact_count_sketch")
                .await?,
            artifact_size_sketch: bool_property(ctx, root_cell, "log_artifact_size_sketch").await?,
            log_sketch_cardinalities: bool_property(ctx, root_cell, "log_sketch_cardinalities")
                .await?,
        };

        let providers_to_skip_in_artifact_path_sketch = ctx
            .parse_legacy_config_list_property::<SkipProvider>(
                root_cell,
                BuckconfigKeyRef {
                    section: "buck2",
                    property: "providers_to_skip_in_artifact_path_sketch",
                },
            )
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(SkipProvider::into_build_provider_type)
            .collect();

        Ok(Self {
            graph_properties,
            detailed_aggregated_metrics: bool_property(
                ctx,
                root_cell,
                "detailed_aggregated_metrics",
            )
            .await?,
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

async fn bool_property(
    ctx: &mut DiceComputations<'_>,
    root_cell: CellName,
    property: &'static str,
) -> buck2_error::Result<bool> {
    Ok(ctx
        .parse_legacy_config_property(
            root_cell,
            BuckconfigKeyRef {
                section: "buck2",
                property,
            },
        )
        .await?
        .unwrap_or_default())
}

enum SkipProvider {
    Build,
    Run,
    Test,
}

impl SkipProvider {
    fn into_build_provider_type(self) -> BuildProviderType {
        match self {
            Self::Build => BuildProviderType::Default,
            Self::Run => BuildProviderType::Run,
            Self::Test => BuildProviderType::Test,
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
            "build" => Ok(Self::Build),
            "run" => Ok(Self::Run),
            "test" => Ok(Self::Test),
            other => Err(SkipProviderParseError(other.to_owned())),
        }
    }
}
