/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::new_generic::ExplainRequest;
use buck2_cli_proto::new_generic::NewGenericRequest;
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
use buck2_event_log::file_names::get_local_logs;
use clap::Parser as _;
use tonic::async_trait;

use crate::commands::build::BuildCommand;

/// Generates web browser view that shows actions that ran in the last build
/// mapped to the target graph
#[derive(Debug, clap::Parser)]
#[clap(name = "explain")]
pub struct ExplainCommand {
    /// Output file path for profile data.
    ///
    /// File will be created if it does not exist, and overwritten if it does.
    #[clap(long, short = 'o')]
    output: Option<PathArg>,
    /// Whether to upload the output to Manifold
    /// Deprecated: now we always upload to Manifold, this flag is a no-op
    #[clap(long)]
    upload: bool,
    /// Add target code pointer. This invalidates cache, slowing things down
    #[clap(long)]
    stack: bool,
    /// Dev only: dump the flatbuffer info to file path
    #[clap(long, hide = true)]
    fbs_dump: Option<PathArg>,
}

// TODO: not sure I need StreamingCommand
#[async_trait(?Send)]
impl StreamingCommand for ExplainCommand {
    const COMMAND_NAME: &'static str = "explain";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        _matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        if cfg!(windows) {
            return ExitResult::bail("Not implemented for windows");
        }

        let output = self.output.clone().map(|o| o.resolve(&ctx.working_dir));

        // Get the most recent log
        let paths = ctx.paths()?;
        let logs = get_local_logs(&paths.log_dir())?; // oldest first
        let mut logs = logs
            .into_iter()
            .filter(|l| match l.command_from_filename().ok() {
                // Support only build commands for now
                Some(c) => c == "build",
                None => false,
            });

        let build_log = match logs.next_back() {
            Some(log) => log,
            None => {
                return ExitResult::bail(
                    "No recent build commands found, did you try building something first?",
                );
            }
        };

        // Check things are the same as last build
        let (invocation, _) = build_log.unpack_stream().await?;
        buck2_client_ctx::eprintln!(
            "\nUsing last build invocation `buck2 {}`\n",
            invocation.command_line_args[1..].join(" ")
        )?;

        if invocation.working_dir != ctx.working_dir.to_string() {
            return ExitResult::bail(format!(
                "working dir mismatch {} and {}",
                invocation.working_dir, ctx.working_dir,
            ));
        }

        let uuid = invocation.trace_id;

        // We are interested in the args passed only to a build command
        let command = invocation.expanded_command_line_args;
        let build_index = command.iter().position(|word| word == "build");
        let index = match build_index {
            Some(index) => index,
            None => return ExitResult::bail("Only build command is supported"),
        };
        let command = &command[index..];

        // Parse retrived args
        let build_args = BuildCommand::parse_from(command);

        // TODO iguridi: get things like configs and target universe too
        let patterns = build_args.patterns();
        if patterns.len() != 1 {
            return ExitResult::bail("Only one target pattern is supported");
        }
        let target = patterns[0].to_owned();
        let target_universe = build_args.target_universe().clone();
        let target_cfg = build_args.target_cfg();

        // TODO iguridi: add option to turn manifold upload off for OSS
        let manifold_path = Some(format!("flat/{uuid}-explain.html"));

        let mut context = ctx.empty_client_context("explain")?;
        context.target_call_stacks = self.stack;
        context.reuse_current_config = true;
        context.reuse_current_config_implicitly = true;

        buckd
            .with_flushing()
            .new_generic(
                context,
                NewGenericRequest::Explain(ExplainRequest {
                    output,
                    target,
                    fbs_dump: self.fbs_dump.map(|x| x.resolve(&ctx.working_dir)),
                    manifold_path: manifold_path.clone(),
                    target_universe,
                    target_cfg,
                    log_path: build_log.path().to_owned(),
                }),
                events_ctx,
                None,
            )
            .await??;

        if let Some(p) = manifold_path {
            buck2_client_ctx::eprintln!(
                "\nView html in your browser: https://interncache-all.fbcdn.net/manifold/buck2_logs/{} (requires VPN/lighthouse)\n",
                p
            )?;
        }

        ExitResult::success()
    }

    fn existing_only() -> bool {
        true
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
