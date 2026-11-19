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

use buck2_analysis::analysis::calculation::AnalysisKey;
use buck2_build_signals::env::CriticalPathBackendName;
use buck2_build_signals::env::NodeDuration;
use buck2_build_signals::env::WaitingData;
use buck2_build_signals::error::CriticalPathError;
use buck2_build_signals::node_key::BuildSignalsNodeKey;
use buck2_core::soft_error_or_panic;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_critical_path::AddEdgesError;
use buck2_critical_path::Graph;
use buck2_critical_path::GraphBuilder;
use buck2_critical_path::OptionalVertexId;
use buck2_critical_path::PushError;
use buck2_critical_path::TopoSortError;
use buck2_critical_path::VertexData;
use buck2_critical_path::VertexId;
use buck2_critical_path::VertexKeys;
use buck2_critical_path::compute_critical_path_potentials;
use buck2_error::BuckErrorOptionContext;
use buck2_events::span::SpanId;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use dupe::Dupe;
use itertools::Itertools;
use smallvec::SmallVec;

use crate::AnalysisFinishNodeKey;
use crate::BuildInfo;
use crate::DetailedCriticalPath;
use crate::DetailedCriticalPathEntry;
use crate::NodeData;
use crate::NodeExtraData;
use crate::NodeKey;
use crate::backend::backend::BuildListenerBackend;

/// An implementation of critical path that uses a longest-paths graph in order to produce
/// potential savings in addition to the critical path.
pub(crate) struct LongestPathGraphBackend {
    builder: Result<GraphBuilder<NodeKey, NodeData>, CriticalPathError>,
    top_level_targets: Vec<TopLevelTarget>,
}

/// Represents nodes that block us "seeing" other parts of the graph until they finish evaluating.
struct TopLevelTarget {
    target: ConfiguredTargetLabel,
    artifacts: Vec<NodeKey>,
}

impl LongestPathGraphBackend {
    pub(crate) fn new() -> Self {
        Self {
            builder: Ok(GraphBuilder::new()),
            top_level_targets: Vec::new(),
        }
    }
}

impl BuildListenerBackend for LongestPathGraphBackend {
    fn process_node(
        &mut self,
        key: NodeKey,
        extra_data: NodeExtraData,
        duration: NodeDuration,
        dep_keys: impl IntoIterator<Item = NodeKey>,
        span_ids: SmallVec<[SpanId; 1]>,
        waiting_data: WaitingData,
    ) {
        let builder = match self.builder.as_mut() {
            Ok(b) => b,
            Err(..) => return,
        };

        let res = builder.push(
            key,
            dep_keys,
            NodeData {
                extra_data,
                duration,
                waiting_data,
                span_ids,
            },
        );

        let res = res.or_else(|err| match err {
            PushError::Overflow => Err(CriticalPathError::GraphBuildOverflow),
            e @ PushError::DuplicateKey { .. } => {
                soft_error_or_panic!("critical_path_duplicate_key", e.into());
                Ok(())
            }
        });

        match res {
            Ok(()) => {}
            Err(e) => self.builder = Err(e),
        }
    }

    fn process_top_level_target(
        &mut self,
        target: ConfiguredTargetLabel,
        artifacts: impl IntoIterator<Item = NodeKey>,
    ) {
        self.top_level_targets.push(TopLevelTarget {
            target,
            artifacts: artifacts.into_iter().collect(),
        })
    }

