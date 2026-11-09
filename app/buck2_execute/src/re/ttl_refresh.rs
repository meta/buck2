/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Keeps alive in the CAS every blob that some live [`TrackedFileDigest`] refers to. The walk
//! over the digest interner that finds them is the BlobStore's (`crate::blob_store`); this is
//! the asking, which is what extends a blob's TTL.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::cas_digest::SweepStats;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_error::BuckErrorContext;
use dupe::Dupe;
use itertools::Itertools;
use jiff::Timestamp;

use crate::materialize::materializer::CasDownloadInfo;
use crate::re::manager::ReConnectionHandle;
use crate::re::manager::ReConnectionManager;
use crate::re::presence::HEADROOM;
use crate::re::presence::NegativeCache;
use crate::re::presence::Presence;

/// Where a pass's questions go. Abstracted so a pass can be tested without RE.
pub trait PresenceChecker: Send + Sync + 'static {
    /// Whatever the returned checker holds open (an RE connection) lives for one pass.
    fn begin_pass(&self) -> Box<dyn PassPresenceChecker>;
}

#[async_trait]
pub trait PassPresenceChecker: Send + Sync {
    /// Authoritative presence of `digests`, one answer per digest in order; every answer the CAS
    /// gives is stamped onto the digest asked about, the way `CasPresence::check` does.
    async fn check(&self, digests: Vec<TrackedFileDigest>) -> buck2_error::Result<Vec<Presence>>;
}

/// Asks the CAS through the daemon's RE connection, under the daemon's default use case: the
/// interner has no producer to attribute a blob to, so refresh traffic is the daemon's own.
pub struct ReChecker {
    re_manager: Arc<ReConnectionManager>,
    use_case: RemoteExecutorUseCase,
}

impl ReChecker {
    pub fn new(re_manager: Arc<ReConnectionManager>, use_case: RemoteExecutorUseCase) -> Self {
        Self {
            re_manager,
            use_case,
        }
    }
}

impl PresenceChecker for ReChecker {
    fn begin_pass(&self) -> Box<dyn PassPresenceChecker> {
        // A connection per pass rather than one held for the daemon's lifetime: an idle daemon
        // should still let its RE connection go between passes.
        Box::new(RePassChecker {
            connection: self.re_manager.get_re_connection(),
            use_case: self.use_case,
        })
    }
}

struct RePassChecker {
    connection: ReConnectionHandle,
    use_case: RemoteExecutorUseCase,
}

#[async_trait]
impl PassPresenceChecker for RePassChecker {
    async fn check(&self, digests: Vec<TrackedFileDigest>) -> buck2_error::Result<Vec<Presence>> {
        let client = self.connection.get_client().with_use_case(self.use_case);
        let info = CasDownloadInfo::new_probed(self.use_case);
        client
            .check_presence(digests, NegativeCache::Bypassed, &info)
            .await
    }
}

/// Digests per `check` call. The presence check batches them into RPCs of its own size under its
/// own concurrency bound; this only caps how many answers one pass has outstanding at once.
const CHECK_CHUNK: usize = 5000;

#[derive(Default, Allocative)]
pub struct StandaloneTtlRefreshCounters {
    passes: AtomicU64,
    errors: AtomicU64,
    last_pass_visited: AtomicU64,
    last_pass_swept: AtomicU64,
    last_pass_candidates: AtomicU64,
    last_pass_refreshed: AtomicU64,
    last_pass_gone: AtomicU64,
    last_pass_duration_us: AtomicU64,
}

/// A reading of [`StandaloneTtlRefreshCounters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StandaloneTtlRefreshStats {
    pub passes: u64,
    /// Chunks of a pass whose presence check failed, cumulative.
    pub errors: u64,
    pub last_pass_visited: u64,
    pub last_pass_swept: u64,
    pub last_pass_candidates: u64,
    pub last_pass_refreshed: u64,
    pub last_pass_gone: u64,
    pub last_pass_duration_us: u64,
}

impl StandaloneTtlRefreshCounters {
    pub fn stats(&self) -> StandaloneTtlRefreshStats {
        StandaloneTtlRefreshStats {
            passes: self.passes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_pass_visited: self.last_pass_visited.load(Ordering::Relaxed),
            last_pass_swept: self.last_pass_swept.load(Ordering::Relaxed),
            last_pass_candidates: self.last_pass_candidates.load(Ordering::Relaxed),
            last_pass_refreshed: self.last_pass_refreshed.load(Ordering::Relaxed),
            last_pass_gone: self.last_pass_gone.load(Ordering::Relaxed),
            last_pass_duration_us: self.last_pass_duration_us.load(Ordering::Relaxed),
        }
    }
}

