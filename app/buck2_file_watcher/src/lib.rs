/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project_rel_path::ProjectRelativePath;

pub mod dep_files;
#[cfg(fbcode_build)]
mod edenfs;
pub mod file_watcher;
mod fs_hash_crawler;
pub mod ignores;
pub mod mergebase;
mod notify;
mod stats;
mod watchman;

/// Returns true if the given path is a Watchman cookie file.
///
/// Watchman creates and deletes `.watchman-cookie-*` files as synchronization
/// markers to establish ordering barriers with the underlying filesystem
/// notification backend. These are not user source changes and should never
/// trigger DICE invalidation or rebuilds.
pub(crate) fn is_watchman_cookie(path: &ProjectRelativePath) -> bool {
    path.file_name()
        .is_some_and(|f| f.as_str().starts_with(".watchman-cookie-"))
}
