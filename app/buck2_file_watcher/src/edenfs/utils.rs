/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::str;

use buck2_data::FileWatcherKind;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use edenfs::Dtype;

pub(crate) fn dtype_into_file_watcher_kind(dtype: Dtype) -> FileWatcherKind {
    match dtype {
        Dtype::DIR => FileWatcherKind::Directory,
        Dtype::LINK => FileWatcherKind::Symlink,
        _ => FileWatcherKind::File,
    }
}

pub(crate) fn bytes_to_string_or_unknown(bytes: &[u8]) -> &str {
    str::from_utf8(bytes).ok().unwrap_or("unknown")
}

pub(crate) fn find_first_valid_parent(mut path: &Path) -> Option<&ForwardRelativePath> {
    loop {
        path = path.parent()?;

        if let Ok(path) = ForwardRelativePath::new(path) {
            return Some(path);
        }
    }
}
