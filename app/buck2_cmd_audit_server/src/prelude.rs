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
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::prelude::AuditPreludeCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_interpreter::load_module::INTERPRETER_CALCULATION_IMPL;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::prelude_path::prelude_path;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;

use crate::ServerAuditSubcommand;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum AuditPreludeError {
    #[error("Project has no prelude")]
    NoPrelude,
}

#[async_trait]
impl ServerAuditSubcommand for AuditPreludeCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|_server_ctx, ctx| async move {
                let mut stdout = stdout.as_writer();
                // Print out all the Prelude-like stuff that is loaded into each module
                let cell_resolver = ctx.ctx().get_cell_resolver().await?;
                let Some(prelude_path) = prelude_path(&cell_resolver)? else {
                    return Err(AuditPreludeError::NoPrelude.into());
                };
                writeln!(
                    stdout,
                    "{}",
                    INTERPRETER_CALCULATION_IMPL
                        .get()?
                        .global_env(&mut ctx.ctx())
                        .await?
                        .describe()
                )?;
                writeln!(
                    stdout,
                    "{}",
                    ctx.ctx()
                        .get_loaded_module_from_import_path(prelude_path.import_path())
                        .await?
                        .env()
                        .describe()
                )?;

                Ok(())
            })
            .await?)
    }
}
