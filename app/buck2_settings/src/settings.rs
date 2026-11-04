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
use buck2_error::BuckErrorContext;
use dupe::Dupe;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::agent_build::AgentBuild;
use crate::agent_config_change::AgentConfigChange;
use crate::agent_interactive::AgentInteractive;
use crate::agent_materialization::AgentMaterialization;
use crate::analysis::Analysis;
use crate::buckconfig::BuckconfigSource;
use crate::daemon::Daemon;
use crate::digest::Digest;
use crate::event_log::EventLog;
use crate::execution_platforms::ExecutionPlatforms;
use crate::health_check::HealthCheck;
use crate::http::Http;
use crate::hydration::Hydration;
use crate::log_download::LogDownload;
use crate::materializer::Materializer;
use crate::metadata::SectionMetadata;
use crate::metadata::SectionReport;
use crate::metadata::SectionScope;
use crate::metadata::SettingsSection;
use crate::package_visibility::PackageVisibility;
use crate::resource_control::ResourceControl;
use crate::section::Section;
use crate::starlark::Starlark;

/// Assembles `BuckSettings` from the sections, each listed under its scope.
///
/// Generates the flat on-disk form (`BuckSettingsData`), the three scope
/// structs, the conversion between them, and the registry the parser and the
/// rollout selector consult. A section listed under the wrong scope fails to
/// compile.
macro_rules! buck_settings {
    (
        daemon { $($daemon:ident: $daemon_ty:ty,)* }
        invocation { $($invocation:ident: $invocation_ty:ty,)* }
        dice { $($dice:ident: $dice_ty:ty,)* }
    ) => {
        pub(crate) static ALL_SECTION_METADATA: &[SectionMetadata] = &[
            $(<$daemon_ty as SettingsSection>::METADATA,)*
            $(<$invocation_ty as SettingsSection>::METADATA,)*
            $(<$dice_ty as SettingsSection>::METADATA,)*
        ];

        const _: () = {
            $(assert!(
                matches!(<$daemon_ty as SettingsSection>::METADATA.scope, SectionScope::Daemon),
                concat!("`", stringify!($daemon_ty), "` is listed as a daemon section but declares another scope"),
            );)*
            $(assert!(
                matches!(<$invocation_ty as SettingsSection>::METADATA.scope, SectionScope::Invocation),
                concat!("`", stringify!($invocation_ty), "` is listed as an invocation section but declares another scope"),
            );)*
            $(assert!(
                matches!(<$dice_ty as SettingsSection>::METADATA.scope, SectionScope::Dice),
                concat!("`", stringify!($dice_ty), "` is listed as a dice section but declares another scope"),
            );)*
        };

        // The file format: every section at the top level, unknown ones rejected
        // before grouping by scope.
        #[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq, Allocative)]
        #[serde(deny_unknown_fields)]
        pub(crate) struct BuckSettingsData {
            $(#[serde(default)] $daemon: $daemon_ty,)*
            $(#[serde(default)] $invocation: $invocation_ty,)*
            $(#[serde(default)] $dice: $dice_ty,)*
        }

        /// Sections whose values are fixed for the daemon's lifetime. Every
        /// section in this struct participates in restart checks and may be
        /// retained in global state.
        ///
        /// ```compile_fail
        /// use buck2_settings::DaemonBuckSettings;
        /// DaemonBuckSettings::default().agent_materialization.enabled();
        /// ```
        #[derive(
            Clone,
            Dupe,
            Debug,
            Default,
            Serialize,
            Deserialize,
            PartialEq,
            Eq,
            Allocative
        )]
        #[serde(default, deny_unknown_fields)]
        pub struct DaemonBuckSettings {
            $(pub $daemon: Section<$daemon_ty>,)*
        }

        /// Sections whose values belong to one invocation, on either the client
        /// or daemon. A request carries only these sections, and missing
        /// overrides use the section's defaults.
        #[derive(
            Clone,
            Dupe,
            Debug,
            Default,
            Serialize,
            Deserialize,
            PartialEq,
            Eq,
            Allocative
        )]
        #[serde(default, deny_unknown_fields)]
        pub struct InvocationBuckSettings {
            $(pub $invocation: Section<$invocation_ty>,)*
        }

        /// Sections whose values belong to one invocation and are read only
        /// through DICE, which tracks the dependency: see `dice::HasDiceBuckSettings`.
        /// A request carries them next to the invocation sections.
        #[derive(
            Clone,
            Dupe,
            Debug,
            Default,
            Serialize,
            Deserialize,
            PartialEq,
            Eq,
            Allocative
        )]
        #[serde(default, deny_unknown_fields)]
        pub struct DiceBuckSettings {
            $(pub $dice: Section<$dice_ty>,)*
        }

        $(impl crate::dice::HasDiceSection<$dice_ty> for DiceBuckSettings {
            fn section(&self) -> &Section<$dice_ty> {
                &self.$dice
            }
        })*

        impl From<BuckSettingsData> for BuckSettings {
            fn from(data: BuckSettingsData) -> Self {
                Self {
                    daemon: DaemonBuckSettings {
                        $($daemon: Section::new(data.$daemon),)*
                    },
                    invocation: InvocationBuckSettings {
                        $($invocation: Section::new(data.$invocation),)*
                    },
                    dice: DiceBuckSettings {
                        $($dice: Section::new(data.$dice),)*
                    },
                }
            }
        }

        impl BuckSettings {
            /// Every section with its metadata and this value's keys, daemon
            /// sections first. For reporting. `buckconfig` is what keys with
            /// a declared fallback read when unset.
            pub fn sections(
                &self,
                buckconfig: &mut dyn BuckconfigSource,
            ) -> buck2_error::Result<Vec<SectionReport>> {
                Ok(vec![
                    $(SectionReport {
                        metadata: <$daemon_ty as SettingsSection>::METADATA,
                        values: self.daemon.$daemon.values(buckconfig)?,
                    },)*
                    $(SectionReport {
                        metadata: <$invocation_ty as SettingsSection>::METADATA,
                        values: self.invocation.$invocation.values(buckconfig)?,
                    },)*
                    $(SectionReport {
                        metadata: <$dice_ty as SettingsSection>::METADATA,
                        values: self.dice.$dice.values(buckconfig)?,
                    },)*
                ])
            }
        }
    };
}

