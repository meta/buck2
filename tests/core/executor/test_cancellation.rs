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

use buck2_e2e::BuckCommand;
use buck2_e2e::InvocationRecord;
use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;

/// This test starts a test that writes its PID to a file then runs for 60
/// seconds. We test cancellation by sending a CTRL+C as soon as a test
/// starts. We then check that the process exited, and that nothing else
/// started (or if anything did, that they stopped).
async fn test_cancellation_helper(
    buck: &Buck,
    tmp_path: &Path,
    runner: impl for<'a> Fn(&'a Buck, &[&str]) -> BuckCommand<'a>,
) -> anyhow::Result<()> {
    let pid_path = tmp_path.join("pids");
    std::fs::create_dir(&pid_path)?;
    let record_path = tmp_path.join("record.json");
    let pids_config = format!("test.pids={}", pid_path.display());
    let opts = [
        "-c",
        &pids_config,
        "-c",
        "test.duration=60",
        "--unstable-write-invocation-record",
        record_path.to_str().unwrap(),
    ];
    buck.audit(&[&["providers", ":slow"][..], &opts].concat())
        .await?;
    let command = runner(buck, &[&opts[..], &["--local-only"]].concat()).spawn();

    let mut started = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if std::fs::read_dir(&pid_path)?.next().is_some() {
            started = true;
            break;
        }
    }
    assert!(started, "Commands never started");

    // Wait for the command to exit
    let _ignored = command.interrupt().await;

    // Give stuff time to settle, PIDS don't necessarily disappear
    // instantly. Also, verify that we are not starting more tests.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // At this point, nothing should be alive.
    for entry in std::fs::read_dir(&pid_path)? {
        let pid = entry?.file_name().to_string_lossy().into_owned();
        assert!(!daemon_is_alive(pid.parse()?), "PID existed: {pid}");
    }

    let record = InvocationRecord::read(&record_path)?;
    assert_eq!(record["exit_code"], ExitCode::SignalInterrupt as i32);
    assert_eq!(record["exit_result_name"], "SIGNAL_INTERRUPT");
    Ok(())
}

fn build_slow<'a>(buck: &'a Buck, opts: &[&str]) -> BuckCommand<'a> {
    buck.build(&[opts, &[":slow"]].concat())
}

fn bxl_build<'a>(buck: &'a Buck, opts: &[&str]) -> BuckCommand<'a> {
    buck.bxl(&[opts, &["//build.bxl:build"]].concat())
}

#[buck_test]
async fn test_cancellation(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_cancellation_helper(buck, tmp_path, build_slow).await
}

#[buck_test]
async fn test_cancellation_bxl(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_cancellation_helper(buck, tmp_path, bxl_build).await
}
