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

/// Watches the first attempt at a command, to decide whether the command is restarted.
#[derive(Default)]
pub struct Restarter {
    /// `FORCE_WANT_RESTART`: restart the command whatever its result.
    force: bool,
    /// The daemon the command runs against, once it has connected to one.
    daemon: Option<buck2_cli_proto::DaemonConstraints>,
    /// `[buck2] restarter`, as the daemon reports it.
    restarter_is_enabled: bool,
    /// Whether a soft error reported the daemon's in-memory state as corrupted.
    daemon_in_memory_state_is_corrupted: bool,
    /// The error that the first such soft error reported, if it carried one.
    triggering_error: Option<Box<buck2_data::ErrorReport>>,
    /// Whether a soft error reported the daemon's materializer state as corrupted.
    daemon_materializer_state_is_corrupted: bool,
}

impl Restarter {
    pub fn try_new() -> buck2_error::Result<Self> {
        Ok(Self {
            force: buck2_env!("FORCE_WANT_RESTART", bool)?,
            ..Self::default()
        })
    }

    /// The error that the daemon reported its in-memory state as corrupted with.
    pub(crate) fn triggering_error(&self) -> Option<&buck2_data::ErrorReport> {
        self.triggering_error.as_deref()
    }

    /// Called when the command has connected to `daemon`.
    pub(crate) fn connected(&mut self, daemon: &buck2_cli_proto::DaemonConstraints) {
        self.daemon = Some(daemon.clone());
    }

    pub(crate) fn handle_event(&mut self, event: &buck2_data::instant_event::Data) {
        match event {
            buck2_data::instant_event::Data::RestartConfiguration(conf) => {
                self.restarter_is_enabled = conf.enable_restarter;
            }
            buck2_data::instant_event::Data::StructuredError(err)
                if err.soft_error_category.is_some() =>
            {
                self.daemon_in_memory_state_is_corrupted |= err.daemon_in_memory_state_is_corrupted;
                if self.triggering_error.is_none() {
                    self.triggering_error = err.error.clone();
                }
                self.daemon_materializer_state_is_corrupted |=
                    err.daemon_materializer_state_is_corrupted;
            }
            _ => {}
        }
    }

    /// Whether to restart the command, which had `result`: the restart is forced, or the
    /// command failed on corrupted state with the restarter on. A command that the user
    /// interrupted did not fail, and neither did a `buck2 run` that is to run its target.
    pub(crate) fn should_restart(&self, result: &ExitResult) -> Option<RestartRequest> {
        let daemon = self.daemon.as_ref();
        let reject_daemon = if self.daemon_in_memory_state_is_corrupted {
            daemon.map(|daemon| daemon.daemon_id.clone())
        } else {
            None
        };
        let reject_materializer_state = if self.daemon_materializer_state_is_corrupted {
            daemon
                .and_then(|daemon| daemon.extra.as_ref())
                .and_then(|extra| extra.materializer_state_identity.clone())
        } else {
            None
        };
        let found_corrupted_state = reject_daemon.is_some() || reject_materializer_state.is_some();
        let failed = result
            .exit_code()
            .is_some_and(|code| !matches!(code, ExitCode::Success | ExitCode::SignalInterrupt));

        let should_restart =
            self.force || (failed && self.restarter_is_enabled && found_corrupted_state);
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

    fn error(
        soft: bool,
        daemon_state_is_corrupted: bool,
        materializer_state_is_corrupted: bool,
    ) -> buck2_data::instant_event::Data {
        buck2_data::StructuredError {
            soft_error_category: soft.then(buck2_data::SoftError::default),
            daemon_in_memory_state_is_corrupted: daemon_state_is_corrupted,
            daemon_materializer_state_is_corrupted: materializer_state_is_corrupted,
            ..Default::default()
        }
        .into()
    }

    /// The events of an attempt.
    fn observed(
        restarter_is_enabled: bool,
        daemon_state_is_corrupted: bool,
        materializer_state_is_corrupted: bool,
    ) -> Vec<buck2_data::instant_event::Data> {
        let enabled = buck2_data::RestartConfiguration {
            enable_restarter: restarter_is_enabled,
        };
        vec![
            enabled.into(),
            error(
                true,
                daemon_state_is_corrupted,
                materializer_state_is_corrupted,
            ),
        ]
    }

    fn after(mut restarter: Restarter, events: &[buck2_data::instant_event::Data]) -> Restarter {
        for event in events {
            restarter.handle_event(event);
        }
        restarter
    }

    fn should_restart(
        force: bool,
        observed: Vec<buck2_data::instant_event::Data>,
        result: ExitResult,
    ) -> Option<RestartRequest> {
        let restarter = Restarter {
            force,
            daemon: Some(daemon()),
            ..Default::default()
        };
        after(restarter, &observed).should_restart(&result)
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
    fn test_no_restart_of_a_command_that_did_not_fail_or_that_had_no_daemon() {
        assert_eq!(
            should_restart(
                false,
                observed(true, true, false),
                ExitResult::signal_interrupt()
            ),
            None
        );
        let run_target = ExitResult::exec("target".into(), Vec::new(), None, Default::default());
        assert_eq!(
            should_restart(false, observed(true, true, false), run_target),
            None
        );
        assert_eq!(
            after(Restarter::default(), &observed(true, true, true)).should_restart(&failed()),
            None
        );
    }

    #[test]
    fn test_ignores_corruption_that_no_soft_error_reports() {
        let events = [observed(true, false, false), vec![error(false, true, true)]].concat();
        assert_eq!(should_restart(false, events, failed()), None);
    }
}
