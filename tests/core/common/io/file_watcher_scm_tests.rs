/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! File watcher tests of moving between commits.

use std::fs;
use std::path::Path;

use buck2_e2e::Buck;

use crate::cmd::output;
use crate::cmd::run;
use crate::file_watcher::FileWatcherProvider;
use crate::file_watcher::get_file_watcher_events;
use crate::file_watcher_tests::FileSystemType;
use crate::file_watcher_tests::setup_file_watcher_test;

/// Creates the empty file `files/<name>` and commits it with `message`, giving the new commit as
/// `sl whereami` prints it.
async fn create_and_commit(cwd: &Path, name: &str, message: &str) -> anyhow::Result<String> {
    // Create a file
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cwd.join("files").join(name))?;

    // Commit it
    run(&["sl", "commit", "--addremove", "-m", message], cwd).await?;
    output(&["sl", "whereami"], cwd).await
}

/// The commits a, b, c and d of a simple stack, with b and c both on a, and d on c, which is
/// checked out. Run after `setup_file_watcher_test`.
pub async fn setup_file_watcher_scm_test(
    buck: &Buck,
) -> anyhow::Result<(String, String, String, String)> {
    let cwd = buck.cwd();
    let commit_a = output(&["sl", "whereami"], cwd).await?;

    let commit_b = create_and_commit(cwd, "def", "commit_b").await?;

    // Go back to commit_a
    run(&["sl", "co", &commit_a], cwd).await?;

    let commit_c = create_and_commit(cwd, "ghi", "commit_c").await?;
    let commit_d = create_and_commit(cwd, "jkl", "commit_d").await?;

    // clear log - run build twice
    buck.targets(&["root//:"]).await?;
    buck.targets(&["root//:"]).await?;

    Ok((commit_a, commit_b, commit_c, commit_d))
}

pub async fn run_checkout_mergebase_changes_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let cwd = buck.cwd();

    let commit_a = create_and_commit(cwd, "def", "next").await?;
    let commit_b = create_and_commit(cwd, "ghi", "next").await?;

    // Go back to the previous commit
    run(&["sl", "co", &commit_a], cwd).await?;

    let counts_events = [
        FileWatcherProvider::FsHashCrawler,
        FileWatcherProvider::RustNotify,
    ]
    .contains(&file_watcher_provider);

    let (is_fresh_instance, _) = get_file_watcher_events(buck).await?;
    if counts_events {
        // Stats only records the first 100 events (https://fburl.com/code/x9esqun4)
        // so we can't verify the results when making commit transitions
        assert!(!is_fresh_instance);
    } else {
        // We might have some events even for a fresh instance, so we ignore
        assert!(is_fresh_instance);
    }

    // Go back to the next commit
    run(&["sl", "co", &commit_b], cwd).await?;

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    eprintln!("{results:?}");

    if counts_events {
        assert!(!is_fresh_instance);
    } else {
        assert!(is_fresh_instance);
    }
    Ok(())
}

pub async fn run_checkout_with_mergebase_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    _file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let (_, _, commit_c, _) = setup_file_watcher_scm_test(buck).await?;

    // Go back to commit_c
    run(&["sl", "co", &commit_c], buck.cwd()).await?;

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    eprintln!("{results:?}");

    assert!(!is_fresh_instance);
    Ok(())
}

pub async fn run_rebase_with_mergebase_test(
    buck: &Buck,
    file_system_type: FileSystemType,
    file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let (_, commit_b, commit_c, _) = setup_file_watcher_scm_test(buck).await?;

    // Rebase C->D from A to B
    run(
        &["sl", "rebase", "-s", &commit_c, "-d", &commit_b],
        buck.cwd(),
    )
    .await?;

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    eprintln!("{results:?}");

    if file_system_type == FileSystemType::Native {
        // Watchman is flaky on native file systems
        if file_watcher_provider != FileWatcherProvider::Watchman {
            assert!(!is_fresh_instance);
        }
    } else {
        assert!(is_fresh_instance);
    }
    Ok(())
}

pub async fn run_restack_with_mergebase_test(
    buck: &Buck,
    _file_system_type: FileSystemType,
    _file_watcher_provider: FileWatcherProvider,
) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let (commit_a, _, _, commit_d) = setup_file_watcher_scm_test(buck).await?;

    // Rebase D from C to A
    run(
        &["sl", "rebase", "-s", &commit_d, "-d", &commit_a],
        buck.cwd(),
    )
    .await?;

    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    eprintln!("{results:?}");

    assert!(!is_fresh_instance);
    Ok(())
}
