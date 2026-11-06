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
use serde_json::Value;

#[buck_test]
async fn test_bxl_build(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//build.bxl:build_test", "--", "--target", ":trivial_build"])
        .await?;
    let outputs: Value = serde_json::from_str(&result.stdout)?;
    let output = outputs["root//:trivial_build"][0]
        .as_str()
        .expect("an output path");
    assert_eq!(std::fs::read_to_string(buck.cwd().join(output))?, "abcd");

    let result = buck.bxl(&["//build.bxl:cquery_build_test"]).await?;
    let outputs = result.stdout.lines().next().expect("an output line");
    assert_eq!(std::fs::read_to_string(buck.cwd().join(outputs))?, "abcd");
    Ok(())
}

#[buck_test]
async fn test_bxl_build_stats(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//build.bxl:build_stats",
            "--",
            "--targets",
            "root//build/...",
        ])
        .await?;
    let stats: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(stats["root//build:pass"]["artifacts"], 1);
    assert_eq!(stats["root//build:pass"]["failures"], 0);
    assert_eq!(stats["root//build:fail"]["artifacts"], 0);
    assert_eq!(stats["root//build:fail"]["failures"], 1);
    Ok(())
}

#[buck_test]
async fn test_bxl_target_platform_from_unpacking_providers_expr(buck: &Buck) -> anyhow::Result<()> {
    // Pass in explicit target platform from client. Result should be configured with this target platform.
    let result = buck
        .bxl(&[
            "--target-platforms",
            "root//:platform2",
            "//build.bxl:build_with_target_platform_test",
            "--",
            "--target",
            ":trivial_build",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//:trivial_build (root//:platform2#<HASH>)]\n"
    );

    // No target platform specified from client context. Result should be configured with root//:platform1
    let result = buck
        .bxl(&[
            "//build.bxl:build_with_target_platform_test",
            "--",
            "--target",
            ":trivial_build",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//:trivial_build (root//:platform3#<HASH>)]\n"
    );

    // Target platform from client context should be overridden by what's declared in build().
    let result = buck
        .bxl(&[
            "//build.bxl:build_with_target_platform_test",
            "--target-platforms",
            "root//:platform2",
            "--",
            "--target",
            ":trivial_build",
            "--target_platform",
            "root//:platform1",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//:trivial_build (root//:platform1#<HASH>)]\n"
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_build_order(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//build_artifacts_order/check.bxl:check"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_build_no_materialization(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//materializations.bxl:build",
            "--",
            "--materializations=skip",
        ])
        .await?;

    let [output] = result.stdout.lines().collect::<Vec<_>>()[..] else {
        panic!("Expected one line, got:\n{}", result.stdout);
    };
    assert!(!buck.cwd().join(output).exists());

    let result = buck
        .bxl(&[
            "//materializations.bxl:build",
            "--",
            "--materializations=materialize",
        ])
        .await?;

    let [output] = result.stdout.lines().collect::<Vec<_>>()[..] else {
        panic!("Expected one line, got:\n{}", result.stdout);
    };
    assert!(buck.cwd().join(output).exists());
    Ok(())
}
