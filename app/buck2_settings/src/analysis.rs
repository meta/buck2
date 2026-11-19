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

/// Settings controlling what analysis records beyond its providers.
#[buck_settings_section(name = "analysis", version = 1, scope = daemon)]
pub struct Analysis {
    /// Whether each analysis keeps the list of anon targets it requested.
    #[setting(default = true, overridable_in(CommandLine, LocalSettings))]
    record_requested_anon_targets: Option<bool>,
}
