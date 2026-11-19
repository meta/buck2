/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(try_blocks)]

use async_trait::async_trait;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::late_bindings::TARGETS_SERVER_COMMANDS;
use buck2_server_ctx::late_bindings::TargetsServerCommands;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;

pub(crate) mod configured_target_hash;
pub mod ctargets;
pub(crate) mod json;
pub mod target_hash;
pub mod targets;
pub mod targets_show_outputs;

struct TargetsServerCommandsInstance;

#[async_trait]
impl TargetsServerCommands for TargetsServerCommandsInstance {
    async fn targets(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::TargetsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::TargetsResponse> {
        targets::targets_command(ctx, partial_result_dispatcher, req).await
    }

    async fn targets_show_outputs(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::TargetsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::TargetsShowOutputsResponse> {
        targets_show_outputs::targets_show_outputs_command(ctx, partial_result_dispatcher, req)
            .await
    }

    async fn ctargets(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::ConfiguredTargetsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::ConfiguredTargetsResponse> {
        ctargets::configured_targets_command(ctx, partial_result_dispatcher, req).await
    }
}

pub fn init_late_bindings() {
    TARGETS_SERVER_COMMANDS.init(&TargetsServerCommandsInstance);
}
