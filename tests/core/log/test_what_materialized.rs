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

use buck2_e2e::prelude::*;
use serde_json::Value;

#[buck_test]
async fn test_what_materialized_csv(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:my_rule"]).await?;
    let out = buck.log(&["what-materialized", "--format", "csv"]).await?;
    let header = ["path", "method", "file_count", "total_bytes"];
    let out = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(out.stdout.as_bytes())
        .records()
        .map(|record| {
            Ok(header
                .into_iter()
                .zip(record?.iter().map(str::to_owned))
                .collect::<BTreeMap<_, _>>())
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    assert!(!out.is_empty(), "out should have some materializations");
    assert_eq!(
        out[0],
        header
            .into_iter()
            .map(|h| (h, h.to_owned()))
            .collect::<BTreeMap<_, _>>(),
        "ensure that first entry in csv is the header"
    );
    assert!(
        out.iter()
            .any(|line| line["path"].ends_with("__my_rule__/out")),
        "should have materialized main test file"
    );
    Ok(())
}

#[buck_test]
async fn test_what_materialized_sorted(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:my_rule"]).await?;
    let out = buck
        .log(&["what-materialized", "--format", "json", "--sort-by-size"])
        .await?;
    let out = out
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()?;
    assert!(!out.is_empty(), "out should have some materializations");
    assert!(
        out.windows(2)
            .all(|pair| pair[0]["total_bytes"].as_u64().unwrap()
                <= pair[1]["total_bytes"].as_u64().unwrap()),
        "should be sorted by size"
    );
    Ok(())
}

#[buck_test]
async fn test_what_materialized_aggregated(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:my_rule"]).await?;
    // buck2 log what-materialized --aggregate-by-ext has the following output:
    // <empty>	cas	1	1
    let out = buck
        .log(&["what-materialized", "--aggregate-by-ext"])
        .await?;
    let out: Vec<Vec<&str>> = out
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert!(!out.is_empty(), "out should have some materializations");
    assert_eq!(out[0][0], "<empty>");
    Ok(())
}
