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
use buck2_e2e::utils::replace_hash;

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_new_target_set(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/new_target_set.bxl:new_ctarget_set"])
        .await?;

    buck.bxl(&["//bxl/new_target_set.bxl:new_utarget_set"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_target_set_ops(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/target_set_ops.bxl:test_operations"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_target_platform_from_value_as_starlark_target_label(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Pass in explicit target platform from client. Result should be configured with this target platform.
    let result = buck
        .bxl(&[
            "--target-platforms",
            "root//platforms:platform2",
            "//bxl/cquery.bxl:owner_test",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform2#<HASH>)]\n"
    );

    // No target platform specified from client context. Result should be configured with root//platforms:platform1
    let result = buck.bxl(&["//bxl/cquery.bxl:owner_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>)]\n"
    );

    // Target platform from client context should be overridden by what's declared in cquery.
    let result = buck
        .bxl(&[
            "--target-platforms",
            "root//platforms:platform2",
            "//bxl/cquery.bxl:owner_test_with_target_platform",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>)]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_unconfigured_sub_targets(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl/providers.bxl:unconfigured_sub_targets"])
        .await?;

    let output: serde_json::Value = serde_json::from_str(result.stdout.trim())?;
    assert_eq!(output["lib1"], "root//lib:lib1");
    assert_eq!(output["lib1_FooInfo"], "root//lib:lib1[FooInfo]");
    assert_eq!(output["lib2"], "root//lib:lib2");
    assert_eq!(output["lib3_FooInfo"], "root//lib:lib3[FooInfo]");
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_target_exists(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/target_exists.bxl:target_exists"]).await?;

    buck.bxl(&["//bxl/target_exists.bxl:target_exists_no_target_patterns"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Expected a single target as a string literal, not a target pattern",
        );

    buck.bxl(&["//bxl/target_exists.bxl:target_exists_no_subtargets"])
        .await
        .expect_failure()
        .assert_stderr_matches("Expecting target pattern, without providers");
    Ok(())
}
