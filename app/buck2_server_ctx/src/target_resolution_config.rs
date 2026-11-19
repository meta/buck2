/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::TargetCfg;
use buck2_core::configuration::bound_id::BoundConfigurationId;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::pattern::pattern::ModifiersError;
use buck2_core::pattern::pattern::ProvidersLabelWithModifiers;
use buck2_core::pattern::pattern::TargetLabelWithExtra;
use buck2_core::pattern::pattern_type::ConfigurationPredicate;
use buck2_core::pattern::pattern_type::ConfiguredTargetPatternExtra;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::configured_universe::UNIVERSE_FROM_LITERALS;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use dice::DiceComputations;
use dupe::Dupe;

use crate::ctx::ServerCommandContextTrait;
use crate::global_cfg_options::global_cfg_options_from_client_context;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PatternNotSupportedError {
    #[error("Builtin configurations are not supported: `{0}`")]
    BuiltinConfigurationsNotSupported(String),
    #[error(
        "Patterns with configuration label without configuration hash are not supported: `{0}`"
    )]
    ConfigurationLabelWithoutHashNotSupported(String),
}

pub enum TargetResolutionConfig {
    /// Resolve using target platform.
    Default(GlobalCfgOptions),
    /// Resolve in the universe.
    Universe(CqueryUniverse),
}

impl TargetResolutionConfig {
    pub async fn from_args(
        ctx: &mut DiceComputations<'_>,
        target_cfg: &TargetCfg,
        server_ctx: &dyn ServerCommandContextTrait,
        target_universe: &[String],
    ) -> buck2_error::Result<TargetResolutionConfig> {
        let global_cfg_options =
            global_cfg_options_from_client_context(target_cfg, server_ctx, ctx).await?;
        if target_universe.is_empty() {
            Ok(TargetResolutionConfig::Default(global_cfg_options))
        } else {
            Ok(TargetResolutionConfig::Universe(
                (UNIVERSE_FROM_LITERALS.get()?)(
                    ctx,
                    server_ctx.working_dir(),
                    target_universe,
                    global_cfg_options,
                )
                .await?,
            ))
        }
    }

    pub async fn get_configured_target(
        &self,
        ctx: &mut DiceComputations<'_>,
        label: &TargetLabel,
        modifiers: Option<&[String]>,
    ) -> buck2_error::Result<Vec<ConfiguredTargetLabel>> {
        match self {
            TargetResolutionConfig::Default(global_cfg_options) => {
                let local_cfg_options = match modifiers {
                    None => global_cfg_options.dupe(),
                    Some(modifiers) => {
                        if !global_cfg_options.cli_modifiers.is_empty() {
                            return Err(ModifiersError::PatternModifiersWithGlobalModifiers.into());
                        }

                        GlobalCfgOptions {
                            target_platform: global_cfg_options.target_platform.dupe(),
                            cli_modifiers: modifiers.to_vec().into(),
                        }
                    }
                };
                Ok(vec![
                    ctx.get_configured_target(label, &local_cfg_options).await?,
                ])
            }
            TargetResolutionConfig::Universe(universe) => {
                if modifiers.is_some() {
                    return Err(ModifiersError::PatternModifiersWithTargetUniverse.into());
                }

                // TODO(nga): whoever called this function,
                //    they may have resolved pattern unnecessarily.
                Ok(universe.get_target_label(label))
            }
        }
    }

    pub async fn get_configured_provider_label(
        &self,
        ctx: &mut DiceComputations<'_>,
        label: &ProvidersLabel,
    ) -> buck2_error::Result<Vec<ConfiguredProvidersLabel>> {
        Ok(self
            .get_configured_target(ctx, label.target(), None)
            .await?
            .into_iter()
            .map(|configured_target_label| {
                ConfiguredProvidersLabel::new(configured_target_label, label.name().clone())
            })
            .collect::<Vec<_>>())
    }

    pub async fn get_configured_provider_label_with_modifiers(
        &self,
        ctx: &mut DiceComputations<'_>,
        label_with_modifiers: &ProvidersLabelWithModifiers,
    ) -> buck2_error::Result<Vec<ConfiguredProvidersLabel>> {
        let ProvidersLabelWithModifiers {
            providers_label,
            modifiers,
        } = label_with_modifiers;

        Ok(self
            .get_configured_target(ctx, providers_label.target(), modifiers.as_slice())
            .await?
            .into_iter()
            .map(|configured_target_label| {
                ConfiguredProvidersLabel::new(
                    configured_target_label,
                    providers_label.name().clone(),
                )
            })
            .collect::<Vec<_>>())
    }

    pub async fn get_configured_targets_for_configured_target_literals(
        &self,
        ctx: &mut DiceComputations<'_>,
        label: &TargetLabelWithExtra<ConfiguredTargetPatternExtra>,
    ) -> buck2_error::Result<Vec<ConfiguredTargetLabel>> {
        let TargetLabelWithExtra {
            target_label,
            extra,
            modifiers: _,
        } = &label;
        match &extra.cfg {
            ConfigurationPredicate::Any => {
                self.get_configured_target(ctx, target_label, None).await
            }
            ConfigurationPredicate::Builtin(p) => Err(
                PatternNotSupportedError::BuiltinConfigurationsNotSupported(p.to_string()).into(),
            ),
            ConfigurationPredicate::Bound(label, None) => Err(
                PatternNotSupportedError::ConfigurationLabelWithoutHashNotSupported(
                    label.to_string(),
                )
                .into(),
            ),
            ConfigurationPredicate::Bound(label, Some(hash)) => {
                let cfg = ConfigurationData::lookup_bound(BoundConfigurationId {
                    label: label.clone(),
                    hash: hash.clone(),
                })?;
                let configured = target_label.configure(cfg);
                match self {
                    TargetResolutionConfig::Default(_) => Ok(vec![configured]),
                    TargetResolutionConfig::Universe(universe) => {
                        if universe.contains(&configured) {
                            Ok(vec![configured])
                        } else {
                            Ok(Vec::new())
                        }
                    }
                }
            }
        }
    }
}
