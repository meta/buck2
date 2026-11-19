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
use std::path::Path;
use std::process::Stdio;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

/// Runs `SKETCH_SIZE_BIN` on `input` and parses the cardinality it prints.
async fn sketch_size(input: String) -> anyhow::Result<f64> {
    let sketch_size_bin =
        std::env::var("SKETCH_SIZE_BIN").context("`SKETCH_SIZE_BIN` is not set")?;
    let mut child = tokio::process::Command::new(&sketch_size_bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Running `{sketch_size_bin}`"))?;
    let mut stdin = child.stdin.take().context("stdin is piped")?;
    stdin.write_all(input.as_bytes()).await?;
    drop(stdin);
    let output = child.wait_with_output().await?;
    anyhow::ensure!(
        output.status.success(),
        "`{sketch_size_bin}` failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

/// Get the cardinality of a single sketch string.
async fn get_sketch_cardinality(sketch_str: &str) -> anyhow::Result<f64> {
    let base64_str = sketch_str.strip_prefix("V1:").with_context(|| {
        format!(
            "Unexpected sketch version: {}",
            sketch_str.chars().take(10).collect::<String>()
        )
    })?;
    sketch_size(base64_str.to_owned()).await
}

/// Verify that merged sketch cardinality matches expected value within 10% tolerance.
///
/// Args:
///     sketch_strs: List of sketch strings to merge
///     expected: Expected cardinality value
async fn check_merged_cardinality(sketch_strs: &[&str], expected: f64) -> anyhow::Result<()> {
    // Remove version prefixes
    let base64_strs = sketch_strs
        .iter()
        .map(|sketch_str| {
            sketch_str.strip_prefix("V1:").with_context(|| {
                format!(
                    "Unexpected sketch version: {}",
                    sketch_str.chars().take(10).collect::<String>()
                )
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let cardinality = sketch_size(base64_strs.join("\n")).await?;

    let tolerance = 0.05;
    let lower_bound = expected * (1.0 - tolerance);
    let upper_bound = expected * (1.0 + tolerance);

    assert!(
        lower_bound <= cardinality && cardinality <= upper_bound,
        "Sketch cardinality {cardinality:.0} should be between {lower_bound:.0} and {upper_bound:.0} (expected ~{expected})"
    );
    Ok(())
}

/// Verify that actual value matches expected within a tolerance (default 10%).
fn check_approx(actual: f64, expected: f64) {
    let tolerance = 0.10;
    let lower_bound = expected * (1.0 - tolerance);
    let upper_bound = expected * (1.0 + tolerance);

    assert!(
        lower_bound <= actual && actual <= upper_bound,
        "Value {actual:.0} should be between {lower_bound:.0} and {upper_bound:.0} (expected ~{expected:.0})"
    );
}

fn read_report(report: &Path) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(report)?)?)
}

/// Build targets with a sketch config enabled and extract sketches from the build report.
///
/// Args:
///     sketch_field: The field name in the build report (e.g., "retained_analysis_memory_sketch")
///     config_key: The buckconfig key to enable (e.g., "buck2.log_retained_analysis_memory_sketch")
async fn build_and_extract_sketches(
    buck: &Buck,
    tmp_path: &Path,
    targets: &[&str],
    sketch_field: &str,
    config_key: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let report = tmp_path.join("build-report.json");

    // Build with sketch enabled
    let config = format!("{config_key}=true");
    let mut args = targets.to_vec();
    args.extend(["-c", &config, "--build-report", report.to_str().unwrap()]);
    buck.build(&args).await?;

    // Read and parse build report
    let report_data = read_report(&report)?;

    // Extract sketches for each target
    let mut sketches = BTreeMap::new();
    for target in targets {
        // Convert target pattern to full target name
        // "//:target1" -> "root//:target1"
        let full_target = format!("root{target}");
        let sketch = report_data["results"][&full_target]["configured"]["<unspecified>"]
            [sketch_field]
            .as_str()
            .unwrap_or_else(|| panic!("Sketch should be present for {full_target}"));
        assert!(
            sketch.starts_with("V1:"),
            "Sketch should have V1 version prefix for {full_target}"
        );
        sketches.insert((*target).to_owned(), sketch.to_owned());
    }

    Ok(sketches)
}

async fn get_retained_sketches(
    buck: &Buck,
    tmp_path: &Path,
    targets: &[&str],
) -> anyhow::Result<BTreeMap<String, String>> {
    build_and_extract_sketches(
        buck,
        tmp_path,
        targets,
        "retained_analysis_memory_sketch",
        "buck2.log_retained_analysis_memory_sketch",
    )
    .await
}

async fn get_analysis_peak_sketches(
    buck: &Buck,
    tmp_path: &Path,
    targets: &[&str],
) -> anyhow::Result<BTreeMap<String, String>> {
    build_and_extract_sketches(
        buck,
        tmp_path,
        targets,
        "peak_analysis_memory_sketch",
        "buck2.log_peak_analysis_memory_sketch",
    )
    .await
}

async fn get_load_peak_sketches(
    buck: &Buck,
    tmp_path: &Path,
    targets: &[&str],
) -> anyhow::Result<BTreeMap<String, String>> {
    build_and_extract_sketches(
        buck,
        tmp_path,
        targets,
        "peak_load_memory_sketch",
        "buck2.log_peak_load_memory_sketch",
    )
    .await
}

/// The `<unspecified>` configuration's entry for `target` in the build report.
fn configured_entry<'a>(
    report_data: &'a Value,
    target: &str,
) -> &'a serde_json::Map<String, Value> {
    report_data["results"][target]["configured"]["<unspecified>"]
        .as_object()
        .unwrap_or_else(|| {
            panic!("No `<unspecified>` configuration of {target} in the build report")
        })
}

/// The sketch in `field` of `entry`.
fn sketch<'a>(entry: &'a serde_json::Map<String, Value>, field: &str) -> &'a str {
    entry
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("No `{field}` in {entry:?}"))
}

