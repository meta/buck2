/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_agent_interactive::Advice;
use buck2_agent_interactive::Decision;
use buck2_agent_interactive::Invocation;
use buck2_build_api::build;
use buck2_build_api::build::BuildConfiguredLabelOptions;
use buck2_build_api::build::BuildEvent;
use buck2_build_api::build::BuildEventConsumer;
use buck2_build_api::build::ConfiguredBuildEventVariant;
use buck2_build_api::build::ProvidersToBuild;
use buck2_build_api::build::outputs::get_outputs_for_top_level_target;
use buck2_build_api::materialize::MaterializationAndUploadContext;
use buck2_cli_proto::BuildRequest;
use buck2_cli_proto::build_request::Materializations;
use buck2_cli_proto::build_request::Uploads;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_data::AgentContextEntry;
use buck2_error::ErrorTag;
use buck2_events::dispatch::instant_event;
use buck2_settings::InvocationBuckSettings;
use dice::LinearRecomputeDiceComputations;

pub(super) struct BuildAdviceContext<'a> {
    invocation: Invocation<'a>,
    acknowledged: bool,
}

impl<'a> BuildAdviceContext<'a> {
    pub(super) fn new(
        settings: &InvocationBuckSettings,
        request: &'a BuildRequest,
    ) -> Option<Self> {
        let context = request.context.as_ref()?;
        // `run` and other commands share the build RPC but have already chosen an operation.
        if context.command_name != "build"
            || request
                .response_options
                .as_ref()
                .is_some_and(|o| o.return_outputs || o.return_run_args)
            || request.final_artifact_materializations == Materializations::Materialize as i32
            || request.final_artifact_uploads == Uploads::Always as i32
        {
            return None;
        }
        // Match the CLI protocol: last scalar entry wins; acknowledgements can repeat.
        let last = |key: &str| {
            context
                .agent_context
                .iter()
                .rev()
                .find(|entry| entry.key == key)
                .map(|entry| entry.value.as_str())
        };
        if last(AgentContextEntry::KEY_DIRECT_CALL) != Some("true") {
            return None;
        }
        Some(Self {
            invocation: Invocation {
                enabled: settings.agent_build.enabled(),
                direct_call: true,
                reason: last("reason"),
            },
            acknowledged: context.agent_context.iter().any(|entry| {
                entry.key == AgentContextEntry::KEY_ADVICE_ACK && entry.value == "build"
            }),
        })
    }

    async fn check(
        &self,
        ctx: LinearRecomputeDiceComputations<'_, '_>,
        label: &ConfiguredProvidersLabel,
        providers_to_build: &ProvidersToBuild,
    ) -> buck2_error::Result<()> {
        if !matches!(label.name(), ProvidersName::Default) {
            return Ok(());
        }
        // Reuse the build's cached analysis result. The decision is invocation-scoped and must
        // stay outside that cache; a later call may acknowledge the same target's advice.
        let MaybeCompatible::Compatible(outputs) =
            get_outputs_for_top_level_target(&mut ctx.get(), label, providers_to_build).await?
        else {
            return Ok(()); // The normal build path handles incompatible targets and skip flags.
        };
        if outputs.outputs.is_empty() {
            return Ok(());
        }
        let rule_type = outputs.rule_type.name();
        let Some(advice @ Advice::Build(language)) = Advice::for_build_rule(rule_type) else {
            return Ok(());
        };
        let language = language.as_str();
        // Observe both setting states after identical eligibility checks so control builds
        // can be compared with builds where the guard is enabled.
        let state = if self.invocation.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let mut tags = vec![
            format!("agent_interactive.build.{state}"),
            format!("agent_interactive.build.{language}.{state}"),
            format!("agent_interactive.build.{rule_type}.{state}"),
        ];
        let decision = advice.evaluate(&self.invocation, self.acknowledged);
        if let Some(tag) = decision.tag() {
            let outcome = if matches!(decision, Decision::Acknowledged(_)) {
                "ack"
            } else {
                "block"
            };
            tags.push(tag.to_owned());
            tags.push(format!("agent_interactive.build.{language}.{outcome}"));
            tags.push(format!("agent_interactive.build.{rule_type}.{outcome}"));
        }
        instant_event(buck2_data::TagEvent { tags });
        match decision {
            Decision::Inactive | Decision::Acknowledged(_) => Ok(()),
            Decision::Block(advice) => Err(buck2_error::buck2_error!(
                ErrorTag::AgentAdviceBlocked,
                "Buck advice [{}]\nTarget `{}` has rule type `{}`.\n\n{}",
                advice.id(),
                label.unconfigured(),
                rule_type,
                advice
                    .message()
                    .replace("<target>", &label.unconfigured().to_string()),
            )),
        }
    }
}

pub(super) async fn build_configured_label(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    materialization_and_upload: MaterializationAndUploadContext,
    label: ConfiguredProvidersLabel,
    providers_to_build: &ProvidersToBuild,
    opts: BuildConfiguredLabelOptions,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
    advice: Option<&BuildAdviceContext<'_>>,
) {
    if let Some(advice) = advice {
        if let Err(err) = advice.check(ctx, &label, providers_to_build).await {
            event_consumer.consume(BuildEvent::new_configured(
                label,
                ConfiguredBuildEventVariant::Error { err },
            ));
            return;
        }
    }
    build::build_configured_label(
        event_consumer,
        ctx,
        materialization_and_upload,
        label,
        providers_to_build,
        opts,
        timeout_observer,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use buck2_cli_proto::ClientContext;
    use buck2_cli_proto::build_request::ResponseOptions;

    use super::*;

    #[test]
    fn artifact_requests_and_other_commands_skip_advice() -> buck2_error::Result<()> {
        let settings =
            serde_json::from_str::<InvocationBuckSettings>(r#"{"agent_build":{"enabled":true}}"#)?;
        let request = BuildRequest {
            final_artifact_uploads: Uploads::Never as i32,
            context: Some(ClientContext {
                command_name: "build".to_owned(),
                agent_context: vec![AgentContextEntry {
                    key: AgentContextEntry::KEY_DIRECT_CALL.to_owned(),
                    value: "true".to_owned(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(BuildAdviceContext::new(&settings, &request).is_some());

        for (outputs, run_args, materialize, upload, command) in [
            (true, false, false, false, "build"),
            (false, true, false, false, "build"),
            (false, false, true, false, "build"),
            (false, false, false, true, "build"),
            (false, false, false, false, "run"),
            (false, false, false, false, "install"),
        ] {
            let mut request = request.clone();
            request.context.as_mut().unwrap().command_name = command.to_owned();
            request.response_options = Some(ResponseOptions {
                return_outputs: outputs,
                return_run_args: run_args,
            });
            if materialize {
                request.final_artifact_materializations = Materializations::Materialize as i32;
            }
            if upload {
                request.final_artifact_uploads = Uploads::Always as i32;
            }
            assert!(BuildAdviceContext::new(&settings, &request).is_none());
        }
        Ok(())
    }
}
