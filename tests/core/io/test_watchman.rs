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
use common_io::file_watcher::FileWatcherEvent;
use common_io::file_watcher::FileWatcherEventType;
use common_io::file_watcher::FileWatcherKind;
use common_io::file_watcher::FileWatcherProvider;
use common_io::file_watcher::get_file_watcher_events;
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
use common_io::file_watcher_scm_tests::setup_file_watcher_scm_test;
use common_io::file_watcher_symlink_tests::run_change_symlink_target_test;
use common_io::file_watcher_symlink_tests::run_create_symlink_test;
use common_io::file_watcher_symlink_tests::run_replace_file_with_symlink_test;
use common_io::file_watcher_tests::FileSystemType;
use common_io::file_watcher_tests::setup_file_watcher_test;
use common_io::file_watcher_tests::verify_results;

#[buck_test(setup_eden = false)]
async fn test_watchman_create_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_create_file_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_create_file_eden(buck: &Buck) -> anyhow::Result<()> {
    run_create_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_modify_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_modify_file_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_modify_file_eden(buck: &Buck) -> anyhow::Result<()> {
    run_modify_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_remove_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_remove_file_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_remove_file_eden(buck: &Buck) -> anyhow::Result<()> {
    run_remove_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_rename_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_rename_file_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_rename_file_eden(buck: &Buck) -> anyhow::Result<()> {
    run_rename_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

// File replace is not supported on Windows
#[buck_test(setup_eden = false, skip_for_os = ["windows"])]
async fn test_watchman_replace_file_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

// File replace is not supported on Windows
#[buck_test(setup_eden = true, skip_for_os = ["windows"])]
async fn test_watchman_replace_file_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_create_directory_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_create_directory_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_create_directory_eden(buck: &Buck) -> anyhow::Result<()> {
    run_create_directory_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_remove_directory_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_remove_directory_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_remove_directory_eden(buck: &Buck) -> anyhow::Result<()> {
    run_remove_directory_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_rename_directory_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_rename_directory_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_rename_directory_eden(buck: &Buck) -> anyhow::Result<()> {
    run_rename_directory_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_checkout_mergebase_changes_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_mergebase_changes_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_checkout_mergebase_changes_eden(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_mergebase_changes_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_checkout_with_mergebase_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_with_mergebase_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_checkout_with_mergebase_eden(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_with_mergebase_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_rebase_with_mergebase_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_rebase_with_mergebase_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_rebase_with_mergebase_eden(buck: &Buck) -> anyhow::Result<()> {
    run_rebase_with_mergebase_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_restack_with_mergebase_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_restack_with_mergebase_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_restack_with_mergebase_eden(buck: &Buck) -> anyhow::Result<()> {
    run_restack_with_mergebase_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "disable_watchman_empty_on_fresh_instance", "true")],
)]
async fn test_watchman_files_report_on_fresh_instance(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    setup_file_watcher_scm_test(buck).await?;
    buck.kill().await?;

    let required = [
        FileWatcherEvent::new(
            FileWatcherEventType::Create,
            FileWatcherKind::File,
            "files/ghi",
        ),
        FileWatcherEvent::new(
            FileWatcherEventType::Create,
            FileWatcherKind::File,
            "files/jkl",
        ),
    ];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_watchman_create_symlink_test_eden(buck: &Buck) -> anyhow::Result<()> {
    run_create_symlink_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_create_symlink_test_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_create_symlink_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman).await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_replace_file_with_symlink_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_with_symlink_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_replace_file_with_symlink_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_with_symlink_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_watchman_change_symlink_target_test_eden(buck: &Buck) -> anyhow::Result<()> {
    run_change_symlink_target_test(buck, FileSystemType::EdenFs, FileWatcherProvider::Watchman)
        .await
}

#[buck_test(setup_eden = false)]
async fn test_watchman_change_symlink_target_test_no_eden(buck: &Buck) -> anyhow::Result<()> {
    run_change_symlink_target_test(buck, FileSystemType::Native, FileWatcherProvider::Watchman)
        .await
}
