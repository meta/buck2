/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The shared daemon's last resort: ending itself and every action it started, from outside
//! itself when it can.

use std::process::Stdio;
use std::time::Duration;

use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_wrapper_common::kill;
use buck2_wrapper_common::pid::Pid;

pub fn emergency_exit() -> ! {
    #[cfg(target_os = "linux")]
    kill::kill_unidaemon_tree_within(
        Pid::from_u32(std::process::id()).expect("valid current pid"),
        0,
        Duration::from_secs(10),
    );

    let helper = (|| -> buck2_error::Result<()> {
        let mut command = buck2_util::process::background_command(std::env::current_exe()?);
        command
            .args([
                "daemon",
                "--dont-daemonize",
                "--skip-macos-qos",
                "--daemon-id",
                "00000000-0000-0000-0000-000000000000",
                "--unidaemon-reap",
            ])
            .arg(std::process::id().to_string())
            .arg(buck2_common::init::DaemonStartupConfig::testing_empty().serialize()?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if !command.spawn()?.wait()?.success() {
            return Err(buck2_error!(ErrorTag::Input, "Emergency reaper failed"));
        }
        Ok(())
    })();
    if helper.is_ok() {
        // The helper normally kills this process before its wait can return.
        // SAFETY: `_exit` does not return, run destructors or depend on the async executor.
        unsafe { libc::_exit(1) }
    }
    #[cfg(target_os = "linux")]
    kill::kill_unidaemon_tree_within(
        Pid::from_u32(std::process::id()).expect("valid current pid"),
        0,
        Duration::from_secs(10),
    );
    // SAFETY: _exit does not run destructors or depend on the async executor.
    unsafe { libc::_exit(1) }
}
