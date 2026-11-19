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
use buck2_cli_proto::ConfiguredTargetsRequest;
use buck2_cli_proto::ConfiguredTargetsResponse;
use buck2_cli_proto::configured_targets_request::OutputFormat;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::query_args::CommonAttributeArgs;
use buck2_client_ctx::streaming::StreamingCommand;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum TargetHashFunction {
    Fast,
    Strong,
}

/// Resolve target patterns to configured targets.
#[derive(Debug, clap::Parser)]
#[clap(name = "ctargets")]
pub struct ConfiguredTargetsCommand {
    /// Print targets as JSON
    #[clap(long)]
    json: bool,

    /// Print compatible targets and valid incompatible targets as JSON
    #[clap(long, conflicts_with = "json")]
    json_report: bool,

    /// Skip missing targets from `BUCK` files when non-glob pattern is specified.
    /// This option does not skip missing packages
    /// and does not ignore errors of `BUCK` file evaluation.
    #[clap(long)]
    skip_missing_targets: bool,

    /// On errors, put buck.error in the output stream and continue
    #[clap(long)]
    keep_going: bool,

    /// Print a stable configured-target hash that omits configuration information.
    #[clap(long)]
    show_target_hash: bool,

    /// Include all transitive dependencies in each target hash.
    #[clap(long, requires = "show_target_hash")]
    target_hash_recursive: bool,

    /// Change hashes for these targets and targets that directly depend on them.
    /// With --target-hash-recursive, this also affects transitive dependents.
    /// Takes space-separated unconfigured target labels and can be repeated.
    #[clap(long, requires = "show_target_hash", num_args = 1.., value_name = "TARGET")]
    require_hash_change_deps: Vec<String>,

    /// Select the target hash function.
    #[clap(
        long,
        ignore_case = true,
        default_value = "fast",
        requires = "show_target_hash",
        value_enum
    )]
    target_hash_function: TargetHashFunction,

    #[clap(flatten)]
    attributes: CommonAttributeArgs,

    /// Patterns to interpret.
    #[clap(name = "TARGET_PATTERNS", value_hint = clap::ValueHint::Other)]
    patterns: Vec<String>,

    #[clap(flatten)]
    target_cfg: TargetCfgOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

impl ConfiguredTargetsCommand {
    fn output_format(&self) -> buck2_error::Result<OutputFormat> {
        if self.json_report {
            Ok(OutputFormat::JsonReport)
        } else if self.json || !self.attributes.get()?.is_empty() {
            Ok(OutputFormat::Json)
        } else {
            Ok(OutputFormat::Text)
        }
    }
}

#[async_trait(?Send)]
impl StreamingCommand for ConfiguredTargetsCommand {
    const COMMAND_NAME: &'static str = "ctargets";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = Some(ctx.client_context(matches, &self)?);
        let output_format = self.output_format()?;
        let ConfiguredTargetsResponse {
            serialized_targets_output,
        } = buckd
            .with_flushing()
            .ctargets(
                ConfiguredTargetsRequest {
                    context,
                    target_patterns: self.patterns,
                    target_cfg: Some(self.target_cfg.target_cfg()),
                    skip_missing_targets: self.skip_missing_targets,
                    output_format: output_format as i32,
                    output_attributes: self.attributes.get()?,
                    keep_going: self.keep_going,
                    show_target_hash: self.show_target_hash,
                    target_hash_recursive: self.target_hash_recursive,
                    require_hash_change_deps: self.require_hash_change_deps,
                    target_hash_use_strong_hash: matches!(
                        self.target_hash_function,
                        TargetHashFunction::Strong
                    ),
                },
                events_ctx,
                ctx.console_interaction_stream(&self.common_opts.console_opts),
                &mut NoPartialResultHandler,
            )
            .await??;

        buck2_client_ctx::print!("{}", serialized_targets_output)?;

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
