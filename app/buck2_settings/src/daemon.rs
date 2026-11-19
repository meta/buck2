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

use allocative::Allocative;
use buck2_settings_derive::buck_settings_section;
use serde::Deserialize;
use serde::Serialize;

/// The macOS QoS class the daemon lowers itself to when it starts, or
/// `skip_lowering` to stay at the system default. `utility` and `background`
/// are the classes `posix_spawnattr_set_qos_class_np` accepts.
#[derive(Allocative, Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MacosQosClass {
    Utility,
    Background,
    SkipLowering,
}

impl FromStr for MacosQosClass {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "utility" => Ok(Self::Utility),
            "background" => Ok(Self::Background),
            "skip_lowering" => Ok(Self::SkipLowering),
            other => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid macOS QoS class `{other}`, expected one of [`utility`, `background`, `skip_lowering`]"
            )),
        }
    }
}

/// The daemon process: when it exits, how many threads it runs on, and how
/// the OS schedules it.
#[buck_settings_section(name = "daemon", version = 0, scope = daemon)]
pub struct Daemon {
    /// Any value; changing it starts a new daemon. Set in the repository, it
    /// restarts every daemon after a change that invalidates their state.
    // FIXME(jakobdegen): drop the fallback once fbsource sets this in `.bucksettings.toml`.
    #[setting(
        buckconfig_fallback = "buck2.daemon_buster",
        overridable_in(CommandLine, LocalSettings)
    )]
    buster: Option<String>,
    /// Seconds without a command after which the daemon exits; the daemon's
    /// own default when unset.
    #[setting(
        buckconfig_fallback = "buck2.daemon_idle_timeout_s",
        overridable_in(CommandLine, LocalSettings)
    )]
    idle_timeout_s: Option<u64>,
    /// Worker threads of the daemon's tokio runtime; `0` or unset lets tokio
    /// choose.
    // FIXME(jakobdegen): drop the fallback once Sandcastle passes this as a setting.
    #[setting(
        buckconfig_fallback = "build.num_tokio_workers",
        overridable_in(CommandLine, LocalSettings)
    )]
    num_tokio_workers: Option<usize>,
    /// `utility` when unset.
    // FIXME(jakobdegen): drop the fallback once fbsource sets this in `.bucksettings.toml`.
    #[setting(
        buckconfig_fallback = "buck2.macos_qos_class",
        overridable_in(CommandLine, LocalSettings)
    )]
    macos_qos_class: Option<MacosQosClass>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buckconfig fallback parses the same names the setting does.
    #[test]
    fn qos_class_names_match_between_toml_and_buckconfig() -> buck2_error::Result<()> {
        for (name, class) in [
            ("utility", MacosQosClass::Utility),
            ("background", MacosQosClass::Background),
            ("skip_lowering", MacosQosClass::SkipLowering),
        ] {
            assert_eq!(name.parse::<MacosQosClass>()?, class);
            assert_eq!(
                serde_json::to_value(class)?,
                serde_json::Value::String(name.to_owned())
            );
        }
        assert!("realtime".parse::<MacosQosClass>().is_err());
        Ok(())
    }
}
