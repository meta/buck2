/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use async_trait::async_trait;
use buck2_cli_proto::BxlRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::command_outcome::CommandOutcome;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::build::CommonBuildOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::StdoutPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::streaming::StreamingCommand;

use crate::commands::build::FinalArtifactMaterializations;
use crate::commands::build::FinalArtifactUploads;
use crate::commands::build::MaterializationsToProto;
use crate::commands::build::UploadsToProto;
use crate::commands::build::print_build_result;

#[derive(Debug, clap::Parser)]
#[clap(name = "bxl", about = "Run BXL scripts")]
pub struct BxlCommand {
    #[clap(flatten)]
    bxl_opts: BxlCommandOptions,

    #[clap(flatten)]
    target_cfg: TargetCfgOptions,

    #[clap(flatten)]
    common_ops: CommonCommandOptions,
}

#[derive(Debug, clap::Parser)]
pub struct BxlCommandOptions {
    #[clap(
        long = "materializations",
        short = 'M',
        help = "Materialize (or skip) the final artifacts, bypassing buckconfig.",
        ignore_case = true,
        value_enum
    )]
    materializations: Option<FinalArtifactMaterializations>,

    #[clap(
        long = "upload-final-artifacts",
        help = "Upload (or skip) the final artifacts.",
        ignore_case = true,
        value_enum
    )]
    upload_final_artifacts: Option<FinalArtifactUploads>,

    #[clap(
        name = "BXL label",
        help = "The bxl function to execute as defined by the label of form `<cell>//path/file.bxl:<function>`"
    )]
    pub bxl_label: String,

    #[clap(
        name = "BXL INPUT ARGS",
        help = "Arguments passed to the bxl script",
        raw = true
    )]
    pub bxl_args: Vec<String>,

    /// Write user events to this log file. Both user and internal events are written to main event log.
    /// If this flag is specified, user events are additionally written to user event log.
    /// Log format is JSONL, uncompressed if no known extensions are detected, or you can explicitly specify
    /// the compression via the file extension (ex: `.json-lines.gz` would be gzip compressed, `.json-lines.zst`
    /// would be zstd compressed). Resulting log is compatible with `buck2 log show-user`.
    #[clap(value_name = "PATH", long = "user-event-log")]
    pub user_event_log: Option<PathArg>,

    #[clap(flatten)]
    build_opts: CommonBuildOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for BxlCommand {
    const COMMAND_NAME: &'static str = "bxl";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = ctx.client_context(matches, &self)?;
        let result = buckd
            .with_flushing()
            .bxl(
                BxlRequest {
                    context: Some(context),
                    bxl_label: self.bxl_opts.bxl_label,
                    bxl_args: self.bxl_opts.bxl_args,
                    build_opts: Some(self.bxl_opts.build_opts.to_proto()),
                    target_cfg: Some(self.target_cfg.target_cfg()),
                    final_artifact_materializations: self.bxl_opts.materializations.to_proto()
                        as i32,
                    final_artifact_uploads: self.bxl_opts.upload_final_artifacts.to_proto() as i32,
                    print_stacktrace: ctx.verbosity.print_success_stderr(),
                },
                events_ctx,
                ctx.console_interaction_stream(&self.common_ops.console_opts),
                &mut StdoutPartialResultHandler,
            )
            .await;
        let success = match &result {
            Ok(CommandOutcome::Success(response)) => response.errors.is_empty(),
            _ => false,
        };

        let console = self.common_ops.console_opts.final_console();

        if success {
            if ctx.verbosity.print_success_message() {
                console.print_success("BXL SUCCEEDED")?;
            }
        } else {
            console.print_error("BXL FAILED")?;
        }

        // Action errors will have already been printed, but any other type
        // of error will be printed below the FAILED line here.
        let response = result??;

        print_build_result(&console, &response.errors)?;
        let mut stdout = Vec::new();
        if let Some(build_report) = response.serialized_build_report {
            stdout.extend(build_report.as_bytes());
            writeln!(&mut stdout)?;
        }

        if !success {
            return ExitResult::from_command_result_errors(response.errors);
        }

        ExitResult::success().with_stdout(stdout)
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        &self.common_ops.console_opts
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_ops.event_log_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        &self.common_ops.config_opts
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        &self.common_ops.starlark_opts
    }

    fn user_event_log(&self) -> &Option<PathArg> {
        &self.bxl_opts.user_event_log
    }
}
