/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Restarting a command in the same process, after its first attempt failed on corrupted
//! daemon state.

use buck2_core::buck2_env;
use buck2_error::ExitCode;

use crate::daemon::client::connect::DaemonConstraintsRequest;
use crate::exit_result::ExitResult;
use crate::subscribers::observer::ErrorObserver;

/// The state of restarting a command, in one attempt at it. A command is restarted at most
/// once.
pub enum RestartState {
    /// No command is being attempted.
    NoRestart,
    /// This is the first attempt.
    Watching(Restarter),
    /// This attempt is the restart.
    Restarted(RestartRequest),
}

/// Decides whether a command is restarted, when its first attempt has run.
#[derive(Default)]
pub struct Restarter {
    /// `FORCE_WANT_RESTART`: restart the command whatever its result.
    force: bool,
    /// The daemon the command runs against, once it has connected to one.
    daemon: Option<buck2_cli_proto::DaemonConstraints>,
}

impl Restarter {
    pub fn try_new() -> buck2_error::Result<Self> {
        Ok(Self {
            force: buck2_env!("FORCE_WANT_RESTART", bool)?,
            daemon: None,
        })
    }

    /// Called when the command has connected to `daemon`.
    pub(crate) fn connected(&mut self, daemon: &buck2_cli_proto::DaemonConstraints) {
        self.daemon = Some(daemon.clone());
    }

    /// Whether to restart the command, which had `result`: the restart is forced, or the
    /// command failed on corrupted state with the restarter on, and the user did not
    /// interrupt it.
    pub(crate) fn should_restart<'a>(
        &self,
        observers: impl Iterator<Item = &'a dyn ErrorObserver>,
        result: &ExitResult,
    ) -> Option<RestartRequest> {
        let mut reject_daemon = None;
        let mut reject_materializer_state = None;
        let mut restarter_is_enabled = false;
        for observer in observers {
            if observer.daemon_in_memory_state_is_corrupted() {
                reject_daemon = self.daemon.as_ref().map(|daemon| daemon.daemon_id.clone());
            }

            if observer.daemon_materializer_state_is_corrupted() {
                reject_materializer_state = self
                    .daemon
                    .as_ref()
                    .and_then(|daemon| daemon.extra.as_ref())
                    .and_then(|extra| extra.materializer_state_identity.clone());
            }

            if observer.restarter_is_enabled() {
                restarter_is_enabled = true;
            }
        }
        let found_corrupted_state = reject_daemon.is_some() || reject_materializer_state.is_some();
        let interrupted_by_user = matches!(result.exit_code(), Some(ExitCode::SignalInterrupt));

        let should_restart = self.force
            || (!result.is_success()
                && !interrupted_by_user
                && restarter_is_enabled
                && found_corrupted_state);
        should_restart.then_some(RestartRequest {
            reject_daemon,
            reject_materializer_state,
        })
    }
}

/// What the restart of a command must reject: the state its first attempt found corrupted.
#[derive(Debug, PartialEq, Eq)]
pub struct RestartRequest {
    reject_daemon: Option<String>,
    reject_materializer_state: Option<String>,
}

impl RestartRequest {
    pub(crate) fn apply_to_constraints(&self, req: &mut DaemonConstraintsRequest) {
        req.reject_daemon = self.reject_daemon.clone();
        req.reject_materializer_state = self.reject_materializer_state.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the events of an attempt reported.
    struct Observed {
        restarter_is_enabled: bool,
        daemon_state_is_corrupted: bool,
        materializer_state_is_corrupted: bool,
    }

    impl ErrorObserver for Observed {
        fn daemon_in_memory_state_is_corrupted(&self) -> bool {
            self.daemon_state_is_corrupted
        }

        fn daemon_materializer_state_is_corrupted(&self) -> bool {
            self.materializer_state_is_corrupted
        }

        fn restarter_is_enabled(&self) -> bool {
            self.restarter_is_enabled
        }
    }

    fn observed(
        restarter_is_enabled: bool,
        daemon_state_is_corrupted: bool,
        materializer_state_is_corrupted: bool,
    ) -> Observed {
        Observed {
            restarter_is_enabled,
            daemon_state_is_corrupted,
            materializer_state_is_corrupted,
        }
    }

    fn should_restart(
        force: bool,
        observed: Observed,
        result: ExitResult,
    ) -> Option<RestartRequest> {
        let restarter = Restarter {
            force,
            daemon: Some(daemon()),
        };
        let observers: [&dyn ErrorObserver; 1] = [&observed];
        restarter.should_restart(observers.into_iter(), &result)
    }

    fn daemon() -> buck2_cli_proto::DaemonConstraints {
        buck2_cli_proto::DaemonConstraints {
            daemon_id: "daemon".to_owned(),
            extra: Some(buck2_cli_proto::ExtraDaemonConstraints {
                trace_io_enabled: false,
                materializer_state_identity: Some("materializer".to_owned()),
            }),
            ..Default::default()
        }
    }

    fn rejects(daemon: bool, materializer_state: bool) -> Option<RestartRequest> {
        Some(RestartRequest {
            reject_daemon: daemon.then(|| "daemon".to_owned()),
            reject_materializer_state: materializer_state.then(|| "materializer".to_owned()),
        })
    }

    fn failed() -> ExitResult {
        ExitResult::bail("failed")
    }

    #[test]
    fn test_restarts_a_failed_command_away_from_corrupted_state() {
        assert_eq!(
            should_restart(false, observed(true, true, false), failed()),
            rejects(true, false)
        );
        assert_eq!(
            should_restart(false, observed(true, false, true), failed()),
            rejects(false, true)
        );
    }

    #[test]
    fn test_no_restart_without_corruption_or_after_success_or_with_the_restarter_off() {
        assert_eq!(
            should_restart(false, observed(true, false, false), failed()),
            None
        );
        assert_eq!(
            should_restart(false, observed(true, true, false), ExitResult::success()),
            None
        );
        assert_eq!(
            should_restart(false, observed(false, true, false), failed()),
            None
        );
    }

    #[test]
    fn test_forced_restart_ignores_the_result_and_the_restarter() {
        assert_eq!(
            should_restart(true, observed(false, false, false), ExitResult::success()),
            rejects(false, false)
        );
        assert_eq!(
            should_restart(true, observed(false, true, false), failed()),
            rejects(true, false)
        );
    }

    #[test]
    fn test_no_restart_of_a_command_that_the_user_interrupted_or_that_had_no_daemon() {
        assert_eq!(
            should_restart(
                false,
                observed(true, true, false),
                ExitResult::signal_interrupt()
            ),
            None
        );

        let observed = observed(true, true, true);
        let observers: [&dyn ErrorObserver; 1] = [&observed];
        assert_eq!(
            Restarter::default().should_restart(observers.into_iter(), &failed()),
            None
        );
    }
}
