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
use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use serde_json::Value;

fn read_json(path: &Path) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// The report's entry for `root//:ok`.
fn ok_entry(report: &Value) -> anyhow::Result<&serde_json::Map<String, Value>> {
    json_get(
        report,
        &["results", "root//:ok", "configured", "<unspecified>"],
    )
    .and_then(Value::as_object)
    .context("No `results.root//:ok.configured.<unspecified>` in the build report")
}

fn field<'a>(entry: &'a serde_json::Map<String, Value>, name: &str) -> anyhow::Result<&'a Value> {
    entry
        .get(name)
        .with_context(|| format!("No `{name}` in the build report entry"))
}

fn number(value: &Value) -> anyhow::Result<f64> {
    value
        .as_f64()
        .with_context(|| format!("Not a number: {value}"))
}

/// The number at `path` in `value`.
fn number_at(value: &Value, path: &[&str]) -> anyhow::Result<f64> {
    number(json_get(value, path).with_context(|| format!("No `{}` in {value}", path.join(".")))?)
}

#[buck_test]
async fn test_build_report_contains_per_target_build_metrics(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    // `--build-default-info` is what a combined build-and-test invocation passes,
    // and it is what gives the target a build output to time.
    buck.test(&[
        "//:ok",
        "--build-default-info",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "--build-report",
        &report.to_string_lossy(),
    ])
    .await?;

    let report = read_json(&report)?;

    // Values change across runs, so assert the fields are populated rather than
    // goldening them.
    assert!(
        report
            .get("build_metrics")
            .and_then(Value::as_object)
            .is_some_and(|metrics| !metrics.is_empty()),
        "{report}"
    );

    let entry = ok_entry(&report)?;
    let metrics = field(entry, "build_metrics")?;
    assert!(number_at(metrics, &["metrics", "declared_actions"])? > 0.0);
    // Measured from the start of the command, as `buck build` reports it, rather
    // than from the start of this target's own build.
    assert!(number_at(metrics, &["wall_clock_completion_ms"])? > 0.0);

    for name in [
        "configured_graph_sketch",
        "retained_analysis_memory_sketch",
        "action_graph_sketch",
        "artifact_count_sketch",
        "artifact_size_sketch",
        "artifact_count_sketch_cardinality",
        "artifact_size_sketch_cardinality",
    ] {
        assert!(!entry.contains_key(name), "{name}");
    }
    Ok(())
}

#[buck_test]
async fn test_build_and_test_reports_have_matching_sketches(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let build_report = tmp_path.join("build-report.json");
    let test_report = tmp_path.join("test-report.json");
    let configs = [
        "-c",
        "buck2.log_configured_graph_size=true",
        "-c",
        "buck2.log_configured_graph_sketch=true",
        "-c",
        "buck2.log_retained_analysis_memory_sketch=true",
        "-c",
        "buck2.log_action_graph_sketch=true",
        "-c",
        "buck2.log_artifact_count_sketch=true",
        "-c",
        "buck2.log_artifact_size_sketch=true",
        "-c",
        "buck2.log_sketch_cardinalities=true",
        "-c",
        "buck2.providers_to_skip_in_artifact_path_sketch=build,run",
    ];

    let build_report_arg = build_report.to_string_lossy();
    let mut args = vec![
        "//:ok",
        "--build-default-info",
        "--skip-run-info",
        "--build-test-info",
    ];
    args.extend(configs);
    args.extend(["--build-report", &build_report_arg]);
    buck.build(&args).await?;

    let test_report_arg = test_report.to_string_lossy();
    let mut args = vec!["//:ok", "--build-default-info"];
    args.extend(configs);
    args.extend(["--build-report", &test_report_arg]);
    buck.test(&args).await?;

    let build_report = read_json(&build_report)?;
    let test_report = read_json(&test_report)?;
    let build_entry = ok_entry(&build_report)?;
    let test_entry = ok_entry(&test_report)?;

    let graph_size = field(build_entry, "configured_graph_size")?;
    assert!(number(graph_size)? > 0.0);
    assert_eq!(field(test_entry, "configured_graph_size")?, graph_size);

    for name in [
        "configured_graph_sketch",
        "retained_analysis_memory_sketch",
        "action_graph_sketch",
        "artifact_count_sketch",
        "artifact_size_sketch",
    ] {
        let sketch = field(build_entry, name)?;
        assert!(
            sketch
                .as_str()
                .with_context(|| format!("`{name}` is not a string: {sketch}"))?
                .starts_with("V1:"),
            "{name}: {sketch}"
        );
        assert_eq!(field(test_entry, name)?, sketch, "{name}");
    }

    for name in [
        "artifact_count_sketch_cardinality",
        "artifact_size_sketch_cardinality",
    ] {
        let cardinality = field(build_entry, name)?;
        assert!(number(cardinality)? > 0.0, "{name}");
        assert_eq!(field(test_entry, name)?, cardinality, "{name}");
    }
    Ok(())
}
