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

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_hash::IntentionallyStdHashMap;
use derive_more::Display;
use derive_more::From;
use dupe::Dupe;
use parking_lot::Mutex;
use rusqlite::Connection;

use crate::sqlite::key_value_table::KeyValueSqliteTable;

#[derive(Display, Allocative, Clone, From, PartialEq, Eq, Debug)]
pub struct SqliteIdentity(String);

const IDENTITY_KEY: &str = "timestamp_on_initialization";

#[derive(buck2_error::Error, Debug, PartialEq, Eq)]
#[buck2(tag = Input)]
enum SqliteDbError {
    #[error("Path {} does not exist", .0)]
    PathDoesNotExist(AbsNormPathBuf),

    #[error("Expected versions {:?}. Found versions {:?} in sqlite db at {}", .expected, .found, .path)]
    VersionMismatch {
        expected: IntentionallyStdHashMap<String, String>,
        found: IntentionallyStdHashMap<String, String>,
        path: AbsNormPathBuf,
    },

    #[error("Sqlite identity was rejected: {}", .identity)]
    RejectedIdentity { identity: SqliteIdentity },
}

/// Trait for specific SQLite table implementations
pub trait SqliteTable {
    /// Create the table schema
    fn create_table(&self) -> buck2_error::Result<()>;
}

/// Trait for SQLite database implementations that can be generalized
pub trait SqliteDb {
    type StateType;
    type TableType: SqliteTable;

    /// Get the database filename
    fn db_filename() -> &'static str {
        "db.sqlite"
    }

    /// Create a new instance from tables
    fn new(tables: SqliteTables<Self::TableType>) -> buck2_error::Result<Self>
    where
        Self: Sized;

    /// Open and configure the database tables
    fn open_tables(path: &AbsNormPath) -> buck2_error::Result<SqliteTables<Self::TableType>>;

    /// Get the database identity
    fn identity(&self) -> &SqliteIdentity;

    fn get_sqlite_db(
        db_dir: &AbsNormPathBuf,
        versions: &IntentionallyStdHashMap<String, String>,
        mut current_instance_metadata: IntentionallyStdHashMap<String, String>,
        reject_identity: Option<&SqliteIdentity>,
    ) -> buck2_error::Result<Self>
    where
        Self: std::marker::Sized,
    {
        let timestamp_on_initialization = jiff::Timestamp::now().to_string();
        current_instance_metadata.insert(IDENTITY_KEY.to_owned(), timestamp_on_initialization);

        let db_path = db_dir.join(FileName::unchecked_new(Self::db_filename()));

        // try reading the existing db, if it exists.
        if !db_path.exists() {
            Err(SqliteDbError::PathDoesNotExist(db_path.clone()))?
        }

        let tables = Self::open_tables(&db_path)?;

        // First check that versions match
        let read_versions = tables.versions_table.read_all()?;
        if read_versions != *versions {
            Err(SqliteDbError::VersionMismatch {
                expected: versions.clone(),
                found: read_versions,
                path: db_path.clone(),
            })?;
        }

        let db = Self::new(tables)?;

        if let Some(reject_identity) = reject_identity {
            if db.identity() == reject_identity {
                Err(SqliteDbError::RejectedIdentity {
                    identity: db.identity().clone(),
                })?;
            }
        }

        Ok(db)
    }

    // Initialize a new db from scratch.
    fn create_sqlite_db(
        db_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        mut current_instance_metadata: IntentionallyStdHashMap<String, String>,
    ) -> buck2_error::Result<Self>
    where
        Self: std::marker::Sized,
    {
        let timestamp_on_initialization = jiff::Timestamp::now().to_string();
        current_instance_metadata.insert(IDENTITY_KEY.to_owned(), timestamp_on_initialization);

        let db_path = db_dir.join(FileName::unchecked_new(Self::db_filename()));

        // Delete the existing db directory and create a new one.
        // We delete the entire directory and not just the db file because sqlite
        // can leave behind other files.
        if db_dir.exists() {
            fs_util::remove_dir_all(&db_dir).categorize_internal()?;
        }
        fs_util::create_dir_all(&db_dir)?;

        // Initialize a new db
        let tables = Self::open_tables(&db_path)?;
        tables.create_all_tables()?;
        tables.versions_table.insert_all(versions)?;
        // Update "created_by" table
        tables
            .created_by_table
            .insert_all(current_instance_metadata)?;

        Self::new(tables)
    }
}

