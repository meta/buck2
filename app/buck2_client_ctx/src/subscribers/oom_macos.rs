/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::time::Duration;

use nix::sys::event::EvFlags;
use nix::sys::event::EventFilter;
use nix::sys::event::FilterFlag;
use nix::sys::event::KEvent64;
use nix::sys::event::Kevent64Flags;
use nix::sys::event::Kqueue;

// What caused a memory kill, from XNU's bsd/sys/event_private.h. Unlike `NOTE_EXIT_MEMORY`
// these bits are not a public interface.
const MEMORY_DETAIL_MASK: u32 = 0xfe000000;
// A kill for page shortage, and for every cause the kernel has no bit for: this one is its
// default.
const MEMORY_VMPAGESHORTAGE: u32 = 0x80000000;
const MEMORY_VMTHRASHING: u32 = 0x40000000;
const MEMORY_HIWAT: u32 = 0x20000000;
// A kill over a memory limit, without saying which limit.
const MEMORY_PID: u32 = 0x10000000;
const MEMORY_IDLE: u32 = 0x08000000;
const MEMORY_VNODE: u32 = 0x04000000;
const MEMORY_FCTHRASHING: u32 = 0x02000000;

/// Receives the kernel's account of why a process exits.
///
/// The process must be alive when this is created. It does not have to be a child of this
/// process, and the account stays readable after the process is reaped.
pub(crate) struct DaemonExitMonitor {
    queue: Kqueue,
    pid: u64,
}

