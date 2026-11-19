/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What is known about every setting without a value: the declarations that
//! `#[buck_settings_section]` turns into [`SectionMetadata`].

use serde::Serialize;
use serde_json::Value;

use crate::buckconfig::BuckconfigSource;

/// Where a setting's value may come from besides the repository file and
/// rollouts, which may always set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideSource {
    CommandLine,
    LocalSettings,
}

/// Classifies a setting's source for policy validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingSource {
    /// Repo-root `.bucksettings.toml`
    Base,
    /// Wrapper-cached rollout settings.
    Rollout,
    Override(OverrideSource),
}

/// How long a section's values live and who may read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionScope {
    /// Fixed for the daemon's lifetime; a change restarts it.
    Daemon,
    /// Per invocation, read untracked from the command context.
    Invocation,
    /// Per invocation, read only through DICE, which tracks the dependency.
    /// Never set by rollouts.
    Dice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettingKeyRef<'a> {
    pub(crate) section: &'a str,
    pub(crate) name: &'a str,
}

/// The buckconfig key a setting reads when no layer set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BuckconfigFallback {
    /// `section.property`.
    pub key: &'static str,
    /// Only open source builds read it; internally every value has moved to
    /// the setting.
    pub oss_only: bool,
}

// No `PartialEq`: the default thunks are function pointers, which do not
// compare meaningfully. Compare names.
#[derive(Debug, Clone, Copy)]
pub struct SettingKeyMetadata {
    pub name: &'static str,
    /// The key's doc comment.
    pub doc: &'static str,
    /// The value's Rust type, as declared.
    pub type_name: &'static str,
    pub overridable_in: &'static [OverrideSource],
    /// The value an unset key takes in Meta-internal builds, as JSON. `None`
    /// when an unset key stays unset.
    pub internal_default: fn() -> Option<Value>,
    /// The same for open source builds.
    pub oss_default: fn() -> Option<Value>,
    /// Read, before the defaults apply, when no layer set the key.
    pub buckconfig_fallback: Option<BuckconfigFallback>,
}

impl SettingKeyMetadata {
    pub(super) fn allows_source(&self, source: SettingSource) -> bool {
        match source {
            SettingSource::Base | SettingSource::Rollout => true,
            SettingSource::Override(source) => self.overridable_in.contains(&source),
        }
    }

    /// The value an unset key takes in this build.
    pub fn default_value(&self) -> Option<Value> {
        if cfg!(fbcode_build) {
            (self.internal_default)()
        } else {
            (self.oss_default)()
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(
        name: &'static str,
        overridable_in: &'static [OverrideSource],
    ) -> Self {
        fn unset() -> Option<Value> {
            None
        }
        Self {
            name,
            doc: "",
            type_name: "",
            overridable_in,
            internal_default: unset,
            oss_default: unset,
            buckconfig_fallback: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SectionMetadata {
    pub section_name: &'static str,
    /// Selects the rollout stanza a binary applies; see `#[buck_settings_section]`.
    pub section_version: u32,
    pub scope: SectionScope,
    /// The section's doc comment.
    pub doc: &'static str,
    pub settings: &'static [SettingKeyMetadata],
}

/// One key's value in a section instance, for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingValue {
    pub name: &'static str,
    /// What the settings layers set; `Null` when none did.
    pub set: Value,
    /// What callers of the accessor see: the set value, the buckconfig
    /// fallback, or this build's default.
    pub resolved: Value,
    /// Whether `resolved` came from the declared buckconfig fallback.
    pub from_buckconfig: bool,
}

/// A section's metadata together with one instance's values.
#[derive(Debug, Clone)]
pub struct SectionReport {
    pub metadata: SectionMetadata,
    pub values: Vec<SettingValue>,
}

/// A section of Buck settings, declared with `#[buck_settings_section]`.
pub trait SettingsSection {
    const METADATA: SectionMetadata;

    /// Every key's value in this instance, in declaration order. `buckconfig`
    /// is what keys with a declared fallback read when unset.
    fn values(
        &self,
        buckconfig: &mut dyn BuckconfigSource,
    ) -> buck2_error::Result<Vec<SettingValue>>;
}

pub(crate) fn find_setting_metadata<'a>(
    sections: &'a [SectionMetadata],
    key: SettingKeyRef<'_>,
) -> Option<&'a SettingKeyMetadata> {
    sections
        .iter()
        .find(|section| section.section_name == key.section)?
        .settings
        .iter()
        .find(|metadata| metadata.name == key.name)
}
