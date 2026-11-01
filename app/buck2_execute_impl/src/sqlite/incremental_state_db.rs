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
use buck2_core::soft_error_or_panic;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckDashMap;
use buck2_hash::IntentionallyStdHashMap;
use dupe::Dupe;
use jiff::Timestamp;

use crate::incremental_actions_helper::IncrementalPathMap;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;
use crate::sqlite::tables::incremental_state_table::IncrementalStateSqliteTable;

/// Hand-maintained schema version for the incremental state sqlite db.
/// PLEASE bump this version if you are making a breaking change to the
/// incremental state sqlite db schema!
///
/// If you forget to bump this version,
/// then you can fix forward by bumping the `buck2.sqlite_incremental_state_version`
/// buckconfig in the project root's .buckconfig.
pub const INCREMENTAL_DB_SCHEMA_VERSION: u64 = 0;

pub(crate) type IncrementalState = BuckDashMap<String, Arc<IncrementalPathMap>>;

pub struct IncrementalDbState {
    pub db: Option<IncrementalStateSqliteDb>,
    pub state: IncrementalState,
}

impl IncrementalDbState {
    pub fn db_disabled() -> Self {
        Self {
            db: None,
            state: BuckDashMap::default(),
        }
    }
}

impl IncrementalDbState {
    pub(crate) fn get(&self, key: &str) -> Option<Arc<IncrementalPathMap>> {
        self.state.get(key).map(|s| s.dupe())
    }

    pub(crate) fn insert(&self, key: &str, value: IncrementalPathMap) {
        match &self.db {
            Some(db) => {
                // (run_action_key, short_path) combination is the primary key so it should replace the current entry,
                // but let's delete it beforehand just in case so it doesn't keep on inserting more entries. We don't need
                // to do this without db because a map will automatically rewrite the current entry
                self.delete(key);

                self.state.insert(key.to_owned(), value.clone().into());
                if let Err(e) = db.incremental_state_table().insert(key.to_owned(), value) {
                    soft_error_or_panic!(
                        "insert_to_incremental_db",
                        buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Tier0,
                            "Failed to insert {} into sqlite db. {}",
                            key,
                            e
                        )
                    );
                };
            }
            None => {
                self.state.insert(key.to_owned(), value.clone().into());
            }
        }
    }

    pub(crate) fn delete(&self, key: &str) {
        if let Some(db) = &self.db
            && let Err(e) = db.incremental_state_table().delete(key.to_owned())
        {
            // This should be converted into a real error later, marking as a soft error for now as the row not existing
            // might show up as an error but doesn't actually matter in reality.
            soft_error_or_panic!(
                "delete_from_incremental_db",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Failed to remove incremental path map from sqlite db. {}",
                    e
                )
            );
        }

        self.state.remove(key);
    }
}

#[derive(Debug)]
pub struct IncrementalStateEntry {
    pub path: ProjectRelativePathBuf,
    pub metadata: ArtifactMetadata,
    pub last_access_time: Timestamp,
}

/// Concrete implementation of SqliteTable for IncrementalStateSqliteTable
impl SqliteTable for IncrementalStateSqliteTable {
    fn create_table(&self) -> buck2_error::Result<()> {
        IncrementalStateSqliteTable::create_table(self)
    }
}

/// DB that opens the sqlite connection to the incremental state db on disk and
/// holds all the sqlite tables we need for storing/querying incremental state
pub struct IncrementalStateSqliteDb {
    tables: SqliteTables<IncrementalStateSqliteTable>,
    /// A unique ID identifying this particular instance of the database. This will reset when we
    /// recreate it.
    identity: SqliteIdentity,
    lifetime_guard: Option<Arc<dyn Send + Sync>>,
}

impl SqliteDb for IncrementalStateSqliteDb {
    type StateType = IncrementalState;
    type TableType = IncrementalStateSqliteTable;

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
        let incremental_state_table = IncrementalStateSqliteTable::new(connection.dupe());
        Ok(SqliteTables::new(incremental_state_table, connection))
    }

    fn identity(&self) -> &SqliteIdentity {
        &self.identity
    }
}

impl IncrementalStateSqliteDb {
    /// Given path to the sqlite DB, attempts to read `IncrementalState` from the DB. If we encounter
    /// any failure along the way, such as if the DB path does not exist, the sqlite read fails,
    /// or the DB has a different set of versions than the versions this buck2 expects, we
    /// throw away the existing DB and initialize a new DB. Returns (1) the connected sqlite DB and
    /// (2) the `IncrementalState` if loading was successful or the load error.
    pub async fn initialize(
        incremental_state_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        current_instance_metadata: IntentionallyStdHashMap<String, String>,
        io_executor: Arc<dyn BlockingExecutor>,
        reject_identity: Option<&SqliteIdentity>,
    ) -> buck2_error::Result<IncrementalDbState> {
        let guard = io_executor.lifetime_guard();
        let mut state = io_executor
            .execute_io_inline(|| {
                Self::initialize_incremental_sqlite_db(
                    incremental_state_dir,
                    versions,
                    current_instance_metadata,
                    reject_identity,
                )
            })
            .await?;
        if let Some(db) = &mut state.db {
            db.lifetime_guard = guard;
        }
        Ok(state)
    }

