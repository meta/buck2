/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp::max;
use std::sync::Arc;

use buck2_artifact::actions::key::ActionKey;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_data::ActionExecutionKind;
use buck2_data::ToProtoMessage;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_sketches::ActionGraphSketch;
use dupe::Dupe;

use crate::artifact_groups::ArtifactGroup;
use crate::build::BuildProviderType;
use crate::build::detailed_aggregated_metrics::buck2_sketches::ArtifactPathSketches;
use crate::build::sketch_impl::MergeableGraphSketch;

#[derive(Clone)]
pub struct ActionExecutionMetrics {
    pub key: ActionKey,
    pub execution_time_ms: u64,
    pub execution_kind: buck2_data::ActionExecutionKind,
    pub output_size_bytes: u64,
    pub memory_peak: Option<u64>,
    /// RE platform identifier if this action ran remotely. Format is the
    /// worker's `platform` property, optionally suffixed with `.subplatform`
    /// (e.g. `"linux-remote-execution"` or `"gpu-remote-execution.H100"`).
    pub re_platform_name: Option<String>,
}

pub struct AnalysisMetrics {
    pub actions: usize,
    pub retained_memory: usize,
}

#[derive(Clone)]
pub struct TopLevelTargetSpec {
    pub label: ConfiguredProvidersLabel,
    pub outputs: Arc<Vec<(ArtifactGroup, BuildProviderType)>>,
}

#[derive(Default)]
pub struct PerBuildEvents {
    pub executed_actions: buck2_hash::BuckMutSet<ActionKey>,
    pub top_level_targets: Vec<TopLevelTargetSpec>,
}

/// Full configured graphs are needed only by detailed metrics. Recording a
/// top-level build must not restore or retain them for the other consumers.
pub(crate) struct DetailedMetricsInput {
    pub executed_actions: buck2_hash::BuckMutSet<ActionKey>,
    pub top_level_targets: Vec<(TopLevelTargetSpec, ConfiguredTargetNode)>,
}

#[derive(Default)]
pub struct ActionGraphSketchResult {
    pub per_target_sketches: Vec<(
        ConfiguredProvidersLabel,
        Option<MergeableGraphSketch<ActionKey, ActionGraphSketch>>,
    )>,
}

pub struct ArtifactPathSketchResult {
    pub(crate) per_target_sketches: Vec<(ConfiguredProvidersLabel, ArtifactPathSketches)>,
}

#[derive(Default)]
pub struct DetailedAggregatedMetrics {
    pub top_level_target_metrics: Vec<TopLevelTargetAggregatedData>,
    pub all_targets_build_metrics: AllTargetsAggregatedData,
}

impl ToProtoMessage for DetailedAggregatedMetrics {
    type Message = buck2_data::DetailedAggregatedMetrics;

    fn as_proto(&self) -> Self::Message {
        buck2_data::DetailedAggregatedMetrics {
            top_level_target_metrics: self
                .top_level_target_metrics
                .iter()
                .map(|m| m.as_proto())
                .collect(),
            all_targets_build_metrics: Some(self.all_targets_build_metrics.as_proto()),
        }
    }
}

#[derive(Default)]
pub struct AggregatedBuildMetrics {
    pub full_graph_execution_time_ms: f64,
    pub full_graph_output_size_bytes: f64,

    pub local_execution_time_ms: f64,
    pub remote_execution_time_ms: f64,

    pub local_executions: f64,
    pub remote_executions: f64,
    pub remote_cache_hits: f64,

    pub analysis_retained_memory: f64,
    pub declared_actions: f64,
}

impl ToProtoMessage for AggregatedBuildMetrics {
    type Message = buck2_data::AggregatedBuildMetrics;

    fn as_proto(&self) -> Self::Message {
        buck2_data::AggregatedBuildMetrics {
            full_graph_execution_time_ms: self.full_graph_execution_time_ms,
            full_graph_output_size_bytes: self.full_graph_output_size_bytes,
            local_execution_time_ms: self.local_execution_time_ms,
            remote_execution_time_ms: self.remote_execution_time_ms,
            local_executions: self.local_executions,
            remote_executions: self.remote_executions,
            remote_cache_hits: self.remote_cache_hits,
            analysis_retained_memory: self.analysis_retained_memory,
            declared_actions: self.declared_actions,
        }
    }
}

