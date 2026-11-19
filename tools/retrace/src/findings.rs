/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Ranked findings: the build's problems as a single ordered list.
//!
//! Each finding class is grounded in fields the capture actually
//! carries (stage timings, cache hits, retries, the concurrency
//! profile) and, when the dependency DAG is present, costed by
//! CP replay — remove/shrink the offender, recompute the longest
//! path, report the delta. The `estimate` label keeps the two kinds
//! of number apart:
//!
//! - `cp-replay`: the change to the dependency floor (sim.rs)
//! - `bound`: an upper bound on the recoverable time, no replay
//! - `none`: a cost with no defensible opportunity number
//!
//! Findings never silently vanish for lack of data — a class that
//! cannot run degrades its estimate label instead.

use crate::analyses::Facts;
use crate::analyses::UploadSub;
use crate::analyses::fmt_bytes;
use crate::analyses::fmt_dur;
use crate::analyses::peer_stats;
use crate::analyses::stall_audit;
use crate::dag::DagBundle;
use crate::sim;
use crate::sim::SimInput;
use crate::types::F_CACHE_HIT;
use crate::types::F_FAILED;

#[derive(Clone, Debug)]
pub struct Finding {
    pub class: &'static str,
    pub title: String,
    /// `Facts` event index for click-through, when one action anchors it
    pub ev: Option<u32>,
    /// observed time attributed to the problem
    pub cost_us: i64,
    /// Conditional opportunity under the stated intervention; not a
    /// prediction of real build recovery. `None` when unsupported.
    pub opportunity_us: Option<i64>,
    /// `cp-replay` | `bound` | `none`
    pub estimate: &'static str,
    pub detail: String,
}

pub struct FindingsReport {
    pub rows: Vec<Finding>,
    /// Findings before `rows` keeps the top [`FINDINGS_N`].
    pub n_found: usize,
    pub nacts: usize,
    pub with_stages: usize,
    /// present when opportunities were CP-replayed
    pub base_cp_us: Option<i64>,
}

/// The DAG side of findings: the bundle, its per-node costs, and the
/// event -> node map (`u32::MAX` = the event built no node, e.g. it
/// belongs to another run).
pub struct DagCtx<'a> {
    pub b: &'a DagBundle,
    pub si: &'a SimInput,
    node_of_ev: Vec<u32>,
    replay: std::cell::RefCell<Option<ReplayBuffers>>,
}

struct ReplayBuffers {
    weights: Vec<i64>,
    scratch: sim::ReplayScratch,
}

impl<'a> DagCtx<'a> {
    pub fn new(b: &'a DagBundle, si: &'a SimInput) -> DagCtx<'a> {
        Self::new_cancellable(b, si, &|| false).expect("an uncancelled findings index completes")
    }

    pub fn new_cancellable(
        b: &'a DagBundle,
        si: &'a SimInput,
        cancelled: &impl Fn() -> bool,
    ) -> Option<DagCtx<'a>> {
        if cancelled() {
            return None;
        }
        let mut node_of_ev = vec![u32::MAX; b.events.len()];
        for (node, e) in b.node_ev.iter().enumerate() {
            if node.is_multiple_of(4096) && cancelled() {
                return None;
            }
            if let Some(e) = e {
                node_of_ev[*e] = node as u32;
            }
        }
        Some(DagCtx {
            b,
            si,
            node_of_ev,
            replay: std::cell::RefCell::new(None),
        })
    }

    fn node(&self, ev: u32) -> Option<usize> {
        match self.node_of_ev.get(ev as usize) {
            Some(&n) if n != u32::MAX => Some(n as usize),
            _ => None,
        }
    }

    /// CP delta from setting one node's weight.
    fn replay_set(&self, node: usize, w_new: i64, cancelled: &impl Fn() -> bool) -> Option<i64> {
        if cancelled() {
            return None;
        }
        let mut buffers = self.replay.borrow_mut();
        let ReplayBuffers { weights, scratch } = buffers.get_or_insert_with(|| ReplayBuffers {
            weights: self.si.base.clone(),
            scratch: sim::ReplayScratch::new(self.si.base.len()),
        });
        scratch.savings_at(
            &self.b.dag,
            weights,
            self.b.dag.cp,
            node as u32,
            w_new,
            cancelled,
        )
    }
}

pub const FINDINGS_N: usize = 40;

/// Replayed opportunities first, then bounds and observations with no
/// defensible opportunity. Parallel-lane cost does not outrank a replay.
pub fn findings(f: &Facts, d: Option<&DagCtx>) -> FindingsReport {
    findings_cancellable(f, d, &|| false).expect("an uncancelled findings report completes")
}

/// Superseded requests return no report, including when cancellation
/// interrupts a per-candidate replay. Shared census/sort calls are checked
/// on entry and return; this is cooperative cancellation, not preemption.
pub fn findings_cancellable(
    f: &Facts,
    d: Option<&DagCtx>,
    cancelled: &impl Fn() -> bool,
) -> Option<FindingsReport> {
    if cancelled() {
        return None;
    }
    let mut rows: Vec<Finding> = Vec::new();
    let mut with_stages = 0usize;
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if f.args(e).stages.is_some() {
            with_stages += 1;
        }
    }

    queue_tax(f, d, &mut rows, cancelled)?;
    cache_lookup_tax(f, d, &mut rows, cancelled)?;
    permit_wait_tax(f, d, &mut rows, cancelled)?;
    local_race_upload_wait(f, d, &mut rows, cancelled)?;
    cp_wait_share(d, &mut rows, cancelled)?;
    fetch_bound(f, d, &mut rows, cancelled)?;
    cache_miss_hotspots(f, d, &mut rows, cancelled)?;
    peer_outliers(f, d, &mut rows, cancelled)?;
    overhead_tax(f, d, &mut rows, cancelled)?;
    retry_tax(f, d, &mut rows, cancelled)?;
    stall_cohorts(f, &mut rows, cancelled)?;
    serialization_windows(f, &mut rows, cancelled)?;

    rows.sort_by_key(rank_key);
    let n_found = rows.len();
    rows.truncate(FINDINGS_N);
    if cancelled() {
        return None;
    }
    Some(FindingsReport {
        rows,
        n_found,
        nacts: f.sel.len(),
        with_stages,
        base_cp_us: d.map(|d| d.b.dag.cp),
    })
}

fn rank_key(r: &Finding) -> (u8, std::cmp::Reverse<i64>) {
    (
        match r.estimate {
            "cp-replay" => 0,
            "bound" => 1,
            _ => 2,
        },
        std::cmp::Reverse(r.opportunity_us.unwrap_or(r.cost_us)),
    )
}

/// The CAS-call average for the permit-wait finding; an unmeasured call is not reported as zero.
fn cas_call_note(cas: &crate::upload::SubStat) -> String {
    match i64::try_from(cas.n_measured) {
        Ok(n) if n > 0 => format!(
            "the CAS call averaged {} per covered action",
            fmt_dur(cas.sum_us / n)
        ),
        _ => "the CAS call time was not recorded".to_owned(),
    }
}

