/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Stops the child processes retrace started when retrace itself stops.
//!
//! Downloads and command-metadata fetches run in their own process groups, so
//! that cancelling one kills everything it started. The terminal's Ctrl-C goes
//! only to the foreground group, and retrace exiting does not run the worker
//! cleanup that kills them, so without this they outlive retrace.

/// A running child process group, killed if retrace is interrupted or its
/// window closes before the child finishes. Dropping the guard forgets it.
pub struct TrackedChild(#[cfg(unix)] Option<usize>);

impl TrackedChild {
    /// Tracks the process group led by `pid`. With every slot in use the group
    /// is not tracked, and only its own cleanup will stop it.
    pub fn track(pid: u32) -> Self {
        #[cfg(unix)]
        {
            unix::install();
            Self(unix::track(pid))
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            Self()
        }
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(slot) = self.0 {
            unix::forget(slot);
        }
    }
}

/// Kills every tracked child group. For an exit that does not unwind the
/// workers, such as closing the window.
pub fn stop_children() {
    #[cfg(unix)]
    unix::kill_all();
}

#[cfg(unix)]
mod unix {
    use std::sync::Once;
    use std::sync::atomic::AtomicI32;
    use std::sync::atomic::Ordering;

    /// Concurrent downloads and fetches are few; a fixed table keeps the signal
    /// handler free of locks and allocation.
    const SLOTS: usize = 32;
    static GROUPS: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];
    static INSTALL: Once = Once::new();

    pub(super) fn install() {
        INSTALL.call_once(|| {
            for signum in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: `on_signal` only reads atomics and calls the
                // async-signal-safe `kill` and `_exit`.
                let previous = unsafe {
                    libc::signal(
                        signum,
                        on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
                    )
                };
                // Whoever started retrace (`nohup`, a script's background job) asked for it to
                // ignore the signal.
                if previous == libc::SIG_IGN {
                    // SAFETY: restores the disposition retrace started with.
                    unsafe { libc::signal(signum, libc::SIG_IGN) };
                }
            }
        });
    }

    extern "C" fn on_signal(signum: libc::c_int) {
        kill_all();
        // SAFETY: async-signal-safe. 128 + signal is the status a shell
        // reports for a process the signal killed.
        unsafe { libc::_exit(128 + signum) }
    }

    pub(super) fn track(pid: u32) -> Option<usize> {
        let pid = i32::try_from(pid).ok().filter(|&p| p > 0)?;
        GROUPS.iter().position(|slot| {
            slot.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })
    }

    pub(super) fn forget(slot: usize) {
        GROUPS[slot].store(0, Ordering::Release);
    }

    pub(super) fn kill_all() {
        (0..SLOTS).for_each(kill_slot);
    }

    fn kill_slot(slot: usize) {
        let pid = GROUPS[slot].swap(0, Ordering::AcqRel);
        if pid > 0 {
            // SAFETY: a negative pid targets only the group this child leads.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }

    #[cfg(test)]
    mod tests {
        use std::os::unix::process::CommandExt as _;
        use std::os::unix::process::ExitStatusExt as _;
        use std::process::Command;
        use std::time::Duration;
        use std::time::Instant;

        use super::*;

        #[test]
        fn killing_a_slot_kills_its_group_and_frees_the_slot() {
            let mut child = Command::new("sleep")
                .arg("30")
                .process_group(0)
                .spawn()
                .unwrap();
            let pid = i32::try_from(child.id()).unwrap();
            let slot = track(child.id()).expect("a free slot");
            let started = Instant::now();
            // One slot: under cargo, other tests' children share the table, and one may take this
            // slot as soon as it is freed. Until the child is reaped, no other process has its pid.
            kill_slot(slot);
            assert_ne!(
                GROUPS[slot].load(Ordering::Acquire),
                pid,
                "stopping frees the slot"
            );
            let status = child.wait().unwrap();
            assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
            assert!(started.elapsed() < Duration::from_secs(20));
        }

        #[test]
        fn a_forgotten_group_is_left_alone() {
            // No process has this pid, so only this test can have stored it.
            let slot = track(u32::MAX >> 1).expect("a free slot");
            forget(slot);
            assert_ne!(GROUPS[slot].load(Ordering::Acquire), i32::MAX);
            assert_eq!(track(0), None, "pid 0 is not a child");
        }
    }
}
