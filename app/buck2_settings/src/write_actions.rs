/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::rollout_percentage::RolloutFraction;
use buck2_settings_derive::buck_settings_section;
use dupe::Dupe;
use serde::Deserialize;
use serde::Serialize;

/// How write actions use the action cache. Whether a miss is recorded is
/// `write_actions.cache_populate`'s decision, not this one's.
#[derive(
    Allocative,
    Copy,
    Clone,
    Dupe,
    Debug,
    Default,
    Deserialize,
    Serialize,
    PartialEq,
    Eq
)]
#[serde(rename_all = "snake_case")]
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

/// How a command produces the outputs of write actions.
#[buck_settings_section(name = "write_actions", version = 0, scope = invocation)]
pub struct WriteActions {
    /// Whether a write action asks the CAS for its content first and declares
    /// a CAS download instead of a write when it is there. Only takes effect
    /// where the materializer defers writes.
    #[setting(
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    cas_probe: Option<bool>,
    /// How write actions use the action cache: `off`, `verify` or `use`.
    #[setting(
        internal_default = WriteActionCacheMode::Off,
        oss_default = WriteActionCacheMode::Off,
        overridable_in(CommandLine, LocalSettings)
    )]
    cache: Option<WriteActionCacheMode>,
    /// The share of write keys, from 0 to 1, whose misses are recorded in the
    /// action cache for later builds. By key rather than by host, so the
    /// cache fills in steps that are the same on every host. A write within
    /// the share goes through the cache whatever `cache` says.
    #[setting(
        internal_default = RolloutFraction::none(),
        oss_default = RolloutFraction::none(),
        overridable_in(CommandLine, LocalSettings)
    )]
    cache_populate: Option<RolloutFraction>,
}