/// Refreshes the CAS TTL of the blobs the BlobStore's walk hands it: every blob a live digest
/// refers to whose recorded expiration is within the presence check's `HEADROOM`. The two agree
/// by construction: a blob the check would vouch for without asking is one this leaves alone,
/// and every other confirmed blob gets asked about, which is what extends it.
pub struct TtlRefresher {
    checker: Arc<dyn PresenceChecker>,
    chunk_size: usize,
    counters: Arc<StandaloneTtlRefreshCounters>,
}

impl TtlRefresher {
    pub fn new(checker: Arc<dyn PresenceChecker>) -> Self {
        Self {
            checker,
            chunk_size: CHECK_CHUNK,
            counters: Arc::new(StandaloneTtlRefreshCounters::default()),
        }
    }

    /// Asks the CAS about `digests` as a pass does, and stamps what it says on them.
    pub async fn check(
        &self,
        digests: Vec<TrackedFileDigest>,
    ) -> buck2_error::Result<Vec<Presence>> {
        self.checker.begin_pass().check(digests).await
    }

    /// Digests per `check` call, for tests that want to see more than one chunk.
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size.max(1);
        self
    }

    pub fn counters(&self) -> Arc<StandaloneTtlRefreshCounters> {
        self.counters.dupe()
    }

    /// The expiration below which a referenced blob is one to ask about now.
    pub fn deadline(now: Timestamp) -> i64 {
        (now + HEADROOM).as_second()
    }

    /// One pass over `candidates`, the handles a walk of the interner picked as referenced and
    /// expiring within `HEADROOM`; `swept` is what that walk saw, for the counters. The CAS
    /// client extends the TTL of what it is asked about, so asking is the refresh; a blob the CAS
    /// no longer has is recorded as gone on its digest. A chunk whose check fails is counted and
    /// skipped, and the pass goes on with the rest; the first such error is returned.
    pub async fn pass(
        &self,
        swept: SweepStats,
        candidates: Vec<TrackedFileDigest>,
    ) -> buck2_error::Result<()> {
        let start = Instant::now();
        let counters = &self.counters;
        let now = Timestamp::now();
        counters
            .last_pass_visited
            .store(swept.visited, Ordering::Relaxed);
        counters
            .last_pass_swept
            .store(swept.swept, Ordering::Relaxed);
        counters
            .last_pass_candidates
            .store(candidates.len() as u64, Ordering::Relaxed);

        let mut refreshed = 0u64;
        let mut gone = 0u64;
        let mut failed_chunks = 0u64;
        let mut first_error = None;
        // A pass with nothing to ask must not open an RE connection: an idle daemon is meant to
        // let its connection go between passes.
        if !candidates.is_empty() {
            let checker = self.checker.begin_pass();
            for chunk in candidates.chunks(self.chunk_size) {
                match checker.check(chunk.to_vec()).await {
                    Ok(answers) => {
                        for (digest, answer) in chunk.iter().zip_eq(answers) {
                            match answer {
                                // Already stamped by the check.
                                Some(_) => refreshed += 1,
                                // An expiration at or before now is how a digest says its blob
                                // is gone, which is what decides whether an artifact can be
                                // fetched again. Such a digest still falls inside the window on
                                // every later pass for as long as something holds it, so gone
                                // blobs are re-asked each pass; that is bounded by the gone
                                // population and is what the materializer's refresher did too.
                                None => {
                                    digest.update_expires(now);
                                    gone += 1;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        failed_chunks += 1;
                        first_error.get_or_insert(e);
                    }
                }
            }
        }

        counters
            .last_pass_refreshed
            .store(refreshed, Ordering::Relaxed);
        counters.last_pass_gone.store(gone, Ordering::Relaxed);
        counters.last_pass_duration_us.store(
            (Instant::now() - start)
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        counters.passes.fetch_add(1, Ordering::Relaxed);
        counters.errors.fetch_add(failed_chunks, Ordering::Relaxed);
        match first_error {
            None => Ok(()),
            Some(e) => Err(e).with_buck_error_context(|| {
                format!(
                    "{failed_chunks} of {} presence checks failed",
                    candidates.len().div_ceil(self.chunk_size)
                )
            }),
        }
    }
}
