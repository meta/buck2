/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Vocabulary types for persisting the local dep-file (local action) cache to disk.
//!
//! The in-memory cache (`DEP_FILES`) lives in `buck2_action_impl`; the SQLite database lives in
//! `buck2_execute_impl`; and the daemon assembles them in `buck2_server`. None of those three
//! crates depend on each other, so the shared types live here in `buck2_execute`, which they all
//! depend on.

use std::sync::Arc;

use allocative::Allocative;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dice::UserComputationData;

use crate::artifact_value::ArtifactValue;

/// A single dep-file cache entry in the configuration-independent form that is persisted to disk and
/// reloaded across daemon restarts. This carries only what the identical-action (`LocalActionCache`)
/// lookup needs: the command digests, the config-independent declared dep-file identities, and the
/// action's outputs keyed by their target-relative path (the configuration-dependent
/// `BuildArtifactPath` is reconstructed from the live action's declared outputs at lookup time).
#[derive(Clone, Debug)]
pub struct StoredDepFileState {
    /// Raw bytes of the expanded command line digest. Stored as bytes (rather than
    /// `ExpandedCommandLineDigest`) so this type need not depend on `buck2_build_api`.
    pub cli_digest: Vec<u8>,
    /// Digest of the action's input directory.
    pub directory_digest: FileDigest,
    /// Digest of the local worker's input directory, if any.
    pub local_worker_directory_digest: Option<TrackedFileDigest>,
    pub was_produced_locally: bool,
    /// Config-independent identities of the dep files this action declared. Empty if the action
    /// declared no dep files.
    pub declared: Vec<StoredDepFileIdentity>,
    /// The action's outputs, keyed by target-relative path.
    pub outputs: Vec<StoredOutput>,
}

/// The scalar (digest) columns of one persisted entry, without its outputs or declared dep files.
/// Enough to reject a candidate that cannot match the live action, which is the common case.
#[derive(Clone, Debug)]
pub struct StoredDepFileDigests {
    /// Row handle for this entry; pass it back to `DepFileStore::get_entry` to fetch the rest.
    ///
    /// Valid only until the entry is next written or removed, so it must not outlive the
    /// probe that produced it: persisting an entry replaces its row and issues a new id, and the
    /// database reuses the ids of deleted rows, so a stale id can name a *different* entry.
    pub id: i64,
    /// Which configuration's entry this is. Distinct from `id` because eviction and the
    /// malformed-row path remove entries with `DepFileStore::delete`, which is keyed by
    /// `(logical_key, config_key)` -- see the note on that method.
    pub config_key: Vec<u8>,
    /// Raw bytes of the expanded command line digest.
    pub cli_digest: Vec<u8>,
    pub directory_digest: FileDigest,
    pub local_worker_directory_digest: Option<TrackedFileDigest>,
}

/// One persisted output of a dep-file cache entry, keyed by its target-relative path.
#[derive(Clone, Debug, Allocative)]
pub struct StoredOutput {
    pub path: ForwardRelativePathBuf,
    pub value: StoredOutputValue,
}

/// The persisted form of an output's value.
#[derive(Clone, Debug, Allocative)]
pub enum StoredOutputValue {
    /// A leaf output (file/symlink): the value is small and stored in full, so it is reconstructed
    /// directly from the row.
    Leaf(ArtifactValue),
    /// A directory output: only its fingerprint is persisted. The full tree is rehydrated from the
    /// materializer (which already persists+reloads it) at lookup time and verified against this
    /// fingerprint, avoiding a second serialization of the tree here.
    Directory(FileDigest),
}

/// The configuration-independent identity of a declared dep file, mirroring the tuple compared by
/// `DeclaredDepFiles::declares_same_dep_files` in `buck2_action_impl`.
#[derive(Clone, Debug)]
pub struct StoredDepFileIdentity {
    pub label: String,
    /// Target-relative output path of the dep file.
    pub path: String,
    /// Projected path within the output artifact.
    pub projected: String,
    pub is_content_based: bool,
}

