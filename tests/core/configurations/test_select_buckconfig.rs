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
use std::collections::BTreeSet;

use buck2_e2e::prelude::*;

/// The `labels` of the one node in `cquery --output-attribute=labels` output.
fn single_labels(stdout: &str) -> anyhow::Result<Vec<String>> {
    let q: BTreeMap<String, BTreeMap<String, Vec<String>>> = serde_json::from_str(stdout)?;
    assert_eq!(q.len(), 1);
    Ok(q.into_values().next().unwrap()["labels"].clone())
}

/// Test select works with buckconfig.
#[buck_test]
async fn test_select_buckconfig(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&["root//:the-test", "--output-attribute=labels"])
        .await?;
    assert_eq!(single_labels(&out.stdout)?, ["NO"]);

    let out = buck
        .cquery(&[
            "root//:the-test",
            "--output-attribute=labels",
            "-c",
            "aaa.bbb=ccc",
        ])
        .await?;
    assert_eq!(single_labels(&out.stdout)?, ["YES"]);
    Ok(())
}

#[buck_test]
async fn test_select_root_buckconfig(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&[
            "subcell//:the-root-buckconfig-test",
            "--output-attribute=labels",
        ])
        .await?;
    let labels: BTreeSet<String> = single_labels(&out.stdout)?.into_iter().collect();
    assert_eq!(
        labels,
        BTreeSet::from(["ROOT_YES".to_owned(), "TARGET_YES".to_owned()])
    );
    Ok(())
}
