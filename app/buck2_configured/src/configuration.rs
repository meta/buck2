/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_signals::node_key::BuildSignalsNodeKey;
use buck2_build_signals::node_key::BuildSignalsNodeKeyImpl;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::configs::parse_config_section_and_key;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_configuration_providers::configuration_info::ConfigurationInfo;
use buck2_configuration_providers::platform_info::PlatformInfo;
use buck2_core::configuration::config_setting::ConfigSettingData;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::data::ExecPlatformMarking;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckFixedSortedMap;
use buck2_node::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use buck2_node::configuration::calculation::CONFIGURATION_CALCULATION;
use buck2_node::configuration::calculation::CellNameForConfigurationResolution;
use buck2_node::configuration::calculation::ConfigurationCalculationDyn;
use buck2_node::configuration::resolved::ConfigurationNode;
use buck2_node::configuration::resolved::ConfigurationSettingKey;
use buck2_node::configuration::resolved::MatchedConfigurationSettingKeys;
use buck2_node::configuration::resolved::MatchedConfigurationSettingKeysWithCfg;
use buck2_node::nodes::unconfigured::TargetNodeRef;
use derive_more::Display;
use dice::Demand;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use ref_cast::RefCast;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
pub enum ConfigurationError {
    #[error(
        "`{0}` target doesn't have a `ConfigurationInfo` provider so it can't be selected. Possible selectable rules are `config_setting` and `constraint_value`."
    )]
    MissingConfigurationInfoProvider(ProvidersLabel),
    #[error("Expected `{0}` to be a `platform()` target, but it had no `PlatformInfo` provider.")]
    MissingPlatformInfo(TargetLabel),
    #[error(
        "Platform target `{0}` evaluation returned `ProviderInfo` label `{1}` which resolved to an unequal configuration"
    )]
    PlatformEvalUnequalConfiguration(TargetLabel, TargetLabel),
    #[error(
        "Expected `{0}` to be a `constraint_setting()` target, but it had no `ConstraintSettingInfo` provider."
    )]
    MissingConstraintSettingInfo(TargetLabel),
}

async fn configuration_matches(
    ctx: &mut DiceComputations<'_>,
    cfg: &ConfigurationData,
    target_node_cell: CellNameForConfigurationResolution,
    constraints_and_configs: &ConfigSettingData,
) -> buck2_error::Result<bool> {
    for (key, value) in &constraints_and_configs.constraints {
        match cfg.get_constraint_value(key)? {
            Some(v) if v == value => {
                // Configuration explicitly sets this constraint and it matches
            }
            Some(_) => {
                // Configuration explicitly sets this constraint but it doesn't match
                return Ok(false);
            }
            None => {
                // Configuration doesn't set this constraint, check if there's a default
                match &key.default {
                    Some(default) if default == value => {
                        // Default value matches the required value
                    }
                    _ => {
                        // No default or default doesn't match
                        return Ok(false);
                    }
                }
            }
        }
    }

    // Cell used for buckconfigs is set to cell of target that applies select to match Buck v1 behavior.
    // Eventually, we want this to be the cell of the platform instead.
    for (raw_section_and_key, config_value) in &constraints_and_configs.buckconfigs {
        let config_section_and_key = parse_config_section_and_key(raw_section_and_key, None)?;
        let v = ctx
            .get_legacy_config_property(
                target_node_cell.0,
                BuckconfigKeyRef {
                    section: &config_section_and_key.section,
                    property: &config_section_and_key.key,
                },
            )
            .await?;
        match v {
            Some(v) if &*v == config_value => {}
            _ => return Ok(false),
        }
    }

    if !constraints_and_configs.root_buckconfigs.is_empty() {
        let root_config = ctx.get_legacy_root_config_on_dice().await?;

        for (raw_section_and_key, config_value) in &constraints_and_configs.root_buckconfigs {
            let config_section_and_key = parse_config_section_and_key(raw_section_and_key, None)?;
            let v = root_config.lookup(
                ctx,
                BuckconfigKeyRef {
                    section: &config_section_and_key.section,
                    property: &config_section_and_key.key,
                },
            )?;
            match v {
                Some(v) if &*v == config_value => {}
                _ => return Ok(false),
            }
        }
    }

    Ok(true)
}

#[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("ConfigurationNode({}, {})", cfg_target, target_cfg)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct ConfigurationNodeKey {
    target_cfg: ConfigurationData,
    target_cell: CellNameForConfigurationResolution,
    cfg_target: ConfigurationSettingKey,
}

