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

use nix::fcntl::FcntlArg;
use nix::fcntl::FdFlag;
use nix::fcntl::fcntl;
use nix::sys::event::EvFlags;
use nix::sys::event::EventFilter;
use nix::sys::event::FilterFlag;
use nix::sys::event::KEvent64;
use nix::sys::event::Kevent64Flags;
use nix::sys::event::Kqueue;

// Legacy detail bits from XNU's bsd/sys/event_private.h, also used by
// kern_event.c. These are not a public compatibility guarantee: classify only
// recognized memory-pressure or memory-limit reasons; other detail is not OOM evidence.
const MEMORY_DETAIL_MASK: u32 = 0xfe000000;
const MEMORY_VMPAGESHORTAGE: u32 = 0x80000000;
const MEMORY_VMTHRASHING: u32 = 0x40000000;
const MEMORY_HIWAT: u32 = 0x20000000;
// XNU maps per-process and conclave memory-limit kills to this bit. It
// identifies a memory-limit kill, but not which memory limit was exceeded.
const MEMORY_PID: u32 = 0x10000000;
const MEMORY_FCTHRASHING: u32 = 0x02000000;

/// Records kernel exit details, including for non-child daemons.
/// Must be registered while the daemon is alive; evidence survives reaping.
pub(crate) struct DaemonExitMonitor {
    queue: Kqueue,
    pid: u64,
}

