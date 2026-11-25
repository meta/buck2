/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The lock that says an event log is still being written.
//!
//! The process that writes a command's event log holds the lock for as long as it writes the
//! log. The process that uploads the log reads the log's file until the lock is free, which
//! it also is once the writer has died.

use std::fs::TryLockError;
use std::io;

use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use tokio::fs::File;

/// The directory of the lock files, in the directory of the logs they are about. A lock file
/// has the file name of its log.
const DIR: &str = "writing";

/// Held by the process that writes an event log.
pub(crate) struct WriterLock {
    log: AbsPathBuf,
    /// Holds the lock.
    file: std::fs::File,
}

impl WriterLock {
    /// Takes the lock of `log`, which no other process is writing.
    pub(crate) async fn acquire(log: AbsPathBuf) -> buck2_error::Result<Self> {
        let path = lock_file(&log)?;
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let file = File::create(&path).await?.into_std().await;
        file.try_lock()
            .map_err(io::Error::from)
            .with_buck_error_context(|| format!("Locking `{}`", path.display()))?;
        Ok(Self { log, file })
    }

    /// The log that this is the lock of.
    pub(crate) fn log(&self) -> &AbsPath {
        &self.log
    }

    /// Releases the lock, once all of the log is in its file.
    pub(crate) async fn release(self) {
        let Self { log, file } = self;
        // Closed before it is removed: on Windows a file that is removed while it is open can
        // stay, and cannot be opened, until it is closed.
        drop(file);
        remove_lock_file(&log).await;
    }
}

/// Whether no process writes `log` any more: the one that did released the lock, or died.
pub async fn is_writer_finished(log: &AbsPath) -> buck2_error::Result<bool> {
    let path = lock_file(log)?;
    let file = match File::open(&path).await {
        Ok(file) => file.into_std().await,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e.into()),
    };
    match file.try_lock() {
        Ok(()) => {
            // The writer died before it removed this. It is closed before it is removed, as
            // in `release`.
            drop(file);
            let _ignored = tokio::fs::remove_file(&path).await;
            Ok(true)
        }
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Removes the lock file of `log`: its writer is done with it, or the log is being removed
/// and its writer died.
pub(crate) async fn remove_lock_file(log: &AbsPath) {
    if let Ok(path) = lock_file(log) {
        let _ignored = tokio::fs::remove_file(path).await;
    }
}

fn lock_file(log: &AbsPath) -> buck2_error::Result<AbsPathBuf> {
    let (Some(log_dir), Some(name)) = (log.parent(), log.file_name()) else {
        return Err(internal_error!(
            "Event log path `{}` is not a file",
            log.display()
        ));
    };
    Ok(log_dir.join(DIR).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(dir: &tempfile::TempDir) -> AbsPathBuf {
        AbsPathBuf::new(dir.path().canonicalize().unwrap())
            .unwrap()
            .join("a_events.pb.zst")
    }

    #[tokio::test]
    async fn test_writer_is_finished_once_it_releases_the_lock() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let log = log(&dir);
        // A log that was never locked has no writer.
        assert!(is_writer_finished(&log).await?);

        let lock = WriterLock::acquire(log.clone()).await?;
        assert!(!is_writer_finished(&log).await?);

        lock.release().await;
        assert!(is_writer_finished(&log).await?);
        Ok(())
    }

    /// A writer that dies does not release the lock: the system does, and leaves the lock file.
    #[tokio::test]
    async fn test_writer_is_finished_once_it_is_gone() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let log = log(&dir);
        let lock = WriterLock::acquire(log.clone()).await?;
        let lock_file = lock_file(&log)?;

        drop(lock);
        assert!(lock_file.exists());
        assert!(is_writer_finished(&log).await?);
        assert!(!lock_file.exists());
        Ok(())
    }
}
