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
use std::time::Instant;

use buck2_e2e::BuckOutput;
use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;

/// Waits until the build has started an action, as its event log shows.
async fn wait_for_action_start(mut build: BuckProcess, event_log: &Path) -> BuckProcess {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if build.try_wait().is_some() {
            let output = match build.wait().await {
                Ok(output) => output,
                Err(error) => error.into_output(),
            };
            panic!(
                "Build exited before starting an action (exit_code={:?}): {}",
                output.exit_code(),
                output.stderr
            );
        }
        if std::fs::read_to_string(event_log).is_ok_and(|log| log.contains("\"ActionExecution\"")) {
            return build;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for an ActionExecution event in {}",
            event_log.display()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The builds that exit within `timeout` of the first check (as `asyncio.wait` with
/// `FIRST_COMPLETED` reports them), each expected to have failed, and the builds still running.
async fn wait_first_completed(
    mut builds: Vec<BuckProcess>,
    timeout: Duration,
) -> (Vec<BuckOutput>, Vec<BuckProcess>) {
    let deadline = Instant::now() + timeout;
    loop {
        let exited: Vec<bool> = builds.iter_mut().map(|b| b.try_wait().is_some()).collect();
        if exited.contains(&true) || Instant::now() >= deadline {
            let mut done = Vec::new();
            let mut pending = Vec::new();
            for (build, exited) in builds.into_iter().zip(exited) {
                if exited {
                    done.push(build.wait().await.expect_failure());
                } else {
                    pending.push(build);
                }
            }
            return (done, pending);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[buck_test]
async fn test_exit_when_different_state(buck: &Buck) {
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            "--exit-when=differentstate",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let b = buck
        .build(&[
            "-c",
            "foo.bar=2",
            "--exit-when=differentstate",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    assert_eq!(done.len(), 1);
    assert_eq!(pending.len(), 1);

    for result in done {
        assert!(result.stderr.contains("daemon is busy"));
        assert_eq!(result.exit_code(), Some(4));
    }
}

async fn exit_when_preemptible_always(buck: &Buck, same_state: bool) {
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            "--preemptible=always",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let b = buck
        .build(&[
            "-c",
            // We expect to ALWAYS preempt commands, to prevent blocking new callees
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            "--preemptible=always",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    assert_eq!(done.len(), 1);
    assert_eq!(pending.len(), 1);

    for result in done {
        assert!(result.stderr.contains("daemon preempted"));
        assert_eq!(result.exit_code(), Some(5));
    }
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_preemptible_always_True(buck: &Buck) {
    exit_when_preemptible_always(buck, true).await;
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_preemptible_always_False(buck: &Buck) {
    exit_when_preemptible_always(buck, false).await;
}

#[buck_test(write_invocation_record = true)]
async fn test_preemptible_logged(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&["--preemptible=always", ":"]).await?;
    let record = res.invocation_record()?;
    assert_eq!(record["preemptible"], "ALWAYS");
    Ok(())
}

async fn exit_when_preemptible_on_different_state(buck: &Buck, same_state: bool) {
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            "--preemptible=ondifferentstate",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let b = buck
        .build(&[
            "-c",
            // We expect to ALWAYS preempt commands, to prevent blocking new callees
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            "--preemptible=ondifferentstate",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    if same_state {
        // No preempt when state is the same
        assert_eq!(done.len(), 0);
        assert_eq!(pending.len(), 2);
    } else {
        assert_eq!(done.len(), 1);
        assert_eq!(pending.len(), 1);
    }

    // Expect all done builds to be "done" because they're preempted
    for result in done {
        assert!(result.stderr.contains("daemon preempted"));
        assert_eq!(result.exit_code(), Some(5));
    }
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_preemptible_on_different_state_True(buck: &Buck) {
    exit_when_preemptible_on_different_state(buck, true).await;
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_preemptible_on_different_state_False(buck: &Buck) {
    exit_when_preemptible_on_different_state(buck, false).await;
}

/// Test that an incoming command with --exit-when=notidle does not start if there's another command running.
async fn exit_when_not_idle_does_not_start_when_daemon_busy(
    buck: &Buck,
    same_state: bool,
    tmp_path: &Path,
) {
    let event_log = tmp_path.join("first.json-lines");
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
            "--event-log",
            event_log.to_str().unwrap(),
        ])
        .spawn();
    let a = wait_for_action_start(a, &event_log).await;

    let b = buck
        .build(&[
            "-c",
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            "--exit-when=notidle",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    // Wait for one of the builds to complete
    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    assert_eq!(done.len(), 1);
    assert_eq!(pending.len(), 1);

    for result in done {
        assert!(result.stderr.contains("daemon is busy"));
        assert_eq!(result.exit_code(), Some(4));
    }
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_not_idle_does_not_start_when_daemon_busy_True(
    buck: &Buck,
    tmp_path: &Path,
) {
    exit_when_not_idle_does_not_start_when_daemon_busy(buck, true, tmp_path).await;
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_not_idle_does_not_start_when_daemon_busy_False(
    buck: &Buck,
    tmp_path: &Path,
) {
    exit_when_not_idle_does_not_start_when_daemon_busy(buck, false, tmp_path).await;
}

/// Test that a running command with --exit-when=notidle continues running even with an incoming command.
async fn exit_when_not_idle_does_not_gets_preempted(
    buck: &Buck,
    same_state: bool,
    tmp_path: &Path,
) {
    let event_log = tmp_path.join("first.json-lines");
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            "--exit-when=notidle",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
            "--event-log",
            event_log.to_str().unwrap(),
        ])
        .spawn();
    let a = wait_for_action_start(a, &event_log).await;

    // Start another command without the flag (default is --preemptible=never)
    let b = buck
        .build(&[
            "-c",
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    // Wait for one of the builds to complete
    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    assert_eq!(done.len(), 0);
    assert_eq!(pending.len(), 2);
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_not_idle_does_not_gets_preempted_True(buck: &Buck, tmp_path: &Path) {
    exit_when_not_idle_does_not_gets_preempted(buck, true, tmp_path).await;
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_not_idle_does_not_gets_preempted_False(buck: &Buck, tmp_path: &Path) {
    exit_when_not_idle_does_not_gets_preempted(buck, false, tmp_path).await;
}

/// Test that a running command with --exit-when=notidle and --preemptible gets preempted with an incoming command.
async fn preemptible_exit_when_not_idle_gets_preempted(
    buck: &Buck,
    same_state: bool,
    tmp_path: &Path,
) {
    let event_log = tmp_path.join("first.json-lines");
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            "--exit-when=notidle",
            "--preemptible=always",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
            "--event-log",
            event_log.to_str().unwrap(),
        ])
        .spawn();
    let a = wait_for_action_start(a, &event_log).await;

    // Start another command without the flag (default is --preemptible=never)
    let b = buck
        .build(&[
            "-c",
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    // Wait for one of the builds to complete
    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    assert_eq!(done.len(), 1);
    assert_eq!(pending.len(), 1);

    // Expect all done builds to be "done" because they're preempted
    for result in done {
        assert!(result.stderr.contains("daemon preempted"));
        assert_eq!(result.exit_code(), Some(5));
    }
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_preemptible_exit_when_not_idle_gets_preempted_True(buck: &Buck, tmp_path: &Path) {
    preemptible_exit_when_not_idle_gets_preempted(buck, true, tmp_path).await;
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_preemptible_exit_when_not_idle_gets_preempted_False(buck: &Buck, tmp_path: &Path) {
    preemptible_exit_when_not_idle_gets_preempted(buck, false, tmp_path).await;
}

/// Test that a running command with --exit-when=notidle does NOT get preempted by an incoming command that
/// also has --exit-when=notidle set.
async fn multiple_exit_when_not_idle_commands(buck: &Buck, same_state: bool, tmp_path: &Path) {
    let event_log = tmp_path.join("first.json-lines");
    let a = buck
        .build(&[
            "-c",
            "foo.bar=1",
            "--exit-when=notidle",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
            "--event-log",
            event_log.to_str().unwrap(),
        ])
        .spawn();
    let a = wait_for_action_start(a, &event_log).await;

    let b = buck
        .build(&[
            "-c",
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            "--exit-when=notidle",
            ":long_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .spawn();

    let (done, pending) = wait_first_completed(vec![a, b], Duration::from_secs(10)).await;

    assert_eq!(done.len(), 1);
    assert_eq!(pending.len(), 1);

    for result in done {
        assert!(result.stderr.contains("daemon is busy"));
        assert_eq!(result.exit_code(), Some(4));
    }
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_multiple_exit_when_not_idle_commands_True(buck: &Buck, tmp_path: &Path) {
    multiple_exit_when_not_idle_commands(buck, true, tmp_path).await;
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_multiple_exit_when_not_idle_commands_False(buck: &Buck, tmp_path: &Path) {
    multiple_exit_when_not_idle_commands(buck, false, tmp_path).await;
}

/// Verify --exit-when=notidle succeeds after the previous command exits.
async fn exit_when_not_idle_after_command_exits(
    buck: &Buck,
    same_state: bool,
) -> anyhow::Result<()> {
    let a = buck.build(&[
        "-c",
        "foo.bar=1",
        ":short_running_target",
        "--local-only",
        "--no-remote-cache",
    ]);
    // Wait for the first command to actually finish. A fixed delay here made this test observe an
    // active command under load even though it intends to test the state after command exit.
    let result_a = tokio::time::timeout(Duration::from_secs(10), a).await??;

    let result_b = buck
        .build(&[
            "-c",
            if same_state { "foo.bar=1" } else { "foo.bar=2" },
            "--exit-when=notidle",
            ":short_running_target",
            "--local-only",
            "--no-remote-cache",
        ])
        .await?;

    assert!(!result_a.stderr.contains("daemon is busy"));
    assert_eq!(result_a.exit_code(), Some(0));
    assert!(!result_b.stderr.contains("daemon is busy"));
    assert_eq!(result_b.exit_code(), Some(0));
    Ok(())
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_not_idle_after_command_exits_True(buck: &Buck) -> anyhow::Result<()> {
    exit_when_not_idle_after_command_exits(buck, true).await
}

#[buck_test]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_exit_when_not_idle_after_command_exits_False(buck: &Buck) -> anyhow::Result<()> {
    exit_when_not_idle_after_command_exits(buck, false).await
}
