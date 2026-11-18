/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![expect(
    non_snake_case,
    reason = "The tests keep the names the Python harness gave the parametrized cases, `True` and `False` included"
)]

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

async fn test_audit_visibility(buck: &Buck, rule: &str, passes: bool) -> anyhow::Result<()> {
    if passes {
        let out = buck.audit_visibility(&[rule]).await?;
        assert_eq!(out.stdout, "");
    } else {
        buck.audit_visibility(&[rule])
            .await
            .expect_failure()
            .assert_stderr_matches(&format!("not visible to `{rule}`"));
    }
    Ok(())
}

#[buck_test]
async fn test_audit_visibility_self_pass1_True(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:pass1", true).await
}

#[buck_test]
async fn test_audit_visibility_self_pass2_True(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:pass2", true).await
}

#[buck_test]
async fn test_audit_visibility_self_pass3_True(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:pass3", true).await
}

#[buck_test]
async fn test_audit_visibility_self_pass4_True(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:pass4", true).await
}

#[buck_test]
async fn test_audit_visibility_self_fail1_False(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:fail1", false).await
}

#[buck_test]
async fn test_audit_visibility_self_fail2_False(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:fail2", false).await
}

#[buck_test]
async fn test_audit_visibility_self_fail3_False(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:fail3", false).await
}

#[buck_test]
async fn test_audit_visibility_self_fail4_False(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:fail4", false).await
}

#[buck_test]
async fn test_audit_visibility_self_fail5_False(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:fail5", false).await
}

#[buck_test]
async fn test_audit_visibility_self_fail6_False(buck: &Buck) -> anyhow::Result<()> {
    test_audit_visibility(buck, "self//:fail6", false).await
}

/// The command sends its errors as console messages. What it writes to the daemon's own stderr
/// reaches the client through a file that the client polls, which can split a line in two.
#[buck_test]
async fn test_audit_visibility_errors_are_console_messages(buck: &Buck) -> anyhow::Result<()> {
    buck.audit_visibility(&["self//:fail2"])
        .await
        .expect_failure();
    let messages = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "ConsoleMessage",
            "message",
        ],
    )
    .await?;
    let error = "`self//subdir:badpackage` is not visible to `self//:fail2`";
    assert!(
        messages
            .iter()
            .filter_map(|message| message.as_str())
            .flat_map(str::lines)
            .any(|line| line.starts_with(error)),
        "No console message has a line starting with {error}: {messages:?}"
    );
    Ok(())
}