fn replay_note(d: Option<&DagCtx>, intervention: &str) -> String {
    if d.is_some() {
        format!(
            "conditional CP replay: {intervention}; counters may overlap, recorded execution is retained"
        )
    } else {
        "no dependency DAG: parallel action cost only, no replay".to_owned()
    }
}

fn cache_lookup_tax(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let mut total = 0i64;
    let mut n = 0usize;
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(st) = f.args(e).stages.filter(|s| s.cache_query_us > 0) {
            total += st.cache_query_us;
            n += 1;
        }
    }
    if n == 0 {
        return Some(());
    }
    let opp = global_replay(d, |s| s.cache_query_us, cancelled)?;
    out.push(Finding {
        class: "cache_lookup_tax",
        title: format!("cache lookups: {} across {n} actions", fmt_dur(total)),
        ev: None,
        cost_us: total,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "bound" },
        detail: format!(
            "cache-query wall counters include any queueing in the lookup span; \
            this is not storage service time; {}",
            replay_note(d, "recorded cache lookups removed")
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

/// Zero one stage across all nodes and report the CP delta (`None`
/// without a DAG).
fn global_replay(
    d: Option<&DagCtx>,
    pick: impl Fn(&crate::analyses::Stages) -> i64,
    cancelled: &impl Fn() -> bool,
) -> Option<Option<i64>> {
    let Some(d) = d else { return Some(None) };
    let (w, _) = sim::w_zero_stage_cancellable(d.si, pick, cancelled)?;
    Some(Some(
        d.b.dag.cp - sim::replay_cancellable(&d.b.dag, &w, cancelled)?.cp_us,
    ))
}

fn queue_tax(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let Some(q) = crate::analyses::queue_overview(f) else {
        return (!cancelled()).then_some(());
    };
    if cancelled() {
        return None;
    }
    if q.waited == 0 {
        return Some(());
    }
    let opp = global_replay(d, |st| st.queue_us.unwrap_or(0), cancelled)?;
    out.push(Finding {
        class: "re_queue_tax",
        title: format!(
            "RE queue: {} actions waited {} in total",
            crate::analyses::fmt_n(q.waited as f64),
            fmt_dur(q.total_us)
        ),
        ev: Some(q.max_ev),
        cost_us: q.total_us,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "bound" },
        detail: format!(
            "mean {} over {} measured actions (including zeros), p90 {}, max {} ({}); {}",
            fmt_dur(q.mean_us),
            q.covered,
            fmt_dur(q.p90_us),
            fmt_dur(q.max_us),
            f.name(q.max_ev),
            replay_note(d, "every recorded queue wait removed")
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

fn permit_wait_tax(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let Some(r) = crate::upload::upload_report(f) else {
        return (!cancelled()).then_some(());
    };
    if cancelled() {
        return None;
    }
    let s = &r.subs[UploadSub::PHASE_PERMIT_WAIT];
    if s.n == 0 {
        return Some(());
    }
    let opp = if let Some(d) = d {
        let (w, _) = sim::w_zero_permit_wait_cancellable(d.si, cancelled)?;
        Some(d.b.dag.cp - sim::replay_cancellable(&d.b.dag, &w, cancelled)?.cp_us)
    } else {
        None
    };
    out.push(Finding {
        class: "permit_wait_tax",
        title: format!(
            "upload permit wait: {} actions queued {} for a client upload slot",
            crate::analyses::fmt_n(s.n as f64),
            crate::analyses::fmt_secs(s.sum_us as f64)
        ),
        ev: s.max_ev,
        cost_us: s.sum_us,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "bound" },
        detail: format!(
            "p50 {}, p90 {}, max {} ({}); dead queueing for buck2's upload permits, \
             not transfer — {}; {}",
            fmt_dur(s.p50_us),
            fmt_dur(s.p90_us),
            fmt_dur(s.max_us),
            s.max_ev.map(|e| f.name(e)).unwrap_or("not recorded"),
            cas_call_note(&r.subs[UploadSub::PHASE_CAS_CALL]),
            replay_note(d, "every recorded upload-permit wait removed"),
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

/// Uploads that waited on local files while the same action's losing local attempt was preparing
/// its inputs on this machine.
fn local_race_upload_wait(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let Some(r) = crate::upload::upload_report(f) else {
        return (!cancelled()).then_some(());
    };
    let Some(&(worst, worst_us)) = r.local_race.first() else {
        return Some(());
    };
    let total: i64 = r.local_race.iter().map(|&(_, us)| us).sum();
    let (opp, on_cp) = if let Some(d) = d {
        let mut w = d.si.base.clone();
        let mut on_cp = 0usize;
        for &(e, gather) in &r.local_race {
            if let Some(node) = d.node(e) {
                w[node] = (w[node] - gather).max(0);
                on_cp += usize::from(d.b.dag.cp_path.contains(&(node as u32)));
            }
        }
        let replay = sim::replay_cancellable(&d.b.dag, &w, cancelled)?;
        (Some(d.b.dag.cp - replay.cp_us), Some(on_cp))
    } else {
        (None, None)
    };
    out.push(Finding {
        class: "local_race_upload_wait",
        title: format!(
            "local attempts slowed RE input uploads: {} actions spent {} gathering local files \
             while their local attempt was preparing the same inputs",
            crate::analyses::fmt_n(r.local_race.len() as f64),
            crate::analyses::fmt_secs(total as f64)
        ),
        ev: Some(worst),
        cost_us: total,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "bound" },
        detail: format!(
            "worst {} ({}){}; buck2 ran these actions locally and remotely at once and the remote \
             side won, but the upload's files and the local attempt's inputs both come through this \
             machine's materializer. `--prefer-remote` avoids the race. {}",
            fmt_dur(worst_us),
            f.name(worst),
            on_cp
                .map(|n| format!(", {n} on the critical path"))
                .unwrap_or_default(),
            replay_note(d, "each affected upload's file gathering removed"),
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

fn cp_wait_share(
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let Some(d) = d else { return Some(()) };
    let mut candidates = Vec::new();
    for (step, &node) in d.b.dag.cp_path.iter().enumerate() {
        if step.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let i = node as usize;
        let Some(ev) = d.b.node_ev[i] else { continue };
        let Some(st) = d.si.stages[i] else { continue };
        let dur = d.si.base[i];
        let new_weight = if d.si.cache_hit[i] {
            dur.saturating_sub(st.cache_query_us).max(0)
        } else {
            sim::non_execution_weight(d.si, i)
        };
        let cut = dur - new_weight;
        if dur >= 5_000_000 && cut > 0 && cut >= dur / 2 {
            candidates.push((i, ev, cut, new_weight));
        }
    }
    candidates.sort_by_key(|&(_, ev, cut, _)| (std::cmp::Reverse(cut), ev));
    let mut cands: Vec<Finding> = Vec::new();
    for (i, ev, cut, new_weight) in candidates {
        // A step's CP saving cannot exceed the duration removed from it.
        if cands.len() == 10 && cut < cands[9].opportunity_us.unwrap_or(0) {
            break;
        }
        let st = d.si.stages[i].expect("candidate carries stages");
        let dur = d.si.base[i];
        let opp = d.replay_set(i, new_weight, cancelled)?;
        let mut parts: Vec<String> = Vec::new();
        for (label, us) in [
            ("lookup", st.cache_query_us),
            ("cached result", st.cache_hit_us),
            ("request preparation", st.prepare_us),
            ("CAS artifact preparation", st.cas_artifact_us),
            ("local output finalization", st.local_finalize_outputs_us),
            ("local cache check", st.local_cache_check_us),
            (
                "hashing",
                crate::analyses::hashing_composition_us(
                    st.hashing_us,
                    st.hashing_in_finalize_outputs,
                ),
            ),
            ("upload-in", st.input_upload_us),
            ("queue", st.queue_us.unwrap_or(0)),
            ("fetch", st.materialize_us),
            ("upload", st.upload_us),
            ("result wait", st.result_wait_us),
            ("download", st.download_us),
            ("submit window", d.si.submit[i]),
        ] {
            if us > 0 {
                parts.push(format!("{label} {}", fmt_dur(us)));
            }
        }
        cands.push(Finding {
            class: "cp_wait_share",
            title: format!(
                "critical-path {} {}: {}% {} counter contribution",
                if d.si.cache_hit[i] {
                    "cache hit"
                } else {
                    "step"
                },
                d.b.events[ev].name,
                cut * 100 / dur.max(1),
                if d.si.cache_hit[i] {
                    "lookup"
                } else {
                    "non-execution"
                },
            ),
            ev: Some(ev as u32),
            cost_us: cut,
            opportunity_us: Some(opp),
            estimate: "cp-replay",
            detail: format!(
                "{} wall; {}; conditional CP replay removes {} from this step, \
                 retaining unclassified residual and recorded execution. Counter sums \
                 are not a timeline or evidence of idle time",
                fmt_dur(dur),
                parts.join(", "),
                if d.si.cache_hit[i] {
                    "the recorded cache lookup only"
                } else {
                    "recorded non-execution counters"
                },
            ),
        });
        cands.sort_by_key(|c| (std::cmp::Reverse(c.opportunity_us), c.ev));
        cands.truncate(10);
    }
    out.extend(cands);
    if cancelled() {
        return None;
    }
    Some(())
}

fn fetch_bound(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let mut heavy = Vec::new();
    let mut total = 0;
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(stage) = f.args(e).stages
            && f.dur(e) >= 5_000_000
            && stage.materialize_us * 2 >= f.dur(e)
        {
            total += stage.materialize_us;
            heavy.push((
                e,
                stage.materialize_us,
                f.args(e).bytes_in_recorded.then_some(f.args(e).bytes_in),
            ));
        }
    }
    if heavy.is_empty() {
        return Some(());
    }
    heavy.sort_by_key(|&(_, m, _)| std::cmp::Reverse(m));
    // Only these actions' preparation is removed, so the opportunity matches the reported cost.
    let opp = match d {
        None => None,
        Some(d) => {
            let mut selected = vec![false; d.si.base.len()];
            for node in heavy.iter().filter_map(|&(e, _, _)| d.node(e)) {
                selected[node] = true;
            }
            let (w, _) = sim::w_zero_stage_where_cancellable(
                d.si,
                |node| selected[node],
                |st| st.materialize_us,
                cancelled,
            )?;
            Some(d.b.dag.cp - sim::replay_cancellable(&d.b.dag, &w, cancelled)?.cp_us)
        }
    };
    let (e0, m0, b0) = heavy[0];
    out.push(Finding {
        class: "fetch_bound",
        title: format!(
            "{} input-preparation-heavy actions (counter >= half their wall time)",
            crate::analyses::fmt_n(heavy.len() as f64)
        ),
        ev: Some(e0),
        cost_us: total,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "bound" },
        detail: format!(
            "worst: {}; {}; preparation took {}; logical input size is not bytes \
             transferred; {}",
            f.name(e0),
            b0.map(|n| format!("declared input size {}", fmt_bytes(n)))
                .unwrap_or_else(|| "input size not recorded".to_owned()),
            fmt_dur(m0),
            replay_note(d, "these actions' recorded input preparation removed")
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

fn cache_miss_hotspots(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    // Keep measured hit costs separate from successful, eligible misses.
    let mut hit_costs = Vec::new();
    let mut misses = Vec::new();
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(cost) = f.args(e).cache.and_then(|c| c.lookup_us) {
            hit_costs.push(cost);
        }
        if f.act(e).flags & (F_CACHE_HIT | F_FAILED) == 0
            && !f.args(e).cache_ineligible
            && f.dur(e) >= 10_000_000
        {
            misses.push(e);
        }
    }
    let hit_cost = if hit_costs.is_empty() {
        None
    } else {
        let mid = crate::analyses::percentile_index(hit_costs.len(), 50);
        Some(*hit_costs.select_nth_unstable(mid).1)
    };
    drop(hit_costs);

    misses.sort_by_key(|&e| std::cmp::Reverse(f.dur(e)));
    for &e in misses.iter().take(5) {
        let dur = f.dur(e);
        let (opp, estimate, tail) = match (hit_cost, d) {
            (Some(hc), Some(d)) => match d.node(e) {
                Some(node) => (
                    Some(d.replay_set(node, hc.min(dur), cancelled)?),
                    "cp-replay",
                    format!(
                        "opportunity = this action at the median recorded lookup {} \
                         (lookup-only model; restore excluded)",
                        fmt_dur(hc)
                    ),
                ),
                None => (None, "bound", "no DAG node for this action".to_owned()),
            },
            (Some(hc), None) => (
                Some(dur - hc.min(dur)),
                "bound",
                format!(
                    "no DAG: bound = wall minus median recorded lookup {} (lookup-only model; restore excluded)",
                    fmt_dur(hc)
                ),
            ),
            (None, _) => (
                None,
                "none",
                "no hit with a recorded lookup time to model from".to_owned(),
            ),
        };
        out.push(Finding {
            class: "cache_miss_hotspot",
            title: format!("{} took {}", f.name(e), fmt_dur(dur)),
            ev: Some(e),
            cost_us: dur,
            opportunity_us: opp,
            estimate,
            detail: format!(
                "hypothetical cache reuse replaces this successful action's span; {tail}"
            ),
        });
    }
    if cancelled() {
        return None;
    }
    Some(())
}

fn peer_outliers(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let ps = peer_stats(f);
    if cancelled() {
        return None;
    }
    let mut cands: Vec<(u32, i64, i64, String)> = Vec::new();
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let dur = f.dur(e);
        if dur < 10_000_000 {
            continue;
        }
        let Some(r) = ps.rank(f, e) else { continue };
        if r.pct >= 95 && dur >= 2 * r.median_us.max(1) {
            cands.push((e, dur, r.median_us, r.cat));
        }
    }
    cands.sort_by_key(|&(_, dur, med, _)| std::cmp::Reverse(dur - med));
    for (e, dur, med, cat) in cands.into_iter().take(10) {
        let opp = if let Some((d, node)) = d.and_then(|d| d.node(e).map(|n| (d, n))) {
            Some(d.replay_set(node, med, cancelled)?)
        } else {
            None
        };
        out.push(Finding {
            class: "peer_outlier",
            title: format!(
                "{} runs {:.1}x its {} peers",
                f.name(e),
                dur as f64 / med.max(1) as f64,
                cat
            ),
            ev: Some(e),
            cost_us: dur - med,
            opportunity_us: opp,
            estimate: if opp.is_some() { "cp-replay" } else { "bound" },
            detail: format!(
                "{} vs median {} within the same category and cache-hit/execution population; {}",
                fmt_dur(dur),
                fmt_dur(med),
                if d.is_some() {
                    "conditional CP replay sets this entire action duration to its population median; this hypothetical may reduce execution as well as non-execution time"
                } else {
                    "no dependency DAG: the duration difference is not a replay or measured build saving"
                },
            ),
        });
    }
    if cancelled() {
        return None;
    }
    Some(())
}

fn overhead_tax(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let mut total = 0i64;
    let mut n = 0usize;
    let mut pipe_total = 0i64;
    let mut pipe_n = 0usize;
    let mut has_rtt = false;
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(st) = f.args(e).stages {
            has_rtt |= st.rtt_us > 0;
            let o = st.rtt_us + st.daemon_us;
            if o > 0 {
                total += o;
                n += 1;
            }
            let p = st.input_upload_us + st.result_wait_us + st.download_us;
            if p > 0 {
                pipe_total += p;
                pipe_n += 1;
            }
        }
    }
    if pipe_total > 0 {
        let opp = global_replay(
            d,
            |st| st.input_upload_us + st.result_wait_us + st.download_us,
            cancelled,
        )?;
        out.push(Finding {
            class: "client_pipeline_tax",
            title: format!(
                "RE transfer counters (input upload + result wait + download): {} across {} \
                 actions",
                fmt_dur(pipe_total),
                crate::analyses::fmt_n(pipe_n as f64)
            ),
            ev: None,
            cost_us: pipe_total,
            opportunity_us: opp,
            estimate: if opp.is_some() { "cp-replay" } else { "bound" },
            detail: format!(
                "recorded input-upload, result-wait and download counters; result wait can \
                 include the worker's own upload; {}; \
                no storage or network cause is established",
                replay_note(d, "these pipeline counters removed")
            ),
        });
    }
    if total == 0 {
        return Some(());
    }
    let opp = global_replay(d, |st| st.rtt_us + st.daemon_us, cancelled)?;
    out.push(Finding {
        class: "overhead_rtt_daemon",
        title: format!(
            "unclassified client-span residual{}: {} across {} actions",
            if has_rtt { " + legacy rtt" } else { "" },
            fmt_dur(total),
            crate::analyses::fmt_n(n as f64)
        ),
        ev: None,
        cost_us: total,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "bound" },
        detail: format!(
            "derived counter residual, not a positioned idle interval or \
            daemon CPU measurement; {}",
            replay_note(d, "hypothetically remove this residual")
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

/// The retry finding reads the same attempt census as `--json=retries`,
/// so the two surfaces cannot disagree about what a retry cost. A
/// discarded attempt's command duration is not its worker CPU cost, and
/// overlap does not prove zero contention or zero build impact. Replay
/// removes only the classified pre-winner duration budget. This is a
/// conditional calculation on fixed costs and edges, not a causal bound
/// on the benefit of eliminating retries (docs/contract.md, "Retry fates").
fn retry_tax(
    f: &Facts,
    d: Option<&DagCtx>,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    let Some(c) = crate::analyses::retry_census(f, usize::MAX) else {
        return (!cancelled()).then_some(());
    };
    if cancelled() {
        return None;
    }
    let Some(worst) = c.rows.first() else {
        return Some(());
    };
    let with_attempts = c.rows.iter().filter(|r| !r.attempts.is_empty()).count();
    // Older captures carry a retry count and no attempt records: they
    // say that a retry happened and nothing about what it cost, so
    // they contribute no time and no opportunity. Attempts whose fate
    // is unknown likewise support no delay claim, so the scenario covers
    // the classified attempts only and is absent when there are none.
    let count_only = c.actions - with_attempts;
    let classified = c.concurrent + c.rerun;
    let opp = if with_attempts == 0 || classified == 0 {
        None
    } else if let Some(d) = d {
        let mut w = d.si.base.clone();
        for (index, row) in c.rows.iter().enumerate() {
            if index.is_multiple_of(4096) && cancelled() {
                return None;
            }
            if row.delay_us > 0
                && let Some(node) = d.node(row.ev)
            {
                w[node] = w[node]
                    .saturating_sub(row.delay_us)
                    .max(sim::execution_floor(d.si, node));
            }
        }
        Some(d.b.dag.cp - sim::replay_cancellable(&d.b.dag, &w, cancelled)?.cp_us)
    } else {
        None
    };
    let mut notes = String::new();
    if c.unknown > 0 {
        notes.push_str(&format!(
            "; {} discarded attempts carry no interval retrace can compare and are outside \
             this scenario",
            c.unknown
        ));
    }
    if count_only > 0 {
        notes.push_str(&format!(
            "; {count_only} carry only a retry count with no attempt records and contribute \
             no time"
        ));
    }
    out.push(Finding {
        class: "retry_tax",
        title: format!(
            "{} actions needed retries: {} of discarded command time; {} \
             classified pre-winner duration budget",
            crate::analyses::fmt_n(c.actions as f64),
            fmt_dur(c.lost_us),
            fmt_dur(c.delay_us)
        ),
        ev: Some(worst.ev),
        cost_us: c.lost_us,
        opportunity_us: opp,
        estimate: if opp.is_some() { "cp-replay" } else { "none" },
        detail: format!(
            "worst: {} took {} attempts ({} discarded); {} discarded attempts overlapped the \
             attempt that won, {} ended before it started. Opportunity is conditional CP \
             replay with only the classified pre-winner budget removed; overlap proves \
             neither zero contention nor zero build impact{}",
            f.name(worst.ev),
            worst.tries,
            fmt_dur(worst.lost_us),
            c.concurrent,
            c.rerun,
            notes
        ),
    });
    if cancelled() {
        return None;
    }
    Some(())
}

fn stall_cohorts(f: &Facts, out: &mut Vec<Finding>, cancelled: &impl Fn() -> bool) -> Option<()> {
    if cancelled() {
        return None;
    }
    let r = stall_audit(f);
    if cancelled() {
        return None;
    }
    let mut unmeasured = fxhash::FxHashMap::<i64, usize>::default();
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        // The cohorts' own test: a stage-less action counts unless its admission wait is 5% or more.
        if f.args(e).stages.is_none()
            && f.dur(e) >= 30_000_000
            && f.args(e).exec_sem_wait_us < f.dur(e) / 20
        {
            *unmeasured.entry(f.dur(e) / 5_000_000).or_default() += 1;
        }
    }
    for c in r.cohorts.iter().take(5) {
        out.push(Finding {
            class: "stall_cohort",
            title: format!(
                "{} actions of ~{} with only {:.0}% stage-attributed",
                c.n,
                fmt_dur(c.dur_typ),
                c.staged_frac * 100.0
            ),
            ev: Some(c.sample),
            cost_us: c.lost_us,
            opportunity_us: None,
            estimate: "none",
            detail: format!(
                "duration-bucket heuristic; {} actions carry no stage counters; {} summed \
                 parallel action time is not covered by recorded counters (e.g. {}). \
                 Temporal overlap and cause are not established",
                unmeasured
                    .get(&(c.dur_typ / 5_000_000))
                    .copied()
                    .unwrap_or(0),
                fmt_dur(c.lost_us),
                f.name(c.sample)
            ),
        });
    }
    if cancelled() {
        return None;
    }
    Some(())
}

fn serialization_windows(
    f: &Facts,
    out: &mut Vec<Finding>,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    // sweep the selected events for spans where <=1 action is running
    let mut edges: Vec<(i64, i32)> = Vec::with_capacity(2 * f.sel.len());
    for (index, &e) in f.sel.iter().enumerate() {
        if index.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if f.dur(e) <= 0 {
            continue;
        }
        edges.push((f.ts(e), 1));
        edges.push((f.ts(e) + f.dur(e), -1));
    }
    if edges.is_empty() {
        return Some(());
    }
    edges.sort_unstable();
    let t_end = edges.last().expect("non-empty").0;
    let mut active = 0i32;
    // the build starts idle: the window is open from the first edge
    let mut low_since: Option<i64> = Some(edges[0].0);
    let mut windows: Vec<(i64, i64, i64)> = Vec::new();
    let mut idle_us = 0;
    let mut previous = edges[0].0;
    let mut cursor = 0;
    while cursor < edges.len() {
        if cancelled() {
            return None;
        }
        let t = edges[cursor].0;
        if active == 0 && low_since.is_some() {
            idle_us += t - previous;
        }
        let was_low = active <= 1;
        while cursor < edges.len() && edges[cursor].0 == t {
            if cursor.is_multiple_of(4096) && cancelled() {
                return None;
            }
            active += edges[cursor].1;
            cursor += 1;
        }
        let is_low = active <= 1;
        match (was_low, is_low) {
            (false, true) => {
                low_since = Some(t);
                idle_us = 0;
            }
            (true, false) => {
                if let Some(s) = low_since.take() {
                    if t - s >= 5_000_000 {
                        windows.push((s, t, idle_us));
                    }
                }
            }
            _ => {}
        }
        previous = t;
    }
    if let Some(s) = low_since {
        if t_end - s >= 5_000_000 {
            windows.push((s, t_end, idle_us));
        }
    }
    windows.sort_by_key(|&(s, t, _)| std::cmp::Reverse(t - s));
    for &(s, t, idle) in windows.iter().take(5) {
        // the action holding the window, if one spans it
        let mut runner = None;
        let mut n_actions = 0;
        for (index, &e) in f.sel.iter().enumerate() {
            if index.is_multiple_of(4096) && cancelled() {
                return None;
            }
            if runner.is_none() && f.ts(e) <= s && f.ts(e) + f.dur(e) >= t {
                runner = Some(e);
            }
            n_actions += usize::from(f.ts(e) < t && f.ts(e) + f.dur(e) > s);
        }
        out.push(Finding {
            class: "serialization_window",
            title: format!("{} window with at most one action running", fmt_dur(t - s)),
            ev: runner,
            cost_us: t - s,
            opportunity_us: None,
            estimate: "none",
            detail: format!(
                "{n_actions} actions overlap this window; no recorded action \
                running for {}. {}. Readiness and the cause of limited concurrency \
                are not recorded",
                fmt_dur(idle),
                runner
                    .map(|e| format!("{} spans it", f.name(e)))
                    .unwrap_or_else(|| "No single action spans it".to_owned())
            ),
        });
    }
    if cancelled() {
        return None;
    }
    Some(())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmeasured_cas_call_is_not_reported_as_zero() {
        let unmeasured = crate::upload::SubStat::default();
        assert_eq!(
            cas_call_note(&unmeasured),
            "the CAS call time was not recorded"
        );
        let measured = crate::upload::SubStat {
            n_measured: 2,
            sum_us: 3_000,
            ..Default::default()
        };
        assert!(cas_call_note(&measured).starts_with("the CAS call averaged"));
    }
    use crate::analyses::Facts;
    use crate::analyses::load_extras;
    use crate::graph::load_graph;
    use crate::parse;

    #[test]
    fn queue_finding_names_the_mean_population_including_measured_zeros() {
        let (tr, _) = fixture();
        let mut x = load_extras(&tr);
        std::fs::remove_file(&tr.path).unwrap();
        let mut covered = 0;
        for args in &mut x.args {
            if let Some(stage) = &mut args.stages {
                stage.queue_us = Some(if covered == 0 { 5_000_000 } else { 0 });
                covered += 1;
            }
        }
        assert!(covered > 1);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let report = findings(&f, None);
        let queue = report
            .rows
            .iter()
            .find(|row| row.class == "re_queue_tax")
            .unwrap();
        assert!(queue.title.contains("1 actions waited"));
        assert!(queue.detail.contains(&format!(
            "mean {} over {covered} measured actions (including zeros)",
            fmt_dur(5_000_000 / covered as i64),
        )));
    }

    #[test]
    fn findings_cancel_without_returning_partial_ranked_rows() {
        let (tr, g) = fixture();
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).unwrap();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        assert!(DagCtx::new_cancellable(&b, &si, &|| true).is_none());
        let d = DagCtx::new(&b, &si);
        let checks = std::cell::Cell::new(0usize);
        let expected = findings_cancellable(&f, Some(&d), &|| {
            checks.set(checks.get() + 1);
            false
        })
        .unwrap();
        let total = checks.get();
        assert!(total > 20);
        for stop_at in [1, total / 2, total] {
            checks.set(0);
            assert!(
                findings_cancellable(&f, Some(&d), &|| {
                    checks.set(checks.get() + 1);
                    checks.get() >= stop_at
                })
                .is_none()
            );
            assert_eq!(checks.get(), stop_at);
        }
        let actual = findings(&f, Some(&d));
        assert_eq!(actual.rows.len(), expected.rows.len());
        for (a, b) in actual.rows.iter().zip(&expected.rows) {
            assert_eq!(
                (a.class, a.cost_us, a.opportunity_us),
                (b.class, b.cost_us, b.opportunity_us)
            );
        }
    }

    #[test]
    fn findings_cancel_within_a_high_fan_in_replay_and_restore_weights() {
        let (tr, g) = fixture();
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).unwrap();
        let (mut b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        assert!(b.dag.weight_us.len() >= 2);
        b.dag.preds = vec![0; 32_768];
        b.dag.pred_off.fill(0);
        b.dag.pred_off[2..].fill(32_768);
        let d = DagCtx::new(&b, &si);
        let checks = std::cell::Cell::new(0usize);
        assert!(
            d.replay_set(1, 0, &|| {
                checks.set(checks.get() + 1);
                checks.get() == 3
            })
            .is_none()
        );
        assert_eq!(checks.get(), 3);
        assert_eq!(d.replay.borrow().as_ref().unwrap().weights, si.base);
        assert!(d.replay_set(1, 0, &|| false).is_some());
        assert_eq!(d.replay.borrow().as_ref().unwrap().weights, si.base);
    }

    #[test]
    fn cp_non_execution_details_name_finalization_without_recounting_hashing() {
        let trace = format!(
            "[\n{},\n",
            serde_json::json!({
                "ph":"X", "cat":"recipe", "name":"finalize", "ts":0, "dur":10_000_000,
                "args":{"stages":{
                    "execute_us":2_000_000, "cas_artifact_us":1_000_000,
                    "local_finalize_outputs_us":5_000_000,
                    "hashing_us":i64::MAX, "hashing_source":"local_outputs",
                }},
            })
        );
        let graph = "{\"graph\":{\"enc\":2}}\n{\"t\":\"finalize\",\"d\":[]}\n";
        let (tr, g) = write_pair(&trace, graph, "finalization");
        let x = load_extras(&tr);
        assert!(x.source_error.is_none());
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        let d = DagCtx::new(&b, &si);
        let report = findings(&f, Some(&d));
        let row = report
            .rows
            .iter()
            .find(|r| r.class == "cp_wait_share")
            .unwrap();
        assert_eq!(row.opportunity_us, Some(6_000_000));
        assert!(
            row.detail.contains("CAS artifact preparation"),
            "{}",
            row.detail
        );
        assert!(
            row.detail.contains("local output finalization"),
            "{}",
            row.detail
        );
        assert!(!row.detail.contains("hashing"), "{}", row.detail);
        std::fs::remove_file(tr.path).unwrap();
    }

    #[test]
    fn residual_is_not_wait_and_cache_hit_replay_removes_only_lookup() {
        for (name, args, expected_lookup, expected_step) in [
            (
                "residual",
                serde_json::json!({"stages":{"execute_us":1_000_000,"daemon_us":9_000_000}}),
                None,
                None,
            ),
            (
                "hit",
                serde_json::json!({"cache":{"lookup_us":6_000_000},"stages":{"cache_query_us":6_000_000,"materialize_us":4_000_000}}),
                Some(6_000_000),
                Some(6_000_000),
            ),
        ] {
            let trace = format!(
                "[\n{},\n",
                serde_json::json!({
                    "ph":"X", "cat":"recipe", "name":name, "ts":0, "dur":10_000_000, "args":args,
                })
            );
            let graph = format!(
                "{{\"graph\":{{\"enc\":2}}}}\n{}\n",
                serde_json::json!({"t":name,"d":[]})
            );
            let (tr, g) = write_pair(&trace, &graph, name);
            let x = load_extras(&tr);
            let f = Facts::for_run(&tr, &x, None).unwrap();
            let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
            let si = sim::sim_input(&b, &x);
            let d = DagCtx::new(&b, &si);
            let report = findings(&f, Some(&d));
            let row = |class| report.rows.iter().find(|r| r.class == class);
            assert_eq!(
                row("cache_lookup_tax").and_then(|r| r.opportunity_us),
                expected_lookup
            );
            assert_eq!(
                row("cp_wait_share").and_then(|r| r.opportunity_us),
                expected_step
            );
            if name == "residual" {
                let (weights, _) = sim::w_execute_only(&si);
                assert_eq!(sim::replay(&b.dag, &weights).cp_us, b.dag.cp);
            } else {
                assert!(row("cp_wait_share").unwrap().title.contains("cache hit"));
            }
            assert!(!report.rows.iter().any(|r| r.title.contains("% wait")));
            std::fs::remove_file(tr.path).unwrap();
        }
    }

    #[test]
    fn cache_miss_model_ignores_untimed_hits_failed_actions_and_simple_work() {
        let mut trace = String::from("[\n");
        for (i, args, duration) in [
            (0, serde_json::json!({"cache":{}}), 10),
            (1, serde_json::json!({"cache":{}}), 10),
            (
                2,
                serde_json::json!({"cache":{"lookup_us":100_000}}),
                100_000,
            ),
            (3, serde_json::json!({"kind":"remote"}), 12_000_000),
            (4, serde_json::json!({"kind":"remote","exit":1}), 15_000_000),
            (5, serde_json::json!({"kind":"simple"}), 20_000_000),
        ] {
            trace.push_str(&format!(
                "{},\n",
                serde_json::json!({
                    "ph":"X", "cat":"recipe", "name":format!("action{i}"),
                    "ts":0, "dur":duration, "args":args,
                })
            ));
        }
        let (tr, _) = write_pair(&trace, "{\"graph\":{\"enc\":2}}\n", "hit-cost");
        let x = load_extras(&tr);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let report = findings(&f, None);
        let misses: Vec<_> = report
            .rows
            .iter()
            .filter(|r| r.class == "cache_miss_hotspot")
            .collect();
        assert_eq!(misses.len(), 1);
        assert!(misses[0].title.contains("action3"));
        assert_eq!(misses[0].opportunity_us, Some(11_900_000));
        assert!(
            misses[0]
                .detail
                .contains("lookup-only model; restore excluded")
        );
        std::fs::remove_file(tr.path).unwrap();
    }

    #[test]
    fn idle_gap_reports_no_actions_instead_of_inventing_a_short_chain() {
        let trace = concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"first-a","ts":0,"dur":1000000},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"first-b","ts":0,"dur":1000000},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"last-a","ts":11000000,"dur":1000000},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"last-b","ts":11000000,"dur":1000000},"#,
            "\n",
        );
        let (tr, _) = write_pair(trace, "{\"graph\":{\"enc\":2}}\n", "idle");
        let x = load_extras(&tr);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let report = findings(&f, None);
        let row = report
            .rows
            .iter()
            .find(|r| r.class == "serialization_window")
            .unwrap();
        assert_eq!(row.cost_us, 10_000_000);
        assert!(row.detail.contains("0 actions overlap"), "{}", row.detail);
        assert!(
            row.detail.contains("no recorded action running for 10.00s"),
            "{}",
            row.detail
        );
        assert!(!row.detail.contains("chain of short"));
        std::fs::remove_file(tr.path).unwrap();
    }

    #[test]
    fn permit_title_preserves_subhour_duration_and_no_graph_notes_do_not_claim_replay() {
        let trace = concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"upload","ts":0,"dur":60000000,"args":{"stages":{"input_upload_us":59000000},"re":{"iu_sem_wait_us":55000000,"iu_digests_queried":1,"iu_cas_call_us":2000000}}},"#,
            "\n",
        );
        let (tr, _) = write_pair(trace, "{\"graph\":{\"enc\":2}}\n", "permit-title");
        let x = load_extras(&tr);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let report = findings(&f, None);
        let row = report
            .rows
            .iter()
            .find(|r| r.class == "permit_wait_tax")
            .unwrap();
        assert!(!row.title.contains("0h"));
        assert!(row.title.contains("55"));
        assert!(row.detail.contains("no replay"));
        assert!(
            row.detail.contains("the CAS call averaged 2.00s"),
            "{}",
            row.detail
        );
        std::fs::remove_file(tr.path).unwrap();
    }

    fn write_pair(
        trace: &str,
        graph: &str,
        tag: &str,
    ) -> (crate::types::Trace, crate::graph::Graph) {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-find-{}-{seq}-{tag}-trace.json",
            std::process::id()
        ));
        std::fs::write(&p, trace).unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!(
            "retrace-find-{}-{seq}-{tag}-graph.json",
            std::process::id()
        ));
        std::fs::write(&gp, graph).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        std::fs::remove_file(&gp).ok();
        (tr, g)
    }

    /// b waits 8s in queue on a 10s critical-path step; e retried.
    fn fixture() -> (crate::types::Trace, crate::graph::Graph) {
        write_pair(
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a (x)","ts":0,"dur":1000000,"args":{"src":"x","run":"r","stages":{"execute_us":1000000}}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"b (x)","ts":1000000,"dur":10000000,"args":{"src":"x","run":"r","stages":{"queue_us":8000000,"execute_us":2000000}}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"e (x)","ts":1000000,"dur":4000000,"args":{"src":"x","run":"r","tries":3,"stages":{"execute_us":4000000}}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"d (x)","ts":11000000,"dur":500000,"args":{"src":"x","run":"r","stages":{"execute_us":500000}}},"#,
                "\n"
            ),
            concat!(
                "{\"graph\":{\"enc\":2}}\n",
                "{\"t\":\"a (x)\",\"d\":[]}\n",
                "{\"t\":\"b (x)\",\"d\":[\"a (x)\"]}\n",
                "{\"t\":\"e (x)\",\"d\":[\"a (x)\"]}\n",
                "{\"t\":\"d (x)\",\"d\":[\"b (x)\",\"e (x)\"]}\n",
            ),
            "cp",
        )
    }

    #[test]
    fn queue_and_wait_share_are_replayed_and_ranked_first() {
        let (tr, g) = fixture();
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        let d = DagCtx::new(&b, &si);
        let rep = findings(&f, Some(&d));
        assert_eq!(rep.base_cp_us, Some(11_500_000));
        assert_eq!(rep.with_stages, 4);

        let key = rank_key;
        assert!(
            rep.rows.windows(2).all(|w| key(&w[0]) <= key(&w[1])),
            "rows must be ranked"
        );
        // the tail where only b (5..11s) then d (11..11.5s) ran is a
        // genuine 6.5s serialization window, reported as a bound
        let w = rep
            .rows
            .iter()
            .find(|r| r.class == "serialization_window")
            .expect("the 5..11.5s tail");
        assert_eq!(w.cost_us, 6_500_000);
        assert_eq!(w.opportunity_us, None);

        // removing b's 8s queue re-gates on e's branch (1+4+0.5 = 5.5s):
        // saving 6s — the strongest REPLAYED number
        let q = rep
            .rows
            .iter()
            .find(|r| r.class == "re_queue_tax")
            .expect("queue tax");
        assert_eq!(q.opportunity_us, Some(6_000_000));
        assert_eq!(q.estimate, "cp-replay");
        let wait = rep
            .rows
            .iter()
            .find(|r| r.class == "cp_wait_share")
            .expect("b is a waity CP step");
        assert!(wait.title.contains("b (x)"));
        assert_eq!(wait.cost_us, 8_000_000);

        // e carries `tries: 3` and no attempt records: the capture says
        // a retry happened and nothing about what it cost, so the
        // finding reports it without inventing a time or a saving
        let retry = rep
            .rows
            .iter()
            .find(|r| r.class == "retry_tax")
            .expect("e retried");
        assert_eq!(retry.cost_us, 0);
        assert_eq!(retry.opportunity_us, None);
        assert_eq!(retry.estimate, "none");
        assert!(
            retry.detail.contains("only a retry count"),
            "{}",
            retry.detail
        );
    }

    #[test]
    fn cp_wait_share_pruning_matches_all_candidate_replays_including_boundary_ties() {
        // s11 has the largest removable counter but a parallel tail limits
        // its saving to 0.5s. It must not displace any of the ten better rows.
        for tied_boundary in [false, true] {
            let mut trace = String::from("[\n");
            let mut graph = String::from("{\"graph\":{\"enc\":2,\"run\":\"r\"}}\n");
            for k in 0..12 {
                let name = format!("s{k:02}");
                let queue = 5_100_000 + k * 100_000;
                trace.push_str(&format!(
                    "{},\n",
                    serde_json::json!({
                        "ph":"X", "cat":"recipe", "name":name, "ts":k * 10_000_000,
                        "dur":10_000_000, "args":{"run":"r", "stages":{
                            "queue_us":queue, "execute_us":10_000_000 - queue,
                        }},
                    })
                ));
                let deps = if k == 0 {
                    vec![]
                } else {
                    vec![format!("s{:02}", k - 1)]
                };
                graph.push_str(&format!("{}\n", serde_json::json!({"t":name,"d":deps})));
            }
            let mut branches = vec![("tail-side", "s10", 110_000_000, 9_500_000)];
            if tied_boundary {
                // This bypass caps s01..s10 at 5.1s savings. The last
                // candidate s00 ties that bound and wins on event order;
                // pruning on <= instead of < would incorrectly exclude it.
                branches.push(("early-side", "s00", 10_000_000, 104_900_000));
            }
            for (name, dep, ts, dur) in branches {
                trace.push_str(&format!(
                    "{},\n",
                    serde_json::json!({
                        "ph":"X", "cat":"recipe", "name":name, "ts":ts, "dur":dur,
                        "args":{"run":"r", "stages":{"execute_us":dur}},
                    })
                ));
                graph.push_str(&format!("{}\n", serde_json::json!({"t":name,"d":[dep]})));
            }
            trace.push_str("]\n");
            let (tr, g) = write_pair(&trace, &graph, "cp-pruning");
            let x = load_extras(&tr);
            assert!(x.source_error.is_none());
            let f = Facts::for_run(&tr, &x, None).expect("should select fixture run");
            let (b, _) = crate::dag::build_full(&tr, &g, None).expect("should stitch fixture");
            assert_eq!(b.dag.cp, 120_000_000);
            assert_eq!(b.dag.cp_path.len(), 12);
            let si = sim::sim_input(&b, &x);
            let d = DagCtx::new(&b, &si);
            let report = findings(&f, Some(&d));
            let actual: Vec<_> = report
                .rows
                .iter()
                .filter(|row| row.class == "cp_wait_share")
                .collect();
            assert_eq!(actual.len(), 10);
            let names: Vec<_> = actual
                .iter()
                .map(|row| f.name(row.ev.expect("should identify a CP action")))
                .collect();
            let expected_names: Vec<_> = if tied_boundary {
                (0..10).map(|k| format!("s{k:02}")).collect()
            } else {
                (1..=10).rev().map(|k| format!("s{k:02}")).collect()
            };
            assert_eq!(names, expected_names, "tied boundary: {tied_boundary}");

            // The reference prices all twelve steps without the top-10
            // shortcut. Compare both the chosen rows and their exact savings.
            let mut exhaustive = Vec::new();
            for &node in &b.dag.cp_path {
                let i = node as usize;
                let ev = b.node_ev[i].expect("should identify a CP event") as u32;
                let new_weight = sim::non_execution_weight(&si, i);
                let saving = d
                    .replay_set(i, new_weight, &|| false)
                    .expect("should complete exhaustive replay");
                exhaustive.push((ev, saving));
            }
            exhaustive.sort_by_key(|&(ev, saving)| (std::cmp::Reverse(saving), ev));
            exhaustive.truncate(10);
            let reported: Vec<_> = actual
                .iter()
                .map(|row| {
                    (
                        row.ev.expect("should have a finding event"),
                        row.opportunity_us.expect("should have a replayed saving"),
                    )
                })
                .collect();
            assert_eq!(reported, exhaustive, "tied boundary: {tied_boundary}");
            std::fs::remove_file(tr.path).expect("should remove pruning trace fixture");
        }
    }

    /// Findings use the same comparable command intervals as the census,
    /// and label the resulting opportunity as conditional replay.
    #[test]
    fn retry_finding_follows_the_attempt_census() {
        let graph = concat!(
            "{\"graph\":{\"enc\":2}}\n",
            "{\"t\":\"race (compile)\",\"d\":[]}\n"
        );
        // concurrent: the 9s loser started with the 10s winner
        let (tr, g) = write_pair(
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"race (compile)","ts":100000000,"dur":10000000,"args":{"run":"r","src":"compile","wall_time_us":10000000,"winner":{"client_start":100000000,"dur_us":10000000},"tries":2,"attempt_status":["Cancelled"],"attempts":[{"status":"Cancelled","dur_us":9000000,"start":100000000,"client_start":100000000,"elastic":false}],"stages":{"execute_us":10000000}}},"#,
                "\n"
            ),
            graph,
            "race-concurrent",
        );
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        let d = DagCtx::new(&b, &si);
        let rep = findings(&f, Some(&d));
        let retry = rep
            .rows
            .iter()
            .find(|r| r.class == "retry_tax")
            .expect("the race retried");
        assert_eq!(
            retry.cost_us, 9_000_000,
            "the loser's recorded duration is command time, not worker CPU"
        );
        assert_eq!(
            retry.opportunity_us,
            Some(0),
            "this scenario removes no classified pre-winner duration"
        );
        assert_eq!(retry.estimate, "cp-replay");
        assert!(
            retry
                .detail
                .contains("neither zero contention nor zero build impact")
        );
        assert!(
            retry.detail.contains("1 discarded attempts overlapped"),
            "{}",
            retry.detail
        );

        // rerun: same loser, and both attempts carry a client start, so
        // the loser is known to have ended before the winner began
        let (tr, g) = write_pair(
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"race (compile)","ts":100000000,"dur":20000000,"args":{"run":"r","src":"compile","wall_time_us":10000000,"winner":{"client_start":110000000,"dur_us":10000000},"tries":2,"attempt_status":["Cancelled"],"attempts":[{"status":"Cancelled","dur_us":9000000,"start":100000000,"client_start":100000000,"elastic":false}],"stages":{"execute_us":10000000}}},"#,
                "\n"
            ),
            graph,
            "race-rerun",
        );
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        let d = DagCtx::new(&b, &si);
        let rep = findings(&f, Some(&d));
        let retry = rep
            .rows
            .iter()
            .find(|r| r.class == "retry_tax")
            .expect("the race retried");
        assert_eq!(retry.cost_us, 9_000_000);
        assert_eq!(
            retry.opportunity_us,
            Some(9_000_000),
            "conditional replay removes the classified pre-winner duration budget"
        );
        assert_eq!(retry.estimate, "cp-replay");

        // the same 20 s shape without client starts: the loser and the
        // winner fit inside the action together, so nothing places
        // them and no delay may be claimed
        let (tr, g) = write_pair(
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"race (compile)","ts":100000000,"dur":20000000,"args":{"run":"r","src":"compile","wall_time_us":10000000,"tries":2,"attempt_status":["Cancelled"],"attempts":[{"status":"Cancelled","dur_us":9000000,"start":100000000,"elastic":false}],"stages":{"execute_us":10000000}}},"#,
                "\n"
            ),
            graph,
            "race-unknown",
        );
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim::sim_input(&b, &x);
        let d = DagCtx::new(&b, &si);
        let rep = findings(&f, Some(&d));
        let retry = rep
            .rows
            .iter()
            .find(|r| r.class == "retry_tax")
            .expect("the race retried");
        assert_eq!(retry.cost_us, 9_000_000, "the command time is still known");
        assert_eq!(retry.opportunity_us, None);
        assert_eq!(retry.estimate, "none");
        assert!(
            retry
                .detail
                .contains("1 discarded attempts carry no interval"),
            "{}",
            retry.detail
        );
    }

    #[test]
    fn without_a_dag_opportunities_degrade_to_bounds() {
        let (tr, _) = fixture();
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let rep = findings(&f, None);
        assert_eq!(rep.base_cp_us, None);
        assert!(rep.rows.iter().all(|r| r.estimate != "cp-replay"));
        let q = rep
            .rows
            .iter()
            .find(|r| r.class == "re_queue_tax")
            .expect("queue tax still reported");
        assert_eq!(q.cost_us, 8_000_000);
        assert_eq!(q.opportunity_us, None);
    }

    #[test]
    fn serialization_window_names_the_runner() {
        // one long action alone for 20s between two parallel phases
        let trace = concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"p1 (x)","ts":0,"dur":1000000,"args":{"src":"x","run":"r"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"p2 (x)","ts":0,"dur":1000000,"args":{"src":"x","run":"r"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"lonely (x)","ts":1000000,"dur":20000000,"args":{"src":"x","run":"r"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"q1 (x)","ts":21000000,"dur":1000000,"args":{"src":"x","run":"r"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"q2 (x)","ts":21000000,"dur":1000000,"args":{"src":"x","run":"r"}},"#,
            "\n"
        );
        let (tr, _) = write_pair(trace, "{\"graph\":{\"enc\":2}}\n", "ser");
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let rep = findings(&f, None);
        let w = rep
            .rows
            .iter()
            .find(|r| r.class == "serialization_window")
            .expect("the 20s solo run is a window");
        assert_eq!(w.cost_us, 20_000_000);
        assert!(w.detail.contains("lonely (x)"));
    }

    #[test]
    fn cache_hotspots_use_the_observed_hit_cost() {
        // one hit (models the hit cost), one 12s miss
        let trace = concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"hit (x)","ts":0,"dur":300000,"args":{"src":"x","run":"r","cache":{"lookup_us":100000,"restore_us":200000,"bytes":5}}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"miss (x)","ts":0,"dur":12000000,"args":{"src":"x","run":"r"}},"#,
            "\n"
        );
        let (tr, _) = write_pair(trace, "{\"graph\":{\"enc\":2}}\n", "hits");
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let rep = findings(&f, None);
        let h = rep
            .rows
            .iter()
            .find(|r| r.class == "cache_miss_hotspot")
            .expect("the 12s miss");
        assert!(h.title.contains("miss (x)"));
        // This explicitly lookup-only scenario excludes even a recorded
        // restoration cost: 12s minus the measured 100ms lookup.
        assert_eq!(h.opportunity_us, Some(11_900_000));
        assert!(h.detail.contains("lookup-only model; restore excluded"));
        assert_eq!(h.estimate, "bound");
    }
}
