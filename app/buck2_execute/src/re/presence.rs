/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Bounded, batched presence checks against the CAS: "does the CAS have this blob, and until
//! when?", asked at most once per blob however many callers ask, and answered from the digest
//! itself when it already knows.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use jiff::SignedDuration;
use jiff::Timestamp;
use remote_execution::GetDigestsTtlResponse;
use remote_execution::TDigest;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::digest::CasDigestToReExt;
use crate::re::ttl::re_expiration_from_ttl;

/// Whether a presence check may report a miss out of the CAS's negative cache, which can be
/// stale for a blob written moments ago.
#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq, Hash)]
pub enum NegativeCache {
    /// A miss may come from the negative cache. Correct for callers that upload or download on a
    /// miss, which then merely do redundant work.
    Allowed,
    /// A strongly consistent read decides before a miss is reported. Required when a miss is an
    /// error, or is recorded as the blob's expiration.
    Bypassed,
}

impl NegativeCache {
    /// The RPC request flag this maps to: `is_for_upload` is what lets the CAS answer from the
    /// negative cache.
    pub fn is_for_upload(self) -> bool {
        match self {
            NegativeCache::Allowed => true,
            NegativeCache::Bypassed => false,
        }
    }

    fn lane(self) -> usize {
        match self {
            NegativeCache::Allowed => 0,
            NegativeCache::Bypassed => 1,
        }
    }
}

/// The RPC a presence check issues. Abstracted so the batching can be tested without RE.
#[async_trait]
pub trait TtlBackend: Send + Sync + 'static {
    /// Asks the CAS, under `use_case`, how long it will keep each of `digests`.
    async fn get_digests_ttl(
        &self,
        digests: Vec<TDigest>,
        use_case: RemoteExecutorUseCase,
        negative_cache: NegativeCache,
    ) -> buck2_error::Result<GetDigestsTtlResponse>;
}

/// An expiration this far out or further is taken at its word: a digest that already knows one
/// is answered without asking the CAS, and a caller that leaves a blob to the TTL refresher can
/// rely on the blob being there when the refresher next looks. The refresher runs every 30
/// minutes by default and extends everything closer to expiring than this, so a blob is asked
/// about at least once before it expires as long as a pass finishes within the other 15.
pub const HEADROOM: SignedDuration = SignedDuration::from_mins(45);

/// The expiration the CAS reports for a blob, or `None` when it does not have the blob.
pub type Presence = Option<Timestamp>;

type Answer = buck2_error::Result<Presence>;

#[derive(Default, Allocative)]
struct Counters {
    rpcs: AtomicU64,
    digests_queried: AtomicU64,
    digest_answers: AtomicU64,
    largest_batch: AtomicU64,
}

/// A reading of the counters. `largest_batch` covers the interval since the previous reading.
#[derive(Default, Debug, Clone, Copy)]
pub struct PresenceStats {
    pub rpcs: u64,
    pub digests_queried: u64,
    /// Digests answered from an expiration they already knew, without an RPC.
    pub digest_answers: u64,
    pub largest_batch: u64,
}

/// Coalesces presence checks into bounded RPCs. There is one instance per process
/// ([`CasPresence::global`]): its RPC bound holds across every RE client and connection the
/// daemon creates, and its counters outlive any connection.
///
/// The batching state belongs to one task, which every check talks to over a channel; RPCs are
/// spawned by that task and report back to it the same way. Nothing here is shared or locked.
#[derive(Allocative)]
pub struct CasPresence {
    #[allocative(skip)]
    tx: mpsc::UnboundedSender<Message>,
    counters: Arc<Counters>,
}

static GLOBAL: OnceLock<Arc<CasPresence>> = OnceLock::new();

