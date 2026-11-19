/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::LazyLock;

use buck2_common::external_symlink::ExternalSymlink;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::Symlink;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::conversion::rusqlite::Buck2ErrorAsRusqliteError;
use buck2_error::internal_error;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::directory::INTERNER;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutMap;
use itertools::Itertools;
use jiff::Timestamp;
use parking_lot::Mutex;
use rusqlite::Connection;
use rusqlite::types::FromSql;
use rusqlite::types::FromSqlError;
use rusqlite::types::FromSqlResult;
use rusqlite::types::ToSql;
use rusqlite::types::ToSqlOutput;
use rusqlite::types::Value;
use rusqlite::types::ValueRef;

use crate::materializers::artifact_type::ARTIFACT_TYPE_DIRECTORY;
use crate::materializers::artifact_type::ARTIFACT_TYPE_EXTERNAL_SYMLINK;
use crate::materializers::artifact_type::ARTIFACT_TYPE_FILE;
use crate::materializers::artifact_type::ARTIFACT_TYPE_SYMLINK;
use crate::materializers::artifact_type::ArtifactType;
use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;
use crate::materializers::v2::entry::DiskEntry;
use crate::sqlite::materializer_db::MaterializerState;
use crate::sqlite::materializer_db::MaterializerStateEntry;

const STATE_TABLE_NAME: &str = "materializer_state";

#[derive(buck2_error::Error, Debug, PartialEq, Eq)]
#[buck2(tag = InternalError)]
enum MaterializerStateTableError {
    #[error("Internal error: expected field `{}` to be not null for artifact type '{}'", .field, .artifact_type)]
    ExpectedFieldIsMissing {
        field: &'static str,
        artifact_type: ArtifactType,
    },
    #[error(
        "Internal error: expected last access timestamp to be present for sqlite entry which represents an artifact"
    )]
    ExpectedLastAccessTimeForArtifact,
    #[error("Internal error: code should not be reachable for sqlite entry of directory type")]
    CodePathNotSupportedForDirEntry,
    #[error("Internal error: missing an sqlite entry representing directory artifact `{0}`")]
    DirectoryArtifactEntryMissing(ProjectRelativePathBuf),
    #[error("Internal error: expected classification to be present for an artifact")]
    ArtifactClassificationMissing,
}

impl ArtifactClassification {
    fn sqlite_value(self) -> i64 {
        match self {
            Self::IntermediateOnly => 0,
            Self::FinalOutput => 1,
        }
    }
}

impl ToSql for ArtifactClassification {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Integer(self.sqlite_value())))
    }
}

impl FromSql for ArtifactClassification {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_i64()? {
            0 => Ok(Self::IntermediateOnly),
            1 => Ok(Self::FinalOutput),
            invalid => Err(FromSqlError::Other(Box::new(Buck2ErrorAsRusqliteError(
                internal_error!("invalid artifact classification `{invalid}`"),
            )))),
        }
    }
}

/// Sqlite representation of sha1. Can be converted directly into BLOB type
/// in rusqlite. Note we use a Vec and not a fixed length array here because
/// rusqlite can only convert to Vec.
type SqliteDigest = Vec<u8>;

/// Representation of row in sqlite table. All datatypes used implement rusqlite's `FromSql` trait.
/// A single row might represent different things:
/// 1. Row fully represent an artifact.
///    That could be file, symlink or external symlink.
/// 2. Row represents a directory artifact.
///    In this case all directory's content will be stored in other rows as well in order to be able to recreate [`ActionSharedDirectory`] from DB.
///    Only `path`, `last_access_time` and `artifact_type` equal to [`ArtifactType::Directory`] will be present for such rows.
///    Other rows which have `parent_path` equal to `path` of this row represent directory's content.
/// 3. Row represents a content inside a directory.
///    `path` is prefixed by a directory artifact path (not relative to containing directory).
///    `parent_path` will be present for such rows. `last_access_time` will be missing from such rows.
#[derive(Debug)]
struct SqliteEntry<'a> {
    path: Cow<'a, str>,
    artifact_type: ArtifactType,
    entry_size: Option<u64>,
    entry_hash: Option<Cow<'a, [u8]>>,
    entry_hash_kind: Option<u8>,
    file_is_executable: Option<bool>,
    symlink_target: Option<Cow<'a, str>>,
    /// Only present for entries which represent an actual artifact.
    last_access_time: Option<i64>,
    /// Path of the directory artifact which this entry belongs to.
    /// Entry represents the actual artifact when the value is not present.
    parent_path: Option<Cow<'a, str>>,
    classification: Option<ArtifactClassification>,
}

