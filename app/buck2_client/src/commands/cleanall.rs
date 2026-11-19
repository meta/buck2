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
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_wrapper_common::CLEAN_STALE_HELP;

/// Clean Buck2 state for every known project and isolation directory.
#[derive(Debug, clap::Parser)]
pub struct CleanallCommand {
    #[clap(long, help = CLEAN_STALE_HELP)]
    stale: bool,

    #[clap(flatten)]
    pub(crate) event_log_opts: CommonEventLogOptions,
}

impl BuckSubcommand for CleanallCommand {
    const COMMAND_NAME: &'static str = "cleanall";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        if !self.stale {
            return ExitResult::bail("`buck2 cleanall` without `--stale` is not implemented yet");
        }

        match buck2_wrapper_common::cleanall_stale().await {
            Ok(()) => ExitResult::success(),
            Err(error) => ExitResult::err(error),
        }
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }
}