impl CasPresence {
    /// The process-wide instance. The first call sizes it with `concurrent_rpcs` RPCs in flight
    /// and `max_batch` digests per RPC; later calls return the same instance whatever they pass.
    /// Buck2 runs one daemon per process and reads the knobs once per daemon, so there is only
    /// ever one value of each.
    pub fn global(concurrent_rpcs: usize, max_batch: usize) -> &'static Arc<CasPresence> {
        GLOBAL.get_or_init(|| Arc::new(CasPresence::new(concurrent_rpcs, max_batch)))
    }

    /// Counters of the process-wide instance; `None` before anything has used it. Reading resets
    /// `largest_batch`, see [`CasPresence::stats`].
    pub fn global_stats() -> Option<PresenceStats> {
        GLOBAL.get().map(|presence| presence.stats())
    }

    /// `concurrent_rpcs` bounds the RPCs in flight across all use cases and both negative-cache
    /// policies. Must be called from within a tokio runtime, which the batching task then lives
    /// on.
    /// `max_batch` is how many digests one RPC carries at most; the internal CAS client accepts
    /// up to 500.
    pub fn new(concurrent_rpcs: usize, max_batch: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let counters = Arc::new(Counters::default());
        tokio::spawn(
            Batcher {
                rx,
                tx: tx.clone(),
                max_rpcs: concurrent_rpcs.max(1),
                max_batch: max_batch.max(1),
                rpcs: 0,
                pending: BuckMutMap::default(),
                order: Default::default(),
                in_flight: BuckMutMap::default(),
                requests: BuckMutMap::default(),
                next_request: 0,
                next_seq: 0,
                counters: counters.dupe(),
            }
            .run(),
        );
        Self { tx, counters }
    }

    /// One answer per digest, in order. A digest that already knows an expiration beyond
    /// [`HEADROOM`] is answered from that; every other one is asked about, and what the CAS says
    /// is stamped onto the digest, which is the interned entry every holder of that blob shares.
    ///
    /// Requests for the same blob share one RPC, RPCs carry at most as many digests as the
    /// client is configured for (`buck2_re_client.presence_check_max_batch`), and only as many
    /// run at once as it is configured for too; when they are all busy, new
    /// requests queue up and leave together, so batching only happens under load and a lone
    /// request goes out immediately. Queued digests leave in arrival order. A batch goes out
    /// under the use case of its oldest digest's first asker: every caller works in the daemon's
    /// one CAS namespace, so attribution is all a use case decides here, and it is right up to
    /// the coalescing.
    ///
    /// A caller that stops waiting does not stop an RPC already in flight: the answer still
    /// arrives and is stamped, so a check abandoned in a lost race still pays off for the next
    /// caller. A digest still queued when its last asker leaves is not asked. A failed RPC fails
    /// every caller waiting on any digest in it.
    pub async fn check(
        &self,
        backend: Arc<dyn TtlBackend>,
        use_case: RemoteExecutorUseCase,
        negative_cache: NegativeCache,
        digests: Vec<TrackedFileDigest>,
    ) -> buck2_error::Result<Vec<Presence>> {
        let deadline = Timestamp::now() + HEADROOM;
        let mut answers: Vec<Option<Presence>> = vec![None; digests.len()];
        let mut asked = Vec::new();
        let mut asked_at = Vec::new();
        for (i, digest) in digests.into_iter().enumerate() {
            let expires = digest.expires()?;
            if expires > deadline {
                answers[i] = Some(Some(expires));
            } else {
                asked.push(digest);
                asked_at.push(i);
            }
        }
        self.counters
            .digest_answers
            .fetch_add((answers.len() - asked.len()) as u64, Ordering::Relaxed);

        if !asked.is_empty() {
            let (reply, answer) = oneshot::channel();
            self.tx
                .send(Message::Check(CheckRequest {
                    digests: asked,
                    negative_cache,
                    use_case,
                    backend,
                    reply,
                }))
                .map_err(|_| internal_error!("presence check task is gone"))?;
            let asked = answer
                .await
                .map_err(|_| internal_error!("presence check finished without answering"))??;
            for (i, presence) in asked_at.into_iter().zip(asked) {
                answers[i] = Some(presence);
            }
        }

        Ok(answers
            .into_iter()
            .map(|answer| answer.expect("every digest was answered from itself or by the task"))
            .collect())
    }

    /// Reading resets `largest_batch`, so this has exactly one reader: the snapshot collector.
    pub fn stats(&self) -> PresenceStats {
        PresenceStats {
            rpcs: self.counters.rpcs.load(Ordering::Relaxed),
            digests_queried: self.counters.digests_queried.load(Ordering::Relaxed),
            digest_answers: self.counters.digest_answers.load(Ordering::Relaxed),
            largest_batch: self.counters.largest_batch.swap(0, Ordering::Relaxed),
        }
    }

    /// How many digests are queued and how many are inside an RPC right now.
    #[cfg(test)]
    async fn queued(&self) -> (usize, usize) {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Message::Queued(tx)).unwrap();
        rx.await.unwrap()
    }
}

