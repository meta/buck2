/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Selection of settings rollouts from the versioned TOML cache.

use crate::metadata::SectionMetadata;
use crate::metadata::SectionScope;
use crate::parser::IgnoredSetting;
use crate::settings::ALL_SECTION_METADATA;
use crate::settings::BuckSettingsData;

/// What the selector took from the cache: the layer to merge, and what it
/// left out and why.
#[derive(Debug, Default, PartialEq)]
pub(super) struct SelectedRollout {
    pub(super) layer: toml::Table,
    pub(super) ignored: Vec<IgnoredSetting>,
}

/// Selects, for every registered section, the newest stanza at or below the
/// compiled section version, and keeps the keys of it this binary can use.
///
/// A stanza applies from its version on, until a newer one supersedes it. A
/// newer stanza never reaches an older binary, which is what lets a rollout
/// use keys the older binary does not have. Within the selected stanza a key
/// this binary does not know or cannot parse is left out on its own, so one
/// mistake in a rollout costs that key and nothing else.
pub(super) fn select_rollout_table(versioned_settings: toml::Table) -> SelectedRollout {
    select_with_registry(versioned_settings, ALL_SECTION_METADATA, validate_key)
}

fn select_with_registry(
    mut versioned_settings: toml::Table,
    sections: &[SectionMetadata],
    validate: impl Fn(&SectionMetadata, &str, &toml::Value) -> Result<(), String>,
) -> SelectedRollout {
    let mut selected = SelectedRollout::default();

    for section in sections {
        let Some(versions) = versioned_settings.remove(section.section_name) else {
            continue;
        };
        if matches!(section.scope, SectionScope::Dice) {
            selected.ignored.push(IgnoredSetting::section(
                section.section_name,
                "DICE-tracked sections take no rollouts",
            ));
            continue;
        }
        let toml::Value::Table(mut versions) = versions else {
            selected.ignored.push(IgnoredSetting::section(
                section.section_name,
                "not a table of versions",
            ));
            continue;
        };

        let Some(version) = newest_version_at_or_below(&versions, section.section_version) else {
            continue;
        };
        let Some(toml::Value::Table(stanza)) = versions.remove(&version) else {
            selected.ignored.push(IgnoredSetting::section(
                section.section_name,
                format!("version `{version}` is not a table of settings"),
            ));
            continue;
        };

        let mut kept = toml::Table::new();
        for (key, value) in stanza {
            match validate(section, &key, &value) {
                Ok(()) => {
                    kept.insert(key, value);
                }
                Err(reason) => {
                    selected
                        .ignored
                        .push(IgnoredSetting::key(section.section_name, &key, reason));
                }
            }
        }
        if !kept.is_empty() {
            selected
                .layer
                .insert(section.section_name.to_owned(), toml::Value::Table(kept));
        }
    }

    selected
}

/// The key of the newest stanza that applies to `compiled`. Keys that are not
/// versions are not stanzas.
fn newest_version_at_or_below(versions: &toml::Table, compiled: u32) -> Option<String> {
    versions
        .keys()
        .filter_map(|key| {
            let version: u32 = key.parse().ok()?;
            (version <= compiled).then_some((version, key))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, key)| key.clone())
}

