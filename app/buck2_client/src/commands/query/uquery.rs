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
use buck2_cli_proto::UqueryRequest;
use buck2_cli_proto::UqueryResponse;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgUnusedOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::StdoutPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_core::if_else_opensource;

use crate::commands::query::common::CommonQueryOptions;

fn help() -> &'static str {
    concat!(
        "Perform queries on the unconfigured target graph

The unconfigured target graph consists of the targets as they are
defined in the build files. In this graph, each target appears
exactly once and `select()`s are in the unresolved form. For large
queries, the unconfigured graph may be much smaller than the
configured graph and queries can be much more efficiently performed
there.

When querying the unconfigured graph, dependencies appearing in all
branches of `select()` dictionaries will be treated as dependencies.

Run `buck2 docs uquery` or
",
        if_else_opensource!(
            "https://buck2.build/docs/users/query/uquery/",
            "https://www.internalfb.com/intern/staticdocs/buck2/docs/users/query/uquery/",
        ),
        r#"
for more documentation about the functions available in uquery
expressions.

Examples:

Print all the attributes of a target

`buck2 uquery //java/com/example/app:amazing --output-all-attributes

List the deps of a target (special characters in a target will require quotes):
`buck2 uquery 'deps("//java/com/example/app:amazing+more")'`

select() encoding:

When printed, values with `select()`s use a special json encoding.

`1 + select({"//:a": 1, "DEFAULT": 2})` will be encoded as:

`{"__type": "concat", "items": [1, {"__type": "selector", "entries": {"//:a": 1, "DEFAULT": 2}}]}`
"#
    )
}

#[derive(Debug, clap::Parser)]
#[clap(
    name = "uquery",
    about = "Perform queries on the unconfigured target graph",
    long_about = help(),
    after_help = "Run `buck2 docs uquery` for detailed information about query functions such as allbuildfiles, allpaths, attrfilter, and more.",
    after_long_help = "",
    verbatim_doc_comment,
)]
pub struct UqueryCommand {
    #[clap(flatten)]
    query_common: CommonQueryOptions,

    /// Allows querying the best-effort partial graph instead of aborting on the
    /// first error by skipping nodes that fail to load and edges that point to
    /// them. This only applies to the open-ended parts of the graph, e.g. an
    /// `rdeps(//foo/...` pattern or traversal between two explicit points. An
    /// explicit target literal given must resolve, e.g.
    /// `rdeps(//foo:bar, //baz:lib)` will still fail if `//foo:bar` or
    /// `//baz:lib` is missing or broken.
    #[clap(long)]
    allow_partial_graph: bool,

    /// Uquery doesn't need these flags, but they are used in mode files, so we need to keep them.
    #[clap(flatten)]
    _target_cfg: TargetCfgUnusedOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for UqueryCommand {
    // FIXME: Figure out if we can replace this. We used to log this way in Ingress :/
    const COMMAND_NAME: &'static str = "query";

    async fn exec_impl(
        mut self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let (query, query_args) = self.query_common.get_query();
        let unstable_output_format = self.query_common.output_format() as i32;
        let output_attributes = self.query_common.attributes.get()?;
        let context = ctx.client_context(matches, &self)?;

        let UqueryResponse {} = buckd
            .with_flushing()
            .uquery(
                UqueryRequest {
                    query,
                    query_args,
                    context: Some(context),
                    output_attributes,
                    unstable_output_format,
                    allow_partial_graph: self.allow_partial_graph,
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
