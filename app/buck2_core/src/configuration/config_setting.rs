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

use allocative::Allocative;
use pagable::Pagable;

use crate::configuration::constraints::ConstraintKey;
use crate::configuration::constraints::ConstraintValue;

/// Parsed provider returned from `config_setting` rule.
#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub struct ConfigSettingData {
    // contains the full specification of the platform configuration
    pub constraints: BTreeMap<ConstraintKey, ConstraintValue>,
    // contains mappings of `section.key` to `value` for buckconfigs.
    // `buckconfigs` are evaluated from the target's cell (like `read_config`).
    // `root_buckconfigs` are evaluated from the root cell (like `read_root_config`).
    // TODO(scottcao): Make this into a Vec<ConfigArgumentPair> for more structured data
    // This can't be done right now because ConfigArgumentPair lives in buck2_common
    // and buck2_core cannot depend on buck2_common.
    pub buckconfigs: BTreeMap<String, String>,
    /// Buckconfigs that are always evaluated from the root cell's .buckconfig.
    pub root_buckconfigs: BTreeMap<String, String>,
}

impl ConfigSettingData {
    fn is_subset<K: Ord, V: Eq>(a: &BTreeMap<K, V>, b: &BTreeMap<K, V>) -> bool {
        // TODO(nga): this can be done in linear time.
        a.len() <= b.len() && a.iter().all(|(k, v)| b.get(k) == Some(v))
    }

    fn len_sum(&self) -> usize {
        self.constraints.len() + self.buckconfigs.len() + self.root_buckconfigs.len()
    }

    pub fn refines(&self, that: &ConfigSettingData) -> bool {
        if self.len_sum() <= that.len_sum() {
            return false;
        }
        if !Self::is_subset(&that.constraints, &self.constraints) {
            return false;
        }

        Self::is_subset(&that.buckconfigs, &self.buckconfigs)
            && Self::is_subset(&that.root_buckconfigs, &self.root_buckconfigs)
    }

