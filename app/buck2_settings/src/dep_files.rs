/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_settings_derive::buck_settings_section;

/// The daemon's cache of run actions it already ran, keyed by the inputs
/// their dep files say they used.
#[buck_settings_section(name = "dep_files", version = 0, scope = invocation)]
pub struct DepFiles {
    /// Whether a run action whose own cache entry holds a different action is
    /// still served an identical action that another configuration of it ran
    /// in this daemon. Without it, only an action with no entry of its own
    /// looks at other configurations' entries.
    // FIXME(jakobdegen): drop the fallback once `fbandroid/scripts/mm3/reuse_pilot.py`
    // writes the setting instead of the buckconfig key.
    #[setting(
        buckconfig_fallback = "buck2.cross_config_lookup_on_stale_entry",
        internal_default = false,
        oss_default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    cross_config_lookup_on_stale_entry: Option<bool>,
}