// =============================================================================
// Retained analysis memory sketch tests
// =============================================================================

/// Test that retained_analysis_memory_sketch is computed correctly.
///
/// We use a custom rule that allocates memory in analysis,
/// then verify the sketch cardinality matches the expected amount.
#[buck_test]
async fn test_retained_analysis_memory_sketch(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // target1 allocates 1M + shared_dep allocates 5M = ~6M total
    let sketches = get_retained_sketches(buck, tmp_path, &["//:target1"]).await?;
    check_merged_cardinality(&[&sketches["//:target1"]], 6_000_000.0).await
}

/// Test that sketch merging works correctly.
///
/// Build two targets that share a dependency, then verify that merging
/// their sketches gives the correct combined cardinality.
#[buck_test]
async fn test_retained_analysis_memory_sketch_merging(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let sketches = get_retained_sketches(buck, tmp_path, &["//:target1", "//:target2"]).await?;

    let sketch1 = &sketches["//:target1"];
    let sketch2 = &sketches["//:target2"];

    // Expected:
    // target1: 1M + 5M (shared) = 6M
    // target2: 1.5M + 5M (shared) = 6.5M
    // merged: 1M + 1.5M + 5M (shared counted once) = 7.5M
    check_merged_cardinality(&[sketch1], 6_000_000.0).await?;
    check_merged_cardinality(&[sketch2], 6_500_000.0).await?;
    check_merged_cardinality(&[sketch1, sketch2], 7_500_000.0).await
}

/// Test that memory from bzl file globals is included in the sketch.
///
/// Rules defined in bzl files can have global data that gets retained.
/// This test verifies that such memory is reflected in the sketch.
#[buck_test]
async fn test_retained_analysis_memory_sketch_bzl_globals(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let sketches = get_retained_sketches(buck, tmp_path, &["//:target_with_globals"]).await?;

    // Verify global data memory is included (at least 1M from GLOBAL_DATA)
    check_merged_cardinality(&[&sketches["//:target_with_globals"]], 1_000_000.0).await
}

/// Test that sketches are bitwise identical across daemon restarts.
///
/// Build a target, kill the daemon, rebuild, and verify sketches are exactly equal.
#[buck_test]
async fn test_retained_analysis_memory_sketch_daemon_restart(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    // First build
    let sketches1 = get_retained_sketches(buck, tmp_path, &["//:target1"]).await?;

    // Kill daemon
    buck.kill().await?;

    // Second build after restart
    let sketches2 = get_retained_sketches(buck, tmp_path, &["//:target1"]).await?;

    // Sketches should be bitwise identical
    assert_eq!(
        sketches1["//:target1"], sketches2["//:target1"],
        "Sketches should be bitwise identical across daemon restarts"
    );
    Ok(())
}

/// Test that memory from anon targets is included in the sketch.
///
/// Anon targets allocate their own analysis memory which should be
/// reflected in the parent target's sketch.
#[buck_test]
async fn test_retained_analysis_memory_sketch_anon_targets(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let sketches = get_retained_sketches(buck, tmp_path, &["//:target_with_anon"]).await?;

    // The anon target allocates 500K + parent allocates 100K = ~600K
    check_merged_cardinality(&[&sketches["//:target_with_anon"]], 600_000.0).await
}

