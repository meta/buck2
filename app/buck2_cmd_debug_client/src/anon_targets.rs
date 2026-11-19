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
use buck2_cli_proto::new_generic::AnonTargetsRequest;
use buck2_cli_proto::new_generic::NewGenericRequest;
use buck2_cli_proto::new_generic::NewGenericResponse;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::query_args::CommonAttributeArgs;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_error::internal_error;

/// List the anon targets requested while analyzing the deps closure of the given targets.
///
/// Computes analysis for the matched targets and their configured deps closure, then walks
/// the anon targets those analyses requested (transitively, including anon targets
/// requested by other anon targets). An entry means "requested, and therefore analyzed,
/// during analysis of a requester"; whether its artifacts are consumed downstream is an
/// action-graph question, same as for any configured dep.
///
/// Relies on analysis recording the requested anon target keys, which is on by default;
/// errors with a hint if `buck2.record_requested_anon_targets` has been set to `false`.
#[derive(Debug, clap::Parser)]
pub struct AnonTargetsCommand {
    /// Patterns to interpret.
    #[clap(value_name = "TARGET_PATTERNS", required = true)]
    patterns: Vec<String>,

    /// Also print which analyses requested each anon target.
    #[clap(long)]
    with_requesters: bool,

    /// Print JSON instead of text. Implied by any of the attribute selection flags.
    #[clap(long)]
    json: bool,

    #[clap(flatten)]
    attributes: CommonAttributeArgs,

    #[clap(flatten)]
    target_cfg: TargetCfgOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for AnonTargetsCommand {
    const COMMAND_NAME: &'static str = "anon-targets";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = ctx.client_context(matches, &self)?;
        let response = buckd
            .with_flushing()
            .new_generic(
                context,
                NewGenericRequest::DebugAnonTargets(AnonTargetsRequest {
                    patterns: self.patterns,
                    target_cfg: self.target_cfg.target_cfg(),
                    with_requesters: self.with_requesters,
                    json: self.json,
                    output_attributes: self.attributes.get()?,
                }),
                events_ctx,
                ctx.console_interaction_stream(&self.common_opts.console_opts),
            )
            .await??;
        let NewGenericResponse::DebugAnonTargets(response) = response else {
            return ExitResult::err(internal_error!("Unexpected response type").into());
        };

        buck2_client_ctx::print!("{}", response.serialized)?;

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