#[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display(
    "ResolvedConfigurationKey(target_cfg: {}, cell: {}, configuration_deps size {})",
    target_cfg,
    target_cell,
    configuration_deps.len()
)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct MatchedConfigurationSettingKeysKey {
    target_cfg: ConfigurationData,
    target_cell: CellNameForConfigurationResolution,
    configuration_deps: Vec<ConfigurationSettingKey>,
}

async fn compute_platform_configuration_no_label_check(
    ctx: &mut DiceComputations<'_>,
    target: &TargetLabel,
) -> buck2_error::Result<ConfigurationData> {
    ctx
        // TODO(T198223238): Not supporting platforms being supplied via subtargets for now
        .get_configuration_analysis_result(&ProvidersLabel::default_for(target.dupe()))
        .await?
        .provider_collection()
        .builtin_provider::<PlatformInfo>()?
        .ok_or_else(|| ConfigurationError::MissingPlatformInfo(target.dupe()))?
        .to_configuration(ExecPlatformMarking::Unmarked)
}

/// Basically, evaluate `platform()` rule.
async fn compute_platform_configuration(
    ctx: &mut DiceComputations<'_>,
    target: &TargetLabel,
) -> buck2_error::Result<ConfigurationData> {
    let configuration_data = compute_platform_configuration_no_label_check(ctx, target).await?;

    let cell_resolver = ctx.get_cell_resolver().await?;
    let cell_alias_resolver = ctx
        .get_cell_alias_resolver(cell_resolver.root_cell())
        .await?;
    let parsed_target = TargetLabel::parse(
        configuration_data.label()?,
        cell_resolver.root_cell(),
        &cell_resolver,
        &cell_alias_resolver,
    )
    .buck_error_context(
        "`PlatformInfo` label for `platform()` rule should be a valid target label",
    )?;

    if target != &parsed_target {
        // `target` may be an `alias` target. In this case we evaluate the label
        // from the configuration and check it resolves to the same configuration.

        let cfg_again = compute_platform_configuration_no_label_check(
            ctx,
            &parsed_target,
        )
        .await
        .buck_error_context(
            "Checking whether label of returned `PlatformInfo` resolves to the same configuration",
        )?;
        if cfg_again != configuration_data {
            return Err(ConfigurationError::PlatformEvalUnequalConfiguration(
                target.dupe(),
                parsed_target,
            )
            .into());
        }
    }

    Ok(configuration_data)
}

#[async_trait]
impl Key for MatchedConfigurationSettingKeysKey {
    type Value = buck2_error::Result<MatchedConfigurationSettingKeysWithCfg>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        let config_nodes = ctx
            .compute_join(self.configuration_deps.iter(), async |ctx, d| {
                (
                    d.dupe(),
                    get_configuration_node(ctx, &self.target_cfg, self.target_cell, d).await,
                )
            })
            .await;

        let resolved_settings = config_nodes
            .into_iter()
            .map(|(label, node)| Ok((label, node?)))
            .collect::<buck2_error::Result<BuckFixedSortedMap<_, _>>>()?;
        let resolved_settings = MatchedConfigurationSettingKeys::new(resolved_settings);
        Ok(MatchedConfigurationSettingKeysWithCfg::new(
            ConfigurationNoExec::new(self.target_cfg.dupe()),
            resolved_settings,
        ))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::AlwaysUnequal
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }

    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        demand.provide_value_with(|| BuildSignalsNodeKey::new(self.clone()))
    }
}

impl BuildSignalsNodeKeyImpl for MatchedConfigurationSettingKeysKey {
    fn kind(&self) -> &'static str {
        "matched_configuration_settings"
    }
}

async fn get_configuration_node(
    ctx: &mut DiceComputations<'_>,
    target_cfg: &ConfigurationData,
    target_cell: CellNameForConfigurationResolution,
    cfg_target: &ConfigurationSettingKey,
) -> buck2_error::Result<ConfigurationNode> {
    ctx.compute(&ConfigurationNodeKey {
        target_cfg: target_cfg.dupe(),
        target_cell,
        cfg_target: cfg_target.dupe(),
    })
    .await?
    .dupe()
    .with_buck_error_context(|| {
        format!(
            "Error getting configuration node of `{cfg_target}` within the `{target_cfg}` configuration",
        )
    })
}

#[async_trait]
impl Key for ConfigurationNodeKey {
    type Value = buck2_error::Result<ConfigurationNode>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        let providers = ctx
            .get_configuration_analysis_result(&self.cfg_target.0)
            .await?;

