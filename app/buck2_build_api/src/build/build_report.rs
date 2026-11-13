/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Processing and reporting the results of the build

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::BufWriter;
use std::io::Write;
use std::sync::Arc;

use buck2_artifact::artifact::artifact_dump::ArtifactInfo;
use buck2_artifact::artifact::artifact_dump::DirectoryInfo;
use buck2_artifact::artifact::artifact_dump::ExternalSymlinkInfo;
use buck2_artifact::artifact::artifact_dump::FileInfo;
use buck2_artifact::artifact::artifact_dump::SymlinkInfo;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_cli_proto::CommonBuildOptions;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::cells::CellResolver;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::pattern::pattern::Modifiers;
use buck2_core::pattern::pattern::TargetLabelWithModifiers;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::NonDefaultProvidersName;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_data::ErrorReport;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_error::BuckErrorContext;
use buck2_error::UniqueRootId;
use buck2_error::classify::ErrorLike;
use buck2_error::classify::Tier;
use buck2_error::classify::best_error;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_sketches::DependencyGraphSketch;
use buck2_wrapper_common::invocation_id::TraceId;
use derivative::Derivative;
use dice::DiceComputations;
use dice::DiceTransaction;
use dupe::Dupe;
use dupe::OptionDupedExt;
use itertools::Either;
use itertools::EitherOrBoth;
use itertools::Itertools;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::actions::artifact::get_artifact_fs::GetArtifactFs;
use crate::build::BuildProviderType;
use crate::build::BuildTargetResult;
use crate::build::ConfiguredBuildTargetResult;
use crate::build::FinishedBuilds;
use crate::build::action_error::ActionErrorBuildOptions;
use crate::build::action_error::BuildReportActionError;
use crate::build::action_error::MAX_ERROR_CONTENT_BYTES;
use crate::build::detailed_aggregated_metrics::types::ActionGraphSketchResult;
use crate::build::detailed_aggregated_metrics::types::AllTargetsAggregatedData;
use crate::build::detailed_aggregated_metrics::types::ArtifactPathSketchResult;
use crate::build::detailed_aggregated_metrics::types::DetailedAggregatedMetrics;
use crate::build::detailed_aggregated_metrics::types::TopLevelTargetAggregatedData;
use crate::build::graph_properties::GraphPropertiesOptions;
use crate::build::sketch_impl::DEFAULT_SKETCH_VERSION;
use crate::build::sketch_impl::VersionedSketcher;
use crate::bxl::types::BxlFunctionLabel;

#[derive(Debug, Serialize)]
#[allow(clippy::upper_case_acronyms)] // We care about how they serialise
#[derive(Default)]
enum BuildOutcome {
    #[default]
    SUCCESS,
    FAIL,
    #[allow(dead_code)] // Part of the spec, but not yet used
    CANCELED,
    /// Not finished building yet. Only in streaming build report lines.
    #[serde(rename = "IN_PROGRESS")]
    InProgress,
}

impl BuildOutcome {
    /// Until an entry has finished, `SUCCESS` only means that no error has arrived yet.
    fn in_progress_unless(&mut self, finished: bool) {
        if !finished && matches!(self, Self::SUCCESS) {
            *self = Self::InProgress;
        }
    }
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
///
/// Serialization of the report must be deterministic, so any map reachable from here has to
/// iterate in a deterministic order - `BTreeMap`, not `BuckMutMap`.
#[derive(Debug, Serialize)]
pub struct BuildReport {
    trace_id: TraceId,
    success: bool,
    results: BTreeMap<EntryLabel, BuildReportEntry>,
    /// filled only when fill-out-failures is passed for Buck1 backcompat only
    failures: BTreeMap<EntryLabel, String>,
    project_root: AbsNormPathBuf,
    truncated: bool,
    strings: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Build metrics aggregated across all targets.
    build_metrics: Option<AllTargetsBuildMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_configured_graph_sketch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_category: Option<String>,
}

/// The fields that stored in the unconfigured `BuildReportEntry` for buck1 backcompat.
///
/// Do not put new fields in here. Put them in `ConfiguredBuildReportEntry`
#[derive(Default, Debug, Serialize)]
struct MaybeConfiguredBuildReportEntry {
    /// whether this particular target was successful
    success: BuildOutcome,
    /// a map of each subtarget of the current target (outputted as a `|` delimited list) to
    /// the default exposed output of the subtarget
    outputs: BTreeMap<Arc<str>, BuckMutInsertionSet<ProjectRelativePathBuf>>,
    /// a map of each subtarget of the current target (outputted as a `|` delimited list) to
    /// the hidden, implicitly built outputs of the subtarget. There are multiple outputs
    /// per subtarget
    ///
    /// FIXME(JakobDegen): This should be in `ConfiguredBuildReportEntry`
    other_outputs: BTreeMap<Arc<str>, BuckMutInsertionSet<ProjectRelativePathBuf>>,
    /// The size of the graph for this target, if it was produced
    ///
    /// FIXME(JakobDegen): This should be in `ConfiguredBuildReportEntry`
    configured_graph_size: Option<u64>,
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Default, Debug, Serialize)]
pub(crate) struct ConfiguredBuildReportEntry {
    /// A list of errors that occurred while building this target
    errors: Vec<BuildReportError>,
    /// Remote artifact information, including hashes, etc.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    artifact_info: BTreeMap<Arc<str>, ArtifactInfo>,
    #[serde(flatten)]
    inner: MaybeConfiguredBuildReportEntry,
    /// The serialized graph sketch for this target, if it was produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    configured_graph_sketch: Option<String>,
    /// Build metrics for this target.
    #[serde(skip_serializing_if = "Option::is_none")]
    build_metrics: Option<Arc<TargetBuildMetrics>>,
    /// A sketch of the analysis memory used by this target
    #[serde(skip_serializing_if = "Option::is_none")]
    retained_analysis_memory_sketch: Option<String>,
    /// A sketch of peak memory usage during analysis for this target
    #[serde(skip_serializing_if = "Option::is_none")]
    peak_analysis_memory_sketch: Option<String>,
    /// A sketch of peak memory usage during BUCK file loading across transitive packages
    #[serde(skip_serializing_if = "Option::is_none")]
    peak_load_memory_sketch: Option<String>,
    /// A sketch of the action graph for this target
    #[serde(skip_serializing_if = "Option::is_none")]
    action_graph_sketch: Option<String>,
    /// A sketch of artifact counts for this target
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_count_sketch: Option<String>,
    /// A sketch of artifact sizes for this target
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_size_sketch: Option<String>,
    /// Estimated cardinality of `artifact_count_sketch`. Populated only when the
    /// `buck2.log_sketch_cardinalities` buckconfig is set; the corresponding
    /// sketch field is left intact in both cases.
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_count_sketch_cardinality: Option<f64>,
    /// Estimated cardinality of `artifact_size_sketch`. Populated only when the
    /// `buck2.log_sketch_cardinalities` buckconfig is set; the corresponding
    /// sketch field is left intact in both cases.
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_size_sketch_cardinality: Option<f64>,
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Default, Debug, Serialize)]
pub(crate) struct TargetBuildMetrics {
    /// The total number of nodes in the action graph, if we were able to fully
    /// traverse it.
    pub action_graph_size: Option<u64>,
    /// These are metrics aggregated without normalization.
    pub metrics: AggregatedBuildMetrics,
    /// "Amortized" metrics are aggregated by dividing the metric/cost evenly
    /// across all top-level targets that require the node that produced the
    /// metric.
    pub amortized_metrics: AggregatedBuildMetrics,
    /// Max value for peak memory usage across all remote actions.
    pub remote_max_memory_peak_bytes: u64,
    /// Max value for peak memory usage across all local actions.
    pub local_max_memory_peak_bytes: u64,
    /// Distinct RE platform identifiers used by actions for this target. Each
    /// value is the worker's `platform` property, optionally suffixed with
    /// `.subplatform` (e.g. `gpu-remote-execution.H100`) when set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub re_platform_names: Vec<String>,
    /// Wall-clock time in milliseconds from the start of the build at which
    /// this top-level target succeeded, failed, or timed out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_clock_completion_ms: Option<u64>,
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Default, Debug, Serialize)]
pub(crate) struct AggregatedBuildMetrics {
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

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Default, Debug, Serialize)]
pub(crate) struct AllTargetsBuildMetrics {
    pub action_graph_size: Option<u64>,
    pub metrics: AggregatedBuildMetrics,
    pub compute_time_ms: Option<u64>,
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Debug, Serialize)]
struct BuildReportEntry {
    /// The buck1 build report did not support multiple configurations of the same target. We
    /// do, which is why we have the `configured` field below, which users should ideally use.
    /// This field is kept around for buck1 compatibility only and should ideally be removed.
    ///
    /// We avoid the `WithErrors` variant here, to keep the errors field from conflicting with
    /// the one on this struct.
    #[serde(flatten)]
    #[serde(skip_serializing_if = "Option::is_none")]
    compatible: Option<MaybeConfiguredBuildReportEntry>,

