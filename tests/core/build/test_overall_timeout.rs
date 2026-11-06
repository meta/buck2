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

use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::replay_streaming_report;
use serde_json::Value;
use serde_json::json;

/// The one value of the object `value`.
#[track_caller]
fn single(value: &Value) -> &Value {
    match value
        .as_object()
        .map(|o| o.values().collect::<Vec<_>>())
        .as_deref()
    {
        Some(&[single]) => single,
        _ => panic!("Expected exactly one value in {value}"),
    }
}

fn read_build_report(path: &Path) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

#[buck_test]
async fn test_overall_timeout(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let build_report = tmp_path.join("build-report.json");

    buck.build(&[
        ":slow",
        "--overall-timeout",
        "1s",
        "--build-report",
        build_report.to_str().unwrap(),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Build timed out")
    .assert_exit_code(ExitCode::UserError);

    let build_report_json = read_build_report(&build_report)?;
    assert_eq!(build_report_json["success"], false);

    // One result.
    let result = single(&build_report_json["results"]);
    let configured = single(&result["configured"]);
    assert_eq!(
        configured["errors"][0]["error_tags"],
        json!(["BUILD_DEADLINE_EXPIRED"])
    );
    Ok(())
}

/// A timed-out output adds no line, so only the build-finished lines report it.
#[buck_test]
async fn test_overall_timeout_streaming_build_report(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.build(&[
        ":slow",
        "--overall-timeout",
        "1s",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Build timed out")
    .assert_exit_code(ExitCode::UserError);

    let reports = read_streaming_report(&streaming_report)?;
    // The configured target finishing, then the target.
    assert_eq!(reports.len(), 2, "Expected 2 lines, got {}", reports.len());
    let results = Value::Object(replay_streaming_report(&reports).into_iter().collect());
    let result = single(&results);
    let configured = single(&result["configured"]);
    assert_eq!(
        configured["errors"][0]["error_tags"],
        json!(["BUILD_DEADLINE_EXPIRED"])
    );
    Ok(())
}

#[buck_test]
async fn test_overall_timeout_with_artifact_path_sketch(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    // Artifact path sketching runs *after* the build and re-`ensure_artifact_group`s every
    // top-level output. When the deadline fires mid-build, that post-build re-ensure must not
    // rebuild the action the timeout just cancelled: otherwise the command runs unbounded past
    // its `--overall-timeout`. The `:slow` action sleeps for far longer than the deadline, so a
    // regression would re-run it post-deadline and hang. Bound the wall clock so that failure is
    // fast and legible instead of a multi-minute hang.
    let build_report = tmp_path.join("build-report.json");

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        buck.build(&[
            ":slow",
            "--overall-timeout",
            "1s",
            "-c",
            "buck2.log_artifact_size_sketch=true",
            "--build-report",
            build_report.to_str().unwrap(),
        ]),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "`--overall-timeout` did not stop the build within 60s with artifact path sketching enabled: a post-build phase is re-running an action the deadline already cancelled."
        )
    });
    result
        .expect_failure()
        .assert_stderr_matches("Build timed out")
        .assert_exit_code(ExitCode::UserError);

    let build_report_json = read_build_report(&build_report)?;
    assert_eq!(build_report_json["success"], false);

    let result = single(&build_report_json["results"]);
    let configured = single(&result["configured"]);
    assert_eq!(
        configured["errors"][0]["error_tags"],
        json!(["BUILD_DEADLINE_EXPIRED"])
    );
    Ok(())
}
