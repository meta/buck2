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
use buck2_e2e::utils::json_get;
use serde_json::Value;
use serde_json::json;

async fn command_execution(buck: &Buck) -> anyhow::Result<Value> {
    let raw_log = buck.log(&["show"]).await?.stdout;
    for line in raw_log.trim().lines() {
        let event: Value = serde_json::from_str(line)?;
        let commands = json_get(
            &event,
            &[
                "Event",
                "data",
                "SpanEnd",
                "data",
                "ActionExecution",
                "commands",
            ],
        );
        if let Some(command) = commands.and_then(Value::as_array).and_then(|c| c.last()) {
            return Ok(command.clone());
        }
    }
    anyhow::bail!("Did not find a command execution in the event log:\n{raw_log}")
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_missing_test_binary_reports_missing_materialized_input(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The first run builds and materializes the test binary, and the test passes.
    buck.test(&["//:test_binary", "--local-only", "--no-remote-cache"])
        .await?;
    let result = buck.build(&["//:test_binary"]).await?;
    let binary = result.build_report()?.output_for_target("//:test_binary")?;

    std::fs::remove_file(&binary)?;

    // Nothing is rebuilt because Buck still believes the binary is on disk, so
    // the test command fails to spawn.
    buck.test(&["//:test_binary", "--local-only", "--no-remote-cache"])
        .await
        .expect_failure();
    let test_runs = filter_events(buck, &["Event", "data", "SpanEnd", "data", "TestRun"]).await?;
    let last_run = test_runs
        .last()
        .expect("Expected a TestRun span for the second run");
    let status = &last_run["command_report"]["status"];
    assert_eq!(
        *status,
        json!({"Failure": {"missing_materialized_inputs": true}}),
        "{status}"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_missing_undeclared_executable_is_not_a_missing_input(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&[
        "//:missing_undeclared_executable",
        "--local-only",
        "--no-remote-cache",
    ])
    .await
    .expect_failure();
    let command = command_execution(buck).await?;
    assert_eq!(command["status"], json!({"Failure": {}}), "{command}");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_ordinary_failure_json_stays_empty(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:ordinary_failure", "--local-only", "--no-remote-cache"])
        .await
        .expect_failure();
    let command = command_execution(buck).await?;
    assert_eq!(command["status"], json!({"Failure": {}}));
    Ok(())
}
