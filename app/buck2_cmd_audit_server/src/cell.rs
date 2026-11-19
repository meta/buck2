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
use buck2_build_api::audit_cell::AUDIT_CELL;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::cell::AuditCellCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dice::DiceComputations;
use futures::FutureExt;

use crate::ServerAuditSubcommand;

#[async_trait]
impl ServerAuditSubcommand for AuditCellCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let fs = server_ctx.project_root();
                let cwd = server_ctx.working_dir();

                let mappings = audit_cell(
                    &mut ctx.ctx(),
                    &self.aliases_to_resolve,
                    self.aliases,
                    cwd,
                    fs,
                )
                .await?;

                let mut stdout = stdout.as_writer();
                if self.paths_only {
                    if self.json {
                        let paths: Vec<_> = mappings.values().collect();
                        writeln!(stdout, "{}", serde_json::to_string_pretty(&paths)?)?;
                    } else {
                        for v in mappings.values() {
                            writeln!(stdout, "{v}")?;
                        }
                    }
                } else if self.json {
                    writeln!(stdout, "{}", serde_json::to_string_pretty(&mappings)?)?;
                } else {
                    for (k, v) in mappings {
                        writeln!(stdout, "{k}: {v}")?;
                    }
                }

                Ok(())
            })
            .await?)
    }
}

pub(crate) async fn audit_cell(
    ctx: &mut DiceComputations<'_>,
    aliases_to_resolve: &[String],
    aliases: bool,
    cwd: &ProjectRelativePath,
    fs: &ProjectRoot,
) -> buck2_error::Result<BuckMutInsertionMap<String, AbsNormPathBuf>> {
    let cells = ctx.get_cell_resolver().await?;
    let this_resolver = ctx.get_cell_alias_resolver_for_dir(cwd).await?;
    let mappings: BuckMutInsertionMap<_, _> = {
        if aliases_to_resolve.is_empty() {
            if aliases {
                this_resolver
                    .mappings()
                    .map(|(alias, cell_name)| {
                        (
                            alias.to_string(),
                            fs.resolve(
                                cells
                                    .get(cell_name)
                                    .unwrap()
                                    .path()
                                    .as_project_relative_path(),
                            ),
                        )
                    })
                    .collect()
            } else {
                cells
                    .cells()
                    .map(|(name, cell)| {
                        (
                            name.as_str().to_owned(),
                            fs.resolve(cell.path().as_project_relative_path()),
                        )
                    })
                    .collect()
            }
        } else {
            aliases_to_resolve
                .iter()
                .map(|alias| {
                    Ok((
                        alias.to_owned(),
                        fs.resolve(
                            cells
                                .get(this_resolver.resolve(alias)?)
                                .unwrap()
                                .path()
                                .as_project_relative_path(),
                        ),
                    ))
                })
                .collect::<buck2_error::Result<_>>()?
        }
    };
    Ok(mappings)
}

pub(crate) fn init_audit_cell() {
    AUDIT_CELL.init(|ctx, aliases_to_resolve, aliases, cwd, fs| {
        audit_cell(ctx, aliases_to_resolve, aliases, cwd, fs).boxed()
    });
}
