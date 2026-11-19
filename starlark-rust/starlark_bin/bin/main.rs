/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// Disagree these are good hints
#![allow(clippy::type_complexity)]
#![allow(clippy::manual_map)]

use std::ffi::OsStr;
use std::fmt;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use clap::ValueEnum;
use clap::builder::StringValueParser;
use clap::builder::TypedValueParser;
use dupe::Dupe;
use eval::Context;
use itertools::Either;
use starlark::analysis::LintMessage;
use starlark::docs::DocItem;
use starlark::docs::markdown::render_doc_item_no_link;
use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::errors::EvalMessage;
use starlark::errors::EvalSeverity;
use starlark::read_line::ReadLine;
use starlark::syntax::Dialect;
use suppression::GlobLintSuppression;
use walkdir::WalkDir;

use crate::eval::ContextMode;

mod bazel;
mod dap;
mod eval;
mod suppression;

#[derive(Debug, Parser)]
#[command(name = "starlark", about = "Evaluate Starlark code", version)]
struct Args {
    #[arg(
        long = "lsp",
        help = "Start an LSP server.",
        conflicts_with_all = &[
            "dap",
            "check",
            "json",
            "docs",
            "evaluate",
            "files",
        ],
    )]
    lsp: bool,

    #[arg(
        long = "dap",
        help = "Start a DAP server.",
        // Conflicts with all options.
        conflicts_with_all = &[
            "lsp",
            "check",
            "json",
            "docs",
            "extension",
            "prelude",
            "evaluate",
            "files",
        ],
    )]
    dap: bool,

    #[arg(
        long = "check",
        help = "Run checks and lints.",
        conflicts_with_all = &["lsp", "dap"],
    )]
    check: bool,

    #[arg(
        long = "json",
        help = "Show output as JSON lines.",
        conflicts_with_all = &["lsp", "dap"],
    )]
    json: bool,

    #[arg(
        long = "docs",
        help = "Generate documentation output.",
        conflicts_with_all = &["lsp", "dap"],
    )]
    docs: Option<ArgsDoc>,

    #[arg(
        long = "extension",
        help = "File extension when searching directories."
    )]
    extension: Option<String>,

    #[arg(long = "prelude", help = "Files to load in advance.", num_args = 1..)]
    prelude: Vec<PathBuf>,

    #[arg(
        long = "expression",
        short = 'e',
        id = "evaluate",
        value_name = "EXPRESSION",
        help = "Expressions to evaluate.",
        conflicts_with_all = &["lsp", "dap"],
        num_args = 1..,
    )]
    evaluate: Vec<String>,

    #[arg(
        long = "dialect",
        help = "Dialect to use for features and globals.",
        default_value = "extended"
    )]
    dialect: ArgsDialect,

    #[arg(
        id = "files",
        value_name = "FILE",
        help = "Files to evaluate.",
        conflicts_with_all = &["lsp", "dap"],
    )]
    files: Vec<PathBuf>,

    #[arg(
        long = "bazel",
        help = "Run in Bazel mode (temporary, will be removed)"
    )]
    bazel: bool,

    #[arg(
        long = "suppression",
        help = "Specify lint rules to suppress. You may specify an optional glob pattern to \
suppress rules for files matching the pattern, in the format of `[<glob>:]<rule>[,<rule>]*`.",
        requires = "check",
        value_parser = StringValueParser::new().try_map(GlobLintSuppression::try_parse)
    )]
    suppression: Vec<GlobLintSuppression>,
}

#[derive(ValueEnum, Copy, Clone, Dupe, Debug, PartialEq, Eq)]
enum ArgsDoc {
    Lsp,
    Markdown,
    Code,
}

#[derive(ValueEnum, Copy, Clone, Dupe, Debug, PartialEq, Eq)]
enum ArgsDialect {
    Standard,
    Extended,
}

// Treat directories as things to recursively walk for .<extension> files,
// and everything else as normal files.
fn expand_dirs(extension: &str, xs: Vec<PathBuf>) -> impl Iterator<Item = PathBuf> + use<> {
    let extension = Arc::new(extension.to_owned());
    xs.into_iter().flat_map(move |x| {
        // Have to keep cloning extension so we keep ownership
        let extension = extension.dupe();
        if x.is_dir() {
            Either::Left(
                WalkDir::new(x)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(move |e| e.path().extension() == Some(OsStr::new(extension.as_str())))
                    .map(|e| e.into_path()),
            )
        } else {
            Either::Right(Box::new(vec![x].into_iter()))
        }
    })
}

#[derive(Default)]
struct Stats {
    file: usize,
    error: usize,
    warning: usize,
    advice: usize,
    disabled: usize,
}