enum Message {
    Check(CheckRequest),
    /// An RPC ended, with what the CAS said for the keys it carried.
    Done {
        keys: Vec<Key>,
        asked_at: Timestamp,
        result: buck2_error::Result<BuckMutMap<(String, i64), i64>>,
    },
    #[cfg(test)]
    Queued(oneshot::Sender<(usize, usize)>),
}

struct CheckRequest {
    digests: Vec<TrackedFileDigest>,
    negative_cache: NegativeCache,
    use_case: RemoteExecutorUseCase,
    backend: Arc<dyn TtlBackend>,
    reply: oneshot::Sender<buck2_error::Result<Vec<Presence>>>,
}

/// What one RPC answers for: a blob, asked about under one negative-cache policy.
type Key = (FileDigest, NegativeCache);

/// A digest waiting for an RPC or inside one, and who is waiting for its answer.
struct Slot {
    digest: TrackedFileDigest,
    /// Request id and the position in that request.
    waiters: Vec<(u64, usize)>,
    /// The first asker's, which the batch goes out under.
    use_case: RemoteExecutorUseCase,
    backend: Arc<dyn TtlBackend>,
    /// Arrival order across both lanes.
    seq: u64,
}

/// A check the task has not answered yet.
struct Request {
    answers: Vec<Option<Presence>>,
    remaining: usize,
    error: Option<buck2_error::Error>,
    reply: oneshot::Sender<buck2_error::Result<Vec<Presence>>>,
}

struct Batcher {
    rx: mpsc::UnboundedReceiver<Message>,
    /// For the RPC tasks to report back on.
    tx: mpsc::UnboundedSender<Message>,
    max_rpcs: usize,
    max_batch: usize,
    rpcs: usize,
    pending: BuckMutMap<Key, Slot>,
    /// Arrival order, one lane per negative-cache policy; a key is in a lane iff it is pending.
    order: [VecDeque<Key>; 2],
    in_flight: BuckMutMap<Key, Slot>,
    requests: BuckMutMap<u64, Request>,
    next_request: u64,
    next_seq: u64,
    counters: Arc<Counters>,
}

impl Batcher {
    async fn run(mut self) {
        while let Some(message) = self.rx.recv().await {
            match message {
                Message::Check(request) => self.enqueue(request),
                Message::Done {
                    keys,
                    asked_at,
                    result,
                } => self.finish(keys, asked_at, result),
                #[cfg(test)]
                Message::Queued(reply) => {
                    let _ignored = reply.send((self.pending.len(), self.in_flight.len()));
                }
            }
        }
    }

