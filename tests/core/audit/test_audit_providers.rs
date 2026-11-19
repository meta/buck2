/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_hash;

const GOLDEN_DIRECTORY: &str = "modifiers/golden/";

const FILTER_GOLDEN_DIRECTORY: &str = "filter/golden/";

#[buck_test(data_dir = "sorted")]
async fn test_listed_providers_are_sorted(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["providers", "//:target", "--list"]).await?;

    // "  - DefaultInfo" -> "DefaultInfo"
    let providers: Vec<&str> = result
        .stdout
        .split('\n')
        .filter(|line| line.trim().starts_with('-'))
        .map(|line| line.split('-').nth(1).unwrap_or_default().trim())
        .collect();
    assert_eq!(providers, ["AlphaInfo", "DefaultInfo", "ZetaInfo"]);
    Ok(())
}

#[buck_test(data_dir = "universe")]
async fn test_audit_providers_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["providers", "//:aaa", "--quiet"]).await?;
    assert_eq!(
        "root//:aaa (root//:p-aaa#<HASH>)",
        replace_hash(result.stdout.trim())
    );

    let result = buck
        .audit(&["providers", "//:aaa", "--target-universe=//:bbb", "--quiet"])
        .await?;
    assert_eq!(
        "root//:aaa (root//:p-bbb#<HASH>)",
        replace_hash(result.stdout.trim())
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_with_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["providers", "//:dummy?//:macos"]).await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_with_single_modifier.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_with_multiple_target_patterns(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["providers", "//:dummy?//:macos", "//:dummy?//:arm"])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_with_multiple_target_patterns.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_with_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["providers", "//:dummy?//:macos+//:arm"])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_with_multiple_modifiers.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_order_of_modifiers(buck: &Buck) -> anyhow::Result<()> {
    // if passing in modifiers of the same constraint setting,
    // the last one should be the one that applies
    let result = buck
        .audit(&["providers", "//:dummy?//:macos+//:linux"])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_order_of_modifiers.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_all_targets_with_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["providers", "//:?//:macos"]).await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_all_targets_with_modifiers.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_recursive_with_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["providers", "//...?//:macos"]).await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_recursive_with_modifiers.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_modifiers_with_subtarget(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["providers", "//:dummy_with_subtarget[sub]?//:macos"])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_modifiers_with_subtarget.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_modifiers_with_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "providers",
            "//:dummy",
            "--target-universe",
            "//:dummy?//:linux",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{GOLDEN_DIRECTORY}audit_providers_modifiers_with_target_universe.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_modifiers_with_multiple_target_universe(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "providers",
            "//:dummy",
            "--target-universe",
            "//:dummy?//:linux,//:dummy?//:macos",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!(
            "{GOLDEN_DIRECTORY}audit_providers_modifiers_with_multiple_target_universe.golden.txt"
        ),
    );
    Ok(())
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_modifiers_fail_with_global(buck: &Buck) {
    buck.audit(&["providers", "--modifier", "//:linux", "//:dummy?//:arm"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        );

    buck.audit(&[
        "providers",
        "--modifier",
        "//:linux",
        "//:dummy",
        "--target-universe",
        "//:dummy?//:arm",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );
}

#[buck_test(data_dir = "modifiers")]
async fn test_audit_providers_modifiers_fail_with_pattern_modifier_and_target_universe_modifier(
    buck: &Buck,
) {
    buck.audit(&[
        "providers",
        "//:dummy?//:macos",
        "--target-universe",
        "//:dummy?//:linux",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot use \?modifier syntax in target pattern expression with --target-universe flag",
    );
}

#[buck_test(data_dir = "filter")]
async fn test_audit_providers_filter_single(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["providers", "-p", "FooInfo", "//:has_all"])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{FILTER_GOLDEN_DIRECTORY}audit_providers_filter_single.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "filter")]
async fn test_audit_providers_filter_multiple(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["providers", "-p", "FooInfo", "-p", "BarInfo", "//:has_all"])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{FILTER_GOLDEN_DIRECTORY}audit_providers_filter_multiple.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "filter")]
async fn test_audit_providers_filter_not_found(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "providers",
            "-p",
            "Nonexistent1",
            "-p",
            "Nonexistent2",
            "//:has_all",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{FILTER_GOLDEN_DIRECTORY}audit_providers_filter_not_found_stdout.golden.txt"),
    );
    golden_replace_cfg_hash(
        &sanitize_stderr(&result.stderr),
        &format!("{FILTER_GOLDEN_DIRECTORY}audit_providers_filter_not_found_stderr.golden.txt"),
    );
    Ok(())
}

#[buck_test(data_dir = "filter")]
async fn test_audit_providers_filter_multi_target(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "providers",
            "-p",
            "FooInfo",
            "-p",
            "BarInfo",
            "//:has_all",
            "//:has_foo",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        &format!("{FILTER_GOLDEN_DIRECTORY}audit_providers_filter_multi_target_stdout.golden.txt"),
    );
    golden_replace_cfg_hash(
        &sanitize_stderr(&result.stderr),
        &format!("{FILTER_GOLDEN_DIRECTORY}audit_providers_filter_multi_target_stderr.golden.txt"),
    );
    Ok(())
}
