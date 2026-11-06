/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::golden::sanitize_build_report;
use buck2_e2e::golden::sanitize_hashes;
use buck2_e2e::golden::sanitize_python;
#[cfg(not(any(windows, target_os = "macos")))]
use buck2_e2e::golden::sanitize_stderr;
#[cfg(not(any(windows, target_os = "macos")))]
use buck2_e2e::golden::strip_waiting_on;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::replay_streaming_report;
use serde_json::Value;
use serde_json::json;

async fn build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let mut args = vec![
        "--build-report",
        report.to_str().unwrap(),
        "--build-report-options",
        "fill-out-failures",
    ];
    args.extend(command);
    buck.build(&args).await.expect_failure();
    let mut report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    sanitize_build_report(&mut report);

    golden(
        &sanitize_hashes(&sanitize_python(
            &Json::from(report.clone()).dumps(Some(2), true),
            buck.cwd(),
        )),
        &format!("fixtures/{name}.golden.json"),
    );
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn sanitize_error_stderr(stderr: &str, buck: &Buck) -> String {
    strip_waiting_on(&sanitize_stderr(&sanitize_python(stderr, buck.cwd())))
}

const ONE_OF_EACH: [&str; 6] = [
    "//success:success",
    "//missing:missing",
    "//fail_load:first",
    "//fail_config:cat_only",
    "//fail_analysis:fail_analysis",
    "//fail_action:fail_one",
];

type Outcome = (Value, usize, BTreeMap<String, (Value, usize)>);

fn configured_of(entry: &Value) -> &serde_json::Map<String, Value> {
    entry["configured"]
        .as_object()
        .unwrap_or_else(|| panic!("No `configured` in {entry}"))
}

fn error_count(entry: &Value) -> usize {
    entry["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("No `errors` in {entry}"))
        .len()
}

fn results_of(report: &Value) -> &serde_json::Map<String, Value> {
    report["results"]
        .as_object()
        .unwrap_or_else(|| panic!("No `results` in {report}"))
}

/// Each entry's status and error count, and those of each of its configurations.
fn outcomes<'a>(
    results: impl IntoIterator<Item = (&'a String, &'a Value)>,
) -> BTreeMap<String, Outcome> {
    results
        .into_iter()
        .map(|(key, entry)| {
            (
                key.clone(),
                (
                    entry["success"].clone(),
                    error_count(entry),
                    configured_of(entry)
                        .iter()
                        .map(|(cfg, configured)| {
                            (
                                cfg.clone(),
                                (configured["success"].clone(), error_count(configured)),
                            )
                        })
                        .collect(),
                ),
            )
        })
        .collect()
}

/// Each entry's status, keyed `(key, None)`, and each configuration's, `(key, cfg)`.
fn statuses(results: &serde_json::Map<String, Value>) -> BTreeMap<(String, Option<String>), Value> {
    results
        .iter()
        .flat_map(|(key, entry)| {
            std::iter::once(((key.clone(), None), entry["success"].clone())).chain(
                configured_of(entry).iter().map(|(cfg, configured)| {
                    (
                        (key.clone(), Some(cfg.clone())),
                        configured["success"].clone(),
                    )
                }),
            )
        })
        .collect()
}

/// The string cache key of an error's message.
fn message_key(error: &Value) -> &str {
    error["message_content"]
        .as_str()
        .unwrap_or_else(|| panic!("No `message_content` in {error}"))
}

#[buck_test]
async fn test_action_fail_one(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_one",
        &["//fail_action:fail_one"],
    )
    .await
}

#[buck_test]
async fn test_action_fail_two(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_two",
        &["//fail_action:fail_two"],
    )
    .await
}

#[buck_test]
async fn test_action_fail_shared_dep(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_shared_dep",
        &["//fail_action:fail_shared_dep"],
    )
    .await
}

#[buck_test]
async fn test_action_fail_shared_across_targets(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_shared_across_targets",
        &["//fail_action:alias_a", "//fail_action:alias_b"],
    )
    .await
}

#[buck_test]
async fn test_analysis_fail(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_analysis_fail",
        &["//fail_analysis:fail_analysis"],
    )
    .await
}

#[buck_test]
async fn test_analysis_fail_multi(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_analysis_fail_multi",
        &["//fail_analysis:fail_analysis", "//fail_analysis:alias"],
    )
    .await
}

#[buck_test]
async fn test_analysis_and_action(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_analysis_and_action",
        &["//fail_analysis:fail_analysis", "//fail_action:fail_one"],
    )
    .await
}

#[buck_test]
async fn test_configuration_fail(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_configuration_fail",
        &["//fail_config:cat_only"],
    )
    .await
}

