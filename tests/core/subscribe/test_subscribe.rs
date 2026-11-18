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

use anyhow::Context;
use buck2_e2e::prelude::*;

// Length-prefixed protobuf frame for:
// `SubscriptionRequest { subscribe_to_active_commands: SubscribeToActiveCommands {} }`.
// This test intentionally uses the raw frame to verify stdin requests keep the
// daemon alive without going through the CLI's `--active-commands` helper. The
// wire shape is stable enough for this test: the subscription API is part of
// Buck2's client/daemon protocol, and the existing field number for
// `subscribe_to_active_commands` must remain backward-compatible.
const SUBSCRIBE_TO_ACTIVE_COMMANDS_REQUEST: &[u8] = b"\x02\x22\x00";

async fn daemon_pid(buck: &Buck) -> anyhow::Result<serde_json::Value> {
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    status
        .pointer("/process_info/pid")
        .filter(|pid| pid.is_u64())
        .cloned()
        .context("No `process_info.pid` in `buck2 status`")
}

#[buck_test]
async fn test_active_commands(buck: &Buck) -> anyhow::Result<()> {
    let mut subscribe = buck.subscribe(&["--active-commands"]);
    let msg = subscribe.read_message().await?;
    let commands = msg["response"]["ActiveCommandsSnapshot"]["active_commands"]
        .as_array()
        .expect("a list of active commands");
    assert_eq!(commands.len(), 1, "{msg}");
    assert!(
        commands[0]["argv"]
            .as_array()
            .expect("a list of arguments")
            .iter()
            .any(|a| a == "subscribe"),
        "{msg}"
    );
    subscribe.close().await?;
    Ok(())
}

#[buck_test]
async fn test_disconnect_eof(buck: &Buck) -> anyhow::Result<()> {
    let mut subscribe = buck.subscribe(&[]);
    subscribe.process().close_stdin();
    let msg = subscribe.read_message().await?;
    assert!(
        msg["response"]["Goodbye"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("EOF")),
        "{msg}"
    );
    subscribe.close().await?;
    Ok(())
}

#[buck_test(env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")])]
async fn test_requests_keep_daemon_alive(buck: &Buck) -> anyhow::Result<()> {
    let mut subscribe = buck.subscribe(&[]);
    subscribe
        .process()
        .write_stdin(SUBSCRIBE_TO_ACTIVE_COMMANDS_REQUEST)
        .await?;
    subscribe.read_message().await?;

    let pid = daemon_pid(buck).await?;

    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(600)).await;
        subscribe
            .process()
            .write_stdin(SUBSCRIBE_TO_ACTIVE_COMMANDS_REQUEST)
            .await?;
        subscribe.read_message().await?;
    }

    assert_eq!(daemon_pid(buck).await?, pid);
    assert!(subscribe.process().try_wait().is_none());
    subscribe.close().await?;
    Ok(())
}

#[buck_test]
async fn test_disconnect_error(buck: &Buck) -> anyhow::Result<()> {
    let mut subscribe = buck.subscribe(&[]);
    subscribe.process().write_stdin(b"x").await?;
    subscribe.process().close_stdin();
    let msg = subscribe.read_message().await?;
    assert!(
        msg["response"]["Goodbye"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Error parsing request")),
        "{msg}"
    );
    subscribe.close().await.expect_failure();
    Ok(())
}

#[buck_test]
async fn test_unknown_request_error(buck: &Buck) -> anyhow::Result<()> {
    let mut subscribe = buck.subscribe(&[]);
    subscribe.process().write_stdin(b"\x00").await?; // Would decode to a None request
    subscribe.close().await.expect_failure();
    Ok(())
}
