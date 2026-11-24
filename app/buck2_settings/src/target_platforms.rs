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

/// How a target that names no platform of its own gets one.
#[buck_settings_section(name = "target_platforms", version = 0, scope = dice)]
pub struct TargetPlatforms {
    /// The default target platform by package, for targets without a
    /// `default_target_platform`: space-separated entries of the form
    /// `target:<recursive pattern>-><platform target>`, the first matching
    /// entry winning. Unset means such targets get no platform.
    #[setting(
        buckconfig_fallback = "parser.target_platform_detector_spec",
        overridable_in(CommandLine, LocalSettings)
    )]
    detector_spec: Option<String>,
}
