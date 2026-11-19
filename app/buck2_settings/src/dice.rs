/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! How settings reach DICE: the daemon's sections as global data, read
//! untracked, and the `dice` sections as an injected key per command, read
//! through projections that record the dependency.

use allocative::Allocative;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceData;
use dice::DiceDataBuilder;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::NoValueSerialize;
use dice::ProjectionKey;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::DaemonBuckSettings;
use crate::DiceBuckSettings;
use crate::Section;
use crate::metadata::SettingsSection;

/// Reads the daemon's Buck settings from DICE global data. Unlike buckconfig
/// reads this is a plain lookup, not a tracked computation.
/// Only daemon-scoped settings may be consumed from this startup snapshot.
pub trait HasBuckSettings {
    /// The settings the daemon started with; defaults when none were installed
    /// (in-process tests).
    fn get_buck_settings(&self) -> DaemonBuckSettings;
}

pub trait SetBuckSettings {
    fn set_buck_settings(&mut self, settings: DaemonBuckSettings);
}

impl HasBuckSettings for DiceData {
    fn get_buck_settings(&self) -> DaemonBuckSettings {
        match self.get::<DaemonBuckSettings>() {
            Ok(settings) => settings.dupe(),
            Err(_) => DaemonBuckSettings::default(),
        }
    }
}

impl SetBuckSettings for DiceDataBuilder {
    fn set_buck_settings(&mut self, settings: DaemonBuckSettings) {
        self.set(settings)
    }
}

impl SetBuckSettings for DiceData {
    fn set_buck_settings(&mut self, settings: DaemonBuckSettings) {
        self.set(settings)
    }
}

/// The `dice` sections of the current command, injected before anything
/// computes. Equal values re-injected do not invalidate anything.
#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("DiceBuckSettingsKey")]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct DiceBuckSettingsKey;

impl InjectedKey for DiceBuckSettingsKey {
    type Value = DiceBuckSettings;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// Implemented by `DiceBuckSettings` for each of its sections; the
/// projections `#[buck_settings_section]` generates read through it.
pub trait HasDiceSection<S> {
    fn section(&self) -> &Section<S>;
}

/// A section declared with `scope = dice`: its projection key, which is how
/// a computation depends on this section and nothing else.
pub trait DiceSettingsSection: SettingsSection + Sized + 'static {
    type Key: ProjectionKey<DeriveFromKey = DiceBuckSettingsKey, Value = Section<Self>>;

    fn key() -> Self::Key;
}

pub trait SetDiceBuckSettings {
    fn set_dice_buck_settings(&mut self, settings: DiceBuckSettings) -> buck2_error::Result<()>;
}

impl SetDiceBuckSettings for DiceTransactionUpdater {
    fn set_dice_buck_settings(&mut self, settings: DiceBuckSettings) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(DiceBuckSettingsKey, settings)])?)
    }
}

/// The tracked way to read a `dice` section.
pub trait HasDiceBuckSettings {
    /// The section as injected for this command. The computation depends on
    /// this section's value, not on the other sections.
    fn dice_settings<S: DiceSettingsSection>(
        &mut self,
    ) -> impl Future<Output = buck2_error::Result<Section<S>>> + Send;
}

impl HasDiceBuckSettings for DiceComputations<'_> {
    async fn dice_settings<S: DiceSettingsSection>(&mut self) -> buck2_error::Result<Section<S>> {
        let injected = self.compute_opaque(&DiceBuckSettingsKey).await?;
        Ok(self.projection(&injected, &S::key())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuckSettings;

    #[test]
    fn test_dice_exposes_only_daemon_settings() -> buck2_error::Result<()> {
        let mut data = DiceData::new();
        assert!(
            data.get_buck_settings()
                .analysis
                .record_requested_anon_targets()
        );
        let settings: BuckSettings = serde_json::from_str(
            r#"{"analysis":{"record_requested_anon_targets":false},"agent_materialization":{"enabled":true}}"#,
        )?;
        data.set_buck_settings(settings.daemon);
        assert!(
            !data
                .get_buck_settings()
                .analysis
                .record_requested_anon_targets()
        );
        assert!(data.get::<BuckSettings>().is_err());
        Ok(())
    }
}
