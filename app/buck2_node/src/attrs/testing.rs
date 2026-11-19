/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::sync::Arc;

use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::IncompatiblePlatformReasonCause;
use buck2_core::configuration::config_setting::ConfigSettingData;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::data::ConfigurationDataData;
use buck2_core::configuration::data::ExecPlatformMarking;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::configuration::pair::ConfigurationWithExec;
use buck2_core::configuration::transition::applied::TransitionApplied;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;

use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::configuration::resolved::ConfigurationNode;
use crate::configuration::resolved::ConfigurationSettingKey;
use crate::configuration::resolved::MatchedConfigurationSettingKeys;

pub fn configuration_ctx() -> impl AttrConfigurationContext {
    configuration_ctx_with_matched_keys([
        (
            ConfigurationSettingKey::testing_parse("root//other:config"),
            Some(ConfigSettingData {
                constraints: BTreeMap::new(),
                buckconfigs: BTreeMap::new(),
                root_buckconfigs: BTreeMap::new(),
            }),
        ),
        (
            ConfigurationSettingKey::testing_parse("root//some:config"),
            None,
        ),
        (
            ConfigurationSettingKey::testing_parse("cell1//other:config"),
            None,
        ),
    ])
}

/// A configuration context whose `select()` resolution sees exactly
/// `matched_keys`, each with the setting data that decides specificity.
pub fn configuration_ctx_with_matched_keys(
    matched_keys: impl IntoIterator<Item = (ConfigurationSettingKey, Option<ConfigSettingData>)>,
) -> impl AttrConfigurationContext {
    let matched_keys = MatchedConfigurationSettingKeys::new(BuckFixedSortedMap::from_iter(
        matched_keys
            .into_iter()
            .map(|(k, d)| (k, ConfigurationNode::new(d))),
    ));
    struct TestAttrConfigurationContext(
        ConfigurationData,
        ConfigurationData,
        MatchedConfigurationSettingKeys,
        ConfiguredTargetLabel,
    );
    impl AttrConfigurationContext for TestAttrConfigurationContext {
        fn cfg(&self) -> ConfigurationNoExec {
            ConfigurationNoExec::new(self.0.dupe())
        }

        fn base_exec_cfg(&self) -> buck2_error::Result<ConfigurationNoExec> {
            Ok(ConfigurationNoExec::new(self.1.dupe()))
        }

        fn matched_cfg_keys(&self) -> &MatchedConfigurationSettingKeys {
            &self.2
        }

        fn toolchain_cfg(&self) -> ConfigurationWithExec {
            ConfigurationWithExec::new(self.0.dupe(), self.1.dupe())
        }

        fn platform_cfg(&self, _label: &TargetLabel) -> buck2_error::Result<ConfigurationData> {
            panic!("not used in tests")
        }

        fn resolved_transitions(
            &self,
        ) -> buck2_error::Result<&BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>>
        {
            panic!("not used in tests")
        }

        fn incompatible_platform_reason(
            &self,
            cause: IncompatiblePlatformReasonCause,
        ) -> Arc<IncompatiblePlatformReason> {
            Arc::new(IncompatiblePlatformReason {
                target: self.3.dupe(),
                cause,
            })
        }
    }

    let cfg = ConfigurationData::testing_new();
    TestAttrConfigurationContext(
        cfg.dupe(),
        ConfigurationData::from_platform(
            "cfg_for//:testing_exec".to_owned(),
            ConfigurationDataData {
                constraints: BTreeMap::new(),
            },
            ExecPlatformMarking::Unmarked,
        )
        .unwrap(),
        matched_keys,
        TargetLabel::testing_parse("root//:test_target").configure(cfg),
    )
}
