/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::str::FromStr;
use std::sync::Arc;

use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckFixedSortedMap;
use buck2_settings::buckconfig::BuckconfigSource;
use buck2_util::env_vars::substitute_env_vars;

use crate::legacy_configs::configs::ConfigValue;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::configs::LegacyBuckConfigSection;
use crate::legacy_configs::configs::LegacyBuckConfigValue;
use crate::legacy_configs::key::BuckconfigKeyRef;
use crate::legacy_configs::view::LegacyBuckConfigView;

/// Read the `[buck2_metadata]` section from a `LegacyBuckConfig` and resolve any `$VAR`
/// references. Entries whose env vars are not set are skipped and reported.
pub fn parse_buckconfig_metadata(config: &LegacyBuckConfig) -> BuckFixedSortedMap<String, String> {
    let Some(section) = config.get_section("buck2_metadata") else {
        return BuckFixedSortedMap::default();
    };
    section
        .iter()
        .filter_map(|(key, value)| match substitute_env_vars(value.as_str()) {
            Ok(resolved) => Some((key.to_owned(), resolved)),
            Err(e) => {
                soft_error_or_panic!(
                    "buckconfig_metadata_env_var_unset",
                    e.context(format!("Skipping [buck2_metadata] key `{}`", key))
                );
                None
            }
        })
        .collect()
}

impl LegacyBuckConfigView for &LegacyBuckConfig {
    fn get(&mut self, key: BuckconfigKeyRef) -> buck2_error::Result<Option<Arc<str>>> {
        Ok(LegacyBuckConfig::get(self, key).map(|v| v.to_owned().into()))
    }
}

impl BuckconfigSource for &LegacyBuckConfig {
    fn get(&mut self, section: &str, property: &str) -> buck2_error::Result<Option<String>> {
        Ok(LegacyBuckConfig::get(self, BuckconfigKeyRef { section, property }).map(str::to_owned))
    }
}

impl LegacyBuckConfigSection {
    /// configs are equal if the data they resolve in is equal, regardless of the origin of the config
    pub(crate) fn compare(&self, other: &Self) -> bool {
        self.values.len() == other.values.len()
            && self.values.iter().all(|(name, value)| {
                other
                    .values
                    .get(name)
                    .is_some_and(|other_val| other_val.as_str() == value.as_str())
            })
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, LegacyBuckConfigValue<'_>)> {
        self.values
            .iter()
            .map(move |(key, value)| (key.as_str(), LegacyBuckConfigValue { value }))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.values.keys()
    }

    pub fn get(&self, key: &str) -> Option<LegacyBuckConfigValue<'_>> {
        self.values
            .get(key)
            .map(move |value| LegacyBuckConfigValue { value })
    }
}

impl LegacyBuckConfig {
    fn get_config_value(&self, key: BuckconfigKeyRef) -> Option<&ConfigValue> {
        let BuckconfigKeyRef { section, property } = key;
        self.0
            .values
            .get(section)
            .and_then(|s| s.values.get(property))
    }

    pub fn get(&self, key: BuckconfigKeyRef) -> Option<&str> {
        self.get_config_value(key).map(|s| s.as_str())
    }

    /// Iterate all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&str, impl IntoIterator<Item = (&str, &str)>)> {
        self.0.values.iter().map(|(section, section_values)| {
            (
                section.as_str(),
                section_values
                    .values
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
        })
    }

    fn parse_impl<T: FromStr>(key: BuckconfigKeyRef, value: &str) -> buck2_error::Result<T>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
    {
        let BuckconfigKeyRef { section, property } = key;
        value
            .parse()
            .map_err(buck2_error::Error::from)
            .with_buck_error_context(|| {
                format!(
                    "Invalid value for buckconfig `{}.{}`: conversion to {} failed, value as `{}`",
                    section.to_owned(),
                    property.to_owned(),
                    std::any::type_name::<T>(),
                    value.to_owned(),
                )
            })
    }

    pub fn parse<T: FromStr>(&self, key: BuckconfigKeyRef) -> buck2_error::Result<Option<T>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
    {
        self.get_config_value(key)
            .map(|s| {
                Self::parse_impl(key, s.as_str()).with_buck_error_context(|| {
                    format!("Defined {}", s.source.as_legacy_buck_config_location())
                })
            })
            .transpose()
    }

    pub fn parse_value<T: FromStr>(
        key: BuckconfigKeyRef,
        value: Option<&str>,
    ) -> buck2_error::Result<Option<T>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
    {
        value.map(|s| Self::parse_impl(key, s)).transpose()
    }

    pub fn parse_list<T: FromStr>(
        &self,
        key: BuckconfigKeyRef,
    ) -> buck2_error::Result<Option<Vec<T>>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
    {
        Self::parse_list_value(key, self.get(key))
    }

    pub fn parse_list_value<T: FromStr>(
        key: BuckconfigKeyRef,
        value: Option<&str>,
    ) -> buck2_error::Result<Option<Vec<T>>>
    where
        buck2_error::Error: From<<T as FromStr>::Err>,
    {
        /// A wrapper type so we can use .parse() on this.
        struct ParseList<T>(Vec<T>);

        impl<T> FromStr for ParseList<T>
        where
            T: FromStr,
        {
            type Err = <T as FromStr>::Err;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(
                    s.split(',').map(T::from_str).collect::<Result<_, _>>()?,
                ))
            }
        }

        Ok(Self::parse_value::<ParseList<T>>(key, value)?.map(|l| l.0))
    }

    pub fn sections(&self) -> impl Iterator<Item = &String> {
        self.0.values.keys()
    }

    pub fn all_sections(&self) -> impl Iterator<Item = (&String, &LegacyBuckConfigSection)> + '_ {
        self.0.values.iter()
    }

    pub fn get_section(&self, section: &str) -> Option<&LegacyBuckConfigSection> {
        self.0.values.get(section)
    }

    /// configs are equal if the data they resolve in is equal, regardless of the origin of the config
    pub(crate) fn compare(&self, other: &Self) -> bool {
        self.0.values.len() == other.0.values.len()
            && self.0.values.iter().all(|(section_name, section)| {
                other
                    .0
                    .values
                    .get(section_name)
                    .is_some_and(|other_sec| other_sec.compare(section))
            })
    }
}
