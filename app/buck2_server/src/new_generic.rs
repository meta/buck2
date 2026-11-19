/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::new_generic::NewGenericRequest;
use buck2_cli_proto::new_generic::NewGenericResponse;
use buck2_error::BuckErrorContext;
use buck2_server_ctx::late_bindings::DOCS_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::OTHER_SERVER_COMMANDS;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;

use crate::ctx::ServerCommandContext;

pub(crate) async fn new_generic_command(
    context: &ServerCommandContext<'_>,
    req: buck2_cli_proto::NewGenericRequestMessage,
    partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
) -> buck2_error::Result<buck2_cli_proto::NewGenericResponseMessage> {
    let req = req.new_generic_request;
    let req: NewGenericRequest = serde_json::from_str(&req)
        .buck_error_context("Could not deserialize `NewGenericRequest`")?;
    let resp = match req {
        NewGenericRequest::Complete(e) => NewGenericResponse::Complete(
            OTHER_SERVER_COMMANDS
                .get()?
                .complete(context, partial_result_dispatcher, e)
                .await?,
        ),
        NewGenericRequest::DebugEval(e) => NewGenericResponse::DebugEval(
            OTHER_SERVER_COMMANDS.get()?.debug_eval(context, e).await?,
        ),
        NewGenericRequest::DebugAnonTargets(e) => NewGenericResponse::DebugAnonTargets(
            OTHER_SERVER_COMMANDS
                .get()?
                .debug_anon_targets(context, e)
                .await?,
        ),
        NewGenericRequest::Explain(m) => NewGenericResponse::Explain(
            OTHER_SERVER_COMMANDS
                .get()?
                .explain(context, partial_result_dispatcher, m)
                .await?,
        ),
        NewGenericRequest::ExpandExternalCells(e) => NewGenericResponse::ExpandExternalCells(
            OTHER_SERVER_COMMANDS
                .get()?
                .expand_external_cells(context, partial_result_dispatcher, e)
                .await?,
        ),
        NewGenericRequest::Docs(d) => NewGenericResponse::Docs(
            DOCS_SERVER_COMMAND
                .get()?
                .docs(context, partial_result_dispatcher, d)
                .await?,
        ),
    };
    let resp = serde_json::to_string(&resp)
        .buck_error_context("Could not serialize `NewGenericResponse`")?;
    Ok(buck2_cli_proto::NewGenericResponseMessage {
        new_generic_response: resp,
    })
}
