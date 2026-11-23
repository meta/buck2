/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_query::query::syntax::simple::functions::description::QUERY_ENVIRONMENT_DESCRIPTION_BY_TYPE;
use buck2_query::query::syntax::simple::functions::description::QueryType;
use buck2_query::query::syntax::simple::functions::docs::MarkdownOptions;
use buck2_query::query::syntax::simple::functions::docs::QueryEnvironmentDescription;
use dupe::Dupe;

#[derive(Debug, Clone, Dupe, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
enum OutputFormatArg {
    Markdown,
    Rendered,
}

#[derive(Debug, clap::Parser)]
pub(crate) struct OutputFormatOptions {
    /// How to format the documentation
    #[clap(
        long = "format",
        default_value = "rendered",
        value_enum,
        ignore_case = true
    )]
    format: OutputFormatArg,
}

impl OutputFormatOptions {
    pub(crate) fn emit_markdown(&self, markdown: &str) -> buck2_error::Result<()> {
        match self.format {
            OutputFormatArg::Markdown => {
                buck2_client_ctx::println!("{}", markdown)?;
            }
            OutputFormatArg::Rendered => {
                let skin = termimad::MadSkin::default();
                let area = termimad::Area::full_screen();
                let width = std::cmp::min(100, area.width) as usize;
                let rendered = skin.text(markdown, Some(width));
                buck2_client_ctx::println!("{}", rendered)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
#[clap(name = "docs-uquery", about = "Print documentation for query/uquery")]
pub(crate) struct DocsUqueryCommand {
    #[clap(flatten)]
    docs_options: OutputFormatOptions,
}

#[derive(Debug, clap::Parser)]
#[clap(name = "docs-cquery", about = "Print documentation for cquery")]
pub(crate) struct DocsCqueryCommand {
    #[clap(flatten)]
    docs_options: OutputFormatOptions,
}

#[derive(Debug, clap::Parser)]
#[clap(name = "docs-aquery", about = "Print documentation for aquery")]
pub(crate) struct DocsAqueryCommand {
    #[clap(flatten)]
    docs_options: OutputFormatOptions,
}

fn output(options: OutputFormatOptions, description: QueryEnvironmentDescription) -> ExitResult {
    let markdown = description.render_markdown(&MarkdownOptions {
        links_enabled: match options.format {
            OutputFormatArg::Rendered => false,
            OutputFormatArg::Markdown => true,
        },
    });
    options.emit_markdown(&markdown)?;
    ExitResult::success()
}

impl DocsUqueryCommand {
    pub(crate) fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
    ) -> ExitResult {
        let description = (QUERY_ENVIRONMENT_DESCRIPTION_BY_TYPE.get()?)(QueryType::Uquery);
        output(self.docs_options, description)
    }
}

impl DocsCqueryCommand {
    pub(crate) fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
    ) -> ExitResult {
        let description = (QUERY_ENVIRONMENT_DESCRIPTION_BY_TYPE.get()?)(QueryType::Cquery);
        output(self.docs_options, description)
    }
}

impl DocsAqueryCommand {
    pub(crate) fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
    ) -> ExitResult {
        let description = (QUERY_ENVIRONMENT_DESCRIPTION_BY_TYPE.get()?)(QueryType::Aquery);
        output(self.docs_options, description)
    }
}
