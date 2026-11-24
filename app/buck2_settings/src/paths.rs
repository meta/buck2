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

/// Which paths build files may name.
#[buck_settings_section(name = "paths", version = 0, scope = dice)]
pub struct Paths {
    /// Directories, as cell paths such as `root//foo`, under which a build
    /// file may reach files of a parent directory with `../`, up to the
    /// listed directory itself. At most one entry may cover a given build
    /// file.
    #[setting(
        buckconfig_fallback = "buck2.directories_to_allow_relative_paths",
        overridable_in(CommandLine, LocalSettings)
    )]
    allow_relative_in: Option<Vec<String>>,
}
