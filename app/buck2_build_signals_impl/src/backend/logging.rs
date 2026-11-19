/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_signals::env::CriticalPathBackendName;
use buck2_build_signals::env::NodeDuration;
use buck2_build_signals::env::WaitingData;
use buck2_build_signals::error::CriticalPathError;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_data::QuickUnstableE2eData;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::span::SpanId;
use buck2_hash::BuckMutMap;
use serde::Deserialize;
use serde::Serialize;
use smallvec::SmallVec;

use crate::BuildInfo;
use crate::DetailedCriticalPath;
use crate::NodeExtraData;
use crate::NodeKey;
use crate::backend::backend::BuildListenerBackend;

pub(crate) struct LoggingBackend {
    events: EventDispatcher,
    start_time: std::time::Instant,
}

impl LoggingBackend {
    pub(crate) fn new(events: EventDispatcher) -> Self {
        Self {
            events,
            start_time: std::time::Instant::now(),
        }
    }
}

/// A discovery edge logged for debugging/testing the critical path: `key` depends on `dep`.
#[derive(Serialize, Deserialize)]
struct Edge {
    key: String,
    dep: String,
}

/// Node data logged for debugging/testing the critical path.
#[derive(Serialize, Deserialize)]
struct Node {
    key: String,
    deps: Vec<String>,
    /// Time span as (start, end) in microseconds since the backend was created.
    time_span: (u64, u64),
}

impl BuildListenerBackend for LoggingBackend {
    fn process_node(
        &mut self,
        key: NodeKey,
        _extra_data: NodeExtraData,
        duration: NodeDuration,
        dep_keys: impl IntoIterator<Item = NodeKey>,
        _span_ids: SmallVec<[SpanId; 1]>,
        _waiting_data: WaitingData,
    ) {
        self.events.instant_event(QuickUnstableE2eData {
            key: "critical_path_logging_node".to_owned(),
            data: serde_json::to_string(&Node {
                key: key.to_string(),
                deps: dep_keys.into_iter().map(|v| v.to_string()).collect(),
                time_span: (
                    duration
                        .total
                        .start()
                        .duration_since(self.start_time)
                        .as_micros() as u64,
                    duration
                        .total
                        .end()
                        .duration_since(self.start_time)
                        .as_micros() as u64,
                ),
            })
            .unwrap(),
        });
    }

    fn process_top_level_target(
        &mut self,
        _analysis: ConfiguredTargetLabel,
        _artifacts: impl IntoIterator<Item = NodeKey>,
    ) {
    }

    fn finish(
        self,
        discovery_edges: BuckMutMap<NodeKey, NodeKey>,
    ) -> Result<BuildInfo, CriticalPathError> {
        for (key, dep) in &discovery_edges {
            self.events.instant_event(QuickUnstableE2eData {
                key: "critical_path_logging_edge".to_owned(),
                data: serde_json::to_string(&Edge {
                    key: key.to_string(),
                    dep: dep.to_string(),
                })
                .unwrap(),
            });
        }
        Ok(BuildInfo {
            critical_path: DetailedCriticalPath::empty(),
            slowest_path: DetailedCriticalPath::empty(),
            num_nodes: 0,
            num_edges: 0,
            top_level_targets: Default::default(),
        })
    }

    fn name() -> CriticalPathBackendName {
        CriticalPathBackendName::Logging
    }
}
