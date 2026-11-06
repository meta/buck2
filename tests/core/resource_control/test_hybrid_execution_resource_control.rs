/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

fn use_some_memory_args(buck: &Buck) -> [String; 2] {
    let bin = buck
        .env_var("USE_SOME_MEMORY_BIN")
        .expect("`USE_SOME_MEMORY_BIN` is set");
    [
        "-c".to_owned(),
        format!("use_some_memory.path={}", bin.to_string_lossy()),
    ]
}

// get the daemon cgroup path
fn get_daemon_cgroup_path(pid: u64) -> anyhow::Result<PathBuf> {
    let cgroup_path = format!("/proc/{pid}/cgroup");

    for line in std::fs::read_to_string(&cgroup_path)?.lines() {
        // cgroup v2 format: 0::/path/to/cgroup
        if line.starts_with("0::") {
            let cgroup_relative_path = line.trim().split_once("::").unwrap().1;
            return Ok(PathBuf::from(format!(
                "/sys/fs/cgroup{cgroup_relative_path}"
            )));
        }
    }

    anyhow::bail!("Could not find cgroup v2 entry for PID {pid}")
}

// To not fail listing on Mac or Windows
#[buck_test]
async fn test_dummy() {}

#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_memory_pressure_telemetry(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        concat!(
            "[buck2_resource_control]\n",
            "memory_high_per_action = 1048576\n", // 1 MiB
        ),
    )?;

    buck.build(&[
        ":allocate_10_10M",
        "--no-remote-cache",
        "-c",
        "build.use_limited_hybrid=False",
        "-c",
        "build.execution_platforms=//:platforms",
    ])
    .await?;

    let resource_control_events = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "ResourceControlEvent"],
    )
    .await?;

    // We can't reliably predict how many events will be fired and how high the pressure % will reach,
    // but it should be more than 0 with how low we set the memory_high as and obviously % should be <= 100
    assert!(!resource_control_events.is_empty());
    let last_event = resource_control_events.last().unwrap();
    let pressure = &last_event["allprocs_memory_pressure"];
    assert!(
        pressure.as_f64().unwrap() <= 100.0,
        "Expected % memory_pressure to be at most 100, got {pressure}"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_resource_control_events_created(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        format!(
            concat!(
                "[buck2_resource_control]\n",
                "status = required\n",
                "enable_action_cgroup_pool_v2 = true\n",
                "memory_high_actions = {}\n", // 200 MiB
                "enable_suspension = true\n",
            ),
            200 * 1024 * 1024
        ),
    )?;

    let use_some_memory_args = use_some_memory_args(buck);
    buck.build(&[
        "prelude//:freeze_unfreeze_target",
        "--no-remote-cache",
        "-c",
        "build.use_limited_hybrid=False",
        "-c",
        "build.execution_platforms=//:platforms",
        "--local-only",
        &use_some_memory_args[0],
        &use_some_memory_args[1],
    ])
    .await?;

    let event = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "ResourceControlEvent"],
    )
    .await?;

    // 10 means scheduled event
    assert!(event.iter().any(|e| e["kind"] == 10));
    // assert len(list(filter(lambda e: e["kind"] != 10, event))) > 0
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
async fn test_daemon_id_in_cgroup_path(buck: &Buck) -> anyhow::Result<()> {
    buck.server(&[]).await?;

    let result = buck.status(&[]).await?;
    let status: Value = serde_json::from_str(&result.stdout)?;
    let daemon_id = status["daemon_constraints"]["daemon_id"].as_str().unwrap();

    let daemon_cgroup_path =
        get_daemon_cgroup_path(status["process_info"]["pid"].as_u64().unwrap())?;

    assert!(
        daemon_cgroup_path
            .to_string_lossy()
            .contains(&daemon_id.replace('-', "_"))
    );
    Ok(())
}
