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
use common_io::file_watcher_replace_tests::run_commit_and_replace_directory_test;
use common_io::file_watcher_replace_tests::run_commit_evaluates_no_untracked_package_test;
use common_io::file_watcher_replace_tests::run_rename_directories_in_one_batch_test;
use common_io::file_watcher_replace_tests::run_replace_directory_file_to_directory_test;
use common_io::file_watcher_replace_tests::run_replace_directory_gone_test;
use common_io::file_watcher_replace_tests::run_replace_directory_missing_to_directory_test;
use common_io::file_watcher_replace_tests::run_replace_directory_nested_test;
use common_io::file_watcher_replace_tests::run_replace_directory_package_file_test;
use common_io::file_watcher_replace_tests::run_replace_directory_same_name_test;
use common_io::file_watcher_replace_tests::run_replace_directory_test;
use common_io::file_watcher_replace_tests::run_replace_empty_directory_test;
use common_io::file_watcher_replace_tests::run_swap_directories_test;
use common_io::file_watcher_tests::run_aba_test;

#[buck_test(setup_eden = true)]
async fn test_edenfs_aba_eden(buck: &Buck) -> anyhow::Result<()> {
    run_aba_test(buck).await
}

// EdenFS reports the rename of a directory, and nothing for what is in it.

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory_nested(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_nested_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory_same_name(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_same_name_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory_gone(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_gone_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory_package_file(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_package_file_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory_file_to_directory(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_file_to_directory_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_directory_missing_to_directory(buck: &Buck) -> anyhow::Result<()> {
    run_replace_directory_missing_to_directory_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_empty_directory(buck: &Buck) -> anyhow::Result<()> {
    run_replace_empty_directory_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_swap_directories(buck: &Buck) -> anyhow::Result<()> {
    run_swap_directories_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_rename_directories_in_one_batch(buck: &Buck) -> anyhow::Result<()> {
    run_rename_directories_in_one_batch_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_changes_evaluate_their_package_only(buck: &Buck) -> anyhow::Result<()> {
    run_changes_evaluate_their_package_only_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_commit_evaluates_no_untracked_package(buck: &Buck) -> anyhow::Result<()> {
    run_commit_evaluates_no_untracked_package_test(buck).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_commit_and_replace_directory(buck: &Buck) -> anyhow::Result<()> {
    run_commit_and_replace_directory_test(buck).await
}
