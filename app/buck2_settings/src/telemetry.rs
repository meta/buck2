/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Identifying a settings value in telemetry.
//!
//! Commands log the hash of the settings they ran under, the daemon's
//! sections together with the invocation's, and a daemon reports each
//! distinct value it serves a command with once, as the hash together with
//! the serialized settings. Keying every record on the hash keeps the
//! per-command telemetry small; the hash is the key into the
//! `buck2_settings` XDB table that ingress fills from those reports.

use std::sync::LazyLock;
use std::sync::Mutex;

use buck2_hash::BuckMutSet;

use crate::BuckSettings;

/// Prefix of every settings hash, so that one seen on its own says what it
/// identifies.
const HASH_PREFIX: &str = "buck2settings-";

/// A settings value as it is reported: the serialization and the hash that
/// identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuckSettingsSnapshot {
    /// `buck2settings-` followed by the hex digest of `json`. Identical
    /// settings hash identically across hosts and daemons, so this is the
    /// join key between a command's record and the stored settings.
    pub hash: String,
    /// The settings as JSON, the same form `buck2 status` prints as part of
    /// the daemon startup config. Keys set by no layer are `null`; their
    /// effective value is the compiled default of the buck2 version in the
    /// same record.
    pub json: String,
}

impl BuckSettings {
    pub fn snapshot(&self) -> BuckSettingsSnapshot {
        // Through `Value`, whose maps sort their keys: the hash must not
        // depend on the order sections and keys are declared in.
        let value = serde_json::to_value(self).expect("settings serialize as JSON");
        let json = serde_json::to_string(&value).expect("JSON values serialize");
        let digest = blake3::hash(json.as_bytes());
        let hash = format!("{HASH_PREFIX}{}", &digest.to_hex()[..32]);
        BuckSettingsSnapshot { hash, json }
    }
}

static OBSERVED: LazyLock<Mutex<BuckMutSet<String>>> =
    LazyLock::new(|| Mutex::new(BuckMutSet::default()));

/// The event announcing `settings`, the first time this process sees their
/// hash. Later calls with the same settings return `None`.
pub fn observe_settings(settings: &BuckSettings) -> Option<buck2_data::BuckSettingsObserved> {
    let snapshot = settings.snapshot();
    let mut observed = OBSERVED.lock().unwrap_or_else(|e| e.into_inner());
    if !observed.insert(snapshot.hash.clone()) {
        return None;
    }
    Some(buck2_data::BuckSettingsObserved {
        hash: snapshot.hash,
        settings_json: snapshot.json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::resolve_setting_flags;
    use crate::parser::table;
    use crate::settings::ALL_SECTION_METADATA;

    #[test]
    fn test_snapshot_identifies_content() -> buck2_error::Result<()> {
        let empty = BuckSettings::empty().snapshot();
        let digest = empty
            .hash
            .strip_prefix("buck2settings-")
            .expect("hash names what it identifies");
        assert_eq!(digest.len(), 32);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(empty, BuckSettings::empty().snapshot());
        // Sections in name order, not declaration order.
        let first_section = ALL_SECTION_METADATA
            .iter()
            .map(|section| section.section_name)
            .min()
            .expect("there are sections");
        assert!(
            empty.json.starts_with(&format!("{{\"{first_section}\":")),
            "{}",
            empty.json
        );

        let changed = resolve_setting_flags(vec![table(
            "[analysis]\nrecord_requested_anon_targets = false",
        )])?
        .snapshot();
        assert_ne!(changed.hash, empty.hash);
        assert!(
            changed
                .json
                .contains("\"record_requested_anon_targets\":false"),
            "{}",
            changed.json
        );
        Ok(())
    }

    #[test]
    fn test_settings_are_observed_once() -> buck2_error::Result<()> {
        // Distinct from every other test's settings, since the observed set
        // is process wide.
        let settings = resolve_setting_flags(vec![table(
            "[log_download]\nlog_url = \"https://observed-once.example/\"",
        )])?;
        let event = observe_settings(&settings).expect("first observation is reported");
        assert_eq!(event.hash, settings.snapshot().hash);
        assert_eq!(event.settings_json, settings.snapshot().json);
        assert_eq!(observe_settings(&settings), None);
        Ok(())
    }
}
