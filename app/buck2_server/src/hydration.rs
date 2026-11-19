/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The `buck2 debug hydration` command: manually page DICE node values out to
//! disk, page them back in, or report paging status.
//!
//! The paging mechanism these subcommands drive — page-out itself, the
//! single-flight/cancel state shared with automatic idle page-out, and the idle
//! page-out scheduling — lives in [`crate::paging`].

use std::sync::Arc;

use buck2_cli_proto::HydrationSubcommand;
use buck2_core::logging::log_file::TracingLogFile;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::dispatch::span_async;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_server_ctx::commands::command_end;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dice::Dice;
use dice::PagableStatus;
use dupe::Dupe;
use starlark::pagable::starlark_deserialization_state_retained_bytes;
use starlark::pagable::starlark_serialization_state_retained_bytes;

use crate::ctx::ServerCommandContext;
use crate::paging::evict_backed_measured;
use crate::paging::evict_lru_measured;
use crate::paging::page_out_in_progress;
use crate::paging::page_out_measured;
use crate::paging::stop_idle_page_out;
use crate::paging::wait_for_idle_page_out;

pub(crate) async fn hydration_command(
    ctx: &ServerCommandContext<'_>,
    _partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
    req: buck2_cli_proto::HydrationRequest,
) -> buck2_error::Result<buck2_cli_proto::HydrationResponse> {
    let dice = ctx.base_context.tenant.dice_manager.unsafe_dice().dupe();
    let subcommand = HydrationSubcommand::try_from(req.subcommand)?;
    // These wait for the idle page-out before the command takes its DICE admission lease: the
    // idle page-out waits for every transaction to end, so waiting while holding one would never
    // return.
    match subcommand {
        HydrationSubcommand::Status if req.wait => wait_for_idle_page_out().await,
        // A manual page-out or page-in supersedes the idle page-out, and must not use the storage
        // while a cancelled one drains: the drain discards rows not yet written.
        HydrationSubcommand::PageOut | HydrationSubcommand::PageIn => stop_idle_page_out().await,
        _ => {}
    }
    let command = HydrationServerCommand { dice, subcommand };
    let server_ctx: &dyn ServerCommandContextTrait = ctx;
    let start_event = server_ctx
        .command_start_event(buck2_data::HydrationCommandStart {}.into())
        .await?;
    TracingLogFile::refresh()?;
    span_async(start_event, async {
        let result = match subcommand {
            HydrationSubcommand::PageOut | HydrationSubcommand::PageIn => {
                server_ctx
                    .with_dice_ctx_maybe_exclusive(
                        |server_ctx, ctx| {
                            ctx.per_transaction_data()
                                .get_materializer()
                                .log_materializer_state(server_ctx.events());
                            command.execute(server_ctx)
                        },
                        Some("hydration".to_owned()),
                    )
                    .await
            }
            // Keep manual page-out/page-in exclusive without running configuration setup,
            // whose payload reads would age the working set being inspected or reclaimed.
            HydrationSubcommand::Status
            | HydrationSubcommand::Evict
            | HydrationSubcommand::EvictLru => {
                ctx.with_shared_command_lock(command.execute(server_ctx))
                    .await
            }
        };
        let end_event = command_end(&result, buck2_data::HydrationCommandEnd {});
        (result, end_event)
    })
    .await
}

struct HydrationServerCommand {
    dice: Arc<Dice>,
    subcommand: HydrationSubcommand,
}

impl HydrationServerCommand {
    async fn execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
    ) -> buck2_error::Result<buck2_cli_proto::HydrationResponse> {
        match self.subcommand {
            HydrationSubcommand::PageOut => {
                // Never cancelled: holds the exclusive command lock.
                let (result, summary) =
                    page_out_measured(&self.dice, || false, server_ctx.events()).await;
                result?;
                Ok(buck2_cli_proto::HydrationResponse {
                    page_out_summary: Some(summary),
                    ..Default::default()
                })
            }
            HydrationSubcommand::PageIn => {
                self.dice
                    .page_in()
                    .await
                    .map_err(|e| from_any_with_tag(e, ErrorTag::Environment))?;
                Ok(buck2_cli_proto::HydrationResponse::default())
            }
            HydrationSubcommand::Evict | HydrationSubcommand::EvictLru => {
                let evicted = if self.subcommand == HydrationSubcommand::EvictLru {
                    evict_lru_measured(&self.dice).await?
                } else {
                    evict_backed_measured(&self.dice).await
                };
                Ok(buck2_cli_proto::HydrationResponse {
                    summary: Some(format!("evicted {evicted} backed DICE values\n")),
                    ..Default::default()
                })
            }
            HydrationSubcommand::Status => {
                let status = self.dice.pagable_status().await;
                let starlark_serialization_state_bytes = self
                    .dice
                    .pagable_storage_context()
                    .map(starlark_serialization_state_retained_bytes);
                let starlark_deserialization_state_bytes = self
                    .dice
                    .pagable_storage_handle()
                    .as_ref()
                    .map(starlark_deserialization_state_retained_bytes);
                Ok(buck2_cli_proto::HydrationResponse {
                    summary: Some(format_status_summary(
                        &status,
                        page_out_in_progress(),
                        starlark_serialization_state_bytes,
                        starlark_deserialization_state_bytes,
                    )),
                    ..Default::default()
                })
            }
        }
    }
}

fn format_status_summary(
    status: &PagableStatus,
    page_out_in_progress: bool,
    starlark_serialization_state_bytes: Option<usize>,
    starlark_deserialization_state_bytes: Option<usize>,
) -> String {
    // `total_nodes` counts vacant/in-progress nodes too; the rest is "other".
    // saturating_sub guards an underflow the struct invariant already rules out.
    let other = status
        .total_nodes
        .saturating_sub(status.resident_count)
        .saturating_sub(status.paged_out_count);
    let mut summary = format!(
        "DICE hydration: {} nodes ({} resident, {} paged out, {} other; {} page-out candidates, {} backed)\n",
        status.total_nodes,
        status.resident_count,
        status.paged_out_count,
        other,
        status.candidate_count,
        status.backed_count,
    );
    summary.push_str(&format!(
        "idle page-out in progress: {}\n",
        if page_out_in_progress { "yes" } else { "no" }
    ));
    if let Some(bytes) = starlark_serialization_state_bytes {
        summary.push_str(&format!(
            "starlark serialization state retained bytes: {bytes}\n"
        ));
    }
    if let Some(bytes) = starlark_deserialization_state_bytes {
        summary.push_str(&format!(
            "starlark deserialization state retained bytes: {bytes}\n"
        ));
    }
    if !status.by_type.is_empty() {
        summary.push('\n');
        summary.push_str(&format!(
            "{:>12}  {:>12}  {:>12}  {}\n",
            "resident", "paged-out", "backed", "key type"
        ));
        for t in &status.by_type {
            summary.push_str(&format!(
                "{:>12}  {:>12}  {:>12}  {}\n",
                t.resident, t.paged_out, t.backed, t.key_type
            ));
        }
    }
    summary
}