/// Common SQLite table structure that all implementations will have
pub struct SqliteTables<T: SqliteTable> {
    /// Domain-specific table (e.g., materializer state, build cache, etc.)
    pub domain_table: T,
    /// Table for holding any metadata used to check version match. When loading
    /// from an existing db, we check if the versions from this table match the
    /// versions this buck2 binary expects. If the versions don't match, we throw
    /// away the entire db and initialize a new one. If versions do match, then
    /// we try to read all state from the domain table.
    pub versions_table: KeyValueSqliteTable,
    /// Table for logging metadata associated with the buck2 that created the db.
    pub created_by_table: KeyValueSqliteTable,
}

impl<T: SqliteTable> SqliteTables<T> {
    /// Create a new SqliteTables instance
    pub fn new(domain_table: T, connection: Arc<Mutex<Connection>>) -> Self {
        let versions_table = KeyValueSqliteTable::new("versions".to_owned(), connection.dupe());
        let created_by_table = KeyValueSqliteTable::new("created_by".to_owned(), connection.dupe());

        Self {
            domain_table,
            versions_table,
            created_by_table,
        }
    }

    /// Get the identity from the created_by table
    pub fn get_identity(&self) -> buck2_error::Result<SqliteIdentity> {
        let identity = self
            .created_by_table
            .get(IDENTITY_KEY)
            .buck_error_context("Error reading creation metadata")?
            .map(SqliteIdentity)
            .with_internal_error(|| format!("Identity key is missing in db: `{IDENTITY_KEY}`"))?;

        Ok(identity)
    }

    /// Create all tables in the database
    pub fn create_all_tables(&self) -> buck2_error::Result<()> {
        self.domain_table.create_table()?;
        self.versions_table.create_table()?;
        self.created_by_table.create_table()?;
        Ok(())
    }

