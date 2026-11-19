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
use std::path::Path;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::file_package::AuditFilePackageCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::package_listing::dice::DicePackageListingResolver;
use buck2_common::package_listing::resolver::PackageListingResolver;
use buck2_core::fs::project::ProjectRoot;
use buck2_fs::working_dir::AbsWorkingDir;
use buck2_hash::BuckMutInsertionMap;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dice::DiceComputations;

use crate::ServerAuditSubcommand;

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Package {
    Package(String),
    Error(String),
}

#[async_trait]
impl ServerAuditSubcommand for AuditFilePackageCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let cwd_abs = server_ctx.working_dir_abs();
                let project_root = server_ctx.project_root();

                let mappings =
                    audit_build_package(&mut ctx.ctx(), &self.paths, cwd_abs, project_root).await?;

                let mut stdout = stdout.as_writer();
                if self.json {
                    writeln!(stdout, "{}", serde_json::to_string_pretty(&mappings)?)?;
                } else {
                    for (path, res) in &mappings {
                        match res {
                            Package::Package(package) => writeln!(stdout, "{path}: {package}")?,
                            Package::Error(error) => writeln!(stdout, "{path}: Error: {error}")?,
                        }
                    }
                }

                Ok(())
            })
            .await?)
    }
}

async fn audit_build_package(
    ctx: &mut DiceComputations<'_>,
    paths: &[String],
    cwd_abs: &AbsWorkingDir,
    project_root: &ProjectRoot,
) -> buck2_error::Result<BuckMutInsertionMap<String, Package>> {
    let cells = ctx.get_cell_resolver().await?;

    let mut mappings = BuckMutInsertionMap::default();

    for path_str in paths {
        // Resolve path to absolute (handles both relative and absolute inputs),
        // then convert to cell-relative path and get enclosing package.
        // Errors are caught and returned in the mapping.
        let abs_path = cwd_abs.resolve(Path::new(path_str));
        let val = match cells.get_cell_path_from_abs_path(&abs_path, project_root) {
            Ok(cell_path) => {
                match DicePackageListingResolver(ctx)
                    .get_enclosing_package(cell_path)
                    .await
                {
                    Ok(package) => Package::Package(package.to_string()),
                    Err(e) => Package::Error(format!("{e}")),
                }
            }
            Err(e) => Package::Error(format!("{e}")),
        };

        mappings.insert(path_str.clone(), val);
    }

    Ok(mappings)
}
