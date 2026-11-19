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

/// The daemon's materializer.
#[buck_settings_section(name = "materializer", version = 0, scope = daemon)]
pub struct Materializer {
    /// Which outputs a build materializes: `all`, `deferred` (the default
    /// when unset) or `deferred_skip_final_artifacts`.
    // FIXME(jakobdegen): This configures the daemon although materialization
    // is chosen per build; it is here because it was in the startup config.
    #[setting(
        buckconfig_fallback = "buck2.materializations",
        overridable_in(CommandLine, LocalSettings)
    )]
    materializations: Option<String>,
}
