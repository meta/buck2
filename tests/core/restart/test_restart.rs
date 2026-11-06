/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::remove_output;

const TEST_DIGEST: &str = "76f7aea8c1fc400287312b9608ceb24848ba02ac:14";

#[buck_test]
async fn test_restart_requires_no_stdout(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&["//:stage0"])
        .env("FORCE_WANT_RESTART", "true")
        .await?;
    assert_eq!(res.stdout.matches("//:stage0").count(), 1);
    Ok(())
}

#[buck_test]
async fn test_restart(buck: &Buck) {
    // Normally shows once.
    let res = buck.targets(&["//:invalid"]).await.expect_failure();
    assert_eq!(res.stderr.matches("Unknown target `invalid`").count(), 1);

    // But if we force a restart...
    let res = buck
        .targets(&["//:invalid"])
        .env("FORCE_WANT_RESTART", "true")
        .await
        .expect_failure();
    assert_eq!(res.stderr.matches("Unknown target `invalid`").count(), 2);
}

#[buck_test(allow_soft_errors = true)]
async fn test_restart_materializer_corruption(buck: &Buck) -> anyhow::Result<()> {
    let stage1 = "//:stage1";
    let res = buck.build(&[stage1]).await?;
    let out = res.build_report()?.output_for_target(stage1)?;

    // Now we remove this file (which comes to us via RE)
    // Only way to get it back is by killing the materializer state.
    remove_output(&out)?;

    let res = buck.build(&["//:stage2"]).await?;
    assert!(res.stderr.contains("Your command will now restart"));
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_restart_cas_missing(buck: &Buck) -> anyhow::Result<()> {
    // Make sure Buck is not running.
    buck.kill().await?;

    // Start a daemon with the `src` file tombstoned. This means we cannot download it from RE.
    // This is just the hash of `src`.
    buck.build(&[])
        .env("BUCK2_TEST_TOMBSTONED_DIGESTS", TEST_DIGEST)
        .await?;

    // Now build //:stage2. Buck2 must try to download the file, fail, then
    // restart the daemon.
    let res = buck.build(&["//:stage2"]).await?;
    assert!(res.stderr.contains("Your command will now restart"));

    // TODO: We should also handle the case where the top-level artifact is what
    // fails to download (i.e. build stage1 here instead).
    Ok(())
}

/// Sends `signal` (as `kill` takes it) to the process `pid`, returning whether it was delivered.
async fn send_signal(signal: &str, pid: u64) -> anyhow::Result<bool> {
    Ok(tokio::process::Command::new("kill")
        .args([signal, &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .await?
        .success())
}

#[buck_test(allow_soft_errors = true, skip_for_os = ["windows"])]
async fn test_restart_forkserver_crash(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    buck.build(&[]).await?;

    // Kill its forkserver.
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    let forkserver_pid = status["forkserver_pid"]
        .as_u64()
        .expect("the daemon has a forkserver");
    anyhow::ensure!(
        send_signal("-KILL", forkserver_pid).await?,
        "Could not kill the forkserver {forkserver_pid}"
    );

    // Wait for its forkserver to exit.
    for _ in 0..10 {
        if !send_signal("-0", forkserver_pid).await? {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Now build a thing and check we restart
    let res = buck.build(&["//:stage2"]).await?;
    assert!(res.stderr.contains("Your command will now restart"));
    Ok(())
}

#[buck_test]
async fn test_restart_disabled(buck: &Buck) -> anyhow::Result<()> {
    // Ensure no daemon
    buck.kill().await?;

    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"[buck2]\nrestarter = false")?;

    let result = buck
        .build(&["//:stage2"])
        .env("BUCK2_TEST_TOMBSTONED_DIGESTS", TEST_DIGEST)
        .await
        .expect_failure();
    assert!(!result.stderr.contains("Your command will now restart"));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_trace_id(buck: &Buck) -> anyhow::Result<()> {
    let trace_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    // But if we force a restart...
    let res = buck
        .targets(&["//:invalid"])
        .env("FORCE_WANT_RESTART", "true")
        .env("BUCK_WRAPPER_UUID", trace_id)
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    assert_ne!(record["trace_id"], trace_id);
    assert_eq!(res.buck_build_id()?, record["trace_id"]);
    assert_eq!(record["restarted_trace_id"], trace_id);
    assert_eq!(record["should_restart"], false);
    Ok(())
}