/// Cumulative cost of the persisted store's writes, reported per snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct DepFileWriteStats {
    /// Every message the writer handled, including `Flush`. Counts the queue, not the database.
    pub applied: u64,
    pub duration_us: u64,
    pub max_us: u64,
    /// The writes that reached the database, split by what they did. Their sum is the count
    /// `duration_us` averages over -- `applied` is not, since it includes `Flush`, which does no
    /// database work and is excluded from the duration.
    pub inserts: u64,
    pub insert_duration_us: u64,
    pub deletes: u64,
    pub delete_duration_us: u64,
    pub clears: u64,
    pub clear_duration_us: u64,
}

/// Cumulative cost of the persisted store's reads, reported per snapshot. `MatchDepFilesEnd`
/// carries the same measurements per action, which gives a distribution but only for one
/// invocation and only by parsing its event log; this is the aggregate that answers what the cache
/// costs across many builds.
#[derive(Clone, Copy, Debug, Default)]
pub struct DepFileReadStats {
    pub probes: u64,
    pub probe_duration_us: u64,
    pub fetches: u64,
    /// Fetches that found their row. Not the same as a served hit: `check_action` re-validates the
    /// entry afterwards and can still reject it.
    pub fetches_found: u64,
    pub fetch_duration_us: u64,
    pub max_us: u64,
    /// Entries actually served from disk. Strictly fewer than `fetches_found`: a fetched row can
    /// still be rejected by `check_action` or have lost its outputs.
    pub hits: u64,
    /// Of the durations above, how much was spent waiting for the database connection rather than
    /// querying it. Every read takes one mutex, so this separates a slow lookup caused by the
    /// database working from one caused by queueing behind another reader.
    pub lock_wait_us: u64,
    pub lock_wait_max_us: u64,
    /// Connections reads are spread over, fixed for the daemon's life. Reported with the waits
    /// because it is what sizes them: the same wait means different things at one connection and at
    /// sixteen, and zero connections means reads fell back to sharing the writer's.
    pub read_connections: u64,
}

/// How many entries -- (logical_key, config_key) pairs -- the database holds in the
/// `dep_file_state` table, and what its footprint on disk is.
#[derive(Clone, Copy, Debug)]
pub struct DepFileDbSize {
    pub entries: u64,
    pub bytes: u64,
}

