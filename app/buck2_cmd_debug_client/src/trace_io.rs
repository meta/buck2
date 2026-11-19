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
use buck2_cli_proto::TraceIoRequest;
use buck2_cli_proto::TraceIoResponse;
use buck2_cli_proto::trace_io_request;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::command_outcome::CommandOutcome;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::daemon::client::connect::DesiredTraceIoState;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_offline_archive::ExternalSymlink;
use buck2_offline_archive::OfflineArchiveManifest;
use buck2_offline_archive::RelativeSymlink;
use buck2_offline_archive::RepositoryMetadata;

/// Enable I/O tracing in the buck daemon so we keep track of which files
/// go into a build.
#[derive(Debug, clap::Parser)]
pub struct TraceIoCommand {
    #[clap(subcommand)]
    trace_io_action: Subcommand,
}

/// Sub-settings of I/O tracing
#[derive(Debug, clap::Subcommand)]
enum Subcommand {
    /// Turn on I/O tracing. Has no effect if tracing is already enabled.
    Enable,
    /// Turn off I/O tracing. Has no effect if tracing is already disabled.
    Disable,
    /// Return whether I/O tracing is enabled.
    Status,
    /// Exports the I/O trace taken by the daemon in a structured manifest format.
    ExportManifest {
        #[clap(short, long, help = "Output path to write manifest to")]
        out: Option<PathArg>,
    },
}

impl TraceIoCommand {
    async fn send_request(
        &self,
        req: TraceIoRequest,
        buckd: &mut BuckdClientConnector,
        events_ctx: &mut EventsCtx,
        ctx: &mut ClientCommandContext<'_>,
    ) -> buck2_error::Result<CommandOutcome<TraceIoResponse>> {
        buckd
            .with_flushing()
            .trace_io(
                req,
                events_ctx,
                ctx.console_interaction_stream(self.console_opts()),
                &mut NoPartialResultHandler,
            )
            .await
    }
}

#[async_trait(?Send)]
impl StreamingCommand for TraceIoCommand {
    const COMMAND_NAME: &'static str = "trace-io";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = ctx.client_context(matches, &self)?;
        match &self.trace_io_action {
            Subcommand::Status => {
                let req = TraceIoRequest {
                    context: Some(context),
                    read_state: Some(trace_io_request::ReadIoTracingState { with_trace: false }),
                };
                let resp = self.send_request(req, buckd, events_ctx, ctx).await??;
                buck2_client_ctx::println!("I/O tracing status: {}", resp.enabled)?;
            }
            Subcommand::ExportManifest { out } => {
                let req = TraceIoRequest {
                    context: Some(context),
                    read_state: Some(trace_io_request::ReadIoTracingState { with_trace: true }),
                };
                let resp = self.send_request(req, buckd, events_ctx, ctx).await??;

                let manifest = OfflineArchiveManifest {
                    paths: resp
                        .trace
                        .into_iter()
                        // Note: Safe because these are all ProjectRelativePath's on the daemon side.
                        .map(ProjectRelativePathBuf::unchecked_new)
                        .collect(),
                    external_paths: resp
                        .external_entries
                        .into_iter()
                        .map(|path| {
                            AbsNormPathBuf::try_from(path)
                                .expect("got unexpected non-absolute path")
                        })
                        .collect(),
                    relative_symlinks: resp
                        .relative_symlinks
                        .into_iter()
                        .map(|symlink| RelativeSymlink {
                            link: ProjectRelativePathBuf::unchecked_new(symlink.link),
                            target: ProjectRelativePathBuf::unchecked_new(symlink.target),
                        })
                        .collect(),
                    external_symlinks: resp
                        .external_symlinks
                        .into_iter()
                        .map(|symlink| ExternalSymlink {
                            link: ProjectRelativePathBuf::unchecked_new(symlink.link),
                            target: AbsPathBuf::try_from(symlink.target)
                                .expect("got unexpected non-absolute symlink target"),
                            remaining_path: symlink
                                .remaining_path
                                .map(ForwardRelativePathBuf::unchecked_new),
                        })
                        .collect(),
                    repository: RepositoryMetadata::from_cwd()
                        .buck_error_context("creating repository metadata")?,
                };
                let serialized = serde_json::to_string(&manifest)
                    .buck_error_context("serializing offline archive manifest to json")?;
                if let Some(output_path) = &out {
                    // input path from --export-manifest
                    fs_util::write(output_path.resolve(&ctx.working_dir), &serialized)
                        .categorize_input()
                        .buck_error_context("writing offline archive manifest")?;
                } else {
                    buck2_client_ctx::println!("{}", serialized)?;
                }
            }
            // Subcommand::{Enable, Disable} handled by StreamingCommand::trace_io()
            // via implicit daemon restart.
            _ => {}
        }

        ExitResult::success()
    }

    /// Results in a daemon restart if tracing is not already enabled.
    fn trace_io(&self) -> DesiredTraceIoState {
        match self.trace_io_action {
            Subcommand::Enable => DesiredTraceIoState::Enabled,
            Subcommand::Disable => DesiredTraceIoState::Disabled,
            _ => DesiredTraceIoState::Existing,
        }
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::default_ref()
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
