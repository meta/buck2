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

/// How a command uses the remote action cache.
#[buck_settings_section(name = "action_cache", version = 0, scope = invocation)]
pub struct ActionCache {
    /// Whether an action that does not say uploads the result of running
    /// locally: the default of a run action's `allow_cache_upload`.
    #[setting(
        buckconfig_fallback = "buck2.default_allow_cache_upload",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    default_allow_upload: Option<bool>,
}
