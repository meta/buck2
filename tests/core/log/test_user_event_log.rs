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

use anyhow::Context;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use regex::Regex;
use serde_json::Value;

fn replace_timestamp(s: &str) -> String {
    Regex::new(r"\b[0-9]+\b")
        .unwrap()
        .replace_all(s, "<NUMBER>")
        .into_owned()
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_user_event_log_custom_output(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let local_log = tmp_path.join("test.json");

    buck.bxl(&[
        "root//:test.bxl:instant_event",
        "--user-event-log",
        local_log.to_str().unwrap(),
    ])
    .await?;

    assert!(local_log.exists());

    // do some basic validation - golden tests take care of better validation
    let results = std::fs::read_to_string(&local_log)?;
    let results: Vec<&str> = results.lines().collect();
    // assert these events can be loaded
    let event = |i: usize| -> anyhow::Result<Value> { Ok(serde_json::from_str(results[i])?) };
    assert!(event(0)?.get("command_line_args").is_some());
    assert!(event(1)?.get("StarlarkUserEvent").is_some());
    assert!(event(2)?.get("StarlarkUserEvent").is_some());
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_user_event_log_with_actions(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let local_log = tmp_path.join("test.json-lines");

    buck.bxl(&[
        "root//:test.bxl:action",
        "--event-log",
        local_log.to_str().unwrap(),
    ])
    .await?;

    let results = buck
        .log(&[
            "show-user",
            std::path::absolute(&local_log)?.to_str().unwrap(),
        ])
        .await?
        .stdout;
    let results: Vec<&str> = results.trim().lines().skip(1).collect();

    // Remove any durations
    let mut a = Json::loads(results[0])?;
    let action = a
        .get_mut("ActionExecutionEvent")
        .context("No `ActionExecutionEvent`")?;
    action.set("duration_millis", "<NUMBER>");
    action.set("input_materialization_duration_millis", "<NUMBER>");
    let mut b = Json::loads(results[1])?;
    b.get_mut("BxlEnsureArtifactsEvent")
        .context("No `BxlEnsureArtifactsEvent`")?
        .set("duration_millis", "<NUMBER>");

    let results = replace_timestamp(&format!("{a}\n{b}"));

    // Just validate the user events, let's skip the invocation record
    golden(&results, "action_event.golden.json");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_user_event_with_log_show_user(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["root//:test.bxl:instant_event"]).await?;

    let results = buck.log(&["show-user"]).await?.stdout;
    let results: Vec<&str> = results.trim().lines().skip(1).collect();

    let results = replace_timestamp(&results.join("\n"));

    // Just validate the user events, let's skip the invocation record
    golden(&results, "instant_event.golden.json");
    Ok(())
}

async fn test_user_event_log_with_log_show_user_compatibility(
    buck: &Buck,
    tmp_path: &Path,
    file_extension: &str,
) -> anyhow::Result<()> {
    let local_log = tmp_path.join(format!("test.{file_extension}"));

    buck.bxl(&[
        "root//:test.bxl:instant_event",
        "--event-log",
        local_log.to_str().unwrap(),
    ])
    .await?;

    let results = buck
        .log(&[
            "show-user",
            std::path::absolute(&local_log)?.to_str().unwrap(),
        ])
        .await?
        .stdout;
    let results: Vec<&str> = results.trim().lines().skip(1).collect();

    let results = replace_timestamp(&results.join("\n"));

    // Just validate the user events, let's skip the invocation record
    golden(&results, "instant_event.golden.json");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_user_event_log_with_log_show_user_compatibility_json_lines(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_user_event_log_with_log_show_user_compatibility(buck, tmp_path, ".json-lines").await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_user_event_log_with_log_show_user_compatibility_json_lines_gz(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_user_event_log_with_log_show_user_compatibility(buck, tmp_path, ".json-lines.gz").await
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_user_event_log_with_log_show_user_compatibility_json_lines_zst(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_user_event_log_with_log_show_user_compatibility(buck, tmp_path, ".json-lines.zst").await
}

// Placeholder for tests to be listed successfully on Windows.
#[buck_test]
async fn test_noop(_buck: &Buck) {}
