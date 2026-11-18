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

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_event_roots;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::json_get;
use serde_json::Value;
use serde_json::json;

/// `ActionGraphFooter.Incomplete` values.
const BUILD_FAILED: u64 = 1;
const FAIL_FAST: u64 = 2;

/// The `ActionExecution` span ends of the last command, keyed by action category, each with its
/// span's id as `span_id`.
async fn action_ends(buck: &Buck) -> anyhow::Result<BTreeMap<String, Value>> {
    let events = filter_event_roots(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    events
        .iter()
        .map(|event| {
            let mut end = event["Event"]["data"]["SpanEnd"]["data"]["ActionExecution"].clone();
            let category = end["name"]["category"]
                .as_str()
                .with_context(|| format!("No category in {end}"))?
                .to_owned();
            end["span_id"] = event["Event"]["span_id"].clone();
            Ok((category, end))
        })
        .collect()
}

async fn graph_events(buck: &Buck, name: &str) -> anyhow::Result<Vec<Value>> {
    filter_events(buck, &["Event", "data", "Instant", "data", name]).await
}

/// The one `name` event of the last command.
async fn graph_event(buck: &Buck, name: &str) -> anyhow::Result<Value> {
    let events = graph_events(buck, name).await?;
    let [event] = events.as_slice() else {
        anyhow::bail!("Expected one `{name}`, got {events:?}");
    };
    Ok(event.clone())
}

#[buck_test]
async fn test_edges_join_by_span_id(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy", "--log-action-graph"]).await?;
    let ends = action_ends(buck).await?;
    let (write, copy) = (&ends["write"], &ends["copy"]);
    assert_eq!(
        copy["graph_producers"]["span_ids"],
        json!([write["span_id"]]),
        "the copy's producer ran in this command"
    );
    assert_eq!(copy["graph_producers"]["reused_ids"], json!([]));
    assert_eq!(
        write["graph_producers"]["span_ids"],
        json!([]),
        "the write has no producers"
    );
    Ok(())
}

#[buck_test]
async fn test_nothing_recorded_without_the_flag(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy"]).await?;
    let ends = action_ends(buck).await?;
    assert!(!ends.is_empty());
    assert!(
        ends.values()
            .all(|end| json_get(end, &["graph_producers"]).is_none())
    );
    Ok(())
}

#[buck_test]
async fn test_one_footer_counting_the_action_ends(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy", "--log-action-graph"]).await?;
    let start = graph_event(buck, "ActionGraphStart").await?;
    let footer = graph_event(buck, "ActionGraphFooter").await?;
    assert_eq!(start["schema"], 1);
    assert_eq!(footer["schema"], 1);
    assert_eq!(action_ends(buck).await?.len(), 2);
    assert_eq!(footer["action_ends"], 2);
    assert!(json_get(&footer, &["incomplete"]).is_none());
    Ok(())
}

#[buck_test]
async fn test_footer_marks_a_failed_build_incomplete(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:fail", "--log-action-graph"])
        .await
        .expect_failure();
    let footer = graph_event(buck, "ActionGraphFooter").await?;
    assert_eq!(footer["incomplete"], BUILD_FAILED);
    Ok(())
}

#[buck_test]
async fn test_footer_marks_fail_fast_incomplete(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:fail", "//:copy", "--fail-fast", "--log-action-graph"])
        .await
        .expect_failure();
    let footer = graph_event(buck, "ActionGraphFooter").await?;
    assert_eq!(footer["incomplete"], FAIL_FAST);
    Ok(())
}

#[buck_test]
async fn test_no_start_or_footer_without_the_flag(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy"]).await?;
    assert!(graph_events(buck, "ActionGraphStart").await?.is_empty());
    assert!(graph_events(buck, "ActionGraphFooter").await?.is_empty());
    Ok(())
}
