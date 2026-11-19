/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::materialize::invocation_re_use_case;
use buck2_cli_proto::trace_io_request;
use buck2_cli_proto::trace_io_response;
use buck2_common::file_ops::metadata::RawSymlink;
use buck2_common::io::trace::TracingIoProvider;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::span_async;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_server_ctx::commands::command_end;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use dupe::Dupe;

use crate::ctx::ServerCommandContext;

pub(crate) async fn trace_io_command(
    context: &ServerCommandContext<'_>,
    req: buck2_cli_proto::TraceIoRequest,
) -> buck2_error::Result<buck2_cli_proto::TraceIoResponse> {
    let start_event = context
        .command_start_event(buck2_data::TraceIoCommandStart {}.into())
        .await?;
    span_async(start_event, async move {
        let tracing_provider = TracingIoProvider::from_io(&*context.base_context.repo().io);
        let respond_with_trace = matches!(
            req.read_state,
            Some(trace_io_request::ReadIoTracingState { with_trace: true })
        );

        let result = match (tracing_provider, respond_with_trace) {
            (Some(provider), true) => build_response_with_trace(context, provider).await,
            (Some(_), false) => Ok(buck2_cli_proto::TraceIoResponse {
                enabled: true,
                trace: Vec::new(),
                external_entries: Vec::new(),
                relative_symlinks: Vec::new(),
                external_symlinks: Vec::new(),
            }),
            (None, _) => Ok(buck2_cli_proto::TraceIoResponse {
                enabled: false,
                trace: Vec::new(),
                external_entries: Vec::new(),
                relative_symlinks: Vec::new(),
                external_symlinks: Vec::new(),
            }),
        };

        let end_event = command_end(&result, buck2_data::TraceIoCommandEnd {});
        (result, end_event)
    })
    .await
}

async fn build_response_with_trace(
    context: &ServerCommandContext<'_>,
    provider: &TracingIoProvider,
) -> buck2_error::Result<buck2_cli_proto::TraceIoResponse> {
    // The offline-cache copies were declared during the build and nothing in it consumed them;
    // put them on disk so the offline tooling can archive them.
    let buck_out_entries = provider.trace().buck_out_entries();
    let artifacts = buck_out_entries
        .iter()
        .map(|(path, value)| {
            let value = value
                .as_any()
                .downcast_ref::<ArtifactValue>()
                .internal_error("traced buck-out entries carry artifact values")?;
            buck2_error::Ok((path.clone(), value.dupe()))
        })
        .collect::<buck2_error::Result<Vec<_>>>()?;
    let server_ctx: &dyn ServerCommandContextTrait = context;
    let re_use_case = server_ctx
        .with_dice_ctx(|_, dice_ctx| async move { Ok(invocation_re_use_case(&dice_ctx.ctx())) })
        .await?;
    let response = context
        .materializer()
        .materialize(MaterializeRequest {
            artifacts,
            outputs: Vec::new(),
            purpose: MaterializationPurpose::IntermediateOnly,
            re_use_case,
        })
        .await
        .buck_error_context("Error materializing buck-out paths for trace")?;
    for result in response.results {
        result.buck_error_context("Error materializing buck-out paths for trace")?;
    }
    // The entries are read by the offline tooling outside the daemon, after this command has
    // returned; there is no scope here to hold the lease over.
    drop(response.lease);

    let mut entries = provider.trace().project_entries();
    entries.extend(buck_out_entries.into_iter().map(|(path, _)| path));

    let mut relative_symlinks = Vec::new();
    let mut external_symlinks = Vec::new();
    for link in provider.trace().symlinks.iter() {
        match &link.to {
            RawSymlink::Relative(to, _) => {
                relative_symlinks.push(trace_io_response::RelativeSymlink {
                    link: link.at.to_string(),
                    target: to.to_string(),
                });
            }
            RawSymlink::External(external) => {
                external_symlinks.push(trace_io_response::ExternalSymlink {
                    link: link.at.to_string(),
                    target: external.target_str().to_owned(),
                    remaining_path: if external.remaining_path().is_empty() {
                        None
                    } else {
                        Some(external.remaining_path().to_string())
                    },
                });
            }
        }
    }

    let mut trace: Vec<String> = entries.into_iter().map(|path| path.to_string()).collect();
    trace.sort_unstable();

    let mut external_entries: Vec<String> = provider
        .trace()
        .external_entries()
        .into_iter()
        .map(|path| path.to_string())
        .collect();
    external_entries.sort_unstable();

    relative_symlinks.sort_unstable_by(|a, b| a.link.cmp(&b.link));
    external_symlinks.sort_unstable_by(|a, b| a.link.cmp(&b.link));

    Ok(buck2_cli_proto::TraceIoResponse {
        enabled: true,
        trace,
        external_entries,
        relative_symlinks,
        external_symlinks,
    })
}
