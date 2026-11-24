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

/// How a command runs actions on this machine.
#[buck_settings_section(name = "execution", version = 0, scope = invocation)]
pub struct Execution {
    /// How many actions run locally at once; `0` or unset for one per core.
    /// `-j` on the command line wins.
    #[setting(
        buckconfig_fallback = "build.threads",
        overridable_in(CommandLine, LocalSettings)
    )]
    threads: Option<u32>,
    /// Seconds a persistent worker gets to exit after `SIGTERM` at shutdown
    /// before its process group gets `SIGKILL`.
    #[setting(
        buckconfig_fallback = "build.persistent_worker_shutdown_timeout_s",
        default = 10,
        overridable_in(CommandLine, LocalSettings)
    )]
    persistent_worker_shutdown_timeout_s: Option<u32>,
    /// Whether local actions run under miniperf, which counts the instructions
    /// they execute. Turn it off where perf counters are unavailable, as in
    /// some virtual machines.
    #[setting(
        buckconfig_fallback = "buck2.miniperf2",
        default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    miniperf: Option<bool>,
}