/// Write-through handle to the persisted dep-file database. Implemented in `buck2_execute_impl` and
/// installed by `buck2_server` once the database is opened. All methods must be crash-safe and must
/// never fail the build: implementations downgrade database errors to soft errors.
pub trait DepFileStore: Send + Sync + 'static {
    /// Persist (or replace) the entry for `(logical_key, config_key)`. Keys are opaque digests
    /// (stored as `BLOB`), not decoded.
    fn insert(&self, logical_key: Vec<u8>, config_key: Vec<u8>, state: StoredDepFileState);
    /// Remove the entry for `(logical_key, config_key)`, if present. Removes that one
    /// configuration, never its siblings under the same logical key.
    ///
    /// Keyed by the pair rather than by `StoredDepFileDigests::id` because its callers hold the
    /// action's identity and no row handle: eviction mirrors a removal from the in-memory cache,
    /// which is keyed the same way, so the two stay one-for-one.
    fn delete(&self, logical_key: Vec<u8>, config_key: Vec<u8>);
    /// The digests of every persisted entry for `logical_key`, one per configuration it was built
    /// under (empty on a miss or on any database error). Reads only the scalar table, so a candidate
    /// that cannot match costs nothing more than this. The in-memory cache calls this on demand for a
    /// logical action it has no live entry for, rather than loading the whole database at startup.
    fn get_digests(&self, logical_key: &[u8]) -> Vec<StoredDepFileDigests>;
    /// The complete entry for `id`, including its outputs and declared dep files, or `None` if that
    /// row is gone. Call only once `get_digests` reported a match: this is the expensive half of a
    /// lookup. Pass an `id` from that same probe -- see `StoredDepFileDigests::id`.
    fn get_entry(&self, id: i64) -> Option<StoredDepFileState>;
    /// Remove all entries. May block for as long as `flush` does.
    fn clear(&self);
    /// Writes accepted but not yet applied to the database. Reported per snapshot: the queue is
    /// unbounded, so sustained growth is the signal that the writer is not keeping up.
    fn queue_size(&self) -> u64 {
        0
    }
    /// Writes applied so far, the time spent applying them, and the slowest single one, all
    /// cumulative for the store's life. Reported per snapshot alongside `queue_size`, which says
    /// how far behind the writer is but not what it costs.
    fn write_stats(&self) -> DepFileWriteStats {
        DepFileWriteStats::default()
    }
    /// The same for reads, cumulative for the store's life.
    fn read_stats(&self) -> DepFileReadStats {
        DepFileReadStats::default()
    }
    /// Record that a persisted entry was actually served, i.e. it survived `check_action` and its
    /// outputs were still materialized. Reported by the caller because the store cannot know:
    /// `get_entry` returning a row only means the row exists, and `fetches_found` counts that.
    fn note_persisted_hit(&self) {}
    /// Measures the database size in number of records and disk footprint.
    ///
    /// Blocking, and for as long as the disk takes, so callers keep it off any path that is waited
    /// on.
    fn db_size(&self) -> Option<DepFileDbSize> {
        None
    }
    /// Block until every write issued so far has been applied. Called at the end of a command so
    /// that a daemon restart afterwards sees everything the command produced. Implementations that
    /// write synchronously need do nothing.
    ///
    /// An implementation that defers its writes blocks here for however long draining them takes,
    /// which is unbounded. Call this (and `clear`) from `spawn_blocking` rather than directly from
    /// an async context, or it will park a runtime worker.
    fn flush(&self) {}
}

struct DepFileStoreHolder(Option<Arc<dyn DepFileStore>>);

pub trait SetDepFileStore {
    fn set_dep_file_store(&mut self, store: Option<Arc<dyn DepFileStore>>);
}

pub trait HasDepFileStore {
    fn get_dep_file_store(&self) -> Option<&dyn DepFileStore>;
}

impl SetDepFileStore for UserComputationData {
    fn set_dep_file_store(&mut self, store: Option<Arc<dyn DepFileStore>>) {
        self.data.set(DepFileStoreHolder(store));
    }
}

impl HasDepFileStore for UserComputationData {
    fn get_dep_file_store(&self) -> Option<&dyn DepFileStore> {
        self.data
            .get::<DepFileStoreHolder>()
            .ok()
            .and_then(|holder| holder.0.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDepFileStore(u64);

    impl DepFileStore for TestDepFileStore {
        fn insert(&self, _logical_key: Vec<u8>, _config_key: Vec<u8>, _state: StoredDepFileState) {}

        fn delete(&self, _logical_key: Vec<u8>, _config_key: Vec<u8>) {}

        fn get_digests(&self, _logical_key: &[u8]) -> Vec<StoredDepFileDigests> {
            Vec::new()
        }

        fn get_entry(&self, _id: i64) -> Option<StoredDepFileState> {
            None
        }

        fn clear(&self) {}

        fn queue_size(&self) -> u64 {
            self.0
        }
    }

    #[test]
    fn dep_file_store_is_attached_to_user_computation_data() {
        let mut data = UserComputationData::default();
        assert!(data.get_dep_file_store().is_none());

        data.set_dep_file_store(Some(Arc::new(TestDepFileStore(7))));

        assert_eq!(
            Some(7),
            data.get_dep_file_store().map(|store| store.queue_size())
        );
    }
}