    /// the configured entry
    configured: BTreeMap<ConfigurationData, ConfiguredBuildReportEntry>,

    /// Errors that could not be associated with a particular configured version of the target,
    /// typically because they happened before configuration.
    errors: Vec<BuildReportError>,

    /// The path to the package where this target is defined, relative to the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    package_project_relative_path: Option<ProjectRelativePathBuf>,
}

/// DO NOT UPDATE WITHOUT UPDATING `docs/users/build_observability/build_report.md`!
#[derive(Debug, Clone, Serialize, PartialOrd, Ord, PartialEq, Eq)]
struct BuildReportError {
    message_content: String,
    action_error: Option<BuildReportActionError>,
    error_tags: Vec<String>,
    /// An opaque index that can be use to de-duplicate errors. Two errors with the same
    /// cause index have the same cause
    ///
    /// For example, two targets in different packages may have the same cause (evaluation of
    /// common bzl file), but error stack will be different.
    cause_index: usize,
    error_category: String,
    /// The same key buck2 logs as `category_key` in its error telemetry.
    category_key: String,
}

#[derive(Derivative, Serialize, Eq, PartialEq, Ord, PartialOrd, Hash, Clone)]
#[derivative(Debug)]
#[serde(untagged)]
enum EntryLabel {
    #[derivative(Debug = "transparent")]
    Target(TargetLabelWithModifiers),
    #[derivative(Debug = "transparent")]
    BxlFunction(BxlFunctionLabel),
}

pub struct BuildReportOpts {
    pub print_unconfigured_section: bool,
    pub unstable_include_failures_build_report: bool,
    pub unstable_include_package_project_relative_paths: bool,
    pub unstable_include_artifact_hash_information: bool,
    pub unstable_build_report_filename: String,
    pub graph_properties_opts: GraphPropertiesOptions,
    pub unstable_streaming_build_report_filename: String,
    pub unstable_exclude_action_error_diagnostics: bool,
    pub unstable_truncate_error_content: bool,
}

pub struct BuildReportCollector<'a> {
    artifact_fs: &'a ArtifactFs,
    cell_resolver: &'a CellResolver,
    overall_success: bool,
    include_unconfigured_section: bool,
    error_cause_cache: BuckMutMap<buck2_error::UniqueRootId, usize>,
    next_cause_index: usize,
    strings: BTreeMap<String, String>,
    failures: BTreeMap<EntryLabel, String>,
    include_failures: bool,
    include_package_project_relative_paths: bool,
    include_artifact_hash_information: bool,
    exclude_action_error_diagnostics: bool,
    truncate_error_content: bool,
    graph_properties_opts: GraphPropertiesOptions,
    total_configured_graph_sketch:
        Option<VersionedSketcher<ConfiguredTargetLabel, DependencyGraphSketch>>,
    /// Set on every streaming line, never on the final report. Entries not in it read
    /// `IN_PROGRESS` rather than `SUCCESS`.
    finished_builds: Option<&'a FinishedBuilds>,
}

// Build report generation should never produce an input error, always return an error with an infra tag
#[derive(buck2_error::Error)]
#[error(transparent)]
#[buck2(tag = BuildReport)]
pub struct BuildReportGenerationError(buck2_error::Error);

impl From<buck2_error::Error> for BuildReportGenerationError {
    fn from(e: buck2_error::Error) -> Self {
        Self(e)
    }
}

impl<'a> BuildReportCollector<'a> {
    // ============================================================================
    // Main conversion functions for build reports
    // ============================================================================

    /// Creates a new BuildReportCollector with the given configuration
    fn new(
        artifact_fs: &'a ArtifactFs,
        cell_resolver: &'a CellResolver,
        include_unconfigured_section: bool,
        include_failures: bool,
        include_package_project_relative_paths: bool,
        include_artifact_hash_information: bool,
        exclude_action_error_diagnostics: bool,
        truncate_error_content: bool,
        graph_properties_opts: GraphPropertiesOptions,
    ) -> Self {
        Self {
            artifact_fs,
            cell_resolver,
            overall_success: true,
            include_unconfigured_section,
            error_cause_cache: BuckMutMap::default(),
            next_cause_index: 0,
            strings: BTreeMap::default(),
            failures: BTreeMap::default(),
            include_failures,
            include_package_project_relative_paths,
            include_artifact_hash_information,
            exclude_action_error_diagnostics,
            truncate_error_content,
            graph_properties_opts,
            total_configured_graph_sketch: if graph_properties_opts.total_configured_graph_sketch {
                Some(DEFAULT_SKETCH_VERSION.create_sketcher())
            } else {
                None
            },
            finished_builds: None,
        }
    }

