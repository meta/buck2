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

#[buck_test]
async fn test_target(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.aquery(&["//:test", "-a", "identifier"]).await?.stdout;

    golden(&stdout, "target.golden.json");
    Ok(())
}

#[buck_test]
async fn test_all_outputs(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .aquery(&["all_outputs(//:test)", "-a", "identifier"])
        .await?
        .stdout;

    golden(&stdout, "all_outputs.golden.json");
    Ok(())
}

#[buck_test]
async fn test_all_actions(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .aquery(&["all_actions(//:test)", "-a", "identifier"])
        .await?
        .stdout;

    golden(&stdout, "all_actions.golden.json");
    Ok(())
}

#[buck_test]
async fn test_all_outputs_subtarget(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .aquery(&["all_outputs('//:test[sub]')", "-a", "identifier"])
        .await?
        .stdout;

    golden(&stdout, "all_outputs_subtarget.golden.json");
    Ok(())
}

#[buck_test]
async fn test_filter(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .aquery(&[
            "attrfilter('identifier', 'other', all_actions('//:test[sub]'))",
            "-a",
            "identifier",
        ])
        .await?
        .stdout;

    golden(&stdout, "filter.golden.json");
    Ok(())
}

#[buck_test]
async fn test_deps(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .aquery(&["deps(//:test)", "-a", "identifier"])
        .await?
        .stdout;

    golden(&stdout, "deps.golden.json");
    Ok(())
}

#[buck_test]
async fn test_deps_bounded(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .aquery(&["deps(//:test, 1)", "-a", "identifier"])
        .await?
        .stdout;

    golden(&stdout, "deps_bounded.golden.json");
    Ok(())
}

#[buck_test]
async fn test_rdeps(buck: &Buck) -> anyhow::Result<()> {
    // The universe contains an analysis node (from the target literal), which the
    // flattened-graph rdeps used to fail on with `Not an action`.
    let stdout = buck
        .aquery(&["rdeps(deps(//:test), deps(//:test))", "-a", "identifier"])
        .await?
        .stdout;

    golden(&stdout, "rdeps.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_target(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.bxl(&["//:aquery.bxl:target"]).await?.stdout;
    golden(&stdout, "bxl_target.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_all_outputs(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.bxl(&["//:aquery.bxl:all_outputs"]).await?.stdout;

    golden(&stdout, "bxl_all_outputs.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_all_actions(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.bxl(&["//:aquery.bxl:all_actions"]).await?.stdout;

    golden(&stdout, "bxl_all_actions.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_all_outputs_subtarget(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .bxl(&["//:aquery.bxl:all_outputs_subtarget"])
        .await?
        .stdout;

    golden(&stdout, "bxl_all_outputs_subtarget.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_attrfilter(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.bxl(&["//:aquery.bxl:attrfilter"]).await?.stdout;

    golden(&stdout, "bxl_filter.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_deps(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.bxl(&["//:aquery.bxl:deps"]).await?.stdout;

    golden(&stdout, "bxl_deps.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_eval(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.bxl(&["//:aquery.bxl:eval"]).await?.stdout;

    golden(&stdout, "bxl_eval.golden.json");
    Ok(())
}

#[buck_test]
async fn test_bxl_aquery_action_query_node(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:aquery.bxl:action_query_node"]).await?;
    Ok(())
}

// Tests for bxl.Action support in aquery operations
/// Test passing a bxl.Action to aquery.deps() with depth=0 returns itself
#[buck_test]
async fn test_bxl_action_deps_0(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:aquery.bxl:action_deps_0"]).await?;
    Ok(())
}

/// Test that you can isolate the deps of one action by itself
#[buck_test]
async fn test_bxl_action_deps(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:aquery.bxl:action_deps"]).await?;
    Ok(())
}

/// Test bxl.atarget_set() creates a target set from ActionQueryNodes
#[buck_test]
async fn test_bxl_atarget_set(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:aquery.bxl:atarget_set_test"]).await?;
    Ok(())
}

/// Test analysis.label() returns the configured providers label
#[buck_test]
async fn test_bxl_analysis_label(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:aquery.bxl:analysis_label_test"]).await?;
    Ok(())
}
