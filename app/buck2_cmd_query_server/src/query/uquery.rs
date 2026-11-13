/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use async_trait::async_trait;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_cli_proto::HasClientContext as _;
use buck2_cli_proto::UqueryRequest;
use buck2_cli_proto::UqueryResponse;
use buck2_common::dice::cells::HasCellResolver;
use buck2_error::BuckErrorOptionContext;
use buck2_node::attrs::display::AttrDisplayWithContext;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::serialize::AttrSerializeWithContext;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::nodes::unconfigured::TargetNodeData;
use buck2_query::query::environment::AttrFmtOptions;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::DiceTransaction;
use dupe::Dupe;

use crate::query::printer::QueryResultPrinter;
use crate::query::printer::ShouldPrintProviders;
use crate::query::query_target_ext::QueryCommandTarget;

impl QueryCommandTarget for TargetNode {
    fn call_stack(&self) -> buck2_error::Result<Option<String>> {
        Ok(TargetNodeData::call_stack(self))
    }

    fn attr_to_string_alternate(&self, options: AttrFmtOptions, attr: &Self::Attr<'_>) -> String {
        format!(
            "{:#}",
            attr.as_display(&AttrFmtContext {
                package: Some(self.label().pkg().dupe()),
                options
            })
        )
    }

    fn attr_serialize<S: serde::Serializer>(
        &self,
        attr: &Self::Attr<'_>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        attr.serialize_with_ctx(
            &AttrFmtContext {
                package: Some(self.label().pkg().dupe()),
                options: Default::default(),
            },
            serializer,
        )
    }

    fn attr_fmt(
        &self,
        fmt: &mut std::fmt::Formatter<'_>,
        options: AttrFmtOptions,
        attr: &Self::Attr<'_>,
    ) -> std::fmt::Result {
        AttrDisplayWithContext::fmt(
            attr,
            &AttrFmtContext {
                package: Some(self.label().pkg().dupe()),
                options,
            },
            fmt,
        )
    }
}

pub(crate) async fn uquery_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: UqueryRequest,
) -> buck2_error::Result<UqueryResponse> {
    run_server_command(UqueryServerCommand { req }, ctx, partial_result_dispatcher).await
}

struct UqueryServerCommand {
    req: UqueryRequest,
}

#[async_trait]
impl ServerCommandTemplate for UqueryServerCommand {
    type StartEvent = buck2_data::QueryCommandStart;
    type EndEvent = buck2_data::QueryCommandEnd;
    type Response = UqueryResponse;
    type PartialResult = buck2_cli_proto::StdoutBytes;

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        uquery(
            server_ctx,
            partial_result_dispatcher.as_writer(),
            ctx,
            &self.req,
        )
        .await
    }
}

async fn uquery(
    server_ctx: &dyn ServerCommandContextTrait,
    mut stdout: impl Write,
    ctx: DiceTransaction,
    request: &UqueryRequest,
) -> buck2_error::Result<UqueryResponse> {
    let cell_resolver = ctx.ctx().get_cell_resolver().await?;
    let output_configuration = QueryResultPrinter::from_request_options(
        &cell_resolver,
        &request.output_attributes,
        request.unstable_output_format,
        request.client_context()?.trace_id.clone(),
    )?;

    let UqueryRequest {
        query,
        query_args,
        context,
        allow_partial_graph,
        ..
    } = request;

    let client_ctx = context.as_ref().internal_error("No client context")?;

    let target_call_stacks = client_ctx.target_call_stacks;

    let query_result = QUERY_FRONTEND
        .get()?
        .eval_uquery(
            &mut ctx.ctx(),
            server_ctx.working_dir(),
            query,
            query_args,
            *allow_partial_graph,
        )
        .await?;

    match query_result {
        QueryEvaluationResult::Single(targets) => {
            output_configuration
                .print_single_output(
                    &mut stdout,
                    targets,
                    target_call_stacks,
                    ShouldPrintProviders::No,
                )
                .await?
        }
        QueryEvaluationResult::Multiple(results) => {
            output_configuration
                .print_multi_output(
                    &mut stdout,
                    results,
                    target_call_stacks,
                    ShouldPrintProviders::No,
                )
                .await?
        }
    };

    Ok(UqueryResponse {})
}
