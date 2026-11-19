/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;

use crate::commands::status::process_status;

#[derive(Debug, clap::Parser)]
#[clap(
    about = "Start, query, and control the http server",
    long_about = "Start, query, and control the buck2 server, a long-lived process, spanning buck2 command line invocations.
Using this command can ensure the daemon is running.

To stop a specific server, use `buck2 kill` and add `--isolation-dir` for a specific instance.
To stop all instances, use `buck2 killall`."
)]
pub struct ServerCommand {
    #[clap(
        long,
        help = "Print buckd status as JSON after ensuring the server is running."
    )]
    status: bool,
    #[clap(
        long,
        requires = "status",
        help = "Whether to include a state snapshot in the JSON status output."
    )]
    snapshot: bool,
}

#[async_trait(?Send)]
impl StreamingCommand for ServerCommand {
    const COMMAND_NAME: &'static str = "server";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        _matches: BuckArgMatches<'_>,
        _ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let status = buckd
            .with_flushing()
            .status(events_ctx, self.snapshot, false)
            .await?;
        if self.status {
            let json_status = process_status(status)?;
            buck2_client_ctx::println!("{}", serde_json::to_string_pretty(&json_status)?)?;
        } else {
            buck2_client_ctx::println!("buckd.endpoint={}", status.process_info.unwrap().endpoint)?;
        }
        ExitResult::success()
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::simple_ref()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        CommonEventLogOptions::default_ref()
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        CommonBuildConfigurationOptions::default_ref()
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        CommonStarlarkOptions::default_ref()
    }
}
