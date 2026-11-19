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

async fn check_has_uquery_path(
    buck: &Buck,
    target: &str,
    dep: &str,
    expect_fail: bool,
) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[&format!("somepath({target}, {dep})")])
        .await?;
    let path: Vec<&str> = result.stdout.lines().collect();
    // Apparently, configuration deps never show up in `somepath`. Interesting.
    assert_eq!(path.len(), 0);

    let result = buck
        .uquery(&[
            &format!("deps({target})"),
            "-a",
            "buck.deps",
            "-a",
            "buck.configuration_deps",
        ])
        .await?;
    let nodes: BTreeMap<String, BTreeMap<String, Vec<String>>> =
        serde_json::from_str(&result.stdout)?;
    let all_deps: Vec<&String> = nodes
        .values()
        .flat_map(|node| node.values())
        .flatten()
        .collect();
    if expect_fail {
        assert!(!all_deps.iter().any(|d| *d == dep));
    } else {
        assert!(all_deps.iter().any(|d| *d == dep));
    }
    Ok(())
}

#[buck_test]
async fn test_default_target_platform(buck: &Buck) -> anyhow::Result<()> {
    check_has_uquery_path(buck, ":with_custom_dtp", "root//:base", false).await
}

#[buck_test]
async fn test_configured_dep_platform(buck: &Buck) -> anyhow::Result<()> {
    check_has_uquery_path(buck, ":stub_configured", "root//:base", false).await
}

#[buck_test]
async fn test_transition_dep_refs(buck: &Buck) -> anyhow::Result<()> {
    // FIXME(JakobDegen): Bug.
    check_has_uquery_path(buck, ":pre_out_transition", "root//:cat", true).await?;

    // FIXME(JakobDegen): Bug.
    check_has_uquery_path(buck, ":post_out_transition", "root//:cat", true).await?;

    check_has_uquery_path(buck, ":pre_out_transition_vnew", "root//:transition", false).await?;

    check_has_uquery_path(buck, ":pre_inc_transition_vnew", "root//:transition", false).await
}

#[buck_test]
async fn test_select_keys(buck: &Buck) -> anyhow::Result<()> {
    check_has_uquery_path(buck, ":with_select", "root//:cat", false).await
}