    fn initialize_incremental_sqlite_db(
        incremental_state_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        current_instance_metadata: IntentionallyStdHashMap<String, String>,
        reject_identity: Option<&SqliteIdentity>,
    ) -> buck2_error::Result<IncrementalDbState> {
        let reject_identity = reject_identity.cloned();

        let (db, state) = match Self::get_sqlite_db(
            &incremental_state_dir,
            &versions,
            current_instance_metadata.clone(),
            reject_identity.as_ref(),
        ) {
            Ok(db) => match db.tables.domain_table.read_incremental_state() {
                Ok(state) => (db, Ok(state)),
                Err(e) => {
                    let db = Self::create_sqlite_db(
                        incremental_state_dir,
                        versions,
                        current_instance_metadata,
                    )?;
                    (db, Err(e))
                }
            },
            Err(e) => {
                let db = Self::create_sqlite_db(
                    incremental_state_dir,
                    versions,
                    current_instance_metadata,
                )?;
                (db, Err(e))
            }
        };

        match state {
            Ok(state) => Ok(IncrementalDbState {
                db: Some(db),
                state,
            }),
            Err(e) => {
                tracing::debug!(
                    "Failed to read/load incremental state. Build will continue as if the state is empty.  {}",
                    e
                );

                Ok(IncrementalDbState {
                    db: Some(db),
                    state: BuckDashMap::default(),
                })
            }
        }
    }

    pub(crate) fn incremental_state_table(&self) -> &IncrementalStateSqliteTable {
        &self.tables.domain_table
    }
}

#[allow(unused)] // Used by test modules
pub(crate) fn testing_incremental_state_sqlite_db(
    fs: &ProjectRoot,
    versions: IntentionallyStdHashMap<String, String>,
    metadata: IntentionallyStdHashMap<String, String>,
    reject_identity: Option<&SqliteIdentity>,
) -> buck2_error::Result<IncrementalDbState> {
    IncrementalStateSqliteDb::initialize_incremental_sqlite_db(
        fs.resolve(ProjectRelativePath::unchecked_new(
            "buck-out/v2/cache/incremental_state",
        )),
        versions,
        metadata,
        reject_identity,
    )
}

#[cfg(test)]
mod tests {
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_events::daemon_id::DaemonId;

    use super::*;
    use crate::incremental_actions_helper::IncrementalPathMap;

    #[test]
    fn test_initialize_incremental_sqlite_db() -> buck2_error::Result<()> {
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

        let fs = ProjectRootTemp::new()?;

        let run_action_key = "test_action".to_owned();
        let incremental_path_map = IncrementalPathMap::new(
            [(
                buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf::unchecked_new(
                    "test_file".to_owned(),
                ),
                ProjectRelativePathBuf::unchecked_new("buck-out/content_hash/test_file".to_owned()),
            )]
            .into_iter()
            .collect(),
        );
        let metadatas = testing_metadatas();

        let v0 = IntentionallyStdHashMap::from([("version".to_owned(), "0".to_owned())]);
        let v1 = IntentionallyStdHashMap::from([("version".to_owned(), "1".to_owned())]);

        // Initialize with non-existent DB (should create new DB)
        {
            let IncrementalDbState { db, state } = testing_incremental_state_sqlite_db(
                fs.path(),
                v0.clone(),
                metadatas[0].clone(),
                None,
            )
            .unwrap();
            let db = db.unwrap();
            assert!(state.is_empty());
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[0]);

            db.incremental_state_table()
                .insert(run_action_key.clone(), incremental_path_map.clone())
                .unwrap();
        }

        // Load existing DB with same version (should load successfully)
        {
            let IncrementalDbState { db, state } =
                testing_incremental_state_sqlite_db(fs.path(), v0, metadatas[1].clone(), None)
                    .unwrap();
            assert_eq!(state.len(), 1);
            assert!(state.contains_key(&run_action_key));
            let action_state = state.get(&run_action_key).unwrap();
            assert_eq!(action_state.iter().count(), 1);
            assert_metadata_matches(
                db.unwrap().tables.created_by_table.read_all()?,
                &metadatas[0],
            );
        }

        // Load with different version (should recreate DB)
        {
            let IncrementalDbState { db, state } = testing_incremental_state_sqlite_db(
                fs.path(),
                v1.clone(),
                metadatas[2].clone(),
                None,
            )
            .unwrap();
            let db = db.unwrap();
            assert!(state.is_empty());
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[2]);

            db.incremental_state_table()
                .insert(run_action_key.clone(), incremental_path_map.clone())
                .unwrap();
        }

        // Load existing DB with same version again (should load successfully)
        let identity = {
            let IncrementalDbState { db, state } = testing_incremental_state_sqlite_db(
                fs.path(),
                v1.clone(),
                metadatas[3].clone(),
                None,
            )
            .unwrap();
            let db = db.unwrap();
            assert_eq!(state.len(), 1);
            assert!(state.contains_key(&run_action_key));
            assert_metadata_matches(db.tables.created_by_table.read_all()?, &metadatas[2]);

            db.identity().clone()
        };

        // Reject specific identity (should recreate DB)
        {
            let IncrementalDbState { db, state } = testing_incremental_state_sqlite_db(
                fs.path(),
                v1,
                metadatas[4].clone(),
                Some(&identity),
            )
            .unwrap();
            assert!(state.is_empty());
            assert_metadata_matches(
                db.unwrap().tables.created_by_table.read_all()?,
                &metadatas[4],
            );
        }

        Ok(())
    }
}
