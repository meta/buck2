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
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;

/// A client-computable namespace for daemons with equal startup constraints.
#[derive(Allocative, Clone, Debug, PartialEq, Eq, Hash, derive_more::Display)]
#[display("{}", _0)]
pub struct DaemonCompatibilityKey(FileNameBuf);

impl DaemonCompatibilityKey {
    /// `daemon_startup_config` is the serialized `DaemonStartupConfig` a daemon is started with
    /// and reports in its constraints, so equal keys imply the startup constraints a client
    /// checks against a daemon are equal.
    pub fn new(
        version: &str,
        user_version: Option<&str>,
        daemon_startup_config: &str,
        trace_io_enabled: bool,
    ) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("buck2.unidaemon.compatibility.v1");
        for field in [
            version,
            user_version.unwrap_or_default(),
            daemon_startup_config,
        ] {
            hasher.update(&(field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.update(&[user_version.is_some() as u8, trace_io_enabled as u8]);
        Self(FileNameBuf::unchecked_new(
            hasher.finalize().to_hex().as_str(),
        ))
    }

    pub fn file_name(&self) -> &FileName {
        &self.0
    }
}

/// Loads a key from its digest, as a namespace directory name or a stored record carries it.
/// Nothing is derived here: any well-formed digest is accepted, including one derived by a
/// binary whose constraints this one cannot reproduce.
impl FromStr for DaemonCompatibilityKey {
    type Err = buck2_error::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Invalid daemon compatibility key: expected 64 lowercase hexadecimal characters"
            ));
        }
        Ok(Self(FileNameBuf::try_from(value.to_owned())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::DaemonStartupConfig;

    #[test]
    fn startup_constraints_select_distinct_namespaces() -> buck2_error::Result<()> {
        let config = DaemonStartupConfig::testing_empty().serialize()?;
        let key = DaemonCompatibilityKey::new("release-a", None, &config, false);
        assert_eq!(key, key.to_string().parse()?);
        assert_eq!(
            key,
            DaemonCompatibilityKey::new("release-a", None, &config, false)
        );
        assert_ne!(
            key,
            DaemonCompatibilityKey::new("release-b", None, &config, false)
        );
        assert_ne!(
            key,
            DaemonCompatibilityKey::new("release-a", Some("another-job"), &config, false)
        );
        assert_ne!(
            key,
            DaemonCompatibilityKey::new("release-a", Some(""), &config, false),
            "an empty user version is not the absence of one"
        );
        assert_ne!(
            key,
            DaemonCompatibilityKey::new("release-a", None, &config, true)
        );
        let mut shared = DaemonStartupConfig::testing_empty();
        shared.startup_options.share_dice = true;
        assert_ne!(
            key,
            DaemonCompatibilityKey::new("release-a", None, &shared.serialize()?, false)
        );
        Ok(())
    }

    #[test]
    fn compatibility_key_rejects_noncanonical_paths() {
        for value in ["../daemon", "abc", &"A".repeat(64), &"g".repeat(64)] {
            assert!(value.parse::<DaemonCompatibilityKey>().is_err());
        }
    }
}
