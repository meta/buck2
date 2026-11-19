/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_common::ignores::ignore_set::IgnoreSet;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;

/// Type that file watchers use to determine if a file is ignored
///
/// Although we also check for ignores within dice after reading a file/directory/etc., we need to
/// check this here also specifically so that adding an ignored file doesn't invalidate the listing
/// for that file's parent directory.
#[derive(Allocative, Clone)]
pub struct FileWatcherIgnores {
    ignores: Arc<IgnoreSet>,
}

impl FileWatcherIgnores {
    pub fn new(ignores: Arc<IgnoreSet>) -> Self {
        Self { ignores }
    }

    pub(crate) fn is_ignored(&self, path: IProjectRelativePath) -> bool {
        self.ignores.is_match(&path)
    }
}
