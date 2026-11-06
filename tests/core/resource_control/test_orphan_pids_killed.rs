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
use buck2_e2e::utils::random_string;

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_orphan_pids_killed(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//:spawn_orphan",
        "--no-remote-cache",
        "--local-only",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;

    let events = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "OrphanProcessesKilled"],
    )
    .await?;

    assert!(
        !events.is_empty(),
        "Expected at least one OrphanProcessesKilled instant event"
    );

    let orphan_processes = events[0]["orphan_processes"].as_array().unwrap();
    assert!(
        !orphan_processes.is_empty(),
        "Expected at least one orphan process, got: {orphan_processes:?}"
    );

    // The orphan should be either 'setsid' or 'sleep' that escaped the process group.
    // setsid execs into sleep, so depending on timing we might see either one.
    let comms: Vec<&str> = orphan_processes
        .iter()
        .map(|p| p["comm"].as_str().unwrap())
        .collect();
    assert!(
        comms
            .iter()
            .any(|c| c.contains("setsid") || c.contains("sleep")),
        "Expected to find a 'setsid' or 'sleep' orphan process, got comms: {comms:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_nop(_buck: &Buck) {
    // Pytest gets upset if we have no windows or mac tests in this file
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_no_orphan_same_pg_timeout(buck: &Buck) -> anyhow::Result<()> {
    // Build a target that spawns a background process in the same process
    // group. The action has a short timeout, so it will be cancelled via
    // killpg, which kills the background process too. Cgroup cleanup should
    // find no remaining processes, so no OrphanProcessesKilled event.
    buck.build(&[
        "root//:spawn_same_pg_timeout",
        "--no-remote-cache",
        "--local-only",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("timed out after");

    let events = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "OrphanProcessesKilled"],
    )
    .await?;

    assert!(
        events.is_empty(),
        "Expected no OrphanProcessesKilled events for same-process-group child, got: {events:?}"
    );
    Ok(())
}
