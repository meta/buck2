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
use buck2_core::logging::log_file::TracingLogFile;
use buck2_data::BuildResult;
use buck2_events::dispatch::span_async;
use buck2_execute::materialize::materializer::HasMaterializer;
use dice::DiceTransaction;

use crate::commands::command_end_ext;
use crate::ctx::ServerCommandContextTrait;
use crate::ctx::ServerCommandDiceContext;
use crate::partial_result_dispatcher::PartialResultDispatcher;

/// Typical server command with DICE and span.
#[async_trait]
pub trait ServerCommandTemplate: Send + Sync {
    /// Event to send in the beginning of command.
    type StartEvent: Into<buck2_data::command_start::Data> + Default;
    /// Event to send in the end of command.
    type EndEvent: Into<buck2_data::command_end::Data> + Default;
    /// Command return type.
    /// TODO: This is called `Result` everywhere, we should probably be consistent.
    type Response: Send;
    /// Command partial response.
    type PartialResult: Send + Sync;

    /// Create start event. Called before command is invoked.
    fn start_event(&self) -> Self::StartEvent {
        Self::StartEvent::default()
    }

    /// Create end event. Called after command is invoked.
    fn end_event(&self, _response: &buck2_error::Result<Self::Response>) -> Self::EndEvent {
        Self::EndEvent::default()
    }

    /// Used to report (successful) builds since rebase, only for commands that return a `BuildResult` (build, install, test).
    /// If the command succeeded in building the target specified, `BuildResult.build_completed` should be true,
    /// even if the command failed for another reason.
    fn build_result(&self, _response: &Self::Response) -> Option<BuildResult> {
        None
    }

    /// If not `None`, command will block and be blocked by any concurrent commands.
    fn exclusive_command_name(&self) -> Option<String> {
        None
    }

    /// Command implementation.
    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response>;
}

/// Call this function to run the command template implementation.
pub async fn run_server_command<T: ServerCommandTemplate>(
    command: T,
    server_ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<<T as ServerCommandTemplate>::PartialResult>,
) -> buck2_error::Result<T::Response> {
    let start_event = server_ctx
        .command_start_event(command.start_event().into())
        .await?;
    // refresh our tracing log per command
    TracingLogFile::refresh()?;

    span_async(start_event, async {
        let result = server_ctx
            .with_dice_ctx_maybe_exclusive(
                |server_ctx, ctx| {
                    ctx.per_transaction_data()
                        .get_materializer()
                        .log_materializer_state(server_ctx.events());

                    command.command(server_ctx, partial_result_dispatcher, ctx)
                },
                command.exclusive_command_name(),
            )
            .await;
        let end_event = command_end_ext(&result, command.end_event(&result), |result| {
            command.build_result(result)
        });
        (result, end_event)
    })
    .await
}
