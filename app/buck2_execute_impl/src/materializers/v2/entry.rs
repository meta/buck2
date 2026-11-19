/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What the disk tree knows about a path's content: an `ActionDirectoryEntry` in shape, with
//! plain digests. The materializer holds no `TrackedFileDigest`: a handle keeps the digest's
//! interner entry alive, and with it whatever the stash holds for the digest, where the stash is
//! meant to hold what live values hold and nothing for content that is merely on disk. So a root
//! is tracked as a copy of its metadata, not as a share of the value it came from, and sqlite is
//! written from that copy.

use std::sync::Arc;

use allocative::Allocative;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::external_symlink::ExternalSymlink;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::Symlink;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::directory::INTERNER;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;

use crate::materializers::deferred::artifact_tree::ArtifactMetadata;

/// A tracked root's content, or what a tracked directory holds at a path inside it.
#[derive(Clone, Dupe, Debug, Allocative)]
pub(crate) enum DiskEntry {
    File(DiskFile),
    Symlink(Arc<Symlink>),
    ExternalSymlink(Arc<ExternalSymlink>),
    Dir(Arc<DiskDirectory>),
}

#[derive(Clone, Dupe, Debug, PartialEq, Eq, Allocative)]
pub(crate) struct DiskFile {
    pub(crate) digest: FileDigest,
    pub(crate) executable: bool,
}

/// A directory's contents and the fingerprint that identifies them. Shared, so that the roots an
/// expansion makes of a directory and the lookups into one cost no copy.
#[derive(Debug, Allocative)]
pub(crate) struct DiskDirectory {
    /// The RE digest of the directory, which is what two directories are compared by.
    fingerprint: FileDigest,
    /// In name order.
    entries: MiniBoxSlice<(FileNameBuf, DiskEntry)>,
    /// Bytes of the files below.
    size: u64,
}

impl DiskEntry {
    /// `entry` as the tree keeps it.
    pub(crate) fn from_action(entry: &ArtifactMetadata) -> Self {
        match entry {
            DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) => Self::File(DiskFile {
                digest: file.digest.data().dupe(),
                executable: file.is_executable,
            }),
            DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(symlink)) => {
                Self::Symlink(symlink.dupe())
            }
            DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(symlink)) => {
                Self::ExternalSymlink(symlink.dupe())
            }
            DirectoryEntry::Dir(dir) => Self::Dir(Arc::new(DiskDirectory::from_action(dir))),
        }
    }

    /// The entry as a value's entry, its digests tracked, for IO that wants one: an upload, say.
    /// A directory is rebuilt exhaustive, which tracked content is.
    pub(crate) fn to_action(
        &self,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<ArtifactMetadata> {
        let config = digest_config.cas_digest_config();
        let dir = match self {
            Self::Dir(dir) => dir,
            leaf => {
                return Ok(DirectoryEntry::Leaf(
                    leaf.to_member(config).expect("a leaf"),
                ));
            }
        };
        let mut leaves = Vec::new();
        let mut empty_dirs = Vec::new();
        dir.walk(&mut |path, entry| match entry {
            Self::Dir(dir) if dir.entries.is_empty() => empty_dirs.push(path.to_buf()),
            Self::Dir(_) => {}
            leaf => leaves.push((path.to_buf(), leaf.to_member(config).expect("a leaf"))),
        });
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        for path in empty_dirs {
            builder.mkdir(&path)?;
        }
        for (path, member) in leaves {
            builder.insert(&path, DirectoryEntry::Leaf(member))?;
        }
        builder.mark_uniformly_exhaustive();
        Ok(DirectoryEntry::Dir(
            builder
                .fingerprint(digest_config.as_directory_serializer())
                .shared(&*INTERNER),
        ))
    }

    /// A leaf as a value's member; `None` for a directory.
    fn to_member(&self, config: CasDigestConfig) -> Option<ActionDirectoryMember> {
        Some(match self {
            Self::File(file) => ActionDirectoryMember::File(FileMetadata {
                digest: TrackedFileDigest::new(file.digest.dupe(), config),
                is_executable: file.executable,
            }),
            Self::Symlink(symlink) => ActionDirectoryMember::Symlink(symlink.dupe()),
            Self::ExternalSymlink(symlink) => {
                ActionDirectoryMember::ExternalSymlink(symlink.dupe())
            }
            Self::Dir(_) => return None,
        })
    }

    /// Whether this is the content `entry` describes: directories by fingerprint, files by digest
    /// and mode, symlinks by target.
    pub(crate) fn matches(&self, entry: &ArtifactMetadata) -> bool {
        match (self, entry) {
            (Self::Dir(dir), DirectoryEntry::Dir(wanted)) => {
                dir.fingerprint == *wanted.fingerprint().data()
            }
            (Self::File(file), DirectoryEntry::Leaf(ActionDirectoryMember::File(wanted))) => {
                file.digest == *wanted.digest.data()
                    && executable_agrees(file.executable, wanted.is_executable)
            }
            (
                Self::Symlink(symlink),
                DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(wanted)),
            ) => symlink == wanted,
            (
                Self::ExternalSymlink(symlink),
                DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(wanted)),
            ) => symlink == wanted,
            _ => false,
        }
    }

    /// Whether two tracked entries describe the same content, as [`matches`](Self::matches)
    /// decides it.
    pub(crate) fn same_content(&self, other: &DiskEntry) -> bool {
        match (self, other) {
            (Self::Dir(a), Self::Dir(b)) => a.fingerprint == b.fingerprint,
            (Self::File(a), Self::File(b)) => {
                a.digest == b.digest && executable_agrees(a.executable, b.executable)
            }
            (Self::Symlink(a), Self::Symlink(b)) => a == b,
            (Self::ExternalSymlink(a), Self::ExternalSymlink(b)) => a == b,
            _ => false,
        }
    }

    /// What the entry holds at `path`: itself for the empty path, nothing for a path through a
    /// leaf.
    pub(crate) fn find(&self, path: &ForwardRelativePath) -> Option<DiskEntry> {
        let mut current = self.dupe();
        for name in path.iter() {
            let next = match &current {
                Self::Dir(dir) => dir.get(name)?.dupe(),
                _ => return None,
            };
            current = next;
        }
        Some(current)
    }

    /// Bytes of the files in the entry.
    pub(crate) fn size(&self) -> u64 {
        match self {
            Self::File(file) => file.digest.size(),
            Self::Dir(dir) => dir.size,
            Self::Symlink(_) | Self::ExternalSymlink(_) => 0,
        }
    }

    /// Every file in the entry, with its path relative to the entry.
    pub(crate) fn files(&self, f: &mut impl FnMut(&ForwardRelativePath, &DiskFile)) {
        match self {
            Self::File(file) => f(ForwardRelativePath::empty(), file),
            Self::Dir(dir) => dir.walk(&mut |path, entry| {
                if let Self::File(file) = entry {
                    f(path, file);
                }
            }),
            Self::Symlink(_) | Self::ExternalSymlink(_) => {}
        }
    }

    /// The files' digests as handles, for IO that wants them.
    pub(crate) fn tracked_digests(&self, config: CasDigestConfig) -> Vec<TrackedFileDigest> {
        let mut digests = Vec::new();
        self.files(&mut |_, file| {
            digests.push(TrackedFileDigest::new(file.digest.dupe(), config));
        });
        digests
    }
}

