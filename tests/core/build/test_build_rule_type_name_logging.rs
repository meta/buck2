/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

async fn check_rule_type_names(
    buck: &Buck,
    expected_rule_type_names: &[&str],
) -> anyhow::Result<()> {
    let rule_names = filter_events(
        buck,
        &["Result", "result", "build_response", "build_targets"],
    )
    .await?;
    let rule_names = rule_names[0]
        .as_array()
        .context("`build_targets` is not a list")?;
    assert_eq!(rule_names.len(), expected_rule_type_names.len());
    for (actual, expected) in rule_names.iter().zip(expected_rule_type_names) {
        assert_eq!(actual["target_rule_type_name"], *expected);
    }
    Ok(())
}

#[buck_test]
async fn test_build_nested_subtargets(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:nested[sub][nested_sub]"]).await?;
    check_rule_type_names(buck, &["nested_subtargets"]).await
}

#[buck_test]
async fn test_build_single_dep_touch(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:rule1"]).await?;
    check_rule_type_names(buck, &["one"]).await
}

#[buck_test]
async fn test_build_two_out_of_order(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:rule1", "//:nested[sub][nested_sub]"])
        .await?;
    check_rule_type_names(buck, &["nested_subtargets", "one"]).await
}

#[buck_test]
async fn test_build_rule_with_transition(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:a_writer_with_transition"]).await?;

    check_rule_type_names(buck, &["three_with_transition"]).await
}

#[buck_test]
async fn test_build_all_in_target(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:"]).await?;
    check_rule_type_names(
        buck,
        &[
            "two",
            "three_with_transition",
            "nested_subtargets",
            "one",
            "one",
            "platform",
        ],
    )
    .await
}

#[buck_test]
async fn test_build_all_recursive(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//..."]).await?;
    check_rule_type_names(
        buck,
        &[
            "two",
            "three_with_transition",
            "nested_subtargets",
            "one",
            "one",
            "platform",
        ],
    )
    .await
}
