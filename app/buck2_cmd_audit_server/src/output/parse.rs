/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::output::parse::AuditParseCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dupe::Dupe;

use super::buck_out_path_parser::BuckOutPathParser;
use super::buck_out_path_type_printer::BuckOutPathTypePrinter;
use crate::ServerAuditSubcommand;

#[async_trait]
impl ServerAuditSubcommand for AuditParseCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        server_ctx
            .with_dice_ctx(|_server_ctx, dice_ctx| async move {
                let cell_resolver = dice_ctx.ctx().get_cell_resolver().await?;
                let buck_out_parser = BuckOutPathParser::new(cell_resolver.dupe());
                let parsed_path = buck_out_parser.parse(&self.output_path)?;

                let printer = BuckOutPathTypePrinter::new(self.json, &self.output_attribute)?;

                let stdout = stdout.as_writer();

                printer.print(&parsed_path, stdout)
            })
            .await
    }
}
