/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! SQLite persistence for the local dep-file (local action) cache.
//!
//! Unlike the materializer table, this never serializes a directory's tree: a directory output is
//! stored as just its fingerprint and its tree is rehydrated from the materializer at lookup time.
//! State for one dep-file cache entry is spread across three tables, all keyed by
//! `(logical_key, config_key)`:
//!   - `dep_file_state`: the per-entry scalars (command digests, `was_produced_locally`).
//!   - `dep_file_outputs`: one row per output -- a leaf (file/symlink) in full, or a directory's
//!     fingerprint.
//!   - `dep_file_declared`: one row per declared dep-file identity.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::sqlite::sqlite_db::SqliteTables;
use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_execute::dep_file_state::DepFileDbSize;
use buck2_execute::dep_file_state::StoredDepFileDigests;
use buck2_execute::dep_file_state::StoredDepFileIdentity;
use buck2_execute::dep_file_state::StoredDepFileState;
use buck2_execute::dep_file_state::StoredOutput;
use buck2_execute::digest_config::DigestConfig;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_util::threads::available_parallelism;
use parking_lot::Mutex;
use rusqlite::Connection;
use rusqlite::OptionalExtension;

use crate::sqlite::tables::stored_output_row::COLUMN_NAMES;
use crate::sqlite::tables::stored_output_row::StoredOutputColumns;
use crate::sqlite::tables::stored_output_row::file_digest_parts;
use crate::sqlite::tables::stored_output_row::rebuild_file_digest;
use crate::sqlite::tables::stored_output_row::tracked_digest_parts;

const STATE_TABLE_NAME: &str = "dep_file_state";
const OUTPUTS_TABLE_NAME: &str = "dep_file_outputs";
const DECLARED_TABLE_NAME: &str = "dep_file_declared";

/// An output row of `dep_file_outputs`.
struct OutputRow {
    output_path: String,
    columns: StoredOutputColumns,
}

fn output_row_to_stored_output(
    row: OutputRow,
    digest_config: DigestConfig,
) -> buck2_error::Result<StoredOutput> {
    // Validate the path read from the database rather than trusting it: a corrupt row surfaces as an
    // error (skipped by the hydration caller) instead of constructing an invalid path.
    let path = ForwardRelativePathBuf::new(row.output_path)
        .buck_error_context("Invalid `output_path` in dep-file db")?;
    Ok(StoredOutput {
        path,
        value: row.columns.decode(digest_config)?,
    })
}

/// Delete all rows for `(logical_key, config_key)` across the three tables, within `tx`.
fn delete_key_in_tx(
    tx: &rusqlite::Transaction,
    logical_key: &[u8],
    config_key: &[u8],
) -> buck2_error::Result<()> {
    static OUTPUTS_SQL: LazyLock<String> = LazyLock::new(|| {
        format!(
            "DELETE FROM {OUTPUTS_TABLE_NAME} WHERE entry_id = (SELECT id FROM {STATE_TABLE_NAME} WHERE logical_key = ?1 AND config_key = ?2)"
        )
    });
    static DECLARED_SQL: LazyLock<String> = LazyLock::new(|| {
        format!(
            "DELETE FROM {DECLARED_TABLE_NAME} WHERE entry_id = (SELECT id FROM {STATE_TABLE_NAME} WHERE logical_key = ?1 AND config_key = ?2)"
        )
    });
    static STATE_SQL: LazyLock<String> = LazyLock::new(|| {
        format!("DELETE FROM {STATE_TABLE_NAME} WHERE logical_key = ?1 AND config_key = ?2")
    });
    for (table, sql) in [
        (OUTPUTS_TABLE_NAME, &OUTPUTS_SQL),
        (DECLARED_TABLE_NAME, &DECLARED_SQL),
        (STATE_TABLE_NAME, &STATE_SQL),
    ] {
        tx.prepare_cached(sql)?
            .execute(rusqlite::params![logical_key, config_key])
            .with_buck_error_context(|| format!("deleting from {table}"))?;
    }
    Ok(())
}

/// Ceiling on connections used for reads, set by measurement rather than by the machine: on a
/// 96-worker daemon, 16 left 5us of mean wait per read, which is an uncontended mutex, and more
/// stopped helping. `dep_file_db_read_lock_wait_us` is what would show the ceiling stops holding.
const READ_CONNECTIONS_MAX: usize = 16;

/// Scaled to the machine, so a daemon restricted by cgroup quota or affinity does not open
/// connections it has no threads to use, then capped at the ceiling. This is the count the daemon
/// sizes its tokio workers from, and reads run on those workers, so the two move together.
fn read_connection_count() -> usize {
    available_parallelism().min(READ_CONNECTIONS_MAX)
}

/// Time readers spent waiting for a connection, separate from time spent querying: this is what
/// says whether a slow lookup is the database working or a queue behind another reader, and the two
/// want different fixes.
#[derive(Default)]
struct LockWait {
    total_us: AtomicU64,
    max_us: AtomicU64,
}

impl LockWait {
    fn record(&self, waited: u64) {
        self.total_us.fetch_add(waited, Ordering::Relaxed);
        self.max_us.fetch_max(waited, Ordering::Relaxed);
    }
}

/// Bytes the database occupies on disk: the file itself, plus the log and shared-index files SQLite keeps beside it.
fn disk_bytes(db_path: &AbsNormPath) -> Option<u64> {
    // SQLite names the two companions by appending to the file name, not by replacing an
    // extension, so these are string suffixes rather than `with_extension`.
    ["", "-wal", "-shm"]
        .iter()
        .map(
            |suffix| match std::fs::metadata(format!("{db_path}{suffix}")) {
                Ok(metadata) => Some(metadata.len()),
                // Neither companion exists until the first write, and neither exists at all where
                // the journal is not WAL. Nothing there is nothing on disk, not a failed reading.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(0),
                Err(_) => None,
            },
        )
        .sum()
}

/// The connection `measure` reads through, with the path it was opened from.
struct MeasureHandle {
    connection: Arc<Mutex<Connection>>,
    db_path: AbsNormPathBuf,
}