    /// Create a connection with standard SQLite configuration
    pub fn create_connection(path: &AbsNormPath) -> buck2_error::Result<Arc<Mutex<Connection>>> {
        let connection = Connection::open(path)?;
        // TODO: make this work on Windows too
        if cfg!(unix) {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }

        // Setting synchronous to anything but OFF prevents data corruption in case of power loss,
        // but for many use cases, we are rather happy to run the risk of data
        // corruption (which we recover from by just dropping the state and pretending we have
        // none), rather than running a `fsync` at any point during a build, which tends to be
        // *very* slow, because if we do a `fsync`, that will tend
        // to occur after a lot of writes have been done, which means a lot of data needs syncing!
        //
        // Note that upon power loss there isn't really a guarantee of ordering of things written
        // across different files anyway, so we always run some risk of having our state be
        // incorrect if that happens, unless we fsync after every single write, but, that's
        // definitely not an option.
        //
        // This problem notably manifests itself when routing writes through SQLite
        // on a benchmark build. This causes the set of operations done by SQLite
        // to exceed the WAL max size (which is 1000 pages that are 4KB each,
        // so about 4MB of data), which causes SQLite to write the WAL to the database file, which
        // is the only circumstance under which SQLite does a `fsync` when WAL is enabled, and then
        // we completely stall I/O for a little while.
        connection.pragma_update(None, "synchronous", "OFF")?;

        Ok(Arc::new(Mutex::new(connection)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_hash::IntentionallyStdHashMap;
    use dupe::Dupe;
    use parking_lot::Mutex;
    use rusqlite::Connection;

    use super::*;

    /// Test state type - simple vector of strings
    type TestState = Vec<String>;

    struct TestSqliteTable {
        connection: Arc<Mutex<Connection>>,
    }

    impl TestSqliteTable {
        fn new(connection: Arc<Mutex<Connection>>) -> Self {
            Self { connection }
        }

        fn insert(&self, value: &str) -> buck2_error::Result<()> {
            let sql = "INSERT INTO test_data (value) VALUES (?1)";
            self.connection
                .lock()
                .execute(sql, [value])
                .buck_error_context("Failed to insert test data")?;
            Ok(())
        }

        fn read_all(&self) -> buck2_error::Result<TestState> {
            let sql = "SELECT value FROM test_data ORDER BY id";
            let connection = self.connection.lock();
            let mut stmt = connection.prepare(sql)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }

    impl SqliteTable for TestSqliteTable {
        fn create_table(&self) -> buck2_error::Result<()> {
            let sql = "CREATE TABLE test_data (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                value TEXT NOT NULL
            )";
            self.connection
                .lock()
                .execute(sql, [])
                .buck_error_context("Failed to create test table")?;
            Ok(())
        }
    }

    struct TestSqliteDb {
        tables: SqliteTables<TestSqliteTable>,
        identity: SqliteIdentity,
    }

    impl SqliteDb for TestSqliteDb {
        type StateType = TestState;
        type TableType = TestSqliteTable;

        fn db_filename() -> &'static str {
            "test_db.sqlite"
        }

        fn new(tables: SqliteTables<Self::TableType>) -> buck2_error::Result<Self> {
            let identity = tables.get_identity()?;
            Ok(Self { tables, identity })
        }

        fn open_tables(path: &AbsNormPath) -> buck2_error::Result<SqliteTables<Self::TableType>> {
            let connection = SqliteTables::<Self::TableType>::create_connection(path)?;
            let test_table = TestSqliteTable::new(connection.dupe());
            Ok(SqliteTables::new(test_table, connection))
        }

        fn identity(&self) -> &SqliteIdentity {
            &self.identity
        }
    }

    impl TestSqliteDb {
        fn insert_test_data(&mut self, value: &str) -> buck2_error::Result<()> {
            self.tables.domain_table.insert(value)
        }
    }

    fn create_test_versions() -> IntentionallyStdHashMap<String, String> {
        IntentionallyStdHashMap::from([
            ("schema_version".to_owned(), "1".to_owned()),
            ("app_version".to_owned(), "test".to_owned()),
        ])
    }

    fn create_test_metadata() -> IntentionallyStdHashMap<String, String> {
        IntentionallyStdHashMap::from([
            ("created_by".to_owned(), "test_suite".to_owned()),
            ("test_run".to_owned(), "true".to_owned()),
        ])
    }

    #[test]
    fn test_create_new_sqlite_db() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let db_dir = fs
            .path()
            .resolve(ProjectRelativePath::unchecked_new("test_db_dir"));

        let versions = create_test_versions();
        let metadata = create_test_metadata();

        // Create a new database
        let mut db = TestSqliteDb::create_sqlite_db(db_dir, versions.clone(), metadata.clone())?;

        // Verify identity was set
        assert!(!db.identity().to_string().is_empty());

        // Verify tables exist and can be used
        db.insert_test_data("test_value_1")?;
        db.insert_test_data("test_value_2")?;

        let state = db.tables.domain_table.read_all()?;
        assert_eq!(state, vec!["test_value_1", "test_value_2"]);

        // Verify version table was populated
        let stored_versions = db.tables.versions_table.read_all()?;
        assert_eq!(stored_versions, versions);

        // Verify created_by table was populated (including auto-added timestamp)
        let stored_metadata = db.tables.created_by_table.read_all()?;
        assert!(stored_metadata.get("created_by").unwrap() == "test_suite");
        assert!(stored_metadata.get("test_run").unwrap() == "true");
        assert!(stored_metadata.contains_key("timestamp_on_initialization"));

        Ok(())
    }

    #[test]
    fn test_load_existing_sqlite_db() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let db_dir = fs
            .path()
            .resolve(ProjectRelativePath::unchecked_new("test_db_dir"));

        let versions = create_test_versions();
        let metadata = create_test_metadata();

        // Create a database and add some data
        let mut db =
            TestSqliteDb::create_sqlite_db(db_dir.clone(), versions.clone(), metadata.clone())?;
        db.insert_test_data("persistent_value_1")?;
        db.insert_test_data("persistent_value_2")?;

        // Load the existing database
        let mut loaded_db = TestSqliteDb::get_sqlite_db(&db_dir, &versions, metadata, None)?;

        // Verify data persisted
        let state = loaded_db.tables.domain_table.read_all()?;
        assert_eq!(state, vec!["persistent_value_1", "persistent_value_2"]);

        // Verify we can still add data
        loaded_db.insert_test_data("new_value")?;
        let updated_state = loaded_db.tables.domain_table.read_all()?;
        assert_eq!(
            updated_state,
            vec!["persistent_value_1", "persistent_value_2", "new_value"]
        );

        Ok(())
    }

