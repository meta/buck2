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

/// The Starlark interpreter that evaluates build files and macros.
#[buck_settings_section(name = "starlark", version = 0, scope = dice)]
pub struct Starlark {
    /// The deepest call stack an evaluation may build; unset leaves the
    /// interpreter's own limit.
    // FIXME(jakobdegen): drop the fallback once fbsource sets this in `.bucksettings.toml`.
    #[setting(
        buckconfig_fallback = "buck2.starlark_max_callstack_size",
        overridable_in(CommandLine, LocalSettings)
    )]
    max_callstack_size: Option<usize>,
}