pub struct DepFileStateSqliteTable {
    /// Carries every write, and every read when `read_connections` is empty.
    shared_connection: Arc<Mutex<Connection>>,
    /// Connections used only for reads, so lookups do not queue behind each other on the single
    /// connection the writer holds. WAL allows many concurrent readers; one shared connection gives
    /// that up. Empty when the database has no path to reopen (in-memory, i.e. tests), in which case
    /// reads fall back to `connection`.
    read_connections: Vec<Arc<Mutex<Connection>>>,
    /// Used only by `measure`, and kept out of `read_connections` on purpose: instrumentation must
    /// not queue behind a lookup, must not take a lookup's turn, and must not have its own waiting
    /// charged to `lock_wait`, which exists to say whether *lookups* are queueing.
    measure: Option<MeasureHandle>,
    lock_wait: LockWait,
}

impl DepFileStateSqliteTable {
    pub fn new(shared_connection: Arc<Mutex<Connection>>) -> Self {
        Self {
            shared_connection,
            read_connections: Vec::new(),
            measure: None,
            lock_wait: LockWait::default(),
        }
    }

    /// As `new`, plus a set of read-only-by-convention connections to the same file. A connection
    /// that fails to open is skipped rather than failing startup: fewer connections is slower, not
    /// incorrect, and reads fall back to the shared one if none open.
    pub fn new_with_read_connections(
        shared_connection: Arc<Mutex<Connection>>,
        path: &AbsNormPath,
    ) -> Self {
        let requested = read_connection_count();
        let read_connections: Vec<_> = (0..requested)
            .filter_map(|_| SqliteTables::<Self>::create_connection(path).ok())
            .collect();
        if read_connections.len() < requested {
            // Reads stay correct on a short pool, only slower, so this reports rather than fails.
            soft_error_or_panic!(
                "dep_file_db_read_connections_short",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Opened {} of {} dep-file db read connections; reads will contend",
                    read_connections.len(),
                    requested
                )
            );
        }
        Self {
            shared_connection,
            read_connections,
            measure: match SqliteTables::<Self>::create_connection(path) {
                Ok(connection) => Some(MeasureHandle {
                    connection,
                    db_path: path.to_buf(),
                }),
                Err(e) => {
                    soft_error_or_panic!(
                        "dep_file_db_measure_connection",
                        buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Tier0,
                            "Failed to open the dep-file db connection used for size metrics; \
                             they will be absent for this daemon. {}",
                            e
                        )
                    );
                    None
                }
            },
            lock_wait: LockWait::default(),
        }
    }

    /// Take a connection for a read, charging the wait to `read_lock_wait_us`. Threads are assigned
    /// a connection round-robin on first use and keep it, so a thread never contends with itself and
    /// the spread does not depend on how thread ids happen to hash.
    fn lock_for_read(&self) -> parking_lot::MutexGuard<'_, Connection> {
        let connection = if self.read_connections.is_empty() {
            &self.shared_connection
        } else {
            thread_local! {
                static SHARD: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
            }
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let shard = SHARD.with(|s| {
                s.get().unwrap_or_else(|| {
                    let next = NEXT.fetch_add(1, Ordering::Relaxed) as usize;
                    s.set(Some(next));
                    next
                })
            });
            &self.read_connections[shard % self.read_connections.len()]
        };
        let started = Instant::now();
        let guard = connection.lock();
        self.lock_wait
            .record((Instant::now() - started).as_micros() as u64);
        guard
    }

    /// Measures the database size in number of entries and disk footprint.
    ///
    /// The footprint comes from the files on disk, the item count from SQLite. The rows are
    /// counted before the files are sized, so the two can be slightly out of step.
    ///
    /// Needs no coordination with writers. SQLite allows one writer at a time and enforces that
    /// itself, and a WAL reader sees the snapshot that existed when its statement began, so this
    /// neither blocks a write nor waits for one.
    ///
    /// Returns `None` if either the count or the sizing fails, discarding a count that succeeded.
    pub(crate) fn measure(&self) -> Option<DepFileDbSize> {
        static COUNT_SQL: LazyLock<String> =
            LazyLock::new(|| format!("SELECT count(*) FROM {STATE_TABLE_NAME}"));
        let handle = self.measure.as_ref()?;
        let entries = {
            let conn = handle.connection.lock();
            conn.query_row(&COUNT_SQL, [], |r| r.get::<_, i64>(0))
                .ok()? as u64
        };
        Some(DepFileDbSize {
            entries,
            bytes: disk_bytes(&handle.db_path)?,
        })
    }

    /// Connections reads are spread over. Zero means every read shares the write connection, which
    /// is the in-memory (test) case and the one a short pool degrades to.
    pub(crate) fn read_connections(&self) -> u64 {
        self.read_connections.len() as u64
    }

    /// Cumulative reader wait for a connection, and the longest single wait.
    pub(crate) fn read_lock_wait(&self) -> (u64, u64) {
        (
            self.lock_wait.total_us.load(Ordering::Relaxed),
            self.lock_wait.max_us.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn create_table(&self) -> buck2_error::Result<()> {
        let conn = self.shared_connection.lock();
        for sql in [
            format!(
                "CREATE TABLE {STATE_TABLE_NAME} (
                    id                      INTEGER PRIMARY KEY,
                    logical_key             BLOB NOT NULL,
                    config_key              BLOB NOT NULL,
                    cli_digest              BLOB NOT NULL,
                    directory_size          INTEGER NOT NULL,
                    directory_hash          BLOB NOT NULL,
                    directory_hash_kind     INTEGER NOT NULL,
                    local_worker_size       INTEGER NULL DEFAULT NULL,
                    local_worker_hash       BLOB NULL DEFAULT NULL,
                    local_worker_hash_kind  INTEGER NULL DEFAULT NULL,
                    was_produced_locally    INTEGER NOT NULL,
                    last_write_time         INTEGER NOT NULL,
                    UNIQUE                  (logical_key, config_key)
                )"
            ),
            format!(
                "CREATE TABLE {OUTPUTS_TABLE_NAME} (
                    entry_id                INTEGER NOT NULL,
                    output_path             TEXT NOT NULL,
                    artifact_type           TEXT NOT NULL,
                    entry_size              INTEGER NULL DEFAULT NULL,
                    entry_hash              BLOB NULL DEFAULT NULL,
                    entry_hash_kind         INTEGER NULL DEFAULT NULL,
                    file_is_executable      INTEGER NULL DEFAULT NULL,
                    symlink_target          TEXT NULL DEFAULT NULL,
                    symlink_remaining_path  TEXT NULL DEFAULT NULL,
                    PRIMARY KEY             (entry_id, output_path)
                ) WITHOUT ROWID"
            ),
            format!(
                "CREATE TABLE {DECLARED_TABLE_NAME} (
                    entry_id                INTEGER NOT NULL,
                    label                   TEXT NOT NULL,
                    path                    TEXT NOT NULL,
                    projected               TEXT NOT NULL,
                    is_content_based        INTEGER NOT NULL,
                    PRIMARY KEY             (entry_id, label, path, projected)
                ) WITHOUT ROWID"
            ),
            format!(
                "CREATE INDEX idx_{STATE_TABLE_NAME}_last_write_time ON {STATE_TABLE_NAME} (last_write_time)"
            ),
        ] {
            conn.execute(&sql, [])
                .with_buck_error_context(|| format!("creating sqlite table: {sql}"))?;
        }
        Ok(())
    }

    pub(crate) fn insert(
        &self,
        logical_key: Vec<u8>,
        config_key: Vec<u8>,
        state: StoredDepFileState,
    ) -> buck2_error::Result<()> {
        static STATE_SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "INSERT INTO {STATE_TABLE_NAME} (logical_key, config_key, cli_digest, directory_size, directory_hash, directory_hash_kind, local_worker_size, local_worker_hash, local_worker_hash_kind, was_produced_locally, last_write_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            )
        });
        static OUTPUT_SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "INSERT INTO {OUTPUTS_TABLE_NAME} (entry_id, output_path, {COLUMN_NAMES}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            )
        });
        static DECLARED_SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "INSERT INTO {DECLARED_TABLE_NAME} (entry_id, label, path, projected, is_content_based) VALUES (?1, ?2, ?3, ?4, ?5)"
            )
        });

        let (directory_size, directory_hash, directory_hash_kind) =
            file_digest_parts(&state.directory_digest);
        let local_worker = state
            .local_worker_directory_digest
            .as_ref()
            .map(tracked_digest_parts);
        // Stamped on write (re-stamped every rebuild); `prune` uses it to bound the db by age.
        let last_write_time = jiff::Timestamp::now().as_second();

        let mut conn = self.shared_connection.lock();
        let tx = conn.transaction()?;
        // Clearing all three tables first handles that uniformly, and also avoids the primary-key
        // constraint that would otherwise roll the whole transaction back to a soft error, silently
        // leaving stale rows on disk.
        // Using delete-then-insert rather than UPSERT (`ON CONFLICT DO UPDATE`): `insert` must behave
        // as "persist or replace" (the `DepFileStore` trait contract), and an entry's `outputs`/
        // `declared` rows are a *set* that can shrink between writes of the same
        // `(logical_key, config_key)` (an action re-run may drop an output path). UPSERT keys on the
        // full composite PK, so it would update/insert the surviving rows but leave the vanished ones
        // orphaned -- it can't delete rows no longer in the new set.
        delete_key_in_tx(&tx, &logical_key, &config_key)?;
        tx.prepare_cached(&STATE_SQL)?
            .execute(rusqlite::params![
                logical_key,
                config_key,
                state.cli_digest,
                directory_size,
                directory_hash,
                directory_hash_kind,
                local_worker.map(|(size, _, _)| size),
                local_worker.map(|(_, bytes, _)| bytes),
                local_worker.map(|(_, _, kind)| kind),
                state.was_produced_locally,
                last_write_time,
            ])
            .with_buck_error_context(|| format!("inserting into {STATE_TABLE_NAME}"))?;
        let entry_id = tx.last_insert_rowid();

        for output in &state.outputs {
            let path = &output.path;
            let columns = StoredOutputColumns::encode(&output.value)?;
            tx.prepare_cached(&OUTPUT_SQL)?
                .execute(rusqlite::params![
                    entry_id,
                    path.as_str(),
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

        for identity in &state.declared {
            tx.prepare_cached(&DECLARED_SQL)?
                .execute(rusqlite::params![
                    entry_id,
                    identity.label,
                    identity.path,
                    identity.projected,
                    identity.is_content_based,
                ])
                .with_buck_error_context(|| format!("inserting into {DECLARED_TABLE_NAME}"))?;
        }

        tx.commit()?;
        Ok(())
    }

    pub(crate) fn delete(&self, logical_key: &[u8], config_key: &[u8]) -> buck2_error::Result<()> {
        // One transaction across all three tables so a mid-way failure can never leave an entry
        // half-deleted (state row gone but output/declared rows orphaned).
        let mut conn = self.shared_connection.lock();
        let tx = conn.transaction()?;
        delete_key_in_tx(&tx, logical_key, config_key)?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn clear(&self) -> buck2_error::Result<()> {
        let mut conn = self.shared_connection.lock();
        let tx = conn.transaction()?;
        for table in [STATE_TABLE_NAME, OUTPUTS_TABLE_NAME, DECLARED_TABLE_NAME] {
            tx.execute(&format!("DELETE FROM {table}"), [])
                .with_buck_error_context(|| format!("clearing {table}"))?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Bound the db so it does not grow without limit across daemon sessions. Drops entries at or
    /// before `cutoff` (age-based TTL, unix seconds) and -- if `max_entries` is set and exceeded --
    /// the oldest entries beyond that count. Both bounds reduce to "drop everything written at or
    /// before some timestamp", so their union is a single threshold (the more recent of the two) and
    /// the whole prune is a single indexed range. Only the parent carries `last_write_time`, so the
    /// children are deleted through the ids it selects -- and before it, or they would be orphaned.
    /// Intended to run once at startup. Returns the number of entries pruned.
    pub(crate) fn prune(
        &self,
        cutoff: Option<i64>,
        max_entries: Option<usize>,
    ) -> buck2_error::Result<usize> {
        let mut conn = self.shared_connection.lock();
        let tx = conn.transaction()?;

        // The count cap reduces to a timestamp: the `(max_entries + 1)`-th most-recently-written
        // entry is the newest one to drop. `OFFSET` past the end returns no row (nothing over the
        // cap). Coarse second-granularity ties make this a soft bound, which is fine for a growth cap.
        let max_entries_cutoff: Option<i64> = match max_entries {
            Some(max_entries) => tx
                .query_row(
                    &format!(
                        "SELECT last_write_time FROM {STATE_TABLE_NAME} ORDER BY last_write_time DESC LIMIT 1 OFFSET ?1"
                    ),
                    rusqlite::params![max_entries],
                    |row| row.get(0),
                )
                .optional()
                .with_buck_error_context(|| {
                    format!("reading {STATE_TABLE_NAME} max-entries cutoff for prune")
                })?,
            None => None,
        };

        let Some(cutoff) = [cutoff, max_entries_cutoff].into_iter().flatten().max() else {
            return Ok(0);
        };

        for table in [OUTPUTS_TABLE_NAME, DECLARED_TABLE_NAME] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE entry_id IN (SELECT id FROM {STATE_TABLE_NAME} WHERE last_write_time <= ?1)"
                ),
                rusqlite::params![cutoff],
            )
            .with_buck_error_context(|| format!("pruning {table}"))?;
        }
        let pruned = tx
            .execute(
                &format!("DELETE FROM {STATE_TABLE_NAME} WHERE last_write_time <= ?1"),
                rusqlite::params![cutoff],
            )
            .with_buck_error_context(|| format!("pruning {STATE_TABLE_NAME}"))?;
        tx.commit()?;
        Ok(pruned)
    }

    /// The scalar row of every configuration's entry for `logical_key`. Touches only
    /// `dep_file_state`, so a lookup that is going to be rejected never reads the outputs or
    /// declared rows.
    pub(crate) fn read_digests_by_logical(
        &self,
        logical_key: &[u8],
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Vec<StoredDepFileDigests>> {
        let rows = {
            let conn = self.lock_for_read();
            static SQL: LazyLock<String> = LazyLock::new(|| {
                format!(
                    "SELECT id, config_key, cli_digest, directory_size, directory_hash, directory_hash_kind, local_worker_size, local_worker_hash, local_worker_hash_kind FROM {STATE_TABLE_NAME} WHERE logical_key = ?1"
                )
            });
            let mut stmt = conn.prepare_cached(&SQL)?;
            stmt.query_map(rusqlite::params![logical_key], |row| {
                let id: i64 = row.get(0)?;
                let config_key: Vec<u8> = row.get(1)?;
                let cli_digest: Vec<u8> = row.get(2)?;
                let directory_size: u64 = row.get(3)?;
                let directory_hash: Vec<u8> = row.get(4)?;
                let directory_hash_kind: u8 = row.get(5)?;
                let local_worker_size: Option<u64> = row.get(6)?;
                let local_worker_hash: Option<Vec<u8>> = row.get(7)?;
                let local_worker_hash_kind: Option<u8> = row.get(8)?;
                Ok((
                    id,
                    config_key,
                    cli_digest,
                    directory_size,
                    directory_hash,
                    directory_hash_kind,
                    local_worker_size,
                    local_worker_hash,
                    local_worker_hash_kind,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .with_buck_error_context(|| format!("reading {STATE_TABLE_NAME}"))?
        };

        rows.into_iter()
            .map(
                |(
                    id,
                    config_key,
                    cli_digest,
                    directory_size,
                    directory_hash,
                    directory_hash_kind,
                    local_worker_size,
                    local_worker_hash,
                    local_worker_hash_kind,
                )| {
                    Ok(StoredDepFileDigests {
                        id,
                        config_key,
                        cli_digest,
                        directory_digest: rebuild_file_digest(
                            directory_size,
                            &directory_hash,
                            directory_hash_kind,
                        )?,
                        local_worker_directory_digest: match (
                            local_worker_size,
                            local_worker_hash,
                            local_worker_hash_kind,
                        ) {
                            (Some(size), Some(bytes), Some(kind)) => Some(TrackedFileDigest::new(
                                rebuild_file_digest(size, &bytes, kind)?,
                                digest_config.cas_digest_config(),
                            )),
                            _ => None,
                        },
                    })
                },
            )
            .collect()
    }

    /// The complete entry for the row `id` reported by `read_digests_by_logical`, or `None` if that
    /// row is gone. Reads the scalar row plus that entry's output and declared rows, then
    /// reassembles them.
    pub(crate) fn read_entry(
        &self,
        id: i64,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Option<StoredDepFileState>> {
        let conn = self.lock_for_read();

        let mut output_rows = Vec::new();
        {
            static SQL: LazyLock<String> = LazyLock::new(|| {
                format!(
                    "SELECT output_path, {COLUMN_NAMES} FROM {OUTPUTS_TABLE_NAME} WHERE entry_id = ?1"
                )
            });
            let mut stmt = conn.prepare_cached(&SQL)?;
            let rows = stmt
                .query_map(rusqlite::params![id], |row| {
                    Ok(OutputRow {
                        output_path: row.get(0)?,
                        columns: StoredOutputColumns::read(row, 1)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .with_buck_error_context(|| format!("reading {OUTPUTS_TABLE_NAME}"))?;
            output_rows.extend(rows);
        }

        let mut declared: Vec<StoredDepFileIdentity> = Vec::new();
        {
            static SQL: LazyLock<String> = LazyLock::new(|| {
                format!(
                    "SELECT label, path, projected, is_content_based FROM {DECLARED_TABLE_NAME} WHERE entry_id = ?1"
                )
            });
            let mut stmt = conn.prepare_cached(&SQL)?;
            let rows = stmt
                .query_map(rusqlite::params![id], |row| {
                    Ok(StoredDepFileIdentity {
                        label: row.get(0)?,
                        path: row.get(1)?,
                        projected: row.get(2)?,
                        is_content_based: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .with_buck_error_context(|| format!("reading {DECLARED_TABLE_NAME}"))?;
            declared.extend(rows);
        }

        let scalar_row = {
            static SQL: LazyLock<String> = LazyLock::new(|| {
                format!(
                    "SELECT cli_digest, directory_size, directory_hash, directory_hash_kind, local_worker_size, local_worker_hash, local_worker_hash_kind, was_produced_locally FROM {STATE_TABLE_NAME} WHERE id = ?1"
                )
            });
            let mut stmt = conn.prepare_cached(&SQL)?;
            stmt.query_row(rusqlite::params![id], |row| {
                let cli_digest: Vec<u8> = row.get(0)?;
                let directory_size: u64 = row.get(1)?;
                let directory_hash: Vec<u8> = row.get(2)?;
                let directory_hash_kind: u8 = row.get(3)?;
                let local_worker_size: Option<u64> = row.get(4)?;
                let local_worker_hash: Option<Vec<u8>> = row.get(5)?;
                let local_worker_hash_kind: Option<u8> = row.get(6)?;
                let was_produced_locally: bool = row.get(7)?;
                Ok((
                    cli_digest,
                    directory_size,
                    directory_hash,
                    directory_hash_kind,
                    local_worker_size,
                    local_worker_hash,
                    local_worker_hash_kind,
                    was_produced_locally,
                ))
            })
            .optional()
            .with_buck_error_context(|| format!("reading {STATE_TABLE_NAME}"))?
        };

        // Every row is now materialized, so release the connection before rebuilding digests and
        // artifact values: that work is pure CPU and would otherwise run inside the critical section
        // that every concurrent lookup contends on.
        drop(conn);

        let Some((
            cli_digest,
            directory_size,
            directory_hash,
            directory_hash_kind,
            local_worker_size,
            local_worker_hash,
            local_worker_hash_kind,
            was_produced_locally,
        )) = scalar_row
        else {
            return Ok(None);
        };

        let directory_digest =
            rebuild_file_digest(directory_size, &directory_hash, directory_hash_kind)?;
        let local_worker_directory_digest =
            match (local_worker_size, local_worker_hash, local_worker_hash_kind) {
                (Some(size), Some(bytes), Some(kind)) => Some(TrackedFileDigest::new(
                    rebuild_file_digest(size, &bytes, kind)?,
                    digest_config.cas_digest_config(),
                )),
                _ => None,
            };

        let outputs = output_rows
            .into_iter()
            .map(|out| output_row_to_stored_output(out, digest_config))
            .collect::<buck2_error::Result<Vec<_>>>()?;

        Ok(Some(StoredDepFileState {
            cli_digest,
            directory_digest,
            local_worker_directory_digest,
            was_produced_locally,
            declared,
            outputs,
        }))
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::external_symlink::ExternalSymlink;
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::Symlink;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_directory::directory::entry::DirectoryEntry;
    use buck2_execute::artifact_value::ArtifactValue;
    use buck2_execute::dep_file_state::StoredOutputValue;
    use buck2_execute::directory::ActionDirectoryBuilder;
    use buck2_execute::directory::ActionDirectoryMember;
    use buck2_execute::directory::extract_artifact_value;
    use buck2_execute::directory::insert_entry;
    use buck2_execute::directory::insert_file;
    use dupe::Dupe;

    use super::*;

    fn table() -> DepFileStateSqliteTable {
        let conn = Connection::open_in_memory().unwrap();
        let table = DepFileStateSqliteTable::new(Arc::new(Mutex::new(conn)));
        table.create_table().unwrap();
        table
    }

    #[test]
    fn test_disk_bytes_sums_the_files_that_are_there() {
        let dir = tempfile::tempdir().unwrap();
        let db = AbsNormPathBuf::new(dir.path().join("db.sqlite")).unwrap();
        std::fs::write(&db, vec![0; 100]).unwrap();

        // A database with no log yet is sized, not reported as unreadable.
        assert_eq!(Some(100), disk_bytes(&db));

        std::fs::write(format!("{db}-wal"), vec![0; 20]).unwrap();
        std::fs::write(format!("{db}-shm"), vec![0; 3]).unwrap();
        assert_eq!(Some(123), disk_bytes(&db));
    }

    fn file_value(
        digest_config: DigestConfig,
        content: &[u8],
        is_executable: bool,
    ) -> ArtifactValue {
        ArtifactValue::file(FileMetadata {
            digest: TrackedFileDigest::from_content(content, digest_config.cas_digest_config()),
            is_executable,
        })
    }

    fn leaf_output(path: &str, value: ArtifactValue) -> StoredOutput {
        StoredOutput {
            path: ForwardRelativePathBuf::unchecked_new(path.to_owned()),
            value: StoredOutputValue::Leaf(value),
        }
    }

    fn set_write_time(table: &DepFileStateSqliteTable, logical: &[u8], config: &[u8], t: i64) {
        table
            .shared_connection
            .lock()
            .execute(
                &format!(
                    "UPDATE {STATE_TABLE_NAME} SET last_write_time = ?1 WHERE logical_key = ?2 AND config_key = ?3"
                ),
                rusqlite::params![t, logical, config],
            )
            .unwrap();
    }

    /// For testing only: Probe for `(logical, config)` the way production does, then fetch by the
    /// row handle the probe reports. Returns `None` when no row exists for that configuration.
    fn probe_then_read(
        table: &DepFileStateSqliteTable,
        logical: &[u8],
        config: &[u8],
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Option<StoredDepFileState>> {
        let Some(digests) = table
            .read_digests_by_logical(logical, digest_config)?
            .into_iter()
            .find(|d| d.config_key == config)
        else {
            return Ok(None);
        };
        table.read_entry(digests.id, digest_config)
    }

    /// Child rows whose parent is gone. Deleting a child now depends on reaching it through its
    /// parent, so if the parent is removed first the child becomes unreachable forever -- this is
    /// the assertion that catches that, and a per-key count could not: once the parent row is gone,
    /// any query joining through it reports zero whether or not the child rows are still there.
    fn orphan_row_count(table: &DepFileStateSqliteTable) -> i64 {
        let conn = table.shared_connection.lock();
        [OUTPUTS_TABLE_NAME, DECLARED_TABLE_NAME]
            .iter()
            .map(|child| {
                conn.query_row(
                    &format!(
                        "SELECT COUNT(*) FROM {child} WHERE entry_id NOT IN (SELECT id FROM {STATE_TABLE_NAME})"
                    ),
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
            })
            .sum()
    }

    /// Rows across all three tables. Reads by key can only speak for the keys a test knows about;
    /// this is what says the table itself is empty.
    fn total_row_count(table: &DepFileStateSqliteTable) -> i64 {
        let conn = table.shared_connection.lock();
        [STATE_TABLE_NAME, OUTPUTS_TABLE_NAME, DECLARED_TABLE_NAME]
            .iter()
            .map(|name| {
                conn.query_row(&format!("SELECT COUNT(*) FROM {name}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
            })
            .sum()
    }

    #[test]
    fn test_prune_ttl_and_max_entries() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let directory_digest =
            TrackedFileDigest::from_content(b"d", digest_config.cas_digest_config())
                .data()
                .dupe();
        let make = || StoredDepFileState {
            cli_digest: vec![1u8; 32],
            directory_digest: directory_digest.dupe(),
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![],
            outputs: vec![leaf_output("o", file_value(digest_config, b"c", false))],
        };
        for k in [b"a", b"b", b"c"] {
            table.insert(k.to_vec(), b"cfg".to_vec(), make())?;
        }
        set_write_time(&table, b"a", b"cfg", 100);
        set_write_time(&table, b"b", b"cfg", 200);
        set_write_time(&table, b"c", b"cfg", 300);

        // TTL: drop entries with last_write_time < 150 -> only "a".
        assert_eq!(table.prune(Some(150), None)?, 1);
        assert!(probe_then_read(&table, b"a", b"cfg", digest_config)?.is_none());
        assert!(probe_then_read(&table, b"b", b"cfg", digest_config)?.is_some());
        assert!(probe_then_read(&table, b"c", b"cfg", digest_config)?.is_some());
        // Prune reaches children through the parent, so it must not leave any behind.
        assert_eq!(orphan_row_count(&table), 0);

        // max_entries: keep only the most-recently-written ("c").
        assert_eq!(table.prune(None, Some(1))?, 1);
        assert!(probe_then_read(&table, b"b", b"cfg", digest_config)?.is_none());
        assert!(probe_then_read(&table, b"c", b"cfg", digest_config)?.is_some());
        Ok(())
    }

    #[test]
    fn test_symlink_and_external_symlink_roundtrip() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let directory_digest =
            TrackedFileDigest::from_content(b"d", digest_config.cas_digest_config())
                .data()
                .dupe();

        let symlink = ArtifactValue::new(
            DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(Arc::new(Symlink::new(
                "a/b".into(),
            )))),
            None,
        );
        // An external symlink with a non-empty `remaining_path` -- the case that was silently
        // dropped before `remaining_path` was persisted.
        let external = ArtifactValue::external_symlink(Arc::new(ExternalSymlink::new(
            "/root".into(),
            ForwardRelativePathBuf::unchecked_new("file".to_owned()),
        )?));

        let stored = StoredDepFileState {
            cli_digest: vec![9u8; 32],
            directory_digest,
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![],
            outputs: vec![
                leaf_output("sym", symlink.dupe()),
                leaf_output("ext", external.dupe()),
            ],
        };
        table.insert(b"k".to_vec(), b"c".to_vec(), stored)?;

        let read = probe_then_read(&table, b"k", b"c", digest_config)?.unwrap();
        assert_eq!(read.outputs.len(), 2);

        let value_at = |p: &str| {
            read.outputs
                .iter()
                .find(|o| o.path.as_str() == p)
                .map(|o| &o.value)
        };
        match value_at("sym").unwrap() {
            StoredOutputValue::Leaf(v) => assert_eq!(v.entry(), symlink.entry()),
            StoredOutputValue::Directory(_) => panic!("expected a leaf"),
        }
        // The external symlink -- including its `remaining_path` -- must survive the round-trip.
        match value_at("ext").unwrap() {
            StoredOutputValue::Leaf(v) => assert_eq!(v.entry(), external.entry()),
            StoredOutputValue::Directory(_) => panic!("expected a leaf"),
        }
        Ok(())
    }

    #[test]
    fn test_insert_read_roundtrip() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let table = table();

        let directory_digest =
            TrackedFileDigest::from_content(b"input-dir", digest_config.cas_digest_config())
                .data()
                .dupe();
        let out_value = file_value(digest_config, b"object-file", true);
        let stored = StoredDepFileState {
            cli_digest: vec![7u8; 32],
            directory_digest,
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![StoredDepFileIdentity {
                label: "dep".to_owned(),
                path: "foo/out.d".to_owned(),
                projected: String::new(),
                is_content_based: false,
            }],
            outputs: vec![leaf_output("foo/out.o", out_value.dupe())],
        };

        table.insert(b"logical".to_vec(), b"cfg".to_vec(), stored)?;

        let read = probe_then_read(&table, b"logical", b"cfg", digest_config)?.unwrap();
        assert_eq!(read.cli_digest, vec![7u8; 32]);
        assert_eq!(read.directory_digest, directory_digest);
        assert!(read.local_worker_directory_digest.is_none());
        assert!(read.was_produced_locally);
        assert_eq!(read.declared.len(), 1);
        assert_eq!(read.declared[0].label, "dep");
        assert_eq!(read.declared[0].path, "foo/out.d");
        assert!(!read.declared[0].is_content_based);
        assert_eq!(read.outputs.len(), 1);
        assert_eq!(read.outputs[0].path.as_str(), "foo/out.o");
        // The reconstructed leaf value must match the original entry (this is what `declare_match`
        // compares on a cache hit).
        match &read.outputs[0].value {
            StoredOutputValue::Leaf(v) => assert_eq!(v.entry(), out_value.entry()),
            StoredOutputValue::Directory(_) => panic!("expected a leaf output"),
        }
        Ok(())
    }

    #[test]
    fn test_directory_output_roundtrip() -> buck2_error::Result<()> {
        // A directory output persists only its fingerprint; the tree is rehydrated from the
        // materializer at lookup time. Verify the fingerprint round-trips.
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let directory_digest =
            TrackedFileDigest::from_content(b"input-dir", digest_config.cas_digest_config())
                .data()
                .dupe();
        let dir_fingerprint =
            TrackedFileDigest::from_content(b"dir-tree", digest_config.cas_digest_config())
                .data()
                .dupe();
        let stored = StoredDepFileState {
            cli_digest: vec![3u8; 32],
            directory_digest,
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![],
            outputs: vec![StoredOutput {
                path: ForwardRelativePathBuf::unchecked_new("out/dir".to_owned()),
                value: StoredOutputValue::Directory(dir_fingerprint.dupe()),
            }],
        };

        table.insert(b"k".to_vec(), b"c".to_vec(), stored)?;

        let read = probe_then_read(&table, b"k", b"c", digest_config)?.unwrap();
        assert_eq!(read.outputs.len(), 1);
        assert_eq!(read.outputs[0].path.as_str(), "out/dir");
        match &read.outputs[0].value {
            StoredOutputValue::Directory(fp) => assert_eq!(*fp, dir_fingerprint),
            StoredOutputValue::Leaf(_) => panic!("expected a directory output"),
        }
        Ok(())
    }

    #[test]
    fn test_delete_and_clear() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let directory_digest =
            TrackedFileDigest::from_content(b"d", digest_config.cas_digest_config())
                .data()
                .dupe();
        let make = || StoredDepFileState {
            cli_digest: vec![1u8; 32],
            directory_digest,
            local_worker_directory_digest: None,
            was_produced_locally: false,
            declared: vec![],
            outputs: vec![leaf_output("o", file_value(digest_config, b"c", false))],
        };
        table.insert(b"a".to_vec(), b"c1".to_vec(), make())?;
        table.insert(b"b".to_vec(), b"c1".to_vec(), make())?;
        assert!(probe_then_read(&table, b"a", b"c1", digest_config)?.is_some());
        assert!(probe_then_read(&table, b"b", b"c1", digest_config)?.is_some());

        table.delete(b"a", b"c1")?;
        assert!(probe_then_read(&table, b"a", b"c1", digest_config)?.is_none());
        assert!(probe_then_read(&table, b"b", b"c1", digest_config)?.is_some());
        // Delete reaches children through the parent, so it must not leave any behind.
        assert_eq!(orphan_row_count(&table), 0);

        // Rows survive the delete, so the count below is answering a real question.
        assert_ne!(total_row_count(&table), 0);

        table.clear()?;
        assert!(probe_then_read(&table, b"b", b"c1", digest_config)?.is_none());
        // `clear` empties the whole db, not just the keys this test inserted.
        assert_eq!(total_row_count(&table), 0);
        Ok(())
    }

    #[test]
    fn test_reinsert_same_key_replaces() -> buck2_error::Result<()> {
        // Re-persisting the same (logical_key, config_key) must replace the entry, not fail on a
        // primary-key constraint. This happens whenever an action re-runs.
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let dir = |c: &[u8]| {
            TrackedFileDigest::from_content(c, digest_config.cas_digest_config())
                .data()
                .dupe()
        };
        let make = |content: &[u8], out: &str| StoredDepFileState {
            cli_digest: vec![1u8; 32],
            directory_digest: dir(content),
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![StoredDepFileIdentity {
                label: "dep".to_owned(),
                path: "out.d".to_owned(),
                projected: String::new(),
                is_content_based: false,
            }],
            outputs: vec![leaf_output(out, file_value(digest_config, content, false))],
        };

        table.insert(b"k".to_vec(), b"c".to_vec(), make(b"v1", "a.o"))?;
        // Second insert of the same key with different content and a different output path.
        table.insert(b"k".to_vec(), b"c".to_vec(), make(b"v2", "b.o"))?;

        let read = probe_then_read(&table, b"k", b"c", digest_config)?.unwrap();
        assert_eq!(
            read.directory_digest,
            dir(b"v2"),
            "must hold the newer value"
        );
        // The stale output row from the first insert must be gone (no orphans).
        assert_eq!(read.outputs.len(), 1);
        assert_eq!(read.outputs[0].path.as_str(), "b.o");
        Ok(())
    }

    #[test]
    fn test_deps_do_not_survive_the_round_trip() -> buck2_error::Result<()> {
        // There is no column for `ArtifactValue::deps` -- the artifacts an output's symlinks point
        // at -- so it cannot round-trip. `DepFileState::to_stored` refuses to persist a deps-bearing
        // entry for exactly this reason; this test pins the loss so that guard is not dropped on the
        // assumption that deps survives. Serving such an output would leave a dangling symlink,
        // because nothing downstream would know to materialize the destinations.
        let digest_config = DigestConfig::testing_default();
        let table = table();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_file(
            &mut builder,
            ProjectRelativePathBuf::unchecked_new("target".to_owned()),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        insert_entry(
            &mut builder,
            ProjectRelativePathBuf::unchecked_new("out".to_owned()),
            DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(Arc::new(Symlink::new(
                "target".into(),
            )))),
        )?;
        let with_deps = extract_artifact_value(
            &builder,
            ProjectRelativePath::unchecked_new("out"),
            digest_config,
        )?
        .expect("`out` is in the builder");
        assert!(
            with_deps.deps().is_some(),
            "test setup: the symlink output should have deps"
        );

        let stored = StoredDepFileState {
            cli_digest: vec![7u8; 32],
            directory_digest: TrackedFileDigest::from_content(
                b"d",
                digest_config.cas_digest_config(),
            )
            .data()
            .dupe(),
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![],
            outputs: vec![leaf_output("out", with_deps.dupe())],
        };
        table.insert(b"k".to_vec(), b"c".to_vec(), stored)?;

        let read = probe_then_read(&table, b"k", b"c", digest_config)?.unwrap();
        match &read.outputs[0].value {
            StoredOutputValue::Leaf(v) => {
                // The symlink itself round-trips ...
                assert_eq!(v.entry(), with_deps.entry());
                // ... but what it points at is gone.
                assert!(v.deps().is_none());
            }
            StoredOutputValue::Directory(_) => panic!("expected a leaf"),
        }
        Ok(())
    }

    #[test]
    fn test_read_digests_then_entry() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let dir = |content: &[u8]| {
            TrackedFileDigest::from_content(content, digest_config.cas_digest_config())
                .data()
                .dupe()
        };
        let make = |cli: u8, content: &[u8], output: &str| StoredDepFileState {
            cli_digest: vec![cli; 32],
            directory_digest: dir(content),
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![],
            outputs: vec![leaf_output(
                output,
                file_value(digest_config, content, false),
            )],
        };

        // The same logical action built under two configurations.
        table.insert(b"k".to_vec(), b"cfg1".to_vec(), make(1, b"v1", "a.o"))?;
        table.insert(b"k".to_vec(), b"cfg2".to_vec(), make(2, b"v2", "b.o"))?;

        // The digest probe returns one row per configuration, and nothing else.
        let mut digests = table.read_digests_by_logical(b"k", digest_config)?;
        digests.sort_by(|a, b| a.config_key.cmp(&b.config_key));
        assert_eq!(digests.len(), 2);
        assert_eq!(digests[0].config_key, b"cfg1");
        assert_eq!(digests[0].cli_digest, vec![1u8; 32]);
        assert_eq!(digests[0].directory_digest, dir(b"v1"));
        assert_eq!(digests[1].config_key, b"cfg2");
        assert_eq!(digests[1].cli_digest, vec![2u8; 32]);
        assert_eq!(digests[1].directory_digest, dir(b"v2"));

        // Fetching an entry returns only that configuration's outputs, not the sibling's.
        let entry =
            probe_then_read(&table, b"k", b"cfg2", digest_config)?.expect("cfg2 was inserted");
        assert_eq!(entry.directory_digest, dir(b"v2"));
        assert_eq!(entry.outputs.len(), 1);
        assert_eq!(entry.outputs[0].path.as_str(), "b.o");

        // Absent keys are a miss, not an error.
        assert!(probe_then_read(&table, b"k", b"cfg3", digest_config)?.is_none());
        assert!(probe_then_read(&table, b"nope", b"cfg1", digest_config)?.is_none());
        assert!(
            table
                .read_digests_by_logical(b"nope", digest_config)?
                .is_empty()
        );
        Ok(())
    }
    /// A row handle must not outlive the probe that produced it. Rewriting an entry deletes and
    /// re-inserts its row, and sqlite recycles the id of a deleted row, so an old handle can resolve
    /// to the replacement -- or, once recycled by a different key, to an unrelated entry entirely.
    /// Nothing in the schema prevents that; `check_action` re-validating whatever is served is what
    /// makes it safe. Pinned here so the rule stays visible if the handle ever starts being cached.
    #[test]
    fn test_row_handle_must_not_outlive_the_probe() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let table = table();
        let dir = |b: &[u8]| {
            TrackedFileDigest::from_content(b, digest_config.cas_digest_config())
                .data()
                .dupe()
        };
        let make = |v: &[u8]| StoredDepFileState {
            cli_digest: vec![1u8; 32],
            directory_digest: dir(v),
            local_worker_directory_digest: None,
            was_produced_locally: true,
            declared: vec![],
            outputs: vec![leaf_output("o", file_value(digest_config, v, false))],
        };

        table.insert(b"k".to_vec(), b"c".to_vec(), make(b"v1"))?;
        let handle = table.read_digests_by_logical(b"k", digest_config)?[0].id;

        // Rewriting the same key recycles the id, so the old handle now serves the NEW content.
        table.insert(b"k".to_vec(), b"c".to_vec(), make(b"v2"))?;
        let served = table
            .read_entry(handle, digest_config)?
            .expect("the recycled handle still resolves");
        assert_eq!(
            served.directory_digest,
            dir(b"v2"),
            "a stale handle must never serve the content it was probed for"
        );

        // Recycled across keys, it resolves to a different action's entry entirely.
        table.delete(b"k", b"c")?;
        table.insert(b"other".to_vec(), b"c".to_vec(), make(b"v3"))?;
        let other = table.read_digests_by_logical(b"other", digest_config)?[0].id;
        assert_eq!(other, handle, "sqlite recycled the deleted row's id");
        assert_eq!(
            table
                .read_entry(handle, digest_config)?
                .expect("resolves to the unrelated entry")
                .directory_digest,
            dir(b"v3")
        );
        Ok(())
    }
    /// Every statement on a production path must reach its rows through an index. Asserted on the
    /// real schema so a future column or index change that silently turns a lookup into a table walk
    /// fails here rather than in a profile.
    ///
    /// Deliberately not "must not contain SCAN": `prune` resolving its max-entries cutoff is a
    /// covering-index scan, which is the optimal plan for `ORDER BY ... LIMIT 1 OFFSET ?` and builds
    /// no temp B-tree. What actually matters is that an index is used and nothing is sorted.
    #[test]
    fn test_production_statements_are_indexed() {
        let table = table();
        let conn = table.shared_connection.lock();
        let plan = |sql: &str| -> String {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            // The plan does not depend on the bound values (no `ANALYZE`, so no statistics), but the
            // placeholder count still has to match.
            let binds = vec![rusqlite::types::Value::Null; stmt.parameter_count()];
            let rows: Vec<String> = stmt
                .query_map(rusqlite::params_from_iter(binds), |r| r.get::<_, String>(3))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            rows.join(" | ")
        };
        for sql in [
            // probe
            &format!(
                "SELECT id, config_key, cli_digest FROM {STATE_TABLE_NAME} WHERE logical_key = ?1"
            ) as &str,
            // fetch
            &format!("SELECT cli_digest FROM {STATE_TABLE_NAME} WHERE id = ?1"),
            &format!("SELECT output_path FROM {OUTPUTS_TABLE_NAME} WHERE entry_id = ?1"),
            &format!("SELECT label FROM {DECLARED_TABLE_NAME} WHERE entry_id = ?1"),
            // delete, children through the parent
            &format!(
                "DELETE FROM {OUTPUTS_TABLE_NAME} WHERE entry_id = (SELECT id FROM {STATE_TABLE_NAME} WHERE logical_key = ?1 AND config_key = ?2)"
            ),
            &format!("DELETE FROM {STATE_TABLE_NAME} WHERE logical_key = ?1 AND config_key = ?2"),
            // prune
            &format!(
                "SELECT last_write_time FROM {STATE_TABLE_NAME} ORDER BY last_write_time DESC LIMIT 1 OFFSET ?1"
            ),
            &format!(
                "DELETE FROM {OUTPUTS_TABLE_NAME} WHERE entry_id IN (SELECT id FROM {STATE_TABLE_NAME} WHERE last_write_time <= ?1)"
            ),
            &format!("DELETE FROM {STATE_TABLE_NAME} WHERE last_write_time <= ?1"),
        ] {
            let plan = plan(sql);
            assert!(
                plan.contains("USING INDEX")
                    || plan.contains("USING PRIMARY KEY")
                    || plan.contains("USING INTEGER PRIMARY KEY")
                    || plan.contains("USING COVERING INDEX"),
                "no index used\n  sql:  {sql}\n  plan: {plan}"
            );
            assert!(
                !plan.contains("TEMP B-TREE"),
                "statement sorts\n  sql:  {sql}\n  plan: {plan}"
            );
        }
    }
}
