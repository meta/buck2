/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::io::Write;
use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use common_io::cmd::output;
use common_io::cmd::run;
use common_io::cmd::run_checked;
use common_io::file_watcher::FileWatcherEvent;
use common_io::file_watcher::FileWatcherEventType;
use common_io::file_watcher::FileWatcherKind;
use common_io::file_watcher::FileWatcherProvider;
use common_io::file_watcher::get_file_watcher_events;
use common_io::file_watcher::get_file_watcher_events_for;
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
use regex::Regex;

/// Opens `path` to append, creating it if needed, as Python's `open(path, "a")`.
fn open_append(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

async fn create_and_commit(cwd: &Path, path: &str) -> anyhow::Result<String> {
    let file = cwd.join(path);
    fs::create_dir_all(file.parent().expect("a file in a directory"))?;
    open_append(&file)?;

    run(&["sl", "commit", "--addremove", "-m", path], cwd).await?;
    output(&["sl", "whereami"], cwd).await
}

/// The date of the running EdenFS version, or else of the installed one, as `YYYYMMDD`.
async fn get_eden_version(buck: &Buck) -> anyhow::Result<Option<u32>> {
    let eden_out = output(&["eden", "-v"], buck.cwd()).await?;
    let version = |label: &str| -> anyhow::Result<Option<u32>> {
        let regex = Regex::new(&format!(r"{label}:\s*(\d+)"))?;
        let Some(m) = regex.captures(&eden_out) else {
            return Ok(None);
        };
        let digits = m[1].trim();
        anyhow::ensure!(
            digits.len() == 8,
            "`{digits}` is not a `%Y%m%d` date in `{eden_out}`"
        );
        let date: u32 = digits.parse()?;
        let (month, day) = (date / 100 % 100, date % 100);
        anyhow::ensure!(
            (1..=12).contains(&month) && (1..=31).contains(&day),
            "`{digits}` is not a `%Y%m%d` date in `{eden_out}`"
        );
        Ok(Some(date))
    };
    match version("Running")? {
        Some(date) => Ok(Some(date)),
        // if eden is not running, take installed version
        None => version("Installed"),
    }
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_create_file(buck: &Buck) -> anyhow::Result<()> {
    run_create_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_modify_file(buck: &Buck) -> anyhow::Result<()> {
    run_modify_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_remove_file(buck: &Buck) -> anyhow::Result<()> {
    run_remove_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_rename_file(buck: &Buck) -> anyhow::Result<()> {
    run_rename_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

// File replace is not supported on Windows
#[buck_test(setup_eden = true, skip_for_os = ["windows"])]
async fn test_edenfs_replace_file(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_create_directory(buck: &Buck) -> anyhow::Result<()> {
    run_create_directory_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_remove_directory(buck: &Buck) -> anyhow::Result<()> {
    run_remove_directory_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_rename_directory(buck: &Buck) -> anyhow::Result<()> {
    run_rename_directory_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_checkout_mergebase_changes(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_mergebase_changes_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_checkout_with_mergebase(buck: &Buck) -> anyhow::Result<()> {
    run_checkout_with_mergebase_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_rebase_with_mergebase(buck: &Buck) -> anyhow::Result<()> {
    run_rebase_with_mergebase_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_restack_with_mergebase(buck: &Buck) -> anyhow::Result<()> {
    run_restack_with_mergebase_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_create_symlink_test(buck: &Buck) -> anyhow::Result<()> {
    run_create_symlink_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_replace_file_with_symlink_test(buck: &Buck) -> anyhow::Result<()> {
    run_replace_file_with_symlink_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs)
        .await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_change_symlink_target_test(buck: &Buck) -> anyhow::Result<()> {
    run_change_symlink_target_test(buck, FileSystemType::EdenFs, FileWatcherProvider::EdenFs).await
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_truncate_journal(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    run(&["edenfsctl", "debug", "flush_journal"], buck.cwd()).await?;

    let (is_fresh_instance, _) = get_file_watcher_events(buck).await?;
    assert!(is_fresh_instance);
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_file_watcher_stats(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    let file_stats = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "FileWatcher", "stats"],
    )
    .await?;

    let file_stats = &file_stats[0];
    assert_eq!(file_stats["fresh_instance"], true);
    assert!(!file_stats["branched_from_revision"].is_null());
    assert!(!file_stats["branched_from_revision_timestamp"].is_null());
    // we don't have global revision for test repo
    assert_eq!(
        file_stats.get("branched_from_global_rev"),
        Some(&serde_json::Value::Null)
    );
    assert!(!file_stats["eden_version"].is_null());
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_files_report_on_fresh_instance(buck: &Buck) -> anyhow::Result<()> {
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

#[buck_test(
    setup_eden = true,
    // the test has subproject and creates buck-out in subproject,
    // when we setup eden we assume that buck-out is in project root dir
    // and redirect only that buck-out and not buck-out in subproject.
    // So, ignore soft errors that buck-out isn't redirected
    allow_soft_errors = true,
    // test is flaky on windows
    skip_for_os = ["windows"],
)]
async fn test_edenfs_changes_in_subproject(buck: &Buck) -> anyhow::Result<()> {
    let cwd = Path::new("subproject");
    buck.targets(&["cell//:"]).rel_cwd(cwd).await?;

    open_append(&buck.cwd().join("subproject").join("abc"))?;

    let (_, results) = get_file_watcher_events_for(buck, "cell//:", Some(cwd)).await?;
    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Create,
        FileWatcherKind::File,
        "abc",
    )];
    verify_results(&results, &required);
    Ok(())
}

#[buck_test(
    setup_eden = true,
    // the test has subproject and creates buck-out in subproject,
    // when we setup eden we assume that buck-out is in project root dir
    // and redirect only that buck-out and not buck-out in subproject.
    // So, ignore soft errors that buck-out isn't redirected
    allow_soft_errors = true,
    // test is flaky on windows
    skip_for_os = ["windows"],
)]
async fn test_edenfs_changes_outside_subproject(buck: &Buck) -> anyhow::Result<()> {
    let cwd = Path::new("subproject");
    buck.targets(&["cell//:"]).rel_cwd(cwd).await?;

    open_append(&buck.cwd().join("subproject").join("abc"))?;

    open_append(&buck.cwd().join("cde"))?;

    let (_, results) = get_file_watcher_events_for(buck, "cell//:", Some(cwd)).await?;
    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Create,
        FileWatcherKind::File,
        "abc",
    )];
    verify_results(&results, &required);
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_checkout_dir_changes(buck: &Buck) -> anyhow::Result<()> {
    // We want to create the following commit tree,
    // so when we move between f2 and f1 the mergbase doesn't change:
    //
    // o  xxxxxxxxf3 main
    // │
    // │ o  xxxxxxxxf2
    // ├─╯  files/d2/f2
    // │
    // o  xxxxxxxxf1

    let f1 = create_and_commit(buck.cwd(), "files/d1/f1").await?;
    let f2 = create_and_commit(buck.cwd(), "files/d2/f2").await?;
    run(&["sl", "co", &f1], buck.cwd()).await?;
    run_checked(&["sl", "bookmark", "main"], buck.cwd()).await?;
    create_and_commit(buck.cwd(), "files/d3/f3").await?;
    run(&["sl", "co", &f2], buck.cwd()).await?;

    buck.targets(&["root//:"]).await?;
    run(&["sl", "co", &f1], buck.cwd()).await?;
    let (is_fresh_instance, results) = get_file_watcher_events(buck).await?;
    let required = [
        FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::File,
            "files/d2/f2",
        ),
        FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::Directory,
            "files/d2",
        ),
        FileWatcherEvent::new(
            FileWatcherEventType::Modify,
            FileWatcherKind::Directory,
            "files",
        ),
    ];
    assert!(!is_fresh_instance);
    verify_results(&results, &required);
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_directory_rename(buck: &Buck) -> anyhow::Result<()> {
    fs::create_dir(buck.cwd().join("d1"))?;
    fs::File::create(buck.cwd().join("d1").join("TARGETS.fixture"))?;
    buck.targets(&["d1:"]).await?;

    fs::rename(buck.cwd().join("d1"), buck.cwd().join("d2"))?;
    buck.targets(&["d1:"])
        .await
        .expect_failure()
        .assert_stderr_matches("dir `root//d1` does not exist. Did you mean `root//d2`");
    buck.targets(&["d2:"]).await?;
    Ok(())
}

// Dir replace is not supported on Windows
#[buck_test(setup_eden = true, skip_for_os = ["windows"])]
async fn test_edenfs_directory_replace(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    fs::create_dir(buck.cwd().join("d1"))?;
    fs::create_dir(buck.cwd().join("d2"))?;
    // it's only possible to replace a dir
    // if newname exists and is an empty directory
    fs::rename(buck.cwd().join("d1"), buck.cwd().join("d2"))?;

    // we should get `create` for the newname
    // and `delete` for the oldname
    let required = [
        FileWatcherEvent::new(
            FileWatcherEventType::Create,
            FileWatcherKind::Directory,
            "d2",
        ),
        FileWatcherEvent::new(
            FileWatcherEventType::Delete,
            FileWatcherKind::Directory,
            "d1",
        ),
    ];

    let (_, results) = get_file_watcher_events(buck).await?;
    verify_results(&results, &required);
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_duplicated_notifications(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    open_append(&buck.cwd().join("files").join("abc"))?.write_all(b"test")?;

    open_append(&buck.cwd().join("files").join("bcd"))?;

    open_append(&buck.cwd().join("files").join("abc"))?.write_all(b"test1")?;

    let (_, results) = get_file_watcher_events(buck).await?;
    // eden watcher doesn't report duplicates
    assert_eq!(
        results,
        [
            FileWatcherEvent::new(
                FileWatcherEventType::Modify,
                FileWatcherKind::File,
                "files/abc",
            ),
            FileWatcherEvent::new(
                FileWatcherEventType::Create,
                FileWatcherKind::File,
                "files/bcd",
            ),
        ]
    );
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_edenfs_hg_clean_update(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    open_append(&buck.cwd().join("files").join("abc"))?.write_all(b"test")?;

    let (_, results) = get_file_watcher_events(buck).await?;
    let required = [FileWatcherEvent::new(
        FileWatcherEventType::Modify,
        FileWatcherKind::File,
        "files/abc",
    )];
    verify_results(&results, &required);

    run(&["hg", "up", "-C", "."], buck.cwd()).await?;

    let eden_version = get_eden_version(buck).await?;
    let eden_version = eden_version.expect("Failed to get eden version");

    let mut expected_result = Vec::new();
    if eden_version >= 20250612 {
        // hg up -C . deletes all the changes and eden should report modification in `root//files/abc`
        expected_result.push(FileWatcherEvent::new(
            FileWatcherEventType::Modify,
            FileWatcherKind::File,
            "files/abc",
        ));
    }

    let (_, results) = get_file_watcher_events(buck).await?;
    assert_eq!(results, expected_result);
    Ok(())
}
