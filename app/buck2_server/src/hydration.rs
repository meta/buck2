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

use async_trait::async_trait;
use buck2_cli_proto::HydrationSubcommand;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::template::ServerCommandTemplate;
use buck2_server_ctx::template::run_server_command;
use dice::Dice;
use dice::DiceTransaction;
use dice::PagableStatus;
use dupe::Dupe;
use starlark::pagable::starlark_deserialization_state_retained_bytes;
use starlark::pagable::starlark_serialization_state_retained_bytes;

use crate::ctx::ServerCommandContext;
use crate::paging::evict_backed_measured;
use crate::paging::page_out_in_progress;
use crate::paging::page_out_measured;
use crate::paging::stop_idle_page_out;
use crate::paging::wait_for_idle_page_out;

pub(crate) async fn hydration_command(
    ctx: &ServerCommandContext<'_>,
    partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>,
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
    run_server_command(
        HydrationServerCommand { dice, subcommand },
        ctx,
        partial_result_dispatcher,
    )
    .await
}

struct HydrationServerCommand {
    dice: Arc<Dice>,
    subcommand: HydrationSubcommand,
}

#[async_trait]
impl ServerCommandTemplate for HydrationServerCommand {
    type StartEvent = buck2_data::HydrationCommandStart;
    type EndEvent = buck2_data::HydrationCommandEnd;
    type Response = buck2_cli_proto::HydrationResponse;
    type PartialResult = NoPartialResult;

    fn exclusive_command_name(&self) -> Option<String> {
        match self.subcommand {
            HydrationSubcommand::PageOut | HydrationSubcommand::PageIn => {
                Some("hydration".to_owned())
            }
            HydrationSubcommand::Status | HydrationSubcommand::Evict => None,
        }
    }

    async fn command(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        _partial_result_dispatcher: PartialResultDispatcher<Self::PartialResult>,
        _ctx: DiceTransaction,
    ) -> buck2_error::Result<Self::Response> {
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
            HydrationSubcommand::Evict => {
                let evicted = evict_backed_measured(&self.dice).await;
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
