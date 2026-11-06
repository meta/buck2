/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;
use serde_json::Value;

// Falling back from an unavailable resource control is the soft error under test.
/// When status=if_available and systemd-run --user can't connect to the
/// user session bus, the daemon should start successfully without cgroups
/// instead of failing with DAEMON_STARTUP_FAILED.
#[buck_test(
    skip_for_os = ["darwin", "windows"],
    disable_daemon_cgroup = false,
    allow_soft_errors = true,
)]
async fn test_if_available_fallback_no_user_session(buck: &mut Buck) -> anyhow::Result<()> {
    let mut buckconfig = std::fs::read_to_string(buck.cwd().join(".buckconfig"))?;
    buckconfig.push_str("[buck2_resource_control]\n");
    buckconfig.push_str("status = if_available\n");
    std::fs::write(buck.cwd().join(".buckconfig"), buckconfig)?;

    // Strip the env vars that systemd-run --user needs to find the D-Bus
    // session bus. This simulates the VS Code 3p extension sandbox which
    // filters these out via an allowlist.
    buck.remove_env("DBUS_SESSION_BUS_ADDRESS");
    buck.remove_env("XDG_RUNTIME_DIR");

    // The daemon should start successfully (fallback to no cgroup).
    buck.targets(&[":"]).await?;

    // Verify daemon is running without cgroups.
    let status_result = buck.status(&["--snapshot"]).await?;
    let status_data: Value = serde_json::from_str(&status_result.stdout)?;
    let snapshot = &status_data["snapshot"];
    assert_eq!(snapshot.get("allprocs_cgroup"), Some(&Value::Null));
    Ok(())
}

// Placeholder for tests to be listed successfully on non-Linux platforms.
#[buck_test]
async fn test_noop() {}
