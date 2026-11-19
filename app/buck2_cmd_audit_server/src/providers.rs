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
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::providers::AuditProvidersCommand;
use buck2_events::dispatch::console_message;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::pattern_parse_and_resolve::parse_and_resolve_provider_labels_with_modifiers_from_cli_args;
use buck2_util::indent::indent;
use dice::DiceComputations;
use dice::DiceTransaction;
use futures::StreamExt;
use futures::stream::FuturesOrdered;

use crate::ServerAuditSubcommand;
use crate::common::target_resolution_config::audit_command_target_resolution_config;

#[async_trait]
impl ServerAuditSubcommand for AuditProvidersCommand {
    async fn server_execute(
        &self,
        server_ctx: &dyn ServerCommandContextTrait,
        stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
        _client_ctx: ClientContext,
    ) -> buck2_error::Result<()> {
        server_ctx
            .with_dice_ctx(move |server_ctx, ctx| {
                server_execute_with_dice(self, server_ctx, stdout, ctx)
            })
            .await
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum AuditProvidersError {
    #[error("Evaluation of at least one target providers failed")]
    AtLeastOneFailed,
}

async fn server_execute_with_dice(
    command: &AuditProvidersCommand,
    server_ctx: &dyn ServerCommandContextTrait,
    mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    ctx: DiceTransaction,
) -> buck2_error::Result<()> {
    let target_resolution_config =
        audit_command_target_resolution_config(&mut ctx.ctx(), &command.target_cfg, server_ctx)
            .await?;

    let provider_labels_with_modifiers =
        parse_and_resolve_provider_labels_with_modifiers_from_cli_args(
            &mut ctx.ctx(),
            &command.patterns,
            server_ctx.working_dir(),
        )
        .await?;

    let mut futs = Vec::new();
    for label_with_modifiers in provider_labels_with_modifiers {
        for configured_providers_label in target_resolution_config
            .get_configured_provider_label_with_modifiers(&mut ctx.ctx(), &label_with_modifiers)
            .await?
        {
            futs.push(DiceComputations::declare_closure(async |ctx| {
                let result = ctx.get_providers(&configured_providers_label).await;
                (configured_providers_label, result)
            }));
        }
    }

    let mut dice = ctx.ctx();
    let mut futs: FuturesOrdered<_> = dice.compute_many(futs).into_iter().collect();

    let mut stdout = stdout.as_writer();
    let mut stderr = server_ctx.stderr()?;

    let mut at_least_one_error = false;
    let mut deferred_output = Vec::new();

    while let Some((target, result)) = futs.next().await {
        match result {
            Ok(v) => {
                let v: FrozenProviderCollectionValue = v.require_compatible()?;

                if command.quiet {
                    writeln!(&mut stdout, "{target}")?
                } else if command.list {
                    let mut provider_names = v.provider_collection().provider_names();
                    // Create a deterministic output.
                    provider_names.sort();
                    write!(
                        &mut stdout,
                        "{}:\n{}",
                        target,
                        indent(
                            "  ",
                            &provider_names
                                .iter()
                                .fold(String::new(), |acc, arg| acc + &format!("- {arg}\n"))
                        )
                    )?;
                } else if command.print_debug {
                    write!(
                        &mut stdout,
                        "{}:\n{}",
                        target,
                        indent("  ", &format!("{:?}", v.provider_collection()))
                    )?;
                } else if !command.provider.is_empty() {
                    let collection = v.provider_collection();
                    let providers = collection
                        .iter_providers()
                        .collect::<buck2_error::Result<Vec<_>>>()?;
                    let mut found_providers = Vec::new();
                    let mut missing_providers = Vec::new();

                    for provider_name in &command.provider {
                        match providers.iter().find(|(id, _)| id.name == *provider_name) {
                            Some((_, provider_value)) => {
                                found_providers.push(format!("{provider_value:#}"));
                            }
                            None => {
                                missing_providers.push(provider_name.as_str());
                            }
                        }
                    }

                    if !missing_providers.is_empty() {
                        let label = if missing_providers.len() == 1 {
                            "provider"
                        } else {
                            "providers"
                        };
                        let mut available = collection.provider_names();
                        available.sort();
                        let available_str = available
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let missing_str = missing_providers
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        console_message(format!(
                            "{}: {label} {missing_str} not found. Available providers: [{available_str}]",
                            target.unconfigured()
                        ));
                    }

                    if !found_providers.is_empty() {
                        let inner = found_providers.join(",\n");
                        deferred_output.push(format!("{}:\n{}\n", target, indent("  ", &inner)));
                    }
                } else {
                    write!(
                        &mut stdout,
                        "{}:\n{}",
                        target,
                        indent("  ", &format!("{:#}", v.provider_collection()))
                    )?;
                }
            }
            Err(e) => {
                write!(
                    &mut stderr,
                    "{}: failed:\n{}",
                    target,
                    indent("  ", &format!("{e:?}"))
                )?;
                at_least_one_error = true;
            }
        }
    }

    for output in &deferred_output {
        write!(&mut stdout, "{output}")?;
    }

    stdout.flush()?;
    stderr.flush()?;

    // FIXME(JakobDegen): We should try preserving error metadata here
    if at_least_one_error {
        Err(AuditProvidersError::AtLeastOneFailed.into())
    } else {
        Ok(())
    }
}
