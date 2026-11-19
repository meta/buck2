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

/// The health checks the client runs alongside a command.
#[buck_settings_section(name = "health_check", version = 0, scope = invocation)]
pub struct HealthCheck {
    /// Whether the client runs health checks and shows what they find.
    // FIXME(jakobdegen): drop the fallback once fbsource sets this in `.bucksettings.toml`.
    #[setting(
        buckconfig_fallback = "buck2_health_check.enable_health_checks",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    enabled: Option<bool>,
}
