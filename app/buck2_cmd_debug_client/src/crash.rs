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
use buck2_cli_proto::UnstableCrashRequest;
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

#[derive(Debug, Clone, clap::ValueEnum)]
enum CrashType {
    Panic,
    Abort,
    Oom,
}

impl CrashType {
    fn to_proto(&self) -> i32 {
        let crash_type = match self {
            CrashType::Panic => buck2_cli_proto::unstable_crash_request::CrashType::Panic,
            CrashType::Abort => buck2_cli_proto::unstable_crash_request::CrashType::Abort,
            CrashType::Oom => buck2_cli_proto::unstable_crash_request::CrashType::Oom,
        };
        crash_type as i32
    }
}

#[derive(Debug, clap::Parser)]
pub struct CrashCommand {
    #[arg(value_enum)]
    crash_type: CrashType,
    /// Number of bytes to allocate to trigger OOM killing (only used with `oom` crash type).
    #[arg(long)]
    bytes: Option<u64>,
    /// Event-log options.
    #[clap(flatten)]
    pub event_log_opts: CommonEventLogOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for CrashCommand {
    const COMMAND_NAME: &'static str = "crash";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        _matches: BuckArgMatches<'_>,
        _ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        buckd
            .with_flushing()
            .unstable_crash(
                UnstableCrashRequest {
                    crash_type: self.crash_type.to_proto(),
                    bytes: self.bytes.unwrap_or(0),
                },
                events_ctx,
            )
            .await??;
        unreachable!("request should have failed")
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::default_ref()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        CommonBuildConfigurationOptions::default_ref()
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        CommonStarlarkOptions::default_ref()
    }
}