impl DaemonExitMonitor {
    pub(crate) fn new(pid: i64) -> io::Result<Self> {
        if pid <= 0 || pid > i32::MAX as i64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid daemon PID",
            ));
        }
        let queue = Kqueue::new()?;
        fcntl(&queue, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        let monitor = Self {
            queue,
            pid: pid as u64,
        };
        let change = KEvent64::new(
            monitor.pid,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT | EvFlags::EV_RECEIPT,
            // NOTE_EXITSTATUS has additional permission checks that vary across
            // macOS versions. NOTE_EXIT_DETAIL does not need that status.
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
            return Err(io::Error::other("missing daemon exit registration receipt"));
        }
        if receipt.data() != 0 {
            return Err(io::Error::from_raw_os_error(receipt.data() as i32));
        }
        Ok(monitor)
    }

    /// A nonblocking read. The notification is one-shot and belongs to the
    /// registered process, not a later process that happens to reuse its PID.
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
        Ok(exit_detail_for_pid(&event, self.pid))
    }

    pub(crate) async fn oom_reason(&self) -> Option<String> {
        // Socket teardown can precede NOTE_EXIT. Bound the wait for live-daemon
        // transport errors, without blocking a runtime thread.
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match self.exit_detail() {
                    Ok(Some(detail)) => return self.reason_for_detail(detail),
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

    fn reason_for_detail(&self, detail: u32) -> Option<String> {
        let reason = memory_kill_reason(detail)?;
        Some(format!("macOS killed daemon PID {}: {reason}", self.pid))
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

fn exit_detail_for_pid(event: &KEvent64, pid: u64) -> Option<u32> {
    (event.ident() == pid
        && event.filter() == Ok(EventFilter::EVFILT_PROC)
        && !event.flags().contains(EvFlags::EV_ERROR)
        && event.fflags().bits() & (libc::NOTE_EXIT | libc::NOTE_EXIT_DETAIL)
            == (libc::NOTE_EXIT | libc::NOTE_EXIT_DETAIL))
        .then_some(event.data() as u32)
}

fn memory_kill_reason(detail: u32) -> Option<&'static str> {
    if detail & libc::NOTE_EXIT_MEMORY == 0 {
        return None;
    }
    Some(match detail & MEMORY_DETAIL_MASK {
        MEMORY_VMPAGESHORTAGE => "VM page shortage",
        MEMORY_VMTHRASHING => "VM/compressor thrashing or compressor space shortage",
        MEMORY_HIWAT => "high-water memory limit exceeded",
        MEMORY_PID => "memory limit exceeded",
        MEMORY_FCTHRASHING => "file-cache thrashing",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::process::Command;
    use std::process::Stdio;

    use super::*;

    const MEMORY_IDLE: u32 = 0x08000000;
    const MEMORY_VNODE: u32 = 0x04000000;

    #[test]
    fn memory_kills_have_specific_reasons() {
        for (flag, reason) in [
            (MEMORY_VMPAGESHORTAGE, "VM page shortage"),
            (
                MEMORY_VMTHRASHING,
                "VM/compressor thrashing or compressor space shortage",
            ),
            (MEMORY_HIWAT, "high-water memory limit exceeded"),
            (MEMORY_PID, "memory limit exceeded"),
            (MEMORY_FCTHRASHING, "file-cache thrashing"),
        ] {
            assert_eq!(
                memory_kill_reason(libc::NOTE_EXIT_MEMORY | flag),
                Some(reason)
            );
            assert_eq!(memory_kill_reason(flag), None);
        }
    }

    #[test]
    fn non_memory_exits_are_not_oom() {
        for detail in [
            0,
            libc::SIGKILL as u32,
            libc::NOTE_EXIT_CSERROR,
            libc::NOTE_EXIT_DECRYPTFAIL,
            libc::NOTE_EXIT_MEMORY | MEMORY_IDLE,
            libc::NOTE_EXIT_MEMORY | MEMORY_VNODE,
            libc::NOTE_EXIT_MEMORY,
            libc::NOTE_EXIT_MEMORY | MEMORY_HIWAT | MEMORY_IDLE,
        ] {
            assert_eq!(memory_kill_reason(detail), None);
        }
    }

    #[test]
    fn evidence_requires_matching_process_exit() {
        let detail = libc::NOTE_EXIT_MEMORY | MEMORY_VMPAGESHORTAGE;
        let exit_flags = libc::NOTE_EXIT | libc::NOTE_EXIT_DETAIL;
        let event = KEvent64::new(
            42,
            EventFilter::EVFILT_PROC,
            EvFlags::empty(),
            FilterFlag::from_bits_retain(exit_flags),
            detail as i64,
            0,
            [0; 2],
        );
        assert_eq!(exit_detail_for_pid(&event, 42), Some(detail));
        assert_eq!(exit_detail_for_pid(&event, 43), None);
        for (filter, flags, fflags) in [
            (EventFilter::EVFILT_PROC, EvFlags::EV_ERROR, exit_flags),
            (EventFilter::EVFILT_PROC, EvFlags::empty(), libc::NOTE_EXIT),
            (EventFilter::EVFILT_READ, EvFlags::empty(), exit_flags),
        ] {
            let event = KEvent64::new(
                42,
                filter,
                flags,
                FilterFlag::from_bits_retain(fflags),
                detail as i64,
                0,
                [0; 2],
            );
            assert_eq!(exit_detail_for_pid(&event, 42), None);
        }
    }

    #[test]
    fn invalid_pid_does_not_register() {
        for pid in [0, -1, i64::MAX] {
            assert!(DaemonExitMonitor::new(pid).is_err());
        }
    }

    #[test]
    fn manual_sigkill_is_not_oom() {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let monitor = DaemonExitMonitor::new(child.id() as i64).unwrap();
        assert_eq!(monitor.exit_detail().unwrap(), None);
        child.kill().unwrap();
        child.wait().unwrap();
        let detail = monitor
            .exit_detail()
            .unwrap()
            .expect("kernel exit notification");
        assert!(monitor.reason_for_detail(detail).is_none());
        assert_eq!(monitor.exit_detail().unwrap(), None);
    }

    #[test]
    fn existing_non_child_exit_is_observable() {
        // The daemon is normally reparented. Monitor a grandchild to ensure that
        // exit details do not rely on our being the process's parent.
        let mut child = Command::new("/bin/sh")
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
        let monitor = DaemonExitMonitor::new(pid.trim().parse().unwrap()).unwrap();
        assert_eq!(monitor.exit_detail().unwrap(), None);
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        let detail = monitor
            .exit_detail()
            .unwrap()
            .expect("non-child exit notification");
        assert!(monitor.reason_for_detail(detail).is_none());
    }

    #[tokio::test]
    async fn live_process_wait_is_bounded() {
        let monitor = DaemonExitMonitor::new(std::process::id() as i64).unwrap();
        assert_eq!(monitor.exit_detail().unwrap(), None);
        let reason = tokio::time::timeout(Duration::from_secs(5), monitor.oom_reason())
            .await
            .expect("exit detection must not hang on a live process");
        assert!(reason.is_none());
    }
}
