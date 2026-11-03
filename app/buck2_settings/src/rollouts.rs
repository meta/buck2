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

use buck2_error::buck2_error;

use crate::metadata::SectionMetadata;
use crate::settings::ALL_SECTION_METADATA;
use crate::settings::BuckSettingsData;

/// Selects, for every registered section, the newest stanza at or below the
/// compiled section version.
///
/// A stanza applies from its version on, until a newer stanza supersedes it.
/// A newer stanza never reaches an older binary, which is what lets a rollout
/// use keys the older binary does not have.
///
/// `Ok(None)` means no stanza applies to any registered section. `Err` means
/// a registered section or its selected stanza is malformed, or the selected
/// layer does not match the compiled settings schema.
pub(super) fn select_rollout_table(
    versioned_settings: toml::Table,
) -> buck2_error::Result<Option<toml::Table>> {
    let Some(layer) = select_rollout_layer_with_registry(versioned_settings, ALL_SECTION_METADATA)?
    else {
        return Ok(None);
    };
    validate_rollout_layer(&layer)?;
    Ok(Some(layer))
}

fn select_rollout_layer_with_registry(
    mut versioned_settings: toml::Table,
    sections: &[SectionMetadata],
) -> buck2_error::Result<Option<toml::Table>> {
    let mut layer = toml::Table::new();

    for section in sections {
        let Some(versions) = versioned_settings.remove(section.section_name) else {
            continue;
        };
        let toml::Value::Table(mut versions) = versions else {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Buck settings rollout section `{}` must contain a table of versions",
                section.section_name,
            ));
        };

        let Some(version) = newest_version_at_or_below(&versions, section.section_version) else {
            continue;
        };
        let Some(toml::Value::Table(settings)) = versions.remove(&version) else {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Buck settings rollout section `{}` version `{version}` must contain a table of settings",
                section.section_name,
            ));
        };

        layer.insert(
            section.section_name.to_owned(),
            toml::Value::Table(settings),
        );
    }

    Ok((!layer.is_empty()).then_some(layer))
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

fn validate_rollout_layer(layer: &toml::Table) -> buck2_error::Result<()> {
    let _validated: BuckSettingsData =
        toml::Value::Table(layer.clone())
            .try_into()
            .map_err(|error| {
                buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Validating settings rollout layer: {error}"
                )
            })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::SectionScope;

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

    fn select(content: &str) -> buck2_error::Result<Option<toml::Table>> {
        select_at(content, 0)
    }

    fn select_at(content: &str, compiled: u32) -> buck2_error::Result<Option<toml::Table>> {
        select_rollout_layer_with_registry(table(content), &[metadata(compiled)])
    }

    #[test]
    fn selects_compiled_version_over_newer() -> buck2_error::Result<()> {
        let selected = select(
            r#"
                [test_section.0]
                test_flag = true
                test_value = "compiled"

                [test_section.1]
                test_flag = false
                test_value = "future"
            "#,
        )?
        .expect("The compiled section version is present");

        assert_eq!(
            selected,
            table(
                r#"
                    [test_section]
                    test_flag = true
                    test_value = "compiled"
                "#,
            ),
        );
        Ok(())
    }

    #[test]
    fn selects_newest_version_at_or_below_compiled() -> buck2_error::Result<()> {
        let selected = select_at(
            r#"
                [test_section.0]
                test_value = "oldest"

                [test_section.1]
                test_value = "newest applicable"

                [test_section.3]
                test_value = "future"
            "#,
            2,
        )?
        .expect("An older section version is present");

        assert_eq!(
            selected,
            table(
                r#"
                    [test_section]
                    test_value = "newest applicable"
                "#,
            ),
        );
        Ok(())
    }

    #[test]
    fn skips_section_with_only_newer_versions() -> buck2_error::Result<()> {
        assert_eq!(select("[test_section.1]\ntest_flag = true")?, None);
        Ok(())
    }

    #[test]
    fn ignores_keys_that_are_not_versions() -> buck2_error::Result<()> {
        assert_eq!(
            select_at(
                r#"
                    [test_section.latest]
                    test_flag = false

                    [test_section.1]
                    test_flag = true
                "#,
                2,
            )?,
            Some(table("[test_section]\ntest_flag = true")),
        );
        Ok(())
    }

    #[test]
    fn ignores_unknown_section() -> buck2_error::Result<()> {
        assert_eq!(
            select(
                r#"
                    unknown_section = false

                    [test_section.0]
                    test_flag = true
                "#,
            )?,
            Some(table(
                r#"
                    [test_section]
                    test_flag = true
                "#,
            )),
        );
        Ok(())
    }

    #[test]
    fn rejects_non_table_section() {
        select("test_section = false")
            .expect_err("A registered section must contain version tables");
    }

    #[test]
    fn rejects_non_table_version() {
        select("[test_section]\n0 = false")
            .expect_err("The selected version must contain a settings table");
    }

    #[test]
    fn accepts_valid_settings() -> buck2_error::Result<()> {
        let selected = select_rollout_table(table(
            r#"
                [log_download.0]
                log_use_manifold = true
                log_url = "https://example.com"
            "#,
        ))?
        .expect("The production section version is present");

        assert_eq!(
            selected,
            table(
                r#"
                    [log_download]
                    log_use_manifold = true
                    log_url = "https://example.com"
                "#,
            ),
        );
        Ok(())
    }

    #[test]
    fn accepts_valid_hydration_settings() -> buck2_error::Result<()> {
        let selected = select_rollout_table(table(
            r#"
                [hydration.4]
                enable_paging = true
                defer_field_reads = true
                page_out_on_idle = true
                page_out_on_idle_isolation_dir_scope = "non_default"
                page_out_min_free_disk_gb = 0
                page_out_policy = "once_per_key"
                eviction_policy = "on_idle"
                pagable_storage_backend = "sqlite"
            "#,
        ))?
        .expect("The hydration section version is present");

        assert_eq!(
            selected,
            table(
                r#"
                    [hydration]
                    enable_paging = true
                    defer_field_reads = true
                    page_out_on_idle = true
                    page_out_on_idle_isolation_dir_scope = "non_default"
                    page_out_min_free_disk_gb = 0
                    page_out_policy = "once_per_key"
                    eviction_policy = "on_idle"
                    pagable_storage_backend = "sqlite"
                "#,
            ),
        );
        Ok(())
    }

    #[test]
    fn applies_previous_hydration_version() -> buck2_error::Result<()> {
        assert_eq!(
            select_rollout_table(table(
                r#"
                    [hydration.3]
                    enable_paging = true
                    page_out_on_idle = true
                "#,
            ))?,
            Some(table(
                r#"
                    [hydration]
                    enable_paging = true
                    page_out_on_idle = true
                "#,
            )),
        );
        Ok(())
    }

    #[test]
    fn rejects_unknown_setting_in_selected_version() {
        select_rollout_table(table(
            r#"
                [log_download.0]
                not_a_setting = true
            "#,
        ))
        .expect_err("An unknown setting must fail standalone validation");
    }

    #[test]
    fn rejects_wrong_value_type_in_selected_version() {
        select_rollout_table(table(
            r#"
                [log_download.0]
                log_use_manifold = 1
            "#,
        ))
        .expect_err("A setting with the wrong type must fail standalone validation");
    }
}
