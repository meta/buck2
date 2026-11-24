/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::str::FromStr;

use allocative::Allocative;
use buck2_error::buck2_error;
use buck2_settings_derive::buck_settings_section;
use dupe::Dupe;
use serde::Deserialize;
use serde::Serialize;

/// A kind of output of a top-level target, for
/// `build_metrics.artifact_sketch_skip_providers`.
#[derive(
    Allocative,
    Clone,
    Copy,
    Dupe,
    Debug,
    Deserialize,
    Serialize,
    PartialEq,
    Eq
)]
#[serde(rename_all = "snake_case")]
pub enum SketchedProvider {
    /// The default outputs.
    Build,
    /// What `buck2 run` needs.
    Run,
    /// What `buck2 test` needs.
    Test,
}

/// Parses an element of the `buck2.providers_to_skip_in_artifact_path_sketch` buckconfig value;
/// the names are the TOML ones.
impl FromStr for SketchedProvider {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "build" => Ok(Self::Build),
            "run" => Ok(Self::Run),
            "test" => Ok(Self::Test),
            other => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid skip provider: `{other}`. Valid values are: [`build`, `run`, `test`]"
            )),
        }
    }
}

/// Measurements of the size of a build that a build or test command
/// computes on request and reports with its results. A sketch is a summary
/// of a set that can be merged with others to estimate the size of their
/// union.
#[buck_settings_section(name = "build_metrics", version = 0, scope = invocation)]
pub struct BuildMetrics {
    /// For each top-level target, the number of configured targets in its
    /// dependency graph.
    #[setting(
        buckconfig_fallback = "buck2.log_configured_graph_size",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    configured_graph_size: Option<bool>,
    /// For each top-level target, a sketch of its configured dependency
    /// graph.
    #[setting(
        buckconfig_fallback = "buck2.log_configured_graph_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    configured_graph_sketch: Option<bool>,
    /// One sketch of the configured dependency graph of all top-level
    /// targets together.
    #[setting(
        buckconfig_fallback = "buck2.log_total_configured_graph_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    total_configured_graph_sketch: Option<bool>,
    /// A sketch of the memory that analysis retains.
    #[setting(
        buckconfig_fallback = "buck2.log_retained_analysis_memory_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    retained_analysis_memory_sketch: Option<bool>,
    /// A sketch of the peak memory of analysis.
    #[setting(
        buckconfig_fallback = "buck2.log_peak_analysis_memory_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    peak_analysis_memory_sketch: Option<bool>,
    /// A sketch of the peak memory of loading build files.
    #[setting(
        buckconfig_fallback = "buck2.log_peak_load_memory_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    peak_load_memory_sketch: Option<bool>,
    /// For each top-level target, a sketch of its action graph.
    #[setting(
        buckconfig_fallback = "buck2.log_action_graph_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    action_graph_sketch: Option<bool>,
    /// For each top-level target, a sketch of its artifacts by count.
    #[setting(
        buckconfig_fallback = "buck2.log_artifact_count_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    artifact_count_sketch: Option<bool>,
    /// For each top-level target, a sketch of its artifacts by size.
    #[setting(
        buckconfig_fallback = "buck2.log_artifact_size_sketch",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    artifact_size_sketch: Option<bool>,
    /// The outputs of a top-level target that the artifact sketches leave
    /// out: any of `build`, `run` and `test`.
    #[setting(
        buckconfig_fallback = "buck2.providers_to_skip_in_artifact_path_sketch",
        overridable_in(CommandLine, LocalSettings)
    )]
    artifact_sketch_skip_providers: Option<Vec<SketchedProvider>>,
    /// Whether each sketch in the build report comes with a
    /// `<field>_cardinality` field that holds its estimated cardinality.
    #[setting(
        buckconfig_fallback = "buck2.log_sketch_cardinalities",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    sketch_cardinalities: Option<bool>,
    /// Aggregated metrics of the actions behind each top-level target and
    /// behind all of them together.
    #[setting(
        buckconfig_fallback = "buck2.detailed_aggregated_metrics",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    detailed_aggregated: Option<bool>,
}
