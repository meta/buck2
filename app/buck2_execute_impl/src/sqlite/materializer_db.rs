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

use buck2_common::sqlite::sqlite_db::SqliteDb;
use buck2_common::sqlite::sqlite_db::SqliteIdentity;
use buck2_common::sqlite::sqlite_db::SqliteTable;
use buck2_common::sqlite::sqlite_db::SqliteTables;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::IntentionallyStdHashMap;
use dupe::Dupe;
use jiff::Timestamp;

use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;
use crate::sqlite::tables::materializer_state_table::MaterializerStateSqliteTable;

/// Hand-maintained schema version for the materializer state sqlite db.
/// PLEASE bump this version if you are making a breaking change to the
/// materializer state sqlite db schema! If you forget to bump this version,
/// then you can fix forward by bumping the `buck2.sqlite_materializer_state_version`
/// buckconfig in the project root's .buckconfig.
pub const MATERIALIZER_DB_SCHEMA_VERSION: u64 = 9;

#[derive(Debug)]
pub struct MaterializerStateEntry {
    pub path: ProjectRelativePathBuf,
    pub metadata: ArtifactMetadata,
    pub last_access_time: Timestamp,
    pub classification: ArtifactClassification,
}

pub type MaterializerState = Vec<MaterializerStateEntry>;

/// Concrete implementation of SqliteTable for MaterializerStateSqliteTable
impl SqliteTable for MaterializerStateSqliteTable {
    fn create_table(&self) -> buck2_error::Result<()> {
        MaterializerStateSqliteTable::create_table(self)
    }
}

/// DB that opens the sqlite connection to the materializer state db on disk and
/// holds all the sqlite tables we need for storing/querying materializer state
pub struct MaterializerStateSqliteDb {
    tables: SqliteTables<MaterializerStateSqliteTable>,
    /// A unique ID identifying this particular instance of the database. This will reset when we
    /// recreate it.
    identity: SqliteIdentity,
    lifetime_guard: Option<Arc<dyn Send + Sync>>,
}

impl SqliteDb for MaterializerStateSqliteDb {
    type StateType = MaterializerState;
    type TableType = MaterializerStateSqliteTable;

    fn new(tables: SqliteTables<Self::TableType>) -> buck2_error::Result<Self> {
        let identity = tables.get_identity()?;
        Ok(Self {
            tables,
            identity,
            lifetime_guard: None,
        })
    }

    fn open_tables(path: &AbsNormPath) -> buck2_error::Result<SqliteTables<Self::TableType>> {
        let connection = SqliteTables::<Self::TableType>::create_connection(path)?;
        let materializer_state_table = MaterializerStateSqliteTable::new(connection.dupe());
        Ok(SqliteTables::new(materializer_state_table, connection))
    }

    fn identity(&self) -> &SqliteIdentity {
        &self.identity
    }
}

impl MaterializerStateSqliteDb {
    /// Given path to the sqlite DB, attempts to read `MaterializerState` from the DB. If we encounter
    /// any failure along the way, such as if the DB path does not exist, the sqlite read fails,
    /// or the DB has a different set of versions than the versions this buck2 expects, we
    /// throw away the existing DB and initialize a new DB. Returns (1) the connected sqlite DB and
    /// (2) the `MaterializerState` if loading was successful or the load error.
    /// The `Result<MaterializerState>` captures any failure encountered when attempting to load
    /// from the existing DB. These failures are expected if db doesn't exist or versions don't match.
    /// The outer `Result` captures any failure encountered when trying to delete the existing DB and
    /// create a new one.
    pub async fn initialize(
        materializer_state_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        current_instance_metadata: IntentionallyStdHashMap<String, String>,
        // Using `BlockingExecutor` out of convenience. This function should be called during startup
        // when there's not a lot of I/O so it shouldn't matter.
        io_executor: Arc<dyn BlockingExecutor>,
        digest_config: DigestConfig,
        reject_identity: Option<&SqliteIdentity>,
    ) -> buck2_error::Result<(Self, buck2_error::Result<MaterializerState>)> {
        let guard = io_executor.lifetime_guard();
        let (mut db, state) = io_executor
            .execute_io_inline(|| {
                Self::initialize_materializer_sqlite_db(
                    materializer_state_dir,
                    versions,
                    current_instance_metadata,
                    digest_config,
                    reject_identity,
                )
            })
            .await?;
        db.lifetime_guard = guard;
        Ok((db, state))
    }

