/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::daemon::client::BuckdLifecycleLock;
use buck2_client_ctx::daemon::client::kill::TenantShutdownCommand;
use buck2_client_ctx::daemon::client::kill::kill_command_impl;
use buck2_client_ctx::daemon::client::kill::kill_unidaemon_tenant;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::startup_deadline::StartupDeadline;

/// Kill the buck daemon.
///
/// Note there's also `buck2 killall` and `buck2 clean`.
///
/// `buck2 killall` kills all the buck2 processes on the machine.
///
/// `buck2 clean` kills the buck2 daemon and also deletes the buck2 state files.
///
/// Under daemon sharing (`buck2.share_daemon`), this evicts the current tenant from the shared
/// daemon instead; the daemon exits after its final tenant is evicted.
#[derive(Debug, clap::Parser)]
pub struct KillCommand {
    #[clap(flatten)]
    pub(crate) event_log_opts: CommonEventLogOptions,
}

impl BuckSubcommand for KillCommand {
    const COMMAND_NAME: &'static str = "kill";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let paths = ctx.paths()?;
        if paths.routing()?.is_shared() {
            let lifecycle = BuckdLifecycleLock::lock_with_timeout(
                paths.tenant_paths().unshared_daemon_dir()?,
                StartupDeadline::duration_from_now(Duration::from_secs(10))?,
            )
            .await?;
            return kill_unidaemon_tenant(&lifecycle, TenantShutdownCommand::Kill, None)
                .await
                .into();
        }
        let lifecycle_lock = BuckdLifecycleLock::lock_with_timeout(
            paths.daemon_dir()?,
            StartupDeadline::duration_from_now(Duration::from_secs(10))?,
        )
        .await?;

        kill_command_impl(&lifecycle_lock, "`buck kill` was invoked", false)
            .await
            .into()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }
}