buck_settings! {
    daemon {
        analysis: Analysis,
        daemon: Daemon,
        digest: Digest,
        http: Http,
        hydration: Hydration,
        log_download: LogDownload,
        materializer: Materializer,
        package_visibility: PackageVisibility,
        resource_control: ResourceControl,
    }
    invocation {
        agent_build: AgentBuild,
        agent_config_change: AgentConfigChange,
        agent_interactive: AgentInteractive,
        agent_materialization: AgentMaterialization,
        event_log: EventLog,
        health_check: HealthCheck,
    }
    dice {
        execution_platforms: ExecutionPlatforms,
        starlark: Starlark,
    }
}

/// Settings for a client invocation or server command. Shared daemon state must
/// retain only `DaemonBuckSettings`, since invocation values can change on every request.
#[derive(
    Clone,
    Dupe,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    Allocative
)]
#[serde(from = "BuckSettingsData")]
pub struct BuckSettings {
    #[serde(flatten)]
    pub daemon: DaemonBuckSettings,
    #[serde(flatten)]
    pub invocation: InvocationBuckSettings,
    // Not public: on the daemon these are read through DICE, and the only
    // other uses are sending them there and reporting them.
    #[serde(flatten)]
    dice: DiceBuckSettings,
}

impl BuckSettings {
    pub fn new(
        daemon: DaemonBuckSettings,
        invocation: InvocationBuckSettings,
        dice: DiceBuckSettings,
    ) -> Self {
        Self {
            daemon,
            invocation,
            dice,
        }
    }

