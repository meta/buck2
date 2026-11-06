/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The daemon must survive its clients going away. The fixture fails any test whose daemon
//! panicked, so these only check that the same daemon is still running.

use std::time::Duration;

use anyhow::Context;
use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;
use serde_json::Value;

/// Generous, so the client has exited before the stall ends even on a slow CI machine.
const STALL_MS: &str = "2000";

async fn daemon_pid(buck: &Buck) -> anyhow::Result<u32> {
    let status: Value = serde_json::from_str(&buck.server(&["--status"]).await?.stdout)?;
    let pid = status
        .get("process_info")
        .and_then(|info| info.get("pid"))
        .and_then(Value::as_u64)
        .with_context(|| format!("No `process_info.pid` in {status}"))?;
    Ok(u32::try_from(pid)?)
}

/// Whether `buck2 status` lists any active command.
async fn has_active_commands(buck: &Buck) -> anyhow::Result<bool> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    Ok(!status
        .get("active_commands")
        .and_then(Value::as_array)
        .with_context(|| format!("No `active_commands` in {status}"))?
        .is_empty())
}

/// Waits for the daemon to list the command of `client`, which is taken if it exits first.
async fn wait_for_active_command(
    buck: &Buck,
    client: &mut Option<BuckProcess>,
) -> anyhow::Result<()> {
    for _ in 0..600 {
        if has_active_commands(buck).await? {
            return Ok(());
        }
        let running = client.as_mut().context("The client was waited for")?;
        if running.try_wait().is_some() {
            let output = match client
                .take()
                .context("The client was waited for")?
                .wait()
                .await
            {
                Ok(output) => output,
                Err(error) => error.into_output(),
            };
            anyhow::bail!(
                "The client exited before its command was registered:\n{}",
                output.stderr
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("The command never showed up in active_commands")
}

/// The client exits as soon as it has its result, which cancels the command's task on the daemon
/// while that task may still be finishing. The daemon once mistook that for a cancelled command
/// and crashed (D129074969). The stall makes the test hit that window every time.
#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_TEST_COMMAND_END_STALL_MS", STALL_MS)],
)]
async fn test_client_leaving_with_its_result(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let pid = daemon_pid(buck).await?;

    buck.targets(&["//:rule"]).await?;
    tokio::time::sleep(Duration::from_millis(STALL_MS.parse()?) + Duration::from_secs(1)).await;

    assert!(
        daemon_is_alive(pid),
        "The daemon died after the client left with its result"
    );
    assert_eq!(daemon_pid(buck).await?, pid);
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data")]
async fn test_client_killed_while_its_command_runs(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let pid = daemon_pid(buck).await?;

    let mut client = Some(
        buck.build(&["//:long_running", "--local-only", "--no-remote-cache"])
            .spawn(),
    );
    let registered = wait_for_active_command(buck, &mut client).await;
    if let Some(client) = client.take() {
        let _ignored = client.kill().await;
    }
    registered?;

    let mut cancelled = false;
    for _ in 0..100 {
        if !has_active_commands(buck).await? {
            cancelled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(cancelled, "The killed client's command was not cancelled");

    assert!(
        daemon_is_alive(pid),
        "The daemon died when its client was killed"
    );
    assert_eq!(daemon_pid(buck).await?, pid);
    buck.targets(&["//:rule"]).await?;
    Ok(())
}
