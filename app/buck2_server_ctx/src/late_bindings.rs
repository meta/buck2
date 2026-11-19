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
use buck2_cli_proto::new_generic::AnonTargetsRequest;
use buck2_cli_proto::new_generic::AnonTargetsResponse;
use buck2_cli_proto::new_generic::CompleteRequest;
use buck2_cli_proto::new_generic::CompleteResponse;
use buck2_cli_proto::new_generic::DebugEvalRequest;
use buck2_cli_proto::new_generic::DebugEvalResponse;
use buck2_cli_proto::new_generic::ExpandExternalCellsRequest;
use buck2_cli_proto::new_generic::ExpandExternalCellsResponse;
use buck2_cli_proto::new_generic::ExplainRequest;
use buck2_cli_proto::new_generic::ExplainResponse;
use buck2_util::late_binding::LateBinding;

use crate::ctx::ServerCommandContextTrait;
use crate::partial_result_dispatcher::NoPartialResult;
use crate::partial_result_dispatcher::PartialResultDispatcher;
use crate::streaming_request_handler::StreamingRequestHandler;

#[async_trait]
pub trait OtherServerCommands: Send + Sync + 'static {
    async fn build(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::BuildRequest,
    ) -> buck2_error::Result<buck2_cli_proto::BuildResponse>;
    async fn install(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::InstallRequest,
    ) -> buck2_error::Result<buck2_cli_proto::InstallResponse>;
    async fn complete(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: CompleteRequest,
    ) -> buck2_error::Result<CompleteResponse>;
    async fn debug_eval(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        req: DebugEvalRequest,
    ) -> buck2_error::Result<DebugEvalResponse>;
    async fn debug_anon_targets(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        req: AnonTargetsRequest,
    ) -> buck2_error::Result<AnonTargetsResponse>;
    async fn explain(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: ExplainRequest,
    ) -> buck2_error::Result<ExplainResponse>;
    async fn expand_external_cells(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: ExpandExternalCellsRequest,
    ) -> buck2_error::Result<ExpandExternalCellsResponse>;
}

pub static OTHER_SERVER_COMMANDS: LateBinding<&'static dyn OtherServerCommands> =
    LateBinding::new("OTHER_SERVER_COMMANDS");

#[async_trait]
pub trait TargetsServerCommands: Send + Sync + 'static {
    async fn targets(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::TargetsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::TargetsResponse>;
    async fn targets_show_outputs(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::TargetsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::TargetsShowOutputsResponse>;
    async fn ctargets(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::ConfiguredTargetsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::ConfiguredTargetsResponse>;
}

pub static TARGETS_SERVER_COMMANDS: LateBinding<&'static dyn TargetsServerCommands> =
    LateBinding::new("TARGETS_SERVER_COMMANDS");

#[async_trait]
pub trait QueryServerCommands: Send + Sync + 'static {
    async fn uquery(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::UqueryRequest,
    ) -> buck2_error::Result<buck2_cli_proto::UqueryResponse>;
    async fn cquery(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::CqueryRequest,
    ) -> buck2_error::Result<buck2_cli_proto::CqueryResponse>;
    async fn aquery(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::AqueryRequest,
    ) -> buck2_error::Result<buck2_cli_proto::AqueryResponse>;
}

pub static QUERY_SERVER_COMMANDS: LateBinding<&'static dyn QueryServerCommands> =
    LateBinding::new("QUERY_SERVER_COMMANDS");

#[async_trait]
pub trait DocsServerCommand: Send + Sync + 'static {
    async fn docs(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
        req: buck2_cli_proto::new_generic::DocsRequest,
    ) -> buck2_error::Result<buck2_cli_proto::new_generic::DocsResponse>;
}

pub static DOCS_SERVER_COMMAND: LateBinding<&'static dyn DocsServerCommand> =
    LateBinding::new("DOCS_SERVER_COMMAND");

#[async_trait]
pub trait AuditServerCommand: Send + Sync + 'static {
    async fn audit(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::GenericRequest,
    ) -> buck2_error::Result<buck2_cli_proto::GenericResponse>;
}

pub static AUDIT_SERVER_COMMAND: LateBinding<&'static dyn AuditServerCommand> =
    LateBinding::new("AUDIT_SERVER_COMMAND");

#[async_trait]
pub trait StarlarkServerCommand: Send + Sync + 'static {
    async fn starlark(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        req: buck2_cli_proto::GenericRequest,
    ) -> buck2_error::Result<buck2_cli_proto::GenericResponse>;
}

pub static STARLARK_SERVER_COMMAND: LateBinding<&'static dyn StarlarkServerCommand> =
    LateBinding::new("STARLARK_SERVER_COMMAND");

#[async_trait]
pub trait LspServerCommand: Send + Sync + 'static {
    async fn lsp(
        &self,
        ctx: &dyn ServerCommandContextTrait,
        partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::LspMessage>,
        req: StreamingRequestHandler<buck2_cli_proto::LspRequest>,
    ) -> buck2_error::Result<buck2_cli_proto::LspResponse>;
}

pub static LSP_SERVER_COMMAND: LateBinding<&'static dyn LspServerCommand> =
    LateBinding::new("LSP_SERVER_COMMAND");
