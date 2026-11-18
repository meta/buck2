/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! File watcher tests of directories.

use std::fs;

use buck2_e2e::Buck;

use crate::file_watcher::FileWatcherEvent;
use crate::file_watcher::FileWatcherEventType;
use crate::file_watcher::FileWatcherKind;
use crate::file_watcher::FileWatcherProvider;
use crate::file_watcher::get_file_watcher_events;
use crate::file_watcher_tests::FileSystemType;
use crate::file_watcher_tests::setup_file_watcher_test;
use crate::file_watcher_tests::verify_results;

pub async fn run_create_directory_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    _file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let path = buck.cwd().join("files").join("def");
    fs::create_dir(path)?;

    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Create,
        FileWatcherKind::Directory,
        "files/def",
    )];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_remove_directory_test(
    buck: &Buck,
    file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let path = buck.cwd().join("files").join("d");
    fs::remove_dir_all(path)?;

    let required = if file_watcher_provider == FileWatcherProvider::Watchman
        && file_system_type == FileSystemType::EdenFs
    {
        // Watchman on EdenFS reports a delete *file* event for removing a directory
        [FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::File,
            "files/d",
        )]
    } else {
        [FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::Directory,
            "files/d",
        )]
    };

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_rename_directory_test(
    buck: &Buck,
    file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let from_path = buck.cwd().join("files").join("d");
    let to_path = buck.cwd().join("files").join("def");
    fs::rename(from_path, to_path)?;

    let required = [
        FileWatcherEvent::new(
            FileWatcherEventType::Create,
            FileWatcherKind::Directory,
            "files/def",
        ),
        // Watchman on EdenFS reports a delete file event for the fromPath directory
        FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            if file_watcher_provider == FileWatcherProvider::Watchman
                && file_system_type == FileSystemType::EdenFs
            {
                FileWatcherKind::File
            } else {
                FileWatcherKind::Directory
            },
            "files/d",
        ),
    ];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}
