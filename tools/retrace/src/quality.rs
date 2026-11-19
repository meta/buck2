/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The one place an envelope's status, bound, coverage and limitations
//! are decided from what the inputs declare against what was loaded
//! (docs/contract.md). Every `--json` topic, the `--stats` text and the
//! GUI chips go through here, so they cannot disagree about whether a
//! capture or a graph is whole.
//!
//! Ownership of the strings: the trace's `health.warnings` carries the
//! capture and outcome shortfalls once; [`Quality::limitations`] hands
//! the same capture strings to each topic's envelope, and the graph,
//! stitch and pairing strings exist only here.

use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::dag::DagStats;
use crate::graph::Graph;
use crate::types::CaptureScope;
use crate::types::CaptureState;
use crate::types::Outcome;
use crate::types::Trace;

/// What a topic's numbers depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dep {
    Trace,
    Graph,
}

/// What kind of number a topic emits; decides what a gap does to the
/// bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Measurement,
    /// a longest path over the graph: an omission can only shorten it
    LongestPath,
    /// Descendant path plus fanout counts: omitted non-current descendants limit
    /// full-build coverage even when current-action ancestry is intact.
    DescendantPath,
    /// a replayed saving: an omission can move it either way
    Savings,
}

pub const SAVINGS_CONDITIONAL: &str = "savings are conditional on the captured graph: a missing \
                                       edge can raise or lower them, so no bound is claimed";

pub const ACTIVITY_SCOPE_NOTE: &str = "graph scope: current-action ancestors, not a full build \
    graph; current-action critical-path and replay analyses remain available, but complete coverage \
    of dependencies and dependent fanout outside this scope is not claimed";

const REPAIRED_PATH: &str = "the graph was repaired, so the path is a calculation on the repaired \
                             graph, not a bound on the original build";

const UNBOUNDED_PATH: &str = "the input has contradictory capture declarations, integrity failures or surplus counts, so omissions alone \
                               are not established and this path is not a bound on the original build";

/// The shared wire envelope lives below its CLI/GUI consumers.
pub(crate) fn envelope(
    status: &str,
    bound: Option<&str>,
    coverage: Vec<(&str, u64, u64)>,
    mut limitations: Vec<String>,
    data: Value,
) -> Value {
    let mut m = Map::new();
    m.insert("status".into(), json!(status));
    if let Some(b) = bound {
        m.insert("bound".into(), json!(b));
    }
    if !coverage.is_empty() {
        m.insert(
            "coverage".into(),
            Value::Array(
                coverage
                    .into_iter()
                    .map(|(s, n, d)| json!({"scope": s, "numerator": n, "denominator": d}))
                    .collect(),
            ),
        );
    }
    let mut seen = std::collections::HashSet::new();
    limitations.retain(|s| seen.insert(s.clone()));
    m.insert("limitations".into(), json!(limitations));
    m.insert("data".into(), data);
    Value::Object(m)
}

/// The graph header's declarations against what the loader kept.
#[derive(Clone, Debug, Default)]
pub struct GraphQuality {
    pub declared_nodes: Option<i64>,
    pub loaded_rows: usize,
    pub declared_edges: Option<i64>,
    pub loaded_edges: usize,
    /// dangling references the writer itself declared
    pub declared_dangling: i64,
    pub contradictory_counts: bool,
    pub dropped_lines: u64,
    pub dropped_interior: u64,
    pub bad_rows: u64,
    pub undefined: u64,
    pub undefined_samples: Vec<String>,
    pub targets_in_cycles: u64,
    pub cycle_samples: Vec<String>,
    /// the writer's own capture state, when the header carries one
    pub header_capture: Option<CaptureState>,
}

/// How the trace's actions joined the graph.
#[derive(Clone, Debug, Default)]
pub struct StitchQuality {
    pub n_events: usize,
    /// actions whose name has a defining graph row
    pub n_defined: usize,
    /// actions whose name appears only as another row's dependency:
    /// the defining row is missing
    pub n_named_only: usize,
    pub n_unattributed: usize,
    pub n_cycle_edges: u64,
    pub cycle_samples: Vec<String>,
}

