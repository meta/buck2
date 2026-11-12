/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_log_common::chrome_trace::ChromeTraceCommand;
use dupe::Dupe;

mod critical_path;
mod diff;
mod external_configs;
pub(crate) mod path_log;
mod replay;
mod shed;
mod show_log;
mod show_user_log;
mod snoop;
mod summary;
#[cfg(fbcode_build)]
mod trailcam;
mod what_cmd;
mod what_failed;
mod what_materialized;
pub(crate) mod what_ran;
mod what_up;
mod what_uploaded;

/// Output format options for log commands.
///
/// Determines how the command output is formatted and displayed.
#[derive(
    Debug,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Dupe,
    clap::ValueEnum
)]
#[clap(rename_all = "snake_case")]
pub(crate) enum LogCommandOutputFormatOptions {
    /// Human-readable output (default).
    Readable,
    /// Tab-delimited output. Deprecated in favor of `readable`.
    Tabulated,
    /// JSON format, one object per line.
    Json,
    /// Comma-separated values (CSV) format.
    Csv,
}

/// Clap parser for output format command-line option.
///
/// This struct can be flattened into command structs to provide a consistent
/// `--format` flag across all log commands.
#[derive(Debug, Clone, clap::Parser)]
pub(crate) struct LogCommandOutputFormat {
    #[clap(
        long,
        help = "Which output format to use for this command",
        default_value = "readable",
        ignore_case = true,
        value_enum
    )]
    format: LogCommandOutputFormatOptions,
}

/// Writer wrapper that combines the output format with a writer.
///
/// This enum pairs each output format option with its corresponding writer,
/// allowing commands to write output in the selected format.
pub(crate) enum LogCommandOutputFormatWithWriter<'a> {
    /// Human-readable output writer.
    Readable(&'a mut dyn std::io::Write),
    /// Tab-delimited output writer.
    Tabulated(&'a mut dyn std::io::Write),
    /// JSON output writer.
    Json(&'a mut dyn std::io::Write),
    /// CSV output writer.
    Csv(Box<csv::Writer<&'a mut dyn std::io::Write>>),
}

pub(crate) struct OutputFormatWithWriter<'a> {
    pub(crate) format: LogCommandOutputFormatWithWriter<'a>,
    pub(crate) include_std_err: bool,
    pub(crate) omit_empty_std_err: bool,
}

pub(crate) fn transform_format(
    format: LogCommandOutputFormat,
    w: &mut dyn std::io::Write,
) -> LogCommandOutputFormatWithWriter<'_> {
    match format.format {
        LogCommandOutputFormatOptions::Readable => LogCommandOutputFormatWithWriter::Readable(w),
        LogCommandOutputFormatOptions::Tabulated => LogCommandOutputFormatWithWriter::Tabulated(w),
        LogCommandOutputFormatOptions::Json => LogCommandOutputFormatWithWriter::Json(w),
        LogCommandOutputFormatOptions::Csv => LogCommandOutputFormatWithWriter::Csv(Box::new(
            csv::WriterBuilder::new().from_writer(w),
        )),
    }
}

#[derive(Debug, clap::Subcommand)]
#[clap(about = "Commands for interacting with buck2 logs")]
pub enum LogCommand {
    #[clap(alias = "whatran")]
    WhatRan(what_ran::WhatRanCommand),
    #[clap(alias = "whatfailed")]
    WhatFailed(what_failed::WhatFailedCommand),
    #[clap(alias = "last")]
    Path(path_log::PathLogCommand),
    Show(show_log::ShowLogCommand),
    #[clap(alias = "whatcmd", alias = "what-cmd")]
    Cmd(what_cmd::WhatCmdCommand),
    #[clap(alias = "whatup")]
    WhatUp(what_up::WhatUpCommand),
    WhatMaterialized(what_materialized::WhatMaterializedCommand),
    WhatUploaded(what_uploaded::WhatUploadedCommand),
    CriticalPath(critical_path::CriticalPathCommand),
    SlowestPath(critical_path::SlowestPathCommand),
    Replay(replay::ReplayCommand),
    Snoop(snoop::SnoopCommand),
    ShowUser(show_user_log::ShowUserLogCommand),
    Summary(summary::SummaryCommand),
    #[clap(subcommand)]
    Diff(diff::DiffCommand),
    ExternalConfigs(external_configs::ExternalConfigsCommand),
    ChromeTrace(ChromeTraceCommand),
    #[clap(subcommand, hide = true)]
    Shed(shed::ShedCommand),
    #[cfg(fbcode_build)]
    Trailcam(trailcam::TrailcamCommand),
}

impl LogCommand {
    pub fn connects_to_daemon(&self) -> bool {
        matches!(self, Self::Snoop(_))
    }

    pub fn exec(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        match self {
            Self::WhatRan(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::WhatFailed(cmd) => cmd.exec(matches, ctx, events_ctx),
            Self::Path(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Show(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Cmd(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::WhatUp(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::WhatMaterialized(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::WhatUploaded(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::CriticalPath(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::SlowestPath(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Replay(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Snoop(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::ShowUser(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Summary(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Diff(cmd) => cmd.exec(matches, ctx, events_ctx),
            Self::ExternalConfigs(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::ChromeTrace(cmd) => ctx.exec(cmd, matches, events_ctx),
            Self::Shed(cmd) => cmd.exec(matches, ctx, events_ctx),
            #[cfg(fbcode_build)]
            Self::Trailcam(cmd) => ctx.exec(cmd, matches, events_ctx),
        }
    }

    pub fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }

    pub fn command_name(&self) -> &'static str {
        match self {
            Self::WhatRan(cmd) => cmd.logging_name(),
            Self::WhatFailed(_) => "log-what-failed",
            Self::Path(cmd) => cmd.logging_name(),
            Self::Show(cmd) => cmd.logging_name(),
            Self::Cmd(cmd) => cmd.logging_name(),
            Self::WhatUp(cmd) => cmd.logging_name(),
            Self::WhatMaterialized(cmd) => cmd.logging_name(),
            Self::WhatUploaded(cmd) => cmd.logging_name(),
            Self::CriticalPath(cmd) => cmd.logging_name(),
            Self::SlowestPath(cmd) => cmd.logging_name(),
            Self::Replay(cmd) => cmd.logging_name(),
            Self::Snoop(cmd) => cmd.logging_name(),
            Self::ShowUser(cmd) => cmd.logging_name(),
            Self::Summary(cmd) => cmd.logging_name(),
            Self::Diff(_) => "log-diff",
            Self::ExternalConfigs(cmd) => cmd.logging_name(),
            Self::ChromeTrace(cmd) => cmd.logging_name(),
            Self::Shed(_) => "log-shed",
            #[cfg(fbcode_build)]
            Self::Trailcam(cmd) => cmd.logging_name(),
        }
    }
}
