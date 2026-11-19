/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::types::OptionalVertexId;
use crate::types::VertexData;
use crate::types::VertexId;

#[derive(Copy, Clone)]
pub struct GraphVertex {
    pub edges_idx: u32,
    pub edges_count: u32,
}

#[derive(Clone)]
pub struct Graph {
    pub(crate) vertices: VertexData<GraphVertex>,
    pub(crate) edges: Vec<VertexId>,
}

impl Graph {
    #[inline]
    pub fn iter_vertices(&self) -> impl DoubleEndedIterator<Item = VertexId> + use<> {
        self.vertices.keys()
    }

    #[inline]
    pub fn iter_edges(&self, idx: VertexId) -> impl Iterator<Item = VertexId> + '_ {
        let vertex = self.vertices[idx];
        let range_from = vertex.edges_idx as usize;
        let range_to = range_from + vertex.edges_count as usize;
        self.edges[range_from..range_to].iter().copied()
    }

    pub fn iter_all_edges(&self) -> impl Iterator<Item = (VertexId, VertexId)> + '_ {
        self.iter_vertices()
            .flat_map(|i| self.iter_edges(i).map(move |j| (i, j)))
    }

    /// Allocate a VertexData with space for each of the vertices in this graph.
    pub fn allocate_vertex_data<T>(&self, default: T) -> VertexData<T>
    where
        T: Clone,
    {
        VertexData::new(vec![default; self.vertices.len()])
    }

    /// Reverse this Graph. The VertexIds are unchanged (they still represent the same vertices so
    /// they can be used to index into VertexData).
    pub fn reversed(&self) -> Self {
        let mut reverse_vertices = self.allocate_vertex_data(GraphVertex {
            edges_idx: 0,
            edges_count: 0,
        });

        // We could compute that count ahead of time while reading, probably not worth it.
        for edge in &self.edges {
            reverse_vertices[*edge].edges_count += 1;
        }

        let mut idx = 0;
        for vertex in reverse_vertices.values_mut() {
            vertex.edges_idx = idx;
            idx += vertex.edges_count;
        }
        assert_eq!(idx as usize, self.edges.len());

        let mut vertex_edge_offset = self.allocate_vertex_data(0usize);
        let mut reverse_edges = vec![VertexId::default(); self.edges.len()];

        for from in self.iter_vertices() {
            for to in self.iter_edges(from) {
                let offset = &mut vertex_edge_offset[to];
                reverse_edges[reverse_vertices[to].edges_idx as usize + *offset] = from;
                *offset += 1;
            }
        }

        Self {
            vertices: reverse_vertices,
            edges: reverse_edges,
        }
    }

    /// Obtain a topological ordering of this graph. The graph must be a DAG (but that's the only
    /// thing that GraphBuilder can construct).
    pub fn topo_sort(&self) -> Result<Vec<VertexId>, TopoSortError> {
        enum Work {
            Push,
            Pop,
        }

        let mut topo_order = vec![VertexId::new(0); self.vertices.len()];

        // A pair of (marked, visited), where marked means this node is on the stack and visited
        // means it was popped off.
        let mut state = self.allocate_vertex_data((false, false));
        let mut current_topo_order_index = self.vertices.len().saturating_sub(1);

        let mut queue = Vec::new();

        for i in self.iter_vertices() {
            queue.push((i, Work::Push));

            while let Some((j, work)) = queue.pop() {
                let (marked, visited) = &mut state[j];

                match work {
                    Work::Push => {
                        if *visited {
                            continue;
                        }

                        if *marked {
                            // Extract the cycle from the DFS stack. The queue contains
                            // interleaved Push/Pop entries; the Pop entries represent
                            // the current DFS path. Walk backward to find `j` and
                            // collect the cycle.
                            let mut cycle = vec![j];
                            for (node, w) in queue.iter().rev() {
                                if let Work::Pop = w {
                                    cycle.push(*node);
                                    if *node == j {
                                        break;
                                    }
                                }
                            }
                            cycle.reverse();
                            return Err(TopoSortError::Cycle(cycle));
                        }

                        *marked = true;

                        queue.push((j, Work::Pop));
                        queue.extend(self.iter_edges(j).map(|k| (k, Work::Push)));
                    }
                    Work::Pop => {
                        *visited = true;
                        topo_order[current_topo_order_index] = j;
                        current_topo_order_index = current_topo_order_index.saturating_sub(1);
                    }
                }
            }
        }

        Ok(topo_order)
    }

    /// Given a DAG and a *reverse topological* ordering thereof, return the predecessor for each
    /// node after aggregating by runtime.
    pub fn find_longest_paths(
        &self,
        reverse_topo_order: impl IntoIterator<Item = VertexId>,
        weights: &VertexData<u64>,
    ) -> (VertexData<PathCost>, VertexData<OptionalVertexId>) {
        let mut predecessor = self.allocate_vertex_data(OptionalVertexId::none());

        // The runtime for this vertex
        let mut costs = self.allocate_vertex_data(PathCost::default());

        for idx in reverse_topo_order {
            let mut max: Option<(PathCost, VertexId)> = None;

            // Visit my dependencies.
            for from in self.iter_edges(idx) {
                let from_cost = &costs[from];

                let replace = match max {
                    Some((max_cost, _)) => max_cost < *from_cost,
                    _ => true,
                };

                if replace {
                    max = Some((*from_cost, from));
                }
            }

            let me = PathCost {
                runtime: weights[idx],
                len: 1,
            };

            match max {
                Some((cost, vertex)) => {
                    costs[idx] = cost + me;
                    predecessor[idx] = vertex.into();
                }
                None => {
                    costs[idx] = me;
                    predecessor[idx] = OptionalVertexId::none();
                }
            }
        }

        (costs, predecessor)
    }

    pub fn vertices_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn edges_count(&self) -> usize {
        self.edges.len()
    }

    /// Produce a new graph after adding edges. This may produce a multigraph and the graph may
    /// stop being a DAG.
    pub fn add_edges(
        &self,
        add: &impl AddEdges,
        size_hint: impl Into<Option<usize>>,
    ) -> Result<Graph, AddEdgesError> {
        let mut vertices = self.allocate_vertex_data(GraphVertex {
            edges_idx: 0,
            edges_count: 0,
        });

        let mut edges = Vec::with_capacity(self.edges.len() + size_hint.into().unwrap_or_default());
        let mut all_edges_idx = 0;

        for from in self.iter_vertices() {
            let edges_idx = all_edges_idx;
            let mut edges_count = 0;

            for e in self.iter_edges(from) {
                edges.push(e);
                edges_count += 1;
                all_edges_idx += 1;
            }

            for e in add.edges_for_vertex(from) {
                edges.push(e);
                edges_count += 1;
                all_edges_idx += 1;
            }

            vertices[from] = GraphVertex {
                edges_idx,
                edges_count,
            };
        }

        Ok(Graph { vertices, edges })
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tier0)]
pub enum TopoSortError {
    #[error("cycle ({} nodes)", .0.len())]
    Cycle(Vec<VertexId>),
}

