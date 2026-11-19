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
use buck2_cli_proto::new_generic::DocsRequest;
use buck2_cli_proto::new_generic::DocsResponse;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::late_bindings::DOCS_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::DocsServerCommand;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::DiceTransaction;

use crate::builtins::docs_starlark_builtins;
use crate::starlark_::docs_starlark;

mod builtins;
mod json;
mod starlark_;

struct DocsServerCommandImpl;

#[async_trait::async_trait]
impl DocsServerCommand for DocsServerCommandImpl {
    async fn docs(
        &self,
        context: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: DocsRequest,
    ) -> buck2_error::Result<DocsResponse> {
        run_server_command(DocsServerCmd { req }, context, partial_result_dispatcher).await
    }
}

pub fn init_late_bindings() {
    DOCS_SERVER_COMMAND.init(&DocsServerCommandImpl);
}

struct DocsServerCmd {
    req: DocsRequest,
}

#[async_trait]
impl ServerCommandTemplate for DocsServerCmd {
    type StartEvent = buck2_data::DocsCommandStart;
    type EndEvent = buck2_data::DocsCommandEnd;
    type Response = DocsResponse;
    type PartialResult = NoPartialResult;

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        _partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        Ok(docs(server_ctx, ctx, &self.req).await?)
    }
}

async fn docs(
    server_ctx: &dyn ServerCommandContextTrait,
    dice_ctx: DiceTransaction,
    request: &DocsRequest,
) -> buck2_error::Result<DocsResponse> {
    match request {
        DocsRequest::Starlark(request) => docs_starlark(server_ctx, dice_ctx, request).await,
        DocsRequest::StarlarkBuiltins(request) => {
            docs_starlark_builtins(server_ctx, dice_ctx, request).await
        }
    }
}
