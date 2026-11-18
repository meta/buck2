/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The incremental records' tables: `incremental_actions`, one row per action with its record's
//! kind, and `incremental_outputs`, one row per recorded output in the stored-output columns
//! (`stored_output_row`), a leaf in full and a directory as its fingerprint.

use std::sync::Arc;
use std::sync::LazyLock;

use buck2_directory::directory::entry::DirectoryEntry;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_execute::dep_file_state::StoredOutputValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::incremental_state::IncrementalRecord;
use buck2_execute::incremental_state::RecordedOutput;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use parking_lot::Mutex;
use rusqlite::Connection;

use crate::sqlite::incremental_state_db::IncrementalState;
use crate::sqlite::tables::stored_output_row::COLUMN_NAMES;
use crate::sqlite::tables::stored_output_row::StoredOutputColumns;

const ACTIONS_TABLE_NAME: &str = "incremental_actions";
const OUTPUTS_TABLE_NAME: &str = "incremental_outputs";

const KIND_OUTPUTS: &str = "outputs";
const KIND_LEFTOVERS: &str = "leftovers";

pub struct IncrementalStateSqliteTable {
    connection: Arc<Mutex<Connection>>,
}

impl IncrementalStateSqliteTable {
    pub fn new(connection: Arc<Mutex<Connection>>) -> Self {
        Self { connection }
    }

    pub(crate) fn create_table(&self) -> buck2_error::Result<()> {
        let statements = [
            format!(
                "CREATE TABLE {ACTIONS_TABLE_NAME} (
                    run_action_key          TEXT NOT NULL PRIMARY KEY,
                    kind                    TEXT CHECK(kind IN ('{KIND_OUTPUTS}','{KIND_LEFTOVERS}')) NOT NULL
                )"
            ),
            format!(
                "CREATE TABLE {OUTPUTS_TABLE_NAME} (
                    run_action_key          TEXT NOT NULL,
                    short_path              TEXT NOT NULL,
                    artifact_type           TEXT NOT NULL,
                    entry_size              INTEGER NULL DEFAULT NULL,
                    entry_hash              BLOB NULL DEFAULT NULL,
                    entry_hash_kind         INTEGER NULL DEFAULT NULL,
                    file_is_executable      INTEGER NULL DEFAULT NULL,
                    symlink_target          TEXT NULL DEFAULT NULL,
                    symlink_remaining_path  TEXT NULL DEFAULT NULL,
                    PRIMARY KEY             (run_action_key, short_path)
                ) WITHOUT ROWID"
            ),
        ];
        let connection = self.connection.lock();
        for sql in statements {
            tracing::trace!(sql = %*sql, "creating table");
            connection
                .execute(&sql, [])
                .with_buck_error_context(|| format!("creating sqlite table: `{sql}`"))?;
        }
        Ok(())
    }

    /// Replaces the action's rows with `record`'s.
    pub(crate) fn insert(
        &self,
        run_action_key: &str,
        record: &IncrementalRecord,
    ) -> buck2_error::Result<()> {
        static ACTION_SQL: LazyLock<String> = LazyLock::new(|| {
            format!("INSERT INTO {ACTIONS_TABLE_NAME} (run_action_key, kind) VALUES (?1, ?2)")
        });
        static OUTPUT_SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "INSERT INTO {OUTPUTS_TABLE_NAME} (run_action_key, short_path, {COLUMN_NAMES}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            )
        });
        let mut conn = self.connection.lock();
        let tx = conn.transaction()?;
        delete_in_tx(&tx, run_action_key)?;
        let kind = match record {
            IncrementalRecord::Outputs(_) => KIND_OUTPUTS,
            IncrementalRecord::Leftovers => KIND_LEFTOVERS,
        };
        tx.prepare_cached(&ACTION_SQL)?
            .execute(rusqlite::params![run_action_key, kind])
            .with_buck_error_context(|| format!("inserting into {ACTIONS_TABLE_NAME}"))?;
        if let IncrementalRecord::Outputs(outputs) = record {
            for (short_path, output) in outputs.iter() {
                let columns = StoredOutputColumns::encode(&stored_output_value(output))?;
                tx.prepare_cached(&OUTPUT_SQL)?
                    .execute(rusqlite::params![
                        run_action_key,
                        short_path.as_str(),
                        columns.artifact_type,
                        columns.entry_size,
                        columns.entry_hash,
                        columns.entry_hash_kind,
                        columns.file_is_executable,
                        columns.symlink_target,
                        columns.symlink_remaining_path,
                    ])
                    .with_buck_error_context(|| format!("inserting into {OUTPUTS_TABLE_NAME}"))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn read_incremental_state(
        &self,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<IncrementalState> {
        static ACTIONS_SQL: LazyLock<String> =
            LazyLock::new(|| format!("SELECT run_action_key, kind FROM {ACTIONS_TABLE_NAME}"));
        static OUTPUTS_SQL: LazyLock<String> = LazyLock::new(|| {
            format!("SELECT run_action_key, short_path, {COLUMN_NAMES} FROM {OUTPUTS_TABLE_NAME}")
        });
        let connection = self.connection.lock();

        let actions: Vec<(String, String)> = connection
            .prepare(&ACTIONS_SQL)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()
            .with_buck_error_context(|| {
                format!("reading from sqlite table {ACTIONS_TABLE_NAME}")
            })?;

        let mut outputs_by_action: BuckMutMap<
            String,
            Vec<(ForwardRelativePathBuf, RecordedOutput)>,
        > = BuckMutMap::default();
        let output_rows: Vec<(String, String, StoredOutputColumns)> = connection
            .prepare(&OUTPUTS_SQL)?
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, StoredOutputColumns::read(row, 2)?))
            })?
            .collect::<Result<_, _>>()
            .with_buck_error_context(|| {
                format!("reading from sqlite table {OUTPUTS_TABLE_NAME}")
            })?;
        for (run_action_key, short_path, columns) in output_rows {
            let short_path = ForwardRelativePathBuf::new(short_path)
                .buck_error_context("Invalid `short_path` in the incremental state db")?;
            let output = recorded_output(columns.decode(digest_config)?);
            outputs_by_action
                .entry(run_action_key)
                .or_default()
                .push((short_path, output));
        }

        let state = IncrementalState::default();
        for (run_action_key, kind) in actions {
            let record = match kind.as_str() {
                KIND_OUTPUTS => IncrementalRecord::Outputs(
                    outputs_by_action
                        .remove(&run_action_key)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                ),
                KIND_LEFTOVERS => IncrementalRecord::Leftovers,
                other => {
                    return Err(internal_error!(
                        "Unknown incremental record kind `{other}` for `{run_action_key}`"
                    ));
                }
            };
            state.insert(run_action_key, Arc::new(record));
        }
        Ok(state)
    }
}

