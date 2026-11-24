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

/// What a command leaves in `buck-out` besides the outputs themselves.
#[buck_settings_section(name = "buck_out", version = 0, scope = invocation)]
pub struct BuckOut {
    /// Whether a build symlinks each output it was asked for at a path
    /// without the configuration hash.
    #[setting(
        buckconfig_fallback = "buck2.create_unhashed_links",
        default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    unhashed_links: Option<bool>,
}