/// Whether the graph is known to belong to the selected trace run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pairing {
    Verified,
    /// one or both sides name no run; analysed, with a limitation
    Unverified(String),
    /// the graph names another run; graph topics are refused
    Mismatch {
        graph_run: String,
        trace_run: String,
    },
    /// The run or capture identity cannot pass the admission gate.
    Incompatible(String),
}

fn pairing_of(graph_run: Option<&str>, trace_run: Option<&str>) -> Pairing {
    match (
        graph_run.filter(|r| !r.is_empty()),
        trace_run.filter(|r| !r.is_empty()),
    ) {
        (Some(gr), Some(tr)) if gr == tr => Pairing::Verified,
        (Some(gr), Some(tr)) => Pairing::Mismatch {
            graph_run: gr.to_owned(),
            trace_run: tr.to_owned(),
        },
        (None, None) => Pairing::Unverified(
            "neither the graph header nor the trace names a run, so their pairing could not be checked"
                .to_owned(),
        ),
        (None, Some(_)) => Pairing::Unverified(
            "the graph header names no run, so its pairing with this trace could not be checked"
                .to_owned(),
        ),
        (Some(_), None) => Pairing::Unverified(
            "the trace names no run, so its pairing with the graph could not be checked".to_owned(),
        ),
    }
}

#[derive(Clone, Debug)]
pub struct Quality {
    pub graph_warning: Option<String>,
    /// the selected run's capture state; `None` on traces from before
    /// the record carried one
    pub capture: Option<CaptureState>,
    pub outcome: Outcome,
    /// the run record ended without a result
    pub outcome_unrecorded: bool,
    /// Export-artifact losses, independent of producer footer counts.
    pub dropped_trace_lines: usize,
    pub unended_runs: usize,
    pub graph: Option<GraphQuality>,
    pub stitch: Option<StitchQuality>,
    pub pairing: Option<Pairing>,
}

impl Quality {
    /// The cheap part: the run's capture and outcome, and the graph
    /// header against what was loaded. Stitch coverage is added by
    /// [`Quality::stitched`] by the topics that build the DAG.
    pub fn assess(tr: &Trace, run: Option<&str>, g: Option<&Graph>) -> Quality {
        let selected_run = crate::parse::select_trace_run(tr, run).ok().flatten();
        let sel = selected_run
            .as_deref()
            .and_then(|id| tr.runs.iter().find(|r| r.run == id))
            .or_else(|| {
                (selected_run.is_none() && tr.runs.len() == 1 && tr.runs[0].run.is_empty())
                    .then(|| &tr.runs[0])
            });
        // a trace with no run records whose actions all name one run is
        // still unambiguous about which run it is
        let trace_run = selected_run.as_deref().filter(|r| !r.is_empty());
        let graph = g.map(|g| {
            let cov = g.meta.coverage.clone().unwrap_or_default();
            GraphQuality {
                declared_nodes: cov.nodes,
                loaded_rows: g.n_rows,
                declared_edges: cov.edges,
                loaded_edges: g.n_edges,
                declared_dangling: cov
                    .dangling_deps
                    .unwrap_or(0)
                    .saturating_add(cov.dangling_tsets.unwrap_or(0)),
                contradictory_counts: [
                    cov.nodes,
                    cov.edges,
                    cov.dangling_deps,
                    cov.dangling_tsets,
                    cov.action_rows,
                    cov.join_rows,
                    cov.structural_rows,
                    cov.canonical_aliases,
                ]
                .into_iter()
                .flatten()
                .any(|n| n < 0)
                    || matches!((cov.nodes, cov.action_rows, cov.join_rows),
                        (Some(n), Some(a), Some(j)) if a.checked_add(j) != Some(n))
                    || matches!((cov.structural_rows, cov.join_rows),
                        (Some(s), Some(j)) if s > j),
                dropped_lines: g.dropped_lines,
                dropped_interior: g.dropped_interior,
                bad_rows: g.bad_rows,
                undefined: g.undefined_joins + g.undefined_deps,
                undefined_samples: g.undefined_samples.clone(),
                targets_in_cycles: g.targets_in_cycles,
                cycle_samples: g.cycle_samples.clone(),
                header_capture: cov.dep_capture,
            }
        });
        let pairing = g.map(|g| {
            let pairing = pairing_of(g.meta.run.as_deref(), trace_run);
            match crate::graph::check_trace_pair(tr, run, &g.meta) {
                Err(e) if !matches!(pairing, Pairing::Mismatch { .. }) => {
                    Pairing::Incompatible(format!("{e:#}"))
                }
                _ => pairing,
            }
        });
        Quality {
            graph_warning: tr.health.graph_warning.clone(),
            capture: sel.and_then(|r| r.dep_capture.clone()),
            outcome: sel.map(|r| r.outcome).unwrap_or_default(),
            outcome_unrecorded: sel.is_some_and(|r| r.ended && r.outcome == Outcome::Unknown),
            dropped_trace_lines: tr.health.dropped,
            unended_runs: sel.map_or(tr.health.unended, |r| usize::from(!r.ended)),
            graph,
            stitch: None,
            pairing,
        }
    }

