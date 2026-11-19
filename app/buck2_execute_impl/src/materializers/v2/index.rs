/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The reverse index: which tracked paths hold a file of a given digest, so that a
//! materialization of content that is already on disk somewhere copies it rather than downloads
//! it. The processor keeps it from the tree's changes; download tasks read it without the
//! processor, so that the common case, nothing on disk holds the digest, costs a lookup and no
//! round trip. A hit is a candidate until the processor holds the path for reading, where the
//! tree has the last word.

use allocative::Allocative;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_hash::BuckDashMap;
use dupe::Dupe;

use crate::materializers::v2::entry::DiskEntry;
use crate::materializers::v2::tree::TreeChange;

#[derive(Default, Allocative)]
pub(crate) struct DigestIndex {
    /// The paths holding each digest, most recently tracked last.
    files: BuckDashMap<FileDigest, Vec<ProjectRelativePathBuf>>,
}

impl DigestIndex {
    /// Follows a change to the tracked roots.
    pub(crate) fn apply(&self, change: &TreeChange) {
        for (path, tracked) in &change.removed {
            self.removed(path, &tracked.entry);
        }
        for (path, tracked) in &change.added {
            self.added(path, &tracked.entry);
        }
    }

    pub(crate) fn added(&self, root: &ProjectRelativePath, entry: &DiskEntry) {
        entry.files(&mut |relative, file| {
            self.files
                .entry(file.digest.dupe())
                .or_default()
                .push(root.join(relative));
        });
    }

    pub(crate) fn removed(&self, root: &ProjectRelativePath, entry: &DiskEntry) {
        entry.files(&mut |relative, file| {
            let path = root.join(relative);
            if let Some(mut paths) = self.files.get_mut(&file.digest) {
                if let Some(at) = paths.iter().position(|p| *p == path) {
                    paths.swap_remove(at);
                }
            }
            self.files
                .remove_if(&file.digest, |_, paths| paths.is_empty());
        });
    }

    /// The tracked paths holding a file of `digest`, as of now.
    pub(crate) fn holders(&self, digest: &FileDigest) -> Vec<ProjectRelativePathBuf> {
        self.files
            .get(digest)
            .map_or_else(Vec::new, |paths| paths.clone())
    }

    /// Whether any tracked path holds a file of `digest`, as of now.
    pub(crate) fn holds(&self, digest: &FileDigest) -> bool {
        self.files.contains_key(digest)
    }
}
