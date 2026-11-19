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

use buck2_core::fs::project_rel_path::ProjectRelativePath;

pub(crate) fn find_first_valid_parent(mut path: &Path) -> Option<&ProjectRelativePath> {
    loop {
        path = path.parent()?;

        if let Ok(path) = ProjectRelativePath::new(path) {
            return Some(path);
        }
    }
}
