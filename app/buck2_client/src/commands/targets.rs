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
use buck2_cli_proto::TargetsRequest;
use buck2_cli_proto::targets_request;
use buck2_cli_proto::targets_request::OutputFormat;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::PrintOutputsFormat;
use buck2_client_ctx::common::build::CommonOutputOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::console_interaction_stream::ConsoleInteractionStream;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::daemon::client::StdoutPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ClientIoError;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::query_args::CommonAttributeArgs;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use dupe::Dupe;

use crate::print::PrintOutputs;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum TargetsError {
    /// Clap should report it, but if we missed something, this is a fallback.
    #[error("Flags are mutually exclusive")]
    IncompatibleArguments,
}

// Use non-camel case so the possible values match buck1's
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Dupe, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
enum TargetHashFileMode {
    PathsOnly,
    PathsAndContents,
    None,
}

impl TargetHashFileMode {
    fn to_proto(&self) -> targets_request::TargetHashFileMode {
        match self {
            TargetHashFileMode::PathsOnly => targets_request::TargetHashFileMode::PathsOnly,
            TargetHashFileMode::PathsAndContents => {
                targets_request::TargetHashFileMode::PathsAndContents
            }
            TargetHashFileMode::None => targets_request::TargetHashFileMode::NoFiles,
        }
    }
}

#[derive(Debug, clap::ValueEnum, Clone, Dupe)]
enum TargetHashGraphType {
    None,
    Unconfigured,
    Configured,
}

// Use non-camel case so the possible values match buck1's
/// Possible values for the --target-hash-function arg. We don't actually
/// honor the specific algorithms, we use them as a hint to pick "fast" or "strong".
#[allow(non_camel_case_types)]
#[derive(Debug, clap::ValueEnum, Clone, Dupe)]
enum TargetHashFunction {
    Sha1,
    Sha256,
    Murmur_Hash3,
    Fast,
    Strong,
}

#[derive(Debug, clap::ValueEnum, Clone, Dupe)]
enum Compression {
    None,
    Gzip,
    Zstd,
}

impl Compression {
    fn to_proto(&self) -> buck2_cli_proto::targets_request::Compression {
        match self {
            Compression::None => buck2_cli_proto::targets_request::Compression::Uncompressed,
            Compression::Gzip => buck2_cli_proto::targets_request::Compression::Gzip,
            Compression::Zstd => buck2_cli_proto::targets_request::Compression::Zstd,
        }
    }
}

/// Show details about the specified targets.
///
/// This command is meant to only handle unconfigured targets,
/// but for historical reasons, with certain flags it can also work with configured targets.
#[derive(Debug, clap::Parser)]
#[clap(name = "utargets")]
pub struct TargetsCommand {
    /// Print targets as JSON
    #[clap(long)]
    json: bool,

    /// Print targets as JSON-lines
    #[clap(long, conflicts_with = "json")]
    json_lines: bool,

    /// Print statistics of how many entries were processed
    #[clap(long)]
    stats: bool,

    /// Print the fully-qualified build target for the specified aliases
    #[clap(long, alias = "resolvealias")]
    resolve_alias: bool,

    /// Print a stable hash of each target after the target name.
    #[clap(long, conflicts_with = "streaming")]
    show_target_hash: bool,

    /// Print a stable unconfigured hash of each target after the target name.
    #[clap(long, conflicts_with = "show_target_hash")]
    show_unconfigured_target_hash: bool,