    /// Converts build results and errors into a BuildReport for regular builds.
    /// This processes both configured build targets and associated errors.
    pub(crate) fn convert(
        trace_id: &TraceId,
        artifact_fs: &'a ArtifactFs,
        cell_resolver: &'a CellResolver,
        project_root: &ProjectRoot,
        include_unconfigured_section: bool,
        include_failures: bool,
        include_package_project_relative_paths: bool,
        include_artifact_hash_information: bool,
        exclude_action_error_diagnostics: bool,
        truncate_error_content: bool,
        configured: &BTreeMap<ConfiguredProvidersLabel, Option<ConfiguredBuildTargetResult>>,
        configured_to_pattern_modifiers: &BuckMutMap<ConfiguredProvidersLabel, BTreeSet<Modifiers>>,
        other_errors: &BTreeMap<Option<ProvidersLabel>, Vec<buck2_error::Error>>,
        finished_builds: Option<&'a FinishedBuilds>,
        detailed_metrics: Option<DetailedAggregatedMetrics>,
        action_graph_sketch_result: Option<ActionGraphSketchResult>,
        artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
        graph_properties_opts: GraphPropertiesOptions,
    ) -> Result<BuildReport, BuildReportGenerationError> {
        let mut this = Self::new(
            artifact_fs,
            cell_resolver,
            include_unconfigured_section,
            include_failures,
            include_package_project_relative_paths,
            include_artifact_hash_information,
            exclude_action_error_diagnostics,
            truncate_error_content,
            graph_properties_opts,
        );
        this.finished_builds = finished_builds;
        let mut entries = BTreeMap::new();

        if other_errors.values().flatten().next().is_some() {
            // Do this check ahead of time. We don't check for errors that aren't associated
            // with a target below, so we'd miss this otherwise.
            this.overall_success = false;
        }

        let mut metrics_by_configured: BuckMutMap<
            ConfiguredProvidersLabel,
            Arc<TargetBuildMetrics>,
        > = BuckMutMap::default();
        let mut action_graph_sketches_by_configured: BuckMutMap<ConfiguredProvidersLabel, String> =
            BuckMutMap::default();
        let mut artifact_count_sketches_by_configured: BuckMutMap<
            ConfiguredProvidersLabel,
            String,
        > = BuckMutMap::default();
        let mut artifact_size_sketches_by_configured: BuckMutMap<ConfiguredProvidersLabel, String> =
            BuckMutMap::default();
        let mut artifact_count_cardinalities_by_configured: BuckMutMap<
            ConfiguredProvidersLabel,
            f64,
        > = BuckMutMap::default();
        let mut artifact_size_cardinalities_by_configured: BuckMutMap<
            ConfiguredProvidersLabel,
            f64,
        > = BuckMutMap::default();
        if let Some(detailed_metrics) = detailed_metrics.as_ref() {
            for top_level_metrics in &detailed_metrics.top_level_target_metrics {
                metrics_by_configured.insert(
                    top_level_metrics.target.clone(),
                    Self::convert_per_target_metrics(top_level_metrics).into(),
                );
            }
        }
        if let Some(sketch_result) = action_graph_sketch_result.as_ref() {
            for (label, sketch) in &sketch_result.per_target_sketches {
                if let Some(sketch) = sketch {
                    if !sketch.is_empty() {
                        action_graph_sketches_by_configured
                            .insert(label.clone(), sketch.serialize());
                    }
                }
            }
        }
        let log_sketch_cardinalities = graph_properties_opts.log_sketch_cardinalities;
        if let Some(sketch_result) = artifact_path_sketch_result.as_ref() {
            for (label, sketches) in &sketch_result.per_target_sketches {
                if let Some(count_sketch) = sketches.count.as_ref() {
                    if !count_sketch.is_empty() {
                        artifact_count_sketches_by_configured
                            .insert(label.clone(), count_sketch.serialize());
                        if log_sketch_cardinalities {
                            artifact_count_cardinalities_by_configured
                                .insert(label.clone(), count_sketch.estimated_cardinality());
                        }
                    }
                }
                if let Some(size_sketch) = sketches.size.as_ref() {
                    if !size_sketch.is_empty() {
                        artifact_size_sketches_by_configured
                            .insert(label.clone(), size_sketch.serialize());
                        if log_sketch_cardinalities {
                            artifact_size_cardinalities_by_configured
                                .insert(label.clone(), size_sketch.estimated_cardinality());
                        }
                    }
                }
            }
        }

        // Collect all ErrorReports to find the best (most severe) one
        let mut all_error_reports = Vec::new();

        // The `BuildTargetResult` doesn't group errors by their unconfigured target, so we need
        // to do a little iterator munging to achieve that ourselves
        let results_by_unconfigured = configured
            .iter()
            .chunk_by(|x| x.0.target().unconfigured().dupe());
        let errors_by_unconfigured = other_errors
            .iter()
            .filter_map(|(l, e)| Some((l.as_ref()?.target().dupe(), e)));

        for i in Itertools::merge_join_by(
            IntoIterator::into_iter(&results_by_unconfigured),
            errors_by_unconfigured,
            |(l1, _), (l2, _)| Ord::cmp(l1, l2),
        ) {
            let (label, results, errors) = match i {
                EitherOrBoth::Both((label, results), (_, errors)) => {
                    (label, Either::Left(results), &**errors)
                }
                EitherOrBoth::Left((label, results)) => (label, Either::Left(results), &[][..]),
                EitherOrBoth::Right((label, errors)) => {
                    (label, Either::Right(std::iter::empty()), &**errors)
                }
            };

            // Group results by Modifiers
            let mut results_by_modifiers: BTreeMap<
                Modifiers,
                Vec<(
                    &ConfiguredProvidersLabel,
                    &Option<ConfiguredBuildTargetResult>,
                )>,
            > = BTreeMap::new();

            for (configured_label, result) in results {
                let modifiers_vec = configured_to_pattern_modifiers.get(configured_label);

                if let Some(modifiers_vec) = modifiers_vec {
                    for modifier in modifiers_vec {
                        results_by_modifiers
                            .entry(modifier.dupe())
                            .or_default()
                            .push((configured_label, result));
                    }
                } else {
                    results_by_modifiers
                        .entry(Modifiers::new(None))
                        .or_default()
                        .push((configured_label, result));
                }
            }

            // If the mapping is empty, results were empty, indicating the EitherOrBoth::Right case.
            // We still need to create an entry for the error.
            if results_by_modifiers.is_empty() {
                results_by_modifiers.insert(Modifiers::new(None), Vec::new());
            }

            all_error_reports.extend(errors.iter().map(ErrorReport::from));

            for (modifiers, modifiers_results) in results_by_modifiers {
                let target_with_modifiers = TargetLabelWithModifiers {
                    target_label: label.dupe(),
                    modifiers,
                };

                let entry = this.collect_results_for_unconfigured(
                    target_with_modifiers.dupe(),
                    modifiers_results,
                    errors,
                    &mut metrics_by_configured,
                    &action_graph_sketches_by_configured,
                    &artifact_count_sketches_by_configured,
                    &artifact_size_sketches_by_configured,
                    &artifact_count_cardinalities_by_configured,
                    &artifact_size_cardinalities_by_configured,
                    &mut all_error_reports,
                )?;

                entries.insert(EntryLabel::Target(target_with_modifiers), entry);
            }
        }
        this.assemble_build_report(
            trace_id,
            project_root,
            entries,
            all_error_reports,
            detailed_metrics,
        )
    }

