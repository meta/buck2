/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;
use std::time::SystemTime;

use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;

use crate::invocation_roots::home_buck_dir;

/// `~/.buck/tmp` after old files removed.
///
/// We use this directory when we need tmp dir with short file names (to connect to unix socket).
pub fn home_buck_tmp_dir() -> buck2_error::Result<&'static AbsNormPath> {
    fn remove_old_files(tmp_dir: &AbsNormPath) -> buck2_error::Result<()> {
        let mut now = None;

        for entry in fs_util::read_dir(tmp_dir).categorize_internal()? {
            let entry = entry?;
            let timestamp = match entry.metadata().and_then(|m| m.modified()) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Possible if invoked concurrently.
                    continue;
                }
                Err(e) => return Err(e.into()),
                Ok(metadata) => metadata,
            };

            let now = *now.get_or_insert_with(SystemTime::now);
            if now.duration_since(timestamp).unwrap_or_default().as_secs() > 3 * 86400 {
                fs_util::remove_all(entry.path()).categorize_internal()?;
            }
        }

        Ok(())
    }

    fn find_dir() -> buck2_error::Result<AbsNormPathBuf> {
        let home_buck_dir = home_buck_dir()?;
        let tmp_dir = home_buck_dir.join(FileName::new("tmp")?);
        fs_util::create_dir_all(&tmp_dir)?;
        remove_old_files(&tmp_dir)?;
        Ok(tmp_dir)
    }

    static DIR: LazyLock<buck2_error::Result<AbsNormPathBuf>> = LazyLock::new(find_dir);

    Ok(LazyLock::force(&DIR).as_ref().map_err(dupe::Dupe::dupe)?)
}
