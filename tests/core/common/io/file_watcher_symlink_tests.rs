/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! File watcher tests of symlinks.

use std::fs;
use std::path::Path;

use buck2_e2e::Buck;

use crate::file_watcher::FileWatcherEvent;
use crate::file_watcher::FileWatcherEventType;
use crate::file_watcher::FileWatcherKind;
use crate::file_watcher::FileWatcherProvider;
use crate::file_watcher::get_file_watcher_events;
use crate::file_watcher_tests::FileSystemType;
use crate::file_watcher_tests::setup_file_watcher_test;
use crate::file_watcher_tests::verify_results;
use crate::utils::get_files;

fn symlink_file_type(file_watcher_provider: FileWatcherProvider) -> FileWatcherKind {
    // FIXME(JakobDegen): Bug. Notify consistently reports this incorrectly. It might be fine in
    // practice
    if file_watcher_provider == FileWatcherProvider::RustNotify {
        FileWatcherKind::File
    } else {
        FileWatcherKind::Symlink
    }
}

/// Whether the file watcher can report a path that is removed and recreated as one modification of
/// the path, of the kind it had before. EdenFS on Windows handles ProjFS notifications
/// asynchronously and looks at the path when it gets to one: when the path is back by the time it
/// gets to the deletion, it sees a modification.
fn may_coalesce_replacement(file_watcher_provider: FileWatcherProvider) -> bool {
    cfg!(windows) && file_watcher_provider == FileWatcherProvider::EdenFs
}

/// Checks that `results` report the replacement of `files/def`: as `replaced`, or where the file
/// watcher may coalesce it, as one modification of a path of kind `kind_before`.
fn verify_replacement(
    results: &[FileWatcherEvent],
    replaced: &[FileWatcherEvent],
    file_watcher_provider: FileWatcherProvider,
    kind_before: FileWatcherKind,
) {
    let coalesced = FileWatcherEvent::new(FileWatcherEventType::Modify, kind_before, "files/def");
    if may_coalesce_replacement(file_watcher_provider) && results.contains(&coalesced) {
        return;
    }
    verify_results(results, replaced);
}

/// Makes `link` a symlink to the file `target`, relative to the link's directory, as Python's
/// `Path.symlink_to`.
fn symlink_to(link: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
}

pub async fn run_create_symlink_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    fs::write(buck.cwd().join("target"), "")?;

    get_files(buck).await?;

    let path = buck.cwd().join("files").join("def");
    symlink_to(&path, &Path::new("..").join("target"))?;

    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Create,
        symlink_file_type(file_watcher_provider),
        "files/def",
    )];

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

pub async fn run_replace_file_with_symlink_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let path = buck.cwd().join("files").join("def");
    fs::write(&path, "")?;
    fs::write(buck.cwd().join("target"), "")?;
    get_files(buck).await?;

    fs::remove_file(&path)?;
    symlink_to(&path, &Path::new("..").join("target"))?;

    let required = if file_watcher_provider == FileWatcherProvider::Watchman {
        // FIXME(JakobDegen): Watchman non-deterministically produces either a create or modify
        // event
        // required = [
        //     FileWatcherEvent(
        //         FileWatcherEventType.MODIFY | FileWatcherEventType.CREATE,
        //         FileWatcherKind.SYMLINK,
        //         "files/def",
        //     ),
        // ]
        vec![]
    } else {
        vec![
            FileWatcherEvent::new(
                FileWatcherEventType::Delete,
                FileWatcherKind::File,
                "files/def",
            ),
            FileWatcherEvent::new(
                FileWatcherEventType::Create,
                symlink_file_type(file_watcher_provider),
                "files/def",
            ),
        ]
    };

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_replacement(
        &results,
        &required,
        file_watcher_provider,
        FileWatcherKind::File,
    );
    Ok(())
}

pub async fn run_change_symlink_target_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    fs::write(buck.cwd().join("target1"), "")?;
    fs::write(buck.cwd().join("target2"), "")?;
    let path = buck.cwd().join("files").join("def");
    symlink_to(&path, &Path::new("..").join("target1"))?;
    get_files(buck).await?;

    fs::remove_file(&path)?;
    symlink_to(&path, &Path::new("..").join("target2"))?;

    let required = match file_watcher_provider {
        // FIXME(JakobDegen): Bug
        FileWatcherProvider::FsHashCrawler => vec![],
        FileWatcherProvider::Watchman => vec![
            // FIXME(JakobDegen): Bug. Watchman non-deterministically flip-flops between reporting
            // create and modify events
            // FileWatcherEvent(
            //     FileWatcherEventType.CREATE, FileWatcherKind.SYMLINK, "files/def"
            // ),
        ],
        _ => vec![
            FileWatcherEvent::new(
                FileWatcherEventType::Delete,
                symlink_file_type(file_watcher_provider),
                "files/def",
            ),
            FileWatcherEvent::new(
                FileWatcherEventType::Create,
                symlink_file_type(file_watcher_provider),
                "files/def",
            ),
        ],
    };

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    assert!(!is_fresh_instance);
    verify_replacement(
        &results,
        &required,
        file_watcher_provider,
        symlink_file_type(file_watcher_provider),
    );
    Ok(())
}