    /// The same assessment with the stitch's coverage folded in.
    pub fn stitched(&self, st: &DagStats) -> Quality {
        let mut q = self.clone();
        q.stitch = Some(StitchQuality {
            n_events: st.n_events,
            n_defined: st.n_matched,
            n_named_only: st.n_named_only,
            n_unattributed: st.n_unattributed,
            n_cycle_edges: st.n_cycle_edges,
            cycle_samples: st.cycle_samples.clone(),
        });
        q
    }

    /// A capture that was enabled and fell short is incomplete; one
    /// that was never enabled, or never recorded, is not a gap.
    pub fn capture_complete(&self) -> bool {
        self.dropped_trace_lines == 0
            && self.unended_runs == 0
            && !matches!(&self.capture, Some(CaptureState::Started(s)) if !s.is_complete())
    }

    /// Every declaration the graph and the stitch make agrees with what
    /// was loaded. No graph is not a graph gap.
    pub fn graph_complete(&self) -> bool {
        let Some(g) = &self.graph else {
            return true;
        };
        let header_ok = g.declared_nodes.is_none_or(|n| n == g.loaded_rows as i64)
            && g.declared_edges.is_none_or(|e| e == g.loaded_edges as i64)
            && g.declared_dangling == 0
            && !g.contradictory_counts
            && g.dropped_lines == 0
            && g.bad_rows == 0
            && g.undefined == 0
            && g.targets_in_cycles == 0
            && !matches!(&g.header_capture, Some(CaptureState::Started(s)) if !s.is_complete())
            && !matches!((&self.capture, &g.header_capture), (Some(a), Some(b)) if a != b);
        let stitch_ok = self.stitch.as_ref().is_none_or(|s| {
            s.n_defined == s.n_events
                && s.n_named_only == 0
                && s.n_unattributed == 0
                && s.n_cycle_edges == 0
        });
        header_ok
            && stitch_ok
            && !matches!(
                self.pairing,
                Some(Pairing::Mismatch { .. } | Pairing::Incompatible(_))
            )
    }

    /// Scope is separate from whether the declared capture was recorded intact.
    pub fn graph_scope(&self) -> Option<CaptureScope> {
        self.graph
            .as_ref()
            .and_then(|g| g.header_capture.as_ref())
            .and_then(CaptureState::scope)
            .or_else(|| self.capture.as_ref().and_then(CaptureState::scope))
    }

