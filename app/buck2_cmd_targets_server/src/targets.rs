/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod default;
pub mod fmt;
pub mod resolve_alias;
pub mod streaming;

use std::fs::File;
use std::io::BufWriter;
use std::io::Write;

use async_trait::async_trait;
use buck2_cli_proto::TargetsRequest;
use buck2_cli_proto::TargetsResponse;
use buck2_cli_proto::targets_request;
use buck2_cli_proto::targets_request::Compression;
use buck2_cli_proto::targets_request::TargetHashGraphType;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::pattern::parse_from_cli::parse_patterns_from_cli_args;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::DiceTransaction;
use flate2::write::GzEncoder;
use zstd::stream::write as zstd;

use crate::targets::default::TargetHashOptions;
use crate::targets::default::targets_batch;
use crate::targets::fmt::create_formatter;
use crate::targets::resolve_alias::targets_resolve_aliases;
use crate::targets::streaming::targets_streaming;

#[derive(PartialEq, Eq)]
enum OutputType {
    Stdout,
    File,
}

trait Compressor: Write + Send {
    fn finish(self: Box<Self>) -> buck2_error::Result<()>;
}

struct UncompressedCompressor<T>(T);

impl<T: Write> Write for UncompressedCompressor<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf)
    }
}

impl<T: Write + Send> Compressor for UncompressedCompressor<T> {
    fn finish(self: Box<Self>) -> buck2_error::Result<()> {
        Ok(())
    }
}

impl<T: Write + Send> Compressor for GzEncoder<T> {
    fn finish(self: Box<Self>) -> buck2_error::Result<()> {
        (*self).finish()?;
        Ok(())
    }
}

impl<T: Write + Send> Compressor for zstd::Encoder<'_, T> {
    fn finish(self: Box<Self>) -> buck2_error::Result<()> {
        (*self).finish()?;
        Ok(())
    }
}

fn outputter<'a, W: Write + Send + 'a>(
    request: &TargetsRequest,
    stdout: W,
) -> buck2_error::Result<(OutputType, Box<dyn Compressor + 'a>)> {
    let (output_type, output): (_, Box<dyn Compressor>) = match &request.output {
        None => (OutputType::Stdout, Box::new(UncompressedCompressor(stdout))),
        Some(file) => {
            let file = BufWriter::new(File::create(file).with_buck_error_context(|| {
                format!("Failed to open file `{file}` for `targets` output ")
            })?);
            (OutputType::File, Box::new(UncompressedCompressor(file)))
        }
    };

    let compression = Compression::try_from(request.compression)
        .internal_error("buck cli should send valid compression type")?;
    let output = match compression {
        Compression::Uncompressed => output,
        Compression::Gzip => Box::new(GzEncoder::new(output, Default::default())),
        Compression::Zstd => Box::new(zstd::Encoder::new(output, 9)?),
    };
    Ok((output_type, output))
}

pub async fn targets_command(
    server_ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: TargetsRequest,
) -> buck2_error::Result<TargetsResponse> {
    run_server_command(
        TargetsServerCommand { req },
        server_ctx,
        partial_result_dispatcher,
    )
    .await
}

struct TargetsServerCommand {
    req: TargetsRequest,
}

#[async_trait]
impl ServerCommandTemplate for TargetsServerCommand {
    type StartEvent = buck2_data::TargetsCommandStart;
    type EndEvent = buck2_data::TargetsCommandEnd;
    type Response = TargetsResponse;
    type PartialResult = buck2_cli_proto::StdoutBytes;

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        dice: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        targets(
            server_ctx,
            &mut partial_result_dispatcher.as_writer(),
            dice,
            &self.req,
        )
        .await
    }

    fn end_event(&self, _response: &buck2_error::Result<Self::Response>) -> Self::EndEvent {
        buck2_data::TargetsCommandEnd {
            unresolved_target_patterns: self
                .req
                .target_patterns
                .iter()
                .map(|p| buck2_data::TargetPattern { value: p.clone() })
                .collect(),
        }
    }
}

async fn targets(
    server_ctx: &dyn ServerCommandContextTrait,
    stdout: &mut (impl Write + Send),
    dice: DiceTransaction,
    request: &TargetsRequest,
) -> buck2_error::Result<TargetsResponse> {
    let (output_type, mut output) = outputter(request, stdout)?;
    let mut res = targets_with_output(server_ctx, dice, request, &mut output).await;
    match &mut res {
        Ok(response)
            if !response.serialized_targets_output.is_empty()
                && output_type == OutputType::File =>
        {
            output.write_all(response.serialized_targets_output.as_bytes())?;
            response.serialized_targets_output.clear();
        }
        _ => {}
    }
    output.flush()?;
    output.finish()?;
    res
}

async fn targets_with_output(
    server_ctx: &dyn ServerCommandContextTrait,
    dice: DiceTransaction,
    request: &TargetsRequest,
    output: &mut (impl Write + Send),
) -> buck2_error::Result<TargetsResponse> {
    let cwd = server_ctx.working_dir();
    let cell_resolver = dice.ctx().get_cell_resolver().await?;
    let parsed_target_patterns = parse_patterns_from_cli_args::<TargetPatternExtra>(
        &mut dice.ctx(),
        &request.target_patterns,
        cwd,
    )
    .await?;

    match &request.targets {
        Some(targets_request::Targets::ResolveAlias(_)) => {
            targets_resolve_aliases(&dice, request, parsed_target_patterns).await
        }
        Some(targets_request::Targets::Other(other)) => {
            if other.streaming {
                let formatter = create_formatter(request, other)?;
                let hashing = match TargetHashGraphType::try_from(other.target_hash_graph_type)
                    .expect("buck cli should send valid target hash graph type")
                {
                    TargetHashGraphType::None => None,
                    _ => Some(other.target_hash_use_fast_hash),
                };

                let res = targets_streaming(
                    server_ctx,
                    dice,
                    formatter,
                    output,
                    parsed_target_patterns,
                    other.keep_going,
                    other.cached,
                    other.imports,
                    hashing,
                    request.concurrency.as_ref().map(|x| x.concurrency as usize),
                )
                .await?;
                Ok(TargetsResponse {
                    error_count: res.errors,
                    serialized_targets_output: String::new(),
                })
            } else {
                let formatter = create_formatter(request, other)?;
                let global_cfg_options = global_cfg_options_from_client_context(
                    request
                        .target_cfg
                        .as_ref()
                        .internal_error("target_cfg must be set")?,
                    server_ctx,
                    &mut dice.ctx(),
                )
                .await?;
                let fs = server_ctx.project_root();
                targets_batch(
                    server_ctx,
                    &dice,
                    &*formatter,
                    parsed_target_patterns,
                    &global_cfg_options,
                    TargetHashOptions::new(other, &cell_resolver, fs)?,
                    other.keep_going,
                )
                .await
            }
        }
        None => Err(internal_error!("Missing field in proto request")),
    }
}