pub trait AddEdges {
    type EdgeIterator: Iterator<Item = VertexId>;

    fn edges_for_vertex(&self, idx: VertexId) -> Self::EdgeIterator;
}

impl AddEdges for VertexData<OptionalVertexId> {
    type EdgeIterator = std::option::IntoIter<VertexId>;

    #[inline]
    fn edges_for_vertex(&self, idx: VertexId) -> Self::EdgeIterator {
        self[idx].into_option().into_iter()
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tier0)]
pub enum AddEdgesError {
    #[error("overflow")]
    Overflow,
}

#[derive(
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    derive_more::Add,
    Debug
)]
pub struct PathCost {
    pub runtime: u64,
    pub len: u32,
}

#[cfg(test)]
mod tests {
    use buck2_hash::BuckMutSet;

    use super::*;
    use crate::builder::GraphBuilder;
    use crate::test_utils::make_dag;
    use crate::test_utils::seeded_rng;
    use crate::types::VertexKeys;

    const K0: &str = "key0";
    const K1: &str = "key1";
    const K2: &str = "key2";
    const K3: &str = "key3";
    const K4: &str = "key4";

    fn test_graph() -> (Graph, VertexKeys<&'static str>, VertexData<&'static str>) {
        let mut builder = GraphBuilder::new();
        builder.push(K3, std::iter::empty(), K3).unwrap();
        builder.push(K2, vec![K3], K2).unwrap();
        builder.push(K1, std::iter::empty(), K1).unwrap();
        builder.push(K0, vec![K1, K2], K0).unwrap();

        builder.finish()
    }

    #[test]
    fn test_iter() {
        let (graph, _keys, data) = test_graph();
        let edges = graph
            .iter_all_edges()
            .map(|(l, r)| (data[l], data[r]))
            .collect::<Vec<_>>();

        assert_eq!(vec![(K2, K3), (K0, K1), (K0, K2)], edges);
    }

    #[test]
    fn test_reverse() {
        let (graph, _keys, data) = test_graph();
        let rev_graph = graph.reversed();
        let rev_edges = rev_graph
            .iter_all_edges()
            .map(|(l, r)| (data[l], data[r]))
            .collect::<Vec<_>>();

        assert_eq!(vec![(K3, K2), (K2, K0), (K1, K0)], rev_edges);
    }

