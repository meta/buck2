/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

#[buck_test]
async fn test_forward_node_supports_cutoff(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["--show-output", "root//:main"]).await?;
    // Add a file to the root directory
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join("TARGETS.fixture"))?
        .write_all(b"\n# a comment\n")?;
    buck.targets(&["--show-output", "root//:main"]).await?;

    let events = filter_events(buck, &["Event", "data", "SpanEnd", "data"]).await?;
    let has =
        |ev: &serde_json::Value, key: &str| ev.as_object().is_some_and(|o| o.contains_key(key));
    let loads: Vec<_> = events.iter().filter(|ev| has(ev, "Load")).collect();
    let analyses: Vec<_> = events.iter().filter(|ev| has(ev, "Analysis")).collect();

    assert!(!loads.is_empty());
    // TODO(cjhopman): fix
    assert!(analyses.is_empty(), "should not have analysed anything");
    Ok(())
}