impl<'a> SqliteEntry<'a> {
    fn new(
        path: String,
        artifact_type: ArtifactType,
        entry_size: Option<u64>,
        entry_hash: Option<SqliteDigest>,
        entry_hash_kind: Option<u8>,
        file_is_executable: Option<bool>,
        symlink_target: Option<String>,
        last_access_time: Option<i64>,
        parent_path: Option<String>,
        classification: Option<ArtifactClassification>,
    ) -> Self {
        Self {
            path: Cow::Owned(path),
            artifact_type,
            entry_size,
            entry_hash: entry_hash.map(Cow::Owned),
            entry_hash_kind,
            file_is_executable,
            symlink_target: symlink_target.map(Cow::Owned),
            last_access_time,
            parent_path: parent_path.map(Cow::Owned),
            classification,
        }
    }
}

fn digest_parts(digest: &TrackedFileDigest) -> (u64, &[u8], u8) {
    (
        digest.size(),
        digest.raw_digest().as_bytes(),
        digest.raw_digest().algorithm() as _,
    )
}

fn convert_artifact_metadata_to_sqlite_entries<'a>(
    path: &'a ProjectRelativePath,
    metadata: &'a ArtifactMetadata,
    timestamp: &'_ Timestamp,
    classification: ArtifactClassification,
) -> Vec<SqliteEntry<'a>> {
    let last_access_time = timestamp.as_second();
    match metadata {
        DirectoryEntry::Dir(action_shared_directory) => {
            convert_action_shared_directory_to_sqlite_entries(
                path,
                action_shared_directory,
                last_access_time,
                classification,
            )
        }
        DirectoryEntry::Leaf(action_directory_member) => {
            vec![convert_action_directory_member_to_sqlite_entry(
                Cow::Borrowed(path.as_str()),
                action_directory_member,
                Some(last_access_time),
                None,
                Some(classification),
            )]
        }
    }
}

/// The rows for a root the disk-state core tracks: the same rows as for an `ArtifactMetadata`,
/// from the tree's own entry type.
fn convert_disk_entry_to_sqlite_entries<'a>(
    path: &'a ProjectRelativePath,
    entry: &'a DiskEntry,
    timestamp: &'_ Timestamp,
    classification: ArtifactClassification,
) -> Vec<SqliteEntry<'a>> {
    let mut result = vec![convert_disk_entry_to_sqlite_entry(
        Cow::Borrowed(path.as_str()),
        entry,
        Some(timestamp.as_second()),
        None,
        Some(classification),
    )];
    if let DiskEntry::Dir(dir) = entry {
        dir.walk(&mut |entry_path, entry| {
            result.push(convert_disk_entry_to_sqlite_entry(
                Cow::Owned(
                    path.join(entry_path)
                        .into_forward_relative_path_buf()
                        .into_string(),
                ),
                entry,
                None,
                Some(Cow::Borrowed(path.as_str())),
                None,
            ));
        });
    }
    result
}

fn convert_disk_entry_to_sqlite_entry<'a>(
    path: Cow<'a, str>,
    entry: &'a DiskEntry,
    last_access_time: Option<i64>,
    parent_path: Option<Cow<'a, str>>,
    classification: Option<ArtifactClassification>,
) -> SqliteEntry<'a> {
    let (
        artifact_type,
        entry_size,
        entry_hash,
        entry_hash_kind,
        file_is_executable,
        symlink_target,
    ) = match entry {
        DiskEntry::File(file) => (
            ArtifactType::File,
            Some(file.digest.size()),
            Some(Cow::Borrowed(file.digest.raw_digest().as_bytes())),
            Some(file.digest.raw_digest().algorithm() as u8),
            Some(file.executable),
            None,
        ),
        DiskEntry::Symlink(symlink) => (
            ArtifactType::Symlink,
            None,
            None,
            None,
            None,
            Some(Cow::Borrowed(symlink.target().as_str())),
        ),
        DiskEntry::ExternalSymlink(symlink) => (
            ArtifactType::ExternalSymlink,
            None,
            None,
            None,
            None,
            Some(Cow::Borrowed(symlink.target_str())),
        ),
        DiskEntry::Dir(_) => (ArtifactType::Directory, None, None, None, None, None),
    };
    SqliteEntry {
        path,
        artifact_type,
        entry_size,
        entry_hash,
        entry_hash_kind,
        file_is_executable,
        symlink_target,
        last_access_time,
        parent_path,
        classification,
    }
}

