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
use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_common::legacy_configs::configs::parse_config_section_and_key;
use buck2_core::configuration::config_setting::ConfigSettingData;
use buck2_core::configuration::data::ConfigurationDataData;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutSet;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::UnpackAndDiscard;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueOfUnchecked;
use starlark::values::ValueTyped;
use starlark::values::dict::AllocDict;
use starlark::values::dict::DictMut;
use starlark::values::dict::DictRef;
use starlark::values::dict::DictType;
use starlark::values::dict::UnpackDictEntries;
use starlark::values::none::NoneOr;

use crate::constraint_setting_info::ConstraintSettingInfo;
use crate::constraint_value_info::ConstraintValueInfo;

/// Provider that signals that a rule contains configuration info. This is used both as part of
/// defining configurations (`platform()`, `constraint_value()`) and defining whether a target "matches"
/// a configuration or not (`config_setting()`, `constraint_value()`)
#[internal_provider(
    configuration_info_creator,
    methods = configuration_info_methods,
)]
#[derive(Debug, Trace, Freeze, ProvidesStaticType, Allocative, StarlarkPagable)]
#[repr(C)]
pub struct ConfigurationInfo<'v> {
    constraints: ValueOfUnchecked<'v, DictType<StarlarkTargetLabel, ConstraintValueInfo<'static>>>,
    values: ValueOfUnchecked<'v, DictType<String, String>>,
    root_values: ValueOfUnchecked<'v, DictType<String, String>>,
}

impl<'v> ConfigurationInfo<'v> {
    pub fn to_config_setting_data(&self) -> ConfigSettingData {
        let constraints =
            DictRef::from_value(self.constraints.get()).expect("type checked on construction");
        let mut converted_constraints = BTreeMap::new();
        for (k, v) in constraints.iter() {
            let key_target = StarlarkTargetLabel::from_value(k.to_value())
                .expect("type checked on construction");
            let value_target = ConstraintValueInfo::from_value(v.to_value())
                .expect("type checked on construction");
            let (constraint_key, constraint_value) = value_target.to_constraint_key_value();
            debug_assert_eq!(
                key_target.label(),
                &constraint_key.key,
                "dict key should match constraint setting label"
            );
            converted_constraints.insert(constraint_key, constraint_value);
        }

        let values = DictRef::from_value(self.values.get()).expect("type checked on construction");
        let mut converted_values = BTreeMap::new();
        for (k, v) in values.iter() {
            let key_config = k.to_value().to_str();
            let value_config = v.to_value().to_str();
            converted_values.insert(key_config, value_config);
        }

        let root_values =
            DictRef::from_value(self.root_values.get()).expect("type checked on construction");
        let mut converted_root_values = BTreeMap::new();
        for (k, v) in root_values.iter() {
            let key_config = k.to_value().to_str();
            let value_config = v.to_value().to_str();
            converted_root_values.insert(key_config, value_config);
        }

        ConfigSettingData {
            constraints: converted_constraints,
            buckconfigs: converted_values,
            root_buckconfigs: converted_root_values,
        }
    }

    pub fn to_configuration_data(&self) -> buck2_error::Result<ConfigurationDataData> {
        let ConfigSettingData {
            constraints,
            buckconfigs,
            root_buckconfigs,
        } = self.to_config_setting_data();
        if !buckconfigs.is_empty() || !root_buckconfigs.is_empty() {
            return Err(ConfigurationInfoError::BuckConfigsNotAllowed.into());
        }
        Ok(ConfigurationDataData { constraints })
    }

