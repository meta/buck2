/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_settings_derive::buck_settings_section;

/// Where the daemon's event logs go.
#[buck_settings_section(name = "log_download", version = 0, scope = daemon)]
pub struct LogDownload {
    /// Whether event logs are kept in Manifold.
    #[setting(
        buckconfig_fallback = "buck2.log_use_manifold",
        internal_default = true,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    log_use_manifold: Option<bool>,
    /// Where event logs are uploaded when not kept in Manifold.
    #[setting(
        buckconfig_fallback = "buck2.log_url",
        overridable_in(CommandLine, LocalSettings)
    )]
    log_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::BuckSettings;
    use crate::buckconfig::BuckconfigSource;
    use crate::buckconfig::NoBuckconfig;
    use crate::metadata::SettingKeyRef;
    use crate::metadata::find_setting_metadata;
    use crate::parser::resolve_setting_flags;
    use crate::parser::table;
    use crate::settings::ALL_SECTION_METADATA;

    struct Buckconfig(Vec<(&'static str, &'static str)>);

    impl BuckconfigSource for Buckconfig {
        fn get(&mut self, section: &str, property: &str) -> buck2_error::Result<Option<String>> {
            let key = format!("{section}.{property}");
            Ok(self
                .0
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned()))
        }
    }

    #[test]
    fn setting_then_buckconfig_then_default() -> buck2_error::Result<()> {
        let unset = BuckSettings::empty().daemon.log_download;
        assert_eq!(
            unset.log_use_manifold(&mut NoBuckconfig)?,
            cfg!(fbcode_build)
        );
        assert_eq!(unset.log_url(&mut NoBuckconfig)?, None);

        let mut buckconfig = Buckconfig(vec![
            ("buck2.log_use_manifold", "false"),
            ("buck2.log_url", "https://buckconfig/"),
        ]);
        assert!(!unset.log_use_manifold(&mut buckconfig)?);
        assert_eq!(
            unset.log_url(&mut buckconfig)?.as_deref(),
            Some("https://buckconfig/")
        );

        let set = resolve_setting_flags(vec![table(
            "[log_download]\nlog_use_manifold = true\nlog_url = \"https://setting/\"",
        )])?
        .daemon
        .log_download;
        assert!(set.log_use_manifold(&mut buckconfig)?);
        assert_eq!(
            set.log_url(&mut buckconfig)?.as_deref(),
            Some("https://setting/")
        );

        let mut invalid = Buckconfig(vec![("buck2.log_use_manifold", "yes")]);
        assert!(unset.log_use_manifold(&mut invalid).is_err());
        Ok(())
    }

    #[test]
    fn test_find_setting_metadata() {
        let find = |section, name| {
            find_setting_metadata(ALL_SECTION_METADATA, SettingKeyRef { section, name })
                .map(|metadata| metadata.name)
        };
        assert_eq!(
            find("log_download", "log_use_manifold"),
            Some("log_use_manifold")
        );
        assert_eq!(find("log_download", "log_use_maniflod"), None);
        assert_eq!(find("buck2", "log_url"), None);
    }
}
