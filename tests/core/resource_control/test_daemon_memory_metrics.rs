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
use buck2_e2e::utils::filter_events;
use serde_json::Value;

fn write_config(buck: &Buck, resource_control: bool) -> anyhow::Result<()> {
    let path = buck.cwd().join(".buckconfig");
    let mut buckconfig = std::fs::read_to_string(&path)?;
    buckconfig.push_str("[buck2_resource_control]\n");
    buckconfig.push_str(&format!(
        "status = {}\n",
        if resource_control { "required" } else { "off" }
    ));
    std::fs::write(&path, buckconfig)?;
    Ok(())
}

async fn start_daemon_and_get_snapshot(buck: &Buck) -> anyhow::Result<Value> {
    // Start the daemon
    buck.targets(&[":"]).await?;

    // Get the snapshot
    let status_result = buck.status(&["--snapshot"]).await?;
    let mut status_data: Value = serde_json::from_str(&status_result.stdout)?;
    let snapshot = status_data["snapshot"].take();
    Ok(snapshot)
}

#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_metrics_cgroup_no_resource_control(buck: &Buck) -> anyhow::Result<()> {
    write_config(buck, false)?;
    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert_eq!(snapshot.get("allprocs_cgroup"), Some(&Value::Null));
    assert_eq!(
        snapshot.get("forkserver_actions_cgroup"),
        Some(&Value::Null)
    );
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_metrics_cgroup_resource_control(buck: &Buck) -> anyhow::Result<()> {
    write_config(buck, true)?;
    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    let allprocs = |key: &str| snapshot["allprocs_cgroup"][key].as_u64().unwrap();
    let forkserver_actions =
        |key: &str| snapshot["forkserver_actions_cgroup"][key].as_u64().unwrap();
    // Daemon should have allocated at least 500KB of anon memory
    assert!(allprocs("anon") >= forkserver_actions("anon") + 500000);
    assert!(allprocs("file") >= forkserver_actions("file"));
    assert!(allprocs("kernel") >= forkserver_actions("kernel"));
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_cgroup_path_tag(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;
    let events = filter_events(buck, &["Event", "data", "Instant", "data", "SystemInfo"]).await?;
    assert!(!events.is_empty());
    let path = &events[0]["daemon_cgroup_slice_path"];
    assert!(!path.is_null());
    assert!(path.as_str().unwrap().starts_with("/sys/fs/cgroup/"));
    Ok(())
}

// Placeholder for tests to be listed successfully on non-Linux platforms.
#[buck_test]
async fn test_noop() {}