    /// Modifies computation of target hashes. If set to `PATHS_AND_CONTENTS` (the default), the contents
    /// of all files referenced from the targets will be used to compute the target hash. If set to
    /// `PATHS_ONLY`, only files' paths contribute to the hash. If set to `NONE` no files will be used.
    /// See also --target-hash-modified-paths.
    #[clap(
        long,
        value_enum,
        ignore_case = true,
        default_value = "paths_and_contents",
        conflicts_with = "streaming"
    )]
    target_hash_file_mode: TargetHashFileMode,

    /// Modifies computation of target hashes. Only effective when --target-hash-file-mode is set to
    /// `PATHS_ONLY`. If a target or its dependencies reference a file from this set, the target's hash
    /// will be different than if this option was omitted. Otherwise, the target's hash will be the same
    /// as if this option was omitted.
    #[clap(long, num_args=1.., conflicts_with = "streaming")]
    target_hash_modified_paths: Vec<PathArg>,

    /// Selects either the "fast" or the "strong" target hash function to be used for computing target hashes.
    /// While we don't specify the exact algorithm, the "strong" algorithm should be a reasonable cryptographic
    /// hash (ex. blake3) while the "fast" function will likely be a non-crypto hash. Both functions are
    /// guaranteed to be deterministic and to have the same value across different platforms/architectures.
    #[clap(long, ignore_case = true, default_value = "fast", value_enum)]
    target_hash_function: TargetHashFunction,

    /// When true, emit the hash or target node and all dependencies recursively.
    /// When false, hash only the target node.
    #[clap(long, action = clap::ArgAction::Set, default_value = "false", conflicts_with = "streaming")]
    target_hash_recursive: bool,

    #[clap(flatten)]
    attributes: CommonAttributeArgs,

    /// Enables printing of default attributes. This would be attributes in a target that aren't
    /// explicitly set in the target but instead use the default set in the rule declaration.
    #[clap(long)]
    include_defaults: bool,

    #[clap(flatten)]
    show_output: CommonOutputOptions,

    /// On loading errors, put buck.error in the output stream and continue
    #[clap(long)]
    keep_going: bool,

    /// Write output as soon as it is available. The order of the output items
    /// is non-deterministic and if multiple patterns cover the same target, may
    /// have duplicates.
    #[clap(long)]
    streaming: bool,

    /// Don't cache the target information on the build graph
    #[clap(long, requires = "streaming")]
    no_cache: bool,

    /// Show the imports of each package/import. Shows an additional output per package/import
    /// (not per target), including implicit dependencies (e.g. the prelude) but only direct
    /// dependencies (not the transitive closure).
    #[clap(long, requires = "streaming")]
    imports: bool,

    /// Show the package values. Produces an additional attribute representing all the package values
    /// for the package containing the target.
    #[clap(long, conflicts_with = "package_values_regex")]
    package_values: bool,

    /// Regular expressions to match package values. Produces an additional attribute representing package values
    /// for the package containing the target. Regular expressions are used in "search" mode so,
    /// for example, empty string matches all package values.
    #[clap(long, value_name = "VALUES", conflicts_with = "package_values")]
    package_values_regex: Vec<String>,

    /// File to put the output in, rather than sending to stdout.
    ///
    /// File will be created if it does not exist, and overwritten if it does.
    #[clap(long, short = 'o', value_name = "PATH")]
    output: Option<PathArg>,

    /// Compress the output.
    #[clap(
        long,
        default_value = "none",
        value_name = "SCHEME",
        requires = "output"
    )]
    compression: Compression,

    /// Patterns to interpret
    #[clap(name = "TARGET_PATTERNS", value_hint = clap::ValueHint::Other)]
    patterns: Vec<String>,

    /// Number of threads to use during execution (default is # cores)
    #[clap(short = 'j', long = "num-threads", value_name = "THREADS")]
    pub num_threads: Option<u32>,

    #[clap(flatten)]
    target_cfg: TargetCfgOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

impl TargetsCommand {
    #[allow(clippy::if_same_then_else)]
    fn output_format(&self) -> buck2_error::Result<OutputFormat> {
        if self.json {
            if self.json_lines || self.stats {
                return Err(TargetsError::IncompatibleArguments.into());
            }
            Ok(OutputFormat::Json)
        } else if self.json_lines {
            if self.json || self.stats {
                return Err(TargetsError::IncompatibleArguments.into());
            }
            Ok(OutputFormat::JsonLines)
        } else if !self.attributes.get()?.is_empty() {
            Ok(OutputFormat::Json)
        } else if self.package_values || !self.package_values_regex.is_empty() {
            Ok(OutputFormat::Json)
        } else if self.stats {
            if self.json || self.json_lines {
                return Err(TargetsError::IncompatibleArguments.into());
            }
            Ok(OutputFormat::Stats)
        } else {
            Ok(OutputFormat::Text)
        }
    }

    /// Return each of the strings that were supplied as arguments to `--package-values-regex` or,
    /// if `--package-values` is used, return an empty string that effectively matches all package values.
    fn package_values_as_regexes(&self) -> buck2_error::Result<Vec<String>> {
        if self.package_values {
            if self.package_values_regex.is_empty() {
                Ok(vec![String::new()])
            } else {
                Err(TargetsError::IncompatibleArguments.into())
            }
        } else {
            Ok(self.package_values_regex.clone())
        }
    }
}

#[async_trait(?Send)]
impl StreamingCommand for TargetsCommand {
    const COMMAND_NAME: &'static str = "targets";

