/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(windows)]

use std::io;
use std::time::Duration;

use buck2_error::BuckErrorContext;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Foundation::STILL_ACTIVE;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::GetProcessTimes;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_INFORMATION;
use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;
use windows_sys::Win32::System::Threading::TerminateProcess;

use crate::pid::Pid;
use crate::win::winapi_handle::WinapiHandle;

/// `HANDLE` which points to a process.
pub(crate) struct WinapiProcessHandle {
    handle: WinapiHandle,
    pid: Pid,
}

impl WinapiProcessHandle {
    /// Open a process handle to query. `None` if process doesn't exist.
    pub(crate) fn open_for_info(pid: Pid) -> Option<WinapiProcessHandle> {
        WinapiProcessHandle::open_impl(pid, PROCESS_QUERY_INFORMATION)
    }

    /// Open a process handle to terminate. `None` if process doesn't exist.
    pub(crate) fn open_for_terminate(pid: Pid) -> Option<WinapiProcessHandle> {
        WinapiProcessHandle::open_impl(pid, PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION)
    }

    fn open_impl(pid: Pid, desired_access: u32) -> Option<WinapiProcessHandle> {
        let proc_handle = unsafe { OpenProcess(desired_access, 0, pid.to_u32()) };
        let Some(handle) = (unsafe { WinapiHandle::new(proc_handle) }) else {
            // If proc_handle is null, process died already, or other error like access denied.
            // TODO(nga): handle error properly.
            return None;
        };
        Some(WinapiProcessHandle { handle, pid })
    }

    /// Terminate the process. Do not fail if process is already dead.
    pub(crate) fn terminate(&self) -> buck2_error::Result<()> {
        unsafe {
            if TerminateProcess(self.handle.handle(), 1) == 0 {
                // Stash the error before calling `exit_code` to avoid overwriting it.
                let os_error = io::Error::last_os_error();

                // From WinAPI doc:
                // After a process has terminated, call to `TerminateProcess` with open handles
                // to the process fails with `ERROR_ACCESS_DENIED` (5) error code.
                if let Ok(true) = self.has_exited() {
                    return Ok(());
                }

                Err(os_error).with_buck_error_context(|| format!("Failed to kill pid {}", self.pid))
            } else {
                Ok(())
            }
        }
    }

    pub(crate) fn process_creation_time(&self) -> buck2_error::Result<Duration> {
        let mut creation_time: FILETIME = unsafe { std::mem::zeroed() };
        let mut exit_time: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel_time: FILETIME = unsafe { std::mem::zeroed() };
        let mut user_time: FILETIME = unsafe { std::mem::zeroed() };

        let result = unsafe {
            GetProcessTimes(
                self.handle.handle(),
                &mut creation_time,
                &mut exit_time,
                &mut kernel_time,
                &mut user_time,
            )
        };

        if result == 0 {
            return Err(io::Error::last_os_error()).with_buck_error_context(|| {
                format!("Failed to call GetProcessTimes for pid {}", self.pid)
            });
        }

        // `creation_time` stores intervals of 100 ns, so multiply by 100 to obtain
        // proper nanoseconds. The u64 type will overflow around the year 2185.
        let intervals =
            ((creation_time.dwHighDateTime as u64) << 32) | (creation_time.dwLowDateTime as u64);
        Ok(Duration::from_nanos(intervals * 100))
    }

    /// Exit code, or `None` if process is still running.
    fn exit_code(&self) -> buck2_error::Result<Option<u32>> {
        let mut exit_code = 0;

        if unsafe { GetExitCodeProcess(self.handle.handle(), &mut exit_code) } != 0 {
            if exit_code == STILL_ACTIVE as u32 {
                return Ok(None);
            } else {
                return Ok(Some(exit_code));
            }
        }

        Err(io::Error::last_os_error()).with_buck_error_context(|| {
            format!("Failed to call GetExitCodeProcess for pid {}", self.pid)
        })
    }

    pub(crate) fn has_exited(&self) -> buck2_error::Result<bool> {
        Ok(self.exit_code()?.is_some())
    }
}