impl Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!(
            "{} files, {} errors, {} warnings, {} advices, {} disabled",
            self.file, self.error, self.warning, self.advice, self.disabled
        ))
    }
}

impl Stats {
    fn increment_file(&mut self) {
        self.file += 1;
    }

    fn increment(&mut self, x: EvalSeverity) {
        match x {
            EvalSeverity::Error => self.error += 1,
            EvalSeverity::Warning => self.warning += 1,
            EvalSeverity::Advice => self.advice += 1,
            EvalSeverity::Disabled => self.disabled += 1,
        }
    }
}

fn drain(
    xs: impl Iterator<Item = EvalMessage>,
    json: bool,
    stats: &mut Stats,
) -> anyhow::Result<()> {
    for x in xs {
        stats.increment(x.severity);
        if json {
            println!(
                "{}",
                serde_json::to_string(&LintMessage::new(x))
                    .map_err(|e| anyhow::anyhow!("Failed to serialize lint to JSON: {e}"))?
            );
        } else if let Some(error) = x.full_error_with_span {
            let mut error = error.to_owned();
            if !error.is_empty() && !error.ends_with('\n') {
                error.push('\n');
            }
            print!("{error}");
        } else {
            println!("{x}");
        }
    }
    Ok(())
}

fn interactive(ctx: &Context) -> anyhow::Result<()> {
    let mut rl = ReadLine::new("STARLARK_RUST_HISTFILE")?;
    loop {
        match rl.read_line("$> ")? {
            Some(line) => {
                let mut stats = Stats::default();
                drain(ctx.expression(line).messages, false, &mut stats)?;
            }
            // User pressed EOF - disconnected terminal, or similar
            None => return Ok(()),
        }
    }
}

/// starlark-rust does not support panic.
/// Terminate on panic even if compiled without `-Cpanic=abort`.
fn terminate_on_panic() {
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        std::process::exit(1);
    }));
}

fn main() -> anyhow::Result<()> {
    terminate_on_panic();

    let args = argfile::expand_args(argfile::parse_fromfile, argfile::PREFIX)?;
    let args: Args = Args::parse_from(args);

    let (dialect, globals) = match args.dialect {
        ArgsDialect::Standard => (Dialect::Standard, Globals::standard()),
        ArgsDialect::Extended => (Dialect::Extended, Globals::extended_internal()),
    };

    if args.dap {
        dap::server(dialect, globals);
    } else {
        let is_interactive = args.evaluate.is_empty() && args.files.is_empty();

        let ext = args
            .extension
            .as_ref()
            .map_or("bzl", |x| x.strip_prefix('.').unwrap_or(x.as_str()));
        let prelude = expand_dirs(ext, args.prelude).collect::<Vec<_>>();
        let print_non_none = !args.evaluate.is_empty() || is_interactive;

        // TODO: Remove this when extracting the Bazel binary to its own
        // repository, after the LspContext interface stabilizes.
        if args.bazel {
            Module::with_temp_heap(|module| {
                let module = if is_interactive { Some(module) } else { None };
                bazel::main(args.lsp, print_non_none, module, &prelude, dialect, globals)
            })?;
            return Ok(());
        }

        Module::with_temp_heap(|module| {
            let module = if is_interactive { Some(module) } else { None };
            let mut ctx = Context::new(
                if args.check {
                    ContextMode::Check
                } else {
                    ContextMode::Run
                },
                print_non_none,
                &prelude,
                module,
                dialect,
                globals,
                args.suppression,
            )?;

            if args.lsp {
                ctx.mode = ContextMode::Check;
                starlark_lsp::server::stdio_server(ctx)?;
            } else if let Some(docs) = args.docs {
                let global_module = DocItem::Module(Globals::extended_internal().documentation());

                match docs {
                    ArgsDoc::Markdown | ArgsDoc::Lsp => {
                        println!(
                            "{}",
                            if docs == ArgsDoc::Markdown {
                                render_doc_item_no_link("globals", &global_module)
                            } else {
                                String::new()
                            }
                        )
                    }
                    ArgsDoc::Code => println!("{}", global_module.render_as_code("globals")),
                };
            } else if is_interactive {
                interactive(&ctx)?;
            } else {
                let mut stats = Stats::default();
                for e in args.evaluate.clone() {
                    stats.increment_file();
                    drain(ctx.expression(e).messages, args.json, &mut stats)?;
                }

                for file in expand_dirs(ext, args.files.clone()) {
                    stats.increment_file();
                    drain(ctx.file(&file).messages, args.json, &mut stats)?;
                }

                if !args.json {
                    println!("{stats}");
                    if stats.error > 0 {
                        return Err(anyhow::anyhow!("Failed with {} errors", stats.error));
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}