    fn enqueue(&mut self, request: CheckRequest) {
        let id = self.next_request;
        self.next_request += 1;
        self.requests.insert(
            id,
            Request {
                answers: vec![None; request.digests.len()],
                remaining: request.digests.len(),
                error: None,
                reply: request.reply,
            },
        );
        for (i, digest) in request.digests.into_iter().enumerate() {
            let key = (digest.data().dupe(), request.negative_cache);
            if let Some(slot) = self.in_flight.get_mut(&key) {
                slot.waiters.push((id, i));
            } else if let Some(slot) = self.pending.get_mut(&key) {
                slot.waiters.push((id, i));
            } else {
                let seq = self.next_seq;
                self.next_seq += 1;
                self.order[request.negative_cache.lane()].push_back(key.dupe());
                self.pending.insert(
                    key,
                    Slot {
                        digest,
                        waiters: vec![(id, i)],
                        use_case: request.use_case,
                        backend: request.backend.dupe(),
                        seq,
                    },
                );
            }
        }
        self.dispatch();
    }

    /// Starts RPCs while there is room for them and something to ask, oldest waiter first.
    fn dispatch(&mut self) {
        while self.rpcs < self.max_rpcs {
            let Some(lane) = self.oldest_lane() else {
                return;
            };
            let mut batch = Vec::new();
            while batch.len() < self.max_batch
                && let Some(key) = self.order[lane].pop_front()
            {
                let mut slot = self
                    .pending
                    .remove(&key)
                    .expect("a lane holds pending keys");
                // A caller that stopped waiting is dropped here rather than asked for; the
                // request it belonged to can never be answered, so it goes too.
                slot.waiters.retain(|(id, _)| {
                    let alive = self.requests.get(id).is_some_and(|r| !r.reply.is_closed());
                    if !alive {
                        self.requests.remove(id);
                    }
                    alive
                });
                if !slot.waiters.is_empty() {
                    batch.push((key, slot));
                }
            }
            if !batch.is_empty() {
                self.start_rpc(lane, batch);
            }
        }
    }

    /// The lane whose front has waited longest, if any lane has a front.
    fn oldest_lane(&self) -> Option<usize> {
        (0..self.order.len())
            .filter_map(|lane| {
                let key = self.order[lane].front()?;
                Some((self.pending[key].seq, lane))
            })
            .min()
            .map(|(_, lane)| lane)
    }

    fn start_rpc(&mut self, lane: usize, batch: Vec<(Key, Slot)>) {
        self.rpcs += 1;
        self.counters.rpcs.fetch_add(1, Ordering::Relaxed);
        self.counters
            .digests_queried
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        self.counters
            .largest_batch
            .fetch_max(batch.len() as u64, Ordering::Relaxed);

        let negative_cache = if lane == NegativeCache::Allowed.lane() {
            NegativeCache::Allowed
        } else {
            NegativeCache::Bypassed
        };
        let (use_case, backend) = {
            let first = &batch[0].1;
            (first.use_case, first.backend.dupe())
        };
        let re_digests: Vec<TDigest> = batch.iter().map(|(_, slot)| slot.digest.to_re()).collect();
        let keys: Vec<Key> = batch.iter().map(|(key, _)| key.dupe()).collect();
        for (key, slot) in batch {
            self.in_flight.insert(key, slot);
        }

        let tx = self.tx.clone();
        // Detached on purpose: whoever asked may stop waiting, the answer is still wanted.
        tokio::spawn(async move {
            let asked_at = Timestamp::now();
            let result = backend
                .get_digests_ttl(re_digests, use_case, negative_cache)
                .await
                .map(|response| {
                    response
                        .digests_with_ttl
                        .into_iter()
                        .map(|d| ((d.digest.hash, d.digest.size_in_bytes), d.ttl))
                        .collect::<BuckMutMap<_, _>>()
                });
            let _ignored = tx.send(Message::Done {
                keys,
                asked_at,
                result,
            });
        });
    }

