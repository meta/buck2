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
use buck2_cli_proto::UnstableFlushPgoProfileRequest;
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

/// Flush PGO profile data from the daemon to disk.
///
/// Only useful when the daemon was built with `-Cprofile-generate`.
/// Returns whether PGO instrumentation is active.
#[derive(Debug, clap::Parser)]
pub struct FlushPgoProfileCommand {}

#[async_trait(?Send)]
impl StreamingCommand for FlushPgoProfileCommand {
    const COMMAND_NAME: &'static str = "flush_pgo_profile";

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
        let resp = buckd
            .with_flushing()
            .unstable_flush_pgo_profile(UnstableFlushPgoProfileRequest {}, events_ctx)
            .await?;

        if resp.pgo_active {
            buck2_client_ctx::eprintln!("PGO profile data flushed to disk.")?;
        } else {
            buck2_client_ctx::eprintln!(
                "PGO profile data was not flushed. Either the binary was not built with -Cprofile-generate, or the profile write failed (check daemon logs)."
            )?;
        }

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
