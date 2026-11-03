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
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use allocative::Allocative;
use buck2_common::init::ResourceControlConfig;
use buck2_events::daemon_id::DaemonId;
use buck2_events::dispatch::EventDispatcher;
use buck2_hash::BuckMutMap;
use buck2_util::threads::thread_spawn;
use dupe::Dupe;
use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::action_scene::ActionScene;
use crate::buck_cgroup_tree::BuckCgroupTree;
use crate::cgroup::MemoryPressureHandle;
use crate::pool::CgroupPool;
use crate::scheduler::SceneIdRef;
use crate::scheduler::SceneResourceReading;
use crate::scheduler::Scheduler;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MemoryReading {
    pub allprocs_memory_current: u64,
    pub allprocs_swap_current: u64,
    /// The average "full" memory pressure since the last sample
    pub allprocs_memory_pressure: f64,
    pub daemon_memory_current: u64,
    pub daemon_swap_current: u64,
    /// Time this data was collected - only for logs
    pub time_collected: SystemTime,
}

pub type MemoryTrackerHandle = Arc<MemoryTrackerSharedState>;

#[derive(Allocative)]
#[allocative(skip)]
pub struct MemoryTrackerSharedState {
    pub cgroup_tree: BuckCgroupTree,
    // Written to by executors and tracker, read by executors
    pub(crate) action_cgroups: std::sync::Mutex<Scheduler>,
    /// A pool of cgroups to be used for actions.
    ///
    /// There's only one invariant associated with this pool, which is that cgroups will never be
    /// destroyed. Other than that, take one out, it's yours, put it back, someone else can use it.
    /// Hold onto it for reasonable amounts of time to prevent having to make too many cgroups.
    ///
    /// In particular, there's no guarantees about any particular correspondence between things
    /// being taken out of this pool and the lifecycle of an action.
    pub(crate) pool: tokio::sync::Mutex<CgroupPool>,
    /// The memory tracker regularly updates the scheduler with information about the memory state
    /// of the scenes. This map stores the current pairing of scenes to the actions they're
    /// associated with, and is used by the memory tracker to update the scheduler.
    pub(crate) scene_action_mapping: tokio::sync::Mutex<BuckMutMap<SceneIdRef, ActionScene>>,
    /// `true` while the scheduler sees memory pressure; see [`Self::memory_pressure`].
    memory_pressure: watch::Sender<bool>,
}

impl MemoryTrackerSharedState {
    /// Whether the system is under memory pressure: `true` from a few seconds into a pressure
    /// episode until it has eased for longer, once per episode however the scheduler's own
    /// intent moves meanwhile. Daemon-wide, and independent of whether any action is running.
    pub fn memory_pressure(&self) -> watch::Receiver<bool> {
        self.memory_pressure.subscribe()
    }
}

pub struct MemoryReporter {
    cancel: CancellationToken,
}

// Per command task to listen to memory state changes and send events to the client.
pub fn spawn_memory_reporter(
    dispatcher: EventDispatcher,
    memory_tracker: MemoryTrackerHandle,
) -> MemoryReporter {
    let cancel = CancellationToken::new();
    tokio::task::spawn(buck2_util::async_move_clone!(cancel, {
        let (resource_control_event_tx, mut resource_control_event_rx) = mpsc::unbounded_channel();
        memory_tracker
            .action_cgroups
            .lock()
            .unwrap()
            .command_started(resource_control_event_tx);

        loop {
            tokio::select! {
                Some(resource_control_event) = resource_control_event_rx.recv() => {
                    let event = resource_control_event.complete(dispatcher.trace_id());
                    // Spawn and detach the send_now call so we don't block
                    // the event loop. send_now bypasses the scribe producer
                    // queue; under memory pressure the queue-based path gets
                    // blocked while send_now can still deliver events directly
                    // to scribed.
                    let dispatcher = dispatcher.dupe();
                    tokio::spawn(async move {
                        dispatcher.instant_event_send_now(event).await;
                    });
                }
                _ = cancel.cancelled() => {
                    break;
                }
            }
        }
    }));
    MemoryReporter { cancel }
}

impl Drop for MemoryReporter {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub async fn create_memory_tracker(
    cgroup_tree: Option<BuckCgroupTree>,
    resource_control_config: &ResourceControlConfig,
    daemon_id: &DaemonId,
) -> buck2_error::Result<Option<MemoryTrackerHandle>> {
    let Some(cgroup_tree) = cgroup_tree else {
        return Ok(None);
    };

    let cgroup_pool = CgroupPool::create_in_parent_cgroup(
        cgroup_tree.forkserver_and_actions(),
        resource_control_config,
    )
    .await?;
    let effective_resource_constraints = *cgroup_tree.effective_resource_constraints();
    let action_cgroups = Scheduler::init(
        resource_control_config,
        daemon_id,
        effective_resource_constraints,
        buck2_util::system_stats::system_memory_stats(),
        Instant::now(),
    );
    let handle = MemoryTrackerSharedState {
        cgroup_tree,
        action_cgroups: std::sync::Mutex::new(action_cgroups),
        pool: tokio::sync::Mutex::new(cgroup_pool),
        scene_action_mapping: tokio::sync::Mutex::new(BuckMutMap::default()),
        memory_pressure: watch::Sender::new(false),
    };
    let handle = Arc::new(handle);
    let memory_tracker = MemoryTracker {
        handle: handle.dupe(),
        allprocs_memory_pressure_handle: MemoryPressureHandle::new(),
    };
    const TICK_DURATION: Duration = Duration::from_millis(300);
    memory_tracker.spawn(TICK_DURATION).await?;
    Ok(Some(handle))
}

struct MemoryTracker {
    handle: MemoryTrackerHandle,
    allprocs_memory_pressure_handle: MemoryPressureHandle,
}

impl MemoryTracker {
    pub(crate) async fn spawn(self, duration: Duration) -> buck2_error::Result<()> {
        thread_spawn("memory-tracker", move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(self.run(duration));
        })?;
        Ok(())
    }

