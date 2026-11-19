/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::ExitStatus;

use crate::ErrorTag;

#[cfg(unix)]
fn signal_name(signal: i32) -> Option<String> {
    nix::sys::signal::Signal::try_from(signal)
        .ok()
        .map(|s| s.as_str().to_owned())
}

impl From<ExitStatus> for crate::Error {
    #[cold]
    #[track_caller]
    fn from(status: ExitStatus) -> Self {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;

        if let Some(code) = status.code() {
            return crate::Error::from(crate::buck2_error!(
                ErrorTag::ExitStatus,
                "process exited with code {}",
                code
            ))
            .string_tag(&format!("exit_code({})", code));
        }

        #[cfg(unix)]
        if let Some(signal) = status.signal() {
            let (message, string_tag) = if let Some(signal_name) = signal_name(signal) {
                (
                    format!("process killed by {} (signal {})", signal_name, signal),
                    signal_name,
                )
            } else {
                (
                    format!("process killed by signal {}", signal),
                    format!("signal({})", signal),
                )
            };
            return crate::Error::from(crate::buck2_error!(ErrorTag::ExitStatus, "{}", message))
                .string_tag(&string_tag);
        }

        // this shouldn't happen
        crate::Error::from(crate::buck2_error!(
            ErrorTag::ExitStatusUnknown,
            "process exited with unknown status"
        ))
    }
}
