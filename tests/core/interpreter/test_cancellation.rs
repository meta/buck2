/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_e2e::prelude::*;
use serde_json::Value;

/// Whether the daemon runs a command, as `buck2 status` lists it; not while there is no daemon.
async fn has_active_command(buck: &Buck) -> bool {
    let Ok(output) = buck.status(&[]).await else {
        return false;
    };
    serde_json::from_str::<Value>(&output.stdout)
        .ok()
        .and_then(|status| {
            status
                .get("active_commands")?
                .as_array()
                .map(|a| !a.is_empty())
        })
        .unwrap_or(false)
}

/// Waits until the daemon runs a command.
async fn wait_for_active_command(buck: &Buck) -> anyhow::Result<()> {
    for _ in 0..1200 {
        if has_active_command(buck).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("The first command never showed up in `active_commands`")
}

async fn test_cancellation_impl(buck: &Buck, stage: &str) -> anyhow::Result<()> {
    let loop_config = format!("should.loop={stage}");
    let aproc = match stage {
        "bxl" => buck.bxl(&[
            "--preemptible=always",
            "--config",
            "should.loop=bxl",
            "//:root.bxl:loop_test",
        ]),
        "streaming_targets" => buck.targets(&[
            "--streaming",
            "--no-cache",
            "--preemptible=always",
            "--config",
            "should.loop=load",
            ":target",
        ]),
        _ => buck.build(&["--preemptible=always", "--config", &loop_config, ":target"]),
    };

    // The second command must start while the first one runs.
    let aproc = aproc.spawn();
    wait_for_active_command(buck).await?;

    let b = buck.build(&[":target"]).await?;
    let a = match aproc.wait().await {
        Ok(_) => panic!("Expected to be preempted, but command completed successfully"),
        Err(e) => e.into_output(),
    };

    // Note: one of the important features being tested here is not that a is
    // preempted (that already happens), but that b is run quickly afterwards
    // (IE: that the work a was doing was cancelled). That requires running both
    // commands with separate configurations, so that we exercise the wait for
    // the other config work to finish before dropping the existing dice path. If
    // the first command preempts but fails to short circuit the starlark
    // evaluation, the second will get stuck waiting for the first to finish, and
    // this test will time out.
    let preempted = a.stderr.contains("buck daemon preempted this command");
    if a.exit_code() == Some(0) || !preempted {
        eprintln!("A STDERR:\n {}\n\nA STDOUT:\n {}", a.stdout, a.stderr);
        eprintln!("B STDERR:\n {}\n\nB STDOUT:\n {}", b.stdout, b.stderr);
    }
    assert_ne!(a.exit_code(), Some(0));
    assert!(preempted);
    Ok(())
}

#[buck_test]
async fn test_cancellation_load(buck: &Buck) -> anyhow::Result<()> {
    test_cancellation_impl(buck, "load").await
}

#[buck_test]
async fn test_cancellation_package(buck: &Buck) -> anyhow::Result<()> {
    test_cancellation_impl(buck, "package").await
}

#[buck_test]
async fn test_cancellation_analysis(buck: &Buck) -> anyhow::Result<()> {
    test_cancellation_impl(buck, "analysis").await
}

#[buck_test]
async fn test_cancellation_bxl(buck: &Buck) -> anyhow::Result<()> {
    test_cancellation_impl(buck, "bxl").await
}

#[buck_test]
async fn test_cancellation_streaming_targets(buck: &Buck) -> anyhow::Result<()> {
    test_cancellation_impl(buck, "streaming_targets").await
}
