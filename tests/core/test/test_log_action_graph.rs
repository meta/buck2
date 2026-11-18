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
use buck2_e2e::utils::json_get;
use serde_json::Value;

/// The one action graph footer of the last command.
async fn footer(buck: &Buck) -> anyhow::Result<Value> {
    let footers = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "ActionGraphFooter"],
    )
    .await?;
    let [footer] = footers.as_slice() else {
        anyhow::bail!("Expected one action graph footer, got {footers:?}");
    };
    Ok(footer.clone())
}

#[buck_test]
async fn test_test_records_the_action_graph(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["//:passing", "--log-action-graph"]).await?;
    let footer = footer(buck).await?;
    assert_eq!(footer["action_ends"], 1, "the test's data file was written");
    assert!(json_get(&footer, &["incomplete"]).is_none());
    Ok(())
}

#[buck_test]
async fn test_a_failing_test_leaves_the_graph_complete(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["//:failing", "--log-action-graph"])
        .await
        .expect_failure();
    let footer = footer(buck).await?;
    assert!(
        json_get(&footer, &["incomplete"]).is_none(),
        "a failing test is not a failed build"
    );
    Ok(())
}
