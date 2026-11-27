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
        internal_default = 10,
        oss_default = 10,
        overridable_in(CommandLine, LocalSettings)
    )]
    persistent_worker_shutdown_timeout_s: Option<u32>,
    /// Whether a persistent worker outlives the command that started it and
    /// serves later commands, until it has been idle for
    /// `persistent_worker_idle_timeout_s`.
    #[setting(
        buckconfig_fallback = "buck2.persistent_workers_across_commands",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    persistent_workers_across_commands: Option<bool>,
    /// Seconds a persistent worker kept across commands waits for its next
    /// action before it stops.
    #[setting(
        buckconfig_fallback = "buck2.persistent_worker_idle_timeout_s",
        internal_default = 900,
        oss_default = 900,
        overridable_in(CommandLine, LocalSettings)
    )]
    persistent_worker_idle_timeout_s: Option<u32>,
    /// Whether local actions run under miniperf, which counts the instructions
    /// they execute. Turn it off where perf counters are unavailable, as in
    /// some virtual machines.
    #[setting(
        buckconfig_fallback = "buck2.miniperf2",
        internal_default = true,
        oss_default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    miniperf: Option<bool>,
}
