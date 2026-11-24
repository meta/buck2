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

/// What goes into the build report of a build or test command.
#[buck_settings_section(name = "build_report", version = 0, scope = invocation)]
pub struct BuildReport {
    /// Whether each target's entry has the buck1-compatible fields that are
    /// not split by configuration, next to its `configured` map.
    #[setting(
        buckconfig_fallback = "build_report.print_unconfigured_section",
        default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    print_unconfigured_section: Option<bool>,
}