    /// Internal implementation that handles digest config
    fn initialize_materializer_sqlite_db(
        materializer_state_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        current_instance_metadata: IntentionallyStdHashMap<String, String>,
        digest_config: DigestConfig,
        reject_identity: Option<&SqliteIdentity>,
    ) -> buck2_error::Result<(Self, buck2_error::Result<MaterializerState>)> {
        let reject_identity = reject_identity.cloned();

        match Self::get_sqlite_db(
            &materializer_state_dir,
            &versions,
            current_instance_metadata.clone(),
            reject_identity.as_ref(),
        ) {
            Ok(db) => {
                match db
                    .tables
                    .domain_table
                    .read_materializer_state(digest_config)
                {
                    Ok(state) => Ok((db, Ok(state))),
                    Err(e) => {
                        let state = Self::create_sqlite_db(
                            materializer_state_dir,
                            versions,
                            current_instance_metadata,
                        )?;
                        Ok((state, Err(e)))
                    }
                }
            }
            Err(e) => {
                let state = Self::create_sqlite_db(
                    materializer_state_dir,
                    versions,
                    current_instance_metadata,
                )?;
                Ok((state, Err(e)))
            }
        }
    }

    pub(crate) fn materializer_state_table(&mut self) -> &MaterializerStateSqliteTable {
        &self.tables.domain_table
    }
}

#[allow(unused)] // Used by test modules
pub(crate) fn testing_materializer_state_sqlite_db(
    fs: &ProjectRoot,
    versions: IntentionallyStdHashMap<String, String>,
    metadata: IntentionallyStdHashMap<String, String>,
    reject_identity: Option<&SqliteIdentity>,
) -> buck2_error::Result<(
    MaterializerStateSqliteDb,
    buck2_error::Result<MaterializerState>,
)> {
    MaterializerStateSqliteDb::initialize_materializer_sqlite_db(
        fs.resolve(ProjectRelativePath::unchecked_new(
            "buck-out/v2/cache/materializer_state",
        )),
        versions,
        metadata,
        DigestConfig::testing_default(),
        reject_identity,
    )
}

#[cfg(test)]
mod tests {

    use assert_matches::assert_matches;
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::Symlink;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_directory::directory::builder::DirectoryBuilder;
    use buck2_directory::directory::dashmap_directory_interner::DashMapDirectoryInterner;
    use buck2_directory::directory::entry::DirectoryEntry;
    use buck2_events::daemon_id::DaemonId;
    use buck2_execute::directory::ActionDirectoryMember;
    use buck2_execute::directory::new_symlink;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    use buck2_hash::BuckMutMap;
    use itertools::Itertools;
    use parking_lot::Mutex;
    use rusqlite::Connection;

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

    fn now_seconds() -> Timestamp {
        Timestamp::from_second(Timestamp::now().as_second()).unwrap()
    }

