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
use std::time::Instant;

use buck2_build_api::build::graph_properties::debug_compute_configured_graph_properties_uncached;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::perf::configured_graph_size::ConfiguredGraphSizeCommand;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_hash::BuckMutInsertionMap;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dupe::Dupe;
use serde::Serialize;

use crate::common::configured_target_labels::audit_command_configured_target_labels;

pub(crate) async fn server_execute(
    command: &ConfiguredGraphSizeCommand,
    server_ctx: &dyn ServerCommandContextTrait,
    mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    _client_ctx: ClientContext,
) -> buck2_error::Result<()> {
    server_ctx
        .with_dice_ctx(|server_ctx, ctx| async move {
            let targets = audit_command_configured_target_labels(
                &mut ctx.ctx(),
                &command.patterns,
                &command.target_cfg,
                server_ctx,
            )
            .await?;

            #[derive(Serialize)]
            struct Res {
                configured_graph_size: u64,
                sketch: Option<String>,
                duration_ms: u64,
            }
            let mut results = BuckMutInsertionMap::default();

            // We intentionally don't do this in parallel so that we can get the computation time for them.
            for target in &targets {
                let MaybeCompatible::Compatible(node) = ctx
                    .ctx()
                    .get_configured_target_node(target)
                    .await
                    .ok()?
                    .map(|n| n.dupe())
                else {
                    continue;
                };
                let now = Instant::now();
                let props =
                    debug_compute_configured_graph_properties_uncached(node, command.sketch);
                let duration = Instant::now() - now;
                results.insert(
                    target,
                    Res {
                        configured_graph_size: props.configured_graph_size,
                        sketch: props.configured_graph_sketch.map(|v| v.serialize()),
                        duration_ms: duration.as_millis() as u64,
                    },
                );
            }

            let mut stdout = stdout.as_writer();
            if command.json {
                writeln!(stdout, "{}", serde_json::to_string_pretty(&results)?)?;
            } else {
                for (target, res) in results {
                    writeln!(stdout, "{target}")?;
                    writeln!(
                        stdout,
                        "  Configured graph size: {}",
                        res.configured_graph_size
                    )?;
                    writeln!(stdout, "  Compute Duration: {}ms", res.duration_ms)?;
                    writeln!(stdout)?;
                }
            }
            Ok(())
        })
        .await
}