    fn finish(
        self,
        discovery_edges: BuckMutMap<NodeKey, NodeKey>,
    ) -> Result<BuildInfo, CriticalPathError> {
        let (graph, keys, data) = {
            let (graph, keys, data) = self.builder?.finish();

            let mut first_analysis = graph.allocate_vertex_data(OptionalVertexId::none());
            let mut n = 0;

            // Add discovery edges for top-level targets: artifact nodes depend on the
            // top-level analysis that first discovered them. The top-level command
            // doesn't know about any actions until the top-level analysis has finished.
            for top_level_target in &self.top_level_targets {
                // This is a bit wasteful, but transient and the volume is small.
                let analysis_key =
                    NodeKey::AnalysisKey(AnalysisKey(top_level_target.target.dupe()));
                let artifacts = &top_level_target.artifacts;

                // If the analysis was split due to anon targets, prefer the finish key
                // (Part 2) since that represents the full completion of analysis.
                let finish_key = NodeKey::Dyn(
                    "AnalysisFinishKey",
                    BuildSignalsNodeKey::new(AnalysisFinishNodeKey {
                        target: top_level_target.target.dupe(),
                        target_rule_type_name: None,
                    }),
                );

                let analysis = match keys.get(&finish_key) {
                    Some(k) => k,
                    None => match keys.get(&analysis_key) {
                        Some(k) => k,
                        None => continue, // Nothing depends on this,
                    },
                };

                let mut queue = Vec::new();

                // We have an analysis and a set of artifacts that we decided to build after having
                // evaluated this analysis. So, traverse all of those artifacts' dependencies, and
                // label them as depending on this top level analysis (but we only do that once).
                // Concretely, this expresses the idea that we only started knowing we cared about
                // those artifacts once we finished that analysis.

                for artifact in artifacts {
                    let artifact = match keys.get(artifact) {
                        Some(a) => a,
                        None => {
                            // Not built. Unexpected, but we don't report signals in all failure cases so that can happen.
                            continue;
                        }
                    };

                    queue.push(artifact);

                    while let Some(i) = queue.pop() {
                        if first_analysis[i].is_some() {
                            continue;
                        }

                        // We only traverse edges to things that produce artifacts here.
                        match keys[i] {
                            NodeKey::BuildKey(..)
                            | NodeKey::EnsureTransitiveSetProjectionKey(..)
                            | NodeKey::EnsureProjectedArtifactKey(..) => {}
                            _ => {
                                continue;
                            }
                        };

                        first_analysis[i] = analysis.into();
                        queue.extend(graph.iter_edges(i));
                        n += 1;
                    }
                }
            }

            // Discovery edges: each discovered node depends on its discoverer (an anon target
            // on the analysis Part 1 that first discovered it; a package load on the load of
            // the package whose computation first requested it).
            for (discovered_key, discoverer_key) in &discovery_edges {
                let discovered_vertex = match keys.get(discovered_key) {
                    Some(v) => v,
                    None => continue,
                };
                let discoverer_vertex = match keys.get(discoverer_key) {
                    Some(v) => v,
                    None => continue,
                };
                if !first_analysis[discovered_vertex].is_some() {
                    first_analysis[discovered_vertex] = discoverer_vertex.into();
                    n += 1;
                }
            }

            let graph = match graph.add_edges(&first_analysis, n) {
                Ok(g) => g,
                Err(AddEdgesError::Overflow) => {
                    return Err(CriticalPathError::GraphBuildOverflow);
                }
            };

            (graph, keys, data)
        };

        let durations = data.map_ref(|d| {
            d.duration
                .critical_path_duration()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX)
        });

        let (slowest_path, cp_res) = std::thread::scope(|s| {
            let cp = s.spawn(|| {
                compute_critical_paths(&graph, &keys, &data, durations, &self.top_level_targets)
            });
            let slow = s.spawn(|| compute_slowest_paths(&graph, &keys, &data));

            let cp = cp.join().expect("thread panicked");
            let slow = slow.join().expect("thread panicked");

            (slow, cp)
        });

        let slowest_path = slowest_path?;
        let (critical_path, critical_path_for_top_level_targets) = cp_res?;

        Ok(BuildInfo {
            critical_path,
            slowest_path,
            num_nodes: graph.vertices_count() as _,
            num_edges: graph.edges_count() as _,
            top_level_targets: critical_path_for_top_level_targets,
        })
    }

    fn name() -> CriticalPathBackendName {
        CriticalPathBackendName::LongestPathGraph
    }
}

