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
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_common::legacy_configs::agent_context::AgentContextSchema;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;

/// Print repository-specific guidance for AI agents using Buck2.
///
/// Reads configuration locally without connecting to the daemon.
#[derive(Debug, clap::Parser)]
#[clap(name = "docs-agent")]
pub(crate) struct DocsAgentCommand {
    /// Override a buckconfig value when reading the agent guidance.
    #[clap(
        long = "config",
        short = 'c',
        value_name = "SECTION.OPTION=VALUE",
        num_args = 1
    )]
    config_values: Vec<String>,

    /// Read buckconfig overrides from a file.
    #[clap(long = "config-file", value_name = "PATH", num_args = 1)]
    config_files: Vec<String>,

    #[clap(flatten)]
    event_log_opts: CommonEventLogOptions,
}

impl BuckSubcommand for DocsAgentCommand {
    const COMMAND_NAME: &'static str = "docs agent";

    async fn exec_impl(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let config_opts = CommonBuildConfigurationOptions {
            config_values: self.config_values,
            config_files: self.config_files,
            ..Default::default()
        };
        let overrides =
            config_opts.config_overrides(matches, ctx.immediate_config, &ctx.working_dir)?;
        let project_root = ctx.paths()?.project_root();
        let cells = BuckConfigBasedCells::parse_with_config_args(project_root, &overrides).await?;
        // Cell discovery skips includes; the schema must include the complete root config.
        let root_config = cells
            .parse_single_cell(cells.cell_resolver.root_cell(), project_root)
            .await?;
        let schema = AgentContextSchema::from_config(&root_config);
        buck2_client_ctx::println!("{}", render_agent_context_schema(&schema).trim_end())?;
        ExitResult::success()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.event_log_opts
    }
}

fn render_agent_context_schema(schema: &AgentContextSchema) -> String {
    let mut output = vec![
        "For builds at Meta, use Buck2.".to_owned(),
        String::new(),
        "# Rules".to_owned(),
        String::new(),
        "ALWAYS pass `--agent-context direct_call=true` for every `buck2` or `buck` command you invoke directly.".to_owned(),
    ];
    if !schema.is_empty() {
        output.extend([
            String::new(),
            "Also provide `--agent-context key=value` entries following the repository-defined schema below. For multiple entries, repeat the flag: `--agent-context direct_call=true --agent-context key=value`.".to_owned(),
            String::new(),
            "# `--agent-context` schema".to_owned(),
            String::new(),
            schema.to_string(),
        ]);
    }
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_schema_requires_direct_call() {
        assert_eq!(
            "For builds at Meta, use Buck2.\n\n# Rules\n\nALWAYS pass `--agent-context direct_call=true` for every `buck2` or `buck` command you invoke directly.",
            render_agent_context_schema(&AgentContextSchema::default()),
        );
    }
}
