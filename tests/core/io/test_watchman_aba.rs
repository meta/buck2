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
use common_io::file_watcher_replace_tests::run_changes_evaluate_their_package_only_test;
use common_io::file_watcher_replace_tests::run_commit_evaluates_no_untracked_package_test;
use common_io::file_watcher_replace_tests::run_commit_with_same_contents_evaluates_no_package_test;
use common_io::file_watcher_replace_tests::run_directory_to_file_test;
use common_io::file_watcher_replace_tests::run_empty_directory_to_file_test;
use common_io::file_watcher_replace_tests::run_file_to_directory_test;
use common_io::file_watcher_replace_tests::run_goto_and_back_evaluates_no_package_test;
use common_io::file_watcher_replace_tests::run_goto_in_flight_evaluates_no_package_test;
use common_io::file_watcher_replace_tests::run_replace_directory_and_commit_elsewhere_test;
use common_io::file_watcher_replace_tests::run_replace_directory_tests;
use common_io::file_watcher_replace_tests::run_replace_top_level_directory_test;
use common_io::file_watcher_tests::run_aba_test;

// Watchman answers a query that names a merge base, as these projects ask for with
// `watchman_merge_base`, only in a `.hg` or a `.git` repository. Meta's Sapling makes `.hg`
// repositories, the open source one `.sl` ones.

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_aba_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_aba_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_aba_eden(buck: &Buck) -> anyhow::Result<()> {
    run_aba_test(buck).await
}

// Off EdenFS watchman walks a directory that is renamed and reports what is in it; on EdenFS it
// reports the directory as modified, as it does every directory above a file that a change of
// commit touches.

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_replace_directory_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_tests(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_replace_directory_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_tests(buck).await
}

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_replace_top_level_directory_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_top_level_directory_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_replace_top_level_directory_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_top_level_directory_test(buck).await
}

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_replace_directory_and_commit_elsewhere_no_eden(
    buck: &Buck,
) -> anyhow::Result<()> {
    run_replace_directory_and_commit_elsewhere_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_replace_directory_and_commit_elsewhere_eden(
    buck: &Buck,
) -> anyhow::Result<()> {
    run_replace_directory_and_commit_elsewhere_test(buck).await
}

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_file_to_directory_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_file_to_directory_test(buck).await
}

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_directory_to_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_directory_to_file_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_file_to_directory_eden(buck: &Buck) -> anyhow::Result<()> {
    run_file_to_directory_test(buck).await
}

// On Windows this fails as it does without the fix: buck2 keeps taking the path for a directory.
// Fewer events arrive there than on Linux and macOS, where what was in the directory is reported
// as deleted, which is what tells the file from one whose contents changed.
#[buck_test(setup_eden = true, skip_for_os = ["windows"])]
async fn test_watchman_directory_to_file_eden(buck: &Buck) -> anyhow::Result<()> {
    run_directory_to_file_test(buck).await
}

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_empty_directory_to_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_empty_directory_to_file_test(buck).await
}

#[buck_test(setup_eden = false, meta_only = "Meta's Sapling")]
async fn test_watchman_changes_evaluate_their_package_only_no_eden(
    buck: &Buck,
) -> anyhow::Result<()> {
    run_changes_evaluate_their_package_only_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_changes_evaluate_their_package_only_eden(buck: &Buck) -> anyhow::Result<()> {
    run_changes_evaluate_their_package_only_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_commit_evaluates_no_untracked_package_eden(
    buck: &Buck,
) -> anyhow::Result<()> {
    run_commit_evaluates_no_untracked_package_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_commit_with_same_contents_evaluates_no_package_eden(
    buck: &Buck,
) -> anyhow::Result<()> {
    run_commit_with_same_contents_evaluates_no_package_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_goto_and_back_evaluates_no_package_eden(buck: &Buck) -> anyhow::Result<()> {
    run_goto_and_back_evaluates_no_package_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_goto_in_flight_evaluates_no_package_eden(buck: &Buck) -> anyhow::Result<()> {
    run_goto_in_flight_evaluates_no_package_test(buck).await
}
