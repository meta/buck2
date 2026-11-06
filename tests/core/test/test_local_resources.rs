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
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::prelude::*;

/// An empty test executor forces the internal test executor to be used.
fn use_internal_test_executor(buck: &mut Buck) {
    buck.set_env("BUCK2_TPX", "");
}

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
}

/// Waits up to 10 seconds for the broker to record its cleanup in `events`.
async fn wait_for_cleanup(events: &Path) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !events.exists() || !std::fs::read_to_string(events)?.contains("cleanup") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await
    .with_context(|| format!("Timed out waiting for a cleanup in {}", events.display()))?
}

fn read_lines(path: &Path) -> anyhow::Result<Vec<String>> {
    Ok(std::fs::read_to_string(path)?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn index_of(events: &[String], event: &str) -> anyhow::Result<usize> {
    events
        .iter()
        .position(|e| e == event)
        .with_context(|| format!("No `{event}` in {events:?}"))
}

fn count_of(events: &[String], event: &str) -> usize {
    events.iter().filter(|e| *e == event).count()
}

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_platform_resolution(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    buck.test(&[":my_test"]).await?;
    let res = buck.log(&["what-ran"]).await?;
    assert!(res.stdout.contains("MY_RESOURCE_ID=42"));
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows", "darwin"],
    disable_daemon_cgroup = false,
    env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")],
)]
async fn test_local_resource_broker_survives_cgroup_cleanup(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    buck.test(&[":my_daemon_test"]).await?;
    let res = buck.log(&["what-ran"]).await?;
    assert!(res.stdout.contains("BROKER_PID="));
    Ok(())
}

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_two_resource_types_from_one_broker(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    // Two resource types served by the same target: one setup, one resource shared by both.
    buck.test(&[":two_types_one_broker"]).await?;
    let res = buck.log(&["what-ran"]).await?;
    assert_eq!(res.stdout.matches("test.local_resource_setup").count(), 1);
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows", "darwin"],
    env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")],
)]
async fn test_registered_resource_release_and_failure_compensation(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    let events = buck.cwd().join("registration-events");
    for mode in [
        "success",
        "failure",
        "bad_descriptor",
        "ack_loss",
        "timeout",
        "withheld",
    ] {
        remove_file_if_exists(&events)?;
        let target = format!(":registered_{mode}");
        let command = buck.test(&[
            &target,
            "-c",
            "buck2.local_resource_cleanup_registration=true",
        ]);
        if mode == "success" {
            command.await?;
        } else {
            command.await.expect_failure();
        }

        wait_for_cleanup(&events).await?;
        let records = read_lines(&events)?;
        assert!(
            records.first().is_some_and(|r| r == "connected")
                && records.last().is_some_and(|r| r == "cleanup"),
            "{records:?}"
        );
        assert_eq!(
            records.iter().any(|r| r == "registered"),
            mode == "success",
            "{records:?}"
        );
        assert_eq!(
            records.iter().any(|r| r == "register"),
            ["success", "ack_loss", "withheld"].contains(&mode),
            "{records:?}"
        );
    }
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows", "darwin"],
    env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")],
)]
async fn test_pending_ack_keeps_other_setups_running(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    buck.test(&[
        ":concurrent_registered",
        "-c",
        "buck2.local_resource_cleanup_registration=true",
    ])
    .await?;
    let events = read_lines(&buck.cwd().join("registration-events"))?;
    let register_ack_hold = index_of(&events, "register_ack_hold")?;
    let registered_wait_for_holder = index_of(&events, "registered_wait_for_holder")?;
    let registered_ack_hold = index_of(&events, "registered_ack_hold")?;
    assert!(
        register_ack_hold < registered_wait_for_holder
            && registered_wait_for_holder < registered_ack_hold,
        "{events:?}"
    );
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows", "darwin"],
    env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")],
)]
async fn test_subtarget_aliases_share_one_registered_pool(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    buck.test(&[
        ":registered_aliases",
        "-c",
        "buck2.local_resource_cleanup_registration=true",
    ])
    .await?;
    let event_file = buck.cwd().join("registration-events");

    wait_for_cleanup(&event_file).await?;
    let events = read_lines(&event_file)?;
    assert_eq!(count_of(&events, "connected"), 1, "{events:?}");
    assert_eq!(count_of(&events, "registered"), 1, "{events:?}");
    assert_eq!(count_of(&events, "cleanup"), 1, "{events:?}");
    Ok(())
}
