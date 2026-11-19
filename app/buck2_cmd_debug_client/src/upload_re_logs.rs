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
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::upload_re_logs::upload_re_logs;
use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;

#[derive(Debug, clap::Parser)]
#[clap(about = "upload RE logs")]
pub struct UploadReLogsCommand {
    #[clap(long)]
    session_id: String,
}

impl BuckSubcommand for UploadReLogsCommand {
    const COMMAND_NAME: &'static str = "upload-re-logs";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut buck2_client_ctx::events_ctx::EventsCtx,
    ) -> ExitResult {
        buck2_core::facebook_only();
        events_ctx.log_invocation_record = false;
        let manifold = ManifoldClient::new().await?;
        // TODO: This should receive the path from the caller.
        let re_logs_dir = ctx.paths()?.re_logs_dir();
        upload_re_logs(
            &manifold,
            Bucket::RE_LOGS,
            &re_logs_dir,
            &self.session_id,
            &format!("flat/{}.log.zst", self.session_id),
        )
        .await?;
        ExitResult::success()
    }
}