/// Test that sketch is not present when config is disabled.
#[buck_test]
async fn test_retained_analysis_memory_sketch_disabled(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    // Build without sketch config
    buck.build(&["//:target1", "--build-report", report.to_str().unwrap()])
        .await?;

    // Read build report
    let report_data = read_report(&report)?;

    // Verify sketch field is absent
    let configured_entry = configured_entry(&report_data, "root//:target1");
    assert!(
        !configured_entry.contains_key("retained_analysis_memory_sketch"),
        "Sketch should not be present when config is disabled"
    );
    Ok(())
}

// =============================================================================
// Analysis memory peak sketch tests
// =============================================================================

/// Test that peak_analysis_memory_sketch is computed correctly.
///
/// Peak memory should be >= retained memory. For these simple targets that
/// allocate and retain all memory, peak ~= retained.
#[buck_test]
async fn test_peak_analysis_memory_sketch(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // target1 allocates 1M + shared_dep allocates 5M = ~6M total
    let sketches = get_analysis_peak_sketches(buck, tmp_path, &["//:target1"]).await?;
    check_merged_cardinality(&[&sketches["//:target1"]], 6_000_000.0).await
}

/// Test that peak sketch merging works correctly.
///
/// Build two targets that share a dependency, then verify that merging
/// their sketches gives the correct combined cardinality.
#[buck_test]
async fn test_peak_analysis_memory_sketch_merging(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let sketches =
        get_analysis_peak_sketches(buck, tmp_path, &["//:target1", "//:target2"]).await?;

    let sketch1 = &sketches["//:target1"];
    let sketch2 = &sketches["//:target2"];

    // Expected:
    // target1: 1M + 5M (shared) = 6M
    // target2: 1.5M + 5M (shared) = 6.5M
    // merged: 1M + 1.5M + 5M (shared counted once) = 7.5M
    check_merged_cardinality(&[sketch1], 6_000_000.0).await?;
    check_merged_cardinality(&[sketch2], 6_500_000.0).await?;
    check_merged_cardinality(&[sketch1, sketch2], 7_500_000.0).await
}

/// Test that peak sketch is not present when config is disabled.
#[buck_test]
async fn test_peak_analysis_memory_sketch_disabled(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    // Build without peak sketch config
    buck.build(&["//:target1", "--build-report", report.to_str().unwrap()])
        .await?;

    // Read build report
    let report_data = read_report(&report)?;

    // Verify peak sketch field is absent
    let configured_entry = configured_entry(&report_data, "root//:target1");
    assert!(
        !configured_entry.contains_key("peak_analysis_memory_sketch"),
        "Peak sketch should not be present when config is disabled"
    );
    Ok(())
}

/// Test that peak sketch cardinality >= retained sketch cardinality.
///
/// Peak memory includes all allocations during evaluation (including
/// temporaries), while retained only includes what survives freezing.
#[buck_test]
async fn test_peak_analysis_memory_sketch_gte_retained(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:target1",
        "-c",
        "buck2.log_peak_analysis_memory_sketch=true",
        "-c",
        "buck2.log_retained_analysis_memory_sketch=true",
        "--build-report",
        report.to_str().unwrap(),
    ])
    .await?;

    let report_data = read_report(&report)?;

    let entry = configured_entry(&report_data, "root//:target1");
    let peak_sketch = sketch(entry, "peak_analysis_memory_sketch");
    let retained_sketch = sketch(entry, "retained_analysis_memory_sketch");

    let peak_card = get_sketch_cardinality(peak_sketch).await?;
    let retained_card = get_sketch_cardinality(retained_sketch).await?;

    assert!(
        peak_card >= retained_card * 0.95,
        "Peak cardinality ({peak_card:.0}) should be >= retained cardinality ({retained_card:.0})"
    );
    Ok(())
}

