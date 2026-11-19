/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The sqlite columns an action's output is kept in by the tables that persist outputs: a leaf
//! (file or symlink) in full, a directory as its fingerprint alone, the tree being the
//! materializer's to persist and reload. One codec, so that a row means the same in every such
//! table.

use std::sync::Arc;

use buck2_common::external_symlink::ExternalSymlink;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::Symlink;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::dep_file_state::StoredOutputValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

use crate::materializers::artifact_type::ArtifactType;

/// The columns, in the order `StoredOutputColumns::read` expects them, for a table's `INSERT` and
/// `SELECT`.
pub(crate) const COLUMN_NAMES: &str = "artifact_type, entry_size, entry_hash, entry_hash_kind, file_is_executable, symlink_target, symlink_remaining_path";

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = InternalError)]
enum StoredOutputRowError {
    #[error(
        "Internal error: expected field `{field}` to be present for artifact type `{artifact_type}`"
    )]
    ExpectedFieldIsMissing {
        field: &'static str,
        artifact_type: ArtifactType,
    },
}

/// One output as its row's columns hold it.
pub(crate) struct StoredOutputColumns {
    pub(crate) artifact_type: ArtifactType,
    pub(crate) entry_size: Option<u64>,
    pub(crate) entry_hash: Option<Vec<u8>>,
    pub(crate) entry_hash_kind: Option<u8>,
    pub(crate) file_is_executable: Option<bool>,
    pub(crate) symlink_target: Option<String>,
    /// Only set for external symlinks with a non-empty `remaining_path`.
    pub(crate) symlink_remaining_path: Option<String>,
}

impl StoredOutputColumns {
    pub(crate) fn encode(value: &StoredOutputValue) -> buck2_error::Result<Self> {
        let none = |artifact_type| Self {
            artifact_type,
            entry_size: None,
            entry_hash: None,
            entry_hash_kind: None,
            file_is_executable: None,
            symlink_target: None,
            symlink_remaining_path: None,
        };
        Ok(match value {
            StoredOutputValue::Directory(fingerprint) => {
                let (size, bytes, kind) = file_digest_parts(fingerprint);
                Self {
                    entry_size: Some(size),
                    entry_hash: Some(bytes.to_vec()),
                    entry_hash_kind: Some(kind),
                    ..none(ArtifactType::Directory)
                }
            }
            StoredOutputValue::Leaf(value) => match value.entry() {
                DirectoryEntry::Dir(_) => {
                    return Err(internal_error!(
                        "`StoredOutputValue::Leaf` unexpectedly holds a directory"
                    ));
                }
                DirectoryEntry::Leaf(ActionDirectoryMember::File(f)) => {
                    let (size, bytes, kind) = tracked_digest_parts(&f.digest);
                    Self {
                        entry_size: Some(size),
                        entry_hash: Some(bytes.to_vec()),
                        entry_hash_kind: Some(kind),
                        file_is_executable: Some(f.is_executable),
                        ..none(ArtifactType::File)
                    }
                }
                DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(s)) => Self {
                    symlink_target: Some(s.target().as_str().to_owned()),
                    ..none(ArtifactType::Symlink)
                },
                DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(s)) => {
                    // `remaining_path` goes along when non-empty so the symlink round-trips
                    // exactly; `target_str()` alone is just `abs_target`.
                    let remaining = s.remaining_path();
                    Self {
                        symlink_target: Some(s.target_str().to_owned()),
                        symlink_remaining_path: (!remaining.is_empty())
                            .then(|| remaining.as_str().to_owned()),
                        ..none(ArtifactType::ExternalSymlink)
                    }
                }
            },
        })
    }

    /// Reads the columns of `row`, `first` being the index of `artifact_type`.
    pub(crate) fn read(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            artifact_type: row.get(first)?,
            entry_size: row.get(first + 1)?,
            entry_hash: row.get(first + 2)?,
            entry_hash_kind: row.get(first + 3)?,
            file_is_executable: row.get(first + 4)?,
            symlink_target: row.get(first + 5)?,
            symlink_remaining_path: row.get(first + 6)?,
        })
    }

    pub(crate) fn decode(
        self,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<StoredOutputValue> {
        let missing = |field| StoredOutputRowError::ExpectedFieldIsMissing {
            field,
            artifact_type: self.artifact_type,
        };
        let digest = || {
            let size = self.entry_size.ok_or_else(|| missing("entry_size"))?;
            let bytes = self
                .entry_hash
                .as_deref()
                .ok_or_else(|| missing("entry_hash"))?;
            let kind = self
                .entry_hash_kind
                .ok_or_else(|| missing("entry_hash_kind"))?;
            rebuild_file_digest(size, bytes, kind)
        };
        let member = match self.artifact_type {
            ArtifactType::Directory => return Ok(StoredOutputValue::Directory(digest()?)),
            ArtifactType::File => ActionDirectoryMember::File(FileMetadata {
                digest: TrackedFileDigest::new(digest()?, digest_config.cas_digest_config()),
                is_executable: self
                    .file_is_executable
                    .ok_or_else(|| missing("file_is_executable"))?,
            }),
            ArtifactType::Symlink => {
                let target = self
                    .symlink_target
                    .as_deref()
                    .ok_or_else(|| missing("symlink_target"))?;
                ActionDirectoryMember::Symlink(Arc::new(Symlink::new(target.into())))
            }
            ArtifactType::ExternalSymlink => {
                let target = self
                    .symlink_target
                    .as_deref()
                    .ok_or_else(|| missing("symlink_target"))?;
                // An external symlink compares by both fields, so one with a non-empty
                // `remaining_path` would never match if it came back without it.
                let remaining_path = self
                    .symlink_remaining_path
                    .map(ForwardRelativePathBuf::new)
                    .transpose()
                    .buck_error_context("Invalid symlink `remaining_path`")?
                    .unwrap_or_default();
                ActionDirectoryMember::ExternalSymlink(Arc::new(ExternalSymlink::new(
                    target.into(),
                    remaining_path,
                )?))
            }
        };
        Ok(StoredOutputValue::Leaf(ArtifactValue::new(
            DirectoryEntry::Leaf(member),
            None,
        )))
    }
}

pub(crate) fn file_digest_parts(digest: &FileDigest) -> (u64, &[u8], u8) {
    (
        digest.size(),
        digest.raw_digest().as_bytes(),
        digest.raw_digest().algorithm() as _,
    )
}

pub(crate) fn tracked_digest_parts(digest: &TrackedFileDigest) -> (u64, &[u8], u8) {
    (
        digest.size(),
        digest.raw_digest().as_bytes(),
        digest.raw_digest().algorithm() as _,
    )
}

pub(crate) fn rebuild_file_digest(
    size: u64,
    bytes: &[u8],
    kind: u8,
) -> buck2_error::Result<FileDigest> {
    let kind = kind
        .try_into()
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::InternalError))
        .with_buck_error_context(|| format!("Invalid digest kind: `{kind}`"))?;
    FileDigest::from_digest_bytes(kind, bytes, size)
}
