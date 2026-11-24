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

/// What happens to a target that is incompatible with the configuration it
/// is built in.
#[buck_settings_section(name = "target_compatibility", version = 0, scope = dice)]
pub struct TargetCompatibility {
    /// Target patterns under which a target that is incompatible only because
    /// one of its dependencies is, is an error. Elsewhere such a target is
    /// skipped as incompatible, with a soft error.
    #[setting(
        buckconfig_fallback = "buck2.error_on_dep_only_incompatible",
        overridable_in(CommandLine, LocalSettings)
    )]
    error_on_dep_only_incompatible: Option<Vec<String>>,
    /// Target patterns taken back out of `error_on_dep_only_incompatible`.
    #[setting(
        buckconfig_fallback = "buck2.error_on_dep_only_incompatible_excluded",
        overridable_in(CommandLine, LocalSettings)
    )]
    error_on_dep_only_incompatible_excluded: Option<Vec<String>>,
    /// A target whose `DepOnlyIncompatibleInfo` names further soft errors to
    /// report for such a target, each for its own target patterns.
    #[setting(
        buckconfig_fallback = "buck2.dep_only_incompatible_info",
        overridable_in(CommandLine, LocalSettings)
    )]
    dep_only_incompatible_info: Option<String>,
}
