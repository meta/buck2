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

use dice_futures::oneshot::Stored;

use crate::core::graph::ComputedValueUpdate;
use crate::core::internals::ActorState;
use crate::core::state::CoreStateHandle;
use crate::core::state::QueueCounters;
use crate::core::state::StateRequest;
use crate::core::state::UnclaimedBranch;
use crate::core::state::WeakCoreStateHandle;
use crate::epoch::evaluator::VersionState;
use crate::storage::PagingSetup;

/// How many requests the actor takes off its queue before it answers any of them. Bounds how
/// long the first requester of a batch waits for the last to be processed, and how far behind the
/// queue depth counter runs.
const BATCH_SIZE: usize = 64;

/// Batches smaller than this are sent from this thread: handing a batch to a worker costs about
/// as much as sending one reply, and a short queue means this thread is not the bottleneck
/// anyway.
pub(super) const DISPATCH_MIN: usize = 8;

pub(super) struct StateProcessor {
    state: ActorState,
    rx: tokio::sync::mpsc::UnboundedReceiver<StateRequest>,
    /// Shared with the matching `CoreStateHandle`; this thread bumps the
    /// `retired` counter after each batch of receives.
    counters: Arc<QueueCounters>,
    handle: WeakCoreStateHandle,
    /// Where replies are sent from, see `Replies`. `None` when the actor was started outside a
    /// runtime, in which case this thread sends them itself.
    rt: Option<tokio::runtime::Handle>,
}

impl StateProcessor {
    pub(super) fn spawn(paging: Option<PagingSetup>) -> CoreStateHandle {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = ActorState::new(paging);
        let counters = Arc::new(QueueCounters::new());
        let handle = CoreStateHandle::new(tx, counters.clone());

        let processor = StateProcessor {
            state,
            rx,
            counters,
            handle: handle.downgrade(),
            rt: tokio::runtime::Handle::try_current().ok(),
        };
        std::thread::Builder::new()
            .name("buck2-dice".to_owned())
            .spawn(move || processor.event_loop())
            .unwrap();

        handle
    }

    fn event_loop(mut self) {
        loop {
            let mut replies = Replies::default();
            let mut taken = 0;
            // Skip tokio scheduling.
            while taken < BATCH_SIZE
                && let Ok(message) = self.rx.try_recv()
            {
                taken += 1;
                self.iteration(message, &mut replies);
            }
            if taken == 0 {
                let Some(message) = self.rx.blocking_recv() else {
                    break;
                };
                taken = 1;
                self.iteration(message, &mut replies);
            }
            self.counters.record_retire(taken);
            self.dispatch(replies);
        }
    }

    /// Wakes the requesters from a runtime worker, where that is cheaper than doing it here: see
    /// `Replies`.
    fn dispatch(&self, replies: Replies) {
        match &self.rt {
            Some(rt) if replies.0.len() >= DISPATCH_MIN => {
                drop(rt.spawn(async move { drop(replies) }))
            }
            _ => drop(replies),
        }
    }

