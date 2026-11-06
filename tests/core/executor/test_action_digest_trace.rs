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
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use serde_json::Value;

/// Get all ActionDigestTrace events from the log.
async fn get_action_digest_traces(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(
        buck,
        &["Event", "data", "Instant", "data", "ActionDigestTrace"],
    )
    .await
}

/// The `event` of each trace, as `t.get("event")` gives it.
fn trace_events(traces: &[Value]) -> Vec<Value> {
    traces
        .iter()
        .map(|t| t.get("event").cloned().unwrap_or(Value::Null))
        .collect()
}

/// Test that cache miss events are traced.
#[buck_test(env = [("BUCK2_ACTION_DIGEST_TRACE_LG_SAMPLE_RATE", "0")])] // Sample all digests
async fn test_action_digest_trace_cache_miss(buck: &Buck) -> anyhow::Result<()> {
    // Make sure action is not cached by using a random input
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;

    buck.build(&["root//:simple", "--remote-only"]).await?;

    let traces = get_action_digest_traces(buck).await?;
    // Should have at least one trace event
    assert!(
        !traces.is_empty(),
        "Expected at least 1 trace event, got {}",
        traces.len()
    );

    // Check for cache miss or remote execution events
    let events = trace_events(&traces);
    // Event values: CACHE_HIT=1, CACHE_MISS=2, REMOTE_EXECUTION=3, CACHE_UPLOAD=4
    // With --no-remote-cache we skip cache lookup, so we should see REMOTE_EXECUTION
    assert!(
        events.iter().any(|e| *e == 3),
        "Expected REMOTE_EXECUTION event, got events: {events:?}"
    );
    Ok(())
}

/// Test that cache hit events are traced.
#[buck_test(env = [("BUCK2_ACTION_DIGEST_TRACE_LG_SAMPLE_RATE", "0")])] // Sample all digests
async fn test_action_digest_trace_cache_hit(buck: &Buck) -> anyhow::Result<()> {
    // First build to populate cache
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;

    buck.build(&["root//:simple", "--remote-only"]).await?;

    buck.kill().await?;
    // Second build should hit cache
    buck.build(&["root//:simple", "--remote-only"]).await?;

    let traces = get_action_digest_traces(buck).await?;
    // Should have at least one trace event
    assert!(
        !traces.is_empty(),
        "Expected at least 1 trace event, got {}",
        traces.len()
    );

    // Check for cache hit event
    let events = trace_events(&traces);
    // Event values: CACHE_HIT=1
    assert!(
        events.iter().any(|e| *e == 1),
        "Expected CACHE_HIT event, got events: {events:?}"
    );
    Ok(())
}

/// Test that sampling works - with very high rate, nothing should be sampled.
#[buck_test(env = [("BUCK2_ACTION_DIGEST_TRACE_LG_SAMPLE_RATE", "64")])] // Sample rate so high nothing is sampled
async fn test_action_digest_trace_sampling(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;

    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;

    let traces = get_action_digest_traces(buck).await?;
    // With sampling rate of 64 bits (impossible), should have no traces
    assert!(
        traces.is_empty(),
        "Expected 0 trace events with high sampling rate, got {}",
        traces.len()
    );
    Ok(())
}
