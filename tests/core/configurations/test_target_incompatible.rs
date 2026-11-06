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

const INCOMPATIBLE_ERROR: &str = r"root//:incompatible\s*is incompatible with";

async fn check_dep_only_incompatible_soft_err(buck: &Buck, args: &[&str]) -> anyhow::Result<()> {
    buck.cquery(args).await?;
    // Recorded as a deprecation, which is not shown on the console
    let result = buck.log(&["show"]).await?;
    assert!(
        result
            .stdout
            .contains("Soft Error: dep_only_incompatible_version_two")
    );
    let stdout = result.stdout.to_lowercase();
    assert!(stdout.contains(
        "does not pass compatibility check (will be error in future) because its transitive dep root//:incompatible"
    ));
    assert!(stdout.contains("is incompatible with"));
    Ok(())
}

#[buck_test]
async fn test_incompatible_target_skipping(buck: &Buck) -> anyhow::Result<()> {
    // incompatible target should be skipped when a package
    let result = buck.build(&["//:"]).await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets:"));
    assert!(result.stderr.contains("root//:incompatible ("));
    // when explicitly requested, it should be a failure
    buck.build(&["//:incompatible"]).await.expect_failure();
    // should be a failure if it's both explicitly requested and part of a package/recursive pattern
    // TODO(cjhopman): this doesn't work correctly yet
    // buck.build(&["//:", "//:incompatible"]).await.expect_failure();
    Ok(())
}

async fn dep_incompatible_target(buck: &Buck, target_pattern: &str) {
    // a compatible target with incompatible deps should always fail no matter what.
    buck.cquery(&[target_pattern])
        .await
        .expect_failure()
        .assert_stderr_matches(INCOMPATIBLE_ERROR);
    buck.build(&[target_pattern, "--skip-incompatible-targets"])
        .await
        .expect_failure()
        .assert_stderr_matches(INCOMPATIBLE_ERROR);
}

#[buck_test]
async fn test_dep_incompatible_target_dep_incompatible(buck: &Buck) {
    dep_incompatible_target(buck, "//dep_incompatible:").await
}

#[buck_test]
async fn test_dep_incompatible_target_dep_incompatible_dep_incompatible(buck: &Buck) {
    dep_incompatible_target(buck, "//dep_incompatible:dep_incompatible").await
}

#[buck_test]
async fn test_dep_incompatible_target_dep_incompatible_transitive_dep_incompatible(buck: &Buck) {
    dep_incompatible_target(buck, "//dep_incompatible:transitive_dep_incompatible").await
}

#[buck_test]
async fn test_incompatible_target_with_incompatible_dep(buck: &Buck) -> anyhow::Result<()> {
    let target = "//dep_incompatible:target_and_dep_incompatible";
    buck.cquery(&[target]).await?;
    buck.build(&[target, "--skip-incompatible-targets"]).await?;
    buck.build(&[target])
        .await
        .expect_failure()
        .assert_stderr_matches(&format!(r"{target}\s*is incompatible with"));
    Ok(())
}

#[buck_test]
async fn test_exec_dep_transitive_incompatible(buck: &Buck) -> anyhow::Result<()> {
    buck.cquery(&["//exec_dep:one_exec_platform_transitive_incompatible"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_exec_dep_transitive_incompatible_post_transition(buck: &Buck) -> anyhow::Result<()> {
    buck.cquery(&["//exec_dep:one_exec_platform_transitive_incompatible_post_transition"])
        .await?;
    Ok(())
}

async fn error_on_dep_only_incompatible(
    buck: &Buck,
    target_pattern: &str,
    soft_error: bool,
) -> anyhow::Result<()> {
    let config = format!("buck2.error_on_dep_only_incompatible=//some/...,{target_pattern}");
    let args = ["-c", &config, "//dep_incompatible:dep_incompatible"];
    if soft_error {
        check_dep_only_incompatible_soft_err(buck, &args).await?;
    } else {
        buck.cquery(&args)
            .await
            .expect_failure()
            .assert_stderr_matches(INCOMPATIBLE_ERROR);
    }
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
#[expect(
    non_snake_case,
    reason = "the Python harness named the case after its id"
)]
async fn test_error_on_dep_only_incompatible_dep_incompatible_False(
    buck: &Buck,
) -> anyhow::Result<()> {
    error_on_dep_only_incompatible(buck, "//dep_incompatible:", false).await
}

#[buck_test(allow_soft_errors = true)]
#[expect(
    non_snake_case,
    reason = "the Python harness named the case after its id"
)]
async fn test_error_on_dep_only_incompatible_dep_incompatible_False_2(
    buck: &Buck,
) -> anyhow::Result<()> {
    error_on_dep_only_incompatible(buck, "//dep_incompatible/...", false).await
}

#[buck_test(allow_soft_errors = true)]
#[expect(
    non_snake_case,
    reason = "the Python harness named the case after its id"
)]
async fn test_error_on_dep_only_incompatible_False(buck: &Buck) -> anyhow::Result<()> {
    error_on_dep_only_incompatible(buck, "//...", false).await
}

// target pattern doesn't match //dep_incompatible:dep_incompatible
#[buck_test(allow_soft_errors = true)]
#[expect(
    non_snake_case,
    reason = "the Python harness named the case after its id"
)]
async fn test_error_on_dep_only_incompatible_dep_incompatible_dep_incompatible2_True(
    buck: &Buck,
) -> anyhow::Result<()> {
    error_on_dep_only_incompatible(buck, "//dep_incompatible:dep_incompatible2", true).await
}

#[buck_test]
async fn test_error_on_dep_only_incompatible_conf(buck: &Buck) {
    let args = ["//dep_incompatible:dep_incompatible_conf2"];
    buck.cquery(&args)
        .await
        .expect_failure()
        .assert_stderr_matches(INCOMPATIBLE_ERROR);
}

#[buck_test(allow_soft_errors = true)]
async fn test_error_on_dep_only_incompatible_excluded(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "-c",
        "buck2.error_on_dep_only_incompatible_excluded=//dep_incompatible:dep_incompatible_conf2",
        "//dep_incompatible:dep_incompatible_conf2",
    ];
    check_dep_only_incompatible_soft_err(buck, &args).await
}

#[buck_test(allow_soft_errors = true)]
async fn test_dep_only_incompatible_custom_soft_errors_with_exclusions(
    buck: &Buck,
) -> anyhow::Result<()> {
    let config = "buck2.dep_only_incompatible_info=//dep_incompatible/dep_only_incompatible_info:dep_only_incompatible_info_with_exclusions";
    buck.cquery(&["//dep_incompatible:dep_incompatible", "-c", config])
        .await?;
    let result = buck.log(&["show"]).await?;
    assert!(result.stdout.contains("Soft Error: soft_error_one"));
    assert!(result.stdout.contains("Soft Error: soft_error_three"));

    buck.cquery(&[
        "//dep_incompatible:transitive_dep_incompatible",
        "-c",
        config,
    ])
    .await?;
    let result = buck.log(&["show"]).await?;
    assert!(result.stdout.contains("Soft Error: soft_error_two"));
    Ok(())
}
