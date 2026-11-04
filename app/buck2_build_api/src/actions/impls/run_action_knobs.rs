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

use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::rollout_percentage::RolloutFraction;
use buck2_directory::directory::dashmap_directory_interner::DashMapDirectoryInterner;
use buck2_error::buck2_error;
use buck2_execute::directory::ActionDirectoryMember;
use dice::UserComputationData;
use dupe::Dupe;

/// Knobs controlling how RunAction works.
#[derive(Clone, Dupe, Default)]
pub struct RunActionKnobs {
    /// Process dep files as they are generated.
    pub eager_dep_files: bool,

    /// Whether to try reading from the action output cache (in buck-out/*/offline-cache)
    /// for network actions (download_file, cas_artifact). Used to support offline
    /// builds.
    pub use_network_action_output_cache: bool,

    /// Default for per-action `allow_cache_upload`, to make it opt-out instead of opt-in.
    pub default_allow_cache_upload: bool,

    pub action_paths_interner:
        Option<DashMapDirectoryInterner<ActionDirectoryMember, TrackedFileDigest>>,

    /// Whether write actions ask the CAS for their content first and declare a CAS download
    /// instead of a write when it is there.
    pub write_cas_probe: bool,

    /// `buck2.defer_write_actions`: a write made here keeps its content off its output path
    /// until something needs the file. The deferred core does that itself, at `declare_write`;
    /// under the disk-state core the action keeps the content in the BlobStore's memory
    /// (`buck2_action_impl`'s `produce::write_here`).
    pub defer_write_actions: bool,

    pub write_action_cache: WriteActionCacheMode,

    /// The share of write keys whose misses are recorded in the action cache for later builds,
    /// under an executor that publishes local results at all (`allow_cache_uploads`):
    /// `buck2.write_action_cache_populate`. By key rather than by host, so that the cache fills,
    /// and the traffic to it grows, in steps that are the same on every host. A write within
    /// the share goes through the cache whatever `write_action_cache` says.
    pub write_action_cache_populate: RolloutFraction,
}

/// How write actions use the action cache; `buck2.write_action_cache`. Whether a miss is
/// recorded is `write_action_cache_populate`'s decision, not this one's.
#[derive(Copy, Clone, Dupe, Debug, Default, PartialEq, Eq)]
pub enum WriteActionCacheMode {
    #[default]
    Off,
    /// As `Off`, and the cache is asked as well: a hit whose content is not what the write
    /// renders is reported (`write_action_cache_mismatch`), and no miss is recorded. For trying
    /// the key out before `Use`.
    Verify,
    /// A write whose content the cache knows is declared as that content without rendering
    /// it.
    Use,
}

impl FromStr for WriteActionCacheMode {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(Self::Off),
            "verify" => Ok(Self::Verify),
            "use" => Ok(Self::Use),
            _ => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Unknown `write_action_cache` mode `{s}`; expected one of [`off`, `verify`, `use`]"
            )),
        }
    }
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