    async fn exec_impl(
        mut self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let target_hash_use_fast_hash = match self.target_hash_function {
            TargetHashFunction::Sha1 | TargetHashFunction::Sha256 => {
                buck2_client_ctx::eprintln!(
                    "buck2 only supports \"fast\" and \"strong\" target hash functions. Using the \"strong\" hash."
                )?;
                false
            }
            TargetHashFunction::Murmur_Hash3 => {
                buck2_client_ctx::eprintln!(
                    "buck2 only supports \"fast\" and \"strong\" target hash functions. Using the \"fast\" hash."
                )?;
                true
            }
            TargetHashFunction::Fast => true,
            TargetHashFunction::Strong => false,
        };

        let output_attributes = self.attributes.get()?;
        let package_values = self.package_values_as_regexes()?;
        let target_hash_graph_type =
            match (self.show_target_hash, self.show_unconfigured_target_hash) {
                (true, true) => {
                    return ExitResult::err(TargetsError::IncompatibleArguments.into());
                }
                (true, false) => targets_request::TargetHashGraphType::Configured as i32,
                (false, true) => targets_request::TargetHashGraphType::Unconfigured as i32,
                (false, false) => targets_request::TargetHashGraphType::None as i32,
            };

        let output_format = self.output_format()?;

        let context = Some(ctx.client_context(matches, &self)?);

        let target_hash_modified_paths = self
            .target_hash_modified_paths
            .into_iter()
            .map(|path| path.resolve(&ctx.working_dir).into_string())
            .collect::<Result<Vec<_>, _>>()?;

        let target_request = TargetsRequest {
            context,
            target_patterns: self.patterns,
            output_format: output_format as i32,
            targets: Some(if self.resolve_alias {
                targets_request::Targets::ResolveAlias(targets_request::ResolveAlias {})
            } else {
                targets_request::Targets::Other(targets_request::Other {
                    output_attributes,
                    target_hash_file_mode: self.target_hash_file_mode.to_proto() as i32,
                    target_hash_modified_paths,
                    target_hash_use_fast_hash,
                    target_hash_graph_type,
                    include_default_attributes: self.include_defaults,
                    target_hash_recursive: self.target_hash_recursive,
                    keep_going: self.keep_going,
                    streaming: self.streaming,
                    cached: !self.no_cache,
                    imports: self.imports,
                    package_values,
                })
            }),
            target_cfg: Some(self.target_cfg.target_cfg()),
            output: self
                .output
                .map(|x| x.resolve(&ctx.working_dir).into_string())
                .transpose()?,
            concurrency: self
                .num_threads
                .map(|num| buck2_cli_proto::Concurrency { concurrency: num }),
            compression: self.compression.to_proto() as i32,
        };

        if let Some(format) = self.show_output.format() {
            let project_root = ctx.paths()?.roots.project_root.clone();
            targets_show_outputs(
                ctx.console_interaction_stream(&self.common_opts.console_opts),
                buckd,
                events_ctx,
                target_request,
                self.show_output.is_full().then(|| project_root.root()),
                format,
            )
            .await
        } else {
            targets(
                ctx.console_interaction_stream(&self.common_opts.console_opts),
                buckd,
                events_ctx,
                target_request,
            )
            .await
        }
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

async fn targets_show_outputs(
    console_interaction: Option<ConsoleInteractionStream<'_>>,
    buckd: &mut BuckdClientConnector,
    events_ctx: &mut EventsCtx,
    target_request: TargetsRequest,
    root_path: Option<&AbsNormPath>,
    format: PrintOutputsFormat,
) -> ExitResult {
    let response = buckd
        .with_flushing()
        .targets_show_outputs(
            target_request,
            events_ctx,
            console_interaction,
            &mut NoPartialResultHandler,
        )
        .await??;

    buck2_client_ctx::stdio::print_with_writer::<ClientIoError, _>(async move |w| {
        let root_path = root_path.map(|root| root.to_path_buf());
        let mut print = PrintOutputs::new(w, root_path, format)?;
        for target_paths in response.targets_paths {
            for path in target_paths.paths {
                print.output(&target_paths.target, Some(&path))?;
            }
        }
        print.finish()
    })
    .await?;

    ExitResult::success()
}

async fn targets(
    console_interaction: Option<ConsoleInteractionStream<'_>>,
    buckd: &mut BuckdClientConnector,
    events_ctx: &mut EventsCtx,
    target_request: TargetsRequest,
) -> ExitResult {
    let response = buckd
        .with_flushing()
        .targets(
            target_request,
            events_ctx,
            console_interaction,
            &mut StdoutPartialResultHandler,
        )
        .await??;
    if !response.serialized_targets_output.is_empty() {
        buck2_client_ctx::print!("{}", response.serialized_targets_output)?;
    }
    ExitResult::success()
}
