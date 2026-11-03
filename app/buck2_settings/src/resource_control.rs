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

/// The cgroups the daemon and its actions run in, and what happens under
/// memory pressure. Every key keeps its buckconfig fallback: these are set in
/// repositories, by CI, and in local configs.
#[buck_settings_section(name = "resource_control", version = 0, scope = daemon)]
pub struct ResourceControl {
    /// `off`, `if_available` (when systemd can provide a cgroup) or
    /// `required`. Unset defers to `min_version_for_gated_status`.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.status",
        overridable_in(CommandLine, LocalSettings)
    )]
    status: Option<String>,
    /// Where the daemon's cgroup comes from: `systemd`, or `cgroup:<path>`
    /// for one that already exists. `systemd` when unset.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.init",
        overridable_in(CommandLine, LocalSettings)
    )]
    init: Option<String>,
    /// Memory limit for everything buck2 manages, as bytes or a percentage of
    /// the machine's memory.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_max",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_max: Option<String>,
    /// Like `memory_max`, for cgroup v2's `memory.high`.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_high",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_high: Option<String>,
    /// Memory any one action may allocate.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_max_per_action",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_max_per_action: Option<String>,
    /// Memory any one action may reach before it is throttled.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_high_per_action",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_high_per_action: Option<String>,
    /// `memory.high` for all actions together. Mainly for tests.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_high_actions",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_high_actions: Option<String>,
    /// `memory.max` for all actions together. Mainly for tests.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_max_actions",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_max_actions: Option<String>,
    /// `memory.swap.max` for all actions together, as bytes or `max`. `0`
    /// makes the kernel stall an action at its memory limit instead of
    /// swapping, which is the pressure the scheduler suspends on. Mainly for
    /// tests.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.memory_swap_max_actions",
        overridable_in(CommandLine, LocalSettings)
    )]
    memory_swap_max_actions: Option<String>,
    /// Whether actions are suspended under memory pressure.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.enable_suspension",
        overridable_in(CommandLine, LocalSettings)
    )]
    enable_suspension: Option<bool>,
    /// Also enables suspension when the binary's suspension algorithm is at
    /// least this version, so a fix can be rolled out without reaching
    /// binaries that lack it.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.enable_suspension_if_min_algo_version",
        overridable_in(CommandLine, LocalSettings)
    )]
    enable_suspension_if_min_algo_version: Option<u32>,
    /// Selects an experimental variant of the suspension algorithm.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.experimental_suspension_algo_variant",
        overridable_in(CommandLine, LocalSettings)
    )]
    experimental_suspension_algo_variant: Option<u8>,
    /// How an action is suspended: `kill_and_retry` (the default when unset)
    /// or `cgroup_freeze`.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.preferred_action_suspend_strategy",
        overridable_in(CommandLine, LocalSettings)
    )]
    preferred_action_suspend_strategy: Option<String>,
    /// With `status` unset, binaries whose daemon cgroup logic is at least
    /// this version use `version_gated_default_status`; older ones stay off.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.min_version_for_gated_status",
        overridable_in(CommandLine, LocalSettings)
    )]
    min_version_for_gated_status: Option<u32>,
    /// The status a binary new enough for `min_version_for_gated_status`
    /// takes; `if_available` when unset.
    #[setting(
        buckconfig_fallback = "buck2_resource_control.version_gated_default_status",
        overridable_in(CommandLine, LocalSettings)
    )]
    version_gated_default_status: Option<String>,
}
