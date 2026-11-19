/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_trailcam::ServeConfig;

/// Open Trailcam, the web viewer for an event log, on a local port.
///
/// Serves the viewer from the buck2 binary itself, together with the log and
/// what can be read from it about the invocation. On a dev host it listens
/// on a Secure Web Apps port on every interface, so the page opens from a
/// laptop with the VPNless WWW extension and no tunnel. Press Ctrl-C to stop.
#[derive(Debug, clap::Parser)]
pub struct TrailcamCommand {
    /// Port to listen on. Defaults to the first free Secure Web Apps port
    /// (44100-44109); 0 lets the OS pick.
    #[clap(long)]
    port: Option<u16>,

    #[clap(flatten)]
    event_log: EventLogOptions,
}

impl BuckSubcommand for TrailcamCommand {
    const COMMAND_NAME: &'static str = "log-trailcam";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let log = self.event_log.get(&ctx).await?;
        buck2_trailcam::serve(ServeConfig {
            log,
            port: self.port,
        })
        .await?;
        ExitResult::success()
    }
}
