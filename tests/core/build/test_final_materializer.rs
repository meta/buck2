/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Tests for final materialization performance, focusing on the no-op case
//! where artifacts are already materialized.
//!
//! These tests can be run in two modes:
//! 1. Regular test mode: Uses small N values to verify correctness
//! 2. Performance mode: Uses larger N values to measure materialization overhead
//!
//! To run performance tests, use the perf_* test functions or run this file
//! directly with appropriate arguments.

use std::time::Instant;

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

/// Extract materialization duration from critical path log.
async fn get_materialization_duration_from_critical_path(buck: &Buck) -> Option<f64> {
    let result = buck.log(&["critical-path", "--format=json"]).await.ok()?;
    for line in result.stdout.trim().lines() {
        let entry: Value = serde_json::from_str(line).ok()?;
        if entry.get("kind") == Some(&json!("materialization")) {
            // Duration is in microseconds in the log
            let duration_us = entry
                .get("total_duration_us")
                .map_or(Some(0.0), Value::as_f64)?;
            return Some(duration_us / 1_000_000.0);
        }
    }
    None
}

/// Run a build and measure wall-clock time.
///
/// Returns:
///     Tuple of (wall_clock_seconds, materialization_seconds_from_log)
async fn run_build_and_measure(
    buck: &Buck,
    target: &str,
    extra_args: &[&str],
) -> anyhow::Result<(f64, Option<f64>)> {
    let mut args = vec![target, "--no-remote-cache"];
    args.extend(extra_args);

    let start = Instant::now();
    buck.build(&args).await?;
    let wall_clock = start.elapsed().as_secs_f64();

    let mat_duration = get_materialization_duration_from_critical_path(buck).await;

    Ok((wall_clock, mat_duration))
}

fn min(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::min)
}

fn max(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::max)
}

