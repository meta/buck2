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

use buck2_cli_proto::EvictTenantRequest;
use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::daemon::client::BuckdLifecycleLock;
use buck2_client_ctx::daemon::client::connect::BuckdConnectOptions;
use buck2_client_ctx::daemon::client::connect::connect_buckd;
use buck2_client_ctx::daemon::client::connect::shared_daemon_is_inactive;
use buck2_client_ctx::daemon::client::kill::kill_command_impl;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::startup_deadline::StartupDeadline;
use buck2_core::buck2_env;

/// Kill the buck daemon.
///
/// Note there's also `buck2 killall` and `buck2 clean`.
///
/// `buck2 killall` kills all the buck2 processes on the machine.
///
/// `buck2 clean` kills the buck2 daemon and also deletes the buck2 state files.
///
/// With `BUCK2_SHARE_DAEMON=true`, this evicts the current tenant from the shared daemon
/// instead; the daemon exits after its final tenant is evicted.
#[derive(Debug, clap::Parser)]
pub struct KillCommand {
    #[clap(flatten)]
    pub(crate) event_log_opts: CommonEventLogOptions,

    /// Before shutting down, wait up to this long (e.g. `120s`) for the BES server to
    /// acknowledge the daemon's build events, closing every stream. A server that
    /// acknowledges a stream only once it ends, as BuildBuddy does, loses what it was not
    /// sent before the daemon exits. The kill takes at most this long more than without it.
    #[clap(long, value_name = "DURATION", default_value = "0s")]
    bes_drain_timeout: humantime::Duration,
}

impl BuckSubcommand for KillCommand {
    const COMMAND_NAME: &'static str = "kill";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let paths = ctx.paths()?;
        let daemon_dir = paths.daemon_dir()?;

        if !buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
            let lifecycle_lock = BuckdLifecycleLock::lock_with_timeout(
                daemon_dir,
                StartupDeadline::duration_from_now(Duration::from_secs(10))?,
            )
            .await?;

            return kill_command_impl(
                &lifecycle_lock,
                "`buck kill` was invoked",
                false,
                self.bes_drain_timeout.into(),
            )
            .await
            .into();
        }

        match connect_buckd(BuckdConnectOptions::ExistingOnly, events_ctx, paths).await {
            Ok(mut buckd) => {
                let response = buckd
                    .with_flushing()
                    .evict_tenant(
                        EvictTenantRequest {
                            context: Some(ctx.empty_client_context(Self::COMMAND_NAME)?),
                        },
                        events_ctx,
                    )
                    .await?;
                if response.evicted {
                    buck2_client_ctx::eprintln!(
                        "Evicted tenant generation {}",
                        response.generation
                    )?;
                    if response.shutting_down {
                        buck2_client_ctx::eprintln!("Final tenant evicted; shutting down daemon")?;
                    }
                } else {
                    buck2_client_ctx::eprintln!("No matching tenant was registered")?;
                }
                ExitResult::success()
            }
            Err(error) => {
                let _lifecycle_lock = BuckdLifecycleLock::lock_with_timeout(
                    daemon_dir.clone(),
                    StartupDeadline::duration_from_now(Duration::from_secs(10))?,
                )
                .await?;
                if !shared_daemon_is_inactive(&daemon_dir).await? {
                    return error.into();
                }
                buck2_client_ctx::eprintln!("no buckd server running")?;
                ExitResult::success()
            }
        }
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }
}