    #[test]
    fn test_topo_sort() {
        let (graph, _keys, data) = test_graph();
        let topo = graph
            .topo_sort()
            .unwrap()
            .into_iter()
            .map(|k| data[k])
            .collect::<Vec<_>>();
        assert_eq!(topo, vec![K0, K1, K2, K3]);
    }

    #[test]
    fn test_topo_sort_empty() {
        let (graph, _, _) = GraphBuilder::<&'static str, ()>::new().finish();
        assert_eq!(graph.topo_sort().unwrap(), vec![]);
    }

    #[test]
    fn test_topo_sort_large() {
        let mut rng = seeded_rng();
        let graph = make_dag(10000, &mut rng).shuffled(&mut rng).0.graph;
        let topo_order = graph.topo_sort().unwrap();
        let ranks = {
            let mut ranks = VertexData::new(vec![0; graph.vertices.len()]);
            for (rank, idx) in topo_order.iter().enumerate() {
                ranks[*idx] = rank;
            }
            ranks
        };

        for (i, j) in graph.iter_all_edges() {
            assert!(ranks[i] < ranks[j]);
        }
    }

    #[test]
    fn test_topo_sort_cycle() {
        // Graph: a(K0) -> b(K1) -> c(K2) -> d(K3), b -> e(K4)
        // Then add c -> b to create a cycle. Only b and c should be in the cycle;
        // a is above, d is below, and e is a sibling.
        let mut builder = GraphBuilder::new();
        builder.push(K3, std::iter::empty(), ()).unwrap(); // d
        builder.push(K4, std::iter::empty(), ()).unwrap(); // e
        builder.push(K2, vec![K3], ()).unwrap(); // c -> d
        builder.push(K1, vec![K2, K4], ()).unwrap(); // b -> c, e
        builder.push(K0, vec![K1], ()).unwrap(); // a -> b
        let (graph, keys, _data) = builder.finish();

        let vb = keys.get(&K1).unwrap();
        let vc = keys.get(&K2).unwrap();

        // Add c -> b to create a cycle
        let mut new_edges = graph.allocate_vertex_data(OptionalVertexId::none());
        new_edges[vc] = vb.into();
        let graph = graph.add_edges(&new_edges, None).unwrap();

        match graph.topo_sort() {
            Err(TopoSortError::Cycle(cycle)) => {
                let cycle_set: BuckMutSet<_> = cycle.iter().copied().collect();
                assert_eq!(cycle_set.len(), 2, "cycle should contain exactly b and c");
                assert!(cycle_set.contains(&vb), "cycle should contain b");
                assert!(cycle_set.contains(&vc), "cycle should contain c");
            }
            other => panic!("Expected cycle error, got {:?}", other),
        }
    }

    #[test]
    fn test_longest_paths() {
        let (graph, keys, _data) = test_graph();

        // Get the vertex ids
        let v0 = keys.get(&K0).unwrap();
        let v1 = keys.get(&K1).unwrap();
        let v2 = keys.get(&K2).unwrap();
        let v3 = keys.get(&K3).unwrap();

        let mut weights = graph.allocate_vertex_data(0);
        weights[v0] = 5;
        weights[v1] = 15;
        weights[v2] = 10;
        weights[v3] = 20;

        let topo = graph.topo_sort().unwrap();
        let (costs, predecessor) = graph.find_longest_paths(topo.iter().rev().copied(), &weights);

        assert_eq!(
            costs[v3],
            PathCost {
                len: 1,
                runtime: 20
            }
        );

        assert_eq!(
            costs[v2],
            PathCost {
                len: 2,
                runtime: 30
            }
        );

        assert_eq!(
            costs[v1],
            PathCost {
                len: 1,
                runtime: 15
            }
        );

        assert_eq!(
            costs[v0],
            PathCost {
                len: 3,
                runtime: 35
            }
        );

        assert_eq!(predecessor[v0].into_option(), Some(v2));
        assert_eq!(predecessor[v1].into_option(), None);
        assert_eq!(predecessor[v2].into_option(), Some(v3));
        assert_eq!(predecessor[v3].into_option(), None);
    }

    #[test]
    fn test_add_edges() {
        let (graph, keys, data) = test_graph();

        let v0 = keys.get(&K0).unwrap();
        let v2 = keys.get(&K2).unwrap();
        let v3 = keys.get(&K3).unwrap();

        let mut new_edges = graph.allocate_vertex_data(OptionalVertexId::none());
        new_edges[v0] = v3.into(); // new edge
        new_edges[v2] = v3.into(); // already exists
        let graph = graph.add_edges(&new_edges, None).unwrap();

        let edges = graph
            .iter_all_edges()
            .map(|(l, r)| (data[l], data[r]))
            .collect::<Vec<_>>();
        assert_eq!(
            vec![(K2, K3), (K2, K3), (K0, K1), (K0, K2), (K0, K3)],
            edges
        );
    }
}
