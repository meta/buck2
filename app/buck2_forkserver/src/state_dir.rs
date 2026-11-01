/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicU8;
#[cfg(test)]
use std::sync::atomic::Ordering;

use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;

const LEASE: &str = ".lease";
const LIFECYCLE: &str = ".lifecycle";

#[cfg(test)]
pub(crate) type CleanupProbe = Arc<AtomicU8>;

#[cfg(test)]
pub(crate) const CLEANUP_PENDING: u8 = 0;
#[cfg(test)]
pub(crate) const CLEANUP_REMOVED: u8 = 1;
#[cfg(test)]
pub(crate) const CLEANUP_DEFERRED: u8 = 2;
#[cfg(test)]
pub(crate) const CLEANUP_PARENT_GONE: u8 = 3;
#[cfg(test)]
pub(crate) const CLEANUP_ERROR: u8 = 4;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CleanupOutcome {
    Removed,
    Deferred,
    ParentGoneOrUnowned,
    Error,
}

#[cfg(test)]
pub(crate) fn cleanup_outcome(probe: &CleanupProbe) -> Option<CleanupOutcome> {
    match probe.load(Ordering::SeqCst) {
        CLEANUP_REMOVED => Some(CleanupOutcome::Removed),
        CLEANUP_DEFERRED => Some(CleanupOutcome::Deferred),
        CLEANUP_PARENT_GONE => Some(CleanupOutcome::ParentGoneOrUnowned),
        CLEANUP_ERROR => Some(CleanupOutcome::Error),
        _ => None,
    }
}

#[cfg(test)]
fn record_cleanup(probe: Option<&CleanupProbe>, outcome: u8) {
    if let Some(probe) = probe {
        probe.store(outcome, Ordering::SeqCst);
    }
}

/// Owns a private forkserver directory until its process and clients finish.
pub struct ForkserverStateDir {
    path: AbsNormPathBuf,
    lease: Option<File>,
    #[cfg(test)]
    cleanup_probe: Option<CleanupProbe>,
}

impl ForkserverStateDir {
    /// Creates private state and reclaims abandoned siblings under the lifecycle lock.
    /// This performs blocking filesystem operations.
    pub fn create(path: &AbsNormPath) -> buck2_error::Result<Self> {
        if path.exists() {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Forkserver state directory already exists: {}",
                path
            ));
        }
        let parent = path
            .parent()
            .internal_error("Forkserver state directory must have a parent")?;
        fs_util::create_dir_all(parent)?;
        let lifecycle = open_lifecycle(parent)?;
        lifecycle.lock()?;
        for entry in fs_util::read_dir(parent).categorize_internal()? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                #[cfg(test)]
                let reclaimed = reclaim(&entry.path(), None);
                #[cfg(not(test))]
                let reclaimed = reclaim(&entry.path());
                if let Err(error) = reclaimed {
                    tracing::warn!(
                        "Error reclaiming forkserver state `{}`: {error:?}",
                        entry.path()
                    );
                }
            }
        }
        fs_util::create_dir(path).categorize_internal()?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path.as_path().join(LEASE))
            .and_then(|lease| {
                lease.lock_shared()?;
                Ok(lease)
            });
        match lease {
            Ok(lease) => Ok(Self::adopt(path, lease)),
            Err(error) => {
                fs_util::remove_all(path).categorize_internal()?;
                Err(error.into())
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn create_with_probe(
        path: &AbsNormPath,
        probe: CleanupProbe,
    ) -> buck2_error::Result<Self> {
        let mut owner = Self::create(path)?;
        owner.cleanup_probe = Some(probe);
        Ok(owner)
    }

    /// Adopts the lease inherited from the launching daemon.
    pub fn adopt(path: &AbsNormPath, lease: File) -> Self {
        Self {
            path: path.to_owned(),
            lease: Some(lease),
            #[cfg(test)]
            cleanup_probe: None,
        }
    }

    /// Descriptor to inherit through forkserver startup.
    pub fn lease_fd(&self) -> RawFd {
        self.lease
            .as_ref()
            .expect("Directory owner holds a lease")
            .as_raw_fd()
    }

    /// Path of the owned state directory.
    pub(crate) fn path(&self) -> &AbsNormPath {
        &self.path
    }
}

fn open_lifecycle(parent: &AbsNormPath) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.as_path().join(LIFECYCLE))
}

