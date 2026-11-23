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

/// Where a build finds its execution platforms.
#[buck_settings_section(name = "execution_platforms", version = 0, scope = dice)]
pub struct ExecutionPlatforms {
    /// The target whose `ExecutionPlatformRegistrationInfo` lists the
    /// execution platforms, as a label relative to the root cell. Unset,
    /// every target runs its actions on one implicit execution platform in
    /// the target's own configuration.
    #[setting(
        buckconfig_fallback = "build.execution_platforms",
        overridable_in(CommandLine, LocalSettings)
    )]
    target: Option<String>,
}
