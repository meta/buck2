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
use buck2_cli_proto::UnstableRecentAllocDumpRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::streaming::StreamingCommand;

/// Dump jemalloc's recent-allocations ring to a JSON file.
///
/// Unlike `heap-dump` (a snapshot of currently-live memory), this records
/// individual recent allocations -- each with its size, alloc/free stacks, and
/// timestamps -- so allocation churn can be analyzed by size class and
/// lifetime.
///
/// To use this command, restart buckd with env variable
/// `MALLOC_CONF=prof:true,prof_recent_alloc_max:1000000` (the `..._max` value is
/// the ring size; the default of 0 records nothing) -- or `JE_MALLOC_CONF=...` on
/// macOS, where the jemalloc build is `je_`-prefixed and plain `MALLOC_CONF` is
/// ignored.
#[derive(Debug, clap::Parser)]
pub struct RecentAllocDumpCommand {
    /// The path to write the recent-allocations JSON dump to.
    #[clap(short, long, value_name = "PATH")]
    path: PathArg,
}

#[async_trait(?Send)]
impl StreamingCommand for RecentAllocDumpCommand {
    const COMMAND_NAME: &'static str = "recent_alloc_dump";

    fn existing_only() -> bool {
        true
    }

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        _matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let path = self.path.resolve(&ctx.working_dir);
        buckd
            .with_flushing()
            .unstable_recent_alloc_dump(
                UnstableRecentAllocDumpRequest {
                    destination_path: path.to_str()?.to_owned(),
                },
                events_ctx,
            )
            .await?;

        buck2_client_ctx::eprintln!("Recent-allocations dump written to `{}`", path.to_str()?)?;
        ExitResult::success()
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::none_ref()
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
