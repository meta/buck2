/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rand_distr::Normal;

use crate::graph::Graph;
use crate::graph::GraphVertex;
use crate::types::VertexData;
use crate::types::VertexId;

#[derive(Clone)]
pub struct TestDag {
    pub graph: Graph,
    pub keys: VertexData<String>,
    pub weights: VertexData<u64>,
}

pub fn seeded_rng() -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(123)
}

pub fn make_dag(nodes: usize, rng: &mut impl Rng) -> TestDag {
    let degree_distribution = Normal::<f64>::new(2.0, 5.0).unwrap();

    let mut keys = Vec::new();
    let mut weights = Vec::new();
    let mut vertices = Vec::new();
    let mut edges = Vec::new();

    for i in 0..nodes {
        let candidate_count = nodes - 1 - i;
        let edges_count = degree_distribution.sample(rng).round() as usize;
        let edges_count = edges_count.min(candidate_count);

        keys.push(format!("k{i}"));
        weights.push(rng.random_range(0..10_000));
        vertices.push(GraphVertex {
            edges_idx: edges.len().try_into().unwrap(),
            edges_count: edges_count.try_into().unwrap(),
        });

        // Those are relative to i + 1.
        for j in rand::seq::index::sample(rng, candidate_count, edges_count) {
            let j = i + 1 + j;
            edges.push(VertexId::new(j.try_into().unwrap())); // i depends on j
        }
    }

    TestDag {
        keys: VertexData::new(keys),
        weights: VertexData::new(weights),
        graph: Graph {
            vertices: VertexData::new(vertices),
            edges,
        },
    }
}

impl TestDag {
    /// Returns this graph, but shuffled, and a mapping of old to new.
    pub fn shuffled(&self, rng: &mut impl Rng) -> (Self, VertexData<VertexId>) {
        let len = self.graph.vertices.len();

        let mapping = {
            let mut mapping = Vec::with_capacity(len);
            mapping.extend(self.graph.iter_vertices());
            mapping.shuffle(rng);
            VertexData::new(mapping)
        };

        let mut vertices = VertexData::new(vec![
            GraphVertex {
                edges_idx: 0,
                edges_count: 0,
            };
            len
        ]);
        let mut edges = vec![VertexId::new(0); self.graph.edges.len()];
        let mut keys = VertexData::new(vec![String::default(); len]);
        let mut weights = VertexData::new(vec![u64::default(); len]);

        for idx in self.graph.iter_vertices() {
            let new = mapping[idx];

            let edges_idx = edges.len() as _;
            let mut edges_count = 0;

            for e in self.graph.iter_edges(idx) {
                edges.push(mapping[e]);
                edges_count += 1;
            }

            vertices[new] = GraphVertex {
                edges_idx,
                edges_count,
            };

            keys[new] = self.keys[idx].clone();
            weights[new] = self.weights[idx];
        }

        (
            Self {
                graph: Graph { vertices, edges },
                keys,
                weights,
            },
            mapping,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shuffle() {
        let mut rng = seeded_rng();
        let dag = make_dag(100, &mut rng);
        let (shuffled, map) = dag.shuffled(&mut rng);

        let mut edges_before = dag
            .graph
            .iter_all_edges()
            .map(|(a, b)| (map[a].into_inner(), map[b].into_inner()))
            .collect::<Vec<_>>();
        edges_before.sort();

        let mut edges_after = shuffled
            .graph
            .iter_all_edges()
            .map(|(a, b)| (a.into_inner(), b.into_inner()))
            .collect::<Vec<_>>();
        edges_after.sort();

        assert_eq!(edges_before, edges_after);
    }
}
