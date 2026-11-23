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
use buck2_settings::docs::render_markdown;

use crate::query::OutputFormatOptions;

/// Print every Buck setting (`.bucksettings.toml`, `--setting`), with its type, default and
/// description.
///
/// `buck2 audit settings` shows the values in effect.
#[derive(Debug, clap::Parser)]
#[clap(name = "docs-settings")]
pub(crate) struct DocsSettingsCommand {
    #[clap(flatten)]
    docs_options: OutputFormatOptions,
}

impl DocsSettingsCommand {
    pub(crate) fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
    ) -> ExitResult {
        self.docs_options.emit_markdown(&render_markdown())?;
        ExitResult::success()
    }
}
