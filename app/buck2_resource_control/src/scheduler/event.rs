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
use std::time::Instant;
use std::time::SystemTime;

use buck2_events::daemon_id::DaemonId;
use buck2_hash::IntentionallyStdHashMap;
use buck2_wrapper_common::invocation_id::TraceId;
use tokio::sync::mpsc;

use crate::CommandType;
use crate::memory_tracker::MemoryReading;
use crate::scheduler::Scene;

pub(crate) struct EventSenderState {
    metadata: IntentionallyStdHashMap<String, String>,
    estimated_memory_cap: u64,
    estimated_swap_cap: Option<u64>,
    memory_reading: MemoryReading,
    last_scheduled_event_time: Option<Instant>,
    txs: Vec<mpsc::UnboundedSender<ResourceControlEventMostly>>,
    tags: Vec<String>,
}

impl EventSenderState {
    pub(crate) fn new(daemon_id: &DaemonId, estimated_memory_cap: u64) -> Self {
        Self {
            metadata: buck2_events::metadata::collect(daemon_id),
            estimated_memory_cap,
            estimated_swap_cap: None,
            last_scheduled_event_time: None,
            memory_reading: MemoryReading::zero(),
            txs: Vec::new(),
            tags: Vec::new(),
        }
    }

    pub(crate) fn command_started(
        &mut self,
        event_tx: mpsc::UnboundedSender<ResourceControlEventMostly>,
    ) {
        self.txs.push(event_tx);
    }

    pub(crate) fn update_memory_reading(&mut self, memory_reading: MemoryReading) {
        self.memory_reading = memory_reading;
    }

    pub(crate) fn update_estimated_memory_cap(&mut self, estimated_memory_cap: u64) {
        self.estimated_memory_cap = estimated_memory_cap;
    }

    /// The most swap that buck can use, as best we know; `None` if we don't.
    pub(crate) fn update_estimated_swap_cap(&mut self, estimated_swap_cap: Option<u64>) {
        self.estimated_swap_cap = estimated_swap_cap;
    }

    pub(crate) fn set_tags(&mut self, tags: Vec<String>) {
        self.tags = tags;
    }

    pub(crate) fn maybe_send_scheduled_event(
        &mut self,
        freq: Duration,
        actions_running: u64,
        actions_suspended: u64,
        now: Instant,
    ) {
        if self
            .last_scheduled_event_time
            .is_none_or(|last| now.duration_since(last) >= freq)
        {
            self.last_scheduled_event_time = Some(now);
            self.send_event(
                buck2_data::ResourceControlEventKind::Scheduled,
                None,
                actions_running,
                actions_suspended,
            );
        }
    }

    pub(crate) fn send_event(
        &mut self,
        kind: buck2_data::ResourceControlEventKind,
        cgroup: Option<&Scene>,
        actions_running: u64,
        actions_suspended: u64,
    ) {
        let e = self.make_event(kind, cgroup, actions_running, actions_suspended);
        self.txs.retain_mut(|tx| tx.send(e.clone()).is_ok());
    }

    fn make_event(
        &self,
        kind: buck2_data::ResourceControlEventKind,
        cgroup: Option<&Scene>,
        actions_running: u64,
        actions_suspended: u64,
    ) -> ResourceControlEventMostly {
        ResourceControlEventMostly {
            time_event_generated: SystemTime::now(),
            metadata: self.metadata.clone(),
            kind,

            memory_reading: self.memory_reading,
            estimated_memory_cap: self.estimated_memory_cap,
            estimated_swap_cap: self.estimated_swap_cap,
            actions_suspended,
            actions_running,

            action_kind: cgroup.map(|cgroup| cgroup.description.command_type),
            action_digest: cgroup
                .map(|cgroup| cgroup.description.action_digest.clone().unwrap_or_default()),
            action_cgroup_memory_current: cgroup.map(|cgroup| cgroup.memory_current),
            action_cgroup_memory_peak: cgroup.map(|cgroup| cgroup.memory_peak),
            action_cgroup_swap_current: cgroup.map(|cgroup| cgroup.swap_current),
            action_cgroup_swap_peak: cgroup.map(|cgroup| cgroup.swap_peak),
            tags: self.tags.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResourceControlEventMostly {
    time_event_generated: SystemTime,
    metadata: IntentionallyStdHashMap<String, String>,
    kind: buck2_data::ResourceControlEventKind,

    memory_reading: MemoryReading,
    estimated_memory_cap: u64,
    estimated_swap_cap: Option<u64>,
    actions_running: u64,
    actions_suspended: u64,

    action_kind: Option<CommandType>,
    action_digest: Option<String>,
    action_cgroup_memory_current: Option<u64>,
    action_cgroup_memory_peak: Option<u64>,
    action_cgroup_swap_current: Option<u64>,
    action_cgroup_swap_peak: Option<u64>,
    tags: Vec<String>,
}

impl ResourceControlEventMostly {
    pub(crate) fn complete(self, uuid: &TraceId) -> buck2_data::ResourceControlEvent {
        buck2_data::ResourceControlEvent {
            uuid: uuid.to_string(),
            kind: self.kind.into(),

            time_collected: Some(self.memory_reading.time_collected.into()),
            time_event_generated: Some(self.time_event_generated.into()),

            allprocs_memory_current: self.memory_reading.allprocs_memory_current,
            allprocs_memory_swap_current: self.memory_reading.allprocs_swap_current,
            allprocs_page_cache: self.memory_reading.allprocs_page_cache,
            allprocs_page_cache_active: self.memory_reading.allprocs_page_cache_active,
            allprocs_swap_failures: self.memory_reading.allprocs_swap_failures,
            allprocs_memory_pressure: self.memory_reading.allprocs_memory_pressure.round() as u64,

            daemon_memory_current: self.memory_reading.daemon_memory_current,
            daemon_swap_current: self.memory_reading.daemon_swap_current,

            action_kind: self.action_kind.map(|s| s.to_string()),
            action_digest: self.action_digest,

            action_cgroup_memory_current: self.action_cgroup_memory_current,
            action_cgroup_memory_peak: self.action_cgroup_memory_peak,
            action_cgroup_swap_current: self.action_cgroup_swap_current,
            action_cgroup_swap_peak: self.action_cgroup_swap_peak,

            actions_suspended_count: self.actions_suspended,
            actions_running_count: self.actions_running,

            metadata: self.metadata,

            ancestor_cgroup_constraints: Some(buck2_data::AncestorCgroupConstraints {
                // FIXME(JakobDegen): Replace with more appropriate columns
                memory_max: Some(self.estimated_memory_cap),
                memory_high: Some(self.estimated_memory_cap),
                memory_swap_max: self.estimated_swap_cap,
                memory_swap_high: None,
            }),

            tags: self.tags,
        }
    }
}
