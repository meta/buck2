/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs::File;
use std::sync::OnceLock;

use buck2_core::buck2_env;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use object::Object;

/// Provides information about this buck version.
pub struct BuckVersion {
    version: String,
    internal_exe_hash: String,
}

impl BuckVersion {
    pub fn get() -> buck2_error::Result<&'static BuckVersion> {
        static VERSION: OnceLock<buck2_error::Result<BuckVersion>> = OnceLock::new();
        VERSION
            .get_or_init(Self::compute)
            .as_ref()
            .map_err(|err: &buck2_error::Error| err.clone().tag([ErrorTag::BuckVersionError]))
    }

    pub fn get_unique_id() -> buck2_error::Result<&'static str> {
        Ok(Self::get()?.unique_id())
    }

    pub fn get_version() -> buck2_error::Result<&'static str> {
        Ok(Self::get()?.version())
    }

    pub fn get_version_for_clap() -> &'static str {
        Self::get_version().unwrap_or("<version unavailable>")
    }

    fn extract_unique_id(file: &object::File) -> Option<String> {
        if let Ok(Some(build_id)) = file.build_id() {
            Some(hex::encode(build_id))
        } else if let Ok(Some(uuid)) = file.mach_uuid() {
            Some(hex::encode(uuid))
        } else if cfg!(windows) {
            buck2_build_info::win_internal_version().map(|s| s.to_owned())
        } else {
            None
        }
    }

    fn hash_binary(file: &mut File) -> buck2_error::Result<String> {
        let mut blake3 = blake3::Hasher::new();
        std::io::copy(file, &mut blake3).buck_error_context("Error hashing binary")?;
        let hash = blake3.finalize();
        Ok(hash.to_hex().to_string())
    }

    fn compute() -> buck2_error::Result<BuckVersion> {
        // Make sure to use the daemon exe's version, if there is one
        let exe = crate::daemon::client::connect::get_daemon_exe()
            .buck_error_context("Error finding daemon executable for versioning")?;

        let mut file = File::open(&exe).with_buck_error_context(|| {
            format!("Error opening daemon executable at {}", exe.display())
        })?;

        let file_m = unsafe { memmap2::Mmap::map(&file) }.with_buck_error_context(|| {
            format!(
                "Error to mmap daemon executable at {} for versioning",
                exe.display()
            )
        })?;

        let file_object = object::File::parse(&*file_m).map_err(|e| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Error parsing daemon executable at {} for versioning: {e:#}",
                exe.display()
            )
        })?;

        let (internal_exe_hash, internal_exe_hash_kind) = if let Some(internal_exe_hash) =
            Self::extract_unique_id(&file_object)
        {
            (internal_exe_hash, "<build-id>")
        } else {
            if !(buck2_core::is_open_source() || buck2_env!("BUCK2_IGNORE_VERSION_EXTRACTION_FAILURE", type=bool, default=false, applicability=testing).unwrap_or(false)) {
                let _ignored = crate::eprintln!(
                    "version extraction failed. This indicates an issue with the buck2 release, will fallback to binary hash"
                );
            }
            (Self::hash_binary(&mut file)?, "<exe-hash>")
        };

        let version = if let Some(version) = buck2_build_info::revision() {
            version.to_owned()
        } else {
            format!("{internal_exe_hash} {internal_exe_hash_kind}")
        };

        Ok(BuckVersion {
            version,
            internal_exe_hash,
        })
    }

    /// Provides a globally unique identifier for this buck executable.
    pub fn unique_id(&self) -> &str {
        &self.internal_exe_hash
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}