        // capture the result so the temporaries get dropped before providers
        let result = match providers
            .provider_collection()
            .builtin_provider::<ConfigurationInfo>()?
        {
            Some(configuration_info) => configuration_info,
            None => {
                return Err::<_, buck2_error::Error>(
                    ConfigurationError::MissingConfigurationInfoProvider(self.cfg_target.0.dupe())
                        .into(),
                );
            }
        }
        .to_config_setting_data()?;

        let matches =
            configuration_matches(ctx, &self.target_cfg, self.target_cell, &result).await?;

        Ok(ConfigurationNode::new(matches.then_some(result)))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }

    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        demand.provide_value_with(|| BuildSignalsNodeKey::new(self.clone()))
    }
}

impl BuildSignalsNodeKeyImpl for ConfigurationNodeKey {
    fn kind(&self) -> &'static str {
        "configuration_node"
    }

    fn package(&self) -> Option<PackageLabel> {
        Some(self.cfg_target.0.target().pkg())
    }
}

pub(crate) async fn get_platform_configuration(
    ctx: &mut DiceComputations<'_>,
    target: &TargetLabel,
) -> buck2_error::Result<ConfigurationData> {
    #[derive(
        derive_more::Display,
        Debug,
        Eq,
        Hash,
        PartialEq,
        Clone,
        Allocative,
        Pagable
    )]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct PlatformConfigurationKey(TargetLabel);

    #[async_trait]
    impl Key for PlatformConfigurationKey {
        type Value = buck2_error::Result<ConfigurationData>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellation: &CancellationContext,
        ) -> Self::Value {
            compute_platform_configuration(ctx, &self.0).await
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| match (x, y) {
                (Ok(x), Ok(y)) => x == y,
                _ => false,
            })
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            OkPagableValueSerialize::<Self::Value>::new()
        }
    }

    ctx.compute(&PlatformConfigurationKey(target.dupe()))
        .await?
        .dupe()
}

pub(crate) async fn compute_platform_cfgs(
    ctx: &mut DiceComputations<'_>,
    node: TargetNodeRef<'_>,
) -> buck2_error::Result<BuckFixedSortedMap<TargetLabel, ConfigurationData>> {
    let mut platform_cfgs = Vec::new();
    for (platform_target, kind) in node.get_configuration_deps_with_kind() {
        if kind == ConfigurationDepKind::ConfiguredDepPlatform {
            let platform_target = platform_target.target();
            let config = get_platform_configuration(ctx, platform_target).await?;
            platform_cfgs.push((platform_target.dupe(), config));
        }
    }

    Ok(platform_cfgs.into_iter().collect())
}

pub(crate) async fn get_matched_cfg_keys<
    'a,
    'd,
    T: IntoIterator<Item = &'a ConfigurationSettingKey> + Send,
>(
    ctx: &mut DiceComputations<'d>,
    target_cfg: &ConfigurationData,
    target_cell: CellNameForConfigurationResolution,
    configuration_deps: T,
) -> buck2_error::Result<&'d MatchedConfigurationSettingKeysWithCfg> {
    let configuration_deps: Vec<ConfigurationSettingKey> =
        configuration_deps.into_iter().map(|t| t.dupe()).collect();
    ctx.compute(&MatchedConfigurationSettingKeysKey {
        target_cfg: target_cfg.dupe(),
        target_cell,
        configuration_deps,
    })
    .await?
    .as_ref()
    .duped_err()
}

pub(crate) async fn get_matched_cfg_keys_for_node<'d>(
    ctx: &mut DiceComputations<'d>,
    target_cfg: &ConfigurationData,
    target_cell: CellNameForConfigurationResolution,
    node: TargetNodeRef<'_>,
) -> buck2_error::Result<&'d MatchedConfigurationSettingKeysWithCfg> {
    let d = node
        .get_configuration_deps_with_kind()
        .filter_map(|(d, k)| {
            match k {
                ConfigurationDepKind::CompatibilityAttribute => true,
                ConfigurationDepKind::SelectKey => true,
                ConfigurationDepKind::ConfiguredDepPlatform => false,
                ConfigurationDepKind::Transition => false,
                ConfigurationDepKind::DefaultTargetPlatform => false,
            }
            .then_some(d)
        })
        .map(ConfigurationSettingKey::ref_cast);
    get_matched_cfg_keys(ctx, target_cfg, target_cell, d).await
}

