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
use buck2_events::span::SpanId;
use buck2_hash::BuckMutMap;
use smallvec::SmallVec;

use crate::BuildInfo;
use crate::NodeExtraData;
use crate::NodeKey;

pub(crate) trait BuildListenerBackend {
    fn process_node(
        &mut self,
        key: NodeKey,
        extra_data: NodeExtraData,
        duration: NodeDuration,
        dep_keys: impl IntoIterator<Item = NodeKey>,
        span_ids: SmallVec<[SpanId; 1]>,
        waiting_data: WaitingData,
    );

    fn process_top_level_target(
        &mut self,
        analysis: ConfiguredTargetLabel,
        artifacts: impl IntoIterator<Item = NodeKey>,
    );

    /// `discovery_edges` maps a node to the node that discovered it (an anon target to the
    /// analysis that first discovered it; a package load to the load of the package whose
    /// computation first requested it). Each becomes one extra dependency edge.
    fn finish(
        self,
        discovery_edges: BuckMutMap<NodeKey, NodeKey>,
    ) -> Result<BuildInfo, CriticalPathError>;

    fn name() -> CriticalPathBackendName;
}
