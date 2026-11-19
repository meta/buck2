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

/// The `labels` of the one node in `cquery -a labels` output.
fn single_labels(stdout: &str) -> anyhow::Result<Vec<String>> {
    let res: BTreeMap<String, BTreeMap<String, Vec<String>>> = serde_json::from_str(stdout)?;
    assert_eq!(res.len(), 1);
    Ok(res.into_values().next().unwrap()["labels"].clone())
}

#[buck_test]
async fn test_transition_info_outgoing_edge(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .cquery(&[
            "root//:base",
            "-u",
            ":pre_outgoing_transition",
            "-a",
            "labels",
        ])
        .await?;
    assert_eq!(single_labels(&res.stdout)?, ["cat"]);

    let res = buck
        .cquery(&[
            "root//:base",
            "-u",
            ":pre_dynamic_outgoing_transition",
            "-a",
            "labels",
        ])
        .await?;
    assert_eq!(single_labels(&res.stdout)?, ["cat"]);
    Ok(())
}

#[buck_test]
async fn test_transition_info_incoming_edge(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .cquery(&[
            "root//:base",
            "-u",
            ":pre_incoming_transition",
            "-a",
            "labels",
        ])
        .await?;
    assert_eq!(single_labels(&res.stdout)?, ["cat"]);
    Ok(())
}

#[buck_test]
async fn test_unexpected_dynamic_outgoing(buck: &Buck) {
    buck.uquery(&["root//unexpected_dynamic:unexpected_dynamic"])
        .await
        .expect_failure()
        .assert_stderr_matches("Expected `str`, but got `tuple");
}