    /// Converts BXL errors into a BuildReport for BXL's ensure artifacts errors.
    /// This processes errors associated with a single BXL function execution.
    pub(crate) fn convert_bxl(
        trace_id: &TraceId,
        artifact_fs: &'a ArtifactFs,
        cell_resolver: &'a CellResolver,
        project_root: &ProjectRoot,
        include_unconfigured_section: bool,
        include_failures: bool,
        include_package_project_relative_paths: bool,
        include_artifact_hash_information: bool,
        exclude_action_error_diagnostics: bool,
        truncate_error_content: bool,
        bxl_label: &BxlFunctionLabel,
        errors: &[buck2_error::Error],
        detailed_metrics: Option<DetailedAggregatedMetrics>,
        _action_graph_sketch_result: Option<ActionGraphSketchResult>,
        _artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
        graph_properties_opts: GraphPropertiesOptions,
    ) -> Result<BuildReport, BuildReportGenerationError> {
        let mut this = Self::new(
            artifact_fs,
            cell_resolver,
            include_unconfigured_section,
            include_failures,
            include_package_project_relative_paths,
            include_artifact_hash_information,
            exclude_action_error_diagnostics,
            truncate_error_content,
            graph_properties_opts,
        );
        let mut entries = BTreeMap::new();
        let all_error_reports: Vec<ErrorReport> = errors.iter().map(ErrorReport::from).collect();

        if !errors.is_empty() {
            this.overall_success = false;
        }

        // Create entry for the BXL function
        let entry_label = EntryLabel::BxlFunction(bxl_label.clone());
        let errors = this.convert_error_list(errors, entry_label.clone());

        let entry = BuildReportEntry {
            compatible: None,
            configured: BTreeMap::new(),
            errors,
            package_project_relative_path: None, // Package doesn't apply to BXL
        };

        entries.insert(entry_label, entry);

        this.assemble_build_report(
            trace_id,
            project_root,
            entries,
            all_error_reports,
            detailed_metrics,
        )
    }

    /// Assembles the BuildReport from collected data
    fn assemble_build_report(
        self,
        trace_id: &TraceId,
        project_root: &ProjectRoot,
        entries: BTreeMap<EntryLabel, BuildReportEntry>,
        all_error_reports: Vec<ErrorReport>,
        detailed_metrics: Option<DetailedAggregatedMetrics>,
    ) -> Result<BuildReport, BuildReportGenerationError> {
        let total_configured_graph_sketch = self
            .total_configured_graph_sketch
            .map(|sketcher| sketcher.into_mergeable_graph_sketch().serialize());

        // Determine error category using existing Buck2 error classification
        let error_category = if let Some(best_error_report) = best_error(&all_error_reports) {
            Some(best_error_report.category().to_string())
        } else if self.overall_success {
            None // No errors = no error category
        } else {
            Some(Tier::Tier0.to_string()) // Default when build failed but no specific error tracked
        };

        Ok(BuildReport {
            trace_id: trace_id.dupe(),
            success: self.overall_success,
            results: entries,
            failures: self.failures,
            project_root: project_root.root().to_owned(),
            // In buck1 we may truncate build report for a large number of targets.
            // Setting this to false since we don't currently truncate buck2's build report.
            truncated: false,
            strings: self.strings,
            build_metrics: detailed_metrics
                .map(|m| Self::convert_all_target_build_metrics(&m.all_targets_build_metrics)),
            total_configured_graph_sketch,
            error_category,
        })
    }

    // ============================================================================
    // Metrics conversion helpers
    // ============================================================================

    fn convert_all_target_build_metrics(
        all_target_metrics: &AllTargetsAggregatedData,
    ) -> AllTargetsBuildMetrics {
        AllTargetsBuildMetrics {
            action_graph_size: all_target_metrics.action_graph_size,
            metrics: Self::convert_aggregated_build_metrics(&all_target_metrics.metrics),
            compute_time_ms: all_target_metrics.compute_time_ms,
        }
    }

    fn convert_per_target_metrics(metrics: &TopLevelTargetAggregatedData) -> TargetBuildMetrics {
        TargetBuildMetrics {
            action_graph_size: metrics.action_graph_size,
            metrics: Self::convert_aggregated_build_metrics(&metrics.metrics),
            amortized_metrics: Self::convert_aggregated_build_metrics(&metrics.amortized_metrics),
            remote_max_memory_peak_bytes: metrics.remote_max_memory_peak_bytes,
            local_max_memory_peak_bytes: metrics.local_max_memory_peak_bytes,
            re_platform_names: metrics.re_platform_names.to_vec(),
            wall_clock_completion_ms: metrics.wall_clock_completion_ms,
        }
    }

    fn convert_aggregated_build_metrics(
        metrics: &crate::build::detailed_aggregated_metrics::types::AggregatedBuildMetrics,
    ) -> AggregatedBuildMetrics {
        AggregatedBuildMetrics {
            full_graph_execution_time_ms: metrics.full_graph_execution_time_ms,
            full_graph_output_size_bytes: metrics.full_graph_output_size_bytes,
            local_execution_time_ms: metrics.local_execution_time_ms,
            remote_execution_time_ms: metrics.remote_execution_time_ms,
            local_executions: metrics.local_executions,
            remote_executions: metrics.remote_executions,
            remote_cache_hits: metrics.remote_cache_hits,
            analysis_retained_memory: metrics.analysis_retained_memory,
            declared_actions: metrics.declared_actions,
        }
    }

