/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;

fn check_rule_type_names(
    res: &BuckOutput,
    expected_rule_type_names: &[&str],
) -> anyhow::Result<()> {
    let record = res.invocation_record()?;
    assert_eq!(
        record["target_rule_type_names"],
        serde_json::json!(expected_rule_type_names)
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_test_single_dep_touch(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.test(&["//:rule1"]).await?;
    check_rule_type_names(&res, &["one"])
}

#[buck_test(write_invocation_record = true)]
async fn test_test_two_out_of_order(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.test(&["//:rule2", "//:rule1"]).await?;
    check_rule_type_names(&res, &["one", "two"])
}

#[buck_test(write_invocation_record = true)]
async fn test_test_all_in_target(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.test(&["//:"]).await?;
    // Includes `not_a_test` because the recorder receives a
    // `TargetRuleTypeName` event for every CLI-resolved top-level target,
    // not only the ones that resolve to a test rule (T269576064).
    check_rule_type_names(&res, &["not_a_test", "one", "two"])
}

#[buck_test(write_invocation_record = true)]
async fn test_test_all_recursive(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.test(&["//..."]).await?;
    check_rule_type_names(&res, &["not_a_test", "one", "two"])
}

#[buck_test(write_invocation_record = true)]
async fn test_test_non_test_rule_logs_actual_rule_type(buck: &Buck) -> anyhow::Result<()> {
    // `buck test` against a non-test rule logs that rule's actual type in
    // `target_rule_type_names`.
    let res = buck.test(&["//:not_a_test_target"]).await?;
    check_rule_type_names(&res, &["not_a_test"])
}
