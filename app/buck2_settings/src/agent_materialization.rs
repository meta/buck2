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

/// Skip final build output materialization for direct agent calls that do not
/// request outputs.
#[buck_settings_section(name = "agent_materialization", version = 0, scope = invocation)]
pub struct AgentMaterialization {
    #[setting(default = false, overridable_in(CommandLine, LocalSettings))]
    enabled: Option<bool>,
}
