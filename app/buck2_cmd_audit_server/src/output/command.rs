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
use buck2_build_api::actions::query::FIND_MATCHING_ACTION;
use buck2_build_api::actions::query::PRINT_ACTION_NODE;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::audit_output::AUDIT_OUTPUT;
use buck2_build_api::audit_output::AuditOutputResult;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::output::command::AuditOutputCommand;
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::cells::CellResolver;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::global_cfg_options::global_cfg_options_from_client_context;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use dice::DiceComputations;
use dupe::Dupe;

use crate::ServerAuditSubcommand;
use crate::output::buck_out_path_parser::BuckOutPathParser;
use crate::output::buck_out_path_parser::BuckOutPathType;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum AuditOutputError {
    #[error(
        "BXL, anonymous target, test, and tmp artifacts are not supported for audit output. Only rule output artifacts are supported. Path: `{0}`"
    )]
    UnsupportedPathType(String),
    #[error(
        "Invalid path {0} - failed to find either a configuration hash or a content hash in the path. Please make sure that the path is a buck-out path."
    )]
    InvalidPathNoHashFound(String),
    #[error("Invalid path {0} - found both a configuration hash and a content hash in the path!")]
    InvalidPathTwoHashesFound(String),
}

async fn audit_output<'v>(
    output_path: &'v str,
    working_dir: &'v ProjectRelativePath,
    cell_resolver: CellResolver,
    dice_ctx: &'v mut DiceComputations<'_>,
    global_cfg_options: &'v GlobalCfgOptions,
) -> buck2_error::Result<Option<AuditOutputResult>> {
    let buck_out_parser = BuckOutPathParser::new(cell_resolver);
    let parsed = buck_out_parser.parse(output_path)?;

    let (target_label, config_hash, content_hash, short_path) = match parsed {
        BuckOutPathType::RuleOutput {
            target_label,
            common_attrs,
            short_path,
            ..
        } => (
            target_label,
            common_attrs.config_hash,
            common_attrs.content_hash,
            short_path,
        ),
        _ => {
            return Err(AuditOutputError::UnsupportedPathType(output_path.to_owned()).into());
        }
    };

    let config_hash = match (config_hash, content_hash) {
        (Some(config_hash), None) => config_hash,
        (None, Some(_content_hash)) => {
            return Ok(Some(AuditOutputResult::MatchContentBasedPath(target_label)));
        }
        (None, None) => {
            return Err(AuditOutputError::InvalidPathNoHashFound(output_path.to_owned()).into());
        }
        (Some(_), Some(_)) => {
            return Err(AuditOutputError::InvalidPathTwoHashesFound(output_path.to_owned()).into());
        }
    };

    let configured_target_label = dice_ctx
        .get_configured_target(&target_label, global_cfg_options)
        .await?;

    let command_config = configured_target_label.cfg();
    let command_config_hash = command_config.output_hash();
    if command_config_hash.as_str() != config_hash {
        return Ok(Some(
            AuditOutputResult::MaybeRelevantForConfigurationHashPath(target_label),
        ));
    }

    let analysis = dice_ctx
        .get_analysis_result(&configured_target_label)
        .await
        .require_compatible()?;

    Ok(FIND_MATCHING_ACTION.get()?(
        dice_ctx,
        working_dir,
        global_cfg_options,
        analysis,
        short_path,
    )
    .await?
    .map(AuditOutputResult::Match))
}

pub(crate) fn init_audit_output() {
    AUDIT_OUTPUT.init(
        |output_path, working_dir, cell_resolver, dice_ctx, global_cfg_options| {
            Box::pin(audit_output(
                output_path,
                working_dir,
                cell_resolver.dupe(),
                dice_ctx,
                global_cfg_options,
            ))
        },
    );
}

#[async_trait]
impl ServerAuditSubcommand for AuditOutputCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        Ok(server_ctx
            .with_dice_ctx(|server_ctx, dice_ctx| async move {
                // First, we parse the buck-out path to get a target label. Next, we configure the target
                // label and run analysis on it to get the `DeferredTable`. Then, we iterate through the
                // deferred table's entries and look at their build outputs (if they have any) to try to
                // match the inputted buck-out path with the build output's buck-out path. Once we find
                // a matching path, we create the `ActionQueryNode` from the action key associated with the
                // matching build output, and print out the result.

                let working_dir = server_ctx.working_dir();
                let cell_resolver = dice_ctx.ctx().get_cell_resolver().await?;

                let global_cfg_options = global_cfg_options_from_client_context(
                    &self.target_cfg.target_cfg(),
                    server_ctx,
                    &mut dice_ctx.ctx(),
                )
                .await?;

                let result = audit_output(&self.output_path, working_dir, cell_resolver.dupe(), &mut dice_ctx.ctx(), &global_cfg_options).await?;

                let mut stdout = stdout.as_writer();

                match result {
                    Some(result) => {
                        match result {
                            AuditOutputResult::Match(action) => {
                                (PRINT_ACTION_NODE.get()?)(&mut stdout, action, self.json, &self.query_attributes.get()?, &cell_resolver).await?
                            },
                            AuditOutputResult::MaybeRelevantForConfigurationHashPath(label) => {
                                writeln!(
                                    stdout,
                                    "Platform configuration of the buck-out path did not match the one used to invoke this command. Returning the most relevant unconfigured target label for the buck-out path: {label}"
                                )?;
                            },
                            AuditOutputResult::MatchContentBasedPath(label) => {
                                writeln!(stdout, "{label}")?;
                            },
                        }
                    },
                    None => {
                        // If we get here, that means we failed to find any matching actions
                        writeln!(
                            stdout,
                            "Failed to find an action that produced the output path. Make sure that you did not input a symlinked buck-out path."
                        )?;
                    }
                }

                Ok(())
            })
            .await?)
    }
}
