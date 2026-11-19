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
use buck2_server_ctx::late_bindings::AUDIT_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::AuditServerCommand;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;

use crate::AuditCommand;
use crate::AuditCommandExt;

pub(crate) fn init_audit_server_command() {
    AUDIT_SERVER_COMMAND.init(&AuditServerCommandImpl);
}

struct AuditServerCommandImpl;

#[async_trait::async_trait]
impl AuditServerCommand for AuditServerCommandImpl {
    async fn audit(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::GenericRequest,
    ) -> buck2_error::Result<buck2_cli_proto::GenericResponse> {
        let start_event = ctx
            .command_start_event(buck2_data::AuditCommandStart {}.into())
            .await?;
        span_async(
            start_event,
            server_audit_command_inner(ctx, partial_result_dispatcher, req),
        )
        .await
    }
}

async fn server_audit_command_inner(
    context: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: buck2_cli_proto::GenericRequest,
) -> (
    buck2_error::Result<buck2_cli_proto::GenericResponse>,
    buck2_data::CommandEnd,
) {
    let result = parse_command_and_execute(context, partial_result_dispatcher, req).await;
    let end_event = command_end(&result, buck2_data::AuditCommandEnd {});

    let result = result.map(|()| buck2_cli_proto::GenericResponse {});

    (result, end_event)
}

async fn parse_command_and_execute(
    context: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: buck2_cli_proto::GenericRequest,
) -> buck2_error::Result<()> {
    let command: AuditCommand = serde_json::from_str(&req.serialized_opts)?;
    command
        .server_execute(
            context,
            partial_result_dispatcher,
            req.context.expect("buck cli always sets a client context"),
        )
        .await
}
