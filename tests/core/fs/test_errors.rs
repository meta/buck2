/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;

async fn check(buck: &Buck, pattern: &str, golden_name: &str) {
    let res = buck
        .uquery(&[pattern, "-v=0", "--console=none"])
        .await
        .expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        &format!("golden/{golden_name}.golden.stderr"),
    );
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_missing_dir(buck: &Buck) {
    check(
        buck,
        "//completely_missing/foo/x/y/lmnop:target",
        "missing_dir",
    )
    .await;
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_missing_targets_file(buck: &Buck) {
    check(
        buck,
        "//missing_targets_file:target",
        "missing_targets_file",
    )
    .await;
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_path_is_file_leaf(buck: &Buck) {
    check(buck, "//data.file:target", "path_is_file_leaf").await;
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_path_is_file_intermediate(buck: &Buck) {
    check(
        buck,
        "//data.file/subdir:target",
        "path_is_file_intermediate",
    )
    .await;
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_top_level_typo(buck: &Buck) {
    // `top_level_present/` is a real directory; the query mistypes it.
    check(buck, "//top_level_presents:target", "top_level_typo").await;
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_subdir_typo(buck: &Buck) {
    // `typo_subdir/subdir/` is real; the query mistypes the leaf.
    check(buck, "//typo_subdir/subdirs:target", "subdir_typo").await;
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_wrong_cell_suggestion(buck: &Buck) {
    // `something/` exists at the root of the `cell` cell, not in `root`.
    check(buck, "root//something:target", "wrong_cell_suggestion").await;
}