impl DaemonExitMonitor {
    pub(crate) fn try_new(pid: i64) -> io::Result<Self> {
        if pid <= 0 || pid > i32::MAX as i64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid daemon PID",
            ));
        }
        // The kernel creates every kqueue descriptor close-on-exec and close-on-fork.
        let monitor = Self {
            queue: Kqueue::new()?,
            pid: pid as u64,
        };
        let change = KEvent64::new(
            monitor.pid,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT | EvFlags::EV_RECEIPT,
            // `NOTE_EXITSTATUS` is refused for some processes, `NOTE_EXIT_DETAIL` for none.
            FilterFlag::from_bits_retain(libc::NOTE_EXIT | libc::NOTE_EXIT_DETAIL),
            0,
            0,
            [0; 2],
        );
        let mut receipts = [empty_event()];
        let count =
            monitor
                .queue
                .kevent64(&[change], &mut receipts, Kevent64Flags::empty(), None)?;
        let receipt = receipts[0];
        if count != 1 || !receipt.flags().contains(EvFlags::EV_ERROR) {
            return Err(io::Error::other("Missing daemon exit registration receipt"));
        }
        if receipt.data() != 0 {
            return Err(io::Error::from_raw_os_error(receipt.data() as i32));
        }
        Ok(monitor)
    }

    /// The exit detail if the process has exited, without blocking.
    ///
    /// The registration is for the process that had the PID when it was made, not for one that
    /// reuses the PID later, and it reports once.
    fn exit_detail(&self) -> io::Result<Option<u32>> {
        let mut events = [empty_event()];
        let count =
            self.queue
                .kevent64(&[], &mut events, Kevent64Flags::KEVENT_FLAG_IMMEDIATE, None)?;
        if count == 0 {
            return Ok(None);
        }
        let event = events[0];
        if event.flags().contains(EvFlags::EV_ERROR) {
            return Err(io::Error::from_raw_os_error(event.data() as i32));
        }
        Ok(Some(event.data() as u32))
    }

    /// Why macOS memory management killed the process, if it did.
    ///
    /// The kill macOS makes when swap runs out is not one of these: the kernel sends the
    /// process holding the most compressed memory an ordinary `SIGKILL`, with no exit detail.
    pub(crate) async fn oom_reason(&self) -> Option<String> {
        // The daemon's sockets close before the kernel posts its exit, and a connection can fail
        // while the daemon lives on, so wait for the exit, but not for long.
        let detail = self.wait_for_exit(Duration::from_secs(1)).await?;
        let reason = memory_kill_reason(detail)?;
        Some(format!(
            "macOS memory management killed daemon PID {}: {reason}",
            self.pid
        ))
    }

    /// The exit detail, once the process exits within `timeout`.
    async fn wait_for_exit(&self, timeout: Duration) -> Option<u32> {
        tokio::time::timeout(timeout, async {
            loop {
                match self.exit_detail() {
                    Ok(Some(detail)) => return Some(detail),
                    Ok(None) => tokio::time::sleep(Duration::from_millis(10)).await,
                    Err(error) => {
                        tracing::debug!("Failed to read daemon exit notification: {error}");
                        return None;
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
    }
}

fn empty_event() -> KEvent64 {
    KEvent64::new(
        0,
        EventFilter::EVFILT_PROC,
        EvFlags::empty(),
        FilterFlag::empty(),
        0,
        0,
        [0; 2],
    )
}

/// Why the kernel killed the process to reclaim memory, if that is how it exited.
fn memory_kill_reason(detail: u32) -> Option<&'static str> {
    if detail & libc::NOTE_EXIT_MEMORY == 0 {
        return None;
    }
    Some(match detail & MEMORY_DETAIL_MASK {
        MEMORY_VMPAGESHORTAGE => "the system was low on memory",
        MEMORY_VMTHRASHING => "VM/compressor thrashing or compressor space shortage",
        MEMORY_HIWAT => "high-water memory limit exceeded",
        MEMORY_PID => "memory limit exceeded",
        MEMORY_FCTHRASHING => "file-cache thrashing",
        // The kernel retiring an idle process or freeing vnodes is not the daemon running out
        // of memory.
        MEMORY_IDLE | MEMORY_VNODE => return None,
        _ => "unrecognized cause",
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;
    use std::io::BufRead;
    use std::mem;
    use std::mem::MaybeUninit;
    use std::process::Stdio;
    use std::ptr;

    use buck2_util::process::background_command;

    use super::*;

    const POSIX_SPAWN_JETSAM_MEMLIMIT_ACTIVE_FATAL: libc::c_short = 0x04;
    const POSIX_SPAWN_JETSAM_MEMLIMIT_INACTIVE_FATAL: libc::c_short = 0x08;

    const MEMORY_LIMIT_MB: libc::c_int = 50;

    type SetJetsam = unsafe extern "C" fn(
        attr: *mut libc::posix_spawnattr_t,
        flags: libc::c_short,
        priority: libc::c_int,
        memlimit_active: libc::c_int,
        memlimit_inactive: libc::c_int,
    ) -> libc::c_int;

    /// What launchd gives a job its memory limit with. libSystem exports it, but the SDK has no
    /// header for it; XNU declares it in libsyscall's spawn_private.h. It is looked up by name
    /// so that a macOS without it fails the tests that need it, not every test in this binary.
    fn posix_spawnattr_setjetsam_ext() -> SetJetsam {
        // SAFETY: The name is a NUL-terminated string.
        let symbol = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"posix_spawnattr_setjetsam_ext".as_ptr(),
            )
        };
        assert!(
            !symbol.is_null(),
            "this macOS has no `posix_spawnattr_setjetsam_ext` to limit a child's memory with"
        );
        // SAFETY: The symbol is the function that XNU declares with this signature.
        unsafe { mem::transmute::<*mut libc::c_void, SetJetsam>(symbol) }
    }

    /// A child process that is killed and reaped if the test does not get to wait for it.
    struct MemoryLimitedChild(libc::pid_t);

    impl MemoryLimitedChild {
        /// Starts a stopped `dd` that will read one block of `block_size` into memory, under a
        /// memory limit the kernel enforces by killing it.
        fn spawn_stopped(block_size: &CStr) -> Self {
            let args = [
                c"/bin/dd",
                c"if=/dev/zero",
                c"of=/dev/null",
                block_size,
                c"count=1",
            ];
            let argv: Vec<*mut libc::c_char> = args
                .iter()
                .map(|arg| arg.as_ptr().cast_mut())
                .chain([ptr::null_mut()])
                .collect();
            let envp: [*mut libc::c_char; 1] = [ptr::null_mut()];
            let set_jetsam = posix_spawnattr_setjetsam_ext();
            let mut pid = 0;
            // SAFETY: The attributes and file actions are initialized before use and destroyed
            // after, and every pointer is to a NUL-terminated string or a null-terminated array
            // that outlives the calls.
            unsafe {
                let mut attr = MaybeUninit::uninit();
                assert_eq!(libc::posix_spawnattr_init(attr.as_mut_ptr()), 0);
                let mut attr = attr.assume_init();
                assert_eq!(
                    libc::posix_spawnattr_setflags(
                        &mut attr,
                        libc::POSIX_SPAWN_START_SUSPENDED as libc::c_short,
                    ),
                    0
                );
                assert_eq!(
                    set_jetsam(
                        &mut attr,
                        POSIX_SPAWN_JETSAM_MEMLIMIT_ACTIVE_FATAL
                            | POSIX_SPAWN_JETSAM_MEMLIMIT_INACTIVE_FATAL,
                        // The default priority band.
                        -1,
                        MEMORY_LIMIT_MB,
                        MEMORY_LIMIT_MB,
                    ),
                    0
                );
                let mut file_actions = MaybeUninit::uninit();
                assert_eq!(
                    libc::posix_spawn_file_actions_init(file_actions.as_mut_ptr()),
                    0
                );
                let mut file_actions = file_actions.assume_init();
                // `dd` reports what it copied on stderr.
                assert_eq!(
                    libc::posix_spawn_file_actions_addopen(
                        &mut file_actions,
                        libc::STDERR_FILENO,
                        c"/dev/null".as_ptr(),
                        libc::O_WRONLY,
                        0,
                    ),
                    0
                );
                let spawned = libc::posix_spawn(
                    &mut pid,
                    args[0].as_ptr(),
                    &file_actions,
                    &attr,
                    argv.as_ptr(),
                    envp.as_ptr(),
                );
                libc::posix_spawn_file_actions_destroy(&mut file_actions);
                libc::posix_spawnattr_destroy(&mut attr);
                assert_eq!(spawned, 0);
            }
            Self(pid)
        }

        fn pid(&self) -> i64 {
            self.0 as i64
        }

        fn resume(&self) {
            // SAFETY: Signals a child of this process that has not been waited for.
            assert_eq!(unsafe { libc::kill(self.0, libc::SIGCONT) }, 0);
        }

        /// The status the child exits with.
        fn wait(self) -> libc::c_int {
            let pid = self.0;
            mem::forget(self);
            let mut status = 0;
            // SAFETY: `pid` is a child of this process that has not been waited for, and
            // `status` is valid for the write.
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            status
        }
    }

    impl Drop for MemoryLimitedChild {
        fn drop(&mut self) {
            // SAFETY: Signals and reaps a child of this process that has not been waited for.
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
                libc::waitpid(self.0, ptr::null_mut(), 0);
            }
        }
    }

    #[test]
    fn test_process_that_cannot_be_monitored() {
        for pid in [0, -1, i64::MAX] {
            assert_eq!(
                DaemonExitMonitor::try_new(pid).err().map(|e| e.kind()),
                Some(io::ErrorKind::InvalidInput)
            );
        }
        let mut exited = background_command("/usr/bin/true").spawn().unwrap();
        exited.wait().unwrap();
        assert_eq!(
            DaemonExitMonitor::try_new(exited.id() as i64)
                .err()
                .and_then(|e| e.raw_os_error()),
            Some(libc::ESRCH)
        );
    }

    /// One kill by the kernel covers the wait and the classification: every such kill has the OS
    /// write a diagnostic report.
    #[tokio::test]
    async fn test_kernel_memory_limit_kill_is_reported() {
        let child = MemoryLimitedChild::spawn_stopped(c"bs=100m");
        let pid = child.pid();
        let monitor = DaemonExitMonitor::try_new(pid).unwrap();
        // The wait has seen the process alive by the time the process is let run.
        let (detail, ()) = tokio::join!(monitor.wait_for_exit(Duration::from_secs(60)), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            child.resume();
        });
        assert_eq!(
            detail.and_then(memory_kill_reason),
            Some("memory limit exceeded")
        );
        let status = child.wait();
        assert!(
            libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGKILL,
            "The kernel should kill a process over its memory limit, wait status {status:#x}"
        );
    }

    #[tokio::test]
    async fn test_exit_within_memory_limit_is_not_oom() {
        let child = MemoryLimitedChild::spawn_stopped(c"bs=1m");
        let monitor = DaemonExitMonitor::try_new(child.pid()).unwrap();
        child.resume();
        let status = child.wait();
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "`dd` should succeed within its memory limit, wait status {status:#x}"
        );
        assert_eq!(monitor.oom_reason().await, None);
    }

    #[test]
    fn test_manual_sigkill_is_not_oom() {
        let mut child = background_command("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let monitor = DaemonExitMonitor::try_new(child.id() as i64).unwrap();
        assert_eq!(monitor.exit_detail().unwrap(), None);
        child.kill().unwrap();
        child.wait().unwrap();
        let detail = monitor
            .exit_detail()
            .unwrap()
            .expect("kernel exit notification");
        assert_eq!(memory_kill_reason(detail), None);
        assert_eq!(monitor.exit_detail().unwrap(), None);
    }

    #[test]
    fn test_exit_of_a_process_that_is_not_a_child() {
        // The daemon is normally reparented. Monitor a grandchild to ensure that
        // exit details do not rely on our being the process's parent.
        let mut child = background_command("/bin/sh")
            // Preserve the pipe on FD 3 so the background process waits for EOF
            // until registration is complete.
            .args(["-c", "exec 3<&0; cat <&3 & echo $!; wait"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pid = String::new();
        io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut pid)
            .unwrap();
        let monitor = DaemonExitMonitor::try_new(pid.trim().parse().unwrap()).unwrap();
        assert_eq!(monitor.exit_detail().unwrap(), None);
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        let detail = monitor
            .exit_detail()
            .unwrap()
            .expect("non-child exit notification");
        assert_eq!(memory_kill_reason(detail), None);
    }

    #[tokio::test]
    async fn test_wait_for_a_live_process_is_bounded() {
        let monitor = DaemonExitMonitor::try_new(std::process::id() as i64).unwrap();
        assert_eq!(monitor.exit_detail().unwrap(), None);
        let reason = tokio::time::timeout(Duration::from_secs(5), monitor.oom_reason())
            .await
            .expect("exit detection must not hang on a live process");
        assert!(reason.is_none());
    }
}