    #[test]
    fn test_version_mismatch_error() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let db_dir = fs
            .path()
            .resolve(ProjectRelativePath::unchecked_new("test_db_dir"));

        let v1_versions =
            IntentionallyStdHashMap::from([("schema_version".to_owned(), "1".to_owned())]);
        let v2_versions =
            IntentionallyStdHashMap::from([("schema_version".to_owned(), "2".to_owned())]);
        let metadata = create_test_metadata();

        // Create database with version 1
        let _db = TestSqliteDb::create_sqlite_db(db_dir.clone(), v1_versions, metadata.clone())?;

        // Try to load with version 2
        let result = TestSqliteDb::get_sqlite_db(&db_dir, &v2_versions, metadata, None);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(
                e.to_string().contains("Expected versions {\"schema_version\": \"2\"}. Found versions {\"schema_version\": \"1\"} in sqlite db")
            );
        }

        Ok(())
    }

    #[test]
    fn test_rejected_identity() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let db_dir = fs
            .path()
            .resolve(ProjectRelativePath::unchecked_new("test_db_dir"));

        let versions = create_test_versions();
        let metadata = create_test_metadata();

        // Create database and get its identity
        let db =
            TestSqliteDb::create_sqlite_db(db_dir.clone(), versions.clone(), metadata.clone())?;
        let identity = db.identity().clone();

        // Try to load the same database but reject its identity
        let result = TestSqliteDb::get_sqlite_db(&db_dir, &versions, metadata, Some(&identity));
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(e.to_string().contains("Sqlite identity was rejected"));
        }

        Ok(())
    }

    #[test]
    fn test_path_does_not_exist_error() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let nonexistent_db_dir = fs
            .path()
            .resolve(ProjectRelativePath::unchecked_new("nonexistent_db_dir"));

        let versions = create_test_versions();
        let metadata = create_test_metadata();

        // Try to load from non-existent path
        let result = TestSqliteDb::get_sqlite_db(&nonexistent_db_dir, &versions, metadata, None);
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(e.to_string().contains("does not exist"));
        }

        Ok(())
    }

    #[test]
    fn test_sqlite_tables_functionality() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        let db_path = fs
            .path()
            .resolve(ProjectRelativePath::unchecked_new("test.db"));

        // Create tables manually
        let connection = SqliteTables::<TestSqliteTable>::create_connection(&db_path)?;
        let test_table = TestSqliteTable::new(connection.dupe());
        let tables = SqliteTables::new(test_table, connection);

        // Test table creation
        tables.create_all_tables()?;

        // Test version table functionality
        let test_versions = IntentionallyStdHashMap::from([
            ("key1".to_owned(), "value1".to_owned()),
            ("key2".to_owned(), "value2".to_owned()),
        ]);
        tables.versions_table.insert_all(test_versions.clone())?;
        let read_versions = tables.versions_table.read_all()?;
        assert_eq!(read_versions, test_versions);

        // Test created_by table functionality
        let test_metadata = IntentionallyStdHashMap::from([
            (
                "timestamp_on_initialization".to_owned(),
                "test_timestamp".to_owned(),
            ),
            ("meta_key".to_owned(), "meta_value".to_owned()),
        ]);
        tables.created_by_table.insert_all(test_metadata)?;

        // Test identity retrieval
        let identity = tables.get_identity()?;
        assert_eq!(identity.to_string(), "test_timestamp");

        // Test domain table functionality
        tables.domain_table.insert("domain_value")?;
        let domain_data = tables.domain_table.read_all()?;
        assert_eq!(domain_data, vec!["domain_value"]);

        Ok(())
    }
}
