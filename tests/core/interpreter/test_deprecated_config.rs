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

async fn test_deprecated_config_impl(buck: &Buck, section: &str, root: &str) {
    buck.build(&[
        &format!(":test_target_{section}_config1"),
        "-c",
        &format!("test.section={section}"),
        "-c",
        "test.conf=config1",
        "-c",
        &format!("test.root={root}"),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "{section}.config1 is no longer used. Please use other.config2"
    ));
}

#[buck_test]
async fn test_deprecated_config_true_some(buck: &Buck) {
    test_deprecated_config_impl(buck, "some", "true").await
}

#[buck_test]
async fn test_deprecated_config_true_other(buck: &Buck) {
    test_deprecated_config_impl(buck, "other", "true").await
}

#[buck_test]
async fn test_deprecated_config_false_some(buck: &Buck) {
    test_deprecated_config_impl(buck, "some", "false").await
}

#[buck_test]
async fn test_deprecated_config_false_other(buck: &Buck) {
    test_deprecated_config_impl(buck, "other", "false").await
}

async fn test_not_deprecated_config_impl(buck: &Buck, root: &str) -> anyhow::Result<()> {
    let section = "other";
    buck.build(&[
        &format!(":test_target_{section}_config2"),
        "-c",
        &format!("test.section={section}"),
        "-c",
        "test.conf=config2",
        "-c",
        &format!("test.root={root}"),
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_not_deprecated_config_true(buck: &Buck) -> anyhow::Result<()> {
    test_not_deprecated_config_impl(buck, "true").await
}

#[buck_test]
async fn test_not_deprecated_config_false(buck: &Buck) -> anyhow::Result<()> {
    test_not_deprecated_config_impl(buck, "false").await
}

#[buck_test]
async fn test_no_deprecated_cell_config(buck: &Buck) -> anyhow::Result<()> {
    let section = "other";
    buck.build(&[
        &format!("cell//:test_target_{section}_config1"),
        "-c",
        &format!("test.section={section}"),
        "-c",
        "test.conf=config1",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_deprecated_cell_config2(buck: &Buck) {
    let section = "other";
    buck.build(&[
        &format!("cell//:test_target_{section}_config2"),
        "-c",
        &format!("test.section={section}"),
        "-c",
        "test.conf=config2",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(
        "{section}.config2 is no longer used. Please use other.config3"
    ));
}
