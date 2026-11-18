/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;
use std::io::Write;

use async_trait::async_trait;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_cli_proto::ClientContext;
use buck2_cmd_audit_client::subtargets::AuditSubtargetsCommand;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::buck2_error;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::ctx::ServerCommandDiceContext;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::pattern_parse_and_resolve::parse_and_resolve_provider_labels_with_modifiers_from_cli_args;
use buck2_server_ctx::stdout_partial_output::StdoutPartialOutput;
use buck2_util::indent::indent;
use dice::DiceTransaction;
use futures::StreamExt;
use futures::stream::FuturesOrdered;

use crate::ServerAuditSubcommand;
use crate::common::target_resolution_config::audit_command_target_resolution_config;

#[async_trait]
impl ServerAuditSubcommand for AuditSubtargetsCommand {
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

async fn server_execute_with_dice(
    command: &AuditSubtargetsCommand,
    server_ctx: &dyn ServerCommandContextTrait,
    mut stdout: PartialResultDispatcher<buck2_cli_proto::StdoutBytes>,
    ctx: DiceTransaction,
) -> buck2_error::Result<()> {
    // TODO(raulgarcia4): Extract function where possible, shares a lot of code with audit providers.
    let target_resolution_config =
        audit_command_target_resolution_config(&mut ctx.ctx(), &command.target_cfg, server_ctx)
            .await?;

    let provider_labels = parse_and_resolve_provider_labels_with_modifiers_from_cli_args(
        &mut ctx.ctx(),
        &command.patterns,
        server_ctx.working_dir(),
    )
    .await?;

    let mut futs = FuturesOrdered::new();

    for label_with_modifiers in provider_labels {
        for providers_label in target_resolution_config
            .get_configured_provider_label_with_modifiers(&mut ctx.ctx(), &label_with_modifiers)
            .await?
        {
            // `.push` is deprecated in newer `futures`,
            // but we did not updated vendored `futures` yet.
            let ctx = ctx.clone();
            #[allow(deprecated)]
            futs.push(async move {
                let result = ctx.ctx().get_providers(&providers_label).await;
                (providers_label, result)
            });
        }
    }

    let mut stdout = stdout.as_writer();
    let mut stderr = server_ctx.stderr()?;
    let recursive = !command.shallow;
    let json_format = command.json;
    let mut subtargets_map = serde_json::Map::new();

    let mut at_least_one_evaluation_error = false;
    while let Some((target, result)) = futs.next().await {
        match result {
            Ok(v) => {
                if recursive {
                    if json_format {
                        fn serialize_nested_subtargets(
                            providers: &ProviderCollection<'_>,
                        ) -> buck2_error::Result<serde_json::Value> {
                            let mut entries = serde_json::Map::new();
                            for (subtarget, providers) in
                                providers.default_info()?.sub_targets()?.iter()
                            {
                                entries.insert(
                                    subtarget.to_string(),
                                    serialize_nested_subtargets(providers)?,
                                );
                            }
                            Ok(serde_json::Value::Object(entries))
                        }
                        subtargets_map.insert(
                            target.to_string(),
                            serialize_nested_subtargets(
                                v.require_compatible()?.provider_collection(),
                            )?,
                        );
                    } else {
                        fn recursive_iterate(
                            providers: &ProviderCollection<'_>,
                            stdout: &mut StdoutPartialOutput,
                            label: &mut Subtarget,
                        ) -> buck2_error::Result<()> {
                            for (subtarget, providers) in
                                providers.default_info()?.sub_targets()?.iter()
                            {
                                label.push(subtarget.to_string());
                                writeln!(stdout, "{label}")?;
                                recursive_iterate(providers, stdout, label)?;
                                label.pop();
                            }
                            Ok(())
                        }
                        recursive_iterate(
                            v.require_compatible()?.provider_collection(),
                            &mut stdout,
                            &mut Subtarget::new(target),
                        )?
                    }
                } else if json_format {
                    let mut entries = serde_json::Map::new();
                    for sub in v
                        .require_compatible()?
                        .provider_collection()
                        .default_info()?
                        .sub_targets()?
                        .keys()
                    {
                        entries.insert(
                            sub.to_string(),
                            serde_json::Value::Object(Default::default()),
                        );
                    }
                    subtargets_map.insert(target.to_string(), serde_json::Value::Object(entries));
                } else {
                    let mut label = Subtarget::new(target);
                    for sub in v
                        .require_compatible()?
                        .provider_collection()
                        .default_info()?
                        .sub_targets()?
                        .keys()
                    {
                        label.push(sub.to_string());
                        writeln!(&mut stdout, "{label}")?;
                        label.pop();
                    }
                }
            }
            Err(e) => {
                write!(
                    &mut stderr,
                    "{}: failed:\n{}",
                    target,
                    indent("  ", &format!("{e:?}"))
                )?;
                at_least_one_evaluation_error = true;
            }
        }
    }

    if json_format {
        write!(
            &mut stdout,
            "{}",
            serde_json::to_string_pretty(&subtargets_map)?
        )?;
    }

    stdout.flush()?;
    stderr.flush()?;

    if at_least_one_evaluation_error {
        Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "Evaluation of at least one target provider failed"
        ))
    } else {
        Ok(())
    }
}

struct Subtarget {
    target: ConfiguredProvidersLabel,
    subtargets: Vec<String>,
}

impl Subtarget {
    fn new(target: ConfiguredProvidersLabel) -> Self {
        Self {
            target,
            subtargets: Vec::new(),
        }
    }

    fn push(&mut self, subtarget: String) {
        self.subtargets.push(subtarget);
    }

    fn pop(&mut self) {
        self.subtargets.pop();
    }
}

impl Display for Subtarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let subtargets = self
            .subtargets
            .iter()
            .map(|s| format!("[{s}]"))
            .collect::<Vec<_>>()
            .join("");
        write!(
            f,
            "{}{} ({})",
            self.target.unconfigured(),
            subtargets,
            self.target.cfg()
        )?;
        Ok(())
    }
}