fn avg(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

// Performance test functions - these can be called directly for benchmarking
// or integrated with a performance testing framework

/// Performance test for no-op materialization.
///
/// Args:
///     buck: Buck instance
///     artifacts_per_node: Artifacts per logical node
///     nodes_per_tset: Nodes per sub-tset
///     number_of_tsets: Number of sub-tsets
///     iterations: Number of no-op builds to measure
///
/// Returns:
///     Dictionary with performance metrics
async fn perf_noop_materialization(
    buck: &Buck,
    artifacts_per_node: u64,
    nodes_per_tset: u64,
    number_of_tsets: u64,
    iterations: usize,
) -> anyhow::Result<Value> {
    let target = "//:configurable";
    let total_artifacts = artifacts_per_node * nodes_per_tset * number_of_tsets;
    let config = [
        format!("test.artifacts_per_node={artifacts_per_node}"),
        format!("test.nodes_per_tset={nodes_per_tset}"),
        format!("test.number_of_tsets={number_of_tsets}"),
    ];
    let config_args: Vec<&str> = config.iter().flat_map(|c| ["-c", c.as_str()]).collect();

    // Initial build to populate artifacts
    let initial_start = Instant::now();
    let mut args = vec![target, "--no-remote-cache"];
    args.extend(&config_args);
    buck.build(&args).await?;
    let initial_duration = initial_start.elapsed().as_secs_f64();

    // Measure no-op builds
    let mut noop_durations: Vec<f64> = Vec::new();
    let mut mat_durations: Vec<f64> = Vec::new();

    for _ in 0..iterations {
        let (wall_clock, mat_duration) = run_build_and_measure(buck, target, &config_args).await?;
        noop_durations.push(wall_clock);
        if let Some(mat_duration) = mat_duration {
            mat_durations.push(mat_duration);
        }
    }

    anyhow::ensure!(!noop_durations.is_empty(), "No no-op builds to measure");
    Ok(json!({
        "artifacts_per_node": artifacts_per_node,
        "nodes_per_tset": nodes_per_tset,
        "number_of_tsets": number_of_tsets,
        "total_artifacts": total_artifacts,
        "initial_build_seconds": initial_duration,
        "noop_iterations": iterations,
        "noop_wall_clock_seconds": {
            "min": min(&noop_durations),
            "max": max(&noop_durations),
            "avg": avg(&noop_durations),
            "all": noop_durations,
        },
        "materialization_seconds": {
            "min": min(&mat_durations),
            "max": max(&mat_durations),
            "avg": avg(&mat_durations),
            "all": mat_durations,
        },
    }))
}

/// Test that building a small flat tset (10 artifacts) works correctly.
#[buck_test]
async fn test_small_flat_build(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:small_flat"]).await?;
    let build_report = result.build_report()?;
    let output = build_report.output_for_target("root//:small_flat")?;
    assert!(output.exists());
    let content = std::fs::read_to_string(&output)?;
    assert!(content.contains("10 artifacts"));
    Ok(())
}

/// Test that building a medium balanced tset (100 artifacts) works correctly.
#[buck_test]
async fn test_medium_balanced_build(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:medium_balanced"]).await?;
    let build_report = result.build_report()?;
    let output = build_report.output_for_target("root//:medium_balanced")?;
    assert!(output.exists());
    let content = std::fs::read_to_string(&output)?;
    assert!(content.contains("100 artifacts"));
    Ok(())
}

/// Test that building a medium flat tset (100 artifacts) works correctly.
#[buck_test]
async fn test_medium_flat_build(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:medium_flat"]).await?;
    let build_report = result.build_report()?;
    let output = build_report.output_for_target("root//:medium_flat")?;
    assert!(output.exists());
    let content = std::fs::read_to_string(&output)?;
    assert!(content.contains("100 artifacts"));
    Ok(())
}

/// Test that building a medium deep tset (100 artifacts) works correctly.
#[buck_test]
async fn test_medium_deep_build(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:medium_deep"]).await?;
    let build_report = result.build_report()?;
    let output = build_report.output_for_target("root//:medium_deep")?;
    assert!(output.exists());
    let content = std::fs::read_to_string(&output)?;
    assert!(content.contains("100 artifacts"));
    Ok(())
}

/// Test no-op materialization for small flat tset.
///
/// Builds the target twice - the second build should have minimal
/// materialization overhead since artifacts are already present.
#[buck_test]
async fn test_noop_materialization_small_flat(buck: &Buck) -> anyhow::Result<()> {
    let target = "//:small_flat";

    // First build - materialize everything
    buck.build(&[target, "--no-remote-cache"]).await?;

    // Second build - should be a no-op for materialization
    let (wall_clock, _mat_duration) = run_build_and_measure(buck, target, &[]).await?;

    // Basic sanity check - second build should complete reasonably fast
    assert!(
        wall_clock < 30.0,
        "No-op build took too long: {wall_clock}s"
    );
    Ok(())
}

/// Test no-op materialization for medium balanced tset.
///
/// Builds the target twice - the second build should have minimal
/// materialization overhead since artifacts are already present.
#[buck_test]
async fn test_noop_materialization_medium_balanced(buck: &Buck) -> anyhow::Result<()> {
    let target = "//:medium_balanced";

    // First build - materialize everything
    buck.build(&[target, "--no-remote-cache"]).await?;

    // Second build - should be a no-op for materialization
    let (wall_clock, _mat_duration) = run_build_and_measure(buck, target, &[]).await?;

    // Basic sanity check - second build should complete reasonably fast
    assert!(
        wall_clock < 30.0,
        "No-op build took too long: {wall_clock}s"
    );
    Ok(())
}

/// Test the configurable target with custom parameters.
#[buck_test]
async fn test_configurable_target(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//:configurable",
            "-c",
            "test.artifacts_per_node=2",
            "-c",
            "test.nodes_per_tset=5",
            "-c",
            "test.number_of_tsets=3",
        ])
        .await?;
    let build_report = result.build_report()?;
    let output = build_report.output_for_target("root//:configurable")?;
    assert!(output.exists());
    let content = std::fs::read_to_string(&output)?;
    // 2 * 5 * 3 = 30 artifacts
    assert!(content.contains("30 artifacts"));
    Ok(())
}

/// Small-scale performance test that runs as part of the test suite.
///
/// This validates that the performance test infrastructure works correctly
/// with a small number of artifacts.
#[buck_test]
async fn test_perf_small_scale(buck: &Buck) -> anyhow::Result<()> {
    let results = perf_noop_materialization(buck, 1, 10, 2, 2).await?;

    assert_eq!(results["total_artifacts"], 20);
    assert_eq!(results["noop_iterations"], 2);
    assert_eq!(
        results["noop_wall_clock_seconds"]["all"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        results["noop_wall_clock_seconds"]["avg"]
            .as_f64()
            .is_some_and(|avg| avg > 0.0)
    );
    Ok(())
}
