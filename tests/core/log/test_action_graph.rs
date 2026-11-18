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
use buck2_e2e::utils::json_get;
use serde_json::Value;
use serde_json::json;

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