    /// The `dice` sections, for the client to send with a request and the
    /// daemon to inject into its transaction. Computations read them through
    /// `dice::HasDiceBuckSettings` instead, so the dependency is recorded.
    pub fn dice_sections(&self) -> &DiceBuckSettings {
        &self.dice
    }
}

impl InvocationBuckSettings {
    pub fn from_request(request: Option<&str>) -> buck2_error::Result<Self> {
        match request {
            Some(request) => {
                serde_json::from_str(request).buck_error_context("Invalid invocation Buck Settings")
            }
            None => Ok(Self::default()),
        }
    }
}

impl DiceBuckSettings {
    pub fn from_request(request: Option<&str>) -> buck2_error::Result<Self> {
        match request {
            Some(request) => {
                serde_json::from_str(request).buck_error_context("Invalid DICE Buck Settings")
            }
            None => Ok(Self::default()),
        }
    }
}

/// A stored setting value changing from the daemon's snapshot to the current client's settings.
/// JSON null represents an unset override, which can differ from an explicit default value.
#[derive(Debug, PartialEq, Eq, derive_more::Display)]
#[display("{key}: {old_value} -> {new_value}")]
pub struct BuckSettingChange {
    pub key: String,
    pub old_value: Value,
    pub new_value: Value,
}

impl BuckSettings {
    pub fn empty() -> Self {
        Self::default()
    }
}

impl DaemonBuckSettings {
    /// Changes that require a daemon restart, from its snapshot to this invocation's settings.
    /// Stored values preserve unset overrides, and names are sorted for deterministic diagnostics.
    pub fn changes_from(&self, previous: &Self) -> Vec<BuckSettingChange> {
        if self == previous {
            return Vec::new();
        }
        self.collect_daemon_changes(previous)
    }