fn reclaim(
    path: &AbsNormPath,
    #[cfg(test)] probe: Option<&CleanupProbe>,
) -> buck2_error::Result<()> {
    let lease = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.as_path().join(LEASE))
    {
        Ok(lease) => lease,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_PARENT_GONE);
            return Ok(());
        }
        Err(error) => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_ERROR);
            return Err(error.into());
        }
    };
    match lease.try_lock() {
        Ok(()) => {
            let result = fs_util::remove_all(path).categorize_internal();
            #[cfg(test)]
            record_cleanup(
                probe,
                if result.is_ok() {
                    CLEANUP_REMOVED
                } else {
                    CLEANUP_ERROR
                },
            );
            result
        }
        Err(TryLockError::WouldBlock) => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_DEFERRED);
            Ok(())
        }
        Err(TryLockError::Error(error)) => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_ERROR);
            Err(error.into())
        }
    }
}

fn cleanup(
    path: &AbsNormPath,
    #[cfg(test)] probe: Option<&CleanupProbe>,
) -> buck2_error::Result<()> {
    let Some(parent) = path.parent() else {
        #[cfg(test)]
        record_cleanup(probe, CLEANUP_PARENT_GONE);
        return Ok(());
    };
    let lifecycle = match open_lifecycle(parent) {
        Ok(lifecycle) => lifecycle,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_PARENT_GONE);
            return Ok(());
        }
        Err(error) => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_ERROR);
            return Err(error.into());
        }
    };
    match lifecycle.try_lock() {
        Ok(()) => {
            #[cfg(test)]
            return reclaim(path, probe);
            #[cfg(not(test))]
            return reclaim(path);
        }
        Err(TryLockError::WouldBlock) => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_DEFERRED);
            Ok(())
        }
        Err(TryLockError::Error(error)) => {
            #[cfg(test)]
            record_cleanup(probe, CLEANUP_ERROR);
            Err(error.into())
        }
    }
}

impl Drop for ForkserverStateDir {
    fn drop(&mut self) {
        drop(self.lease.take());
        let path = self.path.to_owned();
        #[cfg(test)]
        let probe = self.cleanup_probe.clone();
        let deferred = move || {
            #[cfg(test)]
            let result = cleanup(&path, probe.as_ref());
            #[cfg(not(test))]
            let result = cleanup(&path);
            if let Err(error) = result {
                tracing::warn!("Error removing forkserver state `{}`: {error:?}", path);
            }
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(deferred);
        } else {
            deferred();
        }
    }
}