/// Test that peak sketch captures temporary memory that is NOT retained.
///
/// target_peak_only allocates 4MB of temporary memory (not stored in any
/// provider) and retains only 1MB. The peak sketch should be significantly
/// larger than the retained sketch because it includes the temporaries.
#[buck_test]
async fn test_analysis_memory_peak_captures_temporaries(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:target_peak_only",
        "-c",
        "buck2.log_peak_analysis_memory_sketch=true",
        "-c",
        "buck2.log_retained_analysis_memory_sketch=true",
        "--build-report",
        report.to_str().unwrap(),
    ])
    .await?;

    let report_data = read_report(&report)?;

    let entry = configured_entry(&report_data, "root//:target_peak_only");
    let peak_sketch = sketch(entry, "peak_analysis_memory_sketch");
    let retained_sketch = sketch(entry, "retained_analysis_memory_sketch");

    let peak_card = get_sketch_cardinality(peak_sketch).await?;
    let retained_card = get_sketch_cardinality(retained_sketch).await?;

    // Peak should be significantly larger than retained because of the
    // 4MB of temporary allocations that are dropped at freeze time.
    // The temp allocation is 4x the retained allocation, so peak should
    // be at least 2x retained even accounting for overhead differences.
    assert!(
        peak_card > retained_card * 2.0,
        "Peak cardinality ({peak_card:.0}) should be significantly larger than retained cardinality ({retained_card:.0}) due to temporary allocations"
    );
    Ok(())
}

// =============================================================================
// Load memory peak sketch tests
// =============================================================================

/// Test that peak_load_memory_sketch is computed and non-empty.
///
/// The load peak sketch captures peak memory from loading .bzl files.
/// The rules.bzl file is loaded during analysis, so the sketch should
/// have a non-zero cardinality.
#[buck_test]
async fn test_peak_load_memory_sketch(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let sketches = get_load_peak_sketches(buck, tmp_path, &["//:target1"]).await?;
    // The bzl files are small, but loading them still allocates some memory.
    assert!(get_sketch_cardinality(&sketches["//:target1"]).await? > 0.0);
    Ok(())
}

/// Test that load peak sketch reflects memory from bzl file globals.
///
/// rules_with_globals.bzl allocates ~1MB of global data at load time.
/// This should be captured in the load peak sketch.
#[buck_test]
async fn test_peak_load_memory_sketch_with_globals(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let sketches = get_load_peak_sketches(buck, tmp_path, &["//:target_with_globals"]).await?;
    // The bzl file allocates ~1MB of globals at load time
    assert!(get_sketch_cardinality(&sketches["//:target_with_globals"]).await? >= 500_000.0);
    Ok(())
}

/// Test that load peak sketch is not present when config is disabled.
#[buck_test]
async fn test_peak_load_memory_sketch_disabled(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    // Build without load peak sketch config
    buck.build(&["//:target1", "--build-report", report.to_str().unwrap()])
        .await?;

    // Read build report
    let report_data = read_report(&report)?;

    // Verify load peak sketch field is absent
    let configured_entry = configured_entry(&report_data, "root//:target1");
    assert!(
        !configured_entry.contains_key("peak_load_memory_sketch"),
        "Load peak sketch should not be present when config is disabled"
    );
    Ok(())
}

/// Test that load and analysis peak sketches separate load/analysis memory.
#[buck_test]
async fn test_load_and_analysis_peak_sketches(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.build(&[
        "//:target_with_load_memory",
        "//:target1",
        "-c",
        "buck2.log_peak_load_memory_sketch=true",
        "-c",
        "buck2.log_peak_analysis_memory_sketch=true",
        "--build-report",
        report.to_str().unwrap(),
    ])
    .await?;

    let report_data = read_report(&report)?;

    // Load includes all package .bzl imports (~2M from rules_for_load_test +
    // rules_with_globals); analysis only refs rules_for_load_test (~1M). Diff ≈ 1M.
    let load_entry = configured_entry(&report_data, "root//:target_with_load_memory");
    let load_memory_peak =
        get_sketch_cardinality(sketch(load_entry, "peak_load_memory_sketch")).await?;
    let analysis_memory_peak =
        get_sketch_cardinality(sketch(load_entry, "peak_analysis_memory_sketch")).await?;
    check_approx(load_memory_peak - analysis_memory_peak, 1_000_000.0);

    // Analysis ≈ 6M (1M target1 + 5M shared_dep). Load includes ~2M from
    // .bzl globals not referenced by target1's analysis. Diff ≈ 4M.
    let analysis_entry = configured_entry(&report_data, "root//:target1");
    let target1_analysis_memory_peak =
        get_sketch_cardinality(sketch(analysis_entry, "peak_analysis_memory_sketch")).await?;
    let target1_load_memory_peak =
        get_sketch_cardinality(sketch(analysis_entry, "peak_load_memory_sketch")).await?;
    check_approx(
        target1_analysis_memory_peak - target1_load_memory_peak,
        4_000_000.0,
    );
    Ok(())
}
