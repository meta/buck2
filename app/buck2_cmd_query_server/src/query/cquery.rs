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
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_cli_proto::CqueryRequest;
use buck2_cli_proto::CqueryResponse;
use buck2_cli_proto::HasClientContext;
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_node::attrs::display::AttrDisplayWithContext;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::serialize::AttrSerializeWithContext;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::environment::AttrFmtOptions;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use buck2_util::truncate::truncate;
use dice::DiceTransaction;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;
use futures::FutureExt;

use crate::query::printer::ProviderLookUp;
use crate::query::printer::QueryResultPrinter;
use crate::query::printer::ShouldPrintProviders;
use crate::query::query_target_ext::QueryCommandTarget;
use crate::query::starlark_profile::write_query_profile_for_targets;

impl QueryCommandTarget for ConfiguredTargetNode {
    fn call_stack(&self) -> buck2_error::Result<Option<String>> {
        Ok(self.body()?.call_stack())
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

pub(crate) async fn cquery_command(
    ctx: &dyn ServerCommandContextTrait,
    partial_result_dispatcher: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    req: CqueryRequest,
) -> buck2_error::Result<CqueryResponse> {
    run_server_command(CqueryServerCommand { req }, ctx, partial_result_dispatcher).await
}

struct CqueryServerCommand {
    req: CqueryRequest,
}

#[async_trait]
impl ServerCommandTemplate for CqueryServerCommand {
    type StartEvent = buck2_data::CQueryCommandStart;
    type EndEvent = buck2_data::CQueryCommandEnd;
    type Response = CqueryResponse;
    type PartialResult = buck2_cli_proto::StdoutBytes;

    fn start_event(&self) -> buck2_data::CQueryCommandStart {
        buck2_data::CQueryCommandStart {
            query: truncate(&self.req.query, 50000),
            query_args: truncate(&self.req.query_args.join(","), 1000),
            target_universe: truncate(&self.req.target_universe.join(","), 1000),
        }
    }

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
        cquery(
            server_ctx,
            partial_result_dispatcher.as_writer(),
            ctx,
            &self.req,
        )
        .await
    }
}

async fn cquery(
    server_ctx: &dyn ServerCommandContextTrait,
    mut stdout: impl Write + Send,
    ctx: DiceTransaction,
    request: &CqueryRequest,
) -> buck2_error::Result<CqueryResponse> {
    let cell_resolver = ctx.ctx().get_cell_resolver().await?;
    let output_configuration = QueryResultPrinter::from_request_options(
        &cell_resolver,
        &request.output_attributes,
        request.unstable_output_format,
        request.client_context()?.trace_id.clone(),
    )?;

    let CqueryRequest {
        query,
        query_args,
        target_universe,
        context,
        show_providers,
        target_cfg,
        ..
    } = request;

    // The request will always have a universe value, an empty one indicates the user didn't provide a universe.
    let target_universe: Option<&[String]> = if target_universe.is_empty() {
        None
    } else {
        Some(target_universe)
    };
    let client_ctx = context.as_ref().internal_error("No client context")?;

    let target_call_stacks = client_ctx.target_call_stacks;

    let global_cfg_options = global_cfg_options_from_client_context(
        target_cfg
            .as_ref()
            .internal_error("target_cfg must be set")?,
        server_ctx,
        &mut ctx.ctx(),
    )
    .await?;

    let profile_mode = request
        .profile_mode
        .map(|i| buck2_cli_proto::ProfileMode::try_from(i).internal_error("Invalid profile mode"))
        .transpose()?;

    let (query_result, universes) = QUERY_FRONTEND
        .get()?
        .eval_cquery(
            &mut ctx.ctx(),
            server_ctx.working_dir(),
            query,
            query_args,
            global_cfg_options,
            target_universe,
            profile_mode.is_some(),
        )
        .await?;

    if let Some(profile_mode) = profile_mode {
        let universes = universes.internal_error("No universes")?;
        if universes.is_empty() {
            // Sanity check.
            return Err(internal_error!("Empty universes list"));
        }

        write_query_profile_for_targets(
            &mut ctx.ctx(),
            profile_mode,
            request.profile_output.as_deref(),
            universes.iter().flat_map(|u| {
                u.iter()
                    .map(|t| t.label().unconfigured().pkg())
                    // `collect` should not be needed, but I was defeated by the compiler.
                    .collect::<Vec<_>>()
            }),
        )
        .await?;
    } else if universes.is_some() {
        return Err(internal_error!("We did not request universes"));
    }

    ctx.ctx()
        .with_linear_recompute(|ctx| {
            async move {
                let should_print_providers = if *show_providers {
                    ShouldPrintProviders::Yes(&ctx as &dyn ProviderLookUp<ConfiguredTargetNode>)
                } else {
                    ShouldPrintProviders::No
                };

                match query_result {
                    QueryEvaluationResult::Single(targets) => {
                        output_configuration
                            .print_single_output(
                                &mut stdout,
                                targets,
                                target_call_stacks,
                                should_print_providers,
                            )
                            .await?
                    }
                    QueryEvaluationResult::Multiple(results) => {
                        output_configuration
                            .print_multi_output(
                                &mut stdout,
                                results,
                                target_call_stacks,
                                should_print_providers,
                            )
                            .await?
                    }
                };
                buck2_error::Ok(())
            }
            .boxed()
        })
        .await?;

    Ok(CqueryResponse {})
}

#[async_trait]
impl ProviderLookUp<ConfiguredTargetNode> for LinearRecomputeDiceComputations<'_, '_> {
    async fn lookup(
        &self,
        t: &ConfiguredTargetNode,
    ) -> buck2_error::Result<MaybeCompatible<FrozenProviderCollectionValue>> {
        Ok(self
            .get()
            .get_providers(&ConfiguredProvidersLabel::new(
                t.label().dupe(),
                ProvidersName::Default,
            ))
            .await?)
    }
}