fn delete_in_tx(tx: &rusqlite::Transaction, run_action_key: &str) -> buck2_error::Result<()> {
    static ACTION_SQL: LazyLock<String> =
        LazyLock::new(|| format!("DELETE FROM {ACTIONS_TABLE_NAME} WHERE run_action_key = ?1"));
    static OUTPUTS_SQL: LazyLock<String> =
        LazyLock::new(|| format!("DELETE FROM {OUTPUTS_TABLE_NAME} WHERE run_action_key = ?1"));
    tx.prepare_cached(&ACTION_SQL)?
        .execute(rusqlite::params![run_action_key])
        .with_buck_error_context(|| format!("deleting from {ACTIONS_TABLE_NAME}"))?;
    tx.prepare_cached(&OUTPUTS_SQL)?
        .execute(rusqlite::params![run_action_key])
        .with_buck_error_context(|| format!("deleting from {OUTPUTS_TABLE_NAME}"))?;
    Ok(())
}

/// The persisted form: a directory, whole or already a fingerprint, as its fingerprint.
fn stored_output_value(output: &RecordedOutput) -> StoredOutputValue {
    match output {
        RecordedOutput::Value(value) => match value.entry() {
            DirectoryEntry::Dir(dir) => {
                StoredOutputValue::Directory(dir.fingerprint().data().dupe())
            }
            DirectoryEntry::Leaf(_) => StoredOutputValue::Leaf(value.dupe()),
        },
        RecordedOutput::DirectoryFingerprint(fingerprint) => {
            StoredOutputValue::Directory(fingerprint.dupe())
        }
    }
}

