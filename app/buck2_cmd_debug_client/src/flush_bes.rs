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

use async_trait::async_trait;
use buck2_cli_proto::FlushBesRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::kill::report_bes_drain;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;

/// A running command's stream stays open, so this fails while a build runs against a server
/// that acknowledges a stream only once it ends. Before `buck2 kill`, which closes those too,
/// use `buck2 kill --bes-drain-timeout` instead.
#[derive(Debug, clap::Parser)]
pub struct FlushBesCommand {
    /// How long to wait for the server's acknowledgements, e.g. `120s`.
    #[clap(long, value_name = "DURATION", default_value = "60s")]
    timeout: humantime::Duration,

    #[clap(flatten)]
    common_event_opts: CommonEventLogOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for FlushBesCommand {
    const COMMAND_NAME: &'static str = "flush_bes";

    fn existing_only() -> bool {
        true
    }

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        _matches: BuckArgMatches<'_>,
        _ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let timeout: Duration = self.timeout.into();
        let result = buckd
            .with_flushing()
            .flush_bes(
                FlushBesRequest {
                    timeout: Some(timeout.try_into()?),
                },
                events_ctx,
            )
            .await?;
        if report_bes_drain(&result)? {
            ExitResult::success()
        } else {
            ExitResult::bail("the BES server did not acknowledge every event")
        }
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::none_ref()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_event_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        CommonBuildConfigurationOptions::default_ref()
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        CommonStarlarkOptions::default_ref()
    }
}
