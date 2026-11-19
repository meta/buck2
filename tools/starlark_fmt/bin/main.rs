/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod subcommands;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use clap::Subcommand;
#[cfg(fbcode_build)]
use lint_message::LintSeverity;
use starlark_fmt_lib::Config;
use starlark_map::small_set::SmallSet;
use tracing::error;
use tracing::info;
use tracing::info_span;
use tracing_subscriber::fmt::format::FmtSpan;

use crate::subcommands::diff_file;
use crate::subcommands::format_files;
use crate::subcommands::format_stdin;
#[cfg(fbcode_build)]
use crate::subcommands::lint_files;

#[derive(Debug, Parser)]
#[clap(name = "starlark_fmt")]
#[clap(bin_name = "starlark_fmt")]
#[clap(about = "An Opinionated Starlark Formatter", long_about = None)]
#[clap(color = clap::ColorChoice::Never)]
#[clap(max_term_width = 80)]
struct Args {
    /// Path to the JSON configuration file.
    #[clap(long, required = true, value_name = "CONFIG_PATH")]
    config: PathBuf,

    /// Show timing breakdown for each phase.
    #[clap(long)]
    timing: bool,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Format files in-place.
    Fmt {
        /// File paths to format in-place (supports @file for reading paths from a file).
        #[clap(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,
    },
    /// Lint files for issues.
    #[cfg(fbcode_build)]
    Lint {
        /// File paths to lint (supports @file for reading paths from a file).
        #[clap(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Severity mode for linting.
        #[clap(long, default_value = "warning")]
        severity: LintSeverity,
    },
    /// Show diff between original and formatted file.
    Diff {
        /// File path to diff.
        #[clap(required = true, value_name = "FILE")]
        file: PathBuf,

        /// Show all lines including unchanged context.
        #[clap(long)]
        show_all: bool,
    },
    /// Format content from stdin and write to stdout.
    Stdin {
        /// File path used for config matching (content is read from stdin).
        #[clap(long, required = true, value_name = "FILE")]
        path: PathBuf,
    },
}

fn try_main(args: Args) -> anyhow::Result<()> {
    let config = info_span!("load_config").in_scope(|| Config::from_path(&args.config))?;

    match args.command {
        Command::Fmt { files } => {
            let files = deduplicated_files(files);
            if args.timing {
                info!("Formatting {} file(s)", files.len());
            }
            format_files(&files, &config)
        }
        #[cfg(fbcode_build)]
        Command::Lint { files, severity } => {
            let files = deduplicated_files(files);
            if args.timing {
                info!("Linting {} file(s)", files.len());
            }
            lint_files(&files, severity, &config)
        }
        Command::Diff { file, show_all } => diff_file(&file, show_all, &config),
        Command::Stdin { path } => format_stdin(&path, &config),
    }
}

fn deduplicated_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    info_span!("dedupping_files").in_scope(|| {
        files
            .into_iter()
            // SmallSet keeps the iteration order which makes the output deterministic.
            .collect::<SmallSet<_>>()
            .into_iter()
            .collect()
    })
}

fn main() {
    let raw_args = argfile::expand_args(argfile::parse_fromfile, argfile::PREFIX)
        .context("could not parse arguments")
        .unwrap_or_else(|err| {
            error!("{:#}", err);
            std::process::exit(1);
        });
    let args = Args::parse_from(raw_args);

    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false);

    if args.timing {
        subscriber
            .with_span_events(FmtSpan::CLOSE)
            .with_timer(tracing_subscriber::fmt::time::uptime())
            .init();
    } else {
        subscriber.without_time().init();
    }

    if let Err(err) = try_main(args) {
        error!("{:#}", err);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deduplicated_files_preserves_first_occurrence_order() {
        let args = Args::try_parse_from([
            "starlark_fmt",
            "--config",
            "config.json",
            "fmt",
            "a.bzl",
            "b.bzl",
            "a.bzl",
        ])
        .expect("args should parse");

        let Command::Fmt { files } = args.command else {
            panic!("expected fmt command");
        };

        let files = deduplicated_files(files);
        assert_eq!(files.len(), 2, "duplicate input paths should be removed");
        assert_eq!(
            files,
            vec![PathBuf::from("a.bzl"), PathBuf::from("b.bzl")],
            "file order should follow first occurrence order"
        );
    }
}
