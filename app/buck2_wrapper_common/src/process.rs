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
