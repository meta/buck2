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

/// How labels written in build files are read.
#[buck_settings_section(name = "labels", version = 0, scope = dice)]
pub struct Labels {
    /// Whether a label in a build or `.bzl` file that names no target means
    /// the target named like the last directory of its package: `//foo/bar`
    /// for `//foo/bar:bar`.
    #[setting(
        oss_buckconfig_fallback = "buck2.infer_target_names",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    infer_target_names: Option<bool>,
}
