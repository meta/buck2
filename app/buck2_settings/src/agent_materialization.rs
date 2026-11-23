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

/// Skips materializing the final outputs of builds an AI agent runs directly
/// (commands given `--agent-context direct_call=true`) without requesting
/// outputs.
#[buck_settings_section(name = "agent_materialization", version = 0, scope = invocation)]
pub struct AgentMaterialization {
    /// Whether such a build leaves its final outputs unmaterialized.
    #[setting(default = false, overridable_in(CommandLine, LocalSettings))]
    enabled: Option<bool>,
}