    // ============================================================================
    // Result collection and processing
    // ============================================================================

    pub(crate) fn update_string_cache(&mut self, string: String) -> String {
        let mut hasher = DefaultHasher::new();
        string.hash(&mut hasher);
        let hash = hasher.finish().to_string();
        self.strings.insert(hash.clone(), string);
        hash
    }

    /// Always called for one unconfigured target at a time
    fn collect_results_for_unconfigured<'b>(
        &mut self,
        target_with_modifiers: TargetLabelWithModifiers,
        results: impl IntoIterator<
            Item = (
                &'b ConfiguredProvidersLabel,
                &'b Option<ConfiguredBuildTargetResult>,
            ),
        >,
        errors: &[buck2_error::Error],
        metrics: &mut BuckMutMap<ConfiguredProvidersLabel, Arc<TargetBuildMetrics>>,
        action_graph_sketches: &BuckMutMap<ConfiguredProvidersLabel, String>,
        artifact_count_sketches: &BuckMutMap<ConfiguredProvidersLabel, String>,
        artifact_size_sketches: &BuckMutMap<ConfiguredProvidersLabel, String>,
        artifact_count_cardinalities: &BuckMutMap<ConfiguredProvidersLabel, f64>,
        artifact_size_cardinalities: &BuckMutMap<ConfiguredProvidersLabel, f64>,
        all_error_reports: &mut Vec<ErrorReport>,
    ) -> buck2_error::Result<BuildReportEntry> {
        // NOTE: if we're actually building a thing, then the package path must exist, but be
        // conservative and don't crash the overall processing if that happens.
        let package_project_relative_path = if self.include_package_project_relative_paths {
            self.cell_resolver
                .resolve_path(target_with_modifiers.target_label.pkg().as_cell_path())
                .ok()
        } else {
            None
        };

        let mut unconfigured_report = if self.include_unconfigured_section {
            Some(MaybeConfiguredBuildReportEntry::default())
        } else {
            None
        };
        let mut configured_reports = BTreeMap::new();

        for (label, results) in &results
            .into_iter()
            // We omit skipped targets here.
            .filter_map(|(label, result)| Some((label, result.as_ref()?)))
            .chunk_by(|x| x.0.target().dupe())
        {
            let mut configured_report = self.collect_results_for_configured(
                target_with_modifiers.dupe(),
                results,
                metrics,
                action_graph_sketches,
                artifact_count_sketches,
                artifact_size_sketches,
                artifact_count_cardinalities,
                artifact_size_cardinalities,
                all_error_reports,
            )?;
            if let Some(finished) = self.finished_builds {
                let key = (label.dupe(), target_with_modifiers.modifiers.dupe());
                configured_report
                    .inner
                    .success
                    .in_progress_unless(finished.configured_targets.contains(&key));
            }

            if let Some(report) = unconfigured_report.as_mut() {
                if !configured_report.errors.is_empty() {
                    report.success = BuildOutcome::FAIL;
                }

                // FIXME(JakobDegen): This potentially overwrites entries from other
                // configurations. Is that intended? Send a diff with a comment if you know
                report.outputs.extend(
                    configured_report
                        .inner
                        .outputs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
                report.other_outputs.extend(
                    configured_report
                        .inner
                        .other_outputs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
                if let Some(configured_graph_size) = configured_report.inner.configured_graph_size {
                    report.configured_graph_size = Some(configured_graph_size);
                }
            }

            configured_reports.insert(label.cfg().dupe(), configured_report);
        }

        if let Some(finished) = self.finished_builds
            && let Some(report) = unconfigured_report.as_mut()
        {
            report
                .success
                .in_progress_unless(finished.targets.contains(&target_with_modifiers));
        }
        let errors = self.convert_error_list(errors, EntryLabel::Target(target_with_modifiers));
        if !errors.is_empty() {
            if let Some(report) = unconfigured_report.as_mut() {
                report.success = BuildOutcome::FAIL;
            }
        }

        Ok(BuildReportEntry {
            compatible: unconfigured_report,
            configured: configured_reports,
            errors,
            package_project_relative_path,
        })
    }

    fn collect_results_for_configured<'b>(
        &mut self,
        target_with_modifiers: TargetLabelWithModifiers,
        results: impl IntoIterator<
            Item = (
                &'b ConfiguredProvidersLabel,
                &'b ConfiguredBuildTargetResult,
            ),
        >,
        metrics: &mut BuckMutMap<ConfiguredProvidersLabel, Arc<TargetBuildMetrics>>,
        action_graph_sketches: &BuckMutMap<ConfiguredProvidersLabel, String>,
        artifact_count_sketches: &BuckMutMap<ConfiguredProvidersLabel, String>,
        artifact_size_sketches: &BuckMutMap<ConfiguredProvidersLabel, String>,
        artifact_count_cardinalities: &BuckMutMap<ConfiguredProvidersLabel, f64>,
        artifact_size_cardinalities: &BuckMutMap<ConfiguredProvidersLabel, f64>,
        all_error_reports: &mut Vec<ErrorReport>,
    ) -> buck2_error::Result<ConfiguredBuildReportEntry> {
        let mut configured_report = ConfiguredBuildReportEntry::default();
        let mut errors = Vec::new();
        for (label, result) in results {
            let provider_name: Arc<str> = report_providers_name(label).into();

            result.outputs.iter().for_each(|timed| match &timed.inner {
                Ok(artifacts) => {
                    if artifacts.provider_type == BuildProviderType::Default {
                        for (artifact, value) in artifacts.values.iter() {
                            if self.include_artifact_hash_information {
                                update_artifact_info(
                                    &mut configured_report.artifact_info,
                                    provider_name.dupe(),
                                    value.entry(),
                                );
                            }
                            configured_report
                                .inner
                                .outputs
                                .entry(provider_name.dupe())
                                .or_default()
                                .insert({
                                    artifact
                                        .resolve_configuration_hash_path(self.artifact_fs)
                                        .unwrap()
                                });
                        }
                    }
                }
                Err(e) => {
                    errors.push(e.dupe());
                    // Collect errors into all_error_reports for global error categorization
                    all_error_reports.push(ErrorReport::from(e));
                }
            });

            errors.extend(result.errors.iter().map(|t| t.inner.clone()));
            // Collect result errors into all_error_reports for global error categorization
            all_error_reports.extend(errors.iter().map(ErrorReport::from));

            if let Some(Ok(MaybeCompatible::Compatible(graph_properties))) =
                &result.graph_properties
            {
                configured_report.inner.configured_graph_size =
                    Some(graph_properties.configured.configured_graph_size);

                if let Some(configured_graph_sketch) =
                    graph_properties.configured.configured_graph_sketch.as_ref()
                {
                    if self.graph_properties_opts.configured_graph_sketch {
                        configured_report.configured_graph_sketch =
                            Some(configured_graph_sketch.serialize());
                    }

                    if let Some(sketcher) = self.total_configured_graph_sketch.as_mut() {
                        sketcher.merge(configured_graph_sketch)?;
                    }
                }

                if let Some(retained_analysis_memory_sketch) =
                    graph_properties.retained_analysis_memory_sketch.as_ref()
                    && self.graph_properties_opts.retained_analysis_memory_sketch
                {
                    configured_report.retained_analysis_memory_sketch =
                        Some(retained_analysis_memory_sketch.serialize());
                }

                if let Some(peak_analysis_memory_sketch) =
                    graph_properties.peak_analysis_memory_sketch.as_ref()
                    && self.graph_properties_opts.peak_analysis_memory_sketch
                {
                    configured_report.peak_analysis_memory_sketch =
                        Some(peak_analysis_memory_sketch.serialize());
                }

                if let Some(peak_load_memory_sketch) =
                    graph_properties.peak_load_memory_sketch.as_ref()
                    && self.graph_properties_opts.peak_load_memory_sketch
                {
                    configured_report.peak_load_memory_sketch =
                        Some(peak_load_memory_sketch.serialize());
                }
            }

            configured_report.build_metrics = metrics.get(label).duped();
            configured_report.action_graph_sketch = action_graph_sketches.get(label).cloned();
            configured_report.artifact_count_sketch = artifact_count_sketches.get(label).cloned();
            configured_report.artifact_size_sketch = artifact_size_sketches.get(label).cloned();
            configured_report.artifact_count_sketch_cardinality =
                artifact_count_cardinalities.get(label).copied();
            configured_report.artifact_size_sketch_cardinality =
                artifact_size_cardinalities.get(label).copied();
        }
        configured_report.errors =
            self.convert_error_list(&errors, EntryLabel::Target(target_with_modifiers));
        if !configured_report.errors.is_empty() {
            configured_report.inner.success = BuildOutcome::FAIL;
        }
        Ok(configured_report)
    }

    // ============================================================================
    // Error processing and conversion
    // ============================================================================

    /// Note: In order for production of the build report to be deterministic, the order in
    /// which this function is called, and which errors it is called with, must be
    /// deterministic. The particular order of the errors need not be.
    fn convert_error_list(
        &mut self,
        errors: &[buck2_error::Error],
        entry_label: EntryLabel,
    ) -> Vec<BuildReportError> {
        if errors.is_empty() {
            return Vec::new();
        }
        self.overall_success = false;

        struct ExpandedErrorInfo {
            root: UniqueRootId,
            cause_index: Option<usize>,
            message: String,
            error_tags: Vec<String>,
            action_error: Option<BuildReportActionError>,
            error_category: String,
            category_key: String,
        }

        let mut temp = Vec::with_capacity(errors.len());
        for e in errors {
            // we initially avoid assigning new cause indexes and instead use a sentinal value.
            // This is to make sure that we can be deterministic
            let root = e.root_id();
            let error_report: ErrorReport = e.into();
            let error_category = error_report.category().to_string();
            let category_key = error_report.category_key.clone().unwrap_or_default();
            let message = if let Some(telemetry_message) = error_report.telemetry_message {
                telemetry_message
            } else {
                error_report.message
            };
            // Apply truncation if enabled
            let message = if self.truncate_error_content {
                buck2_util::truncate::truncate(&message, MAX_ERROR_CONTENT_BYTES)
            } else {
                message
            };
            let error_tags = e
                .tags()
                .into_iter()
                .map(|tag| tag.as_str_name().to_owned())
                .collect_vec();

            temp.push(ExpandedErrorInfo {
                root,
                cause_index: self.error_cause_cache.get(&root).copied(),
                message,
                error_tags,
                action_error: e.action_error().map(|e| {
                    BuildReportActionError::new(
                        e,
                        self,
                        ActionErrorBuildOptions {
                            exclude_action_error_diagnostics: self.exclude_action_error_diagnostics,
                            truncate_error_content: self.truncate_error_content,
                        },
                    )
                }),
                error_category,
                category_key,
            });
        }
        // Sort the errors. This sort *almost* guarantees full determinism, but unfortunately
        // not quite; it is hypothetically non-deterministic if the same configured target has
        // two errors with different error roots but the same error message. Probably unlikely?
        temp.sort_unstable_by(|x, y| {
            Ord::cmp(&(x.cause_index, &x.message), &(y.cause_index, &y.message))
        });

        // Deduplicate errors with the same root. We have to do this after sorting to retain
        // determinism.
        //
        // FIXME(JakobDegen): Ideally we wouldn't need this. It originally wasn't here, but this
        // caused the size of the build report to grow very large in some cases. I suspect this
        // is the result of some rules producing large amounts of `other_outputs`. Because those
        // are all top level artifacts that get their own `BuildEvent`, if they all fail, they
        // all get their own error in the build report. Completing the migration to artifact
        // groups would likely let us get rid of this.
        let mut found_roots = BuckMutSet::default();
        temp.retain(|info| found_roots.insert(info.root));

        let mut out = Vec::with_capacity(temp.len());
        // Now assign new cause indexes if we haven't yet
        for info in temp {
            let cause_index = match info.cause_index {
                Some(i) => i,
                None => {
                    // We need to recheck the cache first, as a previous iteration of this loop
                    // may have inserted our root
                    self.error_cause_cache
                        .get(&info.root)
                        .copied()
                        .unwrap_or_else(|| {
                            let index = self.next_cause_index;
                            self.next_cause_index += 1;
                            self.error_cause_cache.insert(info.root, index);
                            index
                        })
                }
            };

            let message_content = self.update_string_cache(info.message.clone());

            out.push(BuildReportError {
                message_content,
                action_error: info.action_error,
                error_tags: info.error_tags,
                cause_index,
                error_category: info.error_category,
                category_key: info.category_key,
            });
        }

        if self.include_failures {
            // Order is deterministic now, so picking the last one is fine. Also, we checked that
            // there was at least one error above.
            //
            // This both omits errors and overwrites previous ones. That's the price you pay for
            // using buck1
            self.failures.insert(
                entry_label,
                self.strings
                    .get(&out.last().unwrap().message_content)
                    .unwrap()
                    .to_owned(),
            );
        }

        out
    }
}