fn compute_critical_paths(
    graph: &Graph,
    keys: &VertexKeys<NodeKey>,
    data: &VertexData<NodeData>,
    durations: VertexData<u64>,
    top_level_targets: &[TopLevelTarget],
) -> Result<(DetailedCriticalPath, Vec<(ConfiguredTargetLabel, Duration)>), CriticalPathError> {
    let (critical_path, critical_path_cost, replacement_durations, critical_path_accessor) =
        compute_critical_path_potentials(graph, &durations)
            .map_err(|e| format_topo_sort_cycle_error(e, keys))?;

    let critical_path_for_top_level_targets = top_level_targets
        .iter()
        .filter_map(|t| {
            let max_cost = (|| {
                let (path_cost, _critical_path) = t
                    .artifacts
                    .iter()
                    .map(|a| {
                        let idx = keys
                            .get(a)
                            .with_internal_error(|| format!("Cannot find artifact: {a}"))?;
                        critical_path_accessor
                            .critical_path_for_vertex(idx)
                            .with_internal_error(|| format!("Invalid index for artifact: {a}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max_by_key(|p| p.0)
                    .internal_error("No critical path")?;

                buck2_error::Result::Ok(Duration::from_micros(path_cost.runtime))
            })();

            let max_cost = match max_cost {
                Ok(max_cost) => max_cost,
                Err(e) => {
                    // This would happen if we're given a target at the top
                    // level but then its DICE node never gets computed.
                    // This may happen if the command was e.g. cancelled.
                    tracing::debug!("No critical path for target {}: {:#}", t.target, e);
                    return None;
                }
            };

            Some((t.target.dupe(), max_cost))
        })
        .collect::<Vec<_>>();

    drop(durations);

    let critical_path = critical_path
        .iter()
        .filter_map(|(cp_idx, vertex_idx)| {
            let vertex_idx = *vertex_idx;
            if matches!(keys[vertex_idx], NodeKey::PageInConnector(..)) {
                return None;
            }

            let key = keys[vertex_idx].dupe();

            let node_data = data[vertex_idx].clone();
            let deps_finished_time = graph
                .iter_edges(vertex_idx)
                .map(|d| data[d].duration.total.end())
                .max();
            let potential_improvement = Some(Duration::from_micros(
                critical_path_cost.runtime - replacement_durations[cp_idx].runtime,
            ));
            Some(DetailedCriticalPathEntry {
                key,
                data: node_data,
                potential_improvement,
                deps_finished_time,
            })
        })
        .collect();

    Ok((
        DetailedCriticalPath::new(critical_path),
        critical_path_for_top_level_targets,
    ))
}

/// Computes the "slowest path" where each node's predecessor is the dependency that finished last.
/// This differs from critical path where predecessors have the greatest critical path length.
/// The slowest path makes waiting time directly attributable to what a node is immediately waiting on.
fn compute_slowest_paths(
    graph: &Graph,
    keys: &VertexKeys<NodeKey>,
    data: &VertexData<NodeData>,
) -> Result<DetailedCriticalPath, CriticalPathError> {
    let mut last = None;

    for d in graph.iter_vertices() {
        let d_end = data[d].duration.total.end();
        if last.is_none_or(|(l_end, _)| l_end < d_end) {
            last = Some((d_end, d));
        }
    }

    let mut visited: BuckMutSet<VertexId> = BuckMutSet::default();
    let mut slowest_path = Vec::new();
    let mut node = last.unzip().1;
    while let Some(curr) = node {
        if !visited.insert(curr) {
            return Err(format_slowest_path_cycle_error(keys, &slowest_path, curr));
        }

        let mut prev = None;
        for d in graph.iter_edges(curr) {
            let d_end = data[d].duration.total.end();
            if prev.is_none_or(|(p_end, _)| p_end < d_end) {
                prev = Some((d_end, d));
            }
        }

        let (deps_finished_time, prev_node) = prev.unzip();

        if !matches!(keys[curr], NodeKey::PageInConnector(..)) {
            slowest_path.push(DetailedCriticalPathEntry {
                key: keys[curr].dupe(),
                data: data[curr].clone(),
                potential_improvement: None,
                deps_finished_time,
            });
        }
        node = prev_node;
    }

    slowest_path.reverse();

    Ok(DetailedCriticalPath::new(slowest_path))
}

fn format_topo_sort_cycle_error(
    error: TopoSortError,
    keys: &VertexKeys<NodeKey>,
) -> CriticalPathError {
    let TopoSortError::Cycle(cycle_vertices) = error;
    let cycle_desc = cycle_vertices.iter().map(|v| &keys[*v]).join(" -> ");
    CriticalPathError::CycleDetected(format!(
        "{} nodes in cycle: {}",
        cycle_vertices.len(),
        cycle_desc
    ))
}

fn format_slowest_path_cycle_error(
    keys: &VertexKeys<NodeKey>,
    path: &[DetailedCriticalPathEntry],
    cycle_target: VertexId,
) -> CriticalPathError {
    let cycle_target_key = &keys[cycle_target];

    let cycle_start_idx = path
        .iter()
        .position(|e| &e.key == cycle_target_key)
        .unwrap_or(0);

    let cycle_nodes = &path[cycle_start_idx..];
    let cycle_len = cycle_nodes.len() + 1;
    let cycle_desc = cycle_nodes
        .iter()
        .map(|e| &e.key)
        .chain(std::iter::once(cycle_target_key))
        .join(" -> ");

    CriticalPathError::CycleDetected(format!("{} nodes in cycle: {}", cycle_len, cycle_desc))
}
