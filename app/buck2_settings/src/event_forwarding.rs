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

/// Whether the daemon starts a helper program for each command that carries an event forwarding
/// key, and which program.
#[buck_settings_section(name = "event_forwarding", version = 0, scope = daemon)]
pub struct EventForwarding {
    /// Whether the daemon starts the helper for each command that carries an event forwarding
    /// key.
    // Enforcement control: nothing local may turn it on.
    #[setting(internal_default = false, oss_default = false, overridable_in())]
    enabled: Option<bool>,
    /// The helper program: an absolute path, or a path relative to the project root.
    // Enforcement control: nothing local may point it at another program.
    #[setting(overridable_in())]
    helper: Option<String>,
}
