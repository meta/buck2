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

/// How a command uses remote execution.
#[buck_settings_section(name = "remote_execution", version = 0, scope = invocation)]
pub struct RemoteExecution {
    /// Cancels a remote action once the seconds remote execution estimates it
    /// will wait in the queue exceed this.
    #[setting(
        buckconfig_fallback = "build.remote_execution_cancel_on_estimated_queue_time_exceeds_s",
        overridable_in(CommandLine, LocalSettings)
    )]
    cancel_on_estimated_queue_time_exceeds_s: Option<u64>,
    /// Lets an action that may also run locally start locally once the
    /// seconds remote execution estimates it will wait in the queue exceed
    /// this. Wins over an executor's own
    /// `remote_execution_queue_time_threshold_s`.
    #[setting(
        buckconfig_fallback = "build.remote_execution_fallback_on_estimated_queue_time_exceeds_s",
        overridable_in(CommandLine, LocalSettings)
    )]
    fallback_on_estimated_queue_time_exceeds_s: Option<u64>,
    /// Bytes of output directory listings a command downloads at once; a
    /// download that would exceed this waits. Unset for no limit.
    #[setting(
        buckconfig_fallback = "buck2.output_trees_download_semaphore_size",
        overridable_in(CommandLine, LocalSettings)
    )]
    output_trees_download_max_concurrent_bytes: Option<u32>,
}