    /// Create a provider from configuration data.
    pub fn from_configuration_data(conf: &ConfigurationDataData, heap: Heap<'v>) -> Self {
        let mut constraints = BuckMutInsertionMap::new();
        for (k, v) in &conf.constraints {
            let constraint_setting_label = heap.alloc_typed(StarlarkTargetLabel::new(k.key.dupe()));
            let default_value = k
                .default
                .as_ref()
                .map(|v| heap.alloc_typed(StarlarkProvidersLabel::new(v.0.dupe())));
            let constraint_value_label = heap.alloc_typed(StarlarkProvidersLabel::new(v.0.dupe()));
            let constraint_setting = heap.alloc_typed(ConstraintSettingInfo::new(
                constraint_setting_label,
                NoneOr::from_option(default_value),
            ));
            let constraint_value =
                ConstraintValueInfo::new(constraint_setting, constraint_value_label);
            let prev = constraints.insert_hashed(
                constraint_setting_label
                    .to_value()
                    .get_hashed()
                    .expect("StarlarkTargetLabel is hashable"),
                heap.alloc(constraint_value),
            );
            assert!(prev.is_none());
        }

        ConfigurationInfo {
            constraints: ValueOfUnchecked::new(heap.alloc(constraints)),
            values: heap.alloc_typed_unchecked(AllocDict([("", ""); 0])).cast(),
            root_values: heap.alloc_typed_unchecked(AllocDict([("", ""); 0])).cast(),
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ConfigurationInfoError {
    #[error("key `{0}` in constraints dict does not match constraint value `{1}`")]
    ConstraintsKeyValueMismatch(String, String),
    #[error(
        "`ConfigurationInfo` cannot have buckconfigs or root buckconfigs when it is used to create a platform"
    )]
    BuckConfigsNotAllowed,
    #[error(
        "key `{0}` appears in both `values` and `root_values`; a buckconfig may only be checked in one cell"
    )]
    DuplicateKeyAcrossValuesAndRootValues(String),
}

/// Helper function to validate and build a constraints map from dictionary entries.
fn build_constraints_map_from_dict<'v>(
    dict_entries: UnpackDictEntries<
        ValueTyped<'v, StarlarkTargetLabel>,
        ValueTyped<'v, ConstraintValueInfo<'v>>,
    >,
) -> starlark::Result<BuckMutInsertionMap<Value<'v>, Value<'v>>> {
    let mut new_constraints = BuckMutInsertionMap::new();

    for (constraint_setting, constraint_value) in dict_entries.entries {
        let constraint_setting_hashed = constraint_setting
            .to_value()
            .get_hashed()
            .expect("should be hashable, we picked it from dict");
        let constraint_setting_from_constraint_value = constraint_value.setting().label();
        if *constraint_setting != *constraint_setting_from_constraint_value {
            return Err(buck2_error::Error::from(
                ConfigurationInfoError::ConstraintsKeyValueMismatch(
                    constraint_setting.to_string(),
                    constraint_value.to_string(),
                ),
            )
            .into());
        }
        let prev =
            new_constraints.insert_hashed(constraint_setting_hashed, constraint_value.to_value());
        assert!(prev.is_none());
    }

    Ok(new_constraints)
}

#[starlark_module]
fn configuration_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ConfigurationInfo<'static>)]
    fn ConfigurationInfo<'v>(
        #[starlark(require = named)] constraints: UnpackDictEntries<
            ValueTyped<'v, StarlarkTargetLabel>,
            ValueTyped<'v, ConstraintValueInfo<'v>>,
        >,
        #[starlark(require = named)] values: ValueOf<
            'v,
            UnpackDictEntries<&'v str, UnpackAndDiscard<&'v str>>,
        >,
        #[starlark(
            require = named,
            default = ValueOf::new_unchecked(starlark::values::Value::new_empty_dict())
        )]
        root_values: ValueOf<'v, UnpackDictEntries<&'v str, UnpackAndDiscard<&'v str>>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ConfigurationInfo<'v>> {
        let new_constraints = build_constraints_map_from_dict(constraints)?;

        let value_entries = values.unpack().entries;
        let root_value_entries = root_values.unpack().entries;
        for (k, _) in &value_entries {
            parse_config_section_and_key(k, None)?;
        }
        for (k, _) in &root_value_entries {
            parse_config_section_and_key(k, None)?;
        }

        let value_keys: BuckMutSet<&str> = value_entries.iter().map(|(k, _)| *k).collect();
        for (k, _) in &root_value_entries {
            if value_keys.contains(k) {
                return Err(buck2_error::Error::from(
                    ConfigurationInfoError::DuplicateKeyAcrossValuesAndRootValues(k.to_string()),
                )
                .into());
            }
        }

        Ok(ConfigurationInfo {
            constraints: ValueOfUnchecked::new(eval.heap().alloc(new_constraints)),
            values: values.as_unchecked().cast(),
            root_values: root_values.as_unchecked().cast(),
        })
    }
}