    #[test]
    fn test_materializer_state_sqlite_table() {
        let digest_config = DigestConfig::testing_default();

        let fs = ProjectRootTemp::new().unwrap();
        let connection = Connection::open(
            fs.path()
                .resolve(ProjectRelativePath::unchecked_new("test.db")),
        )
        .unwrap();
        let table = MaterializerStateSqliteTable::new(Arc::new(Mutex::new(connection)));

        table.create_table().unwrap();

        let shared_directory = {
            let mut builder = DirectoryBuilder::empty_non_exhaustive();
            // Create the following directory:
            // ├── foo - file
            // ├── bar - empty directory
            // └── baz
            //     └── qux -> ../foo
            {
                let digest =
                    TrackedFileDigest::from_content(b"hello", digest_config.cas_digest_config());
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
                let empty_directory = DirectoryEntry::Dir(DirectoryBuilder::empty_non_exhaustive());
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
        let file = ActionDirectoryMember::File(FileMetadata {
            digest: TrackedFileDigest::from_content(b"file", digest_config.cas_digest_config()),
            is_executable: false,
        });
        let symlink = new_symlink("foo/bar").unwrap();
        let external_symlink = new_symlink(if cfg!(windows) {
            // Not sure if we actually support any external symlink on windows, but better
            // to just check anyways.
            "C:\\external\\symlink\\to\\artifact"
        } else {
            "/mnt/gvfs/third-party2/zstd/28def025ee38919d509596da7d09e7a5262cbf32/1.4.x/platform010/64091f4/share"
        }).unwrap();
        // We use `new_symlink` as a helper but it can technically create both Symlink and ExternalSymlink.
        // Verify that we have proper symlink and external symlink.
        assert_matches!(symlink, ActionDirectoryMember::Symlink(..));
        assert_matches!(external_symlink, ActionDirectoryMember::ExternalSymlink(..));

        let artifacts = vec![
            MaterializerStateEntry {
                path: ProjectRelativePath::unchecked_new("a").to_owned(),
                metadata: DirectoryEntry::Dir(shared_directory),
                last_access_time: now_seconds(),
                classification: ArtifactClassification::FinalOutput,
            },
            MaterializerStateEntry {
                path: ProjectRelativePath::unchecked_new("b/c").to_owned(),
                metadata: DirectoryEntry::Leaf(file),
                last_access_time: now_seconds(),
                classification: ArtifactClassification::IntermediateOnly,
            },
            MaterializerStateEntry {
                path: ProjectRelativePath::unchecked_new("d").to_owned(),
                metadata: DirectoryEntry::Leaf(symlink),
                last_access_time: now_seconds(),
                classification: ArtifactClassification::FinalOutput,
            },
            MaterializerStateEntry {
                path: ProjectRelativePath::unchecked_new("e").to_owned(),
                metadata: DirectoryEntry::Leaf(external_symlink),
                last_access_time: now_seconds(),
                classification: ArtifactClassification::IntermediateOnly,
            },
        ];
        let mut artifacts: BuckMutMap<_, _> =
            artifacts.into_iter().map(|x| (x.path.clone(), x)).collect();

        for (path, entry) in artifacts.iter() {
            table
                .insert(
                    path,
                    &entry.metadata,
                    entry.last_access_time,
                    entry.classification,
                )
                .unwrap();
        }

        let check_materializer_state_expected =
            |state: &MaterializerState,
             artifacts: &BuckMutMap<ProjectRelativePathBuf, MaterializerStateEntry>| {
                let expected_values = artifacts.values().sorted_by_key(|x| x.path.as_str());
                let result_values = state.iter().sorted_by_key(|x| x.path.as_str());
                assert!(expected_values.eq(result_values));
            };

        let state = table.read_materializer_state(digest_config).unwrap();
        check_materializer_state_expected(&state, &artifacts);

        let paths_to_remove = vec![
            ProjectRelativePath::unchecked_new("d").to_owned(),
            ProjectRelativePath::unchecked_new("doesnt/exist").to_owned(),
        ];
        let rows_deleted = table.delete(paths_to_remove.clone()).unwrap();
        assert_eq!(rows_deleted, 1);

        for path in paths_to_remove.iter() {
            artifacts.remove(path);
        }
        let state = table.read_materializer_state(digest_config).unwrap();

        check_materializer_state_expected(&state, &artifacts);
    }

    impl PartialEq for MaterializerStateEntry {
        fn eq(&self, other: &Self) -> bool {
            self.path == other.path
                && self.last_access_time == other.last_access_time
                && self.classification == other.classification
                && artifact_metadata_eq(&self.metadata, &other.metadata)
        }
    }

    #[test]
    fn test_initialize_sqlite_db() -> buck2_error::Result<()> {
        fn testing_metadatas() -> Vec<IntentionallyStdHashMap<String, String>> {
            let metadata = buck2_events::metadata::collect(&DaemonId::new());
            let mut metadatas = vec![metadata; 5];
            for (i, metadata) in metadatas.iter_mut().enumerate() {
                metadata.insert("version".to_owned(), i.to_string());
            }
            metadatas
        }

        fn assert_metadata_matches(
            mut have: IntentionallyStdHashMap<String, String>,
            want: &IntentionallyStdHashMap<String, String>,
        ) {
            // Remove the key we inject (and check it's there).
            have.remove("timestamp_on_initialization").unwrap();
            assert_eq!(have, *want);
        }

        let digest_config = DigestConfig::testing_default();

        let fs = ProjectRootTemp::new()?;

        let path = ProjectRelativePath::unchecked_new("foo").to_owned();
        let artifact_metadata = DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
            digest: TrackedFileDigest::from_content(b"file", digest_config.cas_digest_config()),
            is_executable: false,
        }));
        let timestamp = now_seconds();
        let metadatas = testing_metadatas();

