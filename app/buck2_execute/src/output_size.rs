/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;

use crate::artifact_value::ArtifactValue;
use crate::directory::ActionDirectory;
use crate::directory::ActionDirectoryMember;

pub struct OutputCountAndBytes {
    pub count: u64,
    pub bytes: u64,
}
pub trait OutputSize {
    fn calc_output_count_and_bytes(&self, include_symlinks: bool) -> OutputCountAndBytes;
}

impl OutputSize for ArtifactValue {
    fn calc_output_count_and_bytes(&self, include_symlinks: bool) -> OutputCountAndBytes {
        self.entry().calc_output_count_and_bytes(include_symlinks)
    }
}

impl<D> OutputSize for DirectoryEntry<D, ActionDirectoryMember>
where
    D: ActionDirectory,
{
    fn calc_output_count_and_bytes(&self, include_symlinks: bool) -> OutputCountAndBytes {
        let mut bytes = 0;
        let mut count = 0;
        let mut walk = unordered_entry_walk(self.as_ref().map_dir(|d| Directory::as_ref(d)));
        while let Some((_path, entry)) = walk.next() {
            match entry {
                DirectoryEntry::Leaf(ActionDirectoryMember::File(f)) => {
                    bytes += f.digest.size();
                    count += 1;
                }
                DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(s)) if include_symlinks => {
                    bytes += s.target().as_str().len() as u64;
                    count += 1;
                }
                DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(s))
                    if include_symlinks =>
                {
                    bytes += s.target_str().len() as u64;
                    count += 1;
                }
                _ => {}
            }
        }
        OutputCountAndBytes { count, bytes }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use buck2_common::external_symlink::ExternalSymlink;
    use buck2_common::file_ops::metadata::FileDigest;
    use buck2_common::file_ops::metadata::FileDigestConfig;
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::Symlink;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_fs::paths::abs_path::AbsPath;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

    use super::*;
    use crate::digest_config::DigestConfig;
    use crate::directory::ActionDirectoryBuilder;
    use crate::directory::INTERNER;
    use crate::directory::insert_entry;
    use crate::directory::insert_file;
    use crate::directory::insert_symlink;

    fn path(s: &str) -> ProjectRelativePathBuf {
        ProjectRelativePath::new(s).unwrap().to_buf()
    }

    #[test]
    fn test_directory_with_multiple_files() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        let content1 = b"hello"; // 5 bytes
        let content2 = b"world!"; // 6 bytes
        let content3 = b"test content here"; // 17 bytes

        insert_file(
            &mut builder,
            path("file1.txt"),
            FileMetadata {
                digest: TrackedFileDigest::from_content(
                    content1,
                    digest_config.cas_digest_config(),
                ),
                is_executable: false,
            },
        )?;
        insert_file(
            &mut builder,
            path("file2.txt"),
            FileMetadata {
                digest: TrackedFileDigest::from_content(
                    content2,
                    digest_config.cas_digest_config(),
                ),
                is_executable: false,
            },
        )?;
        insert_file(
            &mut builder,
            path("file3.txt"),
            FileMetadata {
                digest: TrackedFileDigest::from_content(
                    content3,
                    digest_config.cas_digest_config(),
                ),
                is_executable: false,
            },
        )?;

        let dir = builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);
        let entry = DirectoryEntry::Dir(dir);
        let result = entry.calc_output_count_and_bytes(false);

        assert_eq!(result.count, 3);
        assert_eq!(result.bytes, 5 + 6 + 17);

        Ok(())
    }

    #[test]
    fn test_nested_directory() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        // Create nested directory structure:
        // /
        // ├── top.txt (4 bytes)
        // └── subdir/
        //     ├── mid.txt (3 bytes)
        //     └── deep/
        //         └── bottom.txt (6 bytes)

        insert_file(
            &mut builder,
            path("top.txt"),
            FileMetadata {
                digest: TrackedFileDigest::from_content(b"test", digest_config.cas_digest_config()),
                is_executable: false,
            },
        )?;
        insert_file(
            &mut builder,
            path("subdir/mid.txt"),
            FileMetadata {
                digest: TrackedFileDigest::from_content(b"mid", digest_config.cas_digest_config()),
                is_executable: false,
            },
        )?;
        insert_file(
            &mut builder,
            path("subdir/deep/bottom.txt"),
            FileMetadata {
                digest: TrackedFileDigest::from_content(
                    b"bottom",
                    digest_config.cas_digest_config(),
                ),
                is_executable: false,
            },
        )?;

        let dir = builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);
        let entry = DirectoryEntry::Dir(dir);
        let result = entry.calc_output_count_and_bytes(false);

        assert_eq!(result.count, 3);
        assert_eq!(result.bytes, 4 + 3 + 6);

        Ok(())
    }

    #[test]
    fn test_symlinks_not_counted() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        // Create directory with only symlinks (no regular files)
        insert_symlink(
            &mut builder,
            path("link1"),
            Arc::new(Symlink::new("target1".into())),
        )?;
        insert_symlink(
            &mut builder,
            path("link2"),
            Arc::new(Symlink::new("target2".into())),
        )?;
        insert_symlink(
            &mut builder,
            path("subdir/link3"),
            Arc::new(Symlink::new("../target3".into())),
        )?;

        let dir = builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);
        let entry = DirectoryEntry::Dir(dir);
        let result = entry.calc_output_count_and_bytes(false);

        // Symlinks should not be counted
        assert_eq!(result.count, 0);
        assert_eq!(result.bytes, 0);

        Ok(())
    }

    #[test]
    fn test_symlinks_counted_when_included() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        let target1 = "target1"; // 7 bytes
        let target2 = "target2"; // 7 bytes
        let target3 = "../target3"; // 10 bytes
        let external_target = "/mnt/gvfs/path"; // 14 bytes

        insert_symlink(
            &mut builder,
            path("link1"),
            Arc::new(Symlink::new(target1.into())),
        )?;
        insert_symlink(
            &mut builder,
            path("link2"),
            Arc::new(Symlink::new(target2.into())),
        )?;
        insert_symlink(
            &mut builder,
            path("subdir/link3"),
            Arc::new(Symlink::new(target3.into())),
        )?;

        insert_entry(
            &mut builder,
            path("external_link"),
            DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(Arc::new(
                ExternalSymlink::new(
                    PathBuf::from(external_target),
                    ForwardRelativePathBuf::default(),
                )?,
            ))),
        )?;

        let dir = builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);
        let entry = DirectoryEntry::Dir(dir);
        let result = entry.calc_output_count_and_bytes(true);

        // All symlinks (both regular and external) should be counted
        assert_eq!(result.count, 4);
        assert_eq!(
            result.bytes,
            (target1.len() + target2.len() + target3.len() + external_target.len()) as u64
        );

        Ok(())
    }

    #[test]
    fn test_file_size_matches_physical_disk_size() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let tempdir = tempfile::tempdir()?;
        let file_path = tempdir.path().join("test_file.txt");
        let content = b"This is test content for verifying disk size matching.";
        std::fs::write(&file_path, content)?;

        let disk_size = std::fs::metadata(&file_path)?.len();

        let abs_path = AbsPath::new(&file_path)?;
        let file_digest_config = FileDigestConfig::build(digest_config.cas_digest_config());
        let digest = FileDigest::from_file(abs_path, file_digest_config)?;

        assert_eq!(digest.size(), disk_size);
        assert_eq!(disk_size, content.len() as u64);

        Ok(())
    }

    #[test]
    fn test_directory_size_matches_physical_disk_size() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let tempdir = tempfile::tempdir()?;

        let files = [
            ("file1.txt", b"First file content" as &[u8]),
            ("file2.txt", b"Second file with more content here"),
            ("subdir/file3.txt", b"Nested file"),
        ];

        let mut total_disk_size = 0u64;
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        for (name, content) in &files {
            let file_path = tempdir.path().join(name);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file_path, content)?;

            let disk_size = std::fs::metadata(&file_path)?.len();
            total_disk_size += disk_size;

            let abs_path = AbsPath::new(&file_path)?;
            let file_digest_config = FileDigestConfig::build(digest_config.cas_digest_config());
            let digest = FileDigest::from_file(abs_path, file_digest_config)?;
            let tracked_digest = TrackedFileDigest::new(digest, digest_config.cas_digest_config());

            insert_file(
                &mut builder,
                path(name),
                FileMetadata {
                    digest: tracked_digest,
                    is_executable: false,
                },
            )?;
        }

        let dir = builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);
        let entry = DirectoryEntry::Dir(dir);
        let result = entry.calc_output_count_and_bytes(false);

        assert_eq!(result.count, 3);
        assert_eq!(result.bytes, total_disk_size);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_size_matches_physical_disk_size() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let tempdir = tempfile::tempdir()?;

        // Define symlinks: (link_name, target, resolved_target_path)
        // resolved_target_path is relative to tempdir and is where we create the actual file
        let symlinks = [
            ("link1", "target1", "target1"),
            ("link2", "some/relative/path", "some/relative/path"),
            ("subdir/link3", "../parent_target", "parent_target"),
        ];

        // Create target files for each symlink and add them to the directory
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        let mut total_file_size = 0u64;
        for (_, _, resolved_target) in &symlinks {
            let target_path = tempdir.path().join(resolved_target);
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content = format!("Content for {}", resolved_target);
            std::fs::write(&target_path, &content)?;
            total_file_size += content.len() as u64;

            // Also add the target file to the directory so symlinks aren't dangling
            insert_file(
                &mut builder,
                path(resolved_target),
                FileMetadata {
                    digest: TrackedFileDigest::from_content(
                        content.as_bytes(),
                        digest_config.cas_digest_config(),
                    ),
                    is_executable: false,
                },
            )?;
        }

        let mut total_symlink_size = 0u64;

        for (link_name, target, _) in &symlinks {
            let link_path = tempdir.path().join(link_name);
            if let Some(parent) = link_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Create symlink on disk
            std::os::unix::fs::symlink(target, &link_path)?;

            let disk_target_size = std::fs::symlink_metadata(&link_path)?.len();
            total_symlink_size += disk_target_size;

            insert_symlink(
                &mut builder,
                path(link_name),
                Arc::new(Symlink::new((*target).into())),
            )?;
        }

        let dir = builder
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);
        let entry = DirectoryEntry::Dir(dir);
        let result = entry.calc_output_count_and_bytes(true);

        // Count includes 3 files + 3 symlinks = 6 total items
        assert_eq!(result.count, 6);
        // Bytes includes file content sizes + symlink target sizes
        assert_eq!(result.bytes, total_file_size + total_symlink_size);

        Ok(())
    }
}