// Explicit methods definition for ConfigurationInfo provider.
#[starlark_module]
fn configuration_info_methods(builder: &mut MethodsBuilder) {
    /// A dictionary mapping constraint setting labels to their corresponding constraint values.
    #[starlark(attribute)]
    fn constraints<'v>(
        this: &ConfigurationInfo<'v>,
    ) -> starlark::Result<
        ValueOfUnchecked<'v, DictType<StarlarkTargetLabel, ConstraintValueInfo<'static>>>,
    > {
        Ok(this.constraints)
    }

    /// A dictionary of buckconfig section.key pairs and their values (evaluated from target cell).
    #[starlark(attribute)]
    fn values<'v>(
        this: &ConfigurationInfo<'v>,
    ) -> starlark::Result<ValueOfUnchecked<'v, DictType<String, String>>> {
        Ok(this.values)
    }

    /// A dictionary of buckconfig section.key pairs and their values (evaluated from root cell).
    #[starlark(attribute)]
    fn root_values<'v>(
        this: &ConfigurationInfo<'v>,
    ) -> starlark::Result<ValueOfUnchecked<'v, DictType<String, String>>> {
        Ok(this.root_values)
    }

    /// Get a constraint value by its constraint setting.
    ///
    /// Accepts a `ConstraintSettingInfo` and returns the corresponding
    /// `ConstraintValueInfo`. If the constraint is not set,
    /// returns the default constraint value from the setting (if defined).
    ///
    /// - Arguments
    ///
    ///     - `key`: A `ConstraintSettingInfo` to look up
    ///
    /// - Returns
    ///
    /// The `ConstraintValueInfo` for the given setting. If not set in the
    /// constraints, returns the default constraint value from the setting.
    /// Returns `None` only if the constraint is not present and the constraint has no default.
    ///
    /// ### Example
    ///
    /// ```python
    /// # Get constraint value by setting
    /// cpu_setting = ref.cpu_setting[ConstraintSettingInfo]
    /// value = config_info.get(cpu_setting)
    /// if value:
    ///     print("CPU constraint: {}".format(value))
    /// ```
    fn get<'v>(
        this: &ConfigurationInfo<'v>,
        #[starlark(require = pos)] key: ValueTyped<'v, ConstraintSettingInfo<'v>>,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueTyped<'v, ConstraintValueInfo<'v>>>> {
        let constraints =
            DictRef::from_value(this.constraints.get()).expect("type checked on construction");

        let label = key.label();
        match constraints.get(label.to_value())? {
            Some(v) => {
                let v = ValueTyped::new_err(v).expect("type checked on construction");
                Ok(NoneOr::Other(v))
            }
            // if not find in the constraints, we return the default constraint value
            None => Ok(get_default_constraint_value(key, heap)),
        }
    }

    /// Insert a ConstraintValueInfo into the constraints.
    ///
    /// - Arguments
    ///
    ///     - `value`: The `ConstraintValueInfo` to insert
    ///
    /// - Returns
    ///
    /// The previously set `ConstraintValueInfo` for this setting, if any.
    /// If no previous value existed, returns the default constraint value from the setting
    /// (if defined). Returns `None` only if there was no previous value and the constraint
    /// has no default.
    ///
    /// ### Example
    ///
    /// ```python
    /// # Insert a new constraint value
    /// new_value = ref.some_constraint[ConstraintValueInfo]
    /// old_value = config_info.insert(new_value)
    /// if old_value:
    ///     print("Replaced previous value: {}".format(old_value))
    /// ```
    fn insert<'v>(
        this: &ConfigurationInfo<'v>,
        #[starlark(require = pos)] value: ValueTyped<'v, ConstraintValueInfo<'v>>,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueTyped<'v, ConstraintValueInfo<'v>>>> {
        let constraint_value = value.as_ref();
        let setting_info = constraint_value.setting();
        let label = setting_info.label();

        let mut constraints = DictMut::from_value(this.constraints.get())?;

        let v = constraints.aref.insert_hashed(
            label
                .to_value()
                .get_hashed()
                .expect("StarlarkTargetLabel is hashable"),
            value.to_value(),
        );

        match v {
            Some(v) => {
                let v = ValueTyped::new_err(v).expect("type checked on construction");
                Ok(NoneOr::Other(v))
            }
            // if not found in the constraints, we return the default constraint value
            None => Ok(get_default_constraint_value(setting_info, heap)),
        }
    }

    /// Remove and return a constraint value by its setting.
    ///
    /// Accepts a `ConstraintSettingInfo` and returns the corresponding
    /// `ConstraintValueInfo`. If the constraint is not set,
    /// returns the default constraint value from the setting (if defined).
    ///
    /// - Arguments
    ///
    ///     - `key`: A `ConstraintSettingInfo` to remove
    ///
    /// - Returns
    ///
    /// The removed `ConstraintValueInfo` if it was set.
    /// If not present, returns the default constraint value from the setting (if defined).
    /// Returns `None` only if the constraint was not set and the constraint has no default.
    ///
    /// ### Example
    ///
    /// ```python
    /// # Remove a constraint
    /// cpu_setting = ref.cpu_setting[ConstraintSettingInfo]
    /// removed_value = config_info.pop(cpu_setting)
    /// if removed_value:
    ///     print("Removed: {}".format(removed_value))
    /// ```
    fn pop<'v>(
        this: &ConfigurationInfo<'v>,
        #[starlark(require = pos)] key: ValueTyped<'v, ConstraintSettingInfo<'v>>,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<ValueTyped<'v, ConstraintValueInfo<'v>>>> {
        let label = key.label();
        let mut constraints = DictMut::from_value(this.constraints.get())?;

        // Remove and return the value
        let removed = constraints.aref.remove_hashed(
            label
                .to_value()
                .get_hashed()
                .expect("StarlarkTargetLabel is hashable"),
        );

        match removed {
            Some(v) => {
                let v = ValueTyped::new_err(v).expect("type checked on construction");
                Ok(NoneOr::Other(v))
            }
            // if not found in the constraints, we return the default constraint value
            None => Ok(get_default_constraint_value(key, heap)),
        }
    }

    /// Create a copy of the ConfigurationInfo.
    ///
    /// Returns a new `ConfigurationInfo` instance with copies of the original one.
    /// Operations on the copy will not affect the original.
    ///
    /// - Returns
    ///
    /// A new `ConfigurationInfo` with the same constraints and values as the original.
    ///
    /// ### Example
    ///
    /// ```python
    /// # Create a copy and modify it without affecting the original
    /// config_copy = config_info.copy()
    /// config_copy.insert(new_constraint_value)
    /// # Original config_info remains unchanged
    /// ```
    fn copy<'v>(
        this: &ConfigurationInfo<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ConfigurationInfo<'v>> {
        // Copy constraints dict
        let constraints =
            DictRef::from_value(this.constraints.get()).expect("type checked on construction");
        let mut new_constraints = BuckMutInsertionMap::new();
        for (k, v) in constraints.iter() {
            let key_hashed = k.get_hashed().expect("should be hashable");
            new_constraints.insert_hashed(key_hashed, v);
        }

        // Copy values dict
        let values = DictRef::from_value(this.values.get()).expect("type checked on construction");
        let mut new_values = BuckMutInsertionMap::new();
        for (k, v) in values.iter() {
            let key_hashed = k.get_hashed().expect("should be hashable");
            new_values.insert_hashed(key_hashed, v);
        }

        Ok(ConfigurationInfo {
            constraints: ValueOfUnchecked::new(heap.alloc(new_constraints)),
            values: ValueOfUnchecked::new(heap.alloc(new_values)),
            root_values: {
                let root_values = DictRef::from_value(this.root_values.get())
                    .expect("type checked on construction");
                let mut new_root_values = BuckMutInsertionMap::new();
                for (k, v) in root_values.iter() {
                    let key_hashed = k.get_hashed().expect("should be hashable");
                    new_root_values.insert_hashed(key_hashed, v);
                }
                ValueOfUnchecked::new(heap.alloc(new_root_values))
            },
        })
    }
}

