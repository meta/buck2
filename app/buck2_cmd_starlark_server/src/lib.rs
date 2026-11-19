/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod lint;
mod typecheck;
mod util;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_starlark_client::StarlarkSubcommand;
use buck2_events::dispatch::span_async;
use buck2_server_ctx::commands::command_end;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::late_bindings::STARLARK_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::StarlarkServerCommand;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;

pub fn init_late_bindings() {
    STARLARK_SERVER_COMMAND.init(&StarlarkServerCommandImpl);
}

struct StarlarkServerCommandImpl;

#[async_trait]
impl StarlarkServerCommand for StarlarkServerCommandImpl {
    async fn starlark(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::GenericRequest,
    ) -> buck2_error::Result<buck2_cli_proto::GenericResponse> {
        let start_event = ctx
            .command_start_event(buck2_data::StarlarkCommandStart {}.into())
            .await?;
        span_async(
            start_event,
            server_starlark_command_inner(ctx, partial_result_dispatcher, req),
        )
        .await
    }
}

#[async_trait]
pub(crate) trait StarlarkServerSubcommand: Send + Sync + 'static {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        client_server_ctx: ClientContext,
    ) -> buck2_error::Result<()>;
}

async fn server_starlark_command_inner(
    context: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: buck2_cli_proto::GenericRequest,
) -> (
    buck2_error::Result<buck2_cli_proto::GenericResponse>,
    buck2_data::CommandEnd,
) {
    let result = parse_command_and_execute(context, partial_result_dispatcher, req).await;
    let end_event = command_end(&result, buck2_data::StarlarkCommandEnd {});

    let result = result.map(|()| buck2_cli_proto::GenericResponse {});

    (result, end_event)
}

async fn parse_command_and_execute(
    context: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: buck2_cli_proto::GenericRequest,
) -> buck2_error::Result<()> {
    let command: StarlarkSubcommand = serde_json::from_str(&req.serialized_opts)?;
    as_server_subcommand(&command)
        .server_execute(
            context,
            partial_result_dispatcher,
            req.context.expect("buck cli always sets a client context"),
        )
        .await
}

fn as_server_subcommand(cmd: &StarlarkSubcommand) -> &dyn StarlarkServerSubcommand {
    match cmd {
        StarlarkSubcommand::Lint(cmd) => cmd,
        StarlarkSubcommand::Typecheck(cmd) => cmd,
    }
}
