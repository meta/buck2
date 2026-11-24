/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// Creates a command that does not show a window on Windows.
pub fn background_command<S: AsRef<OsStr>>(program: S) -> Command {
    // ast-grep-ignore: rust/buck2-no-command-new
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command.env_remove(crate::BUCK_WRAPPER_UUID_ENV_VAR);
    command.env_remove(crate::BUCK_WRAPPER_PID_ENV_VAR);
    command
}

/// Creates a `tokio::process::Command` that does not show a window on Windows.
pub fn async_background_command<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    background_command(program).into()
}

/// The program to exec to run this binary again.
///
/// On Linux this is `/proc/self/exe` itself, which execs the running binary's inode even after
/// the file has been unlinked. `std::env::current_exe()` is only the text of that link, and
/// names a path that no longer exists (` (deleted)` appended) once a cache cleaner or an upgrade
/// has removed a long-running daemon's binary.
pub fn current_exe_for_exec() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from("/proc/self/exe"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe()
    }
}

/// Gives this process back the kernel name (`comm`) it had before a launch through
/// [`current_exe_for_exec`]. The kernel names a process after the basename of the path it was
/// exec'd by, so one started through `/proc/self/exe` is called `exe`; this renames it after its
/// binary's file name, minus the ` (deleted)` suffix of an unlinked binary. A process with any
/// other name is left alone.
pub fn restore_process_name_after_exec() {
    #[cfg(target_os = "linux")]
    {
        if process_name().as_deref() != Some("exe") {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let Some(name) = exe.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        set_process_name(name.strip_suffix(" (deleted)").unwrap_or(name));
    }
}

/// The calling thread's `comm`.
#[cfg(target_os = "linux")]
fn process_name() -> Option<String> {
    let mut name = [0u8; 16];
    // SAFETY: `PR_GET_NAME` writes at most 16 bytes, NUL-terminated, into the buffer.
    if unsafe { nix::libc::prctl(nix::libc::PR_GET_NAME, name.as_mut_ptr()) } != 0 {
        return None;
    }
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    Some(String::from_utf8_lossy(&name[..end]).into_owned())
}

/// Sets the calling thread's `comm`; the kernel keeps the first 15 bytes.
#[cfg(target_os = "linux")]
fn set_process_name(name: &str) {
    let Ok(name) = std::ffi::CString::new(name) else {
        return;
    };
    // SAFETY: the pointer is to a NUL-terminated string that outlives the call.
    unsafe { nix::libc::prctl(nix::libc::PR_SET_NAME, name.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_exe_for_exec_runs_this_binary() {
        let output = background_command(current_exe_for_exec().unwrap())
            .arg("--list")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("current_exe_for_exec_runs_this_binary")
        );
    }

    /// `comm` is per thread, and each test runs on a thread of its own.
    #[cfg(target_os = "linux")]
    #[test]
    fn restore_process_name_after_exec_renames_only_exe() {
        set_process_name("other");
        restore_process_name_after_exec();
        assert_eq!(process_name().as_deref(), Some("other"));

        set_process_name("exe");
        restore_process_name_after_exec();
        let exe = std::env::current_exe().unwrap();
        let file_name = exe.file_name().unwrap().to_str().unwrap();
        let expected = &file_name[..file_name.len().min(15)];
        assert_eq!(process_name().as_deref(), Some(expected));
    }
}