    async fn run(mut self, duration: Duration) {
        let mut timer = tokio::time::interval(duration);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            timer.tick().await;

            let now = Instant::now();
            let (memory_reading, scene_readings) = tokio::join!(
                Self::collect_memory_reading(
                    &self.handle,
                    &mut self.allprocs_memory_pressure_handle,
                ),
                Self::collect_scene_readings(&self.handle)
            );
            let Some(memory_reading) = memory_reading else {
                continue;
            };

            let under_pressure = {
                let mut action_cgroups = self.handle.action_cgroups.lock().unwrap();
                action_cgroups.update(memory_reading, scene_readings, now);
                action_cgroups.under_memory_pressure()
            };
            self.handle.memory_pressure.send_if_modified(|current| {
                std::mem::replace(current, under_pressure) != under_pressure
            });
        }
    }

    async fn collect_memory_reading(
        handle: &MemoryTrackerHandle,
        allprocs_memory_pressure_handle: &mut MemoryPressureHandle,
    ) -> Option<MemoryReading> {
        let Ok((
            allprocs_memory_current,
            allprocs_swap_current,
            allprocs_memory_pressure,
            daemon_memory_current,
            daemon_swap_current,
        )) = tokio::try_join!(
            handle.cgroup_tree.allprocs().read_memory_current(),
            handle.cgroup_tree.allprocs().read_swap_current(),
            handle
                .cgroup_tree
                .allprocs()
                .read_memory_pressure_total(allprocs_memory_pressure_handle),
            handle.cgroup_tree.daemon().read_memory_current(),
            handle.cgroup_tree.daemon().read_swap_current(),
        )
        else {
            return None;
        };

        Some(MemoryReading {
            allprocs_memory_current,
            allprocs_swap_current,
            allprocs_memory_pressure,
            daemon_memory_current,
            daemon_swap_current,
            time_collected: SystemTime::now(),
        })
    }

    async fn collect_scene_readings(
        handle: &MemoryTrackerHandle,
    ) -> BuckMutMap<SceneIdRef, SceneResourceReading> {
        let mut scenes = handle.scene_action_mapping.lock().await;
        scenes
            .iter_mut()
            .map(|(id, action)| async move { Some((*id, action.poll_resources().await?)) })
            .collect::<FuturesUnordered<_>>()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use buck2_wrapper_common::invocation_id::TraceId;

    use super::*;
    use crate::buck_cgroup_tree::PreppedBuckCgroups;

    #[tokio::test]
    async fn test_current_memory_changes_tracked() -> buck2_error::Result<()> {
        let Some(prepped) = PreppedBuckCgroups::testing_new().await else {
            return Ok(());
        };
        let config = ResourceControlConfig {
            memory_high: Some("19000000".to_owned()),
            ..ResourceControlConfig::testing_default()
        };
        let cgroup_tree = BuckCgroupTree::set_up(prepped, &config).await?;
        let tracker = create_memory_tracker(Some(cgroup_tree), &config, &DaemonId::new())
            .await?
            .unwrap();
        let (resource_control_event_tx, mut resource_control_event_rx) = mpsc::unbounded_channel();
        tracker
            .action_cgroups
            .lock()
            .unwrap()
            .command_started(resource_control_event_tx);

        // Make sure we get some zeros
        std::thread::sleep(Duration::from_secs(3));

        // Simulate the daemon using some memory
        let daemon = tracker.cgroup_tree.daemon();
        let mut child =
            daemon.spawn_use_some_memory_process(1, 20.0, None, Some(Duration::from_secs(10)))?;
        child.wait()?;

        let events = std::iter::from_fn(|| resource_control_event_rx.try_recv().ok())
            .map(|e| e.complete(&TraceId::new()))
            .collect::<Vec<_>>();
        drop(resource_control_event_rx);
        assert!(events.len() > 10, "{}", events.len());

        assert_eq!(events[0].allprocs_memory_pressure, 0);
        assert_eq!(events[0].daemon_memory_current, 0);
        assert_eq!(events[0].daemon_swap_current, 0);

        let assert_max_over = |f: fn(&buck2_data::ResourceControlEvent) -> u64, val, name: &str| {
            let max = events.iter().map(f).max().unwrap();
            assert!(max > val, "{}: {} is not greater than {}", name, max, val);
        };

        assert_max_over(
            |e| e.allprocs_memory_current,
            10000000,
            "allprocs_memory_current",
        );
        let check_memory_pressure;
        #[cfg(fbcode_build)]
        {
            if environment::is_on_demand() {
                // In OD environments, memory pressure may be lower due to different cgroup configurations
                // or resource constraints, so skip this assertion there.
                check_memory_pressure = false;
            } else {
                check_memory_pressure = true;
            }
        }
        #[cfg(not(fbcode_build))]
        {
            check_memory_pressure = true;
        }
        if check_memory_pressure {
            assert_max_over(
                |e| e.allprocs_memory_pressure,
                10,
                "allprocs_memory_pressure",
            );
        }
        assert_max_over(
            |e| e.daemon_memory_current,
            10000000,
            "daemon_memory_current",
        );
        assert_max_over(|e| e.daemon_swap_current, 500000, "daemon_swap_current");

        Ok(())
    }
}
