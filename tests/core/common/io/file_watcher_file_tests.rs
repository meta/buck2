/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! File watcher tests of files.

use std::fs;
use std::io::Write;

use buck2_e2e::Buck;

use crate::file_watcher::FileWatcherEvent;
use crate::file_watcher::FileWatcherEventType;
use crate::file_watcher::FileWatcherKind;
use crate::file_watcher::FileWatcherProvider;
use crate::file_watcher::get_file_watcher_events;
use crate::file_watcher_tests::FileSystemType;
use crate::file_watcher_tests::setup_file_watcher_test;
use crate::file_watcher_tests::verify_results;

/// Opens `path` to append, creating it if needed, as Python's `open(path, "a")`.
fn open_append(path: &std::path::Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

pub async fn run_create_file_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    _file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let path = buck.cwd().join("files").join("def");
    open_append(&path)?;

    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Create,
        FileWatcherKind::File,
        "files/def",
    )];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_modify_file_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let path = buck.cwd().join("files").join("abc");
    open_append(&path)?.write_all(b"modify")?;

    let required: Vec<FileWatcherEvent> =
        if file_watcher_provider == FileWatcherProvider::RustNotify && cfg!(windows) {
            // The Windows support of notify does not "uniquely" support file modification events.
            // Rather, it generates create/delete events for a file and directory of the same name.
            vec![
                FileWatcherEvent::new(
                    FileWatcherEventType::Create,
                    FileWatcherKind::File,
                    "files/abc",
                ),
                FileWatcherEvent::new(
                    FileWatcherEventType::Delete,
                    FileWatcherKind::File,
                    "files/abc",
                ),
            ]
        } else {
            vec![FileWatcherEvent::new(
                FileWatcherEventType::Modify,
                FileWatcherKind::File,
                "files/abc",
            )]
        };

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_remove_file_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    _file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let path = buck.cwd().join("files").join("abc");
    fs::remove_file(path)?;

    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Delete,
        FileWatcherKind::File,
        "files/abc",
    )];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_rename_file_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    _file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    let from_path = buck.cwd().join("files").join("abc");
    let to_path = buck.cwd().join("files").join("def");
    fs::rename(from_path, to_path)?;

    let required = [
        FileWatcherEvent::new(
            FileWatcherEventType::Create,
            FileWatcherKind::File,
            "files/def",
        ),
        FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::File,
            "files/abc",
        ),
    ];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_replace_file_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    let path = buck.cwd().join("files").join("def");
    open_append(&path)?;

    // clear log - run build twice
    buck.targets(&[":"]).await?;
    buck.targets(&[":"]).await?;

    let from_path = buck.cwd().join("files").join("abc");
    let to_path = buck.cwd().join("files").join("def");
    fs::rename(from_path, to_path)?;

    let required: Vec<FileWatcherEvent> = if [
        FileWatcherProvider::EdenFs,
        FileWatcherProvider::Watchman,
        FileWatcherProvider::RustNotify,
    ]
    .contains(&file_watcher_provider)
    {
        vec![
            // Watchman reports a modify event for replacing a file if we have already cleared the
            // previous file watcher events from the log - this is the case when we get files as above.
            FileWatcherEvent::new(
                if file_watcher_provider == FileWatcherProvider::Watchman {
                    FileWatcherEventType::Modify
                } else {
                    FileWatcherEventType::Create
                },
                FileWatcherKind::File,
                "files/def",
            ),
            FileWatcherEvent::new(
                FileWatcherEventType::Delete,
                FileWatcherKind::File,
                "files/abc",
            ),
        ]
    } else {
        // fs hash crawler returns a delete for replacing a file
        vec![FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::File,
            "files/abc",
        )]
    };

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}