fn convert_action_shared_directory_to_sqlite_entries<'a>(
    path: &'a ProjectRelativePath,
    action_shared_directory: &'a ActionSharedDirectory,
    last_access_time: i64,
    classification: ArtifactClassification,
) -> Vec<SqliteEntry<'a>> {
    let artifact_path = Cow::Borrowed(path.as_str());
    let mut result = vec![SqliteEntry {
        path: artifact_path.clone(),
        artifact_type: ArtifactType::Directory,
        entry_size: None,
        entry_hash: None,
        entry_hash_kind: None,
        file_is_executable: None,
        symlink_target: None,
        last_access_time: Some(last_access_time),
        parent_path: None,
        classification: Some(classification),
    }];
    // Now create and entry for each transitive member of this directory
    let mut walk = unordered_entry_walk(DirectoryEntry::Dir(action_shared_directory));
    while let Some((entry_path, entry)) = walk.next() {
        let parent_path = Some(artifact_path.clone());
        let project_relative_entry_path = {
            let mut res = path.to_owned();
            res.push(entry_path.get());
            Cow::Owned(res.into_forward_relative_path_buf().into_string())
        };
        result.push(match entry {
            DirectoryEntry::Dir(_) => SqliteEntry {
                path: project_relative_entry_path,
                artifact_type: ArtifactType::Directory,
                entry_size: None,
                entry_hash: None,
                entry_hash_kind: None,
                file_is_executable: None,
                symlink_target: None,
                last_access_time: None,
                parent_path,
                classification: None,
            },
            DirectoryEntry::Leaf(action_directory_member) => {
                convert_action_directory_member_to_sqlite_entry(
                    project_relative_entry_path,
                    action_directory_member,
                    None,
                    parent_path,
                    None,
                )
            }
        });
    }
    result
}

fn convert_action_directory_member_to_sqlite_entry<'a>(
    path: Cow<'a, str>,
    action_directory_member: &'a ActionDirectoryMember,
    last_access_time: Option<i64>,
    parent_path: Option<Cow<'a, str>>,
    classification: Option<ArtifactClassification>,
) -> SqliteEntry<'a> {
    match action_directory_member {
        ActionDirectoryMember::File(file_metadata) => {
            let (entry_size, entry_hash, entry_hash_kind) = digest_parts(&file_metadata.digest);
            SqliteEntry {
                path,
                artifact_type: ArtifactType::File,
                entry_size: Some(entry_size),
                entry_hash: Some(Cow::Borrowed(entry_hash)),
                entry_hash_kind: Some(entry_hash_kind),
                file_is_executable: Some(file_metadata.is_executable),
                symlink_target: None,
                last_access_time,
                parent_path,
                classification,
            }
        }
        ActionDirectoryMember::Symlink(symlink) => SqliteEntry {
            path,
            artifact_type: ArtifactType::Symlink,
            entry_size: None,
            entry_hash: None,
            entry_hash_kind: None,
            file_is_executable: None,
            symlink_target: Some(Cow::Borrowed(symlink.target().as_str())),
            last_access_time,
            parent_path,
            classification,
        },
        ActionDirectoryMember::ExternalSymlink(external_symlink) => SqliteEntry {
            path,
            artifact_type: ArtifactType::ExternalSymlink,
            entry_size: None,
            entry_hash: None,
            entry_hash_kind: None,
            file_is_executable: None,
            symlink_target: Some(Cow::Borrowed(external_symlink.target_str())),
            last_access_time,
            parent_path,
            classification,
        },
    }
}

