/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// Common exit codes for buck with stronger semantic meanings
///
/// The exit code is u8 integer and has the following meanings for common exit codes.
/// - Success             : 0
/// - Uncategorized Error : 1
/// - Infra Error         : 2
/// - User Error          : 3
/// - Signal Interruption : 129-192 (128 + signal number)
#[derive(Clone, Copy, Debug)]
pub enum ExitCode {
    Success,
    UnknownFailure,
    InfraError,
    UserError,
    DaemonIsBusy,
    DaemonPreempted,
    Timeout,
    ConnectError,
    FatalOom,
    SignalInterrupt,
    ClientIoBrokenPipe,
    /// Test runner explicitly requested that this exit code be returned
    TestRunner(u8),
}

impl ExitCode {
    pub const fn exit_code(self) -> u32 {
        use ExitCode::*;
        match self {
            Success => 0,
            UnknownFailure => 1,
            InfraError => 2,
            UserError => 3,
            DaemonIsBusy => 4,
            DaemonPreempted => 5,
            Timeout => 6,
            ConnectError => 11,
            FatalOom => 12,
            // Exit codes after 129 should be reserved for exits caused by signals,
            // which should be 128 + signal number.
            // However, SIGINT and SIGPIPE are reversed..
            ClientIoBrokenPipe => 130, // 128 + SIGINT(2)
            SignalInterrupt => 141,    // 128 + SIGPIPE(13)
            TestRunner(code) => code as u32,
        }
    }

    pub fn name(self) -> &'static str {
        use ExitCode::*;
        match self {
            Success => "SUCCESS",
            UnknownFailure => "UNKNOWN_FAILURE",
            InfraError => "INFRA_ERROR",
            UserError => "USER_ERROR",
            DaemonIsBusy => "DAEMON_IS_BUSY",
            DaemonPreempted => "DAEMON_PREEMPTED",
            Timeout => "TIMEOUT",
            ConnectError => "CONNECT_ERROR",
            FatalOom => "FATAL_OOM",
            ClientIoBrokenPipe => "CLIENT_IO_BROKEN_PIPE",
            SignalInterrupt => "SIGNAL_INTERRUPT",
            TestRunner(_) => "TEST_RUNNER",
        }
    }
}
