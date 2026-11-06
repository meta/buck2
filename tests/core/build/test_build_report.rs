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
use std::fmt::Debug;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::replace_digest;
use buck2_e2e::utils::replace_hash;
use buck2_e2e::utils::replay_streaming_report;
use serde_json::Value;
use serde_json::json;

/// Replace timing-dependent fields with a placeholder to avoid flaky tests.
fn sanitize_timing_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                match key.as_str() {
                    "compute_time_ms" => *value = json!("<COMPUTE_TIME_MS>"),
                    "wall_clock_completion_ms" => *value = json!("<WALL_CLOCK_COMPLETION_MS>"),
                    _ => sanitize_timing_fields(value),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(sanitize_timing_fields),
        _ => {}
    }
}

/// Whether Python would take `value` as true.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Reading `{}`", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn object<'a>(value: &'a Value, key: &str) -> &'a serde_json::Map<String, Value> {
    value[key]
        .as_object()
        .unwrap_or_else(|| panic!("No `{key}` object in {value}"))
}

fn collect_output_paths(results: &BTreeMap<String, Value>) -> BTreeSet<String> {
    let mut output_paths = BTreeSet::new();
    for target_result in results.values() {
        for configured_result in object(target_result, "configured").values() {
            for outputs in ["outputs", "other_outputs"] {
                if configured_result.get(outputs).is_none() {
                    continue;
                }
                for provider_outputs in object(configured_result, outputs).values() {
                    output_paths.extend(
                        provider_outputs
                            .as_array()
                            .expect("a list of outputs")
                            .iter()
                            .map(|path| path.as_str().expect("an output path").to_owned()),
                    );
                }
            }
        }
    }
    output_paths
}

fn sorted_outputs(entry: &Value) -> BTreeMap<String, Vec<String>> {
    object(entry, "outputs")
        .iter()
        .map(|(provider, paths)| {
            let mut paths: Vec<String> = paths
                .as_array()
                .expect("a list of outputs")
                .iter()
                .map(|path| path.as_str().expect("an output path").to_owned())
                .collect();
            paths.sort();
            (provider.clone(), paths)
        })
        .collect()
}

/// Line at which each entry first reads `SUCCESS`, checking it never goes back.
fn first_success<K: Ord + Clone + Debug>(statuses: &[(usize, K, Value)]) -> BTreeMap<K, usize> {
    let mut finished_at = BTreeMap::new();
    for (i, key, status) in statuses {
        assert!(
            *status == "IN_PROGRESS" || *status == "SUCCESS",
            "{:?}",
            (key, status)
        );
        if *status == "SUCCESS" {
            finished_at.entry(key.clone()).or_insert(*i);
        } else {
            assert!(
                !finished_at.contains_key(key),
                "{key:?} went back to IN_PROGRESS at line {i}"
            );
        }
    }
    finished_at
}

async fn build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let mut args = vec!["--build-report", report.to_str().unwrap()];
    args.extend(command);
    buck.build(&args).await?;

    let mut report = read_json(&report)?;
    let fields = report.as_object_mut().context("A build report object")?;
    fields.remove("trace_id").context("No `trace_id`")?;
    fields.remove("project_root").context("No `project_root`")?;
    sanitize_timing_fields(&mut report);

    // Build report errors can change based on minor test changes such as
    // 1. Adding a target in TARGETS.fixture
    // 2. Line number changing due to code moving around
    // Sanitize so that we only check the important bits of the error message
    golden(
        &replace_digest(&replace_hash(
            &Json::from(report.clone()).dumps(Some(2), true),
        )),
        &format!("fixtures/{name}.golden.json"),
    );
    Ok(())
}

async fn streaming_build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    golden_name: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let base_report = tmp_path.join("build-report.json");
    let report = tmp_path.join("streaming-build-report.json");

    let mut args = vec![
        "--build-report",
        base_report.to_str().unwrap(),
        "--streaming-build-report",
        report.to_str().unwrap(),
    ];
    args.extend(command);
    buck.build(&args).await?;

    let reports = read_streaming_report(&report)?;
    // Each line carries one target, so the whole stream is what adds up to the report.
    let mut report = reports.last().context("An empty streaming report")?.clone();
    report["results"] = Value::Object(replay_streaming_report(&reports).into_iter().collect());
    report["strings"] = Value::Object(
        reports
            .iter()
            .flat_map(|line| object(line, "strings"))
            .map(|(key, string)| (key.clone(), string.clone()))
            .collect(),
    );

    let fields = report.as_object_mut().context("A build report object")?;
    fields.remove("trace_id").context("No `trace_id`")?;
    fields.remove("project_root").context("No `project_root`")?;
    sanitize_timing_fields(&mut report);

    // Build report errors can change based on minor test changes such as
    // 1. Adding a target in TARGETS.fixture
    // 2. Line number changing due to code moving around
    // Sanitize so that we only check the important bits of the error message
    golden(
        &replace_digest(&replace_hash(
            &Json::from(report.clone()).dumps(Some(2), true),
        )),
        &format!("fixtures/{golden_name}.golden.json"),
    );
    Ok(())
}