fn convert_sqlite_entries_to_materializer_state(
    entries: Vec<SqliteEntry>,
    digest_config: DigestConfig,
) -> buck2_error::Result<MaterializerState> {
    #[derive(Default)]
    struct DirectoryData<'a> {
        timestamp: Option<Timestamp>,
        classification: Option<ArtifactClassification>,
        children: Vec<SqliteEntry<'a>>,
    }

    let mut directories: BuckMutMap<ProjectRelativePathBuf, DirectoryData> = BuckMutMap::default();

    let mut results = Vec::new();

    // On a first pass convert entries representing actual artifacts
    // and collect information about each "full"-representation directory.
    for entry in entries {
        let path = ProjectRelativePathBuf::unchecked_new(entry.path.clone().into_owned());
        if let Some(parent_artifact_path) = entry.parent_path.clone() {
            // Represents a part of directory artifact
            let parent_artifact_path =
                ProjectRelativePathBuf::unchecked_new(parent_artifact_path.into_owned());
            let DirectoryData { children, .. } =
                directories.entry(parent_artifact_path).or_default();
            children.push(entry);
        } else {
            // Represents artifact
            let Some(last_access_time) = entry.last_access_time else {
                return Err(MaterializerStateTableError::ExpectedLastAccessTimeForArtifact.into());
            };
            let last_access_time = Timestamp::from_second(last_access_time)
                .map_err(|_| internal_error!("invalid timestamp"))?;
            match entry.artifact_type {
                ArtifactType::Directory => {
                    let classification = entry
                        .classification
                        .ok_or(MaterializerStateTableError::ArtifactClassificationMissing)?;
                    let DirectoryData {
                        timestamp,
                        classification: stored_classification,
                        ..
                    } = directories.entry(path).or_default();
                    _ = timestamp.insert(last_access_time);
                    _ = stored_classification.insert(classification);
                }
                _ => {
                    let state_entry =
                        convert_non_directory_sqlite_entry_to_materializer_state_entry(
                            entry,
                            last_access_time,
                            digest_config,
                        )?;
                    results.push(state_entry);
                }
            }
        }
    }

    // Now recreate "full" directories
    for (
        path,
        DirectoryData {
            timestamp,
            classification,
            children,
        },
    ) in directories
    {
        let dir = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
            for child in children {
                let child_path = child.path.clone();
                let parent_dir_rel_path = {
                    let child_path = ProjectRelativePath::unchecked_new(&child_path);
                    child_path.strip_prefix(&path)?
                };
                if child.artifact_type == ArtifactType::Directory {
                    builder.mkdir(parent_dir_rel_path)?;
                } else {
                    let member = convert_non_directory_sqlite_entry_to_action_directory_member(
                        child,
                        digest_config,
                    )?;
                    builder.insert(parent_dir_rel_path, DirectoryEntry::Leaf(member))?;
                }
            }
            // A materialized directory artifact reloaded from sqlite is complete content.
            builder.mark_uniformly_exhaustive();
            let fingerprint = builder.fingerprint(digest_config.as_directory_serializer());
            fingerprint.shared(&*INTERNER)
        };
        let Some(last_access_time) = timestamp else {
            return Err(MaterializerStateTableError::DirectoryArtifactEntryMissing(path).into());
        };
        let classification =
            classification.ok_or(MaterializerStateTableError::ArtifactClassificationMissing)?;
        results.push(MaterializerStateEntry {
            path,
            metadata: ActionDirectoryEntry::Dir(dir),
            last_access_time,
            classification,
        })
    }

    Ok(results)
}

fn convert_non_directory_sqlite_entry_to_materializer_state_entry(
    sqlite_entry: SqliteEntry,
    last_access_time: Timestamp,
    digest_config: DigestConfig,
) -> buck2_error::Result<MaterializerStateEntry> {
    let path = ProjectRelativePathBuf::unchecked_new(sqlite_entry.path.clone().into_owned());
    let classification = sqlite_entry
        .classification
        .ok_or(MaterializerStateTableError::ArtifactClassificationMissing)?;
    let member =
        convert_non_directory_sqlite_entry_to_action_directory_member(sqlite_entry, digest_config)?;
    Ok(MaterializerStateEntry {
        path,
        metadata: DirectoryEntry::Leaf(member),
        last_access_time,
        classification,
    })
}

fn digest(
    size: Option<u64>,
    entry_hash: Option<&[u8]>,
    entry_hash_kind: Option<u8>,
    artifact_type: ArtifactType,
    digest_config: DigestConfig,
) -> buck2_error::Result<TrackedFileDigest> {
    let size = size.ok_or(MaterializerStateTableError::ExpectedFieldIsMissing {
        field: "size",
        artifact_type,
    })?;
    let entry_hash = entry_hash.ok_or(MaterializerStateTableError::ExpectedFieldIsMissing {
        field: "entry_hash",
        artifact_type,
    })?;
    let entry_hash_kind = entry_hash_kind.ok_or({
        MaterializerStateTableError::ExpectedFieldIsMissing {
            field: "entry_hash_kind",
            artifact_type,
        }
    })?;
    let entry_hash_kind = entry_hash_kind
        .try_into()
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
        .with_buck_error_context(|| format!("Invalid entry_hash_kind: `{entry_hash_kind}`"))?;

    let file_digest = FileDigest::from_digest_bytes(entry_hash_kind, entry_hash, size)?;
    Ok(TrackedFileDigest::new(
        file_digest,
        digest_config.cas_digest_config(),
    ))
}

