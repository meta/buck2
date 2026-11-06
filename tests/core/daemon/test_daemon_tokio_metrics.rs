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
use serde_json::Value;

/// Returns the Snapshot payloads from the most recent invocation's log.
async fn snapshot_events(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    let out = buck.log(&["show"]).await?;
    let mut snapshots = Vec::new();
    for line in out.stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(line)?;
        let instant = &event["Event"]["data"]["Instant"]["data"];
        if let Some(snapshot) = instant.get("Snapshot") {
            snapshots.push(snapshot.clone());
        }
    }
    Ok(snapshots)
}

fn append_tokio_workers_config(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[daemon]\nnum_tokio_workers = 42\n",
    )?;
    Ok(())
}

async fn start_daemon_and_get_status(
    buck: &Buck,
    include_tokio_runtime_metrics: bool,
) -> anyhow::Result<Value> {
    // Start the daemon
    buck.targets(&[":"]).await?;

    let status_result = if include_tokio_runtime_metrics {
        buck.status(&["--include-tokio-runtime-metrics"]).await?
    } else {
        buck.status(&[]).await?
    };
    Ok(serde_json::from_str(&status_result.stdout)?)
}

#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_default_output(buck: &Buck) -> anyhow::Result<()> {
    let status = start_daemon_and_get_status(buck, false).await?;
    assert!(status.get("tokio_runtime_metrics").is_none());
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_includes_tokio(buck: &Buck) -> anyhow::Result<()> {
    let status = start_daemon_and_get_status(buck, true).await?;
    assert!(status.get("tokio_runtime_metrics").is_some());
    let metrics = &status["tokio_runtime_metrics"];
    assert!(metrics.get("global_queue_depth").is_some());
    assert!(metrics.get("num_alive_tasks").is_some());
    assert!(metrics.get("num_workers").is_some());
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_notices_tokio_worker_override(buck: &Buck) -> anyhow::Result<()> {
    append_tokio_workers_config(buck)?;
    let status = start_daemon_and_get_status(buck, true).await?;
    assert!(status.get("tokio_runtime_metrics").is_some());
    let metrics = &status["tokio_runtime_metrics"];
    assert!(metrics.get("num_workers").is_some());
    assert_eq!(metrics["num_workers"], 42); // from modification below
    Ok(())
}

#[buck_test]
async fn test_snapshot_events_carry_tokio_runtime_stats(buck: &Buck) -> anyhow::Result<()> {
    // Snapshots should always include tokio_runtime_stats — there's no toggle.
    buck.targets(&[":"]).await?;
    let snapshots = snapshot_events(buck).await?;
    assert!(
        !snapshots.is_empty(),
        "expected at least one snapshot event in the log"
    );
    let populated: Vec<&Value> = snapshots
        .iter()
        .filter(|s| is_truthy(&s["tokio_runtime_stats"]))
        .collect();
    assert!(
        !populated.is_empty(),
        "expected at least one snapshot to carry tokio_runtime_stats"
    );
    let stats = &populated.last().unwrap()["tokio_runtime_stats"];
    assert!(stats.get("workers").is_some());
    let workers = stats["workers"].as_array().unwrap();
    assert!(!workers.is_empty());
    // At least one worker should have done some work, exposing cumulative
    // counters. (Default-zero u64 fields are omitted by proto3 JSON, so we
    // look for a worker that has any of them present.)
    let counter_fields = ["park_unpark_count", "poll_count", "total_busy_duration_us"];
    assert!(
        workers
            .iter()
            .any(|w| counter_fields.iter().any(|f| w.get(f).is_some())),
        "expected some worker to expose any of {counter_fields:?}, got {workers:?}"
    );
    Ok(())
}

/// Whether Python would take `value` as true.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
