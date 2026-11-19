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
use std::sync::Arc;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_starlark_client::lint::StarlarkLintCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::file_ops::dice::DiceFileComputations;
use buck2_common::file_ops::error::FileReadErrorContext;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_hash::BuckMutMap;
use buck2_hash::IntentionallyStdHashSet;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_interpreter::paths::path::StarlarkPath;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dice::DiceTransaction;
use dupe::Dupe;
use dupe::OptionDupedExt;
use starlark::analysis::AstModuleLint;
use starlark::codemap::FileSpan;
use starlark::errors::EvalSeverity;
use starlark::errors::Lint;
use starlark::syntax::AstModule;

use crate::StarlarkServerSubcommand;
use crate::util::environment::Environment;
use crate::util::paths::starlark_files;

/// The cache of names for a path, keyed by its CellName and its path type.
struct Cache<'a> {
    dice: &'a DiceTransaction,
    cached: BuckMutMap<(CellName, StarlarkFileType), Arc<IntentionallyStdHashSet<String>>>,
}

impl<'a> Cache<'a> {
    pub(crate) fn new(dice: &'a DiceTransaction) -> Cache<'a> {
        Self {
            dice,
            cached: BuckMutMap::default(),
        }
    }

    pub(crate) async fn get_names(
        &mut self,
        path: &StarlarkPath<'_>,
    ) -> buck2_error::Result<Arc<IntentionallyStdHashSet<String>>> {
        let path_type = path.file_type();
        let cell = path.cell();
        if let Some(res) = self.cached.get(&(cell, path_type)) {
            return Ok(res.dupe());
        }
        let env: Environment = Environment::new(cell, path_type, &mut self.dice.ctx()).await?;
        let res = Arc::new(env.get_names(path_type, self.dice).await?);
        self.cached.insert((cell, path_type), res.dupe());
        Ok(res)
    }
}

async fn lint_file(
    path: &StarlarkPath<'_>,
    cell_resolver: &CellResolver,
    cache: &mut Cache<'_>,
) -> buck2_error::Result<Vec<Lint>> {
    let dialect = path.file_type().dialect(false);
    let proj_path = cell_resolver.resolve_ipath(*path.path())?;
    let path_str = proj_path.to_string();
    let content = DiceFileComputations::read_file(&mut cache.dice.ctx(), *path.path())
        .await
        .without_package_context_information()?;
    match AstModule::parse(&path_str, content.clone(), &dialect) {
        Ok(ast) => Ok(ast.lint(Some(&*cache.get_names(path).await?))),
        Err(err) => {
            // There was a parse error, so we don't want to fail, we want to give a nice error message
            // Do the best we can - it is probably a `Diagnostic`, which gives us more precise info.
            Ok(vec![Lint {
                location: err
                    .span()
                    .duped()
                    .unwrap_or_else(|| FileSpan::new(path_str, content)),
                short_name: "parse_error".to_owned(),
                severity: EvalSeverity::Error,
                problem: format!("{:#}", err.without_diagnostic()),
                original: "".to_owned(),
            }])
        }
    }
}

#[async_trait]
impl StarlarkServerSubcommand for StarlarkLintCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let cell_resolver = &ctx.ctx().get_cell_resolver().await?;

                let mut stdout = stdout.as_writer();
                let mut lint_count = 0;
                let files = starlark_files(
                    &mut ctx.ctx(),
                    &self.paths,
                    &self.exclude,
                    server_ctx,
                    cell_resolver,
                )
                .await?;
                let mut cache = Cache::new(&ctx);

                for file in &files {
                    let lints = lint_file(&file.borrow(), cell_resolver, &mut cache).await?;
                    lint_count += lints.len();
                    for lint in lints {
                        writeln!(stdout, "{lint}")?;
                    }
                }
                if lint_count > 0 {
                    Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Found {} lints",
                        lint_count
                    ))
                } else {
                    writeln!(
                        server_ctx.stderr()?,
                        "Found no lints in {} files",
                        files.len()
                    )?;
                    buck2_error::Ok(())
                }
            })
            .await
    }
}