#[buck_test]
async fn test_build_report_format(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format",
        &[
            "//:rule1",
            "//:rule2",
            "//:rule2[out1]",
            "-c",
            "buck2.log_configured_graph_size=true",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_skip_unconfigured(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_skip_unconfigured",
        &[
            "//:rule1",
            "-c",
            "build_report.print_unconfigured_section=false",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_package_relative_paths(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_package_relative_paths",
        &[
            "//:rule1",
            "//subdir:rule",
            "--build-report-options",
            "package-project-relative-paths",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_artifact_hash_information(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_artifact_hash_information",
        &[
            "//:rule1",
            "//:dir1",
            "//subdir:rule",
            "--build-report-options",
            "include-artifact-hash-information",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_configured_graph_sketch(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_configured_graph_sketch",
        &[
            "//:rule1",
            "//:dir1",
            "//subdir:rule",
            "-c",
            "buck2.log_configured_graph_sketch=true",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_total_configured_graph_sketch(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_total_configured_graph_sketch",
        &[
            "//:rule1",
            "//:dir1",
            "//subdir:rule",
            "-c",
            "buck2.log_total_configured_graph_sketch=true",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_action_graph_sketch(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_action_graph_sketch",
        &[
            "//:rule1",
            "//:dir1",
            "//subdir:rule",
            "-c",
            "buck2.log_action_graph_sketch=true",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_format_all_sketches(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_all_sketches",
        &[
            "//:rule1",
            "//:dir1",
            "//subdir:rule",
            // Let's look at something more interesting than unspecified platform
            "--target-platforms=root//:platform",
            "-c",
            "buck2.log_configured_graph_sketch=true",
            "-c",
            "buck2.log_total_configured_graph_sketch=true",
            "-c",
            "buck2.log_action_graph_sketch=true",
        ],
    )
    .await
}

// The golden belongs to the `build_report_test` of the same name.
#[buck_test]
async fn test_streaming_build_report_format_skip_unconfigured(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    streaming_build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_skip_unconfigured",
        &[
            "//:rule1",
            "-c",
            "build_report.print_unconfigured_section=false",
        ],
    )
    .await
}

#[buck_test]
async fn test_streaming_build_report_format_package_relative_paths(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    streaming_build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_package_relative_paths",
        &[
            "//:rule1",
            "//subdir:rule",
            "--build-report-options",
            "package-project-relative-paths",
        ],
    )
    .await
}

#[buck_test]
async fn test_streaming_build_report_format_artifact_hash_information(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    streaming_build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_artifact_hash_information",
        &[
            "//:rule1",
            "//:dir1",
            "//subdir:rule",
            "--build-report-options",
            "include-artifact-hash-information",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_report_non_existent_directory(buck: &Buck) -> anyhow::Result<()> {
    let build_report = "non_existent_dir/report";

    buck.build(&["//:rule1", "--build-report", build_report])
        .await?;

    let report = read_json(&buck.cwd().join(build_report))?;
    assert!(is_truthy(&report["success"]));
    Ok(())
}

#[buck_test]
async fn test_build_report_contains_metrics(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:rule1",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "--build-report",
        report.to_str().unwrap(),
    ])
    .await?;

    let report = read_json(&report)?;
    // Cannot create a golden test since the values may change across runs.
    // Assert that some of the fields are present to ensure that the values are being populated.
    // Counts compare as numbers: the metrics are floats.
    assert!(is_truthy(&report["build_metrics"]));
    assert_eq!(
        report["build_metrics"]["action_graph_size"].as_f64(),
        Some(1.0)
    );
    assert_eq!(
        report["build_metrics"]["metrics"]["declared_actions"].as_f64(),
        Some(2.0)
    );
    Ok(())
}

#[buck_test]
async fn test_build_report_contains_per_target_build_metrics(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:rule1",
        "//:rule2",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "--build-report",
        report.to_str().unwrap(),
    ])
    .await?;

    let report = read_json(&report)?;
    // Cannot create a golden test since the values may change across runs.
    // Assert that some of the fields are present to ensure that the values are being populated.
    // Counts compare as numbers: the metrics are floats.
    let rule1_metrics =
        &report["results"]["root//:rule1"]["configured"]["<unspecified>"]["build_metrics"];
    assert_eq!(rule1_metrics["action_graph_size"].as_f64(), Some(1.0));
    assert_eq!(
        rule1_metrics["metrics"]["declared_actions"].as_f64(),
        Some(2.0)
    );
    assert_eq!(
        rule1_metrics["amortized_metrics"]["declared_actions"].as_f64(),
        Some(1.0)
    );

    // re_platform_names should be absent for local-only actions (skip_serializing_if empty)
    assert!(rule1_metrics.get("re_platform_names").is_none());

    // wall_clock_completion_ms should be present for completed targets
    assert!(rule1_metrics.get("wall_clock_completion_ms").is_some());
    assert!(
        rule1_metrics["wall_clock_completion_ms"]
            .as_f64()
            .is_some_and(|ms| ms > 0.0)
    );

    let rule2_metrics =
        &report["results"]["root//:rule2"]["configured"]["<unspecified>"]["build_metrics"];
    assert_eq!(rule2_metrics["action_graph_size"].as_f64(), Some(1.0));
    assert_eq!(
        rule2_metrics["metrics"]["declared_actions"].as_f64(),
        Some(4.0)
    );
    assert_eq!(
        rule2_metrics["amortized_metrics"]["declared_actions"].as_f64(),
        Some(3.0)
    );
    assert!(rule2_metrics.get("re_platform_names").is_none());
    assert!(is_truthy(&report["build_metrics"]));
    Ok(())
}

/// Test that --streaming-build-report creates a streaming report file with JSON lines.
#[buck_test]
async fn test_streaming_build_report(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-report.jsonl");

    buck.build(&[
        "//:rule1",
        "//:rule2",
        "//:rule2[out2]",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    for report in &reports {
        assert!(
            ["trace_id", "success", "results", "project_root"]
                .iter()
                .all(|key| report.get(key).is_some()),
            "{report}"
        );
        let results = object(report, "results");
        assert_eq!(
            results.len(),
            1,
            "Expected one target per line, got {:?}",
            results.keys().collect::<Vec<_>>()
        );
    }
    // One line per output, plus one each when a configured target and a target finish.
    // `//:rule2` and `//:rule2[out2]` are a single target.
    assert_eq!(reports.len(), 7, "Expected 7 lines, got {}", reports.len());
    let results = replay_streaming_report(&reports);
    let names: BTreeSet<String> = collect_output_paths(&results)
        .iter()
        .map(|path| {
            Path::new(path)
                .file_name()
                .expect("an output file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["out.txt", "out1.txt", "out2.txt"].map(str::to_owned))
    );
    Ok(())
}

/// A missing target has no outputs, so only its build-finished line reports it.
#[buck_test]
async fn test_streaming_build_report_finishes_missing_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-report.jsonl");

    buck.build(&[
        "//:does_not_exist",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await
    .expect_failure();

    let reports = read_streaming_report(&streaming_report)?;
    assert_eq!(reports.len(), 1, "Expected 1 line, got {}", reports.len());
    assert_eq!(
        reports[0]["results"]["root//:does_not_exist"]["success"],
        "FAIL"
    );
    Ok(())
}

/// An entry reads `IN_PROGRESS` until its build has finished, then `SUCCESS` for good.
#[buck_test]
async fn test_streaming_build_report_in_progress(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-report.jsonl");
    let final_report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:rule1",
        "//:rule2",
        "//:rule2[out2]",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
        "--build-report",
        final_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    let final_text = std::fs::read_to_string(&final_report)?;
    let final_json: Value = serde_json::from_str(&final_text)?;
    let final_results = object(&final_json, "results");

    let mut target_statuses = Vec::new();
    let mut configured_statuses = Vec::new();
    for (i, report) in reports.iter().enumerate() {
        for (target, entry) in object(report, "results") {
            target_statuses.push((i, target.clone(), entry["success"].clone()));
            for (cfg, configured) in object(entry, "configured") {
                configured_statuses.push((
                    i,
                    (target.clone(), cfg.clone()),
                    configured["success"].clone(),
                ));
            }
        }
    }
    let target_finished_at = first_success(&target_statuses);
    let configured_finished_at = first_success(&configured_statuses);

    assert!(
        target_statuses
            .iter()
            .any(|(_, _, status)| *status == "IN_PROGRESS"),
        "No target was ever reported IN_PROGRESS"
    );
    assert_eq!(
        target_finished_at.keys().collect::<BTreeSet<_>>(),
        final_results.keys().collect::<BTreeSet<_>>()
    );
    let final_configured: BTreeSet<(String, String)> = final_results
        .iter()
        .flat_map(|(target, entry)| {
            object(entry, "configured")
                .keys()
                .map(move |cfg| (target.clone(), cfg.clone()))
        })
        .collect();
    assert_eq!(
        configured_finished_at
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        final_configured
    );

    // The first line to report an entry finished already carries everything it built. For
    // `//:rule2` that includes the output of `//:rule2[out2]`, a separate provider label whose
    // build can end before or after the bare label's.
    for (target, &i) in &target_finished_at {
        assert_eq!(
            sorted_outputs(&reports[i]["results"][target]),
            sorted_outputs(&final_results[target])
        );
    }
    for ((target, cfg), &i) in &configured_finished_at {
        assert_eq!(
            sorted_outputs(&reports[i]["results"][target]["configured"][cfg]),
            sorted_outputs(&final_results[target]["configured"][cfg])
        );
    }

    assert!(!final_text.contains("IN_PROGRESS"));
    Ok(())
}

/// Without the unconfigured section, only configured entries have a `success`.
#[buck_test]
async fn test_streaming_build_report_in_progress_without_unconfigured_section(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-report.jsonl");

    buck.build(&[
        "//:rule1",
        "//:does_not_exist",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
        "-c",
        "build_report.print_unconfigured_section=false",
    ])
    .await
    .expect_failure();

    let reports = read_streaming_report(&streaming_report)?;
    let entries: Vec<(&String, &Value)> = reports
        .iter()
        .flat_map(|report| object(report, "results"))
        .collect();
    assert!(
        entries
            .iter()
            .all(|(_, entry)| entry.get("success").is_none()),
        "{entries:?}"
    );

    let rule1_history: Vec<Vec<Value>> = entries
        .iter()
        .filter(|(key, _)| *key == "root//:rule1")
        .map(|(_, entry)| {
            object(entry, "configured")
                .values()
                .map(|configured| configured["success"].clone())
                .collect()
        })
        .collect();
    // The output's line, then the configured target finishing, then the target.
    assert_eq!(
        rule1_history,
        [
            vec![json!("IN_PROGRESS")],
            vec![json!("SUCCESS")],
            vec![json!("SUCCESS")]
        ],
        "{rule1_history:?}"
    );

    let missing: Vec<&Value> = entries
        .iter()
        .filter(|(key, _)| *key == "root//:does_not_exist")
        .map(|(_, entry)| *entry)
        .collect();
    assert!(
        !missing.is_empty(),
        "Expected a line for the missing target"
    );
    assert!(
        missing
            .iter()
            .all(|entry| entry["configured"] == json!({}) && is_truthy(&entry["errors"]))
    );
    Ok(())
}

#[buck_test]
async fn test_streaming_build_report_non_existent_directory(buck: &Buck) -> anyhow::Result<()> {
    let streaming_report = "non_existent_dir/report";

    buck.build(&["//:rule1", "--streaming-build-report", streaming_report])
        .await?;

    let reports = read_streaming_report(&buck.cwd().join(streaming_report))?;
    assert!(
        !reports.is_empty(),
        "Expected the streaming report to be written"
    );
    assert!(reports.iter().all(|report| is_truthy(&report["success"])));
    Ok(())
}

#[buck_test]
async fn test_streaming_build_report_overwrites_existing_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-report.jsonl");

    std::fs::write(&streaming_report, "Not valid JSON!")?;

    buck.build(&[
        "//:rule1",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    // Read and validate the streaming report
    let text = std::fs::read_to_string(&streaming_report)?;

    // Each line should be valid JSON
    for line in text.trim().split('\n') {
        let report_data: Value = serde_json::from_str(line)?;
        // Verify basic structure
        assert!(report_data.get("trace_id").is_some());
        assert!(report_data.get("success").is_some());
        assert!(report_data.get("results").is_some());
        assert!(report_data.get("project_root").is_some());
    }
    Ok(())
}

#[buck_test(data_dir = "re_platform_names")]
async fn test_build_report_re_platform_names(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:run_action",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "--build-report",
        report.to_str().unwrap(),
    ])
    .await?;

    let report_data = read_json(&report)?;
    let metrics = &report_data["results"]["root//:run_action"]["configured"]["<unspecified>"]["build_metrics"];
    assert!(metrics.get("re_platform_names").is_some());
    assert!(
        metrics["re_platform_names"]
            .as_array()
            .is_some_and(|names| names.contains(&json!("linux-remote-execution")))
    );
    Ok(())
}
