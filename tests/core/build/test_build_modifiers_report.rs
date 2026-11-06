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
use buck2_e2e::golden::GOLDEN_DIRECTORY;
use buck2_e2e::golden::sanitize_build_report;
use buck2_e2e::golden::sanitize_hashes;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::replay_streaming_report;
use buck2_e2e::utils::streaming_status_history;
use serde_json::Value;
use serde_json::json;

async fn build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
    expect_error: bool,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let mut command = command.to_vec();
    command.extend(["--build-report", report.to_str().unwrap()]);

    if expect_error {
        command.extend(["--build-report-options", "fill-out-failures"]);
        buck.build(&command).await.expect_failure();
    } else {
        buck.build(&command).await?;
    }

    let mut report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    sanitize_build_report(&mut report);

    golden(
        &sanitize_hashes(&Json::from(report.clone()).dumps(Some(2), true)),
        &format!("{GOLDEN_DIRECTORY}{name}.golden.json"),
    );
    Ok(())
}

async fn streamed_status_history(
    buck: &Buck,
    tmp_path: &Path,
    targets: &[&str],
) -> anyhow::Result<BTreeMap<String, Vec<Value>>> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");
    let mut args = targets.to_vec();
    args.extend([
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ]);
    buck.build(&args).await?;
    Ok(streaming_status_history(&read_streaming_report(
        &streaming_report,
    )?))
}

fn configured_entries<'a>(results: Option<&'a Value>) -> Vec<(&'a String, &'a Value)> {
    results
        .and_then(|entry| entry.get("configured"))
        .and_then(Value::as_object)
        .map(|configured| configured.iter().collect())
        .unwrap_or_default()
}

/// The `success` of the entry's only configured target, if it has one yet.
fn configured_success(results: &BTreeMap<String, Value>, key: &str) -> Option<Value> {
    let configured = configured_entries(results.get(key));
    assert!(
        configured.len() <= 1,
        "{key} has several configurations: {:?}",
        configured.iter().map(|(cfg, _)| cfg).collect::<Vec<_>>()
    );
    configured
        .first()
        .map(|(_, entry)| entry["success"].clone())
}

fn has_output(results: &Value, key: &str, name: &str) -> bool {
    configured_entries(results.get(key))
        .into_iter()
        .flat_map(|(_, configured)| {
            configured["outputs"]
                .as_object()
                .unwrap_or_else(|| panic!("No `outputs` in {configured}"))
                .values()
        })
        .flat_map(|paths| paths.as_array().expect("a list of outputs"))
        .any(|path| path.as_str().expect("an output path").ends_with(name))
}

fn first_line_with_output(reports: &[Value], key: &str, name: &str) -> usize {
    let lines: Vec<usize> = reports
        .iter()
        .enumerate()
        .filter(|(_, r)| has_output(&r["results"], key, name))
        .map(|(i, _)| i)
        .collect();
    let Some(first) = lines.first() else {
        panic!("`{name}` for {key} is on no streaming line");
    };
    *first
}

#[buck_test]
async fn test_build_modifiers_report(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_modifiers_report",
        &[
            "root//:target?root//:macos+root//:arm",
            "root//:target?root//:linux",
        ],
        false,
    )
    .await
}