    fn finish(
        &mut self,
        keys: Vec<Key>,
        asked_at: Timestamp,
        result: buck2_error::Result<BuckMutMap<(String, i64), i64>>,
    ) {
        self.rpcs -= 1;
        for key in keys {
            let slot = self
                .in_flight
                .remove(&key)
                .expect("a key stays in flight until its RPC ends");
            let answer: Answer = match &result {
                Err(e) => Err(e.dupe()),
                Ok(ttls) => {
                    let re_digest = slot.digest.to_re();
                    match ttls.get(&(re_digest.hash, re_digest.size_in_bytes)) {
                        None => Err(buck2_error!(
                            buck2_error::ErrorTag::ReInvalidGetCasResponse,
                            "Invalid response from get_digests_ttl: no TTL for `{}`",
                            slot.digest
                        )),
                        Some(&ttl) if ttl > 0 => {
                            let expiration = re_expiration_from_ttl(asked_at, ttl, &slot.digest);
                            slot.digest.update_expires(expiration);
                            Ok(Some(expiration))
                        }
                        Some(_) => Ok(None),
                    }
                }
            };
            for (id, i) in slot.waiters {
                let Some(request) = self.requests.get_mut(&id) else {
                    continue;
                };
                match &answer {
                    Ok(presence) => request.answers[i] = Some(*presence),
                    Err(e) => {
                        request.error.get_or_insert_with(|| e.dupe());
                    }
                }
                request.remaining -= 1;
                if request.remaining == 0 {
                    let request = self.requests.remove(&id).expect("just found");
                    let _ignored = request.reply.send(match request.error {
                        Some(e) => Err(e),
                        None => Ok(request
                            .answers
                            .into_iter()
                            .map(|a| a.expect("every digest of a finished request answered"))
                            .collect()),
                    });
                }
            }
        }
        self.dispatch();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;

    use buck2_common::cas_digest::CasDigestConfig;
    use futures::FutureExt;
    use futures::future::BoxFuture;
    #[cfg(not(fbcode_build))]
    use remote_execution::DigestWithTtl;
    #[cfg(fbcode_build)]
    use remote_execution::TDigestWithTtl as DigestWithTtl;
    use tokio::sync::Notify;

    use super::*;

    fn config() -> CasDigestConfig {
        CasDigestConfig::testing_default()
    }

    /// A digest of content no other call produces. Digests are interned process-wide and a check
    /// stamps the interned entry, which the expiration then pins past every handle to it, so a
    /// content string two tests share would have the second one answered from the first one's
    /// stamp instead of the RPC it is waiting for.
    fn digest(name: &str) -> TrackedFileDigest {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        TrackedFileDigest::from_content(format!("{name}-{unique}").as_bytes(), config())
    }

    fn use_case() -> RemoteExecutorUseCase {
        RemoteExecutorUseCase::buck2_default()
    }

    /// Answers every digest with `ttl(digest)`, records every call, and blocks the first `hold`
    /// calls until `release` is called.
    struct Stub {
        ttl: Box<dyn Fn(&TDigest) -> i64 + Send + Sync>,
        fail: AtomicBool,
        calls: Mutex<Vec<(RemoteExecutorUseCase, NegativeCache, Vec<TDigest>)>>,
        started: Notify,
        hold: AtomicUsize,
        release: Notify,
    }

    impl Stub {
        fn new(ttl: impl Fn(&TDigest) -> i64 + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(Self {
                ttl: Box::new(ttl),
                fail: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
                started: Notify::new(),
                hold: AtomicUsize::new(0),
                release: Notify::new(),
            })
        }

        fn failing(self: Arc<Self>) -> Arc<Self> {
            self.fail.store(true, Ordering::Relaxed);
            self
        }

        fn holding(self: Arc<Self>, calls: usize) -> Arc<Self> {
            self.hold.store(calls, Ordering::Relaxed);
            self
        }

        /// The digests of every call, in order.
        fn calls(&self) -> Vec<Vec<TDigest>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(_, _, digests)| digests.clone())
                .collect()
        }

