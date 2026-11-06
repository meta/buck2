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
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictMut;
use starlark::values::dict::DictType;
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
    constraints: ValueOf<
        'v,
        DictType<ValueTyped<'v, StarlarkTargetLabel>, ValueTyped<'v, ConstraintValueInfo<'v>>>,
    >,
    values: ValueOf<'v, DictType<StringValue<'v>, StringValue<'v>>>,
    root_values: ValueOf<'v, DictType<StringValue<'v>, StringValue<'v>>>,
}

type Constraints<'v> = ValueOf<
    'v,
    DictType<ValueTyped<'v, StarlarkTargetLabel>, ValueTyped<'v, ConstraintValueInfo<'v>>>,
>;
type Buckconfigs<'v> = ValueOf<'v, DictType<StringValue<'v>, StringValue<'v>>>;

impl<'v> ConfigurationInfo<'v> {
    pub fn to_config_setting_data(&self) -> buck2_error::Result<ConfigSettingData> {
        let mut constraints = BTreeMap::new();
        for entry in self.constraints.iter() {
            let (key, value) = entry?;
            let (constraint_key, constraint_value) = value.unpack().to_constraint_key_value();
            debug_assert_eq!(
                key.unpack().label(),
                &constraint_key.key,
                "dict key should match constraint setting label"
            );
            constraints.insert(constraint_key, constraint_value);
        }
        Ok(ConfigSettingData {
            constraints,
            buckconfigs: buckconfig_map(self.values)?,
            root_buckconfigs: buckconfig_map(self.root_values)?,
        })
    }

    pub fn to_configuration_data(&self) -> buck2_error::Result<ConfigurationDataData> {
        let ConfigSettingData {
            constraints,
            buckconfigs,
            root_buckconfigs,
        } = self.to_config_setting_data()?;
        if !buckconfigs.is_empty() || !root_buckconfigs.is_empty() {
            return Err(ConfigurationInfoError::BuckConfigsNotAllowed.into());
        }
        Ok(ConfigurationDataData { constraints })
    }

    /// Create a provider from configuration data.
    pub fn from_configuration_data(conf: &ConfigurationDataData, heap: Heap<'v>) -> Self {
        let constraints = conf.constraints.iter().map(|(k, v)| {
            let constraint_setting_label = heap.alloc_typed(StarlarkTargetLabel::new(k.key.dupe()));
            let default_value = k
                .default
                .as_ref()
                .map(|v| heap.alloc_typed(StarlarkProvidersLabel::new(v.0.dupe())));
            let constraint_value_label = heap.alloc_typed(StarlarkProvidersLabel::new(v.0.dupe()));
            let constraint_setting = heap.alloc_typed(ConstraintSettingInfo::new(
                constraint_setting_label,
                default_value,
            ));
            let constraint_value = heap.alloc_typed(ConstraintValueInfo::new(
                constraint_setting,
                constraint_value_label,
            ));
            (
                ValueOf::from(constraint_setting_label),
                ValueOf::from(constraint_value),
            )
        });
        ConfigurationInfo {
            constraints: ValueOf::alloc_dict(heap, constraints),
            values: ValueOf::alloc_dict(heap, []),
            root_values: ValueOf::alloc_dict(heap, []),
        }
    }
}

fn buckconfig_map<'v>(values: Buckconfigs<'v>) -> buck2_error::Result<BTreeMap<String, String>> {
    values
        .iter()
        .map(|entry| {
            let (k, v) = entry?;
            Ok((
                k.unpack().as_str().to_owned(),
                v.unpack().as_str().to_owned(),
            ))
        })
        .collect()
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

/// The provider's own copy of `constraints`, so that its mutating methods do not reach the
/// caller's dict; each key must name its value's setting.
fn copy_constraints<'v>(
    constraints: Constraints<'v>,
    heap: Heap<'v>,
) -> starlark::Result<Constraints<'v>> {
    let mut entries = Vec::with_capacity(constraints.dict().len());
    for entry in constraints.iter() {
        let (setting, value) = entry?;
        if *setting.unpack() != *value.unpack().setting().label() {
            return Err(buck2_error::Error::from(
                ConfigurationInfoError::ConstraintsKeyValueMismatch(
                    setting.unpack().to_string(),
                    value.unpack().to_string(),
                ),
            )
            .into());
        }
        entries.push((setting, value));
    }
    Ok(ValueOf::alloc_dict(heap, entries))
}