    fn iteration(&mut self, message: StateRequest, replies: &mut Replies) {
        match message {
            StateRequest::UpdateState {
                branch,
                changes,
                resp,
            } => {
                // ignore error if the requester dropped it.
                resp.send(self.state.update_state(branch, changes), replies);
            }
            StateRequest::CtxAtVersion {
                version,
                guard,
                resp,
            } => {
                let cache = self.state.ctx_at_version(version);

                let ctx = VersionState::new(version, cache);
                resp.send((ctx, guard), replies);
            }
            StateRequest::DropCtxAtVersion { version } => self.state.drop_ctx_at_version(version),
            StateRequest::CurrentVersion { branch, resp } => {
                // ignore error if the requester dropped it.
                resp.send(self.state.current_version(branch), replies);
            }
            StateRequest::Fork { from, resp } => {
                // An answer the requester dropped deletes its branch again.
                let branch = UnclaimedBranch::new(self.state.fork(from), self.handle.clone());
                resp.send(branch, replies);
            }
            StateRequest::NewRoot { resp } => {
                let branch = UnclaimedBranch::new(self.state.new_root(), self.handle.clone());
                resp.send(branch, replies);
            }
            StateRequest::DeleteBranch { branch } => self.state.delete_branch(branch),
            StateRequest::LookupKey { key, resp } => resp.send(self.state.lookup_key(key), replies),
            StateRequest::RecoveryUntracked { key, resp } => {
                resp.send(self.state.recovery_untracked(key), replies);
            }
            StateRequest::UpdateComputed {
                key,
                storage,
                value,
                deps,
                epsilon,
                invalidation_sources,
                resp,
            } => {
                // ignore error if the requester dropped it.
                resp.send(
                    self.state.update_computed(
                        key,
                        storage,
                        ComputedValueUpdate {
                            value,
                            deps,
                            epsilon,
                        },
                        invalidation_sources,
                    ),
                    replies,
                );
            }
            StateRequest::Revalidate {
                key,
                storage,
                candidate,
                invalidation_sources,
                resp,
            } => {
                // ignore error if the requester dropped it.
                resp.send(
                    self.state
                        .revalidate(key, storage, candidate, invalidation_sources),
                    replies,
                );
            }
            StateRequest::IdleStatus { branch, resp } => {
                resp.send(self.state.idle_status(branch), replies);
            }
            StateRequest::RunningTasks { branch, resp } => {
                resp.send(self.state.running_tasks(branch), replies);
            }
            StateRequest::PagedOutKeys { resp } => {
                resp.send(Ok(self.state.paged_out_keys()), replies);
            }
            StateRequest::PagableStatus { resp } => {
                resp.send(self.state.pagable_status(), replies);
            }
            StateRequest::PagableNodeCounts { resp } => {
                resp.send(self.state.pagable_node_counts(), replies);
            }
            StateRequest::KeysToPageOut { resp } => {
                resp.send(self.state.keys_to_page_out(), replies);
            }
            StateRequest::CommitPageOut {
                keys,
                eviction_policy,
            } => {
                self.state.commit_page_out(keys, eviction_policy);
            }
            StateRequest::BackedSnapshot { resp } => {
                resp.send(self.state.backed_snapshot(), replies);
            }
            StateRequest::EvictBacked { keys, cutoff, resp } => {
                resp.send(self.state.evict_backed(&keys, cutoff), replies);
            }
            StateRequest::MarkNonPageable { keys } => {
                self.state.mark_non_pageable(keys);
            }
            StateRequest::Rehydrate {
                key,
                data_key,
                hydrated,
            } => {
                self.state.rehydrate(key, data_key, hydrated);
            }
            StateRequest::Metrics { resp } => {
                resp.send(self.state.metrics(), replies);
            }
            StateRequest::Introspection { resp } => {
                resp.send(self.state.introspection(), replies);
            }
            StateRequest::MakeAvailableForAllocative { resp } => {
                use std::sync::Arc;

                // The actor blocks on the requester below; nothing the requester might wait on
                // may be held back here meanwhile.
                self.dispatch(std::mem::take(replies));
                let (complete_tx, complete_rx) = dice_futures::oneshot::channel();
                // Placeholder, swapped back below, so it needs no paging setup.
                let state = std::mem::replace(&mut self.state, ActorState::new(None));
                let arc_state = Arc::new(state);
                resp.send((Arc::clone(&arc_state), complete_tx), replies);
                let _ = futures::executor::block_on(complete_rx);
                // Correctness: Contract on `MakeAvailableForAllocative`
                let state =
                    Arc::into_inner(arc_state).expect("Other references to have been dropped");
                self.state = state;
            }
        }
    }
}

/// The wakes owed for a batch of replies, delivered when this is dropped.
///
/// A reply is stored in its channel as the request is processed; what is held back here is waking
/// the task that awaits it. Done from this thread, which is not a runtime worker, every wake goes
/// through the runtime's shared inject queue, under a mutex that every worker parking, unparking
/// or taking from that queue contends for, and unparks a worker when none is searching for work.
/// That cost more than processing the request on the workload this thread bottlenecks, graph walks
/// that revalidate many small keys. Done from a worker, a wake is a push to that worker's local
/// queue and the other workers steal from there, so the batch is moved to a worker as a whole and
/// woken from there, at the cost of a single wake.
///
/// A `Stored` wakes when dropped, so no wake is lost if the task carrying the batch is dropped
/// without running, as happens when the runtime shuts down. Requesters that block a thread are
/// woken by this thread as their request is processed: see `Responder`.
#[derive(Default)]
pub(super) struct Replies(Vec<Stored>);

impl Replies {
    pub(super) fn push(&mut self, reply: Stored) {
        self.0.push(reply);
    }
}
