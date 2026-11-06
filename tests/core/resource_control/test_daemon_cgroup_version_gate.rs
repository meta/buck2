/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;
use serde_json::Value;

async fn start_daemon_and_get_snapshot(buck: &Buck) -> anyhow::Result<Value> {
    buck.targets(&[":"]).await?;
    let status_result = buck.status(&["--snapshot"]).await?;
    let mut status_data: Value = serde_json::from_str(&status_result.stdout)?;
    Ok(status_data["snapshot"].take())
}

/// Appends `lines` to the `.buckconfig` of `project`.
fn append_to_buckconfig(project: &Path, lines: &[&str]) -> anyhow::Result<()> {
    let path = project.join(".buckconfig");
    let mut buckconfig = std::fs::read_to_string(&path)?;
    for line in lines {
        buckconfig.push_str(line);
        buckconfig.push('\n');
    }
    std::fs::write(&path, buckconfig)?;
    Ok(())
}

/// When min_version_for_gated_status is set to a version <= the binary's
/// DAEMON_CGROUP_VERSION, resource control should be enabled (status =
/// if_available).
#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_version_gate_enables_cgroup(buck: &Buck) -> anyhow::Result<()> {
    append_to_buckconfig(
        buck.cwd(),
        &[
            "[buck2_resource_control]",
            // Version 1 is the current DAEMON_CGROUP_VERSION, so this should enable.
            "min_version_for_gated_status = 1",
        ],
    )?;

    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert!(
        snapshot
            .get("allprocs_cgroup")
            .is_some_and(|cgroup| !cgroup.is_null())
    );
    Ok(())
}

/// When the version gate passes, version_gated_default_status overrides the
/// default gated status of if_available.
#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_version_gated_default_status_overrides_gated_default(
    buck: &Buck,
) -> anyhow::Result<()> {
    append_to_buckconfig(
        buck.cwd(),
        &[
            "[buck2_resource_control]",
            "min_version_for_gated_status = 1",
            "version_gated_default_status = off",
        ],
    )?;

    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert_eq!(snapshot.get("allprocs_cgroup"), Some(&Value::Null));
    Ok(())
}

/// When the explicit status is present it takes precedence.
#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_explicit_status_overrides_gated_one(buck: &Buck) -> anyhow::Result<()> {
    append_to_buckconfig(
        buck.cwd(),
        &[
            "[buck2_resource_control]",
            "status = off",
            "min_version_for_gated_status = 1",
            "version_gated_default_status = required",
        ],
    )?;

    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert_eq!(snapshot.get("allprocs_cgroup"), Some(&Value::Null));
    Ok(())
}

/// When min_version_for_gated_status is set to a version higher than the
/// binary's DAEMON_CGROUP_VERSION, resource control should remain off.
#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_version_gate_disables_cgroup_when_version_too_high(
    buck: &Buck,
) -> anyhow::Result<()> {
    append_to_buckconfig(
        buck.cwd(),
        &[
            "[buck2_resource_control]",
            // Version 9999 is higher than any DAEMON_CGROUP_VERSION, so this should not enable.
            "min_version_for_gated_status = 9999",
        ],
    )?;

    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert_eq!(snapshot.get("allprocs_cgroup"), Some(&Value::Null));
    Ok(())
}

/// When min_version_for_gated_status is not set and status is off,
/// resource control should be off.
#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_version_gate_not_set_status_off(buck: &Buck) -> anyhow::Result<()> {
    append_to_buckconfig(buck.cwd(), &["[buck2_resource_control]", "status = off"])?;

    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert_eq!(snapshot.get("allprocs_cgroup"), Some(&Value::Null));
    Ok(())
}

// Placeholder for tests to be listed successfully on non-Linux platforms.
#[buck_test]
async fn test_noop() {}
