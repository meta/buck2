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
use buck2_e2e::utils::random_string;

#[buck_test]
async fn test_what_uploaded_csv(buck: &Buck) -> anyhow::Result<()> {
    // Use a random content on every test invocation to make sure we actually get uploads
    let content = &random_string()[..20];
    buck.build(&[
        "//:upload_rule",
        "--remote-only",
        "-c",
        &format!("test.content={content}"),
    ])
    .await?;
    let out = buck.log(&["what-uploaded", "--format", "csv"]).await?;
    let header = ["action", "digests_uploaded", "bytes_uploaded"];
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
    assert!(!out.is_empty(), "out should have some uploads");
    assert_eq!(
        out[0],
        header
            .into_iter()
            .map(|h| (h, h.to_owned()))
            .collect::<BTreeMap<_, _>>(),
        "ensure that first entry in csv is the header"
    );
    assert!(
        out[1]["digests_uploaded"].parse::<i64>()? > 0,
        "second entry should be upload digests"
    );
    Ok(())
}

#[buck_test]
async fn test_what_uploaded_aggregated(buck: &Buck) -> anyhow::Result<()> {
    // Use a random content on every test invocation to make sure we actually get uploads
    let content = &random_string()[..20];
    buck.build(&[
        "//:upload_rule",
        "--remote-only",
        "-c",
        &format!("test.content={content}"),
    ])
    .await?;
    let out = buck.log(&["what-uploaded", "--aggregate-by-ext"]).await?;
    let out: Vec<Vec<&str>> = out
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert!(!out.is_empty(), "out should have some uploads");
    assert_eq!(out[0], ["txt", "1", "20"], "unexpected output: {out:?}");
    Ok(())
}
