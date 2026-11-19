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
use buck2_build_api::audit_dep_files::AUDIT_DEP_FILES;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::dep_files::AuditDepFilesCommand;
use buck2_common::pattern::parse_from_cli::parse_patterns_from_cli_args;
use buck2_core::category::CategoryRef;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_error::BuckErrorOptionContext;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;

use crate::ServerAuditSubcommand;

#[async_trait]
impl ServerAuditSubcommand for AuditDepFilesCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, ctx| async move {
                let global_cfg_options = global_cfg_options_from_client_context(
                    &self.target_cfg.target_cfg(),
                    server_ctx,
                    &mut ctx.ctx(),
                )
                .await?;

                let label = parse_patterns_from_cli_args::<TargetPatternExtra>(
                    &mut ctx.ctx(),
                    std::slice::from_ref(&self.pattern),
                    server_ctx.working_dir(),
                )
                .await?
                .into_iter()
                .next()
                .internal_error("Parsing patterns returned nothing")?
                .as_target_label(&self.pattern)?;

                let label = ctx
                    .ctx()
                    .get_configured_target_post_transition(&label, &global_cfg_options)
                    .await?;

                let category = CategoryRef::new(self.category.as_str())?.to_owned();

                (AUDIT_DEP_FILES.get()?)(
                    &ctx,
                    label,
                    category,
                    self.identifier.clone(),
                    &mut stdout.as_writer(),
                )
                .await?;

                Ok(())
            })
            .await?)
    }
}