        let v0 = IntentionallyStdHashMap::from([("version".to_owned(), "0".to_owned())]);
        let v1 = IntentionallyStdHashMap::from([("version".to_owned(), "1".to_owned())]);

        {
            let (mut db, loaded_state) = testing_materializer_state_sqlite_db(
                fs.path(),
                v0.clone(),
                metadatas[0].clone(),
                None,
            )
            .unwrap();
            assert!(loaded_state.is_err());
            if let Err(e) = loaded_state {
                assert!(e.category_key().ends_with("PathDoesNotExist"));
            }
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[0]);

            db.materializer_state_table()
                .insert(
                    &path,
                    &artifact_metadata,
                    timestamp,
                    ArtifactClassification::FinalOutput,
                )
                .unwrap();
        }

        {
            let (db, loaded_state) =
                testing_materializer_state_sqlite_db(fs.path(), v0, metadatas[1].clone(), None)
                    .unwrap();
            assert_matches!(
                loaded_state,
                Ok(v) => {
                    assert_eq!(v, vec![MaterializerStateEntry {path: path.clone(), metadata: artifact_metadata.clone(), last_access_time: timestamp, classification: ArtifactClassification::FinalOutput}]);
                }
            );
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[0]);
        }

        {
            let (mut db, loaded_state) = testing_materializer_state_sqlite_db(
                fs.path(),
                v1.clone(),
                metadatas[2].clone(),
                None,
            )
            .unwrap();
            assert!(loaded_state.is_err());
            if let Err(e) = loaded_state {
                assert!(e.category_key().ends_with("VersionMismatch"));
            }
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[2]);

            db.materializer_state_table()
                .insert(
                    &path,
                    &artifact_metadata,
                    timestamp,
                    ArtifactClassification::IntermediateOnly,
                )
                .unwrap();
        }

        let identity = {
            let (db, loaded_state) = testing_materializer_state_sqlite_db(
                fs.path(),
                v1.clone(),
                metadatas[3].clone(),
                None,
            )
            .unwrap();
            assert_matches!(
                loaded_state,
                Ok(v) => {
                    assert_eq!(v, vec![MaterializerStateEntry { path, metadata: artifact_metadata, last_access_time: timestamp, classification: ArtifactClassification::IntermediateOnly }]);
                }
            );
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[2]);

            db.identity().clone()
        };

        {
            let (db, loaded_state) = testing_materializer_state_sqlite_db(
                fs.path(),
                v1,
                metadatas[4].clone(),
                Some(&identity),
            )
            .unwrap();
            assert!(loaded_state.is_err());
            if let Err(e) = loaded_state {
                assert!(e.category_key().ends_with("RejectedIdentity"));
            }
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[4]);
        }

        Ok(())
    }
}