    #[cold]
    fn collect_daemon_changes(&self, previous: &Self) -> Vec<BuckSettingChange> {
        let previous = serde_json::to_value(previous)
            .expect("Buck Settings contain only JSON-compatible values");
        let current =
            serde_json::to_value(self).expect("Buck Settings contain only JSON-compatible values");
        let mut changes: Vec<_> = current
            .as_object()
            .expect("Daemon Buck Settings serialize as sections")
            .iter()
            .flat_map(|(section, values)| {
                values
                    .as_object()
                    .expect("Buck Settings sections serialize as objects")
                    .iter()
                    .map(move |(name, value)| (section, name, value))
            })
            .filter_map(|(section, name, new_value)| {
                let old_value = &previous[section][name];
                (old_value != new_value).then(|| BuckSettingChange {
                    key: format!("{section}.{name}"),
                    old_value: old_value.clone(),
                    new_value: new_value.clone(),
                })
            })
            .collect();
        changes.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        changes
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct TestBuckSettingsData {
        pub test_section: Option<TestSection>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct TestSection {
        pub test_flag: Option<bool>,
        pub test_value: Option<String>,
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_daemon_setting_changes() {
        let previous: BuckSettings = serde_json::from_str(
            r#"{"agent_interactive":{"enabled":true},"hydration":{"enable_paging":true},"log_download":{"log_url":"old\nvalue","log_use_manifold":true}}"#,
        ).unwrap();
        let current: BuckSettings = serde_json::from_str(
            r#"{"agent_materialization":{"enabled":false},"analysis":{"record_requested_anon_targets":false},"log_download":{"log_url":"new\"value","log_use_manifold":true}}"#,
        ).unwrap();

        let changes = current.daemon.changes_from(&previous.daemon);
        assert_eq!(
            changes,
            vec![
                BuckSettingChange {
                    key: "analysis.record_requested_anon_targets".to_owned(),
                    old_value: Value::Null,
                    new_value: Value::Bool(false),
                },
                BuckSettingChange {
                    key: "hydration.enable_paging".to_owned(),
                    old_value: Value::Bool(true),
                    new_value: Value::Null,
                },
                BuckSettingChange {
                    key: "log_download.log_url".to_owned(),
                    old_value: Value::String("old\nvalue".to_owned()),
                    new_value: Value::String("new\"value".to_owned()),
                },
            ],
        );
        assert_eq!(
            changes[2].to_string(),
            r#"log_download.log_url: "old\nvalue" -> "new\"value""#,
        );
        assert!(current.daemon.changes_from(&current.daemon).is_empty());
    }

    #[test]
    fn test_section_scope_applies_to_all_settings() -> buck2_error::Result<()> {
        let previous = BuckSettings::default();
        for (section, name) in [
            ("agent_interactive", "enabled"),
            ("agent_build", "enabled"),
            ("agent_interactive", "killall_enabled"),
            ("agent_interactive", "clean_enabled"),
            ("agent_interactive", "new_isolation_dir_enabled"),
            ("agent_config_change", "enabled"),
        ] {
            let current: BuckSettings = serde_json::from_value(serde_json::json!({
                section: {name: true},
            }))?;
            assert!(
                current.daemon.changes_from(&previous.daemon).is_empty(),
                "{section}.{name}"
            );
            let request = serde_json::to_string(&current.invocation)?;
            let values: Value = serde_json::from_str(&request)?;
            assert_eq!(values[section][name], true);
            assert!(values.get("analysis").is_none());
            assert!(values.get("log_download").is_none());
            assert_eq!(
                InvocationBuckSettings::from_request(Some(&request))?,
                current.invocation
            );
        }

        let current: BuckSettings = serde_json::from_value(serde_json::json!({
            "log_download": {"log_url": "new", "log_use_manifold": false},
        }))?;
        let changes = current.daemon.changes_from(&previous.daemon);
        assert_eq!(
            changes
                .iter()
                .map(|change| change.key.as_str())
                .collect::<Vec<_>>(),
            ["log_download.log_url", "log_download.log_use_manifold"]
        );
        let request = serde_json::to_string(&current.invocation)?;
        assert_eq!(
            InvocationBuckSettings::from_request(Some(&request))?,
            previous.invocation
        );
        Ok(())
    }

    #[test]
    fn test_flat_settings_roundtrip() -> buck2_error::Result<()> {
        let settings: BuckSettings = serde_json::from_str(
            r#"{"agent_materialization":{"enabled":true},"analysis":{"record_requested_anon_targets":false},"log_download":{"log_url":"https://example.com/"}}"#,
        )?;
        assert!(settings.invocation.agent_materialization.enabled());
        assert!(!settings.daemon.analysis.record_requested_anon_targets());
        assert_eq!(
            settings
                .daemon
                .log_download
                .log_url(&mut crate::buckconfig::NoBuckconfig)?
                .as_deref(),
            Some("https://example.com/")
        );
        let serialized = serde_json::to_value(&settings)?;
        // The flat form has no scope groups: `daemon` here is the section of
        // that name.
        assert!(serialized["daemon"].get("analysis").is_none());
        assert_eq!(serialized["daemon"]["buster"], Value::Null);
        assert!(serialized.get("invocation").is_none());
        assert_eq!(serialized["agent_materialization"]["enabled"], true);
        assert_eq!(
            serde_json::from_value::<BuckSettings>(serialized)?,
            settings
        );
        assert!(serde_json::from_str::<BuckSettings>(r#"{"unknown_section":{}}"#).is_err());
        Ok(())
    }

    #[test]
    fn test_invocation_sections_roundtrip() -> buck2_error::Result<()> {
        let client: BuckSettings = serde_json::from_str(
            r#"{"agent_materialization":{"enabled":false},"agent_interactive":{"enabled":true,"killall_enabled":true},"analysis":{"record_requested_anon_targets":false}}"#,
        )?;
        let request = serde_json::to_string(&client.invocation)?;
        let values: serde_json::Map<String, Value> = serde_json::from_str(&request)?;
        // Every invocation section travels, set or not, and nothing else.
        let mut sections: Vec<&str> = values.keys().map(String::as_str).collect();
        sections.sort_unstable();
        let mut invocation_sections: Vec<&str> = ALL_SECTION_METADATA
            .iter()
            .filter(|section| matches!(section.scope, SectionScope::Invocation))
            .map(|section| section.section_name)
            .collect();
        invocation_sections.sort_unstable();
        assert_eq!(sections, invocation_sections);
        assert_eq!(values["agent_build"]["enabled"], Value::Null);
        assert_eq!(values["agent_materialization"]["enabled"], false);
        assert_eq!(values["agent_interactive"]["killall_enabled"], true);
        assert_eq!(values["agent_interactive"]["clean_enabled"], Value::Null);
        let invocation = InvocationBuckSettings::from_request(Some(&request))?;
        assert_eq!(invocation, client.invocation);
        assert!(invocation.agent_interactive.killall_enabled());
        assert!(!invocation.agent_interactive.clean_enabled());
        assert!(!invocation.agent_materialization.enabled());
        Ok(())
    }

    #[test]
    fn test_invocation_values_do_not_leak_between_requests() -> buck2_error::Result<()> {
        let first = InvocationBuckSettings::from_request(Some(
            r#"{"agent_materialization":{"enabled":true}}"#,
        ))?;
        let second = InvocationBuckSettings::from_request(Some(
            r#"{"agent_interactive":{"enabled":true}}"#,
        ))?;
        assert!(first.agent_materialization.enabled());
        assert!(!first.agent_interactive.enabled());
        assert!(!second.agent_materialization.enabled());
        assert!(second.agent_interactive.enabled());
        for request in [
            None,
            Some("{}"),
            Some(r#"{"agent_materialization":{"enabled":null}}"#),
        ] {
            let invocation = InvocationBuckSettings::from_request(request)?;
            assert_eq!(invocation, InvocationBuckSettings::default());
            assert!(!invocation.agent_materialization.enabled());
            assert!(!invocation.agent_interactive.enabled());
        }
        Ok(())
    }

    #[test]
    fn test_invalid_invocation_settings() {
        for request in [
            "not json",
            r#"{"agent_materialization":{"enabled":"true"}}"#,
            r#"{"unknown_section":{"enabled":true}}"#,
            r#"{"analysis":{"record_requested_anon_targets":true}}"#,
        ] {
            let error = InvocationBuckSettings::from_request(Some(request)).unwrap_err();
            assert!(format!("{error:#}").contains("Invalid invocation Buck Settings"));
        }
    }

    #[test]
    fn test_sections_have_one_scope() -> buck2_error::Result<()> {
        let settings = BuckSettings::default();
        let daemon = serde_json::to_value(&settings.daemon)?;
        let invocation = serde_json::to_value(&settings.invocation)?;
        let dice = serde_json::to_value(settings.dice_sections())?;
        let daemon = daemon.as_object().unwrap();
        let invocation = invocation.as_object().unwrap();
        let dice = dice.as_object().unwrap();
        assert!(daemon.keys().all(|name| !invocation.contains_key(name)));
        assert!(
            dice.keys()
                .all(|name| !invocation.contains_key(name) && !daemon.contains_key(name))
        );
        let combined: serde_json::Map<_, _> = daemon
            .clone()
            .into_iter()
            .chain(invocation.clone())
            .chain(dice.clone())
            .collect();
        assert_eq!(
            Value::Object(combined),
            serde_json::to_value(BuckSettingsData::default())?
        );
        assert_eq!(
            serde_json::to_value(&settings)?,
            serde_json::to_value(BuckSettingsData::default())?
        );
        assert!(
            serde_json::from_str::<DaemonBuckSettings>(
                r#"{"agent_materialization":{"enabled":true}}"#,
            )
            .is_err()
        );
        Ok(())
    }
}