fn update_artifact_info(
    artifact_info: &mut BTreeMap<Arc<str>, ArtifactInfo>,
    provider_name: Arc<str>,
    entry: &ActionDirectoryEntry<ActionSharedDirectory>,
) {
    match entry {
        DirectoryEntry::Dir(dir) => {
            artifact_info.insert(
                provider_name,
                ArtifactInfo::Directory(DirectoryInfo {
                    digest: dir.fingerprint().clone(),
                }),
            );
        }
        DirectoryEntry::Leaf(ActionDirectoryMember::File(metadata)) => {
            let cas_digest = metadata.digest.data();
            artifact_info.insert(
                provider_name,
                ArtifactInfo::File(FileInfo {
                    digest: *cas_digest,
                    is_exec: metadata.is_executable,
                }),
            );
        }
        DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(symlink_target)) => {
            artifact_info.insert(
                provider_name,
                ArtifactInfo::Symlink(SymlinkInfo {
                    symlink_rel_path: symlink_target.target().into(),
                }),
            );
        }
        DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(external_symlink)) => {
            artifact_info.insert(
                provider_name,
                ArtifactInfo::ExternalSymlink(ExternalSymlinkInfo {
                    target: external_symlink.target().into(),
                    remaining_path: if external_symlink.remaining_path().is_empty() {
                        None
                    } else {
                        Some(external_symlink.remaining_path().to_owned())
                    },
                }),
            );
        }
    }
}

