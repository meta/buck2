/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::configuration::config_setting::ConfigSettingData;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

/// Key in `select` or an item in `target_compatible_with`.
/// Should point to `config_setting` target, or `constraint_value`.
#[derive(
    Debug,
    Eq,
    PartialEq,
    Hash,
    StrongHash,
    Allocative,
    derive_more::Display,
    Clone,
    Dupe,
    Ord,
    PartialOrd,
    ref_cast::RefCast,
    Pagable
)]
#[repr(transparent)]
pub struct ConfigurationSettingKey(pub ProvidersLabel);

impl ConfigurationSettingKey {
    pub fn testing_parse(label: &str) -> ConfigurationSettingKey {
        ConfigurationSettingKey(ProvidersLabel::default_for(TargetLabel::testing_parse(
            label,
        )))
    }
}

/// See [`MatchedConfigurationSettingKeys`].
#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct MatchedConfigurationSettingKeysWithCfg(Arc<MatchedConfigurationSettingKeysWithCfgData>);

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub(crate) struct MatchedConfigurationSettingKeysWithCfgData {
    cfg: ConfigurationNoExec,
    pub(crate) settings: MatchedConfigurationSettingKeys,
}

/// For a given target, this stores the result of matching all of the select keys and compatibility
/// attributes against that target's configuration.
#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct MatchedConfigurationSettingKeys {
    settings: BuckFixedSortedMap<ConfigurationSettingKey, ConfigurationNode>,
}

impl MatchedConfigurationSettingKeysWithCfg {
    pub fn new(cfg: ConfigurationNoExec, settings: MatchedConfigurationSettingKeys) -> Self {
        Self(Arc::new(MatchedConfigurationSettingKeysWithCfgData {
            cfg,
            settings,
        }))
    }

    pub fn cfg(&self) -> &ConfigurationNoExec {
        &self.0.cfg
    }

    pub fn settings(&self) -> &MatchedConfigurationSettingKeys {
        &self.0.settings
    }
}

impl MatchedConfigurationSettingKeys {
    pub fn new(
        settings: BuckFixedSortedMap<ConfigurationSettingKey, ConfigurationNode>,
    ) -> MatchedConfigurationSettingKeys {
        MatchedConfigurationSettingKeys { settings }
    }

    pub fn empty() -> MatchedConfigurationSettingKeys {
        MatchedConfigurationSettingKeys::new(BuckFixedSortedMap::new())
    }

    pub fn setting_matches(&self, key: &ConfigurationSettingKey) -> Option<&ConfigSettingData> {
        let Some(configuration_node) = self.settings.get(key) else {
            panic!("unresolved configuration setting: `{key}`");
        };
        configuration_node.configuration_data()
    }
}

/// A ConfigurationNode contains the information about a config_setting() or similar target in a certain configuration.
#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct ConfigurationNode(Arc<ConfigurationNodeData>);

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
struct ConfigurationNodeData {
    /// `None` when config settings does not match the configuration.
    config_setting: Option<ConfigSettingData>,
}

impl ConfigurationNode {
    pub fn new(config_setting: Option<ConfigSettingData>) -> Self {
        Self(Arc::new(ConfigurationNodeData { config_setting }))
    }

    pub fn configuration_data(&self) -> Option<&ConfigSettingData> {
        self.0.config_setting.as_ref()
    }
}
