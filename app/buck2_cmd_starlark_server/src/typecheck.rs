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

use async_recursion::async_recursion;
use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_starlark_client::typecheck::StarlarkTypecheckCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::dice::data::HasIoProvider;
use buck2_common::io::IoProvider;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_hash::BuckMutMap;
use buck2_hash::IntentionallyStdHashMap;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_interpreter::paths::module::OwnedStarlarkModulePath;
use buck2_interpreter::paths::path::OwnedStarlarkPath;
use buck2_interpreter_for_build::interpreter::dice_calculation_delegate::HasCalculationDelegate;
use buck2_interpreter_for_build::interpreter::interpreter_for_dir::ParseData;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dice::DiceTransaction;
use dupe::Dupe;
use starlark::environment::Globals;
use starlark::typing::AstModuleTypecheck;
use starlark::typing::Interface;

use crate::StarlarkServerSubcommand;
use crate::util::environment::Environment;
use crate::util::paths::starlark_files;

struct Cache<'a> {
    // Things we have access to get information
    dice: &'a DiceTransaction,
    io: &'a dyn IoProvider,
    cell_resolver: &'a CellResolver,
    // Things we have access to write information
    stdout: &'a mut (dyn Write + Send + Sync),
    stderr: &'a mut (dyn Write + Send + Sync),
    // Our accumulated state
    oracle: BuckMutMap<(CellName, StarlarkFileType), Globals>,
    cache: BuckMutMap<OwnedStarlarkModulePath, Interface>,
}

impl Cache<'_> {
    async fn typecheck(&mut self, path: OwnedStarlarkPath) -> buck2_error::Result<()> {
        self.run(path).await?;
        Ok(())
    }

    async fn get_oracle(
        &mut self,
        cell: CellName,
        path_type: StarlarkFileType,
    ) -> buck2_error::Result<Globals> {
        match self.oracle.get(&(cell, path_type)) {
            Some(g) => Ok(g.dupe()),
            None => {
                let env = Environment::new(cell, path_type, &mut self.dice.ctx()).await?;
                let globals = env
                    .globals_with_implicit_symbols(path_type, self.dice)
                    .await?;
                self.oracle.insert((cell, path_type), globals.dupe());
                Ok(globals)
            }
        }
    }

    async fn get(&mut self, path: OwnedStarlarkModulePath) -> buck2_error::Result<Interface> {
        match self.cache.get(&path) {
            Some(x) => Ok(x.dupe()),
            None => {
                let res = self.run(path.clone().into_starlark_path()).await?;
                self.cache.insert(path, res.dupe());
                Ok(res)
            }
        }
    }

    #[async_recursion]
    async fn run(&mut self, path: OwnedStarlarkPath) -> buck2_error::Result<Interface> {
        let path_ref = path.borrow();
        writeln!(self.stderr, "Type checking: {path_ref}")?;
        let proj_path = self.cell_resolver.resolve_ipath(*path_ref.path())?;
        let path_str = proj_path.to_string();
        let src = self
            .io
            .read_file_if_exists(proj_path)
            .await?
            .with_internal_error(|| format!("File not found: `{path_str}`"))?;

        let mut dice = self.dice.ctx();
        let interp = dice
            .get_interpreter_calculator(OwnedStarlarkPath::new(path_ref))
            .await?;

        let ParseData(ast, _) = interp.prepare_eval_with_content(path_ref, src)??;
        let mut loads = IntentionallyStdHashMap::default();
        for x in ast.loads() {
            let y = interp.resolve_load(path_ref, x.module_id).await?;
            let interface = self.get(y).await?;
            loads.insert(x.module_id.to_owned(), interface);
        }
        let globals = self
            .get_oracle(path_ref.cell(), path_ref.file_type())
            .await?;
        let (errors, bindings, interface, approxiomations) = ast.typecheck(&globals, &loads);

        if !approxiomations.is_empty() {
            writeln!(self.stderr, "\n\nAPPROXIMATIONS:")?;
            for x in approxiomations {
                writeln!(self.stderr, "{x}")?;
            }
        }

        writeln!(self.stderr, "\n\nBINDINGS:\n{bindings}")?;

        let errors_count = errors.len();
        if errors_count == 0 {
            Ok(interface)
        } else {
            writeln!(self.stdout, "\n\nERRORS:")?;
            for x in errors {
                writeln!(self.stdout, "{x}")?;
            }
            Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Detected {errors_count} errors"
            ))
        }
    }
}

#[async_trait]
impl StarlarkServerSubcommand for StarlarkTypecheckCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, dice| async move {
                let cell_resolver = &dice.ctx().get_cell_resolver().await?;
                let io = &dice.global_data().get_io_provider();

                let files = starlark_files(
                    &mut dice.ctx(),
                    &self.paths,
                    &[],
                    server_ctx,
                    cell_resolver,
                    &**io,
                )
                .await?;
                let mut stdout = stdout.as_writer();
                let mut stderr = server_ctx.stderr()?;
                let mut cache = Cache {
                    dice: &dice,
                    io: &**io,
                    cell_resolver,
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                    oracle: BuckMutMap::default(),
                    cache: BuckMutMap::default(),
                };
                for file in files {
                    cache.typecheck(file).await?;
                }
                let file_count = cache.cache.len();
                writeln!(stderr, "Found no type errors in {file_count} files")?;
                Ok(())
            })
            .await?)
    }
}