/// Helper function to get the default constraint value from a constraint setting.
/// Returns the default constraint value if one exists, otherwise returns None.
fn get_default_constraint_value<'v>(
    key: ValueTyped<'v, ConstraintSettingInfo<'v>>,
    heap: Heap<'v>,
) -> NoneOr<ValueTyped<'v, ConstraintValueInfo<'v>>> {
    NoneOr::from_option(
        ConstraintValueInfo::default_from_constraint_setting(key)
            .map(|constraint_value| heap.alloc_typed(constraint_value)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_configuration_data_rejects_root_buckconfigs_with_clear_error() {
        Heap::temp(|heap| {
            let info = ConfigurationInfo {
                constraints: ValueOfUnchecked::new(
                    heap.alloc(BuckMutInsertionMap::<Value, Value>::new()),
                ),
                values: heap.alloc_typed_unchecked(AllocDict([("", ""); 0])).cast(),
                root_values: heap
                    .alloc_typed_unchecked(AllocDict([("apple.key", "value")]))
                    .cast(),
            };

            let Err(err) = info.to_configuration_data() else {
                panic!("root buckconfigs should not be accepted for platforms");
            };

            assert!(
                err.to_string().contains("buckconfigs or root buckconfigs"),
                "unexpected error: {err:#}"
            );
        });
    }
}
