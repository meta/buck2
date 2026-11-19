/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::new_generic::DocsRequest;
use buck2_cli_proto::new_generic::DocsResponse;
use buck2_cli_proto::new_generic::DocsStarlarkBuiltinsRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::streaming::StreamingCommand;

/// Generate documentation for starlark builtins.
///
/// This command is designed to support buck2's doc generation and does not have stable output.
#[derive(Debug, clap::Parser)]
#[clap(name = "docs starlark-builtins")]
pub(crate) struct StarlarkBuiltinsCommand {
    /// The directory to output files to
    #[clap(long, required = true)]
    output_dir: PathArg,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

#[async_trait::async_trait(?Send)]
impl StreamingCommand for StarlarkBuiltinsCommand {
    const COMMAND_NAME: &'static str = "docs starlark-builtins";
    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let client_context = ctx.client_context(matches, &self)?;

        let p = self.output_dir.resolve(&ctx.working_dir).to_string();

        let response = buckd
            .with_flushing()
            .new_generic(
                client_context,
                buck2_cli_proto::new_generic::NewGenericRequest::Docs(
                    DocsRequest::StarlarkBuiltins(DocsStarlarkBuiltinsRequest { path: p }),
                ),
                events_ctx,
                ctx.console_interaction_stream(&self.common_opts.console_opts),
            )
            .await??;

        let buck2_cli_proto::new_generic::NewGenericResponse::Docs(DocsResponse::NoOutput) =
            response
        else {
            return ExitResult::bail("Unexpected response type from generic command");
        };

        ExitResult::success()
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        &self.common_opts.console_opts
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_opts.event_log_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        &self.common_opts.config_opts
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        &self.common_opts.starlark_opts
    }
}
