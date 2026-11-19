/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use futures::FutureExt;

use crate::re::presence::PresenceStats;

#[derive(Default)]
pub struct RemoteExecutionClientOpStats {
    pub started: u32,
    pub finished_successfully: u32,
    pub finished_with_error: u32,
}

impl From<&'_ OpStats> for RemoteExecutionClientOpStats {
    fn from(stats: &OpStats) -> RemoteExecutionClientOpStats {
        RemoteExecutionClientOpStats {
            started: stats.started.load(Ordering::Relaxed),
            finished_successfully: stats.finished_successfully.load(Ordering::Relaxed),
            finished_with_error: stats.finished_with_error.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
pub struct RemoteExecutionClientStats {
    /// In bytes.
    pub uploaded: u64,
    /// In bytes.
    pub downloaded: u64,

    pub upload_stats: PerBackendRemoteExecutionClientStats,
    pub download_stats: PerBackendRemoteExecutionClientStats,

    // Per per-operation stats tracked below.
    pub uploads: RemoteExecutionClientOpStats,
    pub downloads: RemoteExecutionClientOpStats,
    pub action_cache: RemoteExecutionClientOpStats,
    pub executes: RemoteExecutionClientOpStats,
    pub materializes: RemoteExecutionClientOpStats,
    pub write_action_results: RemoteExecutionClientOpStats,
    pub get_digest_expirations: RemoteExecutionClientOpStats,

    pub presence: PresenceStats,

    // Local cache hits and misses stats
    pub local_cache: LocalCacheRemoteExecutionClientStats,
}

#[derive(Default, Allocative)]
pub(super) struct OpStats {
    started: AtomicU32,
    finished_successfully: AtomicU32,
    finished_with_error: AtomicU32,
}

impl OpStats {
    pub(super) fn op<'a, R, F>(&'a self, f: F) -> impl Future<Output = buck2_error::Result<R>> + 'a
    where
        F: Future<Output = buck2_error::Result<R>> + 'a,
    {
        // We avoid using `async fn` or `async move` here to avoid doubling the
        // future size. See https://github.com/rust-lang/rust/issues/62958
        self.started.fetch_add(1, Ordering::Relaxed);
        f.map(|result| {
            (if result.is_ok() {
                &self.finished_successfully
            } else {
                &self.finished_with_error
            })
            .fetch_add(1, Ordering::Relaxed);
            result
        })
    }
}

#[derive(Default)]
pub struct PerBackendRemoteExecutionClientStats {
    pub zdb: BackendStats,
    pub zgateway: BackendStats,
    pub manifold: BackendStats,
    pub hedwig: BackendStats,
}

#[derive(Default)]
pub struct BackendStats {
    pub queries: u64,
    pub bytes: u64,
}

impl PerBackendRemoteExecutionClientStats {
    pub fn fill_from_re_client_metrics(&mut self, metrics: &remote_execution::TStorageStats) {
        #[cfg(fbcode_build)]
        {
            for (typ, re_stats) in metrics.per_backend_stats.iter() {
                let stats = match *typ {
                    remote_execution::TStorageBackendType::ZDB => &mut self.zdb,
                    remote_execution::TStorageBackendType::ZGATEWAY => &mut self.zgateway,
                    remote_execution::TStorageBackendType::MANIFOLD => &mut self.manifold,
                    remote_execution::TStorageBackendType::HEDWIG => &mut self.hedwig,
                    _ => continue,
                };
                stats.queries = re_stats.queries_count as _;
                stats.bytes = re_stats.bytes as _;
            }
        }

        #[cfg(not(fbcode_build))]
        {
            let _unused = metrics;
        }
    }
}

#[derive(Default, Allocative)]
pub(super) struct LocalCacheStats {
    hits_files: AtomicI64,
    hits_bytes: AtomicI64,
    misses_files: AtomicI64,
    misses_bytes: AtomicI64,
    hits_from_memory: AtomicI64,
    hits_from_fs: AtomicI64,
    cache_lookups: AtomicI64,
    cache_lookup_latency_microseconds: AtomicI64,
}

impl LocalCacheStats {
    pub(super) fn update(&self, stat: &remote_execution::TLocalCacheStats) {
        self.hits_files
            .fetch_add(stat.hits_files, Ordering::Relaxed);
        self.hits_bytes
            .fetch_add(stat.hits_bytes, Ordering::Relaxed);
        self.misses_files
            .fetch_add(stat.misses_files, Ordering::Relaxed);
        self.misses_bytes
            .fetch_add(stat.misses_bytes, Ordering::Relaxed);
        self.hits_from_memory.fetch_add(
            stat.cache_funnel_stats.digests_served_from_memory,
            Ordering::Relaxed,
        );
        self.hits_from_fs.fetch_add(
            stat.cache_funnel_stats.digests_served_from_fs,
            Ordering::Relaxed,
        );
        self.cache_lookups
            .fetch_add(stat.total_cache_lookup_attempts, Ordering::Relaxed);
        self.cache_lookup_latency_microseconds
            .fetch_add(stat.cache_lookup_latency_microseconds, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct LocalCacheRemoteExecutionClientStats {
    pub hits_files: i64,
    pub hits_bytes: i64,
    pub misses_files: i64,
    pub misses_bytes: i64,
    pub hits_from_memory: i64,
    pub hits_from_fs: i64,
    pub cache_lookups: i64,
    pub cache_lookup_latency_microseconds: i64,
}

impl From<&'_ LocalCacheStats> for LocalCacheRemoteExecutionClientStats {
    fn from(stats: &LocalCacheStats) -> LocalCacheRemoteExecutionClientStats {
        LocalCacheRemoteExecutionClientStats {
            hits_files: stats.hits_files.load(Ordering::Relaxed),
            hits_bytes: stats.hits_bytes.load(Ordering::Relaxed),
            misses_files: stats.misses_files.load(Ordering::Relaxed),
            misses_bytes: stats.misses_bytes.load(Ordering::Relaxed),
            hits_from_memory: stats.hits_from_memory.load(Ordering::Relaxed),
            hits_from_fs: stats.hits_from_fs.load(Ordering::Relaxed),
            cache_lookups: stats.cache_lookups.load(Ordering::Relaxed),
            cache_lookup_latency_microseconds: stats
                .cache_lookup_latency_microseconds
                .load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::future;

    use super::*;

    /// What `re_state.rs` shows as "in progress".
    fn in_progress(stats: &RemoteExecutionClientOpStats) -> u32 {
        stats
            .started
            .saturating_sub(stats.finished_successfully)
            .saturating_sub(stats.finished_with_error)
    }

    /// An operation whose future is dropped before it completes (the losing side of a hybrid
    /// execution, Ctrl-C, DICE cancellation) is counted as started but never as finished, so
    /// the session stats show it as in progress for the rest of the daemon's life.
    #[tokio::test]
    async fn test_dropped_op_stays_in_progress() {
        let stats = OpStats::default();
        stats
            .op(future::ready(Ok::<_, buck2_error::Error>(())))
            .await
            .unwrap();
        stats
            .op(future::ready(Err::<(), _>(buck2_error::internal_error!(
                "boom"
            ))))
            .await
            .unwrap_err();
        drop(stats.op(future::pending::<buck2_error::Result<()>>()));

        let exported = RemoteExecutionClientOpStats::from(&stats);
        assert_eq!(exported.started, 3);
        assert_eq!(exported.finished_successfully, 1);
        assert_eq!(exported.finished_with_error, 1);
        assert_eq!(in_progress(&exported), 1);
    }
}
