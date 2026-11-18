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

/// The records `buck2 log graph` exports from the last command's log.
async fn graph(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    let out = buck.log(&["graph"]).await?;
    out.stdout
        .lines()
        .map(|line| serde_json::from_str(line).with_context(|| format!("Parsing `{line}`")))
        .collect()
}

/// The last record, which holds the export's statistics.
fn stats(records: &[Value]) -> anyhow::Result<&Value> {
    records.last().context("No records")
}

/// The records of `kind`, keyed by category.
fn by_category<'a>(records: &'a [Value], kind: &str) -> BTreeMap<&'a str, &'a Value> {
    records
        .iter()
        .filter(|r| r["kind"] == kind)
        .map(|r| (r["category"].as_str().unwrap_or_default(), r))
        .collect()
}

/// The action record of `category` and `identifier`.
fn action<'a>(records: &'a [Value], category: &str, identifier: &str) -> anyhow::Result<&'a Value> {
    records
        .iter()
        .find(|r| {
            r["kind"] == "action" && r["category"] == category && r["identifier"] == identifier
        })
        .with_context(|| format!("No `{category}` action `{identifier}`"))
}

/// The one element of a JSON array.
fn only(values: &Value) -> anyhow::Result<&Value> {
    let [value] = values.as_array().map(Vec::as_slice).unwrap_or_default() else {
        anyhow::bail!("Expected one element: {values}");
    };
    Ok(value)
}

fn of_kind<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["kind"] == kind).collect()
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

#[buck_test]
async fn test_log_graph_exports_the_recorded_graph(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy", "--log-action-graph"]).await?;
    let records = graph(buck).await?;
    assert_eq!(records[0]["kind"], "header");
    let actions = by_category(&records, "action");
    assert_eq!(actions["copy"]["deps"], json!([actions["write"]["span"]]));
    let stats = stats(&records)?;
    assert_eq!(stats["kind"], "stats");
    assert_eq!(
        json!([stats["actions"], stats["edges"], stats["dangling"]]),
        json!([2, 1, 0])
    );
    assert_eq!(stats["schema"], 1);
    assert_eq!(stats["footer"]["action_ends"], 2);
    Ok(())
}

#[buck_test]
async fn test_log_graph_refuses_an_unrecorded_log(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy"]).await?;
    buck.log(&["graph"])
        .await
        .expect_failure()
        .assert_stderr_matches("no recorded action graph");
    Ok(())
}

#[buck_test]
async fn test_transitive_set_definitions(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:consume", "--log-action-graph"]).await?;
    let records = graph(buck).await?;
    let tsets: BTreeMap<String, &Value> = records
        .iter()
        .filter(|r| r["kind"] == "tset")
        .map(|r| (r["id"].to_string(), r))
        .collect();
    let l1 = action(&records, "write", "l1.txt")?;
    let l2 = action(&records, "write", "l2.txt")?;
    let consume = records
        .iter()
        .find(|r| r["kind"] == "action" && r["category"] == "consume")
        .context("No `consume` action")?;

    let outer = tsets[&only(&consume["tsets"])?.to_string()];
    assert_eq!(outer["own"], json!([l2["span"]]));
    let inner = tsets[&only(&outer["children"])?.to_string()];
    assert_eq!(inner["own"], json!([l1["span"]]));
    assert_eq!(inner["children"], json!([]));

    let stats = stats(&records)?;
    assert_eq!(
        json!([stats["dangling"], stats["dangling_tsets"]]),
        json!([0, 0])
    );
    assert_eq!(stats["footer"]["tset_def_events"], 2);
    assert_eq!(stats["tset_def_events"], 2);
    assert!(json_get(stats, &["footer", "incomplete"]).is_none());
    Ok(())
}

#[buck_test]
async fn test_nothing_is_defined_when_nothing_ran(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy"]).await?;
    buck.build(&["//:copy", "--log-action-graph"]).await?;
    let records = graph(buck).await?;
    let stats = stats(&records)?;
    assert_eq!(
        json!([stats["actions"], stats["structures"], stats["tsets"]]),
        json!([0, 0, 0])
    );
    assert_eq!(stats["footer"]["structure_defs"], 0);
    assert_eq!(stats["complete"], true);
    Ok(())
}

#[buck_test]
async fn test_warm_rebuild_keeps_chains_connected(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy"]).await?;
    buck.build(&["//:copy2", "--log-action-graph"]).await?;
    let records = graph(buck).await?;
    let [copy2] = of_kind(&records, "action")[..] else {
        anyhow::bail!("Expected one action: {records:?}");
    };
    let [write] = of_kind(&records, "structure")[..] else {
        anyhow::bail!("Expected one structure: {records:?}");
    };
    assert_eq!(
        copy2["reused"],
        json!([write["id"]]),
        "the reused producer is defined by a structure"
    );
    assert_eq!(stats(&records)?["dangling_reused"], 0);
    Ok(())
}

#[buck_test]
async fn test_dynamic_output_alias(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:copy_dyn", "--log-action-graph"]).await?;
    let records = graph(buck).await?;
    let actions = of_kind(&records, "action");
    let copy = actions
        .iter()
        .find(|a| a["category"] == "copy")
        .context("No `copy` action")?;
    let [alias] = of_kind(&records, "alias")[..] else {
        anyhow::bail!("Expected one alias: {records:?}");
    };
    assert_eq!(
        copy["reused"],
        json!([alias["id"]]),
        "the copy refers to the output by the id of its declared key"
    );
    assert!(
        actions.iter().any(|a| a["span"] == alias["canonical_span"]),
        "the bound action ran"
    );
    let stats = stats(&records)?;
    assert_eq!(
        json!([stats["dangling"], stats["dangling_reused"]]),
        json!([0, 0])
    );
    assert_eq!(stats["structures"], 0);
    assert_eq!(stats["footer"]["aliases"], 1);
    Ok(())
}

#[buck_test]
async fn test_complete_build_is_complete(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:consume", "--log-action-graph"]).await?;
    let records = graph(buck).await?;
    let stats = stats(&records)?;
    assert_eq!(stats["complete"], true);
    assert_eq!(stats["integrity"], json!([]));
    Ok(())
}
