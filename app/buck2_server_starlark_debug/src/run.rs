/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_events::dispatch::span_async;
use buck2_server_ctx::commands::command_end;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::streaming_request_handler::StreamingRequestHandler;
use debugserver_types as dap;
use tokio::select;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tracing::debug;

use crate::ServerConnection;
use crate::error::StarlarkDebuggerInternalError;

/// Messages from the debugger server to its client (the cli `buck2 starlark debug-attach` which
/// then forwards them along through its stdout).
#[derive(Debug)]
pub(crate) enum ToClientMessage {
    Event(dap::Event),
    Response(dap::Response),
    Shutdown(buck2_error::Result<()>),
}

impl ToClientMessage {
    fn pretty_string(&self) -> buck2_error::Result<String> {
        match self {
            ToClientMessage::Event(ev) => Ok(serde_json::to_string_pretty(&ev)?),
            ToClientMessage::Response(resp) => Ok(serde_json::to_string_pretty(&resp)?),
            ToClientMessage::Shutdown(res) => Ok(format!("{res:?}")),
        }
    }
}

/// Wraps `run_dap_server` with a command start/end span.
pub async fn run_dap_server_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::DapMessage>,
    req: StreamingRequestHandler<buck2_cli_proto::DapRequest>,
) -> buck2_error::Result<buck2_cli_proto::DapResponse> {
    let start_event = ctx
        .command_start_event(buck2_data::StarlarkDebugAttachCommandStart {}.into())
        .await?;
    span_async(start_event, async move {
        let result = run_dap_server(ctx, partial_result_dispatcher, req).await;
        let end_event = command_end(&result, buck2_data::StarlarkDebugAttachCommandEnd {});
        (result, end_event)
    })
    .await
}

async fn run_dap_server(
    ctx: &dyn ServerCommandContextTrait,
    mut partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::DapMessage>,
    mut req: StreamingRequestHandler<buck2_cli_proto::DapRequest>,
) -> buck2_error::Result<buck2_cli_proto::DapResponse> {
    let (to_client_send, mut to_client_recv) = mpsc::unbounded_channel();
    let server_connection = ServerConnection::new(to_client_send, ctx.project_root().clone())?;

    let mut seq = 0;

    // We can get requests from the client (e.g. vscode) and we can get responses/events from the server
    // and mostly we just forward them along to the other side.
    // If an error is encountered or the server sends the Shutdown message, we'll break out, disconnect
    // from the server and return the final response.
    let response = loop {
        select! {
            request = req.next() => {
                let request = match request {
                    Some(Err(e)) => return Err(e),
                    Some(Ok(v)) => v,
                    None => {
                        // client disconnected.
                        break buck2_cli_proto::DapResponse {};
                    }
                };


                let debugserver_req: dap::Request = serde_json::from_str(&request.dap_json)?;
                debug!("received request {}", &serde_json::to_string_pretty(&debugserver_req)?);
                let disconnect_requested = debugserver_req.command == "disconnect";
                server_connection.0.send_request(debugserver_req)?;
                if disconnect_requested {
                     break buck2_cli_proto::DapResponse {};
                }
            }
            message = to_client_recv.recv() => {
                match message {
                    Some(v) => {
                        if let Some(v) = handle_outgoing_message(&mut seq, &mut partial_result_dispatcher, v)? {
                            break v;
                        }
                    }
                    None => return Err(StarlarkDebuggerInternalError::UnexpectedDebuggerShutdown.into())
                }
            }
        }
    };

    debug!("returning debugserver response");
    Ok(response)
}

/// Converts the ToClientMessage to either a partial result (and dispatches it) and/or the final
/// response (for a Shutdown message).
fn handle_outgoing_message(
    seq: &mut u32,
    partial_result_dispatcher: &mut PartialResultDispatcher<buck2_cli_proto::DapMessage>,
    message: ToClientMessage,
) -> buck2_error::Result<Option<buck2_cli_proto::DapResponse>> {
    debug!("sending message {}", &message.pretty_string()?);

    let this_seq = *seq as i64;
    *seq += 1;

    let (dap_json, response) = match message {
        ToClientMessage::Event(mut ev) => {
            ev.seq = this_seq;
            (serde_json::to_vec(&ev)?, None)
        }
        ToClientMessage::Response(mut resp) => {
            resp.seq = this_seq;
            (serde_json::to_vec(&resp)?, None)
        }
        ToClientMessage::Shutdown(res) => {
            let exit_code = match res {
                Ok(..) => 0,
                Err(e) => {
                    debug!("server shutdown with error {:?}", &e);
                    1
                }
            };

            (
                serde_json::to_vec(&dap::ExitedEvent {
                    type_: "event".to_owned(),
                    seq: this_seq,
                    event: "exited".to_owned(),
                    body: dap::ExitedEventBody { exit_code },
                })?,
                Some(buck2_cli_proto::DapResponse {}),
            )
        }
    };

    partial_result_dispatcher.emit(buck2_cli_proto::DapMessage { dap_json });
    Ok(response)
}
