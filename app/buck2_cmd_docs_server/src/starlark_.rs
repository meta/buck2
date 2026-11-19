/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;

use buck2_cli_proto::new_generic::DocsOutputFormat;
use buck2_cli_proto::new_generic::DocsResponse;
use buck2_cli_proto::new_generic::DocsStarlarkRequest;
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::bxl::BxlFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::cells::name::CellName;
use buck2_hash::BuckMutSet;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::parse_import::ParseImportOptions;
use buck2_interpreter::parse_import::RelativeImports;
use buck2_interpreter::parse_import::parse_import_with_config;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use dice::DiceTransaction;
use starlark::docs::multipage::DocModuleInfo;

use crate::builtins::write_docs_to_subdir;
use crate::json;

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub(crate) enum StarlarkFilePath {
    Bzl(ImportPath),
    Bxl(BxlFilePath),
}

impl StarlarkFilePath {
    fn to_starlark_module_path(&self) -> StarlarkModulePath<'_> {
        match self {
            StarlarkFilePath::Bzl(import_path) => StarlarkModulePath::LoadFile(import_path),
            StarlarkFilePath::Bxl(bxl_path) => StarlarkModulePath::BxlFile(bxl_path),
        }
    }

    pub(crate) fn cell(&self) -> CellName {
        match self {
            StarlarkFilePath::Bzl(import_path) => import_path.cell(),
            StarlarkFilePath::Bxl(bxl_file_path) => bxl_file_path.cell(),
        }
    }

    pub(crate) fn path(&self) -> &CellPath {
        match self {
            StarlarkFilePath::Bzl(import_path) => import_path.path(),
            StarlarkFilePath::Bxl(bxl_file_path) => bxl_file_path.path(),
        }
    }
}

impl std::fmt::Display for StarlarkFilePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StarlarkFilePath::Bzl(import_path) => write!(f, "{import_path}"),
            StarlarkFilePath::Bxl(bxl_file_path) => write!(f, "{bxl_file_path}"),
        }
    }
}

fn parse_starlark_paths(
    cell_resolver: &CellAliasResolver,
    current_dir: &CellPath,
    symbol_patterns: &[String],
) -> buck2_error::Result<BuckMutSet<StarlarkFilePath>> {
    let parse_options = ParseImportOptions {
        allow_missing_at_symbol: true,
        relative_import_option: RelativeImports::Allow {
            current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                *current_dir,
                None,
            ),
        },
    };
    let current_cell = BuildFileCell::new(current_dir.cell());

    symbol_patterns
        .iter()
        .map(|symbol_pattern| {
            let path = parse_import_with_config(cell_resolver, symbol_pattern, &parse_options)?;
            if path.path().extension() == Some("bxl") {
                Ok(StarlarkFilePath::Bxl(BxlFilePath::new(path)?))
            } else {
                Ok(StarlarkFilePath::Bzl(
                    ImportPath::new_with_build_file_cells(path, current_cell)?,
                ))
            }
        })
        .collect()
}

pub(crate) async fn docs_starlark(
    server_ctx: &dyn ServerCommandContextTrait,
    dice_ctx: DiceTransaction,
    request: &DocsStarlarkRequest,
) -> buck2_error::Result<DocsResponse> {
    let cell_resolver = dice_ctx.ctx().get_cell_resolver().await?;
    let cwd = server_ctx.working_dir();
    let current_cell_path = cell_resolver.get_cell_path(cwd);
    let cell_alias_resolver = dice_ctx
        .ctx()
        .get_cell_alias_resolver(current_cell_path.cell())
        .await?;

    let lookups = parse_starlark_paths(
        &cell_alias_resolver,
        &current_cell_path,
        &request.symbol_patterns,
    )?;

    let docs: Vec<_> = dice_ctx
        .ctx()
        .try_compute_join(lookups, async |ctx, path| {
            let doc = ctx
                .get_loaded_module(path.to_starlark_module_path())
                .await?
                .env()
                .documentation();
            buck2_error::Ok((path, doc))
        })
        .await?;

    let response = match &request.format {
        DocsOutputFormat::Json => DocsResponse::Json(json::to_json(docs)?),
        DocsOutputFormat::Markdown(output_dir) => {
            let mut render_signature_at_bottom = false;
            let module_infos = docs
                .iter()
                .map(|(path, doc)| {
                    let path = PathBuf::from(path.cell().as_str())
                        .join(path.path().path().as_forward_relative_path().as_path());
                    let path = path.to_str().map_or("".to_owned(), |s| s.to_owned());
                    if path.contains("rules.bzl") {
                        render_signature_at_bottom = true;
                    }

                    DocModuleInfo {
                        module: doc,
                        name: "".to_owned(),
                        page_path: path,
                    }
                })
                .collect();

            write_docs_to_subdir(
                module_infos,
                output_dir.to_str()?,
                None,
                render_signature_at_bottom,
            )?;
            DocsResponse::NoOutput
        }
    };

    Ok(response)
}