fn convert_non_directory_sqlite_entry_to_action_directory_member(
    sqlite_entry: SqliteEntry,
    digest_config: DigestConfig,
) -> buck2_error::Result<ActionDirectoryMember> {
    Ok(match sqlite_entry.artifact_type {
        ArtifactType::Directory => {
            return Err(MaterializerStateTableError::CodePathNotSupportedForDirEntry.into());
        }
        ArtifactType::File => ActionDirectoryMember::File(FileMetadata {
            digest: digest(
                sqlite_entry.entry_size,
                sqlite_entry.entry_hash.as_deref(),
                sqlite_entry.entry_hash_kind,
                sqlite_entry.artifact_type,
                digest_config,
            )?,
            is_executable: sqlite_entry.file_is_executable.ok_or({
                MaterializerStateTableError::ExpectedFieldIsMissing {
                    field: "file_is_executable",
                    artifact_type: sqlite_entry.artifact_type,
                }
            })?,
        }),
        ArtifactType::Symlink => {
            let symlink = Symlink::new(
                sqlite_entry
                    .symlink_target
                    .ok_or(MaterializerStateTableError::ExpectedFieldIsMissing {
                        field: "symlink_target",
                        artifact_type: sqlite_entry.artifact_type,
                    })?
                    .into_owned()
                    .into(),
            );
            ActionDirectoryMember::Symlink(Arc::new(symlink))
        }
        ArtifactType::ExternalSymlink => {
            let external_symlink = ExternalSymlink::new(
                sqlite_entry
                    .symlink_target
                    .ok_or(MaterializerStateTableError::ExpectedFieldIsMissing {
                        field: "symlink_target",
                        artifact_type: sqlite_entry.artifact_type,
                    })?
                    .into_owned()
                    .into(),
                ForwardRelativePathBuf::default(),
            )?;
            ActionDirectoryMember::ExternalSymlink(Arc::new(external_symlink))
        }
    })
}

pub struct MaterializerStateSqliteTable {
    connection: Arc<Mutex<Connection>>,
}

impl MaterializerStateSqliteTable {
    pub fn new(connection: Arc<Mutex<Connection>>) -> Self {
        Self { connection }
    }

