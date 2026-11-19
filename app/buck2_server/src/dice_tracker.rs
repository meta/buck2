/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use allocative::Allocative;
use buck2_core::buck2_env;
use buck2_core::soft_error_or_panic;
use buck2_data::*;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_hash::BuckMutMap;
use buck2_util::threads::thread_spawn;
use dice::DiceEvent;
use dice::DiceEventListener;
use dupe::Dupe;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;

/// Sample of the dice core-state queue taken at each snapshot tick.
pub struct CoreStateQueueSample {
    /// Current depth of the queue feeding the core-state thread.
    pub depth: u64,
    /// Total requests dequeued so far; monotonic across commands.
    pub processed_requests: u64,
}

/// Closure used to sample the dice core-state queue at each snapshot tick.
/// Boxed so that BuckDiceTracker doesn't need a generic parameter.
type CoreStateQueueSampleFn = Box<dyn Fn() -> CoreStateQueueSample + Send + Sync + 'static>;

/// The BuckDiceTracker keeps track of the started/finished events for a dice computation and periodically sends a snapshot to the client.
///
/// There are too many events coming out of dice for us to forward them all to the client, so we need to aggregate
/// them in some way in the daemon.
///
/// The tracker will send a snapshot event every 500ms (only if there have been changes since the last snapshot).
///
/// A client won't necessarily get a final snapshot before a command returns.
#[derive(Allocative)]
pub struct BuckDiceTracker {
    #[allocative(skip)]
    event_forwarder: UnboundedSender<DiceEvent>,
}

impl BuckDiceTracker {
    pub fn new(
        events: EventDispatcher,
        core_state_queue_sample_fn: CoreStateQueueSampleFn,
    ) -> buck2_error::Result<Self> {
        let (event_forwarder, receiver) = mpsc::unbounded();
        let snapshot_interval =
            buck2_env!("BUCK2_DICE_SNAPSHOT_INTERVAL_MS", type=u64, default = 500)
                .map(Duration::from_millis)?;

        thread_spawn("buck2-dice-tracker", move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(with_dispatcher_async(
                events.dupe(),
                Self::run_task(
                    events,
                    receiver,
                    snapshot_interval,
                    core_state_queue_sample_fn,
                ),
            ))
        })
        .unwrap();

        Ok(Self { event_forwarder })
    }

    async fn run_task(
        events: EventDispatcher,
        mut receiver: UnboundedReceiver<DiceEvent>,
        snapshot_interval: Duration,
        core_state_queue_sample_fn: CoreStateQueueSampleFn,
    ) {
        let mut needs_update = false;
        let mut states = BuckMutMap::default();
        let mut interval = tokio::time::interval(snapshot_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // This will loop until the sender side of the channel is dropped.
        loop {
            tokio::select! {
                ev = receiver.next() => {
                    needs_update = true;
                    match ev {
                        Some(DiceEvent::Started{key_type}) => {
                            states.entry(key_type).or_insert_with(DiceKeyState::default).started += 1;
                        }
                        Some(DiceEvent::Finished{key_type}) => {
                            states.entry(key_type).or_insert_with(DiceKeyState::default).finished += 1;
                        }
                        Some(DiceEvent::CheckDepsStarted{key_type}) => {
                            states.entry(key_type).or_insert_with(DiceKeyState::default).check_deps_started += 1;
                        }
                        Some(DiceEvent::CheckDepsFinished{key_type}) => {
                            states.entry(key_type).or_insert_with(DiceKeyState::default).check_deps_finished += 1;
                        }
                        Some(DiceEvent::ComputeStarted{key_type}) => {
                            states.entry(key_type).or_insert_with(DiceKeyState::default).compute_started += 1;
                        }
                        Some(DiceEvent::ComputeFinished{key_type}) => {
                            states.entry(key_type).or_insert_with(DiceKeyState::default).compute_finished += 1;
                        }
                        Some(DiceEvent::HydrationFailed{key_type, error, transient}) => {
                            let error = buck2_error::buck2_error!(
                                buck2_error::ErrorTag::Tier0,
                                "Failed to page a DICE value back in for `{key_type}`: {error}"
                            );
                            // Reported apart so the rate of reads that storage
                            // could not serve in time can be read off against
                            // failures of the stored data.
                            if transient {
                                soft_error_or_panic!("dice_page_in_failed_transient", error);
                            } else {
                                soft_error_or_panic!("dice_page_in_failed", error);
                            }
                        }
                        None => {
                            // This indicates that the sender side has been dropped and we can exit.
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if needs_update {
                        needs_update = false;
                        let queue_sample = core_state_queue_sample_fn();
                        events.instant_event(DiceStateSnapshot {
                            key_states: states
                                .iter()
                                .map(|(k, v)| ((*k).to_owned(), *v))
                                .collect(),
                            core_state_queue_depth: queue_sample.depth,
                            core_state_processed_requests: queue_sample.processed_requests,
                        });
                    }
                }
            }
        }
    }
}

impl DiceEventListener for BuckDiceTracker {
    fn event(&self, event: DiceEvent) {
        let _ignored = self.event_forwarder.unbounded_send(event);
    }
}