#[buck_test]
async fn test_build_modifiers_report_error_failures_includes_modifiers(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_modifiers_report_error_failures_includes_modifiers",
        &["root//fail:fail?root//:macos+root//:arm"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_modifiers_report_package_pattern(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_modifiers_report_package_pattern",
        &["root//:?root//:macos"],
        false,
    )
    .await
}

#[buck_test]
async fn test_build_modifiers_report_recursive_pattern(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_modifiers_report_recursive_pattern",
        &["root//...?root//:macos"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_modifiers_report_ambiguous_pattern(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_modifiers_report_ambiguous_pattern",
        &[
            "root//subdir?root//:macos",
            "root//subdir:subdir?root//:linux",
            "root//subdir:subdir?root//:macos",
        ],
        false,
    )
    .await
}

#[buck_test]
async fn test_build_modifiers_report_deduplication(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_modifiers_report_deduplication",
        &[
            "root//:?root//:macos",
            "root//:target2?root//:linux",
            "root//:target?root//:macos",
        ],
        false,
    )
    .await
}

#[buck_test]
async fn test_build_modifiers_that_lead_to_same_configured(buck: &Buck) -> anyhow::Result<()> {
    let mac_first = "root//:target?root//:macos+root//:arm";
    let arm_first = "root//:target?root//:arm+root//:macos";

    let result = buck
        .build(&[
            mac_first,
            arm_first,
            "-c",
            "buck2.detailed_aggregated_metrics=true",
            "-c",
            "buck2.log_configured_graph_sketch=true",
            "-c",
            "buck2.log_configured_graph_unconfigured_sketch=true",
        ])
        .await?;

    let report: Value = serde_json::from_str(&result.stdout)?;

    let configured = |target: &str| -> anyhow::Result<&Value> {
        report
            .get("results")
            .and_then(|results| results.get(target))
            .and_then(|result| result.get("configured"))
            .with_context(|| format!("No `configured` for `{target}` in the build report"))
    };

    assert_eq!(configured(mac_first)?, configured(arm_first)?);
    Ok(())
}

/// Each `?modifier` entry finishes when its own request has been built.
#[buck_test]
async fn test_streaming_build_report_target_completion_with_modifiers(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mac_arm = "root//:target?root//:macos+root//:arm";
    let linux = "root//:target?root//:linux";

    assert_eq!(
        streamed_status_history(buck, tmp_path, &[mac_arm, linux]).await?,
        BTreeMap::from([
            (
                mac_arm.to_owned(),
                vec![json!("IN_PROGRESS"), json!("SUCCESS")]
            ),
            (
                linux.to_owned(),
                vec![json!("IN_PROGRESS"), json!("SUCCESS")]
            ),
        ])
    );
    Ok(())
}

/// Two entries for one configured target each finish when their own request has.
#[buck_test]
async fn test_streaming_build_report_target_completion_with_modifiers_that_lead_to_same_configured(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mac_first = "root//:target?root//:macos+root//:arm";
    let arm_first = "root//:target?root//:arm+root//:macos";

    assert_eq!(
        streamed_status_history(buck, tmp_path, &[mac_first, arm_first]).await?,
        BTreeMap::from([
            (
                mac_first.to_owned(),
                vec![json!("IN_PROGRESS"), json!("SUCCESS")]
            ),
            (
                arm_first.to_owned(),
                vec![json!("IN_PROGRESS"), json!("SUCCESS")]
            ),
        ])
    );
    Ok(())
}

/// Requests that configure one target each finish their own configured entry.
#[buck_test]
async fn test_streaming_build_report_configured_target_finishes_per_request(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let fast = "root//slow:target?root//:macos+root//:arm";
    let slow = "root//slow:target?root//:arm+root//:macos";
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.build(&[
        fast,
        "root//slow:target[slow]?root//:arm+root//:macos",
        // A cached `slow.txt` could be built before `fast` finishes.
        "--no-remote-cache",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    let final_results = replay_streaming_report(&reports);
    let configurations = |key: &str| -> BTreeSet<String> {
        final_results[key]["configured"]
            .as_object()
            .unwrap_or_else(|| panic!("No `configured` for {key}"))
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(configurations(fast), configurations(slow));

    // `[slow]` also lists `fast.txt`, so a line is written while `slow.txt` builds.
    let slow_built = first_line_with_output(&reports, slow, "slow.txt");
    let statuses: Vec<(Option<Value>, Option<Value>)> = (0..slow_built)
        .map(|i| replay_streaming_report(&reports[..=i]))
        .map(|results| {
            (
                configured_success(&results, fast),
                configured_success(&results, slow),
            )
        })
        .collect();
    assert!(
        statuses
            .iter()
            .all(|(_, slow_status)| *slow_status != Some(json!("SUCCESS"))),
        "{statuses:?}"
    );
    assert!(
        statuses.contains(&(Some(json!("SUCCESS")), Some(json!("IN_PROGRESS")))),
        "{statuses:?}"
    );

    assert_eq!(
        (
            configured_success(&final_results, fast),
            configured_success(&final_results, slow)
        ),
        (Some(json!("SUCCESS")), Some(json!("SUCCESS")))
    );
    Ok(())
}

/// A request's labels for one target share a configured entry, which finishes
/// only once the last of them has built.
#[buck_test]
async fn test_streaming_build_report_configured_target_waits_for_every_label(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let key = "root//slow:target?root//:macos";
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.build(&[
        key,
        "root//slow:target[slow]?root//:macos",
        // A cached `slow.txt` could be built before the default label finishes.
        "--no-remote-cache",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    let slow_built = first_line_with_output(&reports, key, "slow.txt");
    let statuses: Vec<Option<Value>> = (0..slow_built)
        .map(|i| configured_success(&replay_streaming_report(&reports[..=i]), key))
        .collect();
    assert!(
        !statuses.is_empty(),
        "the default label's output is streamed before `slow.txt`"
    );
    assert!(
        statuses
            .iter()
            .all(|status| *status != Some(json!("SUCCESS"))),
        "{statuses:?}"
    );
    assert_eq!(
        configured_success(&replay_streaming_report(&reports), key),
        Some(json!("SUCCESS"))
    );
    Ok(())
}