    pub(crate) fn create_table(&self) -> buck2_error::Result<()> {
        let sql = format!(
            "CREATE TABLE {STATE_TABLE_NAME} (
                path                    TEXT NOT NULL PRIMARY KEY,
                artifact_type           TEXT CHECK(artifact_type IN ('{ARTIFACT_TYPE_DIRECTORY}','{ARTIFACT_TYPE_FILE}','{ARTIFACT_TYPE_SYMLINK}','{ARTIFACT_TYPE_EXTERNAL_SYMLINK}')) NOT NULL,
                digest_size             INTEGER NULL DEFAULT NULL,
                entry_hash              BLOB NULL DEFAULT NULL,
                entry_hash_kind         INTEGER NULL DEFAULT NULL,
                file_is_executable      INTEGER NULL DEFAULT NULL,
                symlink_target          TEXT NULL DEFAULT NULL,
                last_access_time        INTEGER NULL DEFAULT NULL,
                parent_path             TEXT NULL DEFAULT NULL,
                classification          INTEGER NULL DEFAULT NULL CHECK(classification IN (0, 1))
            )",
        );
        tracing::trace!(sql = %*sql, "creating table");
        self.connection
            .lock()
            .execute(&sql, [])
            .with_buck_error_context(|| format!("creating sqlite table {STATE_TABLE_NAME}"))?;
        self.create_parent_path_index()?;
        Ok(())
    }

    // Index is useful for faster removals
    fn create_parent_path_index(&self) -> buck2_error::Result<()> {
        let sql = format!(
            "CREATE INDEX {STATE_TABLE_NAME}_index ON {STATE_TABLE_NAME} (
                parent_path
            )",
        );
        tracing::trace!(sql = %*sql, "creating index");
        self.connection
            .lock()
            .execute(&sql, [])
            .with_buck_error_context(|| format!("creating index on {STATE_TABLE_NAME}"))?;
        Ok(())
    }

    pub(crate) fn insert(
        &self,
        path: &ProjectRelativePath,
        metadata: &ArtifactMetadata,
        timestamp: Timestamp,
        classification: ArtifactClassification,
    ) -> buck2_error::Result<()> {
        let entries =
            convert_artifact_metadata_to_sqlite_entries(path, metadata, &timestamp, classification);
        self.insert_rows(path, entries)
    }

    /// As [`insert`](Self::insert), for a root the disk-state core tracks.
    pub(crate) fn insert_disk_entry(
        &self,
        path: &ProjectRelativePath,
        entry: &DiskEntry,
        timestamp: Timestamp,
        classification: ArtifactClassification,
    ) -> buck2_error::Result<()> {
        let entries = convert_disk_entry_to_sqlite_entries(path, entry, &timestamp, classification);
        self.insert_rows(path, entries)
    }

    fn insert_rows(
        &self,
        path: &ProjectRelativePath,
        entries: Vec<SqliteEntry<'_>>,
    ) -> buck2_error::Result<()> {
        static SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "INSERT INTO {STATE_TABLE_NAME} (path, artifact_type, digest_size, entry_hash, entry_hash_kind, file_is_executable, symlink_target, last_access_time, parent_path, classification) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
            )
        });
        let mut conn = self.connection.lock();
        let tx = conn.transaction()?;
        for entry in entries {
            tracing::trace!(sql = %*SQL, entry = ?entry, "inserting into table");
            tx.execute(
                &SQL,
                rusqlite::params![
                    entry.path,
                    entry.artifact_type,
                    entry.entry_size,
                    entry.entry_hash,
                    entry.entry_hash_kind,
                    entry.file_is_executable,
                    entry.symlink_target,
                    entry.last_access_time,
                    entry.parent_path,
                    entry.classification,
                ],
            )
            .with_buck_error_context(|| {
                format!("inserting `{path}` into sqlite table {STATE_TABLE_NAME}")
            })?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn update_access_times(
        &self,
        updates: Vec<&ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()> {
        let mut conn = self.connection.lock();
        let tx = conn.transaction()?;
        for chunk in updates.chunks(100) {
            let sql = format!(
                "UPDATE {} SET last_access_time = {} WHERE path IN ({})",
                STATE_TABLE_NAME,
                Timestamp::now().as_second(),
                itertools::repeat_n("?", chunk.len()).join(","),
            );
            tracing::trace!(sql = %sql, chunk = ?chunk, "updating last_access_times");
            tx.execute(
                &sql,
                rusqlite::params_from_iter(chunk.iter().map(|p| p.as_str()).collect::<Vec<_>>()),
            )
            .with_buck_error_context(|| format!("updating sqlite table {STATE_TABLE_NAME}"))?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn update_classifications(
        &self,
        paths: &[ProjectRelativePathBuf],
        classification: ArtifactClassification,
    ) -> buck2_error::Result<()> {
        let mut conn = self.connection.lock();
        let tx = conn.transaction()?;
        for chunk in paths.chunks(100) {
            let sql = format!(
                "UPDATE {STATE_TABLE_NAME} SET classification = {} WHERE path IN ({}) AND parent_path IS NULL",
                classification.sqlite_value(),
                itertools::repeat_n("?", chunk.len()).join(","),
            );
            tracing::trace!(sql = %sql, chunk = ?chunk, "updating classifications");
            tx.execute(
                &sql,
                rusqlite::params_from_iter(chunk.iter().map(|path| path.as_str())),
            )
            .with_buck_error_context(|| {
                format!("updating classifications in sqlite table {STATE_TABLE_NAME}")
            })?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn read_materializer_state(
        &self,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<MaterializerState> {
        let entries = self.read_all_entries().with_buck_error_context(|| {
            format!("error reading row of sqlite table {STATE_TABLE_NAME}")
        })?;
        convert_sqlite_entries_to_materializer_state(entries, digest_config)
    }

    fn read_all_entries(&self) -> buck2_error::Result<Vec<SqliteEntry<'_>>> {
        static SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "SELECT path, artifact_type, digest_size, entry_hash, entry_hash_kind, file_is_executable, symlink_target, last_access_time, parent_path, classification FROM {STATE_TABLE_NAME}",
            )
        });
        tracing::trace!(sql = %*SQL, "reading all from table");
        let connection = self.connection.lock();
        let mut stmt = connection.prepare(&SQL)?;
        stmt.query_map([], |row| -> rusqlite::Result<SqliteEntry> {
            Ok(SqliteEntry::new(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .with_buck_error_context(|| format!("reading from sqlite table {STATE_TABLE_NAME}"))
    }

    pub(crate) fn delete(&self, paths: Vec<ProjectRelativePathBuf>) -> buck2_error::Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }

        let mut rows_deleted = 0;

        /// Delete only rows which represent artifacts (that is where `parent_path` is `NULL`). Rows that represent members of a directory artifact are not deleted.
        fn delete_artifact_rows(
            connection: &Arc<Mutex<Connection>>,
            paths: &[ProjectRelativePathBuf],
        ) -> buck2_error::Result<usize> {
            let sql = format!(
                "DELETE FROM {} WHERE path IN ({})",
                STATE_TABLE_NAME,
                // According to rusqlite docs this is the best way to generate the right
                // number of query placeholders.
                itertools::repeat_n("?", paths.len()).join(","),
            );

            tracing::trace!(sql = %sql, chunk = ?paths, "deleting artifact rows from table");
            connection
                .lock()
                .execute(
                    &sql,
                    rusqlite::params_from_iter(paths.iter().map(|p| p.as_str())),
                )
                .with_buck_error_context(|| {
                    format!("deleting artifact rows from sqlite table {STATE_TABLE_NAME}")
                })
        }

        /// Delete rows which represent members of directory artifacts (that is rows where `parent_path` is not `NULL`).
        fn delete_directory_artifact_members(
            connection: &Arc<Mutex<Connection>>,
            paths: &[ProjectRelativePathBuf],
        ) -> buck2_error::Result<usize> {
            let sql = format!(
                "DELETE FROM {} WHERE parent_path IN ({})",
                STATE_TABLE_NAME,
                // According to rusqlite docs this is the best way to generate the right
                // number of query placeholders.
                itertools::repeat_n("?", paths.len()).join(","),
            );

            tracing::trace!(sql = %sql, chunk = ?paths, "deleting directory artifact members from table");
            connection
                .lock()
                .execute(
                    &sql,
                    rusqlite::params_from_iter(paths.iter().map(|p| p.as_str())),
                )
                .with_buck_error_context(|| {
                    format!(
                        "deleting directory artifact members from sqlite table {STATE_TABLE_NAME}"
                    )
                })
        }

        for chunk in paths.chunks(100) {
            rows_deleted += delete_artifact_rows(&self.connection, chunk)?;
            rows_deleted += delete_directory_artifact_members(&self.connection, chunk)?;
        }

        Ok(rows_deleted)
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_directory::directory::builder::DirectoryBuilder;
    use buck2_directory::directory::dashmap_directory_interner::DashMapDirectoryInterner;
    use buck2_execute::directory::new_symlink;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;

    use super::*;

    fn artifact_metadata_eq(a: &ArtifactMetadata, b: &ArtifactMetadata) -> bool {
        match (a, b) {
            (DirectoryEntry::Dir(d1), DirectoryEntry::Dir(d2)) => {
                d1.fingerprint() == d2.fingerprint()
            }
            (DirectoryEntry::Leaf(l1), DirectoryEntry::Leaf(l2)) => l1 == l2,
            _ => false,
        }
    }

    #[test]
    fn test_artifact_metadata_full_dir_sqlite_entry_conversion_succeeds() {
        let digest_config = DigestConfig::testing_default();

        let path = ProjectRelativePath::new("artifact_path").unwrap();
        let last_access_time = Timestamp::UNIX_EPOCH;

        let metadata = {
            let directory = {
                let mut builder = DirectoryBuilder::empty_non_exhaustive();
                {
                    let digest = TrackedFileDigest::from_content(
                        b"hello",
                        digest_config.cas_digest_config(),
                    );
                    let metadata = FileMetadata {
                        digest,
                        is_executable: false,
                    };
                    let file = DirectoryEntry::Leaf(ActionDirectoryMember::File(metadata));
                    builder
                        .insert(ForwardRelativePath::unchecked_new("foo"), file)
                        .unwrap();
                }
                {
                    let empty_directory =
                        DirectoryEntry::Dir(DirectoryBuilder::empty_non_exhaustive());
                    builder
                        .insert(ForwardRelativePath::unchecked_new("bar"), empty_directory)
                        .unwrap();
                }
                {
                    let mut directory_builder = DirectoryBuilder::empty_non_exhaustive();
                    let symlink =
                        ActionDirectoryMember::Symlink(Arc::new(Symlink::new("../foo".into())));
                    directory_builder
                        .insert(
                            ForwardRelativePath::unchecked_new("qux"),
                            DirectoryEntry::Leaf(symlink),
                        )
                        .unwrap();
                    let directory = DirectoryEntry::Dir(directory_builder);
                    builder
                        .insert(ForwardRelativePath::unchecked_new("baz"), directory)
                        .unwrap();
                }
                let interner = DashMapDirectoryInterner::new();
                builder
                    .fingerprint(digest_config.as_directory_serializer())
                    .shared(&interner)
            };
            ActionDirectoryEntry::Dir(directory)
        };

        let classification = ArtifactClassification::FinalOutput;
        let entries = convert_artifact_metadata_to_sqlite_entries(
            path,
            &metadata,
            &last_access_time,
            classification,
        );

        let mut state =
            convert_sqlite_entries_to_materializer_state(entries, digest_config).unwrap();

        assert_eq!(state.len(), 1);

        let MaterializerStateEntry {
            path: result_path,
            metadata: result_metadata,
            last_access_time: result_last_access_time,
            classification: result_classification,
        } = state.pop().unwrap();

        assert_eq!(path, result_path);
        assert!(artifact_metadata_eq(&metadata, &result_metadata));
        assert_eq!(last_access_time, result_last_access_time);
        assert_eq!(classification, result_classification);
    }

    #[test]
    fn test_artifact_metadata_file_sqlite_entry_conversion_succeeds() {
        let digest_config = DigestConfig::testing_default();

        let digest = TrackedFileDigest::from_content(b"foo", digest_config.cas_digest_config());
        let metadata = DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
            digest,
            is_executable: false,
        }));
        let path = ProjectRelativePath::new("foo/bar").unwrap();
        let last_access_time = Timestamp::UNIX_EPOCH;

        let classification = ArtifactClassification::IntermediateOnly;
        let entries = convert_artifact_metadata_to_sqlite_entries(
            path,
            &metadata,
            &last_access_time,
            classification,
        );

        let mut state =
            convert_sqlite_entries_to_materializer_state(entries, digest_config).unwrap();

        assert_eq!(state.len(), 1);

        let MaterializerStateEntry {
            path: result_path,
            metadata: result_metadata,
            last_access_time: result_last_access_time,
            classification: result_classification,
        } = state.pop().unwrap();

        assert_eq!(path, result_path);
        assert!(artifact_metadata_eq(&metadata, &result_metadata));
        assert_eq!(last_access_time, result_last_access_time);
        assert_eq!(classification, result_classification);
    }

    #[test]
    fn test_artifact_metadata_symlink_sqlite_entry_conversion_succeeds() {
        let digest_config = DigestConfig::testing_default();

        let symlink = new_symlink("foo/bar").unwrap();
        // We use `new_symlink` as a helper but it can technically create both Symlink and ExternalSymlink.
        // Verify that we have a Symlink here.
        assert_matches!(symlink, ActionDirectoryMember::Symlink(..));

        let metadata = DirectoryEntry::Leaf(symlink);
        let path = ProjectRelativePath::new("foo/bar").unwrap();
        let last_access_time = Timestamp::UNIX_EPOCH;

        let classification = ArtifactClassification::FinalOutput;
        let entries = convert_artifact_metadata_to_sqlite_entries(
            path,
            &metadata,
            &last_access_time,
            classification,
        );

        let mut state =
            convert_sqlite_entries_to_materializer_state(entries, digest_config).unwrap();

        assert_eq!(state.len(), 1);

        let MaterializerStateEntry {
            path: result_path,
            metadata: result_metadata,
            last_access_time: result_last_access_time,
            classification: result_classification,
        } = state.pop().unwrap();

        assert_eq!(path, result_path);
        assert!(artifact_metadata_eq(&metadata, &result_metadata));
        assert_eq!(last_access_time, result_last_access_time);
        assert_eq!(classification, result_classification);
    }

    #[test]
    fn test_artifact_metadata_external_symlink_sqlite_entry_conversion_succeeds() {
        let digest_config = DigestConfig::testing_default();

        let external_symlink = new_symlink(if cfg!(windows) {
            // Not sure if we actually support any external symlink on windows, but better
            // to just check anyways.
            "C:\\external\\symlink\\to\\artifact"
        } else {
            "/mnt/gvfs/third-party2/zstd/28def025ee38919d509596da7d09e7a5262cbf32/1.4.x/platform010/64091f4/share"
        }).unwrap();
        // We use `new_symlink` as a helper but it can technically create both Symlink and ExternalSymlink.
        // Verify that we have an ExternalSymlink here.
        assert_matches!(external_symlink, ActionDirectoryMember::ExternalSymlink(..));

        let metadata = DirectoryEntry::Leaf(external_symlink);
        let path = ProjectRelativePath::new("foo/bar").unwrap();
        let last_access_time = Timestamp::UNIX_EPOCH;

        let classification = ArtifactClassification::IntermediateOnly;
        let entries = convert_artifact_metadata_to_sqlite_entries(
            path,
            &metadata,
            &last_access_time,
            classification,
        );

        let mut state =
            convert_sqlite_entries_to_materializer_state(entries, digest_config).unwrap();

        assert_eq!(state.len(), 1);

        let MaterializerStateEntry {
            path: result_path,
            metadata: result_metadata,
            last_access_time: result_last_access_time,
            classification: result_classification,
        } = state.pop().unwrap();

        assert_eq!(path, result_path);
        assert!(artifact_metadata_eq(&metadata, &result_metadata));
        assert_eq!(last_access_time, result_last_access_time);
        assert_eq!(classification, result_classification);
    }

    #[test]
    fn test_delete_many() -> buck2_error::Result<()> {
        let conn = Connection::open_in_memory()?;

        let table = MaterializerStateSqliteTable::new(Arc::new(Mutex::new(conn)));
        table.create_table()?;

        // Sqlite has limits on how many variables you can use at once. Check we don't run into
        // those.
        let paths = (0..50000)
            .map(|i| ProjectRelativePathBuf::unchecked_new(format!("foo/{i}")))
            .collect();

        table.delete(paths)?;

        Ok(())
    }
}