fn report_providers_name(label: &ConfiguredProvidersLabel) -> String {
    match label.name() {
        ProvidersName::Default => "DEFAULT".to_owned(),
        ProvidersName::NonDefault(flavor) => match flavor.as_ref() {
            NonDefaultProvidersName::Named(names) => names.iter().join("|"),
        },
    }
}

pub async fn build_report_opts<'a>(
    ctx: &mut DiceComputations<'a>,
    cell_resolver: &CellResolver,
    build_opts: &CommonBuildOptions,
    graph_properties_opts: GraphPropertiesOptions,
) -> buck2_error::Result<BuildReportOpts> {
    let esto = &build_opts.unstable_build_report_filename;
    let build_report_opts = BuildReportOpts {
        print_unconfigured_section: ctx
            .parse_legacy_config_property(
                cell_resolver.root_cell(),
                BuckconfigKeyRef {
                    section: "build_report",
                    property: "print_unconfigured_section",
                },
            )
            .await?
            .unwrap_or(true),
        unstable_include_failures_build_report: build_opts.unstable_include_failures_build_report,
        unstable_include_package_project_relative_paths: build_opts
            .unstable_include_package_project_relative_paths,
        unstable_include_artifact_hash_information: build_opts
            .unstable_include_artifact_hash_information,
        unstable_build_report_filename: esto.clone(),
        graph_properties_opts,
        unstable_streaming_build_report_filename: build_opts
            .unstable_streaming_build_report_filename
            .clone(),
        unstable_exclude_action_error_diagnostics: build_opts
            .unstable_exclude_action_error_diagnostics,
        unstable_truncate_error_content: build_opts.unstable_truncate_error_content,
    };

    Ok(build_report_opts)
}

fn write_or_serialize_build_report(
    build_report: &BuildReport,
    filename: &str,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
) -> Result<Option<String>, buck2_error::Error> {
    let mut serialized_build_report = None;

    if !filename.is_empty() {
        let path = project_root.resolve(cwd).as_abs_path().join(filename);
        if let Some(parent) = path.parent() {
            fs_util::create_dir_all(parent)?;
        }
        let file = fs_util::create_file(path.clone())
            .categorize_internal()
            .buck_error_context("Error writing build report")?;
        let mut file = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut file, build_report)?
    } else {
        serialized_build_report = Some(serde_json::to_string(build_report)?);
    };

    Ok(serialized_build_report)
}

pub fn write_build_report(
    opts: BuildReportOpts,
    artifact_fs: &ArtifactFs,
    cell_resolver: &CellResolver,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
    trace_id: &TraceId,
    configured: &BTreeMap<ConfiguredProvidersLabel, Option<ConfiguredBuildTargetResult>>,
    configured_to_pattern_modifiers: &BuckMutMap<ConfiguredProvidersLabel, BTreeSet<Modifiers>>,
    other_errors: &BTreeMap<Option<ProvidersLabel>, Vec<buck2_error::Error>>,
    detailed_metrics: Option<DetailedAggregatedMetrics>,
    action_graph_sketch_result: Option<ActionGraphSketchResult>,
    artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
) -> Result<Option<String>, buck2_error::Error> {
    let build_report = BuildReportCollector::convert(
        trace_id,
        artifact_fs,
        cell_resolver,
        project_root,
        opts.print_unconfigured_section,
        opts.unstable_include_failures_build_report,
        opts.unstable_include_package_project_relative_paths,
        opts.unstable_include_artifact_hash_information,
        opts.unstable_exclude_action_error_diagnostics,
        opts.unstable_truncate_error_content,
        configured,
        configured_to_pattern_modifiers,
        other_errors,
        None,
        detailed_metrics,
        action_graph_sketch_result,
        artifact_path_sketch_result,
        opts.graph_properties_opts,
    )?;

    write_or_serialize_build_report(
        &build_report,
        &opts.unstable_build_report_filename,
        project_root,
        cwd,
    )
}

pub fn write_bxl_build_report(
    opts: BuildReportOpts,
    artifact_fs: &ArtifactFs,
    cell_resolver: &CellResolver,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
    trace_id: &TraceId,
    bxl_label: &BxlFunctionLabel,
    errors: &[buck2_error::Error],
    detailed_metrics: Option<DetailedAggregatedMetrics>,
    action_graph_sketch_result: Option<ActionGraphSketchResult>,
    artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
) -> Result<Option<String>, buck2_error::Error> {
    let build_report = BuildReportCollector::convert_bxl(
        trace_id,
        artifact_fs,
        cell_resolver,
        project_root,
        opts.print_unconfigured_section,
        opts.unstable_include_failures_build_report,
        opts.unstable_include_package_project_relative_paths,
        opts.unstable_include_artifact_hash_information,
        opts.unstable_exclude_action_error_diagnostics,
        opts.unstable_truncate_error_content,
        bxl_label,
        errors,
        detailed_metrics,
        action_graph_sketch_result,
        artifact_path_sketch_result,
        opts.graph_properties_opts,
    )?;

    write_or_serialize_build_report(
        &build_report,
        &opts.unstable_build_report_filename,
        project_root,
        cwd,
    )
}