pub struct TopLevelTargetAggregatedData {
    pub target: ConfiguredProvidersLabel,
    pub target_rule_type_name: String,
    pub action_graph_size: Option<u64>,
    pub metrics: AggregatedBuildMetrics,
    pub amortized_metrics: AggregatedBuildMetrics,
    pub remote_max_memory_peak_bytes: u64,
    pub local_max_memory_peak_bytes: u64,
    /// Distinct RE platform identifiers used by actions for this target. See
    /// `ActionExecutionMetrics::re_platform_name` for the value format.
    pub re_platform_names: Vec<String>,
    /// Wall-clock time in milliseconds from the start of the build at which
    /// this top-level target succeeded, failed, or timed out.
    /// The exact point at which the build is deemed to have start is set in the
    /// Buck2 daemon, so use this to compare durations but don't add it to a
    /// start timestamp to produce an end timestamp.
    pub wall_clock_completion_ms: Option<u64>,
}

#[derive(Clone, Copy, Dupe)]
pub enum BuiltWhen {
    ThisBuild,
    Previously,
}

impl TopLevelTargetAggregatedData {
    pub fn new(
        target: ConfiguredProvidersLabel,
        target_rule_type_name: String,
        action_graph_size: Option<usize>,
    ) -> Self {
        Self {
            target,
            target_rule_type_name,
            action_graph_size: action_graph_size.map(|v| v as u64),
            metrics: AggregatedBuildMetrics::default(),
            amortized_metrics: AggregatedBuildMetrics::default(),
            remote_max_memory_peak_bytes: 0,
            local_max_memory_peak_bytes: 0,
            re_platform_names: Vec::new(),
            wall_clock_completion_ms: None,
        }
    }

    pub fn aggregate_execution_event(
        &mut self,
        factor: usize,
        ev: &ActionExecutionMetrics,
        when: BuiltWhen,
    ) {
        let factor = 1.0 / (factor as f64);
        self.metrics.aggregate_execution(1.0, ev, when);
        self.amortized_metrics.aggregate_execution(factor, ev, when);

        if let Some(ref platform_name) = ev.re_platform_name {
            if !self.re_platform_names.contains(platform_name) {
                self.re_platform_names.push(platform_name.clone());
            }
        }
    }

    pub fn aggregate_analysis_event(&mut self, factor: usize, ev: &AnalysisMetrics) {
        let factor = 1.0 / (factor as f64);
        self.metrics.aggregate_analysis(1.0, ev);
        self.amortized_metrics.aggregate_analysis(factor, ev);
    }

    pub fn aggregate_max_memory(&mut self, ev: &ActionExecutionMetrics) {
        let Some(memory_peak) = ev.memory_peak else {
            return;
        };
        match ev.execution_kind {
            ActionExecutionKind::Local | ActionExecutionKind::LocalWorker => {
                self.local_max_memory_peak_bytes =
                    max(self.local_max_memory_peak_bytes, memory_peak);
            }
            ActionExecutionKind::Remote
            | ActionExecutionKind::RemoteWorker
            | ActionExecutionKind::RemoteDeduplicated => {
                self.remote_max_memory_peak_bytes =
                    max(self.remote_max_memory_peak_bytes, memory_peak);
            }
            ActionExecutionKind::NotSet
            | ActionExecutionKind::ActionCache
            | ActionExecutionKind::Simple
            | ActionExecutionKind::Deferred
            | ActionExecutionKind::LocalDepFile
            | ActionExecutionKind::RemoteDepFileCache
            | ActionExecutionKind::LocalActionCache
            | ActionExecutionKind::RemoteServedFromCache
            | ActionExecutionKind::InFlight => {
                // ignore
            }
        }
    }
}

impl ToProtoMessage for TopLevelTargetAggregatedData {
    type Message = buck2_data::TopLevelTargetMetrics;

