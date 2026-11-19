/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::SetLogFilterRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::daemon::client::connect::BuckdConnectOptions;
use buck2_client_ctx::daemon::client::connect::connect_buckd;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::subscribers::stdout_stderr_forwarder::StdoutStderrForwarder;

/// Change the log filter that's currently applied by the Buck2 daemon.
#[derive(Debug, clap::Parser)]
#[clap()]
pub struct SetLogFilterCommand {
    /// The log filter to apply.
    #[clap()]
    log_filter: String,

    /// Whether not to apply it to the daemon.
    #[clap(long)]
    no_daemon: bool,

    /// Whether not to apply it to the forkserver.
    #[clap(long)]
    no_forkserver: bool,
}

impl SetLogFilterCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, ctx: ClientCommandContext<'_>) -> ExitResult {
        ctx.with_runtime(|ctx| async move {
            let mut events_ctx = EventsCtx::new(None, vec![Box::new(StdoutStderrForwarder)]);

            let mut buckd = connect_buckd(
                BuckdConnectOptions::ExistingOnly,
                &mut events_ctx,
                ctx.paths()?,
            )
            .await?;

            buckd
                .with_flushing()
                .set_log_filter(
                    &mut events_ctx,
                    SetLogFilterRequest {
                        log_filter: self.log_filter,
                        daemon: !self.no_daemon,
                        forkserver: !self.no_forkserver,
                    },
                )
                .await?;

            ExitResult::success()
        })
    }
}