fn copy_dict<'v, K: UnpackValue<'v>, V: UnpackValue<'v>>(
    dict: ValueOf<'v, DictType<K, V>>,
    heap: Heap<'v>,
) -> starlark::Result<ValueOf<'v, DictType<K, V>>> {
    Ok(ValueOf::alloc_dict(
        heap,
        dict.iter().collect::<starlark::Result<Vec<_>>>()?,
    ))
}

#[starlark_module]
fn configuration_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ConfigurationInfo<'static>)]
    fn ConfigurationInfo<'v>(
        #[starlark(require = named)] constraints: Constraints<'v>,
        #[starlark(require = named)] values: Buckconfigs<'v>,
        #[starlark(require = named, default = ValueOf::new_unchecked(Value::new_empty_dict()))]
        root_values: Buckconfigs<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ConfigurationInfo<'v>> {
        let constraints = copy_constraints(constraints, eval.heap())?;

        let mut value_keys = BuckMutSet::default();
        for entry in values.iter() {
            let (k, _) = entry?;
            let k = k.unpack().as_str();
            parse_config_section_and_key(k, None)?;
            value_keys.insert(k);
        }
        for entry in root_values.iter() {
            let (k, _) = entry?;
            let k = k.unpack().as_str();
            parse_config_section_and_key(k, None)?;
            if value_keys.contains(k) {
                return Err(buck2_error::Error::from(
                    ConfigurationInfoError::DuplicateKeyAcrossValuesAndRootValues(k.to_owned()),
                )
                .into());
            }
        }

        Ok(ConfigurationInfo {
            constraints,
            values,
            root_values,
        })
    }
}

// Explicit methods definition for ConfigurationInfo provider.
#[starlark_module]
fn configuration_info_methods(builder: &mut MethodsBuilder) {
    /// A dictionary mapping constraint setting labels to their corresponding constraint values.
    #[starlark(attribute)]
    fn constraints<'v>(this: &ConfigurationInfo<'v>) -> starlark::Result<Constraints<'v>> {
        Ok(this.constraints)
    }

    /// A dictionary of buckconfig section.key pairs and their values (evaluated from target cell).
    #[starlark(attribute)]
    fn values<'v>(this: &ConfigurationInfo<'v>) -> starlark::Result<Buckconfigs<'v>> {
        Ok(this.values)
    }

    /// A dictionary of buckconfig section.key pairs and their values (evaluated from root cell).
    #[starlark(attribute)]
    fn root_values<'v>(this: &ConfigurationInfo<'v>) -> starlark::Result<Buckconfigs<'v>> {
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
        match this.constraints.get(key.label().to_value())? {
            Some(v) => Ok(NoneOr::Other(v.unpack())),
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

        let mut constraints = DictMut::from_value(this.constraints.to_value())?;

        let previous = constraints.aref.insert_hashed(
            label
                .to_value()
                .get_hashed()
                .expect("StarlarkTargetLabel is hashable"),
            value.to_value(),
        );

        match previous {
            Some(v) => Ok(NoneOr::Other(ValueTyped::new_err(v)?)),
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
        let mut constraints = DictMut::from_value(this.constraints.to_value())?;

        // Remove and return the value
        let removed = constraints.aref.remove_hashed(
            label
                .to_value()
                .get_hashed()
                .expect("StarlarkTargetLabel is hashable"),
        );

        match removed {
            Some(v) => Ok(NoneOr::Other(ValueTyped::new_err(v)?)),
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
        Ok(ConfigurationInfo {
            constraints: copy_dict(this.constraints, heap)?,
            values: copy_dict(this.values, heap)?,
            root_values: copy_dict(this.root_values, heap)?,
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
                constraints: ValueOf::alloc_dict(heap, []),
                values: ValueOf::alloc_dict(heap, []),
                root_values: ValueOf::alloc_dict(
                    heap,
                    [(
                        ValueOf::from(heap.alloc_str("apple.key")),
                        ValueOf::from(heap.alloc_str("value")),
                    )],
                ),
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