impl DiskDirectory {
    fn from_action(dir: &ActionSharedDirectory) -> Self {
        let mut size = 0;
        let entries: MiniBoxSlice<(FileNameBuf, DiskEntry)> = dir
            .entries()
            .into_iter()
            .map(|(name, entry)| {
                let entry = DiskEntry::from_action(entry);
                size += entry.size();
                (name.clone(), entry)
            })
            .collect();
        debug_assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));
        Self {
            fingerprint: dir.fingerprint().data().dupe(),
            entries,
            size,
        }
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&FileName, &DiskEntry)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.as_ref(), entry))
    }

    pub(crate) fn get(&self, name: &FileName) -> Option<&DiskEntry> {
        self.entries
            .binary_search_by(|(candidate, _)| AsRef::<FileName>::as_ref(candidate).cmp(name))
            .ok()
            .map(|i| &self.entries[i].1)
    }

    /// Every entry below this directory, each with its path relative to it, a directory before
    /// its contents.
    pub(crate) fn walk<'a>(&'a self, f: &mut impl FnMut(&ForwardRelativePath, &'a DiskEntry)) {
        let mut at = ForwardRelativePathBuf::default();
        self.walk_at(&mut at, f);
    }

    fn walk_at<'a>(
        &'a self,
        at: &mut ForwardRelativePathBuf,
        f: &mut impl FnMut(&ForwardRelativePath, &'a DiskEntry),
    ) {
        for (name, entry) in &self.entries {
            at.push(name);
            f(at, entry);
            if let DiskEntry::Dir(dir) = entry {
                dir.walk_at(at, f);
            }
            at.pop();
        }
    }
}

/// On Windows the executable bit is not kept on disk, so a difference in it is no difference in
/// content, as the deferred core treats it.
fn executable_agrees(a: bool, b: bool) -> bool {
    a == b || cfg!(windows)
}