        fn calls_with_use_case(&self) -> Vec<(RemoteExecutorUseCase, Vec<TDigest>)> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(use_case, _, digests)| (*use_case, digests.clone()))
                .collect()
        }

        fn policies(&self) -> Vec<NegativeCache> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(_, policy, _)| *policy)
                .collect()
        }
    }

    #[async_trait]
    impl TtlBackend for Stub {
        async fn get_digests_ttl(
            &self,
            digests: Vec<TDigest>,
            use_case: RemoteExecutorUseCase,
            negative_cache: NegativeCache,
        ) -> buck2_error::Result<GetDigestsTtlResponse> {
            self.calls
                .lock()
                .unwrap()
                .push((use_case, negative_cache, digests.clone()));
            self.started.notify_one();
            if self
                .hold
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                .is_ok()
            {
                self.release.notified().await;
            }
            if self.fail.load(Ordering::Relaxed) {
                return Err(internal_error!("injected failure"));
            }
            #[allow(clippy::needless_update)] // Defaults are needed internally but not in OSS
            let response = GetDigestsTtlResponse {
                digests_with_ttl: digests
                    .into_iter()
                    .map(|digest| DigestWithTtl {
                        ttl: (self.ttl)(&digest),
                        digest,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            };
            Ok(response)
        }
    }

    fn check(
        presence: &Arc<CasPresence>,
        stub: &Arc<Stub>,
        digests: Vec<TrackedFileDigest>,
    ) -> BoxFuture<'static, buck2_error::Result<Vec<Presence>>> {
        check_as(presence, stub, use_case(), NegativeCache::Allowed, digests)
    }

    fn check_as(
        presence: &Arc<CasPresence>,
        stub: &Arc<Stub>,
        use_case: RemoteExecutorUseCase,
        negative_cache: NegativeCache,
        digests: Vec<TrackedFileDigest>,
    ) -> BoxFuture<'static, buck2_error::Result<Vec<Presence>>> {
        let presence = presence.dupe();
        let backend: Arc<dyn TtlBackend> = stub.dupe();
        async move {
            presence
                .check(backend, use_case, negative_cache, digests)
                .await
        }
        .boxed()
    }

    async fn wait_for_pending(presence: &CasPresence, n: usize) {
        while presence.queued().await.0 < n {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn test_answers_from_the_digest_without_an_rpc() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let stub = Stub::new(|_| 3600);
        let fresh = TrackedFileDigest::new_expires(
            digest("fresh").data().dupe(),
            Timestamp::now() + HEADROOM + SignedDuration::from_mins(1),
            config(),
        );
        let stale = TrackedFileDigest::new_expires(
            digest("stale").data().dupe(),
            Timestamp::now() + HEADROOM - SignedDuration::from_mins(1),
            config(),
        );

        let answers = check(&presence, &stub, vec![fresh.dupe(), stale.dupe()]).await?;
        assert_eq!(answers[0], Some(fresh.expires()?));
        assert!(answers[1].is_some());
        assert_eq!(stub.calls(), vec![vec![stale.to_re()]]);
        assert_eq!(presence.stats().digest_answers, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_results_follow_input_order_and_stamp_the_digest() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(4, 50));
        let (present, missing) = (digest("present"), digest("missing"));
        let present_hash = present.to_re().hash;
        let stub = Stub::new(move |d| if d.hash == present_hash { 3600 } else { -1 });
        // Interned, so the twin is the same entry, and one stamp serves both holders.
        let twin = TrackedFileDigest::new(present.data().dupe(), config());
        assert!(twin.ptr_eq(&present));

        let answers = check(
            &presence,
            &stub,
            vec![missing.dupe(), present.dupe(), twin.dupe()],
        )
        .await?;
        assert_eq!(answers[0], None);
        let expiration = answers[1].expect("present");
        assert_eq!(answers[2], Some(expiration));
        // Digests record whole seconds.
        assert_eq!(present.expires()?.as_second(), expiration.as_second());
        assert_eq!(twin.expires()?.as_second(), expiration.as_second());
        assert_eq!(
            missing.expires()?,
            Timestamp::UNIX_EPOCH,
            "a miss stamps nothing"
        );
        // One RPC, the duplicated digest asked once.
        assert_eq!(stub.calls(), vec![vec![missing.to_re(), present.to_re()]]);
        Ok(())
    }

    #[tokio::test]
    async fn test_batches_while_the_rpcs_are_busy() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let stub = Stub::new(|_| 3600).holding(1);
        let (a, b, c) = (digest("a"), digest("b"), digest("c"));

        let first = tokio::spawn(check(&presence, &stub, vec![a.dupe()]));
        stub.started.notified().await;
        // The one RPC is busy, so these queue up behind it...
        let second = tokio::spawn(check(&presence, &stub, vec![b.dupe()]));
        let third = tokio::spawn(check(&presence, &stub, vec![c.dupe(), b.dupe()]));
        wait_for_pending(&presence, 2).await;
        // ...and leave together, deduplicated, once it is done.
        stub.release.notify_one();
        first.await.unwrap()?;
        second.await.unwrap()?;
        third.await.unwrap()?;
        assert_eq!(
            stub.calls(),
            vec![vec![a.to_re()], vec![b.to_re(), c.to_re()]]
        );
        assert_eq!(presence.stats().largest_batch, 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_a_cancelled_caller_does_not_cancel_the_rpc() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let stub = Stub::new(|_| 3600).holding(1);
        let a = digest("cancelled");

        let first = tokio::spawn(check(&presence, &stub, vec![a.dupe()]));
        stub.started.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        // A second caller attaches to the in-flight RPC rather than issuing another.
        let second = tokio::spawn(check(&presence, &stub, vec![a.dupe()]));
        while presence.queued().await.1 < 1 {
            tokio::task::yield_now().await;
        }
        stub.release.notify_one();
        let answers = second.await.unwrap()?;
        assert!(answers[0].is_some());
        assert_eq!(stub.calls().len(), 1);
        assert_eq!(
            a.expires()?.as_second(),
            answers[0].unwrap().as_second(),
            "stamped despite the abort"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_a_digest_nobody_waits_for_any_more_is_not_asked() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let stub = Stub::new(|_| 3600).holding(1);
        let (a, b, c) = (digest("in flight"), digest("abandoned"), digest("after"));

        let first = tokio::spawn(check(&presence, &stub, vec![a.dupe()]));
        stub.started.notified().await;
        let second = tokio::spawn(check(&presence, &stub, vec![b.dupe()]));
        wait_for_pending(&presence, 1).await;
        second.abort();
        assert!(second.await.unwrap_err().is_cancelled());

        stub.release.notify_one();
        first.await.unwrap()?;
        // `b` was still queued when its only asker left, so the next RPC does not carry it.
        check(&presence, &stub, vec![c.dupe()]).await?;
        assert_eq!(stub.calls(), vec![vec![a.to_re()], vec![c.to_re()]]);
        Ok(())
    }

    #[tokio::test]
    async fn test_a_failed_rpc_fails_only_its_batch() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let failing = Stub::new(|_| 3600).failing();
        let a = digest("fails");

        let err = check(&presence, &failing, vec![a.dupe()])
            .await
            .expect_err("the RPC failed");
        assert!(format!("{err:#}").contains("injected failure"));
        assert_eq!(a.expires()?, Timestamp::UNIX_EPOCH);

        // Nothing is left behind: the next check asks again and succeeds.
        let working = Stub::new(|_| 3600);
        let answers = check(&presence, &working, vec![a.dupe()]).await?;
        assert!(answers[0].is_some());
        assert_eq!(working.calls().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_a_batch_goes_out_under_its_oldest_askers_use_case() -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let stub = Stub::new(|_| 3600).holding(1);
        let (first_uc, second_uc) = (
            RemoteExecutorUseCase::new("first".to_owned()),
            RemoteExecutorUseCase::new("second".to_owned()),
        );
        let (a0, a1, b1) = (digest("a0"), digest("a1"), digest("b1"));

        let first = tokio::spawn(check_as(
            &presence,
            &stub,
            first_uc,
            NegativeCache::Allowed,
            vec![a0.dupe()],
        ));
        stub.started.notified().await;
        let second = tokio::spawn(check_as(
            &presence,
            &stub,
            second_uc,
            NegativeCache::Allowed,
            vec![b1.dupe()],
        ));
        wait_for_pending(&presence, 1).await;
        let third = tokio::spawn(check_as(
            &presence,
            &stub,
            first_uc,
            NegativeCache::Allowed,
            vec![a1.dupe()],
        ));
        wait_for_pending(&presence, 2).await;
        stub.release.notify_one();
        first.await.unwrap()?;
        second.await.unwrap()?;
        third.await.unwrap()?;

        // One batch for both queued digests, attributed to `b1`'s asker, who waited longest.
        assert_eq!(
            stub.calls_with_use_case(),
            vec![
                (first_uc, vec![a0.to_re()]),
                (second_uc, vec![b1.to_re(), a1.to_re()])
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_policies_do_not_share_an_rpc_and_are_served_oldest_first()
    -> buck2_error::Result<()> {
        let presence = Arc::new(CasPresence::new(1, 50));
        let stub = Stub::new(|_| 3600).holding(2);
        let (a0, a1, a2, b1) = (digest("a0"), digest("a1"), digest("a2"), digest("b1"));

        let first = tokio::spawn(check(&presence, &stub, vec![a0.dupe()]));
        stub.started.notified().await;
        let second = tokio::spawn(check(&presence, &stub, vec![a1.dupe()]));
        wait_for_pending(&presence, 1).await;
        let third = tokio::spawn(check_as(
            &presence,
            &stub,
            use_case(),
            NegativeCache::Bypassed,
            vec![b1.dupe()],
        ));
        wait_for_pending(&presence, 2).await;
        // `a1` has waited longer than `b1`, so its lane goes next...
        stub.release.notify_one();
        stub.started.notified().await;
        // ...and a newcomer on that lane does not jump ahead of `b1`.
        let fourth = tokio::spawn(check(&presence, &stub, vec![a2.dupe()]));
        wait_for_pending(&presence, 2).await;
        stub.release.notify_one();
        first.await.unwrap()?;
        second.await.unwrap()?;
        third.await.unwrap()?;
        fourth.await.unwrap()?;
        assert_eq!(
            stub.calls(),
            vec![
                vec![a0.to_re()],
                vec![a1.to_re()],
                vec![b1.to_re()],
                vec![a2.to_re()]
            ]
        );
        assert_eq!(
            stub.policies(),
            vec![
                NegativeCache::Allowed,
                NegativeCache::Allowed,
                NegativeCache::Bypassed,
                NegativeCache::Allowed
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_a_short_response_fails_the_omitted_digests() {
        struct Short;
        #[async_trait]
        impl TtlBackend for Short {
            async fn get_digests_ttl(
                &self,
                _digests: Vec<TDigest>,
                _use_case: RemoteExecutorUseCase,
                _negative_cache: NegativeCache,
            ) -> buck2_error::Result<GetDigestsTtlResponse> {
                Ok(GetDigestsTtlResponse::default())
            }
        }
        let presence = Arc::new(CasPresence::new(1, 50));
        let err = presence
            .check(
                Arc::new(Short),
                use_case(),
                NegativeCache::Bypassed,
                vec![digest("omitted")],
            )
            .await
            .expect_err("no TTL came back");
        assert!(format!("{err:#}").contains("no TTL for"));
    }
}