#[buck_test]
async fn test_missing(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(buck, tmp_path, "test_missing", &["//missing:missing"]).await
}

#[buck_test]
async fn test_load_fail(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_load_fail",
        &["//fail_load:first", "//fail_load:second"],
    )
    .await
}

// This does not show up in the build report, because there's nowhere we could put the error
#[buck_test]
async fn test_load_fail_full_package(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_load_fail_full_package",
        &["//fail_load:"],
    )
    .await
}

#[buck_test]
async fn test_partially_missing(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_partially_missing",
        &["//missing:missing", "//missing:available"],
    )
    .await
}

#[buck_test]
async fn test_one_of_each(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(buck, tmp_path, "test_one_of_each", &ONE_OF_EACH).await
}

#[buck_test]
async fn test_no_terminal_colors(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_no_terminal_colors",
        &["//terminal_colors:terminal_colors"],
    )
    .await
}

// TODO fix on windows and mac
#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_with_stdout_stderr(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_with_stdout_stderr",
        &["//fail_action:fail_script"],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_infra(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_infra",
        &["//fail_action:fail_infra"],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_one_with_error_handler(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_one_with_error_handler",
        &["//fail_action:fail_one_with_error_handler"],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_many_with_error_handler(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_many_with_error_handler",
        &["//fail_action:fail_many_with_error_handler"],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_one_with_error_handler_no_op(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_one_with_error_handler_no_op",
        &["//fail_action:fail_one_with_error_handler_no_op"],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_stderr_with_empty_error_diagnostics(buck: &Buck) {
    let result = buck
        .build(&["//fail_action:fail_one_with_error_handler_no_op"])
        .await
        .expect_failure();

    golden(
        &sanitize_error_stderr(&result.stderr, buck),
        "fixtures/test_stderr_with_empty_error_diagnostics.golden.txt",
    );
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_stderr_with_error_diagnostics(buck: &Buck) {
    let result = buck
        .build(&["//fail_action:error_handler_produced_multiple_categories"])
        .await
        .expect_failure();

    golden(
        &sanitize_error_stderr(&result.stderr, buck),
        "fixtures/test_stderr_with_error_diagnostics.golden.txt",
    );
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_stderr_with_no_error_diagnostics(buck: &Buck) {
    let result = buck
        .build(&["//fail_action:fail_script"])
        .await
        .expect_failure();

    golden(
        &sanitize_error_stderr(&result.stderr, buck),
        "fixtures/test_stderr_with_no_error_diagnostics.golden.txt",
    );
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_stderr_could_not_produce_error_diagnostics(buck: &Buck) {
    let result = buck
        .build(&["//fail_action:error_handler_failed"])
        .await
        .expect_failure();

    golden(
        &sanitize_error_stderr(&result.stderr, buck),
        "fixtures/test_stderr_could_not_produce_error_diagnostics.golden.txt",
    );
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_error_handler_with_output_remote_only(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_error_handler_with_output_remote_only",
        &[
            "//fail_action:fail_error_handler_with_output",
            "--remote-only",
        ],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_error_handler_with_output_local_only(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_error_handler_with_output_local_only",
        &[
            "//fail_action:fail_error_handler_with_output",
            "--local-only",
        ],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_error_handler_with_output_content_based_path_remote_only(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_error_handler_with_output_content_based_path_remote_only",
        &[
            "//fail_action:fail_error_handler_with_output",
            "--remote-only",
            "-c",
            "test.use_content_based_path=true",
        ],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_error_handler_with_output_content_based_path_local_only(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_error_handler_with_output_content_based_path_local_only",
        &[
            "//fail_action:fail_error_handler_with_output",
            "--local-only",
            "-c",
            "test.use_content_based_path=true",
        ],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_error_handler_output_not_written_remote_only(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_error_handler_output_not_written_remote_only",
        &[
            "//fail_action:fail_error_handler_output_not_written",
            "--remote-only",
        ],
    )
    .await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_action_fail_error_handler_output_not_written_local_only(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_action_fail_error_handler_output_not_written_local_only",
        &[
            "//fail_action:fail_error_handler_output_not_written",
            "--local-only",
        ],
    )
    .await
}

#[buck_test]
async fn test_streaming_one_of_each(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    let mut args = vec![
        "--build-report",
        report.to_str().unwrap(),
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ];
    args.extend(ONE_OF_EACH);
    buck.build(&args).await.expect_failure();

    let reports = read_streaming_report(&streaming_report)?;
    // One line for the output of `//fail_action:fail_one`, one each when the 3 targets that
    // configure finish their configured build, and one each when the 6 targets finish.
    assert_eq!(
        reports.len(),
        10,
        "Expected 10 lines, got {}",
        reports.len()
    );
    for line in &reports {
        let results = results_of(line);
        assert_eq!(
            results.len(),
            1,
            "Expected one target per line, got {:?}",
            results.keys().collect::<Vec<_>>()
        );
    }
    let final_json: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;
    let final_results = results_of(&final_json);
    assert_eq!(
        outcomes(&replay_streaming_report(&reports)),
        outcomes(final_results)
    );

    let failed: BTreeSet<&String> = final_results
        .iter()
        .filter(|(_, entry)| entry["success"] == "FAIL")
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        failed.len(),
        ONE_OF_EACH.len() - 1,
        "Expected every target but `//success:success` to fail, got {failed:?}"
    );
    let streamed_failures: BTreeSet<&String> = reports
        .iter()
        .flat_map(results_of)
        .filter(|(_, entry)| entry["success"] == "FAIL")
        .map(|(key, _)| key)
        .collect();
    assert_eq!(streamed_failures, failed);

    // `//success:success` has no outputs. Its configured target always finishes before the
    // target does, so one line reads the configuration SUCCESS and the target IN_PROGRESS.
    let success_history: Vec<(Value, Vec<Value>)> = reports
        .iter()
        .filter_map(|report| report["results"].get("root//success:success"))
        .map(|entry| {
            (
                entry["success"].clone(),
                configured_of(entry)
                    .values()
                    .map(|c| c["success"].clone())
                    .collect(),
            )
        })
        .collect();
    assert!(
        success_history.contains(&(json!("IN_PROGRESS"), vec![json!("SUCCESS")])),
        "{success_history:?}"
    );
    assert_eq!(
        success_history.last(),
        Some(&(json!("SUCCESS"), vec![json!("SUCCESS")])),
        "{success_history:?}"
    );

    let failed_statuses: BTreeSet<(String, Option<String>)> = statuses(final_results)
        .into_iter()
        .filter(|(_, status)| *status == "FAIL")
        .map(|(key, _)| key)
        .collect();
    let read_success_before_failing: BTreeSet<(String, Option<String>)> = reports
        .iter()
        .flat_map(|report| statuses(results_of(report)))
        .filter(|(key, status)| failed_statuses.contains(key) && *status == "SUCCESS")
        .map(|(key, _)| key)
        .collect();
    assert_eq!(read_success_before_failing, BTreeSet::new());
    Ok(())
}

/// `//success:success` has no outputs, so only its build-finished lines report it.
#[buck_test]
async fn test_streaming_target_without_outputs(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.build(&[
        "//success:success",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    // The configured target finishing, then the target.
    assert_eq!(reports.len(), 2, "Expected 2 lines, got {}", reports.len());
    let results = replay_streaming_report(&reports);
    assert_eq!(results["root//success:success"]["success"], "SUCCESS");
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_two_action_dep_failures(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // When we pass `--keep-going`, we should get error reports for both dependencies of the action.
    // However, we don't. Instead, we just get one error non-deterministically. This is also why we
    // can't use a `build_report_test` for this test.
    let report = tmp_path.join("build-report.json");
    buck.build(&[
        "--keep-going",
        "--build-report",
        report.to_str().unwrap(),
        "//fail_action:fail_two_deps",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//fail_action:fail_two_deps");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;
    let errors = configured_of(&report["results"]["root//fail_action:fail_two_deps"])
        .values()
        .next()
        .context("No configured entry")?["errors"]
        .as_array()
        .context("No `errors`")?;
    assert_eq!(errors.len(), 1);

    let strings_cache = &report["strings"];
    let error_message_index = message_key(&errors[0]);

    assert!(
        strings_cache[error_message_index].as_str().is_some_and(
            |message| message.starts_with("Action failed: root//fail_action:fail_two_deps")
        ),
        "{}",
        strings_cache[error_message_index]
    );
    assert!(
        errors[0]["action_error"]["key"]["owner"]
            .as_str()
            .is_some_and(|owner| owner.contains("fail_two_deps"))
    );
    Ok(())
}

#[buck_test]
async fn test_error_handler_failed(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Starlark error messages change across different modes for some reason (ex: opt-asan vs opt).
    // We have a fair amount of coverage for other functionalities of error handler/build report,
    // so let's just add a simple test here.
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "--build-report",
        report.to_str().unwrap(),
        "//fail_action:error_handler_failed",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//fail_action:error_handler_failed");

    let report = std::fs::read_to_string(&report)?;

    assert!(report.contains("Error handler failed"));
    assert!(report.contains("fail: something went wrong"));
    Ok(())
}

#[buck_test]
async fn test_error_handler_wrong_return_type(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Starlark error messages change across different modes for some reason (ex: opt-asan vs opt).
    // We have a fair amount of coverage for other functionalities of error handler/build report,
    // so let's just add a simple test here.
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "--build-report",
        report.to_str().unwrap(),
        "//fail_action:error_handler_wrong_return_type",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//fail_action:error_handler_wrong_return_type");

    let report = std::fs::read_to_string(&report)?;

    assert!(report.contains("Error handler failed"));
    assert!(
        report.contains("Expected return type `list[ActionSubError]`, got value with type `int`")
    );
    Ok(())
}

#[buck_test]
async fn test_missing_report_on_wrong_package(buck: &Buck, tmp_path: &Path) {
    // If we specify a non-existent package, we don't get an error report
    let report = tmp_path.join("build-report.json");
    buck.build(&[
        "--build-report",
        report.to_str().unwrap(),
        "//nopackage/...",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Error resolving recursive target pattern");
    assert!(!report.exists(), "Expected no report to be written");
}

#[buck_test]
async fn test_streaming_fail_fast_streams_the_stopping_error(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    // The analysis error stops the build before any output exists, so no output line can carry it.
    // `//slow:slow` keeps the build running: if the build ended on its own, `--fail-fast` would not
    // stop it.
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");
    buck.build(&[
        "--fail-fast",
        "--local-only",
        "--no-remote-cache",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
        "//fail_analysis:fail_analysis",
        "//slow:slow",
    ])
    .await
    .expect_failure();

    let text = std::fs::read_to_string(&streaming_report)?;
    let lines: Vec<&str> = text.lines().collect();
    let Some(last) = lines.last() else {
        panic!("the error that stopped the build must reach the streaming report");
    };
    let report: Value = serde_json::from_str(last)?;
    assert_eq!(
        results_of(&report).keys().collect::<Vec<_>>(),
        ["root//fail_analysis:fail_analysis"]
    );
    let configured =
        &report["results"]["root//fail_analysis:fail_analysis"]["configured"]["<unspecified>"];
    assert_eq!(configured["success"], "FAIL");
    let errors = configured["errors"].as_array().context("No `errors`")?;
    let [error] = errors.as_slice() else {
        panic!("Expected one error, got {errors:?}");
    };
    assert!(
        report["strings"][message_key(error)]
            .as_str()
            .is_some_and(|message| message.contains("Failure during analysis"))
    );
    Ok(())
}

// TODO fix on windows and mac
#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test(write_invocation_record = true)]
async fn test_exclude_action_error_diagnostics(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Test that --build-report-options=exclude-action-error-diagnostics removes
    // error_diagnostics from the build report.
    let report = tmp_path.join("build-report.json");
    let res = buck
        .build(&[
            "--build-report",
            report.to_str().unwrap(),
            "--build-report-options",
            "fill-out-failures,exclude-action-error-diagnostics",
            "//fail_action:fail_one_with_error_handler",
        ])
        .await
        .expect_failure();
    let mut report_data: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    let errors = report_data["results"]["root//fail_action:fail_one_with_error_handler"]["configured"]
        ["<unspecified>"]["errors"]
        .as_array()
        .context("No `errors`")?;
    assert_eq!(errors.len(), 1);
    // category_key must match what buck2 logs to its error telemetry.
    let record = res.invocation_record()?;
    let telemetry_error = record.single_error();
    assert_eq!(
        errors[0]["category_key"],
        telemetry_error["category_key"],
        "{:?}",
        (&errors[0], telemetry_error)
    );

    sanitize_build_report(&mut report_data);

    golden(
        &sanitize_hashes(&sanitize_python(
            &Json::from(report_data.clone()).dumps(Some(2), true),
            buck.cwd(),
        )),
        "fixtures/test_exclude_action_error_diagnostics.golden.json",
    );
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
#[buck_test]
async fn test_truncate_error_content(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Test that --build-report-options=truncate-error-content truncates
    // error message content in the build report when errors exceed 20KB.
    // Uses a target that produces a 25KB+ error message.
    let report = tmp_path.join("build-report.json");
    buck.build(&[
        "--build-report",
        report.to_str().unwrap(),
        "--build-report-options",
        "fill-out-failures,truncate-error-content",
        "//fail_action:fail_large_error",
    ])
    .await
    .expect_failure();
    let mut report_data: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    sanitize_build_report(&mut report_data);

    golden(
        &sanitize_hashes(&sanitize_python(
            &Json::from(report_data.clone()).dumps(Some(2), true),
            buck.cwd(),
        )),
        "fixtures/test_truncate_error_content.golden.json",
    );
    Ok(())
}