fn recorded_output(stored: StoredOutputValue) -> RecordedOutput {
    match stored {
        StoredOutputValue::Leaf(value) => RecordedOutput::Value(value),
        StoredOutputValue::Directory(fingerprint) => {
            RecordedOutput::DirectoryFingerprint(fingerprint)
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_execute::artifact_value::ArtifactValue;
    use buck2_execute::directory::ActionDirectoryBuilder;
    use buck2_execute::directory::ActionDirectoryMember;
    use buck2_execute::directory::INTERNER;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;

    use super::*;

    fn table() -> IncrementalStateSqliteTable {
        let conn = Connection::open_in_memory().unwrap();
        let table = IncrementalStateSqliteTable::new(Arc::new(Mutex::new(conn)));
        table.create_table().unwrap();
        table
    }

    fn file(content: &[u8]) -> ArtifactValue {
        ArtifactValue::file(FileMetadata {
            digest: TrackedFileDigest::from_content(
                content,
                DigestConfig::testing_default().cas_digest_config(),
            ),
            is_executable: false,
        })
    }

    fn dir() -> ArtifactValue {
        let digest_config = DigestConfig::testing_default();
        let mut builder = ActionDirectoryBuilder::empty_exhaustive();
        let DirectoryEntry::Leaf(ActionDirectoryMember::File(f)) = file(b"inside").entry().dupe()
        else {
            unreachable!()
        };
        builder
            .insert(
                ForwardRelativePath::unchecked_new("f"),
                DirectoryEntry::Leaf(ActionDirectoryMember::File(f)),
            )
            .unwrap();
        ArtifactValue::dir(
            builder
                .fingerprint(digest_config.as_directory_serializer())
                .shared(&*INTERNER),
        )
    }

    fn outputs(outputs: &[(&str, RecordedOutput)]) -> IncrementalRecord {
        IncrementalRecord::Outputs(
            outputs
                .iter()
                .map(|(path, output)| {
                    (
                        ForwardRelativePathBuf::unchecked_new((*path).to_owned()),
                        output.dupe(),
                    )
                })
                .collect(),
        )
    }

    fn read(table: &IncrementalStateSqliteTable) -> IncrementalState {
        table
            .read_incremental_state(DigestConfig::testing_default())
            .unwrap()
    }

    #[test]
    fn test_outputs_round_trip() {
        let table = table();
        let leaf = file(b"leaf");
        let dir = dir();
        table
            .insert(
                "action",
                &outputs(&[
                    ("leaf", RecordedOutput::Value(leaf.dupe())),
                    ("dir", RecordedOutput::Value(dir.dupe())),
                ]),
            )
            .unwrap();

        let state = read(&table);
        assert_eq!(state.len(), 1);
        let IncrementalRecord::Outputs(recorded) = &**state.get("action").unwrap() else {
            panic!("expected outputs");
        };
        assert_eq!(recorded.len(), 2);
        match recorded
            .get(ForwardRelativePath::unchecked_new("leaf"))
            .unwrap()
        {
            RecordedOutput::Value(value) => assert_eq!(value.entry(), leaf.entry()),
            other => panic!("expected the leaf whole, got {other:?}"),
        }
        match recorded
            .get(ForwardRelativePath::unchecked_new("dir"))
            .unwrap()
        {
            RecordedOutput::DirectoryFingerprint(fingerprint) => {
                let DirectoryEntry::Dir(dir) = dir.entry() else {
                    unreachable!()
                };
                assert_eq!(fingerprint, dir.fingerprint().data());
            }
            other => panic!("expected the directory's fingerprint, got {other:?}"),
        }
    }

    #[test]
    fn test_leftovers_round_trip() {
        let table = table();
        table
            .insert("action", &IncrementalRecord::Leftovers)
            .unwrap();
        let state = read(&table);
        assert!(matches!(
            &**state.get("action").unwrap(),
            IncrementalRecord::Leftovers
        ));
    }

    #[test]
    fn test_insert_replaces_the_action() {
        let table = table();
        table
            .insert(
                "action",
                &outputs(&[
                    ("a", RecordedOutput::Value(file(b"a"))),
                    ("b", RecordedOutput::Value(file(b"b"))),
                ]),
            )
            .unwrap();
        table
            .insert(
                "other",
                &outputs(&[("c", RecordedOutput::Value(file(b"c")))]),
            )
            .unwrap();

        // A record with fewer outputs leaves none of the old rows behind.
        table
            .insert(
                "action",
                &outputs(&[("a", RecordedOutput::Value(file(b"a2")))]),
            )
            .unwrap();
        let state = read(&table);
        let IncrementalRecord::Outputs(recorded) = &**state.get("action").unwrap() else {
            panic!("expected outputs");
        };
        assert_eq!(recorded.len(), 1);
        assert!(
            recorded
                .get(ForwardRelativePath::unchecked_new("a"))
                .is_some()
        );

        // And leftovers leave no output rows at all.
        table
            .insert("action", &IncrementalRecord::Leftovers)
            .unwrap();
        let state = read(&table);
        assert!(matches!(
            &**state.get("action").unwrap(),
            IncrementalRecord::Leftovers
        ));
        let IncrementalRecord::Outputs(other) = &**state.get("other").unwrap() else {
            panic!("expected outputs");
        };
        assert_eq!(other.len(), 1);
    }
}
