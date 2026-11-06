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

use buck2_e2e::golden::sanitize_build_report;
use buck2_e2e::golden::sanitize_build_report_error;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use serde_json::Value;

async fn build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
    should_fail: bool,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let report_arg = report.to_string_lossy();
    if should_fail {
        let mut args = vec![
            "--build-report",
            &report_arg,
            "--build-report-options",
            "fill-out-failures",
        ];
        args.extend(command);
        buck.test(&args).await.expect_failure();
    } else {
        let mut args = vec!["--build-report", &report_arg];
        args.extend(command);
        buck.test(&args).await?;
    }

    let mut report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    sanitize_build_report(&mut report);

    golden(
        &sanitize_build_report_error(&Json::from(report.clone()).dumps(Some(2), true)),
        &format!("fixtures/{name}.golden.json"),
    );
    Ok(())
}

/// Each configured entry's status across report lines; FAIL wins.
///
/// A `buck test` streaming line carries one provider label's build, so a
/// later line for the same entry can omit an earlier failure.
fn configured_statuses(reports: &[Value]) -> BTreeMap<(String, String), Value> {
    let mut statuses: BTreeMap<(String, String), Value> = BTreeMap::new();
    for report in reports {
        let results = report["results"]
            .as_object()
            .unwrap_or_else(|| panic!("No `results` in {report}"));
        for (key, entry) in results {
            let configured_entries = entry["configured"]
                .as_object()
                .unwrap_or_else(|| panic!("No `configured` in {entry}"));
            for (cfg, configured) in configured_entries {
                let status = statuses.entry((key.clone(), cfg.clone())).or_default();
                if *status != "FAIL" {
                    *status = configured["success"].clone();
                }
            }
        }
    }
    statuses
}

async fn assert_error_before_build_streamed(
    buck: &Buck,
    tmp_path: &Path,
    target: &str,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.test(&[
        target,
        "--streaming-build-report",
        &streaming_report.to_string_lossy(),
    ])
    .await
    .expect_failure();

    let reports = read_streaming_report(&streaming_report)?;
    assert!(!reports.is_empty(), "Expected at least 1 line, got 0");
    let key = format!("root{target}");
    assert!(
        reports
            .iter()
            .all(|report| report["results"][&key]["success"] == "FAIL"),
        "Expected every line to report `{key}` as FAIL: {reports:#?}"
    );
    Ok(())
}

#[buck_test]
async fn test_build_report_format(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format",
        &["//:ok", "//:fail_test"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_skip_unconfigured(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_skip_unconfigured",
        &[
            "//:ok",
            "-c",
            "build_report.print_unconfigured_section=false",
        ],
        false,
    )
    .await
}

#[buck_test]
async fn test_failed_build_has_build_report(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_failed_build_has_build_report",
        &["//:fail_build1"],
        true,
    )
    .await
}

#[buck_test]
async fn test_target_doesnt_exist(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_target_doesnt_exist",
        &["//:doesnt_exist"],
        true,
    )
    .await
}

#[buck_test]
async fn test_multiple_failures_included(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_multiple_failures_included",
        &["//:fail_build1", "//:fail_build2"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_with_build_default_info(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_with_build_default_info",
        &["//...", "--build-default-info"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_with_build_run_info(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_with_build_run_info",
        &["//...", "--build-run-info"],
        true,
    )
    .await
}

/// `//:ok` has no outputs, so only its build-finished lines report it.
#[buck_test]
async fn test_streaming_build_report_target_completion(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.test(&[
        "//:ok",
        "--streaming-build-report",
        &streaming_report.to_string_lossy(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    // The configured target finishing, then the target.
    assert_eq!(reports.len(), 2, "Expected 2 lines, got {}", reports.len());
    assert!(reports.iter().all(|report| {
        report["results"]
            .as_object()
            .is_some_and(|results| results.keys().eq(["root//:ok"]))
    }));
    assert_eq!(
        reports
            .iter()
            .map(|report| {
                let entry = &report["results"]["root//:ok"];
                (
                    entry["success"].clone(),
                    entry["configured"]
                        .as_object()
                        .into_iter()
                        .flat_map(|configured| configured.values())
                        .map(|configured| configured["success"].clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
        [
            (Value::from("IN_PROGRESS"), vec![Value::from("SUCCESS")]),
            (Value::from("SUCCESS"), vec![Value::from("SUCCESS")]),
        ]
    );
    Ok(())
}

#[buck_test]
async fn test_streaming_build_report_target_completion_with_failures(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.test(&[
        "--build-report",
        &report.to_string_lossy(),
        "--streaming-build-report",
        &streaming_report.to_string_lossy(),
        "//:ok",
        "//:fail_test",
        "//:fail_build1",
    ])
    .await
    .expect_failure();

    let streamed = configured_statuses(&read_streaming_report(&streaming_report)?);
    assert_eq!(
        streamed
            .keys()
            .map(|(key, _cfg)| key.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["root//:ok", "root//:fail_test", "root//:fail_build1"])
    );
    assert_eq!(
        streamed,
        configured_statuses(&[serde_json::from_str(&std::fs::read_to_string(&report)?)?])
    );
    Ok(())
}

/// `//:fail_build1` fails in analysis, before `buck test` builds it.
#[buck_test]
async fn test_streaming_build_report_target_completion_analysis_error(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_error_before_build_streamed(buck, tmp_path, "//:fail_build1").await
}

#[buck_test]
async fn test_streaming_build_report_target_completion_missing_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_error_before_build_streamed(buck, tmp_path, "//:doesnt_exist").await
}

/// Both targets fail in analysis. Each line carries one target's errors.
#[buck_test]
async fn test_streaming_build_report_target_completion_errors_per_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.test(&[
        "//:fail_build1",
        "//:fail_build2",
        "--streaming-build-report",
        &streaming_report.to_string_lossy(),
    ])
    .await
    .expect_failure();

    let reports = read_streaming_report(&streaming_report)?;
    let mut keys: Vec<Vec<&str>> = reports
        .iter()
        .map(|report| {
            report["results"]
                .as_object()
                .into_iter()
                .flat_map(|results| results.keys().map(String::as_str))
                .collect()
        })
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [vec!["root//:fail_build1"], vec!["root//:fail_build2"]]
    );
    assert!(
        reports
            .iter()
            .flat_map(|report| {
                report["results"]
                    .as_object()
                    .unwrap_or_else(|| panic!("No `results` in {report}"))
            })
            .all(|(_, entry)| entry["success"] == "FAIL")
    );
    Ok(())
}