/// Whether `section.key = value` is a setting of this binary, by the same
/// deserialization every other layer goes through.
fn validate_key(section: &SectionMetadata, key: &str, value: &toml::Value) -> Result<(), String> {
    if !section.settings.iter().any(|setting| setting.name == key) {
        return Err("unknown setting".to_owned());
    }
    let mut stanza = toml::Table::new();
    stanza.insert(key.to_owned(), value.clone());
    let mut layer = toml::Table::new();
    layer.insert(section.section_name.to_owned(), toml::Value::Table(stanza));
    toml::Value::Table(layer)
        .try_into::<BuckSettingsData>()
        .map(|_| ())
        .map_err(|error| {
            // The first line names the problem; the rest repeats the key.
            error
                .to_string()
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTION: &str = "test_section";

    const fn metadata(section_version: u32) -> SectionMetadata {
        SectionMetadata {
            section_name: SECTION,
            section_version,
            scope: SectionScope::Daemon,
            doc: "",
            settings: &[],
        }
    }

    fn table(content: &str) -> toml::Table {
        toml::from_str(content).expect("Test input should be valid TOML")
    }

    /// Selects against a one-section registry at `compiled`, accepting every key.
    fn select_at(content: &str, compiled: u32) -> SelectedRollout {
        select_with_registry(table(content), &[metadata(compiled)], |_, _, _| Ok(()))
    }

    fn selected(content: &str) -> SelectedRollout {
        SelectedRollout {
            layer: table(content),
            ignored: Vec::new(),
        }
    }

    #[test]
    fn selects_compiled_version_over_newer() {
        assert_eq!(
            select_at(
                r#"
                    [test_section.0]
                    test_flag = true
                    test_value = "compiled"

                    [test_section.1]
                    test_flag = false
                    test_value = "future"
                "#,
                0,
            ),
            selected(
                r#"
                    [test_section]
                    test_flag = true
                    test_value = "compiled"
                "#,
            ),
        );
    }

    #[test]
    fn selects_newest_version_at_or_below_compiled() {
        assert_eq!(
            select_at(
                r#"
                    [test_section.0]
                    test_value = "oldest"

                    [test_section.1]
                    test_value = "newest applicable"

                    [test_section.3]
                    test_value = "future"
                "#,
                2,
            ),
            selected("[test_section]\ntest_value = \"newest applicable\""),
        );
    }

    #[test]
    fn skips_section_with_only_newer_versions() {
        assert_eq!(
            select_at("[test_section.1]\ntest_flag = true", 0),
            SelectedRollout::default()
        );
    }

    #[test]
    fn ignores_keys_that_are_not_versions() {
        assert_eq!(
            select_at(
                r#"
                    [test_section.latest]
                    test_flag = false

                    [test_section.1]
                    test_flag = true
                "#,
                2,
            ),
            selected("[test_section]\ntest_flag = true"),
        );
    }

    #[test]
    fn ignores_unknown_section() {
        assert_eq!(
            select_at(
                r#"
                    unknown_section = false

                    [test_section.0]
                    test_flag = true
                "#,
                0,
            ),
            selected("[test_section]\ntest_flag = true"),
        );
    }

    #[test]
    fn ignores_malformed_sections_individually() {
        assert_eq!(
            select_at("test_section = false", 0),
            SelectedRollout {
                layer: toml::Table::new(),
                ignored: vec![IgnoredSetting::section(SECTION, "not a table of versions")],
            }
        );
        assert_eq!(
            select_at("[test_section]\n0 = false", 0),
            SelectedRollout {
                layer: toml::Table::new(),
                ignored: vec![IgnoredSetting::section(
                    SECTION,
                    "version `0` is not a table of settings"
                )],
            }
        );
    }

    #[test]
    fn dice_sections_take_no_rollouts() {
        let dice = SectionMetadata {
            scope: SectionScope::Dice,
            ..metadata(0)
        };
        assert_eq!(
            select_with_registry(
                table("[test_section.0]\ntest_flag = true"),
                &[dice],
                |_, _, _| Ok(()),
            ),
            SelectedRollout {
                layer: toml::Table::new(),
                ignored: vec![IgnoredSetting::section(
                    SECTION,
                    "DICE-tracked sections take no rollouts"
                )],
            }
        );
    }

    #[test]
    fn applies_previous_hydration_version() {
        assert_eq!(
            select_rollout_table(table(
                r#"
                    [hydration.3]
                    enable_paging = true
                    page_out_on_idle = true
                "#,
            )),
            selected(
                r#"
                    [hydration]
                    enable_paging = true
                    page_out_on_idle = true
                "#,
            ),
        );
    }

    #[test]
    fn ignores_unknown_and_mistyped_keys_individually() {
        let result = select_rollout_table(table(
            r#"
                [log_download.0]
                log_url = "https://example.com"
                log_use_manifold = 1
                not_a_setting = true
            "#,
        ));
        assert_eq!(
            result.layer,
            table("[log_download]\nlog_url = \"https://example.com\"")
        );
        assert_eq!(
            result
                .ignored
                .iter()
                .map(|ignored| ignored.setting.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("log_download.log_use_manifold"),
                Some("log_download.not_a_setting")
            ]
        );
        assert_eq!(result.ignored[1].reason, "unknown setting");
        assert!(
            result.ignored[0].reason.contains("expected a boolean"),
            "{}",
            result.ignored[0].reason
        );
    }
}
