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

/// How source files become action inputs.
#[buck_settings_section(name = "source", version = 0, scope = dice)]
pub struct Source {
    /// Whether a source that is itself a symlink reaches actions as that
    /// symlink, with its target as a further input, instead of as a copy of
    /// what it points to.
    #[setting(
        buckconfig_fallback = "buck2.use_correct_source_symlink_reading",
        default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    correct_symlink_reading: Option<bool>,
}
