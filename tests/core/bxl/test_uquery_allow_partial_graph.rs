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

const BXL: &str = "//:allow_partial_graph.bxl:";

#[buck_test]
async fn test_deps_defaults_to_strict(buck: &Buck) {
    buck.bxl(&[&format!("{BXL}deps_default")])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_deps_allow_partial_graph_skips_broken_edge(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}deps_partial")]).await?;
    Ok(())
}

#[buck_test]
async fn test_rdeps_defaults_to_strict(buck: &Buck) {
    buck.bxl(&[&format!("{BXL}rdeps_default")])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_rdeps_allow_partial_graph_skips_broken_package(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}rdeps_partial")]).await?;
    Ok(())
}

#[buck_test]
async fn test_recursive_pattern_as_typed_arg_is_not_covered(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}rdeps_partial_typed_pattern")])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_eval_recursive_defaults_to_strict(buck: &Buck) {
    buck.bxl(&[&format!("{BXL}eval_recursive_default")])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_eval_recursive_allow_partial_graph_skips_broken_package(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}eval_recursive_partial")]).await?;
    Ok(())
}

#[buck_test]
async fn test_explicit_missing_target_fails_with_allow_partial_graph(buck: &Buck) {
    buck.bxl(&[&format!("{BXL}explicit_missing_target")])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `nonexistent`");
}

#[buck_test]
async fn test_explicit_broken_target_fails_with_allow_partial_graph(buck: &Buck) {
    buck.bxl(&[&format!("{BXL}explicit_broken_target")])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_lazy_deps_defaults_to_strict(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}lazy_deps_default")]).await?;
    Ok(())
}

#[buck_test]
async fn test_lazy_deps_allow_partial_graph_skips_broken_edge(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}lazy_deps_partial")]).await?;
    Ok(())
}

#[buck_test]
async fn test_lazy_eval_recursive_defaults_to_strict(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}lazy_eval_recursive_default")])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_lazy_eval_recursive_allow_partial_graph_skips_broken_package(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}lazy_eval_recursive_partial")])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_lazy_explicit_broken_target_fails_with_allow_partial_graph(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.bxl(&[&format!("{BXL}lazy_explicit_broken_target")])
        .await?;
    Ok(())
}