struct ConfigurationCalculationDynImpl;

#[async_trait]
impl ConfigurationCalculationDyn for ConfigurationCalculationDynImpl {
    async fn get_platform_configuration(
        &self,
        ctx: &mut DiceComputations<'_>,
        target: &TargetLabel,
    ) -> buck2_error::Result<ConfigurationData> {
        Ok(get_platform_configuration(ctx, target).await?)
    }
}

pub(crate) fn init_configuration_calculation() {
    CONFIGURATION_CALCULATION.init(&ConfigurationCalculationDynImpl);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use buck2_common::dice::cells::SetCellResolver;
    use buck2_common::legacy_configs::configs::testing::parse;
    use buck2_common::legacy_configs::dice::inject_legacy_configs_for_test;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use dice::UserComputationData;
    use dice::testing::DiceBuilder;

    use super::*;

    #[tokio::test]
    async fn configuration_matches_buckconfigs_from_target_and_root_cells()
    -> buck2_error::Result<()> {
        let root_config = parse(
            &[(
                "root/.buckconfig",
                "[cell_scope]\nkey = root-value\n[root_scope]\nkey = root-value\n",
            )],
            "root/.buckconfig",
        )?;
        let target_config = parse(
            &[(
                "cell/.buckconfig",
                "[cell_scope]\nkey = target-value\n[root_scope]\nkey = target-value\n",
            )],
            "cell/.buckconfig",
        )?;

        let root_cell = CellName::testing_new("root");
        let target_cell = CellName::testing_new("cell");
        let cell_resolver = CellResolver::testing_with_names_and_paths(&[
            (root_cell, CellRootPathBuf::testing_new("")),
            (target_cell, CellRootPathBuf::testing_new("cell")),
        ]);

        let mut dice = DiceBuilder::new()
            .build(UserComputationData::new())
            .await
            .expect("should build test DICE");
        dice.set_cell_resolver(cell_resolver)?;
        inject_legacy_configs_for_test(
            &mut dice,
            [(root_cell, root_config), (target_cell, target_config)],
        )?;

        let dice = dice.commit().await;
        let mut ctx = dice.ctx();
        let cfg = ConfigurationData::testing_new();
        let target_cell = CellNameForConfigurationResolution(target_cell);

        assert!(
            configuration_matches(
                &mut ctx,
                &cfg,
                target_cell,
                &ConfigSettingData {
                    constraints: BTreeMap::new(),
                    buckconfigs: BTreeMap::from_iter([(
                        "cell_scope.key".to_owned(),
                        "target-value".to_owned(),
                    )]),
                    root_buckconfigs: BTreeMap::new(),
                },
            )
            .await?
        );
        assert!(
            !configuration_matches(
                &mut ctx,
                &cfg,
                target_cell,
                &ConfigSettingData {
                    constraints: BTreeMap::new(),
                    buckconfigs: BTreeMap::from_iter([(
                        "cell_scope.key".to_owned(),
                        "root-value".to_owned(),
                    )]),
                    root_buckconfigs: BTreeMap::new(),
                },
            )
            .await?
        );
        assert!(
            configuration_matches(
                &mut ctx,
                &cfg,
                target_cell,
                &ConfigSettingData {
                    constraints: BTreeMap::new(),
                    buckconfigs: BTreeMap::new(),
                    root_buckconfigs: BTreeMap::from_iter([(
                        "root_scope.key".to_owned(),
                        "root-value".to_owned(),
                    )]),
                },
            )
            .await?
        );
        assert!(
            !configuration_matches(
                &mut ctx,
                &cfg,
                target_cell,
                &ConfigSettingData {
                    constraints: BTreeMap::new(),
                    buckconfigs: BTreeMap::new(),
                    root_buckconfigs: BTreeMap::from_iter([(
                        "root_scope.key".to_owned(),
                        "target-value".to_owned(),
                    )]),
                },
            )
            .await?
        );
        assert!(
            configuration_matches(
                &mut ctx,
                &cfg,
                target_cell,
                &ConfigSettingData {
                    constraints: BTreeMap::new(),
                    buckconfigs: BTreeMap::from_iter([(
                        "cell_scope.key".to_owned(),
                        "target-value".to_owned(),
                    )]),
                    root_buckconfigs: BTreeMap::from_iter([(
                        "root_scope.key".to_owned(),
                        "root-value".to_owned(),
                    )]),
                },
            )
            .await?
        );

        Ok(())
    }
}
