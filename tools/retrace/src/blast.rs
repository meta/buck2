/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Recorded descendant reachability and path cost for a target label.
//!
//! Per target label (from `args.target`, so the whole cfg family of a
//! target groups together) over the stitched DAG:
//!
//! - path cost = max `Dag::down` over the label's nodes, using the
//!   recorded durations. Ranking needs no additional path traversal.
//! - downstream set = forward BFS over the DAG's succs from the
//!   label's nodes: how many recorded actions depend on it and their
//!   summed duration, including the label's own actions.
//!
//! This is not an invalidation or warm-build forecast. Descendants may
//! have been cache hits, whose recorded durations are not execution
//! costs. A per-label BFS avoids an O(N²) reachability table.

use fxhash::FxHashMap;

use crate::analyses::Extras;
use crate::dag::DagBundle;
use crate::types::Trace;

/// Bound full descendant walks even when a caller requests a larger top-k.
pub const MAX_CANDIDATES: usize = 20;

#[derive(Clone, Debug)]
pub struct Blast {
    pub label: String,
    /// the label's own traced actions
    pub n_actions: usize,
    /// Recorded descendant path cost; retained field name for API compatibility.
    pub warm_wall_us: i64,
    /// Reachable recorded actions (the label's own included).
    pub downstream_actions: usize,
    /// Reachable actions served from cache, not measured executions.
    pub downstream_cache_hits: usize,
    /// Their summed recorded durations.
    pub downstream_work_us: i64,
    /// `Trace::acts` index of the label's first action, for navigation
    pub act: u32,
}

/// Label -> DAG nodes, over the stitched bundle.
pub struct BlastIndex {
    by_label: FxHashMap<u32, Vec<u32>>,
    /// exact label text -> label id, for O(1) resolve
    by_name: FxHashMap<String, u32>,
    /// scratch for BFS visit stamps, one slot per node
    n_nodes: usize,
}

impl BlastIndex {
    pub fn new(b: &DagBundle, tr: &Trace, x: &Extras) -> BlastIndex {
        Self::new_cancellable(b, tr, x, &|| false).expect("an uncancelled label index completes")
    }

