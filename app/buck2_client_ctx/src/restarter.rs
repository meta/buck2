/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::buck2_env;

use crate::daemon::client::BuckdClientConnector;
use crate::daemon::client::connect::DaemonConstraintsRequest;
use crate::events_ctx::EventsCtx;
use crate::exit_result::ExitResult;

/// Monitor the state of our execution and decide whether we should restart the command we just
/// attempted to execute.
pub struct Restarter {
    /// `FORCE_WANT_RESTART`: restart the command whatever its result.
    force: bool,
    pub reject_daemon: Option<String>,
    pub reject_materializer_state: Option<String>,
    pub enable_restarter: bool,
}

impl Restarter {
    pub fn try_new() -> buck2_error::Result<Self> {
        Ok(Self {
            force: buck2_env!("FORCE_WANT_RESTART", bool)?,
            reject_daemon: None,
            reject_materializer_state: None,
            enable_restarter: false,
        })
    }

    /// Observe our BuckdClientConnector after execution to decide whether we should be
    /// restarting.
    pub fn observe(
        &mut self,
        client: &BuckdClientConnector,
        events_ctx: &mut EventsCtx,
        result: &ExitResult,
    ) {
        for obs in events_ctx.error_observers() {
            if obs.daemon_in_memory_state_is_corrupted() {
                self.reject_daemon = Some(client.daemon_constraints().daemon_id.clone());
            }

            if obs.daemon_materializer_state_is_corrupted() {
                self.reject_materializer_state = client
                    .daemon_constraints()
                    .extra
                    .as_ref()
                    .and_then(|e| e.materializer_state_identity.clone());
            }

            if obs.restarter_is_enabled() {
                self.enable_restarter = true;
            }
        }

        if self.should_restart(result)
            && let Some(recorder) = events_ctx.recorder.as_mut()
        {
            recorder.set_should_restart();
        }
    }

    /// Whether to restart the command: the restart is forced, or the command failed on
    /// corrupted state with the restarter on.
    pub fn should_restart(&self, result: &ExitResult) -> bool {
        let found_corrupted_state =
            self.reject_daemon.is_some() || self.reject_materializer_state.is_some();

        self.force || (!result.is_success() && self.enable_restarter && found_corrupted_state)
    }

    pub fn apply_to_constraints(&self, req: &mut DaemonConstraintsRequest) {
        req.reject_daemon = self.reject_daemon.clone();
        req.reject_materializer_state = self.reject_materializer_state.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_restart() {
        let restarter = |force, enable_restarter, corrupted: bool| Restarter {
            force,
            reject_daemon: corrupted.then(|| "daemon".to_owned()),
            reject_materializer_state: None,
            enable_restarter,
        };
        let failed = ExitResult::bail("failed");

        assert!(restarter(false, true, true).should_restart(&failed));
        assert!(!restarter(false, true, true).should_restart(&ExitResult::success()));
        assert!(!restarter(false, true, false).should_restart(&failed));
        assert!(!restarter(false, false, true).should_restart(&failed));
        assert!(restarter(true, false, false).should_restart(&ExitResult::success()));
    }
}
