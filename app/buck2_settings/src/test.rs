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

/// How `buck2 test` runs tests.
#[buck_settings_section(name = "test", version = 0, scope = invocation)]
pub struct Test {
    /// The path of the test executor, where a leading `$BUCK2_BINARY_DIR/`
    /// is the directory of the buck2 binary. Empty uses the executor built
    /// into buck2, and so does unset in open source.
    #[setting(
        buckconfig_fallback = "test.v2_test_executor",
        internal_default = "$BUCK2_BINARY_DIR/buck2-tpx".to_owned(),
        overridable_in(CommandLine, LocalSettings)
    )]
    executor: Option<String>,
    /// Which tests buck2 runs itself instead of handing them to the executor:
    /// `true` for all of them, which is also what unset means, `false` for
    /// none, or a comma-separated list of test frameworks.
    #[setting(
        buckconfig_fallback = "test.use_internal_runner",
        overridable_in(CommandLine, LocalSettings)
    )]
    internal_runner: Option<String>,
    /// Seconds a test that buck2 runs itself may take.
    #[setting(
        buckconfig_fallback = "test.timeout_default_s",
        internal_default = 600,
        oss_default = 600,
        overridable_in(CommandLine, LocalSettings)
    )]
    default_timeout_s: Option<u64>,
    /// Whether `buck2 test` also builds the default outputs and the run info
    /// of the tests it runs, where the command line does not say.
    #[setting(
        buckconfig_fallback = "buck2.test_builds_targets",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    build_targets: Option<bool>,
    /// Seconds a local test that hit its timeout gets to exit after `SIGTERM`
    /// before it and what is left of its process group get `SIGKILL`. Unset
    /// or `0` kills at once.
    #[setting(
        buckconfig_fallback = "buck2.test_timeout_grace_period_s",
        overridable_in(CommandLine, LocalSettings)
    )]
    timeout_grace_period_s: Option<u64>,
    /// On Linux, whether killing a local test also kills the processes it
    /// left outside its process group, such as daemons.
    #[setting(
        buckconfig_fallback = "buck2.kill_detached_test_processes",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    kill_detached_processes: Option<bool>,
    /// On Linux, whether a local resource's setup command is handed a way to
    /// register cleanup that outlives it.
    #[setting(
        buckconfig_fallback = "buck2.local_resource_cleanup_registration",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    local_resource_cleanup_registration: Option<bool>,
    /// Whether local tests see the `CODING_AGENT_METADATA` of the client that
    /// started the command.
    #[setting(
        buckconfig_fallback = "buck2.tests_inherit_client_coding_agent_metadata",
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    inherit_client_coding_agent_metadata: Option<bool>,
}
