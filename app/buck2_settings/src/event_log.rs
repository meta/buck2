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

/// The event log the client writes for each command.
#[buck_settings_section(name = "event_log", version = 0, scope = invocation)]
pub struct EventLog {
    /// How many event logs are kept on disk, counting the one about to be
    /// written.
    #[setting(default = 12, overridable_in(CommandLine, LocalSettings))]
    retained_logs: Option<usize>,
}
