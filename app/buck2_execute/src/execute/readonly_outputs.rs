/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The two file operations behind `buck2.readonly_outputs`: the seal a materializer puts on the
//! files it tracks, and the unseal an incremental action's previous outputs get for its run.

use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

use crate::directory::ActionDirectoryEntry;
use crate::directory::ActionDirectoryMember;
use crate::directory::ActionSharedDirectory;

/// Takes the write bits off every file of `entry` under `root`, as a materializer does to the
/// files it tracks under `buck2.readonly_outputs`. Directories and symlinks are left as they are.
pub fn seal_files(
    fs: &ProjectRoot,
    root: &ProjectRelativePath,
    entry: &ActionDirectoryEntry<ActionSharedDirectory>,
) -> buck2_error::Result<()> {
    let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
    while let Some((path, entry)) = walk.next() {
        if let DirectoryEntry::Leaf(ActionDirectoryMember::File(_)) = entry {
            let file = fs.resolve(root.join(path.get()));
            fs_util::set_writable(&file, false)
                .categorize_internal()
                .with_buck_error_context(|| format!("Making `{file}` read-only"))?;
        }
    }
    Ok(())
}

/// Makes every file under `paths` one an action may write in place, for its previous outputs: a
/// file the BlobStore holds by hardlink is cloned or copied over itself, so that the write stays
/// out of the store, and every file gets its owner's write permission, which a materializer takes
/// from the files it tracks. The store's name of a shared file keeps its mode, except on Windows,
/// where hardlinks share the read-only attribute and replacing the file clears it. Symlinks are
/// not followed.
pub fn make_output_paths_writable(
    fs: &ProjectRoot,
    paths: &[ProjectRelativePathBuf],
) -> buck2_error::Result<()> {
    for path in paths {
        make_writable(&fs.resolve(path))?;
    }
    Ok(())
}

fn make_writable(path: &AbsNormPath) -> buck2_error::Result<()> {
    let Some(metadata) = fs_util::symlink_metadata_if_exists(path)? else {
        return Ok(());
    };
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        for entry in fs_util::read_dir(path).categorize_internal()? {
            let entry = entry.map_err(|e| {
                buck2_fs::error::IoError::new_with_path("read_dir", path, e).categorize_internal()
            })?;
            let name = entry.file_name();
            make_writable(&path.join(ForwardRelativePath::new(&name)?))?;
        }
    } else if file_type.is_file() {
        if shared(&metadata) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .internal_error("a file has a name")?;
            let parent = path.parent().internal_error("a file has a parent")?;
            let unshared = parent.join(ForwardRelativePathBuf::unchecked_new(format!(
                "{name}.unshare"
            )));
            // What a run interrupted between the clone and the rename left.
            fs_util::remove_all(&unshared).categorize_internal()?;
            fs_util::reflink_or_copy(path, &unshared).categorize_internal()?;
            fs_util::set_permissions(&unshared, metadata.permissions()).categorize_internal()?;
            fs_util::set_writable(&unshared, true).categorize_internal()?;
            fs_util::rename(&unshared, path).categorize_internal()?;
        } else {
            fs_util::set_writable(path, true).categorize_internal()?;
        }
    }
    Ok(())
}

/// Whether the file has another name, so that a write to it would be seen elsewhere.
#[cfg(unix)]
fn shared(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

/// Windows reports the link count only through an unstable API, so every file counts as shared.
#[cfg(not(unix))]
fn shared(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_fs::fs_util::uncategorized as fs_util;

    use super::*;
    use crate::digest_config::DigestConfig;
    use crate::directory::ActionDirectoryBuilder;
    use crate::directory::INTERNER;

    fn rel(s: &str) -> &ProjectRelativePath {
        ProjectRelativePath::unchecked_new(s)
    }

    fn file(content: &str, is_executable: bool) -> ActionDirectoryMember {
        ActionDirectoryMember::File(FileMetadata {
            digest: TrackedFileDigest::from_content(
                content.as_bytes(),
                DigestConfig::testing_default().cas_digest_config(),
            ),
            is_executable,
        })
    }

    #[test]
    fn test_seal_files_takes_the_write_bits_off_files_only() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        let dir = rel("buck-out/v2/gen/dir");
        fs.write_file(dir.join(ForwardRelativePath::new("a")?), "a", false)?;
        fs.write_file(dir.join(ForwardRelativePath::new("sub/b")?), "b", true)?;
        let mut builder = ActionDirectoryBuilder::empty_exhaustive();
        builder
            .insert(
                ForwardRelativePath::new("a")?,
                DirectoryEntry::Leaf(file("a", false)),
            )
            .unwrap();
        builder
            .insert(
                ForwardRelativePath::new("sub/b")?,
                DirectoryEntry::Leaf(file("b", true)),
            )
            .unwrap();
        builder.mark_uniformly_exhaustive();
        let entry = DirectoryEntry::Dir(
            builder
                .fingerprint(DigestConfig::testing_default().as_directory_serializer())
                .shared(&*INTERNER),
        );

        seal_files(fs, dir, &entry)?;

        let perms = |p: &str| fs_util::metadata(fs.resolve(rel(p))).unwrap().permissions();
        assert!(perms("buck-out/v2/gen/dir/a").readonly());
        assert!(perms("buck-out/v2/gen/dir/sub/b").readonly());
        assert!(
            !perms("buck-out/v2/gen/dir/sub").readonly(),
            "directories stay writable"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(perms("buck-out/v2/gen/dir/sub/b").mode() & 0o777, 0o555);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_make_writable_gives_linked_files_their_own_inode_and_the_write_bit()
    -> buck2_error::Result<()> {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;

        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        let out = fs.resolve(rel("buck-out/v2/gen/out"));
        let stash = fs.resolve(rel("buck-out/v2/stash"));
        fs_util::create_dir_all(&out)?;
        fs_util::create_dir_all(&stash)?;
        let a = out.join(ForwardRelativePath::unchecked_new("a"));
        // The store's name for `a`'s content, outside the paths an action writes.
        let b = stash.join(ForwardRelativePath::unchecked_new("b"));
        let alone = out.join(ForwardRelativePath::unchecked_new("alone"));
        fs_util::write(&a, b"shared")?;
        fs_util::set_executable(&a, true)?;
        fs_util::hard_link(&a, &b)?;
        fs_util::write(&alone, b"alone")?;
        let alone_inode = fs_util::metadata(&alone)?.ino();
        // As the disk-state core leaves tracked files.
        for file in [&a, &alone] {
            fs_util::set_writable(file, false)?;
        }

        make_output_paths_writable(fs, &[rel("buck-out/v2/gen/out").to_owned()])?;

        let (a_meta, b_meta) = (fs_util::metadata(&a)?, fs_util::metadata(&b)?);
        assert_ne!(a_meta.ino(), b_meta.ino());
        assert_eq!(a_meta.nlink(), 1);
        assert_eq!(fs_util::read_to_string(&a)?, "shared");
        assert_eq!(fs_util::read_to_string(&b)?, "shared");
        assert_ne!(a_meta.permissions().mode() & 0o111, 0, "the mode survives");
        assert!(
            !a_meta.permissions().readonly(),
            "the copy is the run's to write"
        );
        let alone_meta = fs_util::metadata(&alone)?;
        assert_eq!(alone_meta.ino(), alone_inode);
        assert!(!alone_meta.permissions().readonly());
        assert!(
            b_meta.permissions().readonly(),
            "the store's name of the shared inode keeps its mode"
        );
        Ok(())
    }
}