    fn as_proto(&self) -> Self::Message {
        buck2_data::TopLevelTargetMetrics {
            target: Some(self.target.target().as_proto()),
            provider: match self.target.name() {
                ProvidersName::Default => None,
                v => Some(v.to_string()),
            },
            target_rule_type_name: self.target_rule_type_name.clone(),
            action_graph_size: self.action_graph_size,
            metrics: Some(self.metrics.as_proto()),
            amortized_metrics: Some(self.amortized_metrics.as_proto()),
            remote_max_memory_peak_bytes: Some(self.remote_max_memory_peak_bytes),
            local_max_memory_peak_bytes: Some(self.local_max_memory_peak_bytes),
            re_platform_names: self.re_platform_names.to_vec(),
            wall_clock_completion_ms: self.wall_clock_completion_ms,
        }
    }
}

impl AggregatedBuildMetrics {
    fn aggregate_execution(&mut self, factor: f64, ev: &ActionExecutionMetrics, when: BuiltWhen) {
        // Accumulate metrics computed over the full graph.
        self.full_graph_execution_time_ms += factor * (ev.execution_time_ms as f64);
        self.full_graph_output_size_bytes += factor * (ev.output_size_bytes as f64);

        if let BuiltWhen::ThisBuild = when {
            // Accumulate metrics associated with costs during this build.
            match ev.execution_kind {
                buck2_data::ActionExecutionKind::Local
                | buck2_data::ActionExecutionKind::LocalWorker => {
                    self.local_execution_time_ms += factor * (ev.execution_time_ms as f64);
                    self.local_executions += factor;
                }
                buck2_data::ActionExecutionKind::Remote
                | buck2_data::ActionExecutionKind::RemoteWorker
                | buck2_data::ActionExecutionKind::RemoteDeduplicated => {
                    self.remote_execution_time_ms += factor * (ev.execution_time_ms as f64);
                    self.remote_executions += factor;
                }
                buck2_data::ActionExecutionKind::ActionCache
                | buck2_data::ActionExecutionKind::RemoteDepFileCache
                | buck2_data::ActionExecutionKind::RemoteServedFromCache => {
                    self.remote_cache_hits += factor;
                }
                buck2_data::ActionExecutionKind::NotSet
                | buck2_data::ActionExecutionKind::Simple
                | buck2_data::ActionExecutionKind::Deferred
                | buck2_data::ActionExecutionKind::LocalDepFile
                | buck2_data::ActionExecutionKind::LocalActionCache
                | buck2_data::ActionExecutionKind::InFlight => {
                    // ignored.
                }
            }
        }
    }

    fn aggregate_analysis(&mut self, factor: f64, ev: &AnalysisMetrics) {
        self.analysis_retained_memory += factor * (ev.retained_memory as f64);
        self.declared_actions += factor * (ev.actions as f64);
    }
}

#[derive(Default)]
pub struct AllTargetsAggregatedData {
    pub metrics: AggregatedBuildMetrics,
    pub action_graph_size: Option<u64>,
    pub compute_time_ms: Option<u64>,
}

impl AllTargetsAggregatedData {
    pub fn new(action_graph_size: Option<usize>) -> Self {
        Self {
            metrics: AggregatedBuildMetrics::default(),
            action_graph_size: action_graph_size.map(|v| v as u64),
            compute_time_ms: None,
        }
    }

    pub(crate) fn aggregate_execution_event(
        &mut self,
        ev: &ActionExecutionMetrics,
        built_when: BuiltWhen,
    ) {
        self.metrics.aggregate_execution(1.0, ev, built_when);
    }

    pub(crate) fn aggregate_analysis_event(&mut self, ev: &AnalysisMetrics) {
        self.metrics.aggregate_analysis(1.0, ev);
    }

    pub(crate) fn set_compute_time(&mut self, elapsed: std::time::Duration) {
        self.compute_time_ms = Some(elapsed.as_millis() as u64);
    }
}

impl ToProtoMessage for AllTargetsAggregatedData {
    type Message = buck2_data::AllTargetsBuildMetrics;

    fn as_proto(&self) -> Self::Message {
        buck2_data::AllTargetsBuildMetrics {
            action_graph_size: self.action_graph_size,
            metrics: Some(self.metrics.as_proto()),
            compute_time_ms: self.compute_time_ms,
        }
    }
}