pub fn stream_build_report(
    opts: BuildReportOpts,
    artifact_fs: &ArtifactFs,
    cell_resolver: &CellResolver,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
    trace_id: &TraceId,
    configured: &BTreeMap<ConfiguredProvidersLabel, Option<ConfiguredBuildTargetResult>>,
    configured_to_pattern_modifiers: &BuckMutMap<ConfiguredProvidersLabel, BTreeSet<Modifiers>>,
    other_errors: &BTreeMap<Option<ProvidersLabel>, Vec<buck2_error::Error>>,
    finished_builds: &FinishedBuilds,
    detailed_metrics: Option<DetailedAggregatedMetrics>,
    action_graph_sketch_result: Option<ActionGraphSketchResult>,
    artifact_path_sketch_result: Option<ArtifactPathSketchResult>,
) -> buck2_error::Result<()> {
    let build_report = BuildReportCollector::convert(
        trace_id,
        artifact_fs,
        cell_resolver,
        project_root,
        opts.print_unconfigured_section,
        opts.unstable_include_failures_build_report,
        opts.unstable_include_package_project_relative_paths,
        opts.unstable_include_artifact_hash_information,
        opts.unstable_exclude_action_error_diagnostics,
        opts.unstable_truncate_error_content,
        configured,
        configured_to_pattern_modifiers,
        other_errors,
        Some(finished_builds),
        detailed_metrics,
        action_graph_sketch_result,
        artifact_path_sketch_result,
        opts.graph_properties_opts,
    )?;

    let serialized_build_report = Some(serde_json::to_string(&build_report)?);

    let path = project_root
        .resolve(cwd)
        .as_abs_path()
        .join(opts.unstable_streaming_build_report_filename);

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.clone())
        .buck_error_context("Error opening streaming build report file for appending")?;
    let mut file = BufWriter::new(file);
    file.write_all(serialized_build_report.as_ref().unwrap().as_bytes())?;
    file.write_all(b"\n")?;

    Ok(())
}

pub fn initialize_streaming_build_report(
    opts: BuildReportOpts,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
) -> Result<(), buck2_error::Error> {
    let path = project_root
        .resolve(cwd)
        .as_abs_path()
        .join(opts.unstable_streaming_build_report_filename);
    if let Some(parent) = path.parent() {
        fs_util::create_dir_all(parent)?;
    }

    // create and clear the file
    let _file = fs_util::create_file(path.clone())
        .categorize_internal()
        .buck_error_context("Error initializing streaming build report")?;

    Ok(())
}

async fn process_streaming_build_result(
    ctx: DiceTransaction,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
    trace_id: &TraceId,
    build_opts: &CommonBuildOptions,
    build_result: BuildTargetResult,
    graph_properties_opts: GraphPropertiesOptions,
) -> buck2_error::Result<()> {
    let cell_resolver = ctx.ctx().get_cell_resolver().await?;
    let artifact_fs = ctx.ctx().get_artifact_fs().await?;

    let build_report_opts = build_report_opts(
        &mut ctx.ctx(),
        &cell_resolver,
        build_opts,
        graph_properties_opts,
    )
    .await?;

    stream_build_report(
        build_report_opts,
        artifact_fs,
        &cell_resolver,
        project_root,
        cwd,
        trace_id,
        &build_result.configured,
        &build_result.configured_to_pattern_modifiers,
        &build_result.other_errors,
        &build_result.finished_builds,
        None, // no detailed metrics for streaming build reports to avoid the computation/copy
        None, // no action graph sketch for streaming build reports to avoid the computation/copy
        None, // no artifact_path_sketch_result for streaming build reports
    )?;

    Ok(())
}

async fn init_streaming_build_report(
    ctx: DiceTransaction,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
    build_opts: &CommonBuildOptions,
    graph_properties_opts: GraphPropertiesOptions,
) -> buck2_error::Result<()> {
    let cell_resolver = ctx.ctx().get_cell_resolver().await?;

    let build_report_opts = build_report_opts(
        &mut ctx.ctx(),
        &cell_resolver,
        build_opts,
        graph_properties_opts,
    )
    .await?;

    initialize_streaming_build_report(build_report_opts, project_root, cwd)?;

    Ok(())
}

/// Creates the channel whose receiver `maybe_stream_build_reports` consumes. The sender is `None`
/// when no streaming build report was requested.
pub fn streaming_build_result_channel(
    build_opts: &CommonBuildOptions,
) -> (
    Option<UnboundedSender<BuildTargetResult>>,
    UnboundedReceiver<BuildTargetResult>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let requested = !build_opts
        .unstable_streaming_build_report_filename
        .is_empty();
    (requested.then_some(tx), rx)
}

/// Shared by `buck build` and `buck test`. `buck test` keeps its result senders alive past the
/// build phase, so once the channel closes we stop polling it to avoid busy-looping while
/// `command_future` is still running.
pub async fn maybe_stream_build_reports<T>(
    command_future: impl std::future::Future<Output = buck2_error::Result<T>>,
    build_opts: &CommonBuildOptions,
    ctx: DiceTransaction,
    project_root: &ProjectRoot,
    cwd: &ProjectRelativePath,
    trace_id: TraceId,
    graph_properties: GraphPropertiesOptions,
    mut streaming_build_result_rx: UnboundedReceiver<BuildTargetResult>,
) -> buck2_error::Result<T> {
    if build_opts
        .unstable_streaming_build_report_filename
        .is_empty()
    {
        return command_future.await;
    }

    init_streaming_build_report(ctx.clone(), project_root, cwd, build_opts, graph_properties)
        .await?;

    let mut channel_open = true;
    let mut command_future = std::pin::pin!(command_future);
    loop {
        tokio::select! {
            // Wait for the command future (build or test) to finish
            result = &mut command_future => {
                // Drain any remaining streaming results
                while let Ok(streaming_result) = streaming_build_result_rx.try_recv() {
                    process_streaming_build_result(
                        ctx.clone(),
                        project_root,
                        cwd,
                        &trace_id,
                        build_opts,
                        streaming_result,
                        graph_properties,
                    )
                    .await?;
                }
                return result;
            }
            // As long as the channel is open, continue processing streaming results as they
            // arrive. The `if channel_open` guard disables this select arm once every sender has
            // dropped: `recv()` on a closed channel resolves immediately, so without the guard
            // this arm would spin until `command_future` resolves.
            streaming_result = streaming_build_result_rx.recv(), if channel_open => {
                match streaming_result {
                    Some(result) => {
                        process_streaming_build_result(
                            ctx.clone(),
                            project_root,
                            cwd,
                            &trace_id,
                            build_opts,
                            result,
                            graph_properties,
                        )
                        .await?;
                    }
                    None => {
                        // Channel closed, but continue waiting for the command to complete
                        channel_open = false;
                    }
                }
            }
        }
    }
}
