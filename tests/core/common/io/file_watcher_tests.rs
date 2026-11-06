/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Setting a file watcher test up, and checking its events.

use buck2_e2e::Buck;

use crate::cmd::output;
use crate::cmd::run;
use crate::cmd::run_checked;
use crate::file_watcher::FileWatcherEvent;
use crate::utils::get_files;

/// The file system the project is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemType {
    Native,
    EdenFs,
}

/// Commits the project to a Sapling repository with a `main` bookmark, and checks that buck sees
/// its files.
pub async fn setup_file_watcher_test(buck: &Buck) -> anyhow::Result<()> {
    // Fails on eden because the repo exists, that's ok
    run(&["sl", "init"], buck.cwd()).await?;
    run(&["sl", "commit", "--addremove", "-m", "temp"], buck.cwd()).await?;
    run_checked(&["sl", "bookmark", "main"], buck.cwd()).await?;

    let sl_status = output(&["sl", "status"], buck.cwd()).await?;
    assert_eq!(
        sl_status, "",
        "Expected clean working directory, but `sl status` returned:\n{sl_status}"
    );
    assert_eq!(get_files(buck).await?, ["files/abc", "files/d/empty"]);
    Ok(())
}

/// Panics unless every event of `required` is among `results`.
#[track_caller]
pub fn verify_results(results: &[FileWatcherEvent], required: &[FileWatcherEvent]) {
    for req in required {
        if !results.contains(req) {
            eprintln!("results={results:?}");
            eprintln!("required={required:?}");
            panic!("required not in results");
        }
    }
}

/// Moves a file away and back with `sl`, and checks that buck sees each state.
pub async fn run_aba_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    run_checked(&["sl", "mv", "files/abc", "files/d/"], buck.cwd()).await?;
    assert_eq!(get_files(buck).await?, ["files/d/abc", "files/d/empty"]);

    run_checked(&["sl", "shelve"], buck.cwd()).await?;
    assert_eq!(get_files(buck).await?, ["files/abc", "files/d/empty"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_watcher::FileWatcherEventType;
    use crate::file_watcher::FileWatcherKind;

    fn create(path: &str) -> FileWatcherEvent {
        FileWatcherEvent::new(FileWatcherEventType::Create, FileWatcherKind::File, path)
    }

    #[test]
    fn test_required_events_may_come_with_others() {
        verify_results(&[create("b"), create("a")], &[create("a")]);
        verify_results(&[create("a")], &[]);
    }

    #[test]
    #[should_panic(expected = "required not in results")]
    fn test_a_missing_required_event_panics() {
        verify_results(&[create("a")], &[create("b")]);
    }
}
