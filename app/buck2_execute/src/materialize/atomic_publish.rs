/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Replacing a file atomically, so that a concurrent reader sees either the old
//! or the new content, never a missing or partially written file.
//!
//! A content-based output path has no configuration component, so a target
//! analyzed under several configurations has one writer per configuration for
//! it, all producing the same bytes, while consumers of a writer that finished
//! first already read the file. Such a path is only ever replaced by renaming a
//! complete temporary sibling over it, never removed or truncated in place.

use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_path::AbsPath;
use tempfile::NamedTempFile;

use crate::execute::clean_output_paths::tag_file_busy_error;

/// A freshly created temporary sibling of `dest`, for `publish` to rename over it once written.
///
/// `tempfile` owns temp naming (random, claimed with create-new semantics, so
/// nothing that already exists is ever touched) and deletes the temp on drop
/// when the publish does not happen. The temp creation and content write bypass
/// `fs_util` (no IO counters, retries, or error categorization for those two
/// operations).
pub fn temp_sibling(dest: &AbsNormPath) -> buck2_error::Result<NamedTempFile> {
    let (Some(dir), Some(file_name)) = (dest.parent(), dest.file_name()) else {
        return Err(internal_error!(
            "publish destination `{dest}` has no parent directory"
        ));
    };

    // The destination's name in the prefix is only for debuggability. Cap it
    // (on a char boundary) so the temp name stays under NAME_MAX even when
    // the destination name is itself near the limit.
    const TEMP_PREFIX_NAME_BYTES: usize = 64;
    let name = file_name.to_string_lossy();
    let mut cap = name.len().min(TEMP_PREFIX_NAME_BYTES);
    while !name.is_char_boundary(cap) {
        cap -= 1;
    }
    let prefix = format!(".{}.", &name[..cap]);
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix).suffix(".tmp");
    // The rename carries the temp's mode onto the destination, so give the
    // temp the mode a materialized output is expected to have: umask-defaulted
    // 0666 (`File::create` semantics), not tempfile's owner-only default. The
    // executable bit is applied by `publish`.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    builder
        .tempfile_in(dir)
        .with_buck_error_context(|| format!("creating temp file for `{dest}`"))
}

/// Renames the written `temp` from `temp_sibling` over `dest`, atomically replacing any plain
/// file already there.
///
/// The rename goes through `fs_util` so the operation with replace semantics
/// stays in buck's IO layer.
pub fn publish(
    temp: NamedTempFile,
    dest: &AbsNormPath,
    is_executable: bool,
) -> buck2_error::Result<()> {
    let temp_path = temp.into_temp_path();
    let temp_abs = AbsPath::new(&*temp_path)?;
    #[cfg(unix)]
    if is_executable {
        use std::os::unix::fs::PermissionsExt;
        fs_util::set_permissions(temp_abs, std::fs::Permissions::from_mode(0o755))
            .categorize_internal()?;
    }
    #[cfg(not(unix))]
    let _ = is_executable;

    // On Windows a destination another process holds open cannot be
    // replaced; report that as file-busy.
    fs_util::rename(temp_abs, dest)
        .categorize_internal()
        .map_err(tag_file_busy_error)?;
    // The rename freed the temp name for another writer to claim, so
    // `temp_path`'s delete-on-drop must be disarmed — it deletes by name and
    // could unlink someone else's fresh claim. (On failure above, that drop
    // is exactly what cleans our temp up.)
    // `-W let-underscore-drop` wants a named binding for the dropped Result.
    let _unused = temp_path.keep();
    Ok(())
}
