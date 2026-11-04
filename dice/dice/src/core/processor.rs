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

use crate::core::graph::ComputedValueUpdate;
use crate::core::internals::ActorState;
use crate::core::state::CoreStateHandle;
use crate::core::state::QueueCounters;
use crate::core::state::StateRequest;
use crate::core::state::UnclaimedBranch;
use crate::core::state::WeakCoreStateHandle;
use crate::epoch::evaluator::VersionState;
use crate::storage::PagingSetup;

pub(super) struct StateProcessor {
    state: ActorState,
    rx: tokio::sync::mpsc::UnboundedReceiver<StateRequest>,
    /// Shared with the matching `CoreStateHandle`; this thread bumps the
    /// `retired` counter after each successful receive.
    counters: Arc<QueueCounters>,
    handle: WeakCoreStateHandle,
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
        };
        std::thread::Builder::new()
            .name("buck2-dice".to_owned())
            .spawn(move || processor.event_loop())
            .unwrap();

        handle
    }

    fn event_loop(mut self) {
        loop {
            // Skip tokio scheduling.
            while let Ok(message) = self.rx.try_recv() {
                self.counters.record_retire();
                self.iteration(message);
            }
            if let Some(message) = self.rx.blocking_recv() {
                self.counters.record_retire();
                self.iteration(message);
            } else {
                break;
            }
        }
    }

    fn iteration(&mut self, message: StateRequest) {
        match message {
            StateRequest::UpdateState {
                branch,
                changes,
                resp,
            } => {
                // ignore error if the requester dropped it.
                let _ = resp.send(self.state.update_state(branch, changes));
            }
            StateRequest::CtxAtVersion {
                version,
                guard,
                resp,
            } => {
                let cache = self.state.ctx_at_version(version);

                let ctx = VersionState::new(version, cache);
                let _ignored = resp.send((ctx, guard));
            }
            StateRequest::DropCtxAtVersion { version } => self.state.drop_ctx_at_version(version),
            StateRequest::CurrentVersion { branch, resp } => {
                // ignore error if the requester dropped it.
                let _ = resp.send(self.state.current_version(branch));
            }
            StateRequest::Fork { from, resp } => {
                // An answer the requester dropped deletes its branch again.
                let branch = UnclaimedBranch::new(self.state.fork(from), self.handle.clone());
                drop(resp.send(branch));
            }
            StateRequest::NewRoot { resp } => {
                let branch = UnclaimedBranch::new(self.state.new_root(), self.handle.clone());
                drop(resp.send(branch));
            }
            StateRequest::DeleteBranch { branch } => self.state.delete_branch(branch),
            StateRequest::LookupKey { key, resp } => drop(resp.send(self.state.lookup_key(key))),
            StateRequest::RecoveryUntracked { key, resp } => {
                let _ = resp.send(self.state.recovery_untracked(key));
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
                drop(resp.send(self.state.update_computed(
                    key,
                    storage,
                    ComputedValueUpdate {
                        value,
                        deps,
                        epsilon,
                    },
                    invalidation_sources,
                )));
            }
            StateRequest::Revalidate {
                key,
                storage,
                candidate,
                invalidation_sources,
                resp,
            } => {
                // ignore error if the requester dropped it.
                drop(
                    resp.send(
                        self.state
                            .revalidate(key, storage, candidate, invalidation_sources),
                    ),
                );
            }
            StateRequest::IdleStatus { branch, resp } => {
                let _ignored = resp.send(self.state.idle_status(branch));
            }
            StateRequest::RunningTasks { branch, resp } => {
                let _ignored = resp.send(self.state.running_tasks(branch));
            }
            StateRequest::PagedOutKeys { resp } => {
                drop(resp.send(Ok(self.state.paged_out_keys())));
            }
            StateRequest::PagableStatus { resp } => {
                drop(resp.send(self.state.pagable_status()));
            }
            StateRequest::PagableNodeCounts { resp } => {
                let _ = resp.send(self.state.pagable_node_counts());
            }
            StateRequest::KeysToPageOut { resp } => {
                drop(resp.send(self.state.keys_to_page_out()));
            }
            StateRequest::CommitPageOut {
                keys,
                eviction_policy,
            } => {
                self.state.commit_page_out(keys, eviction_policy);
            }
            StateRequest::EvictBacked { limit, resp } => {
                let _ = resp.send(self.state.evict_backed(limit));
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
                let _ignored = resp.send(self.state.metrics());
            }
            StateRequest::Introspection { resp } => {
                let _ignored = resp.send(self.state.introspection());
            }
            StateRequest::MakeAvailableForAllocative { resp } => {
                use std::sync::Arc;

                let (complete_tx, complete_rx) = tokio::sync::oneshot::channel();
                // Placeholder, swapped back below, so it needs no paging setup.
                let state = std::mem::replace(&mut self.state, ActorState::new(None));
                let arc_state = Arc::new(state);
                drop(resp.send((Arc::clone(&arc_state), complete_tx)));
                drop(complete_rx.blocking_recv());
                // Correctness: Contract on `MakeAvailableForAllocative`
                let state =
                    Arc::into_inner(arc_state).expect("Other references to have been dropped");
                self.state = state;
            }
        }
    }
}
