/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::rollout_percentage::RolloutFraction;
use buck2_directory::directory::dashmap_directory_interner::DashMapDirectoryInterner;
use buck2_execute::directory::ActionDirectoryMember;
pub use buck2_settings::WriteActionCacheMode;
use dice::UserComputationData;
use dupe::Dupe;

/// Knobs controlling how RunAction works.
#[derive(Clone, Dupe, Default)]
pub struct RunActionKnobs {
    /// Process dep files as they are generated.
    pub eager_dep_files: bool,

    /// Whether to try reading from the action output cache (in buck-out/*/offline-cache)
    /// for network actions (download_file, cas_artifact). Used to support offline
    /// builds. The cache is only populated when tracing I/O
    /// (`buck2 debug trace-io`), so this is not a general-purpose download cache.
    pub use_network_action_output_cache: bool,

    /// Default for per-action `allow_cache_upload`, to make it opt-out instead of opt-in.
    pub default_allow_cache_upload: bool,

    /// Shares equal input directories between the actions of a command.
    pub action_paths_interner: DashMapDirectoryInterner<ActionDirectoryMember, TrackedFileDigest>,

    /// Whether write actions ask the CAS for their content first and declare a CAS download
    /// instead of a write when it is there.
    pub write_cas_probe: bool,

    /// `buck2.defer_write_actions`: a write made here keeps its content off its output path
    /// until something needs the file. The deferred core does that itself, at `declare_write`;
    /// under the disk-state core the action keeps the content in the BlobStore's memory
    /// (`buck2_action_impl`'s `produce::write_here`).
    pub defer_write_actions: bool,

    /// `write_actions.cache`.
    pub write_action_cache: WriteActionCacheMode,

    /// `write_actions.cache_populate`: the share of write keys whose misses are recorded in the
    /// action cache for later builds, under an executor that publishes local results at all
    /// (`allow_cache_uploads`).
    pub write_action_cache_populate: RolloutFraction,

    /// `dep_files.cross_config_lookup_on_stale_entry`.
    pub cross_config_lookup_on_stale_entry: bool,

    /// Record the action graph in the event log, from the request's `--log-action-graph` or
    /// `event_log.action_graph`.
    pub log_action_graph: bool,
}

pub trait HasRunActionKnobs {
    fn set_run_action_knobs(&mut self, knobs: RunActionKnobs);

    fn get_run_action_knobs(&self) -> &RunActionKnobs;
}

impl HasRunActionKnobs for UserComputationData {
    fn set_run_action_knobs(&mut self, knobs: RunActionKnobs) {
        self.data.set(knobs);
    }

    fn get_run_action_knobs(&self) -> &RunActionKnobs {
        self.data
            .get::<RunActionKnobs>()
            .expect("RunActionKnobs should be set")
    }
}
