/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(clippy::ref_option_ref)] // within Serialize

use std::fmt::Display;
use std::path::PathBuf;

use buck2_common::cas_digest::CasDigest;
use buck2_common::file_ops::metadata::FileDigestKind;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_fs::paths::RelativePathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use serde::Serialize;
use serde::Serializer;

fn stringify<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Display,
    S: Serializer,
{
    serializer.collect_str(value)
}

#[derive(Serialize, Debug)]
pub struct DirectoryInfo {
    #[serde(serialize_with = "stringify")]
    pub digest: TrackedFileDigest,
}

#[derive(Serialize, Debug)]
pub struct FileInfo {
    #[serde(serialize_with = "stringify")]
    pub digest: CasDigest<FileDigestKind>,
    pub is_exec: bool,
}

#[derive(Serialize, Debug)]
pub struct SymlinkInfo {
    #[serde(serialize_with = "stringify")]
    pub symlink_rel_path: RelativePathBuf,
}

#[derive(Serialize, Debug)]
pub struct ExternalSymlinkInfo {
    pub target: PathBuf,
    pub remaining_path: Option<ForwardRelativePathBuf>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum ArtifactInfo {
    Directory(DirectoryInfo),
    File(FileInfo),
    Symlink(SymlinkInfo),
    ExternalSymlink(ExternalSymlinkInfo),
}

#[derive(Serialize, Debug)]
pub struct ArtifactMetadataJson {
    pub path: ForwardRelativePathBuf,
    #[serde(flatten)]
    pub info: ArtifactInfo,
}

#[cfg(test)]
mod tests {
    use buck2_common::cas_digest::CasDigestConfig;

    use super::*;

    #[test]
    fn test_dir_json() {
        let path = ForwardRelativePathBuf::unchecked_new("test".into());
        let digest = CasDigest::parse_digest(
            "fb19d5b1546753df5f7741efbabd0d24dcaacd65:20",
            CasDigestConfig::testing_default(),
        )
        .expect("failed to create digest")
        .0;
        let metadata = ArtifactMetadataJson {
            path,
            info: ArtifactInfo::Directory(DirectoryInfo {
                digest: TrackedFileDigest::new(digest, CasDigestConfig::testing_default()),
            }),
        };
        let json = serde_json::to_string(&metadata).expect("failed to serialize");
        assert_eq!(
            json,
            r#"{"path":"test","kind":"directory","digest":"fb19d5b1546753df5f7741efbabd0d24dcaacd65:20"}"#,
        );
    }

    #[test]
    fn test_file_json() {
        let path = ForwardRelativePathBuf::unchecked_new("test.txt".into());
        let digest = CasDigest::parse_digest(
            "fb19d5b1546753df5f7741efbabd0d24dcaacd65:20",
            CasDigestConfig::testing_default(),
        )
        .expect("failed to create digest")
        .0;
        let metadata = ArtifactMetadataJson {
            path,
            info: ArtifactInfo::File(FileInfo {
                digest,
                is_exec: false,
            }),
        };
        let json = serde_json::to_string(&metadata).expect("failed to serialize");
        assert_eq!(
            json,
            r#"{"path":"test.txt","kind":"file","digest":"fb19d5b1546753df5f7741efbabd0d24dcaacd65:20","is_exec":false}"#,
        );
    }

    #[test]
    fn test_symlink_json() {
        let path = ForwardRelativePathBuf::unchecked_new("test.txt".into());
        let symlink_rel_path = RelativePathBuf::from("../test.txt");
        let metadata = ArtifactMetadataJson {
            path,
            info: ArtifactInfo::Symlink(SymlinkInfo { symlink_rel_path }),
        };
        let json = serde_json::to_string(&metadata).expect("failed to serialize");
        assert_eq!(
            json,
            r#"{"path":"test.txt","kind":"symlink","symlink_rel_path":"../test.txt"}"#,
        );
    }

    #[test]
    fn test_external_symlink_json() {
        let path = ForwardRelativePathBuf::unchecked_new("test.txt".into());
        let target = PathBuf::from("/mnt/gvfs");
        let remaining =
            ForwardRelativePathBuf::new("test.txt".into()).expect("failed to make remaining path");
        let metadata = ArtifactMetadataJson {
            path,
            info: ArtifactInfo::ExternalSymlink(ExternalSymlinkInfo {
                target,
                remaining_path: Some(remaining),
            }),
        };
        let json = serde_json::to_string(&metadata).expect("failed to serialize");
        assert_eq!(
            json,
            r#"{"path":"test.txt","kind":"external_symlink","target":"/mnt/gvfs","remaining_path":"test.txt"}"#,
        );
    }
}