    pub fn testing_new(
        constraint_values: BTreeMap<ConstraintKey, ConstraintValue>,
    ) -> ConfigSettingData {
        ConfigSettingData {
            constraints: constraint_values,
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dupe::Dupe;

    use crate::configuration::config_setting::ConfigSettingData;
    use crate::configuration::constraints::ConstraintKey;
    use crate::configuration::constraints::ConstraintValue;

    #[test]
    fn is_subset() {
        let m_12_34 = BTreeMap::from_iter([(1, 2), (3, 4)]);
        let m_12_35 = BTreeMap::from_iter([(1, 2), (3, 5)]);
        let m_12 = BTreeMap::from_iter([(1, 2)]);
        let empty = BTreeMap::<u32, u32>::new();

        // A set is a subset of itself
        assert!(ConfigSettingData::is_subset(&m_12_34, &m_12_34));
        assert!(ConfigSettingData::is_subset(&m_12, &m_12));
        assert!(ConfigSettingData::is_subset(&empty, &empty));

        assert!(ConfigSettingData::is_subset(&m_12, &m_12_34));
        assert!(!ConfigSettingData::is_subset(&m_12_34, &m_12));

        assert!(!ConfigSettingData::is_subset(&m_12_34, &m_12_35));
    }

    #[test]
    fn refines() {
        fn constraint_key(t: &str) -> ConstraintKey {
            ConstraintKey::testing_new(t)
        }

        fn constraint_value(t: &str) -> ConstraintValue {
            ConstraintValue::testing_new(t, None)
        }

        let os = constraint_key("config//:os");
        let linux = constraint_value("config//:linux");
        let cpu = constraint_key("config//:cpu");
        let arm64 = constraint_value("config//:arm64");
        let x86_64 = constraint_value("config//:x86_64");

        let c_linux = ConfigSettingData {
            constraints: BTreeMap::from_iter([(os.dupe(), linux.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_arm64 = ConfigSettingData {
            constraints: BTreeMap::from_iter([(cpu.dupe(), arm64.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_linux_arm64 = ConfigSettingData {
            constraints: BTreeMap::from_iter([
                (os.dupe(), linux.dupe()),
                (cpu.dupe(), arm64.dupe()),
            ]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_linux_x86_64 = ConfigSettingData {
            constraints: BTreeMap::from_iter([
                (os.dupe(), linux.dupe()),
                (cpu.dupe(), x86_64.dupe()),
            ]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };

        // Config setting does not refines identical config setting.
        assert!(!c_linux.refines(&c_linux));

        assert!(!c_linux.refines(&c_arm64));
        assert!(!c_arm64.refines(&c_linux));

        assert!(c_linux_arm64.refines(&c_linux));
        assert!(c_linux_arm64.refines(&c_arm64));

        assert!(!c_linux_x86_64.refines(&c_linux_arm64));
    }

    #[test]
    fn buckconfig_refines() {
        let c1 = ConfigSettingData {
            constraints: BTreeMap::new(),
            buckconfigs: BTreeMap::from_iter([("foo.bar".to_owned(), "baz".to_owned())]),
            root_buckconfigs: BTreeMap::new(),
        };
        let c11 = ConfigSettingData {
            constraints: BTreeMap::new(),
            buckconfigs: BTreeMap::from_iter([
                ("foo.bar".to_owned(), "baz".to_owned()),
                ("foo.qux".to_owned(), "quux".to_owned()),
            ]),
            root_buckconfigs: BTreeMap::new(),
        };

        assert!(c11.refines(&c1));
        assert!(!c11.refines(&c11));
        assert!(!c1.refines(&c1));
        assert!(!c1.refines(&c11));
    }

    #[test]
    fn root_buckconfig_refines() {
        let target_cell = ConfigSettingData {
            constraints: BTreeMap::new(),
            buckconfigs: BTreeMap::from_iter([("foo.bar".to_owned(), "baz".to_owned())]),
            root_buckconfigs: BTreeMap::new(),
        };
        let target_cell_more_specific = ConfigSettingData {
            constraints: BTreeMap::new(),
            buckconfigs: BTreeMap::from_iter([
                ("foo.bar".to_owned(), "baz".to_owned()),
                ("foo.qux".to_owned(), "quux".to_owned()),
            ]),
            root_buckconfigs: BTreeMap::new(),
        };
        let root_values = ConfigSettingData {
            constraints: BTreeMap::new(),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::from_iter([("foo.bar".to_owned(), "baz".to_owned())]),
        };
        let root_values_more_specific = ConfigSettingData {
            constraints: BTreeMap::new(),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::from_iter([
                ("foo.bar".to_owned(), "baz".to_owned()),
                ("foo.qux".to_owned(), "quux".to_owned()),
            ]),
        };

        assert!(target_cell_more_specific.refines(&target_cell));
        assert!(root_values_more_specific.refines(&root_values));
        assert!(!target_cell.refines(&root_values));
        assert!(!root_values.refines(&target_cell));
    }

    #[test]
    fn subtarget_values_distinct() {
        // Test that different subtarget values of the same constraint are distinct
        let sanitizer_key = ConstraintKey::testing_new("config//:sanitizer");

        let asan = ConstraintValue::testing_new("config//:sanitizer", Some("asan"));
        let tsan = ConstraintValue::testing_new("config//:sanitizer", Some("tsan"));
        let msan = ConstraintValue::testing_new("config//:sanitizer", Some("msan"));
        let none = ConstraintValue::testing_new("config//:sanitizer", Some("none"));

        let c_asan = ConfigSettingData {
            constraints: BTreeMap::from_iter([(sanitizer_key.dupe(), asan.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_tsan = ConfigSettingData {
            constraints: BTreeMap::from_iter([(sanitizer_key.dupe(), tsan.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_msan = ConfigSettingData {
            constraints: BTreeMap::from_iter([(sanitizer_key.dupe(), msan.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_none = ConfigSettingData {
            constraints: BTreeMap::from_iter([(sanitizer_key.dupe(), none.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };

        // All should be different from each other
        assert_ne!(c_asan, c_tsan);
        assert_ne!(c_asan, c_msan);
        assert_ne!(c_asan, c_none);
        assert_ne!(c_tsan, c_msan);
        assert_ne!(c_tsan, c_none);
        assert_ne!(c_msan, c_none);

        // None of them should refine each other (they're all single constraints)
        assert!(!c_asan.refines(&c_tsan));
        assert!(!c_tsan.refines(&c_asan));
    }

    #[test]
    fn refines_mixed_old_and_new_syntax() {
        // Test that old syntax (separate targets) and new syntax (subtargets) work together
        fn constraint_key(t: &str) -> ConstraintKey {
            ConstraintKey::testing_new(t)
        }

        // Old syntax - separate target for value
        let build_mode_key = constraint_key("config//:build_mode");
        let dev_old = ConstraintValue::testing_new("config//:dev", None);

        // New syntax - subtarget
        let os_key = constraint_key("config//:os");
        let linux_new = ConstraintValue::testing_new("config//:os", Some("linux"));

        let c_dev = ConfigSettingData {
            constraints: BTreeMap::from_iter([(build_mode_key.dupe(), dev_old.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_linux = ConfigSettingData {
            constraints: BTreeMap::from_iter([(os_key.dupe(), linux_new.dupe())]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };
        let c_dev_linux = ConfigSettingData {
            constraints: BTreeMap::from_iter([
                (build_mode_key.dupe(), dev_old.dupe()),
                (os_key.dupe(), linux_new.dupe()),
            ]),
            buckconfigs: BTreeMap::new(),
            root_buckconfigs: BTreeMap::new(),
        };

        // Combined should refine both
        assert!(c_dev_linux.refines(&c_dev));
        assert!(c_dev_linux.refines(&c_linux));
    }
}