    pub fn new_cancellable(
        b: &DagBundle,
        _tr: &Trace,
        x: &Extras,
        cancelled: &impl Fn() -> bool,
    ) -> Option<BlastIndex> {
        if cancelled() {
            return None;
        }
        let mut by_label: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        for (node, &act) in b.dag.act_of_node.iter().enumerate() {
            if node.is_multiple_of(4096) && cancelled() {
                return None;
            }
            if act != u32::MAX
                && let Some(label) = x.action_args(act as usize).label
            {
                by_label.entry(label).or_default().push(node as u32);
            }
        }
        let mut by_name = FxHashMap::default();
        for (i, &label) in by_label.keys().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                return None;
            }
            by_name.insert(x.labels[label as usize].clone(), label);
        }
        Some(BlastIndex {
            by_label,
            by_name,
            n_nodes: b.dag.weight_us.len(),
        })
    }

    pub fn n_labels(&self) -> usize {
        self.by_label.len()
    }

    /// Resolve a query to a label id: exact label, else unique
    /// `...:name` suffix. Ambiguity and misses are loud.
    pub fn resolve(&self, x: &Extras, query: &str) -> anyhow::Result<u32> {
        if let Some(&hit) = self.by_name.get(query) {
            return Ok(hit);
        }
        let query = query.trim();
        let query = query.split_once(" (").map_or(query, |(label, _)| label);
        if let Some(&hit) = self.by_name.get(query) {
            return Ok(hit);
        }
        anyhow::ensure!(!query.is_empty(), "target label must not be empty");
        let suffix = if query.starts_with("//") {
            query.to_owned()
        } else if query.contains(':') && !query.starts_with(':') {
            format!("//{query}")
        } else {
            format!(":{}", query.trim_start_matches(':'))
        };
        let mut hits: Vec<u32> = self
            .by_label
            .keys()
            .copied()
            .filter(|&l| x.labels[l as usize].ends_with(&suffix))
            .collect();
        match hits.len() {
            0 => anyhow::bail!(
                "target {query} matches no traced target label; use a full label, //package:name, package:name, or a unique :name"
            ),
            1 => Ok(hits.remove(0)),
            n => {
                hits.sort_by(|&a, &b| x.labels[a as usize].cmp(&x.labels[b as usize]));
                let mut names: Vec<&str> = hits
                    .iter()
                    .take(5)
                    .map(|&l| x.labels[l as usize].as_str())
                    .collect();
                if n > 5 {
                    names.push("…");
                }
                anyhow::bail!(
                    "target {query} is ambiguous ({n} labels): {}",
                    names.join(", ")
                )
            }
        }
    }

    /// The blast of one label. `visited`/`stamp` are reusable scratch
    /// (pass a zeroed vec and stamp 1, bump the stamp per call).
    pub fn blast_of(
        &self,
        b: &DagBundle,
        x: &Extras,
        label: u32,
        visited: &mut Vec<u32>,
        stamp: u32,
    ) -> Option<Blast> {
        self.blast_of_cancellable(b, x, label, visited, stamp, &|| false)
    }

    /// An interrupted traversal clears its visit stamps, so reusing the
    /// scratch with the same stamp cannot silently omit visited nodes.
    pub fn blast_of_cancellable(
        &self,
        b: &DagBundle,
        x: &Extras,
        label: u32,
        visited: &mut Vec<u32>,
        stamp: u32,
        cancelled: &impl Fn() -> bool,
    ) -> Option<Blast> {
        let nodes = self.by_label.get(&label)?;
        if cancelled() {
            visited.clear();
            return None;
        }
        visited.resize(self.n_nodes, 0);
        let mut queue: Vec<u32> = Vec::new();
        let (mut warm_wall, mut first) = (0, u32::MAX);
        for (i, &n) in nodes.iter().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                visited.clear();
                return None;
            }
            warm_wall = warm_wall.max(b.dag.down[n as usize]);
            first = first.min(n);
            if visited[n as usize] != stamp {
                visited[n as usize] = stamp;
                queue.push(n);
            }
        }
        let dag = &b.dag;
        let mut actions = 0usize;
        let mut cache_hits = 0usize;
        let mut work = 0i64;
        let mut head = 0usize;
        while head < queue.len() {
            if head.is_multiple_of(4096) && cancelled() {
                visited.clear();
                return None;
            }
            let n = queue[head] as usize;
            head += 1;
            if dag.act_of_node[n] != u32::MAX {
                actions += 1;
                cache_hits +=
                    usize::from(x.action_args(dag.act_of_node[n] as usize).cache.is_some());
                work += dag.weight_us[n];
            }
            for (edge, &s) in dag.succs[dag.succ_off[n] as usize..dag.succ_off[n + 1] as usize]
                .iter()
                .enumerate()
            {
                if edge > 0 && edge.is_multiple_of(16_384) && cancelled() {
                    visited.clear();
                    return None;
                }
                if visited[s as usize] != stamp {
                    visited[s as usize] = stamp;
                    queue.push(s);
                }
            }
        }
        if cancelled() {
            visited.clear();
            return None;
        }
        Some(Blast {
            label: x.labels[label as usize].clone(),
            n_actions: nodes.len(),
            warm_wall_us: warm_wall,
            downstream_actions: actions,
            downstream_cache_hits: cache_hits,
            downstream_work_us: work,
            act: dag.act_of_node[first as usize],
        })
    }

    /// The top `k` labels by recorded descendant path cost (the ranking reads
    /// the precomputed `down`; at most `MAX_CANDIDATES` winners pay a BFS).
    pub fn top_candidates(&self, b: &DagBundle, x: &Extras, k: usize) -> Vec<Blast> {
        self.top_candidates_cancellable(b, x, k, &|| false)
            .expect("an uncancelled descendant report completes")
    }

    /// Ranking and each selected BFS share cancellation. No partial
    /// top-k table is returned if the request is superseded.
    pub fn top_candidates_cancellable(
        &self,
        b: &DagBundle,
        x: &Extras,
        k: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Option<Vec<Blast>> {
        if cancelled() {
            return None;
        }
        let dag = &b.dag;
        let mut ranked = Vec::with_capacity(self.by_label.len());
        let mut scanned = 0usize;
        for (&label, nodes) in &self.by_label {
            let mut weight = 0;
            for &node in nodes {
                if scanned.is_multiple_of(4096) && cancelled() {
                    return None;
                }
                scanned += 1;
                weight = weight.max(dag.down[node as usize]);
            }
            ranked.push((weight, label));
        }
        let cut = k.min(MAX_CANDIDATES).min(ranked.len());
        if cut > 0 && cut < ranked.len() {
            ranked.select_nth_unstable_by_key(cut - 1, |&(w, label)| (std::cmp::Reverse(w), label));
        }
        ranked.truncate(cut);
        ranked.sort_unstable_by_key(|&(w, label)| (std::cmp::Reverse(w), label));
        if cancelled() {
            return None;
        }
        let mut visited = Vec::new();
        let mut out = Vec::new();
        for (i, &(_, label)) in ranked.iter().enumerate() {
            out.push(self.blast_of_cancellable(
                b,
                x,
                label,
                &mut visited,
                i as u32 + 1,
                cancelled,
            )?);
        }
        Some(out)
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyses::load_extras;
    use crate::graph::load_graph;
    use crate::parse;

    /// lib (2 actions: metadata+link chain via same label) -> bin;
    /// tool is independent; zlib is an independent leaf whose label
    /// (`fbcode//z:lib`) shares lib's `:lib` suffix.
    fn fixture() -> (Trace, crate::graph::Graph) {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-blast-{}-{seq}-trace.json",
            std::process::id()
        ));
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"lib (rustc meta)","ts":0,"dur":100,"args":{"src":"rustc","run":"r","target":"fbcode//x:lib (rustc meta)"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"lib (rustc emit)","ts":100,"dur":300,"args":{"src":"rustc","run":"r","target":"fbcode//x:lib (rustc emit)"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"bin (link)","ts":400,"dur":500,"args":{"src":"cxx_link","run":"r","target":"fbcode//x:bin (link)"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"tool (link)","ts":0,"dur":50,"args":{"src":"cxx_link","run":"r","target":"fbcode//y:tool (link)"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"zlib (rustc emit)","ts":0,"dur":100,"args":{"src":"rustc","run":"r","target":"fbcode//z:lib (rustc emit)"}},"#,
                "\n"
            ),
        )
        .unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!(
            "retrace-blast-{}-{seq}-graph.json",
            std::process::id()
        ));
        std::fs::write(
            &gp,
            concat!(
                "{\"graph\":{\"enc\":2}}\n",
                "{\"t\":\"lib (rustc meta)\",\"d\":[]}\n",
                "{\"t\":\"lib (rustc emit)\",\"d\":[\"lib (rustc meta)\"]}\n",
                "{\"t\":\"bin (link)\",\"d\":[\"lib (rustc emit)\"]}\n",
                "{\"t\":\"tool (link)\",\"d\":[]}\n",
                "{\"t\":\"zlib (rustc emit)\",\"d\":[]}\n",
            ),
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        std::fs::remove_file(&gp).ok();
        (tr, g)
    }

    fn setup() -> (DagBundle, Extras, Trace) {
        let (tr, g) = fixture();
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        (b, x, tr)
    }

    #[test]
    fn descendant_reports_cancel_without_partial_tables_or_stale_visit_stamps() {
        let (mut b, x, tr) = setup();
        assert!(BlastIndex::new_cancellable(&b, &tr, &x, &|| true).is_none());
        let index = BlastIndex::new(&b, &tr, &x);
        let checks = std::cell::Cell::new(0usize);
        let expected = index
            .top_candidates_cancellable(&b, &x, 4, &|| {
                checks.set(checks.get() + 1);
                false
            })
            .unwrap();
        let total = checks.get();
        assert!(total > 4);
        for stop_at in [1, total / 2, total] {
            checks.set(0);
            assert!(
                index
                    .top_candidates_cancellable(&b, &x, 4, &|| {
                        checks.set(checks.get() + 1);
                        checks.get() >= stop_at
                    })
                    .is_none()
            );
            assert_eq!(checks.get(), stop_at);
        }
        assert_eq!(index.top_candidates(&b, &x, 4).len(), expected.len());

        // A single high-fanout node must not defer cancellation to the
        // next BFS node. Repeated edges keep the fixture small while
        // exercising the complete adjacency scan.
        let label = index.resolve(&x, "x:lib").unwrap();
        let nodes = &index.by_label[&label];
        let node = nodes[0] as usize;
        b.dag.succs = vec![nodes[1]; 32_768];
        b.dag.succ_off.fill(0);
        b.dag.succ_off[node + 1..].fill(32_768);
        let mut visited = Vec::new();
        checks.set(0);
        assert!(
            index
                .blast_of_cancellable(&b, &x, label, &mut visited, 1, &|| {
                    checks.set(checks.get() + 1);
                    checks.get() == 4
                })
                .is_none()
        );
        assert_eq!(checks.get(), 4);
        assert!(
            visited.is_empty(),
            "partial traversal stamps are invalidated"
        );
        let retried = index.blast_of(&b, &x, label, &mut visited, 1).unwrap();
        assert_eq!(retried.downstream_actions, 2);
    }

    #[test]
    fn labels_group_a_target_family_and_blast_walks_downstream() {
        let (b, x, tr) = setup();
        let bi = BlastIndex::new(&b, &tr, &x);
        assert_eq!(bi.n_labels(), 4, "lib groups its two actions");

        let lib = bi.resolve(&x, "fbcode//x:lib").unwrap();
        let mut vis = Vec::new();
        let bl = bi.blast_of(&b, &x, lib, &mut vis, 1).unwrap();
        assert_eq!(bl.n_actions, 2);
        // touching lib reruns lib(meta 100) -> lib(emit 300) -> bin(500)
        assert_eq!(bl.warm_wall_us, 900);
        assert_eq!(bl.downstream_actions, 3);
        assert_eq!(bl.downstream_work_us, 900);

        let tool = bi.resolve(&x, "tool").unwrap();
        let bl = bi.blast_of(&b, &x, tool, &mut vis, 2).unwrap();
        assert_eq!(
            (bl.downstream_actions, bl.warm_wall_us),
            (1, 50),
            "tool is a leaf"
        );
    }

    #[test]
    fn resolution_is_loud_about_misses_and_ambiguity() {
        let (b, x, tr) = setup();
        let bi = BlastIndex::new(&b, &tr, &x);
        let e = bi.resolve(&x, "nope").unwrap_err();
        assert!(format!("{e:#}").contains("matches no"));
        // "fbcode//x:lib" and "fbcode//z:lib" share the ":lib" suffix —
        // the bare suffix must refuse and list both candidates
        let e = bi.resolve(&x, "lib").unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("ambiguous (2 labels)"), "{msg}");
        assert!(
            msg.contains("fbcode//x:lib") && msg.contains("fbcode//z:lib"),
            "candidates listed: {msg}"
        );
        let lib = bi.resolve(&x, "fbcode//x:lib").unwrap();
        for query in ["x:lib", "//x:lib", "fbcode//x:lib (rustc emit)"] {
            assert_eq!(bi.resolve(&x, query).unwrap(), lib, "{query}");
        }
        assert!(bi.resolve(&x, "").is_err());
    }

    #[test]
    fn top_candidates_rank_by_warm_forecast() {
        let (b, x, tr) = setup();
        let bi = BlastIndex::new(&b, &tr, &x);
        let top = bi.top_candidates(&b, &x, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].label, "fbcode//x:lib");
        assert_eq!(top[0].warm_wall_us, 900);
        assert!(top[0].warm_wall_us >= top[1].warm_wall_us);
    }

    #[test]
    fn top_candidates_cap_walks_with_more_unique_labels_than_the_limit() {
        let trace_path = crate::temp_path("blast-many-labels-trace.json");
        let graph_path = crate::temp_path("blast-many-labels-graph.json");
        let n = MAX_CANDIDATES + 5;
        let mut trace = String::from("[\n");
        let mut graph = String::from("{\"graph\":{\"enc\":2}}\n");
        for i in 0..n {
            let name = format!("action{i:02}");
            trace.push_str(&format!(
                "{},\n",
                serde_json::json!({
                    "ph":"X", "cat":"recipe", "name":name, "ts":0, "dur":(i + 1) * 1000,
                    "args":{"target":format!("fbcode//cap:target{i:02}")},
                })
            ));
            graph.push_str(&format!("{}\n", serde_json::json!({"t":name,"d":[]})));
        }
        trace.push_str("]\n");
        std::fs::write(&trace_path, trace).expect("should write many-label trace");
        std::fs::write(&graph_path, graph).expect("should write many-label graph");
        let tr = parse::parse(trace_path.to_str().expect("should have UTF-8 fixture path"))
            .expect("should parse many-label trace");
        let g = load_graph(graph_path.to_str().expect("should have UTF-8 fixture path"))
            .expect("should parse many-label graph");
        let x = load_extras(&tr);
        assert!(x.source_error.is_none());
        let (b, _) =
            crate::dag::build_full(&tr, &g, None).expect("should stitch independent labels");
        let index = BlastIndex::new(&b, &tr, &x);
        assert_eq!(
            index.n_labels(),
            n,
            "the fixture must actually exceed the cap"
        );
        for k in [0, 3, MAX_CANDIDATES, n, usize::MAX] {
            let top = index.top_candidates(&b, &x, k);
            assert_eq!(top.len(), k.min(MAX_CANDIDATES));
            for (rank, row) in top.iter().enumerate() {
                let i = n - rank - 1;
                assert_eq!(row.label, format!("fbcode//cap:target{i:02}"));
                assert_eq!(row.warm_wall_us, (i as i64 + 1) * 1000);
                assert_eq!(row.downstream_actions, 1);
            }
        }
        std::fs::remove_file(trace_path).expect("should remove many-label trace");
        std::fs::remove_file(graph_path).expect("should remove many-label graph");
    }

    #[test]
    fn absent_extras_leave_labels_unknown_and_do_not_panic_during_descendant_walks() {
        let (b, mut x, tr) = setup();
        let empty = Extras::default();
        let unknown = BlastIndex::new(&b, &tr, &empty);
        assert_eq!(unknown.n_labels(), 0);
        assert!(unknown.top_candidates(&b, &empty, usize::MAX).is_empty());
        assert!(unknown.resolve(&empty, "x:lib").is_err());

        // An existing index can still walk known labels when per-action
        // extras are unavailable; missing cache evidence is not a cache hit.
        let known = BlastIndex::new(&b, &tr, &x);
        let label = known
            .resolve(&x, "x:lib")
            .expect("should resolve the recorded label");
        x.args.clear();
        let result = known
            .blast_of(&b, &x, label, &mut Vec::new(), 1)
            .expect("should walk without optional action metadata");
        assert_eq!(result.downstream_actions, 3);
        assert_eq!(result.downstream_work_us, 900);
        assert_eq!(result.downstream_cache_hits, 0);
        assert_eq!(known.top_candidates(&b, &x, usize::MAX).len(), 4);
    }

    #[test]
    fn descendant_counts_disclose_cached_durations_without_repricing_them() {
        let (b, mut x, tr) = setup();
        let bin = tr
            .acts
            .iter()
            .position(|a| tr.names[a.name as usize] == "bin (link)")
            .unwrap();
        x.args[bin].cache = Some(Default::default());
        let index = BlastIndex::new(&b, &tr, &x);
        let label = index.resolve(&x, "x:lib").unwrap();
        let blast = index.blast_of(&b, &x, label, &mut Vec::new(), 1).unwrap();
        assert_eq!(
            (blast.downstream_actions, blast.downstream_cache_hits),
            (3, 1)
        );
        assert_eq!(blast.warm_wall_us, 900);
        assert_eq!(blast.downstream_work_us, 900);
    }
}
