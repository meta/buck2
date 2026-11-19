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
use buck2_cli_proto::CqueryRequest;
use buck2_cli_proto::CqueryResponse;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgWithUniverseOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::StdoutPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_core::if_else_opensource;

use crate::commands::query::common::CommonQueryOptions;
use crate::commands::query::profile::QueryProfileOptions;

fn help() -> &'static str {
    concat!(
        r#"Perform queries on the configured target graph

The configured target graph includes information about the configuration
(platforms) and transitions involved in building targets. In the
configured graph, `selects` are fully resolved. The same target may
appear in multiple different configurations (when printed, the
configuration is after the target in parentheses).

A user can specify a `--target-universe` flag to control how literals
are resolved. When provided, any literals will resolve to all
matching targets within the universe (which includes the targets
passed as the universe and all transitive deps of them).  When not
provided, we implicitly set the universe to be rooted at every
target literal in the `cquery`.

Run `buck2 docs cquery` or
"#,
        if_else_opensource!(
            "https://buck2.build/docs/users/query/cquery/",
            "https://www.internalfb.com/intern/staticdocs/buck2/docs/users/query/cquery/",
        ),
        r#"
for more documentation about the functions available in cquery
expressions.

Examples:

Print all the attributes of a target

`buck2 cquery //java/com/example/app:amazing --output-all-attributes`

List the deps of a target (special characters in a target will
require quotes):

`buck2 cquery 'deps("//java/com/example/app:amazing+more")'`
"#
    )
}

#[derive(Debug, clap::Parser)]
#[clap(
    name = "cquery",
    about = "Perform queries on the configured target graph",
    long_about = help(),
    after_help = "Run `buck2 docs cquery` for detailed information about query functions such as allbuildfiles, allpaths, attrfilter, and more.",
    after_long_help = "",
    verbatim_doc_comment,
)]
pub struct CqueryCommand {
    #[clap(flatten)]
    query_common: CommonQueryOptions,

    #[clap(
        long,
        help = "Show the providers of the query result instead of the attributes and labels"
    )]
    show_providers: bool,

    #[clap(flatten)]
    target_cfg: TargetCfgWithUniverseOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,

    #[clap(flatten)]
    profile_options: QueryProfileOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for CqueryCommand {
    const COMMAND_NAME: &'static str = "cquery";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let (query, query_args) = self.query_common.get_query();
        let unstable_output_format = self.query_common.output_format() as i32;
        let output_attributes = self.query_common.attributes.get()?;
        let context = ctx.client_context(matches, &self)?;

        let CqueryResponse {} = buckd
            .with_flushing()
            .cquery(
                CqueryRequest {
                    query,
                    query_args,
                    context: Some(context),
                    output_attributes,
                    target_universe: self.target_cfg.target_universe,
                    target_cfg: Some(self.target_cfg.target_cfg.target_cfg()),
                    show_providers: self.show_providers,
                    unstable_output_format,
                    profile_mode: self.profile_options.profile_mode_proto().map(|m| m as i32),
                    profile_output: self
                        .profile_options
                        .profile_output
                        .as_ref()
                        .map(|p| buck2_error::Ok(p.resolve(&ctx.working_dir).to_str()?.to_owned()))
                        .transpose()?,
                },
                events_ctx,
                ctx.console_interaction_stream(&self.common_opts.console_opts),
                &mut StdoutPartialResultHandler,
            )
            .await??;

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
