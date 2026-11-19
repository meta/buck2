/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BinaryHeap;

use crossbeam::thread;

use crate::critical_path_accessor::CriticalPathAccessor;
use crate::graph::Graph;
use crate::graph::PathCost;
use crate::graph::TopoSortError;
use crate::types::CriticalPathIndex;
use crate::types::CriticalPathVertexData;
use crate::types::OptionalCriticalPathIndex;
use crate::types::VertexData;
use crate::types::VertexId;

/// This returns:
/// - The critical path, as a list of vertices.
/// - Its cost.
/// - For each of the vertices in the critical path, the new critical path cost
///   if that were vertex's runtime was 0.
/// - An accessor to obtain critical path for arbitrary vertices.
pub fn compute_critical_path_potentials(
    deps: &Graph,
    weights: &VertexData<u64>,
) -> Result<
    (
        CriticalPathVertexData<VertexId>,
        PathCost,
        CriticalPathVertexData<PathCost>,
        CriticalPathAccessor,
    ),
    TopoSortError,
> {
    let mut rdeps = None;
    let mut topo_order = None;

    thread::scope(|s| {
        s.spawn(|_| {
            rdeps = Some(deps.reversed());
        });
        s.spawn(|_| {
            topo_order = Some(deps.topo_sort());
        });
    })
    .expect("Threads panicked");

    let rdeps = rdeps.unwrap();
    let topo_order = topo_order.unwrap()?;

    let mut cost_to_sink = None;
    let mut cost_from_source = None;
    let mut predecessors = None;

    thread::scope(|s| {
        s.spawn(|_| {
            let (paths, _) = rdeps.find_longest_paths(topo_order.iter().copied(), weights);
            cost_to_sink = Some(paths);
        });
        s.spawn(|_| {
            let (paths, pred) = deps.find_longest_paths(topo_order.iter().rev().copied(), weights);
            cost_from_source = Some(paths);
            predecessors = Some(pred);
        });
    })
    .expect("Threads panicked");

    let cost_to_sink = cost_to_sink.unwrap();
    let cost_from_source = cost_from_source.unwrap();
    let predecessors = predecessors.unwrap();

    let critical_path_accessor = CriticalPathAccessor {
        predecessors,
        cost_from_source,
    };

    // Look up the critical path. Find the node with the highest cost from a source, then iterate
    // over predecessors to reconstruct the critical path.
    let critical_path_end = critical_path_accessor
        .cost_from_source
        .iter()
        .max_by_key(|(_idx, cost)| *cost);

    let (critical_path_sink, critical_path_cost) = match critical_path_end {
        Some(c) => c,
        None => {
            // The graph is empty.
            return Ok((
                CriticalPathVertexData::new(Vec::new()),
                PathCost::default(),
                CriticalPathVertexData::new(Vec::new()),
                critical_path_accessor,
            ));
        }
    };

    let critical_path_cost = *critical_path_cost;

    // Now, traverse predecessors to actually get the list of ndoes on the critical path.
    let critical_path = critical_path_accessor
        .critical_path_for_vertex_and_cost(critical_path_sink, critical_path_cost);

    // For each node, we now identify:

    // - The last node on the critical path with a path to this node.
    let mut last_cp_predecessor = deps.allocate_vertex_data(OptionalCriticalPathIndex::none());

    // - The first node on the critical path with a path from this node.
    let mut first_cp_successor = deps.allocate_vertex_data(OptionalCriticalPathIndex::none());

    // To do this, we just need to traverse starting from the critical path nodes.

    struct GraphVisitor<'a> {
        graph: &'a Graph,
        mark: &'a mut VertexData<OptionalCriticalPathIndex>,
        cp_idx: CriticalPathIndex,
    }

    impl GraphVisitor<'_> {
        /// Iterative, with an explicit stack: a chain of vertices can be far deeper than the
        /// thread's stack allows.
        fn visit(&mut self, start: VertexId) {
            let mut stack = vec![start];
            while let Some(vertex_idx) = stack.pop() {
                if self.mark[vertex_idx].is_some() {
                    continue;
                }

                self.mark[vertex_idx] = self.cp_idx.into();

                stack.extend(self.graph.iter_edges(vertex_idx));
            }
        }
    }

    thread::scope(|s| {
        s.spawn(|_| {
            for (cp_idx, vertex_idx) in critical_path.iter().rev() {
                GraphVisitor {
                    graph: &rdeps,
                    mark: &mut last_cp_predecessor,
                    cp_idx,
                }
                .visit(*vertex_idx);
            }
        });

        s.spawn(|_| {
            for (cp_idx, vertex_idx) in critical_path.iter() {
                GraphVisitor {
                    graph: deps,
                    mark: &mut first_cp_successor,
                    cp_idx,
                }
                .visit(*vertex_idx);
            }
        });
    })
    .expect("Threads panicked");

    // Compute the cost of the longest path through each vertex. We do this here instead of inline
    // later to avoid jumping around 3 arrays later (whereas here we can do so linearly).

    let mut vertices_cost = deps.allocate_vertex_data(PathCost::default());

    for idx in deps.iter_vertices() {
        let mut cost = critical_path_accessor.cost_from_source[idx] + cost_to_sink[idx];
        // Don't double-count the vertex at `idx`.
        cost.runtime -= weights[idx];
        cost.len -= 1;
        vertices_cost[idx] = cost
    }

    // Now, we know the longest path through each vertex, so what we need to do is find out when
    // that path does not overlap with the critical path. To do this, we're going lay out our
    // computation as a series of items that represent when nodes have their longest path overlap
    // with the critical path, relative to each critical path node.

    enum WorkItem {
        NodeValid {
            // Which node became valid (there is no longer a path from the current critical path to
            // this node).
            idx: VertexId,
            // When does this node become invalid (there is now a path from the current critical
            // path node to this node).
            invalid_at: Option<CriticalPathIndex>, // Might make sense to move this to a separate array to avoid copying it when we sort.
        },

        // Ordering matters here. We process all the nodes that become valid before we compute.
        Compute,
    }

    let mut work: Vec<(CriticalPathIndex, WorkItem)> =
        Vec::with_capacity(deps.vertices.len() + critical_path.len());

    for cp_idx in critical_path.keys() {
        work.push((cp_idx, WorkItem::Compute));
    }

    for idx in deps.iter_vertices() {
        let pred = last_cp_predecessor[idx];
        let succ = first_cp_successor[idx];

        let valid_at = match pred.into_option() {
            // Valid once we're past the last predecessor.
            Some(pred) => pred.successor(),
            // Valid since the beginning.
            None => CriticalPathIndex::zero(),
        };

        let invalid_at = succ.into_option();

        work.push((valid_at, WorkItem::NodeValid { idx, invalid_at }));
    }

    // Sort all of this by when the nodes become valid. Process all node validities before
    // computing the actual updted critical path cost.

    work.sort_by_key(|(idx, item)| {
        (
            *idx,
            match item {
                WorkItem::NodeValid { .. } => 0,
                WorkItem::Compute => 1,
            },
        )
    });

    // Initialize the new cost for the critical path for each node assuming it now takes zero time.

    let mut updated_critical_path_cost =
        CriticalPathVertexData::new(vec![PathCost::default(); critical_path.len()]);

    for (idx, vertex) in critical_path.iter() {
        // Drop this vertex's runtime.
        let mut cost = critical_path_cost;
        cost.runtime -= weights[*vertex];
        updated_critical_path_cost[idx] = cost;
    }

    // Now, compare to other longest paths. The heap is a max heap and keeps track of the longest
    // path through any of the vertices in the graph that does not overlap with the critical path
    // up to the current index.

    let mut heap = BinaryHeap::new();

    for (critical_path_index, item) in work {
        match item {
            WorkItem::NodeValid { idx, invalid_at } => {
                let cost = vertices_cost[idx];
                heap.push((cost, invalid_at));
            }
            WorkItem::Compute => {
                while let Some(item) = heap.peek() {
                    let replacement_cost = match item {
                        (cost, Some(invalid_at)) if *invalid_at > critical_path_index => cost,
                        (cost, None) => cost,
                        _ => {
                            // This node is invalid now (meaning its longest path now overlaps with
                            // the critical path), so pop it.
                            heap.pop();
                            continue;
                        }
                    };

                    let curr_cost = updated_critical_path_cost[critical_path_index];
                    if curr_cost < *replacement_cost {
                        updated_critical_path_cost[critical_path_index] = *replacement_cost
                    }

                    break;
                }
            }
        }
    }

    Ok((
        critical_path,
        critical_path_cost,
        updated_critical_path_cost,
        critical_path_accessor,
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use super::*;
    use crate::builder::GraphBuilder;
    use crate::test_utils::TestDag;
    use crate::test_utils::make_dag;
    use crate::test_utils::seeded_rng;

    fn naive_critical_path_costs(
        dag: &TestDag,
        replacement: Option<(VertexId, u64)>,
    ) -> VertexData<PathCost> {
        // By construction, TestDag guarantees `vertices` is a topological order, so we iterate in
        // reverse order.
        let mut costs = dag.graph.allocate_vertex_data(PathCost::default());

        for idx in dag.graph.iter_vertices().rev() {
            let mut max: Option<PathCost> = None;

            for edge in dag.graph.iter_edges(idx) {
                let edge_cost = costs[edge];

                let replace = match max {
                    Some(max_cost) => max_cost < edge_cost,
                    _ => true,
                };

                if replace {
                    max = Some(edge_cost)
                }
            }

            let v_runtime = match replacement {
                Some((replacement, runtime)) if replacement == idx => runtime,
                _ => dag.weights[idx],
            };

            let cost = PathCost {
                len: 1,
                runtime: v_runtime,
            } + max.unwrap_or_default();

            costs[idx] = cost;
        }

        costs
    }

    fn naive_critical_path_cost(dag: &TestDag, replacement: Option<(VertexId, u64)>) -> PathCost {
        let costs = naive_critical_path_costs(dag, replacement);
        costs.values().max().copied().unwrap()
    }

    fn do_test(dag: &TestDag) {
        eprintln!("{} vertices", dag.graph.vertices.len());
        eprintln!("{} edges", dag.graph.edges.len());

        let fast = Instant::now();
        let (critical_path, critical_path_cost, replacement_costs, _) =
            compute_critical_path_potentials(&dag.graph, &dag.weights).unwrap();

        for (cp_idx, _) in critical_path.iter() {
            assert!(critical_path_cost >= replacement_costs[cp_idx]);
        }

        let fast = Instant::now() - fast;

        let naive = naive_critical_path_cost(dag, None);
        assert_eq!(naive, critical_path_cost);

        eprintln!();
        eprintln!("critical path = {naive:?}");

        let slow = Instant::now();
        for (idx, replacement) in critical_path.values().zip(replacement_costs.values()) {
            let naive = naive_critical_path_cost(dag, Some((*idx, 0)));
            assert_eq!(naive, *replacement, "replacing node {idx:?} fails");
        }
        let slow = Instant::now() - slow;

        eprintln!("fast: {} us", fast.as_micros());
        eprintln!("slow: {} us", slow.as_micros());
    }

    pub fn test_dag(nodes: usize) -> TestDag {
        make_dag(nodes, &mut seeded_rng())
    }

    const DEEP_SIDE_CHAIN_HELPER_ENV: &str = "BUCK2_CRITICAL_PATH_DEEP_SIDE_CHAIN_HELPER";

    /// A chain of a million zero-cost vertices hangs off the second vertex of the critical path.
    /// Does the work only in the subprocess spawned by `deep_side_chain_overflows_the_stack`.
    #[test]
    fn deep_side_chain_helper() {
        if std::env::var_os(DEEP_SIDE_CHAIN_HELPER_ENV).is_none() {
            return;
        }
        const N: u32 = 1_000_000;
        let mut builder = GraphBuilder::new();
        builder.push(0u32, std::iter::empty(), ()).unwrap();
        for i in 1..N {
            builder.push(i, std::iter::once(i - 1), ()).unwrap();
        }
        let p0 = N;
        let p1 = N + 1;
        builder.push(p0, std::iter::empty(), ()).unwrap();
        builder.push(p1, vec![N - 1, p0], ()).unwrap();
        let (graph, keys, _data) = builder.finish();
        let mut weights = graph.allocate_vertex_data(0u64);
        weights[keys.get(&p0).unwrap()] = 1;

        let (critical_path, cost, _replacement, _) =
            compute_critical_path_potentials(&graph, &weights).unwrap();
        assert_eq!(2, critical_path.len());
        assert_eq!(PathCost { runtime: 1, len: 2 }, cost);
    }

    /// The walk over the chain must not use one stack frame per vertex.
    #[cfg(unix)]
    #[test]
    fn deep_side_chain_does_not_overflow_the_stack() {
        // ast-grep-ignore: rust/buck2-no-command-new
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "potential::tests::deep_side_chain_helper",
                "--nocapture",
            ])
            .env(DEEP_SIDE_CHAIN_HELPER_ENV, "1")
            .output()
            .unwrap();
        // The test harness may run the helper in a child of its own and report the child's fate
        // in its own output, so look at everything.
        let report = format!(
            "{:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{report}");
        assert!(
            !report.contains("overflowed its stack") && !report.contains("signal 6"),
            "{report}"
        );
    }

    #[test]
    fn test_trivial() {
        do_test(&test_dag(2));
    }

    #[test]
    fn test_mini() {
        do_test(&test_dag(4));
    }

    #[test]
    fn test_medium() {
        do_test(&test_dag(100))
    }

    #[test]
    fn test_large() {
        do_test(&test_dag(1000))
    }

    #[test]
    fn test_xlarge() {
        do_test(&test_dag(10_000))
    }

    #[test]
    fn test_xxlarge() {
        do_test(&test_dag(100_000))
    }

    #[test]
    fn test_xxxlarge() {
        do_test(&test_dag(1_000_000))
    }

    /// Run on a larger number of random graphs.
    #[test]
    fn test_random_large() {
        for i in 0..10 {
            eprintln!();
            eprintln!("test {i}");
            let mut this_rng = ChaCha8Rng::seed_from_u64(i);
            let dag = make_dag(1000, &mut this_rng);
            do_test(&dag);
        }
    }

    #[test]
    fn test_accessor() {
        let dag = make_dag(100, &mut seeded_rng());
        let naive = naive_critical_path_costs(&dag, None);

        let (_critical_path, _critical_path_cost, _replacement_costs, accessor) =
            compute_critical_path_potentials(&dag.graph, &dag.weights).unwrap();

        for (idx, _) in dag.keys.iter() {
            assert_eq!(
                accessor.critical_path_for_vertex(idx).unwrap().0,
                naive[idx]
            );
        }
    }
}
