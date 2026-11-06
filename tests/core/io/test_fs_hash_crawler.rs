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
use common_io::file_watcher::FileWatcherProvider;
use common_io::file_watcher_dir_tests::run_create_directory_test;
use common_io::file_watcher_dir_tests::run_remove_directory_test;
use common_io::file_watcher_dir_tests::run_rename_directory_test;
use common_io::file_watcher_file_tests::run_create_file_test;
use common_io::file_watcher_file_tests::run_modify_file_test;
use common_io::file_watcher_file_tests::run_remove_file_test;
use common_io::file_watcher_file_tests::run_rename_file_test;
use common_io::file_watcher_file_tests::run_replace_file_test;
use common_io::file_watcher_scm_tests::run_checkout_mergebase_changes_test;
use common_io::file_watcher_scm_tests::run_checkout_with_mergebase_test;
use common_io::file_watcher_scm_tests::run_rebase_with_mergebase_test;
use common_io::file_watcher_scm_tests::run_restack_with_mergebase_test;
use common_io::file_watcher_symlink_tests::run_change_symlink_target_test;
use common_io::file_watcher_symlink_tests::run_create_symlink_test;
use common_io::file_watcher_symlink_tests::run_replace_file_with_symlink_test;
use common_io::file_watcher_tests::FileSystemType;

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_create_file(buck: &Buck) -> anyhow::Result<()> {
    run_create_file_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_modify_file(buck: &Buck) -> anyhow::Result<()> {
    run_modify_file_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_remove_file(buck: &Buck) -> anyhow::Result<()> {
    run_remove_file_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_rename_file(buck: &Buck) -> anyhow::Result<()> {
    run_rename_file_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

// File replace is not supported on Windows
#[buck_test(setup_eden = false, skip_for_os = ["windows"])]
async fn test_fs_hash_cralwer_replace_file(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_create_directory(buck: &Buck) -> anyhow::Result<()> {
    run_create_directory_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_remove_directory(buck: &Buck) -> anyhow::Result<()> {
    run_remove_directory_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_rename_directory(buck: &Buck) -> anyhow::Result<()> {
    run_rename_directory_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_checkout_mergebase_changes(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_mergebase_changes_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_checkout_with_mergebase(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_with_mergebase_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_rebase_with_mergebase(buck: &Buck) -> anyhow::Result<()> {
    run_rebase_with_mergebase_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_cralwer_restack_with_mergebase(buck: &Buck) -> anyhow::Result<()> {
    run_restack_with_mergebase_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_crawler_create_symlink_test(buck: &Buck) -> anyhow::Result<()> {
    run_create_symlink_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_crawler_replace_file_with_symlink_test(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_with_symlink_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}

#[buck_test(setup_eden = false)]
async fn test_fs_hash_crawler_change_symlink_target_test(buck: &Buck) -> anyhow::Result<()> {
    run_change_symlink_target_test(
        buck,
        FileSystemType::Native,
        FileWatcherProvider::FsHashCrawler,
    )
    .await
}