#[cfg(test)]
pub(crate) async fn await_cleanup_and_reclaim(
    probe: CleanupProbe,
    target: AbsNormPathBuf,
    root: AbsNormPathBuf,
) -> CleanupOutcome {
    tokio::task::spawn_blocking(move || {
        use std::time::Duration;
        use std::time::Instant;

        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            if let Some(outcome) = cleanup_outcome(&probe) {
                break outcome;
            }
            if Instant::now() >= deadline {
                panic!("production Drop never ran cleanup for `{target}`");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let exists = target.exists();
        match outcome {
            CleanupOutcome::Removed => {
                assert!(!exists, "Drop reported removal but `{target}` still exists");
            }
            CleanupOutcome::Deferred => {
                assert!(exists, "Drop deferred but `{target}` is already gone");
            }
            other => panic!("unexpected Drop outcome {other:?} for `{target}`"),
        }
        if exists {
            let mut attempt = 0u64;
            while target.exists() && Instant::now() < deadline {
                create_and_drop_dummy(&root, attempt);
                attempt += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                !target.exists(),
                "abandoned state was not reclaimed: {target}"
            );
        }
        outcome
    })
    .await
    .unwrap()
}

#[cfg(test)]
fn create_and_drop_dummy(root: &AbsNormPath, attempt: u64) {
    let next = AbsNormPathBuf::new(root.as_path().join(format!("reclaim-{attempt}"))).unwrap();
    let owner = ForkserverStateDir::create(&next).unwrap();
    drop(owner);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use buck2_util::process::background_command;

    use super::*;

    // Concurrent forks can hold an otherwise abandoned lease until exec.
    fn assert_reclaimed(root: &AbsNormPath, paths: &[&AbsNormPath], attempt: &mut u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while paths.iter().any(|path| path.exists()) && Instant::now() < deadline {
            create_and_drop_dummy(root, *attempt);
            *attempt += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
        for path in paths {
            assert!(!path.exists(), "abandoned state was not reclaimed: {path}");
        }
    }

    #[test]
    fn live_leases_survive_reclamation_and_abandoned_state_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let root = AbsNormPathBuf::new(root.path().to_owned()).unwrap();
        let unknown = root.as_path().join("unknown");
        std::fs::create_dir(&unknown).unwrap();
        std::fs::write(unknown.join("sentinel"), "unowned").unwrap();
        let first = AbsNormPathBuf::new(root.as_path().join("first")).unwrap();
        let owner = ForkserverStateDir::create(&first).unwrap();
        let child_lease = owner.lease.as_ref().unwrap().try_clone().unwrap();
        drop(owner);
        assert!(first.exists());
        let second = AbsNormPathBuf::new(root.as_path().join("second")).unwrap();
        let second_owner = ForkserverStateDir::create(&second).unwrap();
        assert!(first.exists());
        drop(child_lease);
        assert!(first.exists());
        let third = AbsNormPathBuf::new(root.as_path().join("third")).unwrap();
        let third_owner = ForkserverStateDir::create(&third).unwrap();
        let mut attempt = 0;
        assert_reclaimed(&root, &[&first], &mut attempt);
        assert_eq!(
            std::fs::read_to_string(unknown.join("sentinel")).unwrap(),
            "unowned"
        );
        assert!(second.exists());
        let lifecycle = open_lifecycle(&root).unwrap();
        lifecycle.lock().unwrap();
        drop(second_owner);
        drop(third_owner);
        assert!(second.exists());
        assert!(third.exists());
        drop(lifecycle);
        let fourth = AbsNormPathBuf::new(root.as_path().join("fourth")).unwrap();
        let fourth_owner = ForkserverStateDir::create(&fourth).unwrap();
        assert_reclaimed(&root, &[&second, &third], &mut attempt);
        assert!(fourth.exists());
        assert_eq!(
            std::fs::read_to_string(unknown.join("sentinel")).unwrap(),
            "unowned"
        );
        drop(fourth_owner);
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn live_leases_survive_deferred_reclamation() {
        tokio::task::spawn_blocking(live_leases_survive_reclamation_and_abandoned_state_is_removed)
            .await
            .unwrap();
    }

    #[test]
    fn cleanup_succeeds_after_parent_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let parent = AbsNormPathBuf::new(root.path().to_owned()).unwrap();
        let path = AbsNormPathBuf::new(parent.as_path().join("state")).unwrap();
        let owner = ForkserverStateDir::create(&path).unwrap();
        fs_util::remove_all(&parent).unwrap();
        cleanup(&path, None).unwrap();
        drop(owner);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deferred_cleanup_succeeds_after_parent_is_removed() {
        tokio::task::spawn_blocking(cleanup_succeeds_after_parent_is_removed)
            .await
            .unwrap();
    }

    struct ControlledForkHolder {
        parent: std::os::unix::net::UnixStream,
        #[expect(
            dead_code,
            reason = "held to keep fork-inherited fd open until exec; Drop closes for EOF"
        )]
        child: std::os::unix::net::UnixStream,
        thread: Option<std::thread::JoinHandle<io::Result<std::process::ExitStatus>>>,
    }

    impl ControlledForkHolder {
        fn start() -> io::Result<Self> {
            use std::os::unix::io::AsRawFd;
            use std::os::unix::process::CommandExt;

            let (parent, child) = std::os::unix::net::UnixStream::pair()?;
            let child_fd = child.as_raw_fd();
            let thread = std::thread::spawn(move || {
                let handshake = move || {
                    let byte = b'R';
                    loop {
                        // SAFETY: child_fd valid before exec; byte valid; write is async-signal-safe.
                        let written = unsafe {
                            libc::write(child_fd, &byte as *const u8 as *const libc::c_void, 1)
                        };
                        if written == 1 {
                            break;
                        }
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::EINTR) {
                            return Err(error);
                        }
                    }
                    let mut slot = [0u8; 1];
                    let mut attempts = 0;
                    loop {
                        let mut pending = libc::pollfd {
                            fd: child_fd,
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        // SAFETY: pending is a valid pollfd stack slot; poll is async-signal-safe.
                        let ready = unsafe { libc::poll(&mut pending, 1, 5000) };
                        if ready == 0 {
                            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
                        }
                        if ready < 0 {
                            let error = io::Error::last_os_error();
                            if error.raw_os_error() == Some(libc::EINTR) {
                                attempts += 1;
                                if attempts > 3 {
                                    return Err(error);
                                }
                                continue;
                            }
                            return Err(error);
                        }
                        // SAFETY: slot valid for 1 byte; child_fd valid; read is async-signal-safe.
                        let read = unsafe {
                            libc::read(child_fd, slot.as_mut_ptr() as *mut libc::c_void, 1)
                        };
                        if read == 1 || read == 0 {
                            break;
                        }
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::EINTR) {
                            attempts += 1;
                            if attempts > 3 {
                                return Err(error);
                            }
                            continue;
                        }
                        return Err(error);
                    }
                    Ok(())
                };
                let exe = std::env::current_exe()?;
                let mut command = background_command(exe);
                command
                    .arg("--list")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                // SAFETY: handshake uses only async-signal-safe syscalls and raw errno reads.
                unsafe {
                    command.pre_exec(handshake);
                }
                let mut spawned = command.spawn()?;
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    match spawned.try_wait()? {
                        Some(status) => return Ok(status),
                        None => {
                            if Instant::now() >= deadline {
                                drop(spawned.kill());
                                return spawned.wait();
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            });
            Ok(Self {
                parent,
                child,
                thread: Some(thread),
            })
        }

        fn wait_ready(&self, timeout: Duration) -> io::Result<()> {
            use std::io::Read;

            self.parent.set_read_timeout(Some(timeout))?;
            let mut slot = [0u8; 1];
            (&self.parent).read_exact(&mut slot)?;
            Ok(())
        }

        fn release(&self) -> io::Result<()> {
            use std::io::Write;

            (&self.parent).write_all(b"G")?;
            Ok(())
        }

        fn join(&mut self) -> io::Result<std::process::ExitStatus> {
            let Some(thread) = self.thread.take() else {
                return Err(io::Error::other("fork holder already joined"));
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if !thread.is_finished() {
                self.thread = Some(thread);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fork holder join timeout",
                ));
            }
            match thread.join() {
                Ok(result) => result,
                Err(_) => Err(io::Error::other("fork holder thread panicked")),
            }
        }
    }

    impl Drop for ControlledForkHolder {
        fn drop(&mut self) {
            drop(self.release());
            if let Some(thread) = self.thread.take() {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !thread.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if thread.is_finished() {
                    drop(thread.join());
                }
            }
        }
    }

    #[test]
    fn inherited_fork_lease_defers_drop_cleanup_until_release() {
        let temp = tempfile::tempdir().unwrap();
        let root = AbsNormPathBuf::new(temp.path().to_owned()).unwrap();
        let path = AbsNormPathBuf::new(root.as_path().join("guarded")).unwrap();
        let probe = Arc::new(AtomicU8::new(CLEANUP_PENDING));
        let owner = ForkserverStateDir::create_with_probe(&path, probe.clone()).unwrap();
        let mut holder = ControlledForkHolder::start().unwrap();
        holder.wait_ready(Duration::from_secs(5)).unwrap();
        drop(owner);
        assert_eq!(
            cleanup_outcome(&probe),
            Some(CleanupOutcome::Deferred),
            "one-shot Drop must defer while a fork holds the lease"
        );
        assert!(
            path.exists(),
            "deferred cleanup must skip while fork holds lease"
        );
        holder.release().unwrap();
        let status = holder.join().unwrap();
        assert!(status.success(), "fork holder child must exit successfully");
        let mut attempt = 0u64;
        assert_reclaimed(&root, &[&path], &mut attempt);
        assert_eq!(
            cleanup_outcome(&probe),
            Some(CleanupOutcome::Deferred),
            "one-shot Drop must not run again during reclamation"
        );
    }
}