    /// Visible scope without mislabeling an intact scoped capture as partial.
    pub fn graph_label(&self) -> &'static str {
        if self.graph_scope() == Some(CaptureScope::CurrentActionAncestors) {
            "graph (current-action ancestors)"
        } else {
            "graph"
        }
    }

    /// Missing rows/edges may justify a lower bound. A repaired cycle,
    /// surplus observations or contradictory counts never do.
    pub fn omission_only(&self) -> bool {
        let capture_ok = |c: &Option<CaptureState>| !matches!(c, Some(CaptureState::Started(s)) if !s.omission_only());
        capture_ok(&self.capture)
            && self.graph.as_ref().is_none_or(|g| {
                g.targets_in_cycles == 0
                    && !g.contradictory_counts
                    && g.declared_nodes.is_none_or(|n| n >= g.loaded_rows as i64)
                    && g.declared_edges.is_none_or(|n| n >= g.loaded_edges as i64)
                    && g.declared_dangling >= 0
                    && capture_ok(&g.header_capture)
                    && match (&self.capture, &g.header_capture) {
                        (Some(a), Some(b)) => a == b,
                        _ => true,
                    }
            })
            && self.stitch.as_ref().is_none_or(|s| {
                s.n_cycle_edges == 0
                    && s.n_defined <= s.n_events
                    && s.n_named_only <= s.n_events - s.n_defined
            })
    }

    /// The capture's shortfalls and an unrecorded outcome, for a topic's
    /// envelope; the same strings `health.warnings` already carries.
    pub fn capture_limitations(&self) -> Vec<String> {
        let mut out = self
            .capture
            .as_ref()
            .map(|c| c.problems())
            .unwrap_or_default();
        if self.dropped_trace_lines > 0 {
            out.push(format!("{} undecodable trace line(s) skipped: the exported artifact may be missing actions even if the producer capture was complete", self.dropped_trace_lines));
        }
        if self.unended_runs > 0 {
            out.push(format!("{} trace run(s) in the analysis scope have no run_end footer: the exported artifact is incomplete", self.unended_runs));
        }
        if self.outcome_unrecorded {
            out.push(
                "the command's outcome is unknown: no reliable result was retained".to_owned(),
            );
        }
        out
    }

    /// The graph's, the stitch's and the pairing's shortfalls.
    pub fn graph_limitations(&self) -> Vec<String> {
        let mut out: Vec<String> = self.graph_warning.iter().cloned().collect();
        if self.graph_scope() == Some(CaptureScope::CurrentActionAncestors) {
            out.push(ACTIVITY_SCOPE_NOTE.to_owned());
        }
        match &self.pairing {
            Some(Pairing::Unverified(why) | Pairing::Incompatible(why)) => out.push(why.clone()),
            Some(Pairing::Mismatch {
                graph_run,
                trace_run,
            }) => out.push(format!(
                "the graph is for build {graph_run}, the trace run is {trace_run}; graph \
                 topics are refused"
            )),
            _ => {}
        }
        let Some(g) = &self.graph else {
            return out;
        };
        if g.contradictory_counts {
            out.push(
                "the graph header contains negative or contradictory coverage counts".to_owned(),
            );
        }
        if matches!((&self.capture, &g.header_capture), (Some(a), Some(b)) if a != b) {
            out.push(
                "the trace and graph declare different dependency-capture states or counts"
                    .to_owned(),
            );
        }
        if let Some(n) = g.declared_nodes
            && n != g.loaded_rows as i64
        {
            out.push(format!(
                "the graph header declares {n} node row(s), {} were loaded",
                g.loaded_rows
            ));
        }
        if let Some(e) = g.declared_edges
            && e != g.loaded_edges as i64
        {
            out.push(format!(
                "the graph header declares {e} edge(s), {} were loaded",
                g.loaded_edges
            ));
        }
        if g.declared_dangling > 0 {
            out.push(format!(
                "the graph's writer declared {} dangling reference(s): edges it could not \
                 resolve",
                g.declared_dangling
            ));
        }
        let torn = g.dropped_lines.saturating_sub(g.dropped_interior);
        if torn > 0 {
            out.push(format!(
                "{torn} line(s) at the end of the graph are not JSON: a torn tail, rows after \
                 it are missing"
            ));
        }
        if g.dropped_interior > 0 {
            out.push(format!(
                "{} malformed line(s) inside the graph were dropped",
                g.dropped_interior
            ));
        }
        if g.bad_rows > 0 {
            out.push(format!(
                "{} graph row(s) had the wrong shape and were dropped",
                g.bad_rows
            ));
        }
        if g.undefined > 0 {
            out.push(format!(
                "{} referenced target(s) have no defining row (first: {})",
                g.undefined,
                g.undefined_samples.join(", ")
            ));
        }
        if g.header_capture != self.capture
            && let Some(CaptureState::Started(s)) = &g.header_capture
        {
            for p in s.problems() {
                out.push(format!("the graph's writer recorded: {p}"));
            }
        }
        let cycle_edges = self.stitch.as_ref().map_or(0, |s| s.n_cycle_edges);
        if g.targets_in_cycles > 0 || cycle_edges > 0 {
            let samples = self
                .stitch
                .as_ref()
                .filter(|s| s.n_cycle_edges > 0)
                .map(|s| s.cycle_samples.as_slice())
                .unwrap_or(&g.cycle_samples);
            let n = if cycle_edges > 0 {
                format!("{cycle_edges} dependency edge(s)")
            } else {
                format!("{} target(s)", g.targets_in_cycles)
            };
            out.push(format!(
                "{n} formed a cycle and the closing edges were dropped under --repair-cycles; \
                 every value here is computed on the repaired graph and is not a bound on the \
                 original build (first: {})",
                samples.join(", ")
            ));
        }
        if let Some(s) = &self.stitch {
            if s.n_defined < s.n_events {
                let named = if s.n_named_only > 0 {
                    format!(
                        " ({} of them are named only as another row's dependency)",
                        s.n_named_only
                    )
                } else {
                    String::new()
                };
                out.push(format!(
                    "{} action(s) have no defining graph row{named}",
                    s.n_events - s.n_defined
                ));
            }
            if s.n_unattributed > 0 {
                out.push(format!(
                    "{} action(s) carry no run id among several runs and were left out",
                    s.n_unattributed
                ));
            }
        }
        out
    }

    pub fn limitations(&self, dep: Dep) -> Vec<String> {
        let mut out = self.capture_limitations();
        if dep == Dep::Graph {
            out.extend(self.graph_limitations());
        }
        out
    }

    /// Counted pairs, one scope each: absent declarations yield no pair
    /// rather than a satisfied one.
    pub fn coverage(&self, dep: Dep) -> Vec<(&'static str, u64, u64)> {
        let mut out = Vec::new();
        if let Some(CaptureState::Started(s)) = &self.capture {
            if let (Some(d), Some(o)) = (s.declared.actions_recorded, s.observed.actions_recorded) {
                out.push(("actions-recorded-vs-declared", o, d));
            }
        }
        if dep == Dep::Trace {
            return out;
        }
        if let Some(s) = &self.stitch {
            out.push((
                "actions-with-graph-nodes",
                s.n_defined as u64,
                s.n_events as u64,
            ));
        }
        if let Some(g) = &self.graph {
            // A surplus is reported as a limitation; coverage itself stops at 100%.
            if let Some(n) = g.declared_nodes.filter(|n| *n >= 0) {
                let n = n as u64;
                out.push(("graph-rows-declared", (g.loaded_rows as u64).min(n), n));
            }
            if let Some(e) = g.declared_edges.filter(|n| *n >= 0) {
                let e = e as u64;
                out.push(("graph-edges-declared", (g.loaded_edges as u64).min(e), e));
            }
            let cycle_edges = self.stitch.as_ref().map_or(0, |s| s.n_cycle_edges);
            if cycle_edges > 0 {
                out.push((
                    "dependency-edges-kept",
                    (g.loaded_edges as u64).saturating_sub(cycle_edges),
                    g.loaded_edges as u64,
                ));
            }
        }
        out
    }

    /// Builds the envelope: a known gap in the capture, or (for graph
    /// topics) in the graph, makes the status `partial`; a longest path
    /// over an omission-only gap keeps `lower`; a saving over any gap
    /// carries no bound. A refusal passes through unchanged.
    #[expect(
        clippy::too_many_arguments,
        reason = "one argument per envelope field plus dependency and claim"
    )]
    pub fn seal(
        &self,
        dep: Dep,
        claim: Claim,
        status: &str,
        bound: Option<&str>,
        mut coverage: Vec<(&'static str, u64, u64)>,
        mut lims: Vec<String>,
        data: Value,
    ) -> Value {
        if status == "refused" {
            return envelope(status, None, Vec::new(), lims, data);
        }
        if dep == Dep::Graph
            && let Some(Pairing::Incompatible(why)) = &self.pairing
        {
            return envelope("refused", None, Vec::new(), vec![why.clone()], Value::Null);
        }
        if dep == Dep::Graph
            && let Some(Pairing::Mismatch {
                graph_run,
                trace_run,
            }) = &self.pairing
        {
            return envelope(
                "refused",
                None,
                Vec::new(),
                vec![format!(
                    "the graph is for build {graph_run}, the trace run is {trace_run}; pass \
                     --graph= for that run or --run={graph_run}"
                )],
                Value::Null,
            );
        }
        let graph_gap = dep == Dep::Graph && !self.graph_complete();
        let degraded = !self.capture_complete() || graph_gap;
        let scope_limited = dep == Dep::Graph
            && claim == Claim::DescendantPath
            && self.graph_scope() == Some(CaptureScope::CurrentActionAncestors);
        let status = if degraded || scope_limited {
            "partial"
        } else {
            status
        };
        let bound = match claim {
            Claim::LongestPath | Claim::DescendantPath if degraded => {
                if self.omission_only() {
                    Some("lower")
                } else {
                    let repaired = self.graph.as_ref().is_some_and(|g| g.targets_in_cycles > 0)
                        || self.stitch.as_ref().is_some_and(|s| s.n_cycle_edges > 0);
                    lims.push(
                        if repaired {
                            REPAIRED_PATH
                        } else {
                            UNBOUNDED_PATH
                        }
                        .to_owned(),
                    );
                    None
                }
            }
            Claim::Savings if degraded => {
                lims.push(SAVINGS_CONDITIONAL.to_owned());
                None
            }
            _ => bound,
        };
        coverage.extend(self.coverage(dep));
        lims.extend(self.limitations(dep));
        let mut result = envelope(status, bound, coverage, lims, data);
        if dep == Dep::Graph
            && let Some(scope) = self
                .graph_scope()
                .filter(|scope| *scope != CaptureScope::Full)
        {
            result["graph_scope"] = json!(scope.as_str());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pairing_reports_every_missing_identity_without_claiming_a_mismatch() {
        for graph_run in [None, Some("")] {
            for trace_run in [None, Some("")] {
                let Pairing::Unverified(why) = pairing_of(graph_run, trace_run) else {
                    panic!("two absent identities must remain unverified");
                };
                assert!(why.contains("neither the graph header nor the trace"));
            }
        }
        assert!(matches!(
            pairing_of(Some("run"), Some("run")),
            Pairing::Verified
        ));
        assert!(matches!(
            pairing_of(Some("a"), Some("b")),
            Pairing::Mismatch { .. }
        ));
        for (graph_run, trace_run, missing) in [
            (None, Some("run"), "graph header names no run"),
            (Some("run"), None, "trace names no run"),
        ] {
            let Pairing::Unverified(why) = pairing_of(graph_run, trace_run) else {
                panic!("one absent identity must remain unverified");
            };
            assert!(why.contains(missing), "{why}");
        }
    }

    use super::*;
    use crate::testdata as fixture;
    use crate::types::CaptureStatus;

    fn trace_with_run(tag: &str, run: &str) -> String {
        let p = crate::temp_path(&format!("quality-{tag}.json"));
        std::fs::write(
            &p,
            format!(
                concat!(
                    "[\n",
                    r#"{{"ph":"M","cat":"run","name":"run_start","ts":0,"pid":1,"args":{{"v":1,"run":"{run}"}}}},"#,
                    "\n",
                    r#"{{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{{"run":"{run}"}}}},"#,
                    "\n",
                    r#"{{"ph":"X","cat":"recipe","name":"b","ts":10,"dur":5,"args":{{"run":"{run}"}}}},"#,
                    "\n",
                    r#"{{"ph":"M","cat":"run","name":"run_end","ts":20,"pid":1,"args":{{"v":1,"run":"{run}","exit":0}}}}"#,
                    "\n]\n"
                ),
                run = run
            ),
        )
        .unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_graph_from_another_run_is_refused_before_stitching_and_an_unnamed_run_is_a_limitation() {
        let (other, same, unnamed) = (
            fixture("review-mismatched-run-graph.jsonl"),
            fixture("review-declared-missing-graph.jsonl"),
            fixture("partial-graph.jsonl"),
        );
        let tp = trace_with_run("pairing", "trace-run");
        let tr = crate::parse::parse(&tp).unwrap();

        let g = crate::graph::load_graph(&other).unwrap();
        let q = Quality::assess(&tr, Some("trace-run"), Some(&g));
        assert!(
            matches!(&q.pairing, Some(Pairing::Mismatch { .. })),
            "{:?}",
            q.pairing
        );
        assert!(!q.graph_complete());
        let v = q.seal(
            Dep::Graph,
            Claim::LongestPath,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "refused");
        let msg = v["limitations"][0].as_str().unwrap();
        assert!(
            msg.contains("another-run") && msg.contains("trace-run"),
            "{msg}"
        );
        // a trace-only topic is untouched by the graph
        let v = q.seal(
            Dep::Trace,
            Claim::Measurement,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "available");

        let g = crate::graph::load_graph(&same).unwrap();
        let q = Quality::assess(&tr, Some("trace-run"), Some(&g));
        assert_eq!(q.pairing, Some(Pairing::Verified));

        let g = crate::graph::load_graph(&unnamed).unwrap();
        let q = Quality::assess(&tr, Some("trace-run"), Some(&g));
        assert!(matches!(&q.pairing, Some(Pairing::Unverified(_))));
        assert!(
            q.graph_limitations()
                .iter()
                .any(|l| l.contains("could not be checked")),
            "{:?}",
            q.graph_limitations()
        );
        std::fs::remove_file(&tp).ok();

        // the review's trace has no run records, but every action names
        // one run: that is the trace's run, and the mismatch is caught
        let bare = fixture("review-trace.json");
        let tr = crate::parse::parse(&bare).unwrap();
        assert!(tr.runs.is_empty());
        let g = crate::graph::load_graph(&other).unwrap();
        let q = Quality::assess(&tr, None, Some(&g));
        assert!(crate::dag::build_full(&tr, &g, None).is_none());
        assert!(crate::dag::cp_chain(&tr, &g, None).is_none());
        assert!(
            matches!(&q.pairing, Some(Pairing::Mismatch { .. })),
            "{:?}",
            q.pairing
        );
        let g = crate::graph::load_graph(&same).unwrap();
        let q = Quality::assess(&tr, None, Some(&g));
        assert_eq!(q.pairing, Some(Pairing::Verified));
    }

    #[test]
    fn artifact_losses_are_independent_of_a_complete_producer_capture() {
        use crate::types::CaptureCounts;
        use crate::types::Finality;
        let path = trace_with_run("artifact-health", "run");
        let mut tr = crate::parse::parse(&path).unwrap();
        let counts = CaptureCounts {
            actions_recorded: Some(2),
            tset_def_events: Some(0),
        };
        tr.runs[0].dep_capture = Some(CaptureState::Started(CaptureStatus {
            footer_present: true,
            finality: Finality::Final,
            declared: counts.clone(),
            observed: counts,
            ..Default::default()
        }));
        assert!(Quality::assess(&tr, None, None).capture_complete());
        tr.health.dropped = 1;
        let q = Quality::assess(&tr, None, None);
        assert!(!q.capture_complete());
        assert_eq!(q.capture.as_ref().unwrap().is_complete(), Some(true));
        let v = q.seal(
            Dep::Trace,
            Claim::Measurement,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "partial");
        assert!(
            q.capture_limitations()
                .iter()
                .any(|p| p.contains("exported artifact"))
        );
        tr.health.dropped = 0;
        tr.runs[0].ended = false;
        assert!(!Quality::assess(&tr, None, None).capture_complete());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn surplus_or_contradictory_counts_never_claim_an_omission_bound() {
        let path = trace_with_run("surplus-health", "run");
        let tr = crate::parse::parse(&path).unwrap();
        let mut q = Quality::assess(&tr, None, None);
        q.graph = Some(GraphQuality {
            declared_nodes: Some(1),
            loaded_rows: 2,
            ..Default::default()
        });
        for bad in [
            GraphQuality {
                declared_nodes: Some(1),
                loaded_rows: 2,
                ..Default::default()
            },
            GraphQuality {
                declared_edges: Some(-1),
                ..Default::default()
            },
            GraphQuality {
                contradictory_counts: true,
                ..Default::default()
            },
        ] {
            q.graph = Some(bad);
            let v = q.seal(
                Dep::Graph,
                Claim::LongestPath,
                "available",
                Some("lower"),
                vec![],
                vec![],
                Value::Null,
            );
            assert_eq!(v["status"], "partial");
            assert!(v.get("bound").is_none(), "{v}");
            assert!(
                v["limitations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p.as_str() == Some(UNBOUNDED_PATH))
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn exact_empty_coverage_and_once_only_limitations() {
        use crate::types::CaptureCounts;
        let path = trace_with_run("zero-coverage", "run");
        let tr = crate::parse::parse(&path).unwrap();
        let mut q = Quality::assess(&tr, None, None);
        let counts = CaptureCounts {
            actions_recorded: Some(0),
            ..Default::default()
        };
        q.capture = Some(CaptureState::Started(CaptureStatus {
            declared: counts.clone(),
            observed: counts,
            ..Default::default()
        }));
        q.graph = Some(GraphQuality {
            declared_nodes: Some(0),
            declared_edges: Some(0),
            ..Default::default()
        });
        q.stitch = Some(StitchQuality::default());
        let coverage = q.coverage(Dep::Graph);
        assert_eq!(coverage.len(), 4);
        assert!(coverage.iter().all(|(_, n, d)| (*n, *d) == (0, 0)));
        let limitations = q.limitations(Dep::Graph);
        let result = q.seal(
            Dep::Graph,
            Claim::Measurement,
            "available",
            None,
            vec![],
            limitations,
            Value::Null,
        );
        let values = result["limitations"].as_array().unwrap();
        let unique: std::collections::HashSet<_> =
            values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(values.len(), unique.len());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_repaired_graph_withdraws_the_bound_and_an_omission_keeps_it() {
        let base = Quality {
            graph_warning: None,
            capture: None,
            outcome: Outcome::Success,
            outcome_unrecorded: false,
            dropped_trace_lines: 0,
            unended_runs: 0,
            graph: Some(GraphQuality {
                declared_nodes: Some(3),
                loaded_rows: 2,
                ..Default::default()
            }),
            stitch: None,
            pairing: Some(Pairing::Verified),
        };
        let v = base.seal(
            Dep::Graph,
            Claim::LongestPath,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "partial");
        assert_eq!(v["bound"], "lower");
        assert!(
            v["coverage"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["scope"] == "graph-rows-declared"
                    && c["numerator"] == 2
                    && c["denominator"] == 3),
            "{v}"
        );
        let v = base.seal(
            Dep::Graph,
            Claim::Savings,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "partial");
        assert!(v.get("bound").is_none());
        assert!(
            v["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l == SAVINGS_CONDITIONAL)
        );

        let mut repaired = base.clone();
        repaired.graph.as_mut().unwrap().targets_in_cycles = 2;
        let v = repaired.seal(
            Dep::Graph,
            Claim::LongestPath,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "partial");
        assert!(v.get("bound").is_none(), "{v}");
        assert!(
            v["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap().contains("repaired graph")),
            "{v}"
        );
    }

    #[test]
    fn an_incomplete_capture_degrades_a_trace_topic_and_a_missing_one_does_not() {
        let mut q = Quality {
            graph_warning: None,
            capture: Some(CaptureState::Started(CaptureStatus::default())),
            outcome: Outcome::Success,
            outcome_unrecorded: false,
            dropped_trace_lines: 0,
            unended_runs: 0,
            graph: None,
            stitch: None,
            pairing: None,
        };
        let v = q.seal(
            Dep::Trace,
            Claim::Measurement,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "partial");
        assert!(
            v["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap().contains("no dependency-capture footer"))
        );
        q.capture = Some(CaptureState::NotEnabled);
        assert!(q.capture_complete());
        q.capture = None;
        assert!(q.capture_complete());
        let v = q.seal(
            Dep::Trace,
            Claim::Measurement,
            "available",
            None,
            vec![],
            vec![],
            Value::Null,
        );
        assert_eq!(v["status"], "available");
        assert_eq!(v["limitations"].as_array().unwrap().len(), 0);
    }
}
