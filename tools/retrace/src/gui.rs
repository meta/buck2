/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The window: app shell, menu bar, health chips, popup plumbing, the
//! analysis panels, the timeline canvas and the action-detail popup.
//! Everything a panel shows comes from the SAME `analyses`/`dag`
//! functions `--json` exports — the surfaces cannot disagree.

pub mod canvas;
pub mod cascade;
pub mod command_ui;
pub mod detail_ui;
pub mod execution_ui;
mod load_pipeline;
pub mod open;
pub mod panels;
pub mod reported_path;
pub mod stagebar;
pub mod widgets;

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Instant;

pub use load_pipeline::LoadedTrace;
use widgets::popup_raise;

use crate::analyses;
use crate::config;
use crate::dag;
use crate::detail;
use crate::graph;
use crate::humanize;
use crate::ingest;
use crate::parse;
use crate::quality;
use crate::sim;
use crate::types::CLASS_NAMES;
use crate::types::F_CACHE_HIT;
use crate::types::F_FAILED;
use crate::types::F_NOT_RECIPE;
use crate::types::Trace;

/// Dropping a request cancels its work without joining on the UI thread.
pub struct Worker<T> {
    rx: mpsc::Receiver<T>,
    cancel: Arc<AtomicBool>,
}

impl<T> Worker<T> {
    fn channel() -> (mpsc::Sender<T>, Self, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            tx,
            Self {
                rx,
                cancel: cancel.clone(),
            },
            cancel,
        )
    }

    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    #[cfg(test)]
    fn recv_timeout(&self, timeout: std::time::Duration) -> Result<T, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// egui's proportional face is Ubuntu-Light, which carries no arrows or
/// triangles: `▸` and the sort carets reach the screen as the
/// replacement square. Hack is already linked in as the monospace face
/// and has them, so appending it to the proportional chain costs no
/// bytes and changes no glyph Ubuntu-Light already covers. Every
/// `egui::Context` the crate draws into needs this — the snapshot
/// harness builds its own.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        proportional.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Load one action's full record back from the trace file.
pub fn load_full(tr: &Trace, off: u64, len: u32) -> Option<serde_json::Value> {
    use std::io::Read;
    use std::io::Seek;
    use std::io::SeekFrom;
    if len as usize > detail::MAX_DETAIL_RECORD_BYTES || off.checked_add(u64::from(len)).is_none() {
        return None;
    }
    let mut f = tr.source_reader().ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf).ok()?;
    tr.validate_source().ok()?;
    let s = String::from_utf8_lossy(&buf);
    serde_json::from_str(s.trim_end().trim_end_matches(',')).ok()
}

fn digest_query(query: &str) -> bool {
    let (hash, size) = query
        .split_once(':')
        .map_or((query, None), |(h, s)| (h, Some(s)));
    hash.len() >= 8
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
        && size.is_none_or(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

fn contains_folded(name: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if name.is_ascii() && query.is_ascii() {
        name.as_bytes()
            .windows(query.len())
            .any(|part| part.eq_ignore_ascii_case(query.as_bytes()))
    } else {
        name.to_lowercase().contains(query)
    }
}

/// The rows the Previous and Next step buttons select, given the visible rows in display order
/// and the selected row. Steps move only through visible rows, so a step never selects a row the
/// view hides; with no visible row selected, Next selects the first one.
fn step_targets(visible: &[usize], selected: Option<usize>) -> (Option<usize>, Option<usize>) {
    let position = selected.and_then(|s| visible.iter().position(|&i| i == s));
    let previous = position.and_then(|p| p.checked_sub(1)).map(|p| visible[p]);
    let next = visible.get(position.map_or(0, |p| p + 1)).copied();
    (previous, next)
}

/// Previous and Next step buttons over the visible rows; returns the row to select, if one was
/// clicked.
fn step_buttons(ui: &mut egui::Ui, visible: &[usize], selected: Option<usize>) -> Option<usize> {
    let (previous, next) = step_targets(visible, selected);
    let mut chosen = None;
    if ui
        .add_enabled(previous.is_some(), egui::Button::new("Previous step"))
        .clicked()
    {
        chosen = previous;
    }
    if ui
        .add_enabled(next.is_some(), egui::Button::new("Next step"))
        .clicked()
    {
        chosen = next;
    }
    chosen
}

fn load_digest_index(tr: &Trace, cancel: &AtomicBool) -> anyhow::Result<Vec<Option<String>>> {
    use std::io::Read;
    use std::io::Seek;
    use std::io::SeekFrom;
    #[derive(serde::Deserialize)]
    struct Args {
        digest: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Row {
        args: Option<Args>,
    }
    let mut index = vec![None; tr.acts.len()];
    let mut reader = tr.source_reader()?;
    let mut order: Vec<_> = tr
        .acts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.flags & F_NOT_RECIPE == 0)
        .map(|(i, a)| (a.off, i))
        .collect();
    order.sort_unstable();
    let mut bytes = Vec::new();
    for (off, i) in order {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "digest indexing canceled");
        bytes.resize(tr.acts[i].len as usize, 0);
        reader.seek(SeekFrom::Start(off))?;
        reader.read_exact(&mut bytes)?;
        let raw = std::str::from_utf8(&bytes)?
            .trim_end()
            .trim_end_matches(',');
        if let Ok(row) = serde_json::from_str::<Row>(raw) {
            index[i] = row
                .args
                .and_then(|args| args.digest)
                .map(|d| d.to_ascii_lowercase());
        }
    }
    tr.validate_source()?;
    Ok(index)
}

/// Upload rows named for display, each paired with the act index it
/// opens; the index is `None` where the action is not in the trace.
pub type UploadWaits = Vec<(String, Option<usize>)>;

/// Compact admission weights retained after temporary decoded extras are released.
pub struct AdmissionWeights {
    pub weights: Vec<i64>,
    pub n_recorded: usize,
    pub n_actions: usize,
    pub n_modified: usize,
}

/// Advanced reports are independent requests. Completed empty reports also
/// count, so missing graph data does not cause repeated work or a spinner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnalysisReports {
    whatif: bool,
    findings: bool,
    blast: bool,
}

impl AnalysisReports {
    const ALL: Self = Self {
        whatif: true,
        findings: true,
        blast: true,
    };

    fn for_panels(panels: &Panels) -> Self {
        Self {
            whatif: panels.whatif,
            findings: panels.findings,
            blast: panels.blast,
        }
    }

    fn include(&mut self, other: Self) {
        self.whatif |= other.whatif;
        self.findings |= other.findings;
        self.blast |= other.blast;
    }

    fn pending(self, completed: Self) -> Self {
        Self {
            whatif: self.whatif && !completed.whatif,
            findings: self.findings && !completed.findings,
            blast: self.blast && !completed.blast,
        }
    }
}

/// Compact report results, computed on a worker. Expensive replay reports
/// are requested separately from the base overview; transient Facts/Extras
/// are released after each pass rather than retained for the whole session.
pub struct Analysis {
    pub reports: AnalysisReports,
    pub admission: Option<Arc<AdmissionWeights>>,
    pub ov_rows: Vec<analyses::OverviewRow>,
    pub breakdown: Vec<analyses::BreakdownRow>,
    pub rule_sites: Vec<analyses::RuleSiteRow>,
    pub top_mem: Vec<(analyses::MemRow, Option<usize>)>,
    pub queue: Option<(analyses::QueueOverview, Option<usize>, String)>,
    pub peers: Vec<analyses::PeerRow>,
    pub stall: analyses::StallReport,
    /// cohort samples resolved to act indices, parallel to
    /// `stall.cohorts`
    pub stall_samples: Vec<Option<usize>>,
    pub extras_unmatched: usize,
    /// sorted per-category duration distributions — the same data the
    /// peers table shows, kept for the detail popup's rank line
    pub peer_durs: fxhash::FxHashMap<String, Vec<i64>>,
    /// build-median effective input-fetch rate over >10 MiB transfers
    /// (bytes/s), for the popup's rate-in-context line
    pub fetch_med_bps: Option<f64>,
    /// CP-replay scenarios; `None` when no DAG was stitched yet
    pub whatif: Option<crate::sim::WhatIfReport>,
    /// retry census + each row's act index for navigation
    pub retries: Option<(crate::analyses::RetryCensus, Vec<Option<usize>>)>,
    /// attempts that lost a local-vs-remote race + each row's act index
    pub cancelled: Option<(crate::analyses::CancelledCensus, Vec<Option<usize>>)>,
    /// ranked findings + rows' act indices for navigation (opportunities
    /// degrade to bounds when the DAG was not stitched yet)
    pub findings: Option<(crate::findings::FindingsReport, Vec<Option<usize>>)>,
    /// top blast-radius candidates by recorded descendant path cost; `None`
    /// when no DAG was stitched yet
    pub blast: Option<Vec<crate::blast::Blast>>,
    /// input-upload sub-stage breakdown and its click-through targets: the
    /// worst permit waits, then the uploads that waited beside a local attempt
    pub upload: Option<(crate::upload::UploadReport, UploadWaits, UploadWaits)>,
}

impl Analysis {
    /// A new pass computes only missing reports. Move the compact earlier
    /// results forward without retaining their Facts/Extras or replaying them.
    fn retain_completed_reports(&mut self, previous: &mut Self) {
        if !self.reports.whatif {
            self.whatif = previous.whatif.take();
        }
        if !self.reports.findings {
            self.findings = previous.findings.take();
        }
        if !self.reports.blast {
            self.blast = previous.blast.take();
        }
        self.reports.include(previous.reports);
    }
}

/// The panel computation, callable synchronously (tests) or from the
/// worker thread (`ensure_analysis`).
pub fn compute_analysis(
    tr: &Trace,
    g: Option<&graph::Graph>,
    bundle: Option<&dag::DagBundle>,
    run_id: Option<&str>,
) -> anyhow::Result<Analysis> {
    let summary = g.map(|g| GraphSummary::new(g, g_and_chain(tr, g, run_id)));
    compute_analysis_cancellable(
        tr,
        summary.as_ref(),
        bundle,
        run_id,
        AnalysisReports::ALL,
        &|| false,
    )
}

// Match the displayed path's tie-breaking in the window and headless reports.
fn g_and_chain(tr: &Trace, g: &graph::Graph, run: Option<&str>) -> Option<dag::CpChain> {
    dag::build_full(tr, g, run).map(|(b, st)| dag::chain_from_bundle(&b, &st))
}

fn compute_analysis_cancellable(
    tr: &Trace,
    g: Option<&GraphSummary>,
    bundle: Option<&dag::DagBundle>,
    run_id: Option<&str>,
    reports: AnalysisReports,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<Analysis> {
    anyhow::ensure!(!cancel(), "analysis canceled");
    let x = analyses::load_extras_cancellable(tr, cancel)?;
    if let Some(e) = &x.source_error {
        anyhow::bail!("{e}");
    }
    if let Some(g) = g {
        graph::check_trace_pair(tr, run_id, &g.meta)?;
    }
    let f = analyses::Facts::for_run(tr, &x, run_id)?;
    let resolve = |ev: u32| -> Option<usize> { f.ev.get(ev as usize).map(|&a| a as usize) };
    let o = analyses::overview_facts(&f);
    let chain = g.and_then(|g| g.chain.as_ref());
    let ov_rows = analyses::overview_rows(&f, &o, chain, &fmt_when_local);
    let top_mem = analyses::top_memory(&f, analyses::TOP_MEMORY_N)
        .into_iter()
        .map(|r| {
            let a = resolve(r.ev);
            (r, a)
        })
        .collect();
    let queue = analyses::queue_overview(&f).map(|q| {
        let a = resolve(q.max_ev);
        let name = f.name(q.max_ev).to_owned();
        (q, a, name)
    });
    let stall = analyses::stall_audit(&f);
    let stall_samples = stall.cohorts.iter().map(|c| resolve(c.sample)).collect();
    let ps = analyses::peer_stats(&f);
    let peers = ps.table(&f);
    let peer_durs = ps.into_named(&f);
    anyhow::ensure!(!cancel(), "analysis canceled");
    let sim_input = bundle.map(|b| crate::sim::sim_input(b, &x));
    let admission = sim_input.as_ref().map(|si| {
        let (weights, n_modified) = sim::w_free_admission(si);
        let (n_recorded, n_actions) = sim::admission_coverage(si);
        Arc::new(AdmissionWeights {
            weights,
            n_recorded,
            n_actions,
            n_modified,
        })
    });
    let whatif = if reports.whatif {
        bundle
            .zip(sim_input.as_ref())
            .map(|(b, si)| {
                crate::sim::standard_whatifs_cancellable(b, si, &|| cancel())
                    .ok_or_else(|| anyhow::anyhow!("analysis canceled"))
            })
            .transpose()?
    } else {
        None
    };
    anyhow::ensure!(!cancel(), "analysis canceled");
    let retries = analyses::retry_census(&f, usize::MAX).map(|c| {
        let acts = c.rows.iter().map(|r| resolve(r.ev)).collect();
        (c, acts)
    });
    let cancelled = analyses::cancelled_census(&f).map(|c| {
        let acts = c.rows.iter().map(|r| resolve(r.ev)).collect();
        (c, acts)
    });
    let findings = if reports.findings {
        let d = bundle
            .zip(sim_input.as_ref())
            .map(|(b, si)| {
                crate::findings::DagCtx::new_cancellable(b, si, &|| cancel())
                    .ok_or_else(|| anyhow::anyhow!("analysis canceled"))
            })
            .transpose()?;
        let rep = crate::findings::findings_cancellable(&f, d.as_ref(), &|| cancel())
            .ok_or_else(|| anyhow::anyhow!("analysis canceled"))?;
        let acts: Vec<Option<usize>> = rep.rows.iter().map(|r| r.ev.and_then(resolve)).collect();
        Some((rep, acts))
    } else {
        None
    };
    anyhow::ensure!(!cancel(), "analysis canceled");
    let blast = if reports.blast {
        bundle
            .map(|b| {
                let bi = crate::blast::BlastIndex::new_cancellable(b, tr, &x, &|| cancel())
                    .ok_or_else(|| anyhow::anyhow!("analysis canceled"))?;
                bi.top_candidates_cancellable(b, &x, crate::blast::MAX_CANDIDATES, &|| cancel())
                    .ok_or_else(|| anyhow::anyhow!("analysis canceled"))
            })
            .transpose()?
    } else {
        None
    };
    anyhow::ensure!(!cancel(), "analysis canceled");
    let upload = crate::upload::upload_report(&f).map(|r| {
        let named = |rows: &[(u32, i64)]| -> UploadWaits {
            rows.iter()
                .map(|&(e, _)| (f.name(e).to_owned(), resolve(e)))
                .collect()
        };
        let tops = named(&r.top_wait);
        let race = named(&r.local_race);
        (r, tops, race)
    });
    Ok(Analysis {
        reports,
        admission,
        retries,
        cancelled,
        ov_rows,
        breakdown: analyses::breakdown(&f),
        rule_sites: analyses::rule_sites(&f, analyses::RULE_SITES_N),
        top_mem,
        queue,
        peers,
        stall,
        stall_samples,
        extras_unmatched: x.unmatched,
        peer_durs,
        fetch_med_bps: analyses::fetch_rate_median(&f),
        whatif,
        findings,
        blast,
        upload,
    })
}

/// The overview's `Date` row renders the run's wall clock in the local
/// zone with its abbreviation ("Wed 2026-08-27 17:30:00 PDT"); UTC
/// fallback when the timestamp or the system zone is unusable.
pub fn fmt_when_local(us: i64) -> String {
    let Ok(ts) = jiff::Timestamp::from_microsecond(us) else {
        return analyses::fmt_when_utc(us);
    };
    let z = ts.to_zoned(jiff::tz::TimeZone::system());
    jiff::fmt::strtime::format("%a %Y-%m-%d %H:%M:%S %Z", &z)
        .unwrap_or_else(|_| analyses::fmt_when_utc(us))
}

/// Once stitching is complete the viewer needs counts and provenance, not
/// another copy of every target name and dependency edge.
pub struct GraphSummary {
    pub meta: graph::GraphMeta,
    pub n_rows: usize,
    pub n_edges: usize,
    pub n_joins: usize,
    pub chain: Option<dag::CpChain>,
}

impl GraphSummary {
    pub fn new(g: &graph::Graph, chain: Option<dag::CpChain>) -> Self {
        Self {
            meta: g.meta.clone(),
            n_rows: g.n_rows,
            n_edges: g.n_edges,
            n_joins: g.is_join.iter().filter(|&&v| v).count(),
            chain,
        }
    }
}

pub enum GState {
    Absent,
    Loading(Worker<anyhow::Result<graph::Graph>>),
    Building(
        Worker<(
            Arc<graph::Graph>,
            Option<(Arc<dag::DagBundle>, dag::DagStats)>,
        )>,
    ),
    Ready {
        g: Arc<GraphSummary>,
        dag: Option<Arc<dag::DagBundle>>,
        /// the graph's and the stitch's shortfalls, the same decision
        /// the JSON envelopes make
        quality: Box<quality::Quality>,
    },
    Failed(String),
}

/// The health chips the quality decision owns: an incomplete capture,
/// an unrecorded outcome, and a graph that is not whole. Empty when
/// there is nothing to say.
pub(crate) fn quality_chips(q: &quality::Quality) -> Vec<(String, egui::Color32)> {
    let amber = egui::Color32::from_rgb(0xf5, 0xe1, 0x9c);
    let mut chips = Vec::new();
    if !q.capture_complete() {
        chips.push(("capture: incomplete".to_owned(), amber));
    }
    if q.outcome_unrecorded {
        chips.push(("outcome: unknown".to_owned(), amber));
    }
    if let Some(g) = &q.graph
        && !q.graph_complete()
    {
        let detail = match &q.stitch {
            Some(s) if s.n_defined < s.n_events => format!(
                "{}/{} actions with rows",
                humanize::count(s.n_defined as u64),
                humanize::count(s.n_events as u64)
            ),
            _ => match q.graph_limitations().first() {
                Some(reason) => humanize::mid_ellipsis(reason, 88),
                None => match g.declared_nodes {
                    Some(n) => format!(
                        "{}/{} rows",
                        humanize::count(g.loaded_rows as u64),
                        humanize::count(n.max(0) as u64)
                    ),
                    None => format!("{} rows", humanize::count(g.loaded_rows as u64)),
                },
            },
        };
        chips.push((format!("graph: PARTIAL {detail}"), amber));
    }
    chips
}

#[derive(Default)]
pub struct Panels {
    pub command: bool,
    pub overview: bool,
    pub longest: bool,
    pub failed: bool,
    pub retried: bool,
    pub cancelled: bool,
    pub breakdown: bool,
    pub top_mem: bool,
    pub rule_sites: bool,
    pub whatif: bool,
    pub findings: bool,
    pub blast: bool,
    pub upload: bool,
    pub peers: bool,
    pub stall: bool,
    pub cp: bool,
    /// Buck's reported path, for captures without a dependency graph.
    pub reported_cp: bool,
    pub cascade: bool,
    pub health: bool,
    pub help: bool,
    pub glossary: bool,
}

/// Opens one Analysis-menu panel. Every menu entry carries its own
/// opener, so an entry cannot be written for a panel that does not exist.
type PanelOpener = fn(&mut Ready);

/// One or more live overrides on the critical path. The walker keeps a
/// stack of them rather than a single slot: two steps on the same chain
/// each showing "saves 66s" do not save 132s together, so the only way
/// to answer "what do I get if I fix all three" is to replay them at
/// once.
#[derive(Clone, Debug, PartialEq)]
pub enum CpOverride {
    /// one step's duration replaced; `label` is the menu wording
    /// ("at 50%", "removed") the banner reads back
    Node {
        act: usize,
        w_us: i64,
        label: String,
    },
    /// every node's execution-admission wait removed. Rewrites the whole
    /// weight vector rather than one entry, so it is the base any `Node`
    /// override is then applied on top of: a `Node` override on the same
    /// node wins, and that row's marginal restores the admission-free
    /// weight, not the measured one.
    FreeAdmission,
}

const FREE_ADMISSION_LABEL: &str = "execution_concurrency_limit waits removed";

/// How many top steps the walker's combined-savings caption replays at
/// once.
pub const N_TOP_SAVINGS: usize = 5;

/// The pair behind [`Ready::cp_savings_combined`]: the sum of the
/// `N_TOP_SAVINGS` largest single-step savings, and one replay with all
/// of them free. `None` when there is nothing to compare — fewer than
/// two steps, or no step that saves anything.
fn combined_top_savings(b: &dag::DagBundle, savings: &HashMap<usize, i64>) -> Option<(i64, i64)> {
    let mut top: Vec<(usize, i64)> = savings.iter().map(|(&a, &s)| (a, s)).collect();
    // ties break on act index so the caption is stable across runs
    top.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    top.truncate(N_TOP_SAVINGS);
    if top.len() < 2 {
        return None;
    }
    let sum: i64 = top.iter().map(|&(_, s)| s).sum();
    if sum <= 0 {
        return None;
    }
    let edits: Vec<(u32, i64)> = b
        .dag
        .act_of_node
        .iter()
        .enumerate()
        .filter(|&(_, &a)| top.iter().any(|&(act, _)| act as u32 == a))
        .map(|(n, _)| (n as u32, 0))
        .collect();
    let s = sim::simulate_edits(b, &edits, "top steps free");
    Some((sum, s.base_cp_us - s.cp_us))
}

/// One result from the savings worker.
pub enum CpSavingsMsg {
    /// (act index, wall saved if that step alone were free)
    One(usize, i64),
    /// the pair behind [`Ready::cp_savings_combined`], sent last
    Combined(i64, i64),
    Complete,
}

/// The what-if the CP walker displays in place of the live critical
/// path — the replayed result of every live [`CpOverride`]. Sim-vs-sim:
/// `base_cp_us` is the measured critical path, the same baseline for
/// every override count, so stacking a second one cannot re-score the
/// first.
#[derive(Clone)]
pub struct CpSimView {
    /// the edited step's act index; `None` when the scenario does not
    /// name one action (several steps at once, or a whole-graph replay)
    pub act: Option<usize>,
    pub label: String,
    pub base_cp_us: i64,
    pub cp_us: i64,
    /// the simulated critical path as act indices
    pub path: Vec<usize>,
    /// each `path` step's simulated duration, parallel to `path`. Every
    /// surface that totals or scales the path must read these — a
    /// simulated path drawn with measured durations sums to neither
    /// `cp_us` nor the measured wall.
    pub w_us: Vec<i64>,
}

impl CpSimView {
    pub fn has_consistent_duration(&self) -> bool {
        self.path.len() == self.w_us.len()
            && self.w_us.iter().all(|&weight| weight >= 0)
            && self
                .w_us
                .iter()
                .try_fold(0i64, |total, &weight| total.checked_add(weight))
                == Some(self.cp_us)
    }
}

/// What the scenario worker streams back: the replay first, then one
/// marginal per override as each extra replay lands. They arrive
/// separately because the scenario is one replay and the marginals are
/// N more — making the walker wait for all of them would stall the
/// numbers it already has.
pub enum CpSimMsg {
    View(CpSimView),
    /// (index into `cp_overrides`, that override's marginal saving)
    Margin(usize, i64),
    Complete,
}

/// The critical-path walker's steps and the duration of each: the live
/// path with measured durations, or the simulated path with its replayed
/// weights. The two come back together because a simulated path drawn
/// with measured durations sums to neither `cp_us` nor the measured
/// wall, and every surface that totals or scales the path reads this.
pub fn cp_walk_steps(
    sim: Option<&CpSimView>,
    cp_list: &[usize],
    acts: &[crate::types::Act],
) -> (Vec<usize>, Vec<i64>) {
    match sim {
        Some(s) => (s.path.clone(), s.w_us.clone()),
        None => (
            cp_list.to_vec(),
            cp_list.iter().map(|&i| acts[i].dur).collect(),
        ),
    }
}

pub struct Ready {
    pub command_view: command_ui::State,
    pub reported_path: reported_path::State,
    pub tr: Arc<Trace>,
    pub run_id: Option<String>,
    /// the status-bar line, also quoted into the analysis prompt
    pub summary: String,
    pub selected: Option<usize>,
    /// navigation request the timeline consumes this frame
    pub nav_req: Option<usize>,
    /// previously selected actions, oldest first (Back / Backspace)
    pub hist: Vec<usize>,
    /// set by a Back navigation so `select` unwinds the history instead
    /// of extending it
    pub nav_back: bool,
    // ---- viewport ----
    pub t_left: f64,
    pub upp: f64,
    pub scroll: f32,
    pub row_h: f32,
    pub show_conc: bool,
    pub show_gutter: bool,
    pub cp_on: bool,
    pub painted: usize,
    // ---- filter ----
    pub filter: String,
    pub matches: Option<Vec<bool>>,
    pub match_list: Vec<usize>,
    pub match_cursor: usize,
    /// the Find overlay is open (Cmd/Ctrl+F, View menu)
    pub find_open: bool,
    /// focus the Find input on the frame it opens
    pub find_focus: bool,
    // ---- lazily built render data ----
    pub minimap: Option<egui::TextureHandle>,
    pub conc: Vec<u32>,
    pub conc_max: u32,
    /// act index -> its stage array (None = record has no stages)
    pub stage_cache: HashMap<usize, Option<[i64; crate::detail::STAGE_N]>>,
    /// Compact positioned coverage; stage counters never acquire timestamps.
    pub activity_cache: HashMap<usize, Vec<(i64, i64, bool)>>,
    pub activity_state: Vec<u8>,
    pub activity_cache_intervals: usize,
    /// per act, the recorded `stages.cache_query_us`: `None` when the
    /// record has no such key, which is distinct from a measured zero
    pub lookup_cache: HashMap<usize, Option<i64>>,
    /// ancestry highlight (right-click ▸ show/list)
    pub anc_set: Option<HashSet<usize>>,
    pub anc_list: Vec<usize>,
    pub anc_title: String,
    pub anc_window: bool,
    /// right-click context menu: (act index, where and when it opened)
    pub ctx_menu: Option<(usize, widgets::MenuAt)>,
    /// the open action-detail popup, built once per selection
    pub detail: Option<detail_ui::Detail>,
    /// recipe action count (prompt context)
    pub n_events: usize,
    selected_class_count: [usize; config::CLASS_N],
    /// critical-path walker slider position
    pub cp_step: usize,
    pub cp_selected: Option<usize>,
    /// pan/zoom of the critical-path strip, driven by the same gestures
    /// as the stage bar
    pub cp_view: stagebar::BarState,
    pub panels: Panels,
    pub longest: Vec<usize>,
    /// What each of `longest` moved, read from its record when the list is built
    pub longest_data: HashMap<usize, detail::DataMoved>,
    pub failed_list: Vec<usize>,
    pub cp_list: Vec<usize>,
    /// act -> lowercase RE digest, built lazily by `ensure_digest_ix`
    pub digest_ix: Option<Vec<Option<String>>>,
    digest_running: Option<Worker<anyhow::Result<Vec<Option<String>>>>>,
    digest_error: Option<String>,
    /// The timeline plot's painted x range, recorded each canvas frame.
    /// The minimap, its brush, and the horizontal scrollbar are all
    /// drawn *outside* the central panel, so the only way they can share
    /// the plot's time axis is to be told where it is: the plot starts
    /// one lane gutter in from the window edge and stops one scrollbar
    /// short of the other, and anything that maps time to x with the
    /// window's own rect is off by both.
    pub canvas_x0: f32,
    pub canvas_w: f32,
    /// The plot's painted height, recorded the same way and for the same
    /// reason: the overview's brush has to bracket the lanes on screen,
    /// and how many those are is `canvas_h / row_h`.
    pub canvas_h: f32,
    pub gstate: GState,
    graph_explicit: bool,
    pub graph_warning: Option<String>,
    pub native_popups: bool,
    /// The Open menu's own state. It only ever records what was asked
    /// for; the app is what replaces the trace.
    pub open: open::OpenState,
    pub open_warnings: Vec<String>,
    pub open_status_lines: Vec<String>,
    pub toast: Option<(String, Instant)>,
    pub an: Option<Box<Analysis>>,
    pub an_running: Option<Worker<anyhow::Result<Box<Analysis>>>>,
    analysis_requested: bool,
    analysis_reports_requested: AnalysisReports,
    /// the current `an`/`an_running` was computed without the graph
    /// (kicked off while it was still loading)
    pub an_lacks_graph: bool,
    /// a failed (or panicked) analysis worker, rendered wherever a
    /// panel would otherwise spin forever
    pub an_error: Option<String>,
    pub ov_sort: (u8, bool),
    pub sorts: HashMap<&'static str, (u8, bool)>,
    pub cp_set: Option<HashSet<usize>>,
    /// every live what-if, in the order they were clicked. Mutate only
    /// through `set_cp_override` / `clear_cp_override(s)`, which respawn
    /// the replay — an edit that skips the respawn leaves the walker
    /// drawing the previous scenario's numbers.
    pub cp_overrides: Vec<CpOverride>,
    /// the replay of `cp_overrides` (None = live critical path)
    pub cp_sim: Option<CpSimView>,
    pub cp_sim_rx: Option<Worker<CpSimMsg>>,
    pub cp_sim_error: Option<String>,
    /// each override's marginal saving, parallel to `cp_overrides`;
    /// `None` until its replay lands. Reset by `respawn_cp_sim`, so a
    /// row can never show the previous scenario's number — and never
    /// `0`, which means reverting that override alone does not raise the
    /// current simulated critical path (not that the action is off-path).
    pub cp_margins: Vec<Option<i64>>,
    /// act idx -> simulated wall saved if that CP step were free
    pub cp_savings: HashMap<usize, i64>,
    pub cp_savings_rx: Option<Worker<CpSavingsMsg>>,
    pub cp_savings_error: Option<String>,
    /// (Σ of the five largest single-step savings, one replay with all
    /// five free). Only meaningful as a pair: the gap between them is
    /// the whole point, and neither number answers the question alone.
    pub cp_savings_combined: Option<(i64, i64)>,
    /// CP-walker context menu: (act index, where and when it opened, in
    /// the walker's own viewport)
    pub cp_ctx: Option<(usize, widgets::MenuAt)>,
    /// Compact results and the cancelable worker for greedy CP removals.
    pub cascade: cascade::CascadeState,
}

impl Ready {
    pub fn new(tr: Trace, run_id: Option<String>) -> Self {
        Self::new_with_graph(tr, run_id, None)
    }

    fn new_with_graph(tr: Trace, run_id: Option<String>, graph_path: Option<String>) -> Self {
        Self::new_with_pending_graph(tr, run_id, graph_path, None, parse::DEFAULT_MAX_INPUT_BYTES)
    }

    fn from_loaded(
        loaded: LoadedTrace,
        run_sel: Option<&str>,
        graph_path: Option<String>,
    ) -> anyhow::Result<Self> {
        // Invalid run selection drops the request-owned preload before it
        // can be adopted. The existing pair gate still runs in poll_graph.
        let run_id = parse::select_trace_run(&loaded.tr, run_sel)?;
        Ok(Self::from_validated_load(loaded, run_id, graph_path))
    }

    fn from_validated_load(
        mut loaded: LoadedTrace,
        run_id: Option<String>,
        graph_path: Option<String>,
    ) -> Self {
        let notices = std::mem::take(&mut loaded.notices);
        let status_lines = std::mem::take(&mut loaded.status_lines);
        let (tr, pending, max_input_bytes) = loaded.into_parts();
        let mut ready =
            Self::new_with_pending_graph(tr, run_id, graph_path, pending, max_input_bytes);
        ready.open_warnings = notices;
        ready.open_status_lines = status_lines;
        ready
    }

    fn new_with_pending_graph(
        tr: Trace,
        run_id: Option<String>,
        graph_path: Option<String>,
        pending: Option<Worker<anyhow::Result<graph::Graph>>>,
        max_input_bytes: u64,
    ) -> Self {
        let graph_explicit = graph_path.is_some();
        let gstate = if let Some(rx) = pending {
            GState::Loading(rx)
        } else {
            match graph_path.or_else(|| graph::sibling_graph_path(&tr.path)) {
                Some(gp) => {
                    let (tx, rx, cancel) = Worker::channel();
                    let started = std::thread::Builder::new()
                        .name("retrace-graph".into())
                        .spawn(move || {
                            // the window can close while a worker is still running, so
                            // a closed channel is an ordinary outcome and not an error
                            let result = graph::load_graph_cancellable_with_workers_and_limit(
                                &gp,
                                &cancel,
                                crate::parallelism::graph_workers(),
                                max_input_bytes,
                            );
                            if let Ok(g) = &result {
                                graph::dropped_line_notice(
                                    &gp,
                                    g.dropped_lines,
                                    g.dropped_interior,
                                );
                            }
                            drop(tx.send(result));
                        });
                    match started {
                        Ok(_) => GState::Loading(rx),
                        Err(e) => GState::Failed(format!("cannot start graph worker: {e}")),
                    }
                }
                None => GState::Absent,
            }
        };
        let recipes = tr
            .acts
            .iter()
            .filter(|a| a.flags & F_NOT_RECIPE == 0 && tr.action_in_run(a, run_id.as_deref()));
        let busy: i64 = recipes
            .clone()
            .fold(0i64, |sum, a| sum.saturating_add(a.dur.max(0)));
        let n_events = recipes.count() as i64;
        let nhits = tr
            .acts
            .iter()
            .filter(|a| a.flags & F_CACHE_HIT != 0 && tr.action_in_run(a, run_id.as_deref()))
            .count() as i64;
        let restored: f64 = tr
            .acts
            .iter()
            .filter(|a| tr.action_in_run(a, run_id.as_deref()))
            .filter_map(|a| a.cache_bytes)
            .sum();
        let selected = tr.selected_summary(run_id.as_deref());
        let summary = detail::summary_line(&detail::SummaryFacts {
            n_events,
            span: selected.span_us.max(1),
            busy,
            nlanes: selected.lanes as i64,
            cache_events: Some((nhits, restored)),
        });
        // fit the *plot*, not the window: dividing the span by the
        // window width leaves the last gutter-plus-scrollbar worth of
        // the trace (4.4% of it, 10.2 s on a large capture) hanging off
        // the right edge before the user has touched anything.
        let upp = tr.span as f64 / f64::from(canvas::DEFAULT_CANVAS_W);
        let n_events_v = n_events as usize;
        let command_view = command_ui::State::new(&tr, run_id.as_deref());
        let n_acts = tr.acts.len();
        let mut selected_class_count = [0; config::CLASS_N];
        for a in &tr.acts {
            if tr.action_in_run(a, run_id.as_deref()) {
                selected_class_count[(a.color as usize).min(config::CLASS_N - 1)] += 1;
            }
        }
        let failed_list = tr
            .acts
            .iter()
            .enumerate()
            .filter(|(_, a)| a.flags & F_FAILED != 0 && tr.action_in_run(a, run_id.as_deref()))
            .map(|(i, _)| i)
            .collect();
        Ready {
            command_view,
            reported_path: Default::default(),
            tr: Arc::new(tr),
            run_id,
            summary,
            selected: None,
            nav_req: None,
            hist: Vec::new(),
            nav_back: false,
            t_left: 0.0,
            upp,
            scroll: 0.0,
            row_h: config::get().appearance.row_height.unwrap_or(11.0),
            show_conc: config::get().appearance.concurrency_strip.unwrap_or(true),
            show_gutter: config::get().appearance.lane_gutter.unwrap_or(true),
            cp_on: false,
            painted: 0,
            filter: String::new(),
            matches: None,
            match_list: Vec::new(),
            match_cursor: 0,
            find_open: false,
            find_focus: false,
            minimap: None,
            conc: Vec::new(),
            conc_max: 1,
            stage_cache: HashMap::new(),
            activity_cache: HashMap::new(),
            activity_state: vec![0; n_acts],
            activity_cache_intervals: 0,
            lookup_cache: HashMap::new(),
            anc_set: None,
            anc_list: Vec::new(),
            anc_title: String::new(),
            anc_window: false,
            ctx_menu: None,
            detail: None,
            n_events: n_events_v,
            selected_class_count,
            cp_step: 0,
            cp_selected: None,
            cp_view: stagebar::BarState::default(),
            panels: Panels::default(),
            longest: Vec::new(),
            longest_data: HashMap::new(),
            failed_list,
            cp_list: Vec::new(),
            digest_ix: None,
            digest_running: None,
            digest_error: None,
            // corrected by the first painted frame
            canvas_x0: canvas::GUTTER_W,
            canvas_w: canvas::DEFAULT_CANVAS_W,
            canvas_h: canvas::DEFAULT_CANVAS_H,
            gstate,
            graph_explicit,
            graph_warning: None,
            native_popups: config::get().behavior.native_popups.unwrap_or(true),
            open: open::OpenState::default(),
            open_warnings: Vec::new(),
            open_status_lines: Vec::new(),
            toast: None,
            an: None,
            an_running: None,
            analysis_requested: false,
            analysis_reports_requested: AnalysisReports::default(),
            an_lacks_graph: false,
            an_error: None,
            ov_sort: (2, true),
            sorts: HashMap::new(),
            cp_set: None,
            cp_overrides: Vec::new(),
            cp_sim: None,
            cp_sim_rx: None,
            cp_sim_error: None,
            cp_margins: Vec::new(),
            cp_savings: HashMap::new(),
            cp_savings_rx: None,
            cp_savings_error: None,
            cp_savings_combined: None,
            cp_ctx: None,
            cascade: cascade::CascadeState::default(),
        }
    }

    /// What the menu bar's "Close popups" does. Popup open-state that
    /// lives outside `Panels` has to be cleared here as well, or the
    /// button leaves those windows on screen while claiming otherwise.
    pub fn close_all_popups(&mut self) {
        self.cascade.cancel();
        self.panels = Panels::default();
        self.ctx_menu = None;
        // the prompt window hangs off the detail popup, so dropping the
        // detail closes it too
        self.detail = None;
        self.anc_window = false;
        self.cp_ctx = None;
        self.find_open = false;
    }

    /// Open the action-detail popup on act `idx`: re-read its record
    /// from the file by byte offset and build the row list once.
    pub fn select(&mut self, idx: usize) {
        // Re-clicking the selected action does not add a duplicate Back entry.
        if !std::mem::take(&mut self.nav_back)
            && let Some(prev) = self.selected
            && prev != idx
        {
            self.hist.push(prev);
            if self.hist.len() > 64 {
                self.hist.remove(0);
            }
        }
        self.selected = Some(idx);
        self.detail = detail_ui::build_detail(self, idx);
        if self.detail.is_none() {
            // the record could not be re-read from the file (truncated,
            // replaced, unreadable range): a click that selects but
            // opens nothing must say why
            self.toast = Some((
                "cannot re-read this action's record from the trace file (was it truncated or replaced?)"
                    .to_owned(),
                Instant::now(),
            ));
        }
    }

    /// Return to the previously selected action, if there is one.
    pub fn go_back(&mut self, width: f32, view_h: f32) {
        if let Some(prev) = self.hist.pop() {
            self.nav_back = true;
            self.navigate(prev, width, view_h);
        }
    }

    pub fn fit(&mut self, width: f32) {
        self.t_left = 0.0;
        self.upp = self.tr.span as f64 / width.max(100.0) as f64;
    }

    /// Hard limits on the viewport, applied once per frame AFTER all
    /// input so no individual path (drag, wheel, pinch, keys, minimap,
    /// scrollbar, navigate) can escape them.
    pub fn clamp_view(&mut self, width: f32, view_h: f32) {
        let span = self.tr.span.max(1) as f64;
        let w = width.max(1.0) as f64;
        // never zoom out past the whole trace; the max is data-derived
        // and f64::clamp panics when min > max (degenerate span on a
        // wide window), so order the bounds
        self.upp = self.upp.clamp(0.05, (span / w).max(0.05));
        let view_us = self.upp * w;
        self.t_left = if view_us >= span {
            0.0
        } else {
            self.t_left.clamp(0.0, span - view_us)
        };
        let total_h = self.tr.lanes.len() as f32 * self.row_h;
        self.scroll = self.scroll.clamp(0.0, (total_h - view_h).max(0.0));
    }

    pub fn zoom_at(&mut self, factor: f64, px: f32) {
        let t_at = self.t_left + self.upp * px as f64;
        self.upp = (self.upp / factor).clamp(0.05, (self.tr.span as f64).max(0.05));
        self.t_left = t_at - self.upp * px as f64;
    }

    /// The vertical counterpart of [`Ready::zoom_at`]: taller or shorter
    /// lane rows, holding the lane under the cursor still. `py` is the
    /// cursor's offset from the top of the lane area, not from the top
    /// of the window.
    pub fn zoom_rows_at(&mut self, factor: f32, py: f32) {
        let lane_at = (self.scroll + py.max(0.0)) / self.row_h.max(0.01);
        self.row_h = (self.row_h * factor).clamp(canvas::ROW_H_MIN, canvas::ROW_H_MAX);
        self.scroll = (lane_at * self.row_h - py.max(0.0)).max(0.0);
    }

    pub fn navigate(&mut self, idx: usize, width: f32, view_h: f32) {
        let a = self.tr.acts[idx];
        if self.upp > (a.dur as f64) / (0.15 * width as f64) {
            self.upp = ((a.dur as f64) / (0.3 * width as f64)).max(0.05);
        }
        self.t_left = a.ts as f64 + a.dur as f64 / 2.0 - self.upp * width as f64 / 2.0;
        self.scroll = (a.lane as f32 * self.row_h - view_h / 2.0).max(0.0);
        self.select(idx);
    }

    pub fn clear_selection(&mut self) {
        if let Some(selected) = self.selected.take() {
            if self.hist.last() != Some(&selected) {
                self.hist.push(selected);
            }
        }
    }

    /// Recompute the match set for the current `filter`: case-insensitive
    /// substring over the full target string, plus digest-prefix matching
    /// so a digest copied out of another tool or a log jumps straight to its
    /// action. Clears the set when the query is empty.
    pub fn apply_filter(&mut self) {
        self.match_cursor = 0;
        if self.filter.is_empty() {
            self.matches = None;
            self.match_list.clear();
            return;
        }
        let q = self.filter.to_lowercase();
        let as_digest = digest_query(&q);
        if as_digest {
            self.ensure_digest_ix();
        }
        let dix = self.digest_ix.as_deref().unwrap_or(&[]);
        let mut flags = vec![false; self.tr.acts.len()];
        let mut list = Vec::new();
        for (i, a) in self.tr.acts.iter().enumerate() {
            if a.flags & F_NOT_RECIPE != 0 || !self.tr.action_in_run(a, self.run_id.as_deref()) {
                continue;
            }
            let name = &self.tr.names[a.name as usize];
            let hit = contains_folded(name, &q)
                || (as_digest
                    && dix
                        .get(i)
                        .and_then(|d| d.as_deref())
                        .is_some_and(|d| d.starts_with(&q)));
            if hit {
                flags[i] = true;
                list.push(i);
            }
        }
        self.matches = Some(flags);
        self.match_list = list;
    }

    /// Names remain searchable while the optional digest index loads.
    fn ensure_digest_ix(&mut self) {
        if self.digest_ix.is_some() || self.digest_running.is_some() || self.digest_error.is_some()
        {
            return;
        }
        let tr = self.tr.clone();
        let (tx, worker, cancel) = Worker::channel();
        match std::thread::Builder::new()
            .name("retrace-digests".into())
            .spawn(move || {
                drop(tx.send(load_digest_index(&tr, &cancel)));
            }) {
            Ok(_) => self.digest_running = Some(worker),
            Err(e) => self.digest_error = Some(format!("cannot start digest index: {e}")),
        }
    }

    fn poll_digest_index(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.digest_running else {
            return;
        };
        match worker.try_recv() {
            Ok(Ok(index)) => {
                self.digest_ix = Some(index);
                self.digest_running = None;
                self.apply_filter();
            }
            Ok(Err(e)) => {
                self.digest_error = Some(format!("cannot index digests: {e:#}"));
                self.digest_running = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.digest_error = Some("digest index worker stopped; see stderr".into());
                self.digest_running = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100))
            }
        }
    }

    pub fn jump_match(&mut self, delta: isize, width: f32, view_h: f32) {
        if self.match_list.is_empty() {
            return;
        }
        let n = self.match_list.len() as isize;
        self.match_cursor = (((self.match_cursor as isize + delta) % n + n) % n) as usize;
        let idx = self.match_list[self.match_cursor];
        self.navigate(idx, width, view_h);
    }

    /// Kick the shared analysis computation if it is not already done or
    /// in flight. Every Analysis-menu item calls this.
    pub fn ensure_analysis(&mut self) {
        self.analysis_requested = true;
        self.analysis_reports_requested
            .include(AnalysisReports::for_panels(&self.panels));
        let completed = self
            .an
            .as_ref()
            .map_or(AnalysisReports::default(), |an| an.reports);
        let reports = self.analysis_reports_requested.pending(completed);
        if (self.an.is_some() && reports == AnalysisReports::default()) || self.an_running.is_some()
        {
            return;
        }
        // A pending graph would invalidate a graphless analysis immediately.
        if matches!(self.gstate, GState::Loading(_) | GState::Building(_)) {
            return;
        }
        self.an_error = None;
        let (tx, rx, cancel) = Worker::channel();
        let tr = self.tr.clone();
        let run_id = self.run_id.clone();
        let (g, bundle) = match &self.gstate {
            GState::Ready { g, dag, .. } => (Some(g.clone()), dag.clone()),
            _ => (None, None),
        };
        self.an_lacks_graph = g.is_none();
        let started = std::thread::Builder::new()
            .name("retrace-analysis".into())
            .spawn(move || {
                drop(
                    tx.send(
                        compute_analysis_cancellable(
                            &tr,
                            g.as_deref(),
                            bundle.as_deref(),
                            run_id.as_deref(),
                            reports,
                            &|| cancel.load(Ordering::Relaxed),
                        )
                        .map(Box::new),
                    ),
                );
            });
        match started {
            Ok(_) => self.an_running = Some(rx),
            Err(e) => self.an_error = Some(format!("cannot start analysis worker: {e}")),
        }
    }

    /// Override one step, replacing whatever that step was set to
    /// before. Two weights for one action are not a scenario, so the
    /// same act never appears twice.
    pub fn set_cp_override(&mut self, act: usize, w_us: i64, label: String) {
        if w_us < 0 {
            self.cp_sim_error = Some("a simulated duration cannot be negative".into());
            return;
        }
        let o = CpOverride::Node { act, w_us, label };
        match self
            .cp_overrides
            .iter_mut()
            .find(|x| matches!(x, CpOverride::Node { act: a, .. } if *a == act))
        {
            Some(slot) => *slot = o,
            None => self.cp_overrides.push(o),
        }
        self.respawn_cp_sim();
    }

    /// Whether `act` currently carries an override — what the context
    /// menu keys its "reset" entry off.
    pub fn cp_override_of(&self, act: usize) -> Option<&CpOverride> {
        self.cp_overrides
            .iter()
            .find(|x| matches!(x, CpOverride::Node { act: a, .. } if *a == act))
    }

    pub fn clear_cp_override(&mut self, act: usize) {
        self.cp_overrides
            .retain(|x| !matches!(x, CpOverride::Node { act: a, .. } if *a == act));
        self.respawn_cp_sim();
    }

    /// Drop the `k`th override, so a scenario row can remove exactly the
    /// entry it names.
    pub fn remove_cp_override(&mut self, k: usize) {
        if k < self.cp_overrides.len() {
            self.cp_overrides.remove(k);
            self.respawn_cp_sim();
        }
    }

    pub fn clear_cp_overrides(&mut self) {
        self.cp_overrides.clear();
        self.respawn_cp_sim();
    }

    /// The banner's subject line: one act only when exactly one step is
    /// overridden, since the banner names a single action.
    fn cp_sim_subject(&self) -> (Option<usize>, String) {
        match &self.cp_overrides[..] {
            [CpOverride::Node { act, label, .. }] => return (Some(*act), label.clone()),
            [CpOverride::FreeAdmission] => return (None, FREE_ADMISSION_LABEL.to_owned()),
            _ => {}
        }
        let name = |act: usize| -> String {
            self.tr.names[self.tr.acts[act].name as usize]
                .chars()
                .take(28)
                .collect()
        };
        let mut parts: Vec<String> = self
            .cp_overrides
            .iter()
            .take(3)
            .map(|o| match o {
                CpOverride::Node { act, label, .. } => format!("{} {label}", name(*act)),
                CpOverride::FreeAdmission => FREE_ADMISSION_LABEL.to_owned(),
            })
            .collect();
        if self.cp_overrides.len() > parts.len() {
            parts.push(format!("+{} more", self.cp_overrides.len() - parts.len()));
        }
        (
            None,
            format!(
                "{} overrides: {}",
                self.cp_overrides.len(),
                parts.join(" + ")
            ),
        )
    }

    /// Replay every live override, on a worker thread: the replay walks
    /// a multi-million-node DAG and must never run on the UI thread.
    /// Called by every mutation of `cp_overrides`, so the displayed
    /// scenario is always the one the overrides describe.
    pub fn respawn_cp_sim(&mut self) {
        self.cascade.invalidate();
        self.cp_margins = vec![None; self.cp_overrides.len()];
        // the previous scenario's numbers must not sit under the NEW
        // override rows while the replay is in flight; `cp_sim_rx`
        // doubles as the walker's "replaying" indicator
        self.cp_sim = None;
        self.cp_sim_rx = None;
        self.cp_sim_error = None;
        if self.cp_overrides.is_empty() {
            return;
        }
        let GState::Ready { dag: Some(b), .. } = &self.gstate else {
            self.cp_sim_error = Some(
                match self.gstate {
                    GState::Loading(_) | GState::Building(_) => {
                        "Waiting for the action dependency graph\u{2026}"
                    }
                    _ => "No action dependency graph is available for scenario replay.",
                }
                .into(),
            );
            return;
        };
        let b = b.clone();
        let (act, label) = self.cp_sim_subject();
        // act indices, resolved to node ids on the worker: the lookup is
        // a pass over every node in the graph. The override's own index
        // rides along so its marginal can be streamed back to the row
        // that owns it.
        let mut want: Vec<(usize, usize, i64)> = Vec::new();
        let mut free_row = None;
        for (k, o) in self.cp_overrides.iter().enumerate() {
            match o {
                CpOverride::Node { act, w_us, .. } => want.push((k, *act, *w_us)),
                CpOverride::FreeAdmission => free_row = Some(k),
            }
        }
        let admission = if free_row.is_some() {
            match self.an.as_ref().and_then(|an| an.admission.clone()) {
                Some(a) if a.n_recorded > 0 => Some(a),
                Some(_) => {
                    self.cp_sim_error = Some("Execution-admission timing was not recorded; this scenario is unavailable.".into());
                    return;
                }
                None => {
                    self.ensure_analysis();
                    self.cp_sim_error = Some("Loading execution-admission measurements…".into());
                    return;
                }
            }
        } else {
            None
        };
        let (tx, rx, cancel) = Worker::channel();
        self.cp_sim_rx = Some(rx);
        let started = std::thread::Builder::new()
            .name("retrace-scenario".into())
            .spawn(move || {
                let canceled = || cancel.load(Ordering::Relaxed);
                if canceled() {
                    return;
                }
                let base = admission
                    .as_ref()
                    .map_or_else(|| b.dag.weight_us.clone(), |a| a.weights.clone());
                let acts: Vec<(usize, i64)> = want.iter().map(|&(_, act, w)| (act, w)).collect();
                let Some(sc) = sim::stack_overrides_cancellable(&b, base, &acts, &label, &canceled)
                else {
                    return;
                };
                let mut w = sc.w;
                let s = sc.sim;
                let w_us = s.path.iter().map(|&n| w[n as usize]).collect();
                let path = s
                    .path
                    .iter()
                    .map(|&n| b.dag.act_of_node[n as usize] as usize)
                    .collect();
                let cp_us = s.cp_us;
                if tx
                    .send(CpSimMsg::View(CpSimView {
                        act,
                        label: s.label,
                        base_cp_us: s.base_cp_us,
                        cp_us,
                        path,
                        w_us,
                    }))
                    .is_err()
                {
                    return;
                }
                // N more replays, one per row. That is what a marginal costs;
                // they stream so the scenario's own numbers never wait on them.
                for (g, &(k, _, _)) in sc.groups.iter().zip(&want) {
                    let Some(m) = sim::marginal_at_cancellable(&b.dag, &mut w, cp_us, g, &canceled)
                    else {
                        return;
                    };
                    if tx.send(CpSimMsg::Margin(k, m)).is_err() {
                        return;
                    }
                }
                if let Some(k) = free_row {
                    let g = sim::whole_graph_group(&b.dag.weight_us, &w, &sc.edits);
                    let Some(m) =
                        sim::marginal_at_cancellable(&b.dag, &mut w, cp_us, &g, &canceled)
                    else {
                        return;
                    };
                    drop(tx.send(CpSimMsg::Margin(k, m)));
                }
                tx.send(CpSimMsg::Complete).ok();
            });
        if let Err(e) = started {
            self.cp_sim_rx = None;
            self.cp_sim_error = Some(format!("cannot start scenario worker: {e}"));
        }
    }

    /// The config-level what-if: every node's execution-admission wait
    /// removed (the "raise `execution_concurrency_limit`" replay). It
    /// stacks with per-step overrides, so it is a toggle rather than a
    /// scenario of its own.
    pub fn toggle_cp_free_admission(&mut self) {
        match self
            .cp_overrides
            .iter()
            .position(|o| *o == CpOverride::FreeAdmission)
        {
            Some(k) => {
                self.cp_overrides.remove(k);
            }
            None => self.cp_overrides.push(CpOverride::FreeAdmission),
        }
        self.respawn_cp_sim();
    }

    /// Fill the CP walker's "savings if eliminated" column: one full
    /// replay per CP step. Small graphs compute inline — instant, and
    /// deterministic for the snapshot fixture; big graphs go to a
    /// worker thread streaming results as they land.
    pub fn ensure_cp_savings(&mut self) {
        // A worker that failed is not restarted on every frame; reopening the trace tries again.
        if !self.cp_savings.is_empty()
            || self.cp_savings_rx.is_some()
            || self.cp_savings_error.is_some()
        {
            return;
        }
        let GState::Ready { dag: Some(b), .. } = &self.gstate else {
            return;
        };
        let b = b.clone();
        // inline only when the whole sweep is small: each step costs a
        // full O(V+E) replay, and node count alone does not bound the
        // number of steps (a long serial chain froze the UI here)
        if b.dag
            .cp_path
            .len()
            .saturating_add(1)
            .saturating_mul(b.dag.weight_us.len().saturating_add(b.dag.preds.len()))
            <= 5_000_000
        {
            let dag = &b.dag;
            let mut w = dag.weight_us.clone();
            let base_cp = sim::replay(dag, &w).cp_us;
            for &node in &dag.cp_path {
                let act = dag.act_of_node[node as usize];
                if act == u32::MAX {
                    continue;
                }
                let s = sim::savings_at(dag, &mut w, base_cp, node, 0);
                self.cp_savings.insert(act as usize, s);
            }
            self.cp_savings_combined = combined_top_savings(&b, &self.cp_savings);
            return;
        }
        let (tx, rx, cancel) = Worker::channel();
        self.cp_savings_rx = Some(rx);
        let started = std::thread::Builder::new()
            .name("retrace-cp-savings".into())
            .spawn(move || {
                let canceled = || cancel.load(Ordering::Relaxed);
                let dag = &b.dag;
                let mut w = dag.weight_us.clone();
                let mut scratch = sim::ReplayScratch::new(w.len());
                let Some(base) = scratch.replay(dag, &w, &canceled) else {
                    return;
                };
                let base_cp = base.cp_us;
                let mut all: HashMap<usize, i64> = HashMap::new();
                for &node in &dag.cp_path {
                    let act = dag.act_of_node[node as usize];
                    if act == u32::MAX {
                        continue;
                    }
                    let Some(s) = scratch.savings_at(dag, &mut w, base_cp, node, 0, &canceled)
                    else {
                        return;
                    };
                    all.insert(act as usize, s);
                    if tx.send(CpSavingsMsg::One(act as usize, s)).is_err() {
                        return;
                    }
                }
                if let Some(c) = combined_top_savings(&b, &all) {
                    drop(tx.send(CpSavingsMsg::Combined(c.0, c.1)));
                }
                tx.send(CpSavingsMsg::Complete).ok();
            });
        if let Err(e) = started {
            self.cp_savings_rx = None;
            self.cp_savings_error = Some(format!("cannot start savings worker: {e}"));
        }
    }

    pub fn poll_graph(&mut self) {
        match &self.gstate {
            GState::Loading(rx) => {
                match rx.try_recv() {
                    Err(mpsc::TryRecvError::Empty) => {}
                    // a dead channel means the worker panicked; an
                    // eternal spinner would hide it
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.reject_graph(
                            "the graph worker stopped before producing a result; see stderr"
                                .to_owned(),
                        );
                    }
                    Ok(res) => match res {
                        Ok(g) => {
                            // the selected run, or the sole run; a
                            // sidecar from another capture of the build
                            // is a load failure, not a graph
                            if let Err(e) =
                                graph::check_trace_pair(&self.tr, self.run_id.as_deref(), &g.meta)
                            {
                                self.reject_graph(format!("{e:#}"));
                                return;
                            }
                            let g = Arc::new(g);
                            let (tx, rx2, cancel) = Worker::channel();
                            let tr = self.tr.clone();
                            let g2 = g.clone();
                            let run_id = self.run_id.clone();
                            let started = std::thread::Builder::new()
                                .name("retrace-stitch".into())
                                .spawn(move || {
                                    let Ok(d) = dag::build_full_cancellable(
                                        &tr,
                                        &g2,
                                        run_id.as_deref(),
                                        &cancel,
                                    ) else {
                                        return;
                                    };
                                    let d = d.map(|(b, st)| (Arc::new(b), st));
                                    drop(tx.send((g2, d)));
                                });
                            match started {
                                Ok(_) => self.gstate = GState::Building(rx2),
                                Err(e) => {
                                    self.reject_graph(format!("cannot start stitch worker: {e}"))
                                }
                            }
                        }
                        Err(e) => self.reject_graph(format!("{e:#}")),
                    },
                }
            }
            GState::Building(rx) => match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.reject_graph(
                        "the DAG stitch worker stopped before producing a result; see stderr"
                            .to_owned(),
                    );
                }
                Ok((g, built)) => {
                    let mut quality =
                        quality::Quality::assess(&self.tr, self.run_id.as_deref(), Some(&g));
                    let chain = built.as_ref().map(|(b, st)| dag::chain_from_bundle(b, st));
                    let summary = Arc::new(GraphSummary::new(&g, chain));
                    let dag = built.map(|(b, st)| {
                        quality = quality.stitched(&st);
                        b
                    });
                    if let Some(b) = &dag {
                        let d = &b.dag;
                        self.cp_list = d
                            .cp_path
                            .iter()
                            .map(|&n| d.act_of_node[n as usize] as usize)
                            .collect();
                        self.cp_set = Some(self.cp_list.iter().copied().collect());
                    }
                    self.gstate = GState::Ready {
                        g: summary,
                        dag,
                        quality: Box::new(quality),
                    };
                    if self.an_lacks_graph {
                        self.an = None;
                        self.an_running = None;
                    }
                    if self.analysis_requested {
                        self.ensure_analysis();
                    }
                    if !self.cp_overrides.is_empty() {
                        self.respawn_cp_sim();
                    }
                }
            },
            _ => {}
        }
    }

    fn reject_graph(&mut self, error: String) {
        if self.graph_explicit {
            self.gstate = GState::Failed(error);
        } else {
            let warning = format!("ignoring the graph beside this trace: {error}");
            eprintln!("warning: {warning}");
            self.toast = Some((warning.clone(), Instant::now()));
            self.graph_warning = Some(warning);
            self.gstate = GState::Absent;
        }
        if self.analysis_requested {
            self.ensure_analysis();
        }
        if !self.cp_overrides.is_empty() {
            self.respawn_cp_sim();
        }
    }
}

pub enum State {
    Loading {
        rx: Worker<anyhow::Result<LoadedTrace>>,
        since: Instant,
        path: String,
        converting: Arc<AtomicBool>,
        /// Candidate selectors are committed only after the load is accepted.
        run_sel: Option<String>,
        graph_path: Option<String>,
        /// the build that was on screen when this load started, held so a
        /// failure can put it back
        prev: Option<Box<Ready>>,
    },
    Failed(String),
    Ready(Box<Ready>),
}

pub struct App {
    pub state: State,
    /// the raw `--run` selector; resolved once the parse lands
    pub run_sel: Option<String>,
    /// Explicit CLI graph applies to the initial trace, not later Opens.
    pub graph_path: Option<String>,
    /// The decode limits the first load ran under, kept so the Open menu
    /// reads a second build the same way.
    pub opts: parse::LoadOpts,
    pub ingest_out: Option<std::path::PathBuf>,
    pub ingest_phases: bool,
    recovery_input: String,
}

impl App {
    fn initial(
        source: open::Request,
        run_sel: Option<String>,
        opts: &parse::LoadOpts,
        graph_path: Option<String>,
        ingest_out: Option<std::path::PathBuf>,
        ingest_phases: bool,
    ) -> Self {
        let request = LoadRequest {
            source,
            run_sel: run_sel.clone(),
            opts: opts.clone(),
            out: ingest_out.clone().unwrap_or_else(ingest::default_out_dir),
            phases: ingest_phases,
            graph: graph_path.clone(),
        };
        Self {
            state: spawn_load(request, None, load),
            run_sel,
            graph_path,
            opts: opts.clone(),
            ingest_out,
            ingest_phases,
            recovery_input: String::new(),
        }
    }

    /// Start loading whatever the Open menu asked for.
    ///
    /// A load that succeeds replaces `State::Ready`, which drops the
    /// previous `Ready` and with it the trace, the dependency graph, the
    /// minimap texture and every cache keyed off them. That is the whole
    /// mechanism: there is no list of things to reset, so there is
    /// nothing to leave off it. Until then the previous build rides along
    /// in `prev`, because a load that fails must not be the thing that
    /// discards it.
    fn begin(&mut self, req: open::Request) -> State {
        let reload = matches!((&req, &self.state), (open::Request::Trace(path), State::Ready(r)) if path == std::path::Path::new(&r.tr.path));
        let request = LoadRequest {
            source: req,
            run_sel: if reload { self.run_sel.clone() } else { None },
            opts: self.opts.clone(),
            out: self
                .ingest_out
                .clone()
                .unwrap_or_else(ingest::default_out_dir),
            phases: self.ingest_phases,
            graph: if reload {
                self.graph_path.clone()
            } else {
                None
            },
        };
        spawn_load(request, self.take_ready(), load)
    }

    /// Takes the build currently on screen, leaving the window with no
    /// state worth painting. Only [`App::begin`] calls this, and it puts
    /// a state back on the same expression.
    fn take_ready(&mut self) -> Option<Box<Ready>> {
        match std::mem::replace(&mut self.state, State::Failed(String::new())) {
            State::Ready(mut r) => {
                r.cascade.cancel();
                Some(r)
            }
            other => {
                self.state = other;
                None
            }
        }
    }

    /// Accept a completed request atomically with its selectors. A rejected
    /// parse or run must not change how the restored command is reloaded.
    fn finish_load(
        &mut self,
        loaded: anyhow::Result<LoadedTrace>,
        run_sel: Option<String>,
        graph_path: Option<String>,
        prev: Option<Box<Ready>>,
    ) -> State {
        match loaded.and_then(|tr| Ready::from_loaded(tr, run_sel.as_deref(), graph_path.clone())) {
            Ok(ready) => {
                self.recovery_input.clear();
                self.run_sel = run_sel;
                self.graph_path = graph_path;
                State::Ready(Box::new(ready))
            }
            Err(error) => restore(prev, format!("{error:#}")),
        }
    }
}

/// What a failed load leaves behind. A build that was already on screen
/// stays on screen and the error goes to a window over it, because the
/// person who asked for a second build did not ask to lose the first.
/// `State::Failed` is for a window that never had anything in it.
fn restore(prev: Option<Box<Ready>>, err: String) -> State {
    match prev {
        Some(mut r) => {
            r.open.err = Some(err);
            State::Ready(r)
        }
        None => State::Failed(err),
    }
}

/// Options captured before a load begins; later menu edits cannot change it.
struct LoadRequest {
    source: open::Request,
    run_sel: Option<String>,
    opts: parse::LoadOpts,
    out: std::path::PathBuf,
    phases: bool,
    graph: Option<String>,
}

/// The first command and later Open requests use the same nonblocking path.
fn spawn_load(
    request: LoadRequest,
    prev: Option<Box<Ready>>,
    loader: impl FnOnce(LoadRequest, &AtomicBool, &AtomicBool) -> anyhow::Result<LoadedTrace>
    + Send
    + 'static,
) -> State {
    let (tx, rx, cancel) = Worker::channel();
    let label = request.source.label();
    let run_sel = request.run_sel.clone();
    let graph_path = request.graph.clone();
    let converting = Arc::new(AtomicBool::new(matches!(
        request.source,
        open::Request::Ingest(..)
    )));
    let phase = converting.clone();
    let started = std::thread::Builder::new()
        .name("retrace-open".into())
        .spawn(move || {
            let result = loader(request, &cancel, &phase);
            if !cancel.load(Ordering::Relaxed) {
                drop(tx.send(result));
            }
        });
    match started {
        Ok(_) => State::Loading {
            rx,
            since: Instant::now(),
            path: label,
            converting,
            run_sel,
            graph_path,
            prev,
        },
        Err(e) => restore(prev, format!("cannot start load worker: {e}")),
    }
}

/// Resolve and parse on the loader thread, keeping conversion off the UI thread.
fn load(
    request: LoadRequest,
    cancel: &AtomicBool,
    converting: &AtomicBool,
) -> anyhow::Result<LoadedTrace> {
    let mut warnings = Vec::new();
    let mut status_lines = Vec::new();
    let path = match request.source {
        open::Request::Trace(p) => p,
        open::Request::Ingest(src, from) => {
            let output =
                ingest::run_cached_cancellable(&src, &request.out, &from, request.phases, cancel)?;
            for status in &output.status_lines {
                eprintln!("{status}");
            }
            status_lines = output.status_lines;
            warnings = output.warnings;
            output.trace_path
        }
    };
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{}: path is not utf-8", path.display()))?;
    converting.store(false, Ordering::Relaxed);
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "load canceled");
    let mut loaded = load_pipeline::load_cancellable(
        path,
        &request.opts,
        request.graph.as_deref(),
        true,
        cancel,
    )?;
    loaded.notices = warnings;
    loaded.status_lines = status_lines;
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "load canceled");
    Ok(loaded)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // collected inside the match and acted on after it, because
        // acting on it replaces the very state the match borrows
        let mut open_req = None;
        let mut cancel_load = false;
        match &mut self.state {
            State::Loading {
                rx,
                since,
                path,
                converting,
                run_sel,
                graph_path,
                prev,
            } => {
                match rx.try_recv() {
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let prev = prev.take();
                        self.state =
                            restore(prev, "the parse worker panicked; see stderr".to_owned());
                        ui.ctx().request_repaint();
                        return;
                    }
                    Ok(res) => {
                        let prev = prev.take();
                        let run_sel = run_sel.take();
                        let graph_path = graph_path.take();
                        self.state = self.finish_load(res, run_sel, graph_path, prev);
                        // the parse is what knows the build's name
                        if let State::Ready(r) = &self.state {
                            let label = build_label(&r.tr, r.run_id.as_deref());
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Title(window_title(
                                    label.as_deref(),
                                    &r.tr.path,
                                )));
                        }
                        ui.ctx().request_repaint();
                        return;
                    }
                }
                let elapsed = since.elapsed().as_secs_f32();
                let path = path.clone();
                let mut release_previous = false;
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(200.0);
                            ui.spinner();
                            ui.label(format!("Opening {path}"));
                            ui.weak(if converting.load(Ordering::Relaxed) {
                                "Resolving / converting event log (including download when needed)…"
                            } else {
                                "Loading trace and graph…"
                            });
                            ui.label(format!("{elapsed:.1}s"));
                            cancel_load = ui.button("Cancel open").clicked();
                            if prev.is_some() {
                                release_previous = ui.button("Release previous command to reduce memory").clicked();
                                ui.weak("After release, canceling or failing this Open cannot restore the previous command.");
                            }
                        });
                    });
                });
                if release_previous {
                    drop(prev.take());
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(60));
            }
            State::Failed(e) => {
                let e = e.clone();
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.label(egui::RichText::new(format!("retrace: {e}")).size(13.0));
                    ui.separator();
                    ui.label("Open another Command ID, event log, or trace JSON:");
                    let input = ui.text_edit_singleline(&mut self.recovery_input);
                    if (ui.button("Open").clicked()
                        || input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        && !self.recovery_input.trim().is_empty()
                    {
                        open_req = Some(open::classify(
                            self.recovery_input.trim(),
                            std::env::current_dir().unwrap_or_default(),
                        ));
                    }
                });
            }
            State::Ready(r) => {
                update_ready(r, ui);
                open_req = r.open.req.take();
            }
        }
        if cancel_load {
            if let State::Loading { prev, .. } = &mut self.state {
                let previous = prev.take();
                self.state = restore(
                    previous,
                    "Open canceled. You can open another command below.".into(),
                );
            }
        }
        if let Some(req) = open_req {
            self.state = self.begin(req);
            ui.ctx().request_repaint();
        }
    }
}

/// The Find overlay: the input that feeds the existing match set
/// (`filter` / `match_list`, navigated by n/N). Cmd/Ctrl+F and the View
/// menu open it, Esc closes it and keeps the selection.
fn show_find(app: &mut Ready, ctx: &egui::Context) {
    app.poll_digest_index(ctx);
    let toggled =
        ctx.input(|i| i.key_pressed(egui::Key::F) && (i.modifiers.command || i.modifiers.ctrl));
    if toggled {
        app.find_open = true;
        app.find_focus = true;
    }
    if !app.find_open {
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.find_open = false;
        return;
    }
    let screen = ctx
        .input(|i| i.viewport().inner_rect)
        .unwrap_or(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1500.0, 950.0),
        ));
    let mut nav: Option<usize> = None;
    let mut close = false;
    egui::Area::new(egui::Id::new("find_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x - 260.0, screen.top() + 56.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(520.0);
                if app.digest_running.is_some() {
                    ui.weak("Indexing digests… Name search remains available.");
                }
                if let Some(error) = &app.digest_error {
                    ui.label(error);
                }
                ui.horizontal(|ui| {
                    ui.label("Find action:");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut app.filter)
                            .desired_width(340.0)
                            .hint_text("substring, or a digest prefix"),
                    );
                    if app.find_focus {
                        r.request_focus();
                        app.find_focus = false;
                    }
                    if r.changed() {
                        app.apply_filter();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if let Some(&first) = app.match_list.first() {
                            nav = Some(first);
                            close = true;
                        }
                    }
                });
                ui.label(
                    egui::RichText::new(if app.filter.is_empty() {
                        "type to match action names; n / N cycle matches".to_owned()
                    } else if app.match_list.is_empty() {
                        "no matches".to_owned()
                    } else {
                        format!(
                            "{} matches \u{2014} Enter jumps to the first, n / N cycle",
                            humanize::count(app.match_list.len() as u64)
                        )
                    })
                    .size(10.5)
                    .weak(),
                );
                if !app.match_list.is_empty() {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for &i in app.match_list.iter().take(30) {
                                let a = app.tr.acts[i];
                                if ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{}   {}",
                                                humanize::dur(a.dur as f64),
                                                app.tr.names[a.name as usize]
                                            ))
                                            .monospace()
                                            .size(10.5),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .clicked()
                                {
                                    nav = Some(i);
                                    close = true;
                                }
                            }
                        });
                }
            });
        });
    if let Some(i) = nav {
        let w = screen.width();
        let h = screen.height();
        app.navigate(i, w, h);
    }
    if close {
        app.find_open = false;
    }
}

pub fn update_ready(app: &mut Ready, root: &mut egui::Ui) {
    open::poll(app);
    open::prompt_window(app, &root.ctx().clone());
    open::error_window(app, &root.ctx().clone());
    if app.command_view.actions.is_some() {
        show_find(app, &root.ctx().clone());
    }
    {
        // popup palette-level management: demote to Normal when the app
        // deactivates, promote back when it reactivates — floating
        // popups must not shadow OTHER apps.
        let ctx = root.ctx().clone();
        let popups: Vec<egui::ViewportId> = ctx.data_mut(|dd| {
            std::mem::take(
                dd.get_temp_mut_or_default::<Vec<egui::ViewportId>>(egui::Id::new(
                    "open_popups_prev",
                )),
            )
        });
        let any_popup_focused = ctx
            .data_mut(|dd| dd.remove_temp::<bool>(egui::Id::new("popup_focused")))
            .unwrap_or(false);
        let active = ctx.input(|i| i.viewport().focused.unwrap_or(false)) || any_popup_focused;
        let was = ctx
            .data(|dd| dd.get_temp::<bool>(egui::Id::new("app_was_active")))
            .unwrap_or(true);
        if active != was {
            let lvl = if active {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            for id in &popups {
                ctx.send_viewport_cmd_to(*id, egui::ViewportCommand::WindowLevel(lvl));
            }
            ctx.data_mut(|dd| dd.insert_temp(egui::Id::new("app_was_active"), active));
        }
        ctx.data_mut(|dd| {
            let cur = std::mem::take(
                dd.get_temp_mut_or_default::<Vec<egui::ViewportId>>(egui::Id::new("open_popups")),
            );
            dd.insert_temp(egui::Id::new("open_popups_prev"), cur);
        });
    }

    let ctx = &root.ctx().clone();
    // Both halves of ⇧Esc land here: the root viewport only sees the press
    // while the main window has OS keyboard focus, so a popup in front
    // raises the same request from its own viewport.
    let close_all = widgets::take_close_all(ctx)
        | ctx.input(|i| i.modifiers.shift && i.key_pressed(egui::Key::Escape));
    if close_all {
        app.close_all_popups();
    }
    app.poll_graph();
    app.cascade.poll();
    if app.cascade.is_running() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if matches!(app.gstate, GState::Loading(_) | GState::Building(_)) {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
    if let Some(rx) = &app.an_running {
        match rx.try_recv() {
            Ok(Ok(mut an)) => {
                if let Some(previous) = app.an.as_deref_mut() {
                    an.retain_completed_reports(previous);
                }
                app.an = Some(an);
                app.an_running = None;
                // A panel opened while this worker ran may need another
                // report. Preserve completed reports and launch only that gap.
                app.ensure_analysis();
                if app.cp_overrides.contains(&CpOverride::FreeAdmission) {
                    app.respawn_cp_sim();
                }
            }
            Ok(Err(e)) => {
                app.an_error = Some(format!("{e:#}"));
                app.an_running = None;
            }
            // a worker that panicked drops its sender without a send;
            // swallowing that would leave every panel spinning forever
            // behind a busy repaint loop
            Err(mpsc::TryRecvError::Disconnected) => {
                app.an_error = Some("the analysis worker panicked; see stderr".to_owned());
                app.an_running = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
    if let Some(rx) = &app.cp_sim_rx {
        loop {
            match rx.try_recv() {
                Ok(CpSimMsg::View(sim)) => app.cp_sim = Some(sim),
                // the worker addresses rows by position and the receiver
                // is replaced on every mutation, so a margin can only
                // belong to the list that is on screen
                Ok(CpSimMsg::Margin(k, m)) => {
                    if let Some(slot) = app.cp_margins.get_mut(k) {
                        *slot = Some(m);
                    }
                }
                Ok(CpSimMsg::Complete) => {
                    app.cp_sim_rx = None;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.cp_sim_error = Some(
                        "Scenario worker stopped before all results completed; see stderr.".into(),
                    );
                    app.cp_sim_rx = None;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    break;
                }
            }
        }
    }
    if let Some(rx) = &app.cp_savings_rx {
        loop {
            match rx.try_recv() {
                Ok(CpSavingsMsg::One(act, save)) => {
                    app.cp_savings.insert(act, save);
                }
                Ok(CpSavingsMsg::Combined(sum, together)) => {
                    app.cp_savings_combined = Some((sum, together));
                }
                Ok(CpSavingsMsg::Complete) => {
                    app.cp_savings_rx = None;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.cp_savings_error = Some(
                        "Savings worker stopped before all results completed; see stderr.".into(),
                    );
                    app.cp_savings_rx = None;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150));
                    break;
                }
            }
        }
    }

    // ---- health banner (capture warnings are never silent) ----
    if !app.open_warnings.is_empty() || !app.open_status_lines.is_empty() {
        let mut dismiss = false;
        egui::Panel::top("open-warnings").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Event-log open completed");
                dismiss = ui.small_button("Dismiss").clicked();
            });
            for warning in &app.open_warnings {
                ui.label(warning);
            }
            for status in &app.open_status_lines {
                ui.weak(status);
            }
        });
        if dismiss {
            app.open_warnings.clear();
            app.open_status_lines.clear();
        }
    }
    if !app.tr.health.warnings.is_empty() {
        egui::Panel::top("health")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0xf5, 0xe1, 0x9c))
                    .inner_margin(4.0),
            )
            .show(root, |ui| {
                for w in &app.tr.health.warnings {
                    ui.label(egui::RichText::new(format!("▲ capture: {w}")).size(11.0));
                }
            });
    }

    if app.command_view.actions.is_some() && app.minimap.is_none() {
        canvas::build_minimap(app, ctx);
    }
    if app.command_view.actions.is_some() && app.conc.is_empty() {
        let (c, m) = canvas::build_concurrency(&app.tr);
        app.conc = c;
        app.conc_max = m;
    }

    menu_bar(app, root, ctx);
    show_toast(app, ctx);
    if app.command_view.actions.is_none() {
        panels::show_panels(app, ctx);
        command_ui::show_inline(app, root);
        return;
    }
    command_ui::show(app, ctx);
    panels::show_panels(app, ctx);
    cascade::show(app, ctx);
    // nav_req only: navigate() in show_canvas records the CURRENT selection in
    // the Back history before moving — pre-assigning `selected` here
    // made every popup/walker navigation vanish from that history
    if let Some(n) = detail_ui::show_detail_ui(app, ctx) {
        app.nav_req = Some(n);
    }
    if let Some(n) = detail_ui::show_cp_walker(app, ctx) {
        app.nav_req = Some(n);
    }
    if let Some(n) = reported_path::show(app, ctx) {
        app.nav_req = Some(n);
    }
    let nav = app.nav_req.take();
    canvas::show_minimap(app, root);
    canvas::show_canvas(app, root, nav);
}

fn show_toast(app: &mut Ready, ctx: &egui::Context) {
    if let Some((msg, at)) = &app.toast {
        let age = at.elapsed().as_secs_f32();
        if age < 2.2 {
            let a = ((2.2 - age) / 0.6).clamp(0.0, 1.0);
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -70.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(egui::Color32::from_black_alpha((200.0 * a) as u8))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(msg)
                                    .color(egui::Color32::from_white_alpha((235.0 * a) as u8)),
                            );
                        });
                });
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        } else {
            app.toast = None;
        }
    }
}

fn menu_bar(app: &mut Ready, root: &mut egui::Ui, ctx: &egui::Context) {
    egui::Panel::top("bar").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("Open", |ui| open::menu(app, ui));
            ui.menu_button("Analysis", |ui| {
                if app.command_view.actions.is_none() {
                    ui.label("Command profile is shown in the main window");
                    ui.add_enabled(false, egui::Button::new("Critical Path"))
                        .on_disabled_hover_text(
                            "Not applicable: no completed actions are attributed to this command.",
                        );
                    ui.add_enabled(false, egui::Button::new("Simulation"))
                        .on_disabled_hover_text(
                            "Simulation requires a recorded action dependency graph.",
                        );
                    return;
                }
                if ui.button("Build Overview").clicked() {
                    app.panels.overview = true;
                    popup_raise(ctx, "Build overview");
                    app.ensure_analysis();
                    ui.close();
                }
                let have_dag = matches!(&app.gstate, GState::Ready { dag: Some(_), .. });
                let reported = !have_dag && reported_path::available(app);
                if ui
                    .add_enabled(have_dag || reported, egui::Button::new("Critical Path"))
                    .on_disabled_hover_text(
                        "Needs the recorded dependency graph (buck2 build --log-action-graph) \
                         or the critical path buck2 reports in its log",
                    )
                    .clicked()
                {
                    if have_dag {
                        app.panels.cp = true;
                        popup_raise(ctx, "Critical path (dependency graph)");
                    } else {
                        app.panels.reported_cp = true;
                        popup_raise(ctx, reported_path::TITLE);
                    }
                    ui.close();
                }
                if ui.button("Build Timeline").clicked() {
                    app.panels.command = true;
                    popup_raise(ctx, "Build timeline");
                    ui.close();
                }
                ui.separator();
                let analyses: [(&str, PanelOpener); 9] = [
                    ("Build breakdown by type", |r| r.panels.breakdown = true),
                    ("Busiest categories", |r| r.panels.rule_sites = true),
                    ("Exec-peer percentiles", |r| r.panels.peers = true),
                    ("Top memory (peak RSS)", |r| r.panels.top_mem = true),
                    ("What-if: CP-replay scenarios", |r| {
                        r.panels.whatif = true;
                    }),
                    ("Ranked findings", |r| {
                        r.panels.findings = true;
                    }),
                    ("Blast radius (recorded descendant cost)", |r| {
                        r.panels.blast = true;
                    }),
                    ("Upload breakdown (inputs lane)", |r| r.panels.upload = true),
                    ("Unaccounted time (stall audit)", |r| r.panels.stall = true),
                ];
                for (label, open) in analyses {
                    if ui.button(label).clicked() {
                        open(app);
                        popup_raise(ctx, label);
                        app.ensure_analysis();
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Longest actions").clicked() {
                    if app.longest.is_empty() {
                        let mut idx: Vec<usize> = (0..app.tr.acts.len())
                            .filter(|&i| {
                                app.tr.acts[i].flags & F_NOT_RECIPE == 0
                                    && app.tr.action_in_run(&app.tr.acts[i], app.run_id.as_deref())
                            })
                            .collect();
                        idx.sort_unstable_by_key(|&i| std::cmp::Reverse(app.tr.acts[i].dur));
                        idx.truncate(60);
                        app.longest_data = idx
                            .iter()
                            .filter_map(|&i| {
                                let a = &app.tr.acts[i];
                                let record = load_full(&app.tr, a.off, a.len)?;
                                Some((i, detail::DataMoved::of(&record["args"])))
                            })
                            .collect();
                        app.longest = idx;
                    }
                    app.panels.longest = true;
                    popup_raise(ctx, "Longest actions");
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !app.failed_list.is_empty(),
                        egui::Button::new(format!("Failed actions ({})", app.failed_list.len())),
                    )
                    .clicked()
                {
                    app.panels.failed = true;
                    popup_raise(ctx, "Failed actions");
                    ui.close();
                }
                {
                    let n = app
                        .an
                        .as_ref()
                        .and_then(|a| a.retries.as_ref())
                        .map(|(c, _)| c.actions)
                        .unwrap_or(0);
                    if ui
                        .add_enabled(n > 0, egui::Button::new(format!("Retried actions ({n})")))
                        .clicked()
                    {
                        app.panels.retried = true;
                        popup_raise(ctx, "Retried actions");
                        ui.close();
                    }
                }
                {
                    let n = app
                        .an
                        .as_ref()
                        .and_then(|a| a.cancelled.as_ref())
                        .map(|(c, _)| c.actions)
                        .unwrap_or(0);
                    if ui
                        .add_enabled(
                            n > 0,
                            egui::Button::new(format!("Cancelled attempts ({n})")),
                        )
                        .on_hover_text(
                            "Actions where buck2 raced a local and a remote attempt and \
                             cancelled the one that lost",
                        )
                        .clicked()
                    {
                        app.panels.cancelled = true;
                        popup_raise(ctx, "Cancelled attempts");
                        ui.close();
                    }
                }
            });
            ui.menu_button("View", |ui| {
                {
                    let mut embedded = !app.native_popups;
                    if ui
                        .checkbox(&mut embedded, "Popups inside main window")
                        .changed()
                    {
                        app.native_popups = !embedded;
                    }
                }
                ui.separator();
                // the only mouse path to Find: an always-on-top popup can own
                // the keyboard, and then the chord never reaches the canvas
                let chord = if cfg!(target_os = "macos") {
                    "\u{2318}F"
                } else {
                    "Ctrl+F"
                };
                if ui
                    .add_enabled(
                        app.command_view.actions.is_some(),
                        egui::Button::new(format!("Find action\u{2026}  {chord}")),
                    )
                    .clicked()
                {
                    app.find_open = true;
                    app.find_focus = true;
                    ui.close();
                }
                if ui
                    .add_enabled(
                        app.command_view.actions.is_some(),
                        egui::Button::new("Stage glossary"),
                    )
                    .clicked()
                {
                    app.panels.glossary = true;
                    popup_raise(ctx, "Stage glossary");
                    ui.close();
                }
                if ui.button("Parts of the window, and keys  ?").clicked() {
                    app.panels.help = true;
                    ui.close();
                }
            });
            if ui.button("Close popups  \u{21e7}Esc").clicked() {
                app.close_all_popups();
            }
            if app.command_view.actions.is_none() {
                ui.label("Command profile · no attributed completed actions");
                if let Some(warning) = &app.graph_warning {
                    ui.label(warning);
                }
                if let GState::Failed(error) = &app.gstate {
                    ui.label(format!("Graph unavailable: {error}"));
                }
                return;
            }
            if ui
                .button("Build timeline")
                .on_hover_text("Startup, analysis, actions and installation on one recorded clock")
                .clicked()
            {
                app.panels.command = true;
                popup_raise(ctx, "Build timeline");
            }

            ui.separator();
            for (c, name) in CLASS_NAMES.iter().enumerate() {
                if app.selected_class_count[c] == 0 {
                    continue;
                }
                ui.label(
                    egui::RichText::new(*name)
                        .background_color(config::class_color(c))
                        .color(egui::Color32::BLACK)
                        .size(10.0),
                );
            }
            ui.separator();

            // ---- health chips: the envelope, compact, with drill-in ----
            let mut chips: Vec<(String, egui::Color32)> = Vec::new();
            if !app.tr.health.warnings.is_empty() {
                chips.push((
                    format!("▲ {} warning(s)", app.tr.health.warnings.len()),
                    egui::Color32::from_rgb(0xf5, 0xe1, 0x9c),
                ));
            }
            if let Some(an) = &app.an {
                if an.extras_unmatched > 0 {
                    chips.push((
                        format!("args {} unmatched", an.extras_unmatched),
                        egui::Color32::from_rgb(0xf5, 0xe1, 0x9c),
                    ));
                }
                let st = &an.stall;
                if st.with_stages < st.nacts {
                    chips.push((
                        format!(
                            "stages {}/{}",
                            humanize::count(st.with_stages as u64),
                            humanize::count(st.nacts as u64)
                        ),
                        egui::Color32::from_gray(0xe0),
                    ));
                }
            }
            // capture and outcome chips come from the trace alone, so they
            // show before and without a graph
            let tq = quality::Quality::assess(&app.tr, app.run_id.as_deref(), None);
            chips.extend(quality_chips(&tq));
            let gchip = match &app.gstate {
                GState::Absent => match &app.graph_warning {
                    Some(w) => (
                        format!("graph: ignored ({w})"),
                        egui::Color32::from_rgb(0xf5, 0xe1, 0x9c),
                    ),
                    None => ("graph: none".to_owned(), egui::Color32::from_gray(0xe0)),
                },
                GState::Loading(_) => {
                    ("graph: loading…".to_owned(), egui::Color32::from_gray(0xe0))
                }
                GState::Building(_) => (
                    "graph: stitching…".to_owned(),
                    egui::Color32::from_gray(0xe0),
                ),
                GState::Failed(e) => (
                    format!("graph: failed ({e})"),
                    egui::Color32::from_rgb(0xf0, 0xc0, 0xc0),
                ),
                // a graph with a known gap never reads green
                GState::Ready { g, dag, quality } if !quality.graph_complete() => {
                    let (t, c) = quality_chips(quality)
                        .into_iter()
                        .find(|(t, _)| t.starts_with("graph:"))
                        .unwrap_or_else(|| {
                            (
                                format!(
                                    "graph: PARTIAL {} edges",
                                    humanize::count(g.n_edges as u64)
                                ),
                                egui::Color32::from_rgb(0xf5, 0xe1, 0x9c),
                            )
                        });
                    match dag {
                        Some(b) => (format!("{t} | CP {}", humanize::dur(b.dag.cp as f64)), c),
                        None => (t, c),
                    }
                }
                GState::Ready { g, dag, quality } => match dag {
                    Some(b) => (
                        format!(
                            "{}: {} edges | CP {}",
                            quality.graph_label(),
                            humanize::count(g.n_edges as u64),
                            humanize::dur(b.dag.cp as f64),
                        ),
                        egui::Color32::from_rgb(0xd2, 0xe8, 0xd2),
                    ),
                    None => (
                        format!(
                            "{}: {} edges (no DAG)",
                            quality.graph_label(),
                            humanize::count(g.n_edges as u64)
                        ),
                        egui::Color32::from_gray(0xe0),
                    ),
                },
            };
            chips.push(gchip);
            for (text, color) in chips {
                if ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .background_color(color)
                                .color(egui::Color32::BLACK)
                                .size(10.0),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text("click for capture health detail")
                    .clicked()
                {
                    app.panels.health = true;
                    popup_raise(ctx, "Capture health");
                    app.ensure_analysis();
                }
            }
        });
    });
}

/// Short build identity for the titlebar: the revision this binary was
/// built from when embedded. File timestamps do not establish build identity.
fn build_identity() -> String {
    if let Some(rev) = option_env!("BUCK_SCM_REV").or(option_env!("VERGEN_GIT_SHA")) {
        return rev.chars().take(12).collect();
    }
    "build identity not embedded".to_owned()
}

/// The short name for a capture: the target patterns the build was
/// launched with, e.g. `igios-no-extensions`. Long lists are elided —
/// the Build overview carries the full set, and the full artifact path.
/// `None` when the capture records no patterns, which is every capture
/// written before the ingester started recording them and whose argv
/// defeats the recovery heuristic.
pub fn build_label(tr: &Trace, run_id: Option<&str>) -> Option<String> {
    let t = tr.run_targets(run_id);
    let label = match t.len() {
        0 => return None,
        1 | 2 => t.join(", "),
        n => format!("{}, {} +{} more", t[0], t[1], n - 2),
    };
    Some(match label.char_indices().nth(90) {
        Some((i, _)) => format!("{}\u{2026}", &label[..i]),
        None => label,
    })
}

/// The titlebar text. The build identity always leads, because a viewer
/// window outlives rebuilds of its own binary: a stale process silently
/// renders a trace with fields it cannot map (an "unaccounted" band that
/// a newer build would have named). What follows names the capture —
/// the build's own name when we have it, the artifact path when we do
/// not.
fn window_title(label: Option<&str>, path: &str) -> String {
    match label {
        Some(l) => format!("retrace @ {} \u{2014} {l}", build_identity()),
        None => format!("retrace @ {} \u{2014} {path}", build_identity()),
    }
}

/// The GUI entry point: spawn the parse on a worker thread so the
/// window opens instantly, then hand the channel to eframe. This is the
/// only code in the crate that opens a window.
pub fn run(path: String, run_sel: Option<String>, opts: &parse::LoadOpts) -> anyhow::Result<()> {
    run_with_graph(path, run_sel, opts, None, None, false)
}

pub fn run_with_graph(
    path: String,
    run_sel: Option<String>,
    opts: &parse::LoadOpts,
    graph_path: Option<String>,
    ingest_out: Option<std::path::PathBuf>,
    ingest_phases: bool,
) -> anyhow::Result<()> {
    run_request(
        open::Request::Trace(path.into()),
        run_sel,
        opts,
        graph_path,
        ingest_out,
        ingest_phases,
    )
}

/// Open an unresolved CLI source without blocking on ingestion before the UI.
pub fn run_request(
    request: open::Request,
    run_sel: Option<String>,
    opts: &parse::LoadOpts,
    graph_path: Option<String>,
    ingest_out: Option<std::path::PathBuf>,
    ingest_phases: bool,
) -> anyhow::Result<()> {
    let title = window_title(None, &request.label());
    let app = App::initial(
        request,
        run_sel,
        opts,
        graph_path,
        ingest_out,
        ingest_phases,
    );
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 950.0]),
        // wgpu (Metal on macOS, Vulkan/GL elsewhere), NOT the default
        // glow backend: glow's CGL context loses its current NSView when
        // a macOS window's backing scale changes (a native popup viewport
        // opening on another display, a Retina<->non-Retina drag) and
        // aborts in glutin. The certified proto shipped on egui/wgpu for
        // the same reason.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let result = eframe::run_native(
        &title,
        native,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );
    // The process exits without unwinding the workers that would stop these.
    crate::interrupt::stop_children();
    // `eframe::Error` is not guaranteed Send+Sync, so it is formatted
    // rather than wrapped
    result.map_err(|e| {
        anyhow::anyhow!("cannot open a window here ({e}); use --stats or --json instead")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn steps_move_only_through_visible_rows() {
        let visible = [2, 5, 9];
        assert_eq!(super::step_targets(&visible, Some(5)), (Some(2), Some(9)));
        assert_eq!(super::step_targets(&visible, Some(2)), (None, Some(5)));
        assert_eq!(super::step_targets(&visible, Some(9)), (Some(5), None));
        assert_eq!(
            super::step_targets(&visible, Some(3)),
            (None, Some(2)),
            "a hidden selection steps to the first visible row"
        );
        assert_eq!(super::step_targets(&visible, None), (None, Some(2)));
        assert_eq!(super::step_targets(&[], Some(1)), (None, None));
    }

    #[test]
    fn full_record_read_is_bounded_before_allocating() {
        let (app, _source) = analysis_app();
        let act = app.tr.acts[0];
        assert!(load_full(&app.tr, act.off, act.len).is_some());
        assert!(load_full(&app.tr, act.off, detail::MAX_DETAIL_RECORD_BYTES as u32 + 1).is_none());
        assert!(load_full(&app.tr, u64::MAX, 1).is_none());
    }

    #[test]
    fn pending_node_override_replays_when_the_graph_finishes_loading() {
        let trace = parse::parse(&crate::testdata("smoke-trace.json")).unwrap();
        let graph = Arc::new(graph::load_graph(&crate::testdata("smoke-graph.jsonl")).unwrap());
        let (bundle, stats) = dag::build_full(&trace, &graph, None).unwrap();
        let mut app = Ready::new(trace, None);
        let (tx, rx, _) = Worker::channel();
        app.gstate = GState::Building(rx);
        app.set_cp_override(0, 0, "removed".into());
        assert!(app.cp_sim_error.as_deref().unwrap().contains("Waiting for"));
        assert!(app.cp_sim_rx.is_none());
        tx.send((graph, Some((Arc::new(bundle), stats)))).unwrap();
        app.poll_graph();
        assert!(app.cp_sim_error.is_none());
        let worker = app
            .cp_sim_rx
            .as_ref()
            .expect("loading must start the queued replay");
        let CpSimMsg::View(result) = worker
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
        else {
            panic!("the replay view must precede marginal results")
        };
        assert_eq!(result.act, Some(0));
        assert_eq!(result.path.len(), result.w_us.len());
        assert_eq!(result.w_us.iter().sum::<i64>(), result.cp_us);
    }

    #[test]
    fn rejected_graph_finishes_pending_scenario_with_an_unavailable_status() {
        for explicit in [false, true] {
            for disconnected in [false, true] {
                let trace = parse::parse(&crate::testdata("smoke-trace.json")).unwrap();
                let mut app = Ready::new(trace, None);
                let (tx, rx, _) = Worker::channel();
                app.gstate = GState::Loading(rx);
                app.graph_explicit = explicit;
                app.set_cp_override(0, 0, "removed".into());
                assert!(app.cp_sim_error.as_deref().unwrap().contains("Waiting for"));
                if disconnected {
                    drop(tx);
                } else {
                    tx.send(Err(anyhow::anyhow!("test graph refusal"))).unwrap();
                }
                app.poll_graph();
                assert!(if explicit {
                    matches!(app.gstate, GState::Failed(_))
                } else {
                    matches!(app.gstate, GState::Absent)
                });
                assert_eq!(
                    app.cp_sim_error.as_deref(),
                    Some("No action dependency graph is available for scenario replay.")
                );
                assert!(app.cp_sim.is_none() && app.cp_sim_rx.is_none());
                assert_eq!(app.cp_overrides.len(), 1);
            }
        }
    }

    #[test]
    fn simulated_path_duration_validation_rejects_mismatch_and_overflow() {
        let mut view = CpSimView {
            act: None,
            label: "test".into(),
            base_cp_us: 100,
            cp_us: 30,
            path: vec![0, 1],
            w_us: vec![10, 20],
        };
        assert!(view.has_consistent_duration());
        view.cp_us = 31;
        assert!(!view.has_consistent_duration());
        view.cp_us = 30;
        view.path.pop();
        assert!(!view.has_consistent_duration());
        view.path.push(1);
        view.w_us = vec![i64::MAX, 1];
        assert!(!view.has_consistent_duration());
        view.w_us = vec![-1, 31];
        assert!(!view.has_consistent_duration());
    }

    #[test]
    fn initial_open_preserves_cli_selectors_before_any_result() {
        let opts = parse::LoadOpts {
            threads: 1,
            ..Default::default()
        };
        let out = crate::temp_path("initial-open-cache");
        let app = App::initial(
            open::Request::Trace(crate::temp_path("missing-initial-trace.json")),
            Some("chosen-run".into()),
            &opts,
            Some("chosen-graph.json".into()),
            Some(out.clone()),
            true,
        );
        assert_eq!(app.run_sel.as_deref(), Some("chosen-run"));
        assert_eq!(app.graph_path.as_deref(), Some("chosen-graph.json"));
        assert_eq!(app.ingest_out.as_ref(), Some(&out));
        assert!(app.ingest_phases);
        assert_eq!(app.opts.threads, 1);
        let State::Loading {
            converting, prev, ..
        } = &app.state
        else {
            panic!("initial source should be resolved asynchronously")
        };
        assert!(!converting.load(Ordering::Relaxed));
        assert!(prev.is_none());
    }

    #[test]
    fn initial_native_loader_is_nonblocking_reports_phase_and_cancels() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (advance_tx, advance_rx) = mpsc::channel();
        let (phase_tx, phase_rx) = mpsc::channel();
        let (canceled_tx, canceled_rx) = mpsc::channel();
        let timeout = std::time::Duration::from_secs(60);
        let request = LoadRequest {
            source: open::Request::Ingest(ingest::Source::Last, "checkout".into()),
            run_sel: Some("chosen-run".into()),
            opts: parse::LoadOpts {
                threads: 1,
                ..Default::default()
            },
            out: "chosen-cache".into(),
            phases: true,
            graph: Some("chosen-graph.json".into()),
        };
        let state = spawn_load(request, None, move |request, cancel, converting| {
            assert!(matches!(
                request.source,
                open::Request::Ingest(ingest::Source::Last, _)
            ));
            assert_eq!(request.run_sel.as_deref(), Some("chosen-run"));
            assert_eq!(request.opts.threads, 1);
            assert_eq!(request.out, std::path::PathBuf::from("chosen-cache"));
            assert!(request.phases);
            assert_eq!(request.graph.as_deref(), Some("chosen-graph.json"));
            entered_tx.send(()).unwrap();
            advance_rx.recv_timeout(timeout).unwrap();
            converting.store(false, Ordering::Relaxed);
            phase_tx.send(()).unwrap();
            let deadline = Instant::now() + timeout;
            while !cancel.load(Ordering::Relaxed) {
                assert!(
                    Instant::now() < deadline,
                    "dropping the loader did not cancel it"
                );
                std::thread::yield_now();
            }
            canceled_tx.send(()).unwrap();
            anyhow::bail!("test load canceled")
        });
        entered_rx.recv_timeout(timeout).unwrap();
        let State::Loading {
            rx,
            converting,
            path,
            prev,
            ..
        } = &state
        else {
            panic!("loader must return while its work is still blocked")
        };
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(converting.load(Ordering::Relaxed));
        assert_eq!(path, "the newest local command");
        assert!(prev.is_none());
        advance_tx.send(()).unwrap();
        phase_rx.recv_timeout(timeout).unwrap();
        assert!(!converting.load(Ordering::Relaxed));
        drop(state);
        canceled_rx.recv_timeout(timeout).unwrap();
    }

    fn app_with_reload_selectors() -> App {
        App {
            state: State::Ready(Box::new(one_bar_app(10))),
            run_sel: Some("old-run".into()),
            graph_path: Some("old-explicit-graph.json".into()),
            opts: parse::LoadOpts {
                threads: 1,
                ..Default::default()
            },
            ingest_out: None,
            ingest_phases: false,
            recovery_input: String::new(),
        }
    }

    fn assert_reload_retains_selectors(app: &mut App) {
        assert_eq!(app.run_sel.as_deref(), Some("old-run"));
        assert_eq!(app.graph_path.as_deref(), Some("old-explicit-graph.json"));
        let State::Ready(ready) = &app.state else {
            panic!("the previous command must be restored")
        };
        let path = std::path::PathBuf::from(&ready.tr.path);
        let reload = app.begin(open::Request::Trace(path));
        let State::Loading {
            run_sel,
            graph_path,
            ..
        } = &reload
        else {
            panic!("reload should start with the restored command's selectors")
        };
        assert_eq!(run_sel.as_deref(), Some("old-run"));
        assert_eq!(graph_path.as_deref(), Some("old-explicit-graph.json"));
    }

    #[test]
    fn failed_new_open_preserves_previous_reload_selectors() {
        let mut app = app_with_reload_selectors();
        let pending = app.begin(open::Request::Trace(crate::temp_path(
            "missing-next-trace.json",
        )));
        let State::Loading {
            rx,
            run_sel,
            graph_path,
            prev,
            ..
        } = pending
        else {
            panic!("new Open must start asynchronously")
        };
        assert!(run_sel.is_none() && graph_path.is_none());
        let result = rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap();
        assert!(result.is_err());
        app.state = app.finish_load(result, run_sel, graph_path, prev);
        assert_reload_retains_selectors(&mut app);
    }

    #[test]
    fn canceled_new_open_preserves_previous_reload_selectors() {
        let mut app = app_with_reload_selectors();
        let pending = app.begin(open::Request::Trace(crate::temp_path(
            "cancel-next-trace.json",
        )));
        let State::Loading { prev, .. } = pending else {
            panic!("new Open must start asynchronously")
        };
        app.state = restore(prev, "Open canceled".into());
        assert_reload_retains_selectors(&mut app);
    }

    #[test]
    fn open_commits_selectors_only_after_run_selection_succeeds() {
        let mut app = app_with_reload_selectors();
        app.recovery_input = "stale recovery path".to_owned();
        let path = crate::testdata("smoke-trace.json");
        let loaded = load_pipeline::load(&path, &app.opts, None, false);
        let previous = app.take_ready();
        app.state = app.finish_load(loaded, Some("missing-run".into()), None, previous);
        assert_eq!(app.run_sel.as_deref(), Some("old-run"));
        assert_eq!(app.graph_path.as_deref(), Some("old-explicit-graph.json"));
        let State::Ready(ready) = &app.state else {
            panic!("invalid run selection must restore the previous command")
        };
        assert!(ready.open.err.is_some());
        assert_eq!(app.recovery_input, "stale recovery path");

        let loaded = load_pipeline::load(&path, &app.opts, None, false);
        let previous = app.take_ready();
        app.state = app.finish_load(loaded, None, None, previous);
        assert!(matches!(app.state, State::Ready(_)));
        assert!(app.run_sel.is_none() && app.graph_path.is_none());
        assert!(app.recovery_input.is_empty());
    }

    #[test]
    fn find_queries_and_receiver_cancellation_are_bounded() {
        assert!(super::contains_folded("A TARGET", "target"));
        assert!(super::contains_folded("short", ""));
        assert!(!super::contains_folded("short", "longer"));
        assert!(super::digest_query("ABCDEF0123:1234"));
        assert!(!super::digest_query("ABCDEF0123:-1"));
        assert!(!super::digest_query("bad:12"));
        let (_, worker, cancel) = super::Worker::<()>::channel();
        assert!(!cancel.load(super::Ordering::Relaxed));
        drop(worker);
        assert!(cancel.load(super::Ordering::Relaxed));
    }

    #[test]
    fn graph_pairing_is_checked_before_gui_stitching_for_action_only_traces() {
        let (tp, gp) = (
            crate::testdata("review-trace.json"),
            crate::testdata("review-mismatched-run-graph.jsonl"),
        );
        for explicit in [false, true] {
            let tr = parse::parse(&tp).unwrap();
            assert!(tr.runs.is_empty());
            let mut app = Ready::new(tr, None);
            app.graph_explicit = explicit;
            let (tx, rx, _) = Worker::channel();
            tx.send(graph::load_graph(&gp)).unwrap();
            app.gstate = GState::Loading(rx);
            app.poll_graph();
            assert!(app.cp_set.is_none());
            if explicit {
                assert!(matches!(app.gstate, GState::Failed(_)));
            } else {
                assert!(matches!(app.gstate, GState::Absent));
                assert!(app.graph_warning.as_ref().unwrap().contains("ignoring"));
            }
        }
    }

    #[test]
    fn quality_chips_show_partial_graph_and_unknown_outcome() {
        use crate::quality::GraphQuality;
        use crate::quality::Pairing;
        use crate::quality::Quality;
        use crate::quality::StitchQuality;
        use crate::types::CaptureState;
        use crate::types::CaptureStatus;
        use crate::types::Outcome;
        let q = Quality {
            capture: Some(CaptureState::Started(CaptureStatus::default())),
            outcome: Outcome::Unknown,
            outcome_unrecorded: true,
            dropped_trace_lines: 0,
            unended_runs: 0,
            graph: Some(GraphQuality {
                declared_nodes: Some(2),
                loaded_rows: 2,
                ..Default::default()
            }),
            stitch: Some(StitchQuality {
                n_events: 2,
                n_defined: 1,
                ..Default::default()
            }),
            pairing: Some(Pairing::Verified),
            graph_warning: None,
        };
        let texts: Vec<String> = quality_chips(&q).into_iter().map(|(t, _)| t).collect();
        assert_eq!(
            texts,
            [
                "capture: incomplete",
                "outcome: unknown",
                "graph: PARTIAL 1/2 actions with rows"
            ]
        );
        let whole = Quality {
            capture: Some(CaptureState::NotEnabled),
            outcome: Outcome::Success,
            outcome_unrecorded: false,
            dropped_trace_lines: 0,
            unended_runs: 0,
            graph: Some(GraphQuality {
                declared_nodes: Some(2),
                loaded_rows: 2,
                ..Default::default()
            }),
            stitch: Some(StitchQuality {
                n_events: 2,
                n_defined: 2,
                ..Default::default()
            }),
            pairing: Some(Pairing::Verified),
            graph_warning: None,
        };
        assert!(quality_chips(&whole).is_empty());
        let mut torn = whole;
        torn.graph.as_mut().unwrap().dropped_lines = 1;
        let texts: Vec<String> = quality_chips(&torn).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t.contains("line(s)")));
        assert!(!texts.iter().any(|t| t.contains("2/2 actions")));
    }

    use super::*;

    fn one_bar_app(dur: i64) -> Ready {
        let path = crate::temp_path(&format!("fit-{dur}.json"));
        std::fs::write(
            &path,
            format!(
                "[\n{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\
                 \"ts\":0,\"dur\":{dur}}},\n"
            ),
        )
        .unwrap();
        let tr = parse::parse(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        Ready::new(tr, None)
    }

    #[test]
    fn overview_does_not_request_advanced_replay_reports() {
        let (app, _source) = analysis_app();
        let base = compute_analysis_cancellable(
            &app.tr,
            None,
            None,
            None,
            AnalysisReports::default(),
            &|| false,
        )
        .unwrap();
        assert_eq!(base.reports, AnalysisReports::default());
        assert!(base.whatif.is_none() && base.findings.is_none() && base.blast.is_none());
        let full = compute_analysis(&app.tr, None, None, None).unwrap();
        assert_eq!(full.reports, AnalysisReports::ALL);
        assert!(full.findings.is_some());
        assert_eq!(base.ov_rows, full.ov_rows);
    }

    #[test]
    fn report_demand_tracks_panels_independently() {
        let mut panels = Panels {
            whatif: true,
            ..Panels::default()
        };
        let whatif = AnalysisReports::for_panels(&panels);
        assert!(whatif.whatif && !whatif.findings && !whatif.blast);

        panels.whatif = false;
        panels.findings = true;
        let findings = AnalysisReports::for_panels(&panels);
        assert!(!findings.whatif && findings.findings && !findings.blast);

        panels.findings = false;
        panels.blast = true;
        let blast = AnalysisReports::for_panels(&panels);
        assert!(!blast.whatif && !blast.findings && blast.blast);

        let mut requested = whatif;
        requested.include(findings);
        assert_eq!(requested.pending(whatif), findings);
        assert_eq!(requested.pending(requested), AnalysisReports::default());
    }

    #[test]
    fn completed_reports_are_retained_without_requesting_other_panels() {
        let (app, _source) = analysis_app();
        let findings = AnalysisReports {
            findings: true,
            ..AnalysisReports::default()
        };
        let whatif = AnalysisReports {
            whatif: true,
            ..AnalysisReports::default()
        };
        let mut previous =
            compute_analysis_cancellable(&app.tr, None, None, None, findings, &|| false).unwrap();
        assert!(previous.findings.is_some());
        assert!(previous.whatif.is_none() && previous.blast.is_none());
        let previous_count = previous.findings.as_ref().unwrap().0.nacts;

        let mut next =
            compute_analysis_cancellable(&app.tr, None, None, None, whatif, &|| false).unwrap();
        assert!(next.findings.is_none());
        next.retain_completed_reports(&mut previous);

        assert!(next.reports.whatif && next.reports.findings && !next.reports.blast);
        assert_eq!(next.findings.as_ref().unwrap().0.nacts, previous_count);
        assert!(
            previous.findings.is_none(),
            "the compact report was moved, not copied"
        );
        assert!(
            next.whatif.is_none(),
            "a completed graphless report is still absent"
        );
        assert!(next.blast.is_none(), "an unopened panel must not run");
    }

    #[test]
    fn missing_graph_report_is_complete_and_does_not_restart() {
        let (mut app, _source) = analysis_app();
        app.panels.whatif = true;
        let requested = AnalysisReports::for_panels(&app.panels);
        app.an = Some(Box::new(
            compute_analysis_cancellable(&app.tr, None, None, None, requested, &|| false).unwrap(),
        ));
        assert!(app.an.as_ref().unwrap().whatif.is_none());
        app.ensure_analysis();
        assert!(app.an_running.is_none());
        assert_eq!(app.analysis_reports_requested, requested);
    }

    #[test]
    fn canceled_report_does_not_mark_requested_work_complete() {
        let (app, _source) = analysis_app();
        let mut previous = compute_analysis_cancellable(
            &app.tr,
            None,
            None,
            None,
            AnalysisReports::default(),
            &|| false,
        )
        .unwrap();
        let requested = AnalysisReports {
            findings: true,
            ..AnalysisReports::default()
        };
        let canceled = compute_analysis_cancellable(&app.tr, None, None, None, requested, &|| true);
        assert_eq!(canceled.err().unwrap().to_string(), "analysis canceled");
        assert_eq!(requested.pending(previous.reports), requested);

        let mut retried =
            compute_analysis_cancellable(&app.tr, None, None, None, requested, &|| false).unwrap();
        retried.retain_completed_reports(&mut previous);
        assert_eq!(
            requested.pending(retried.reports),
            AnalysisReports::default()
        );
        assert!(retried.findings.is_some());
    }

    #[test]
    fn analysis_cancellation_is_returned_before_loading_reports() {
        let (app, _source) = analysis_app();
        let result =
            compute_analysis_cancellable(&app.tr, None, None, None, AnalysisReports::ALL, &|| true);
        assert_eq!(result.err().unwrap().to_string(), "analysis canceled");
    }

    struct AnalysisSource(std::path::PathBuf);
    impl Drop for AnalysisSource {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }
    fn analysis_app() -> (Ready, AnalysisSource) {
        let path = crate::temp_path("gui-analysis.json");
        std::fs::write(
            &path,
            "[\n{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":0,\"dur\":600}\n]\n",
        )
        .unwrap();
        let tr = parse::parse(path.to_str().unwrap()).unwrap();
        (Ready::new(tr, None), AnalysisSource(path))
    }

    #[test]
    fn cancelled_attempts_and_the_uploads_beside_them_open_their_actions() {
        let path = crate::temp_path("gui-cancelled.json");
        std::fs::write(
            &path,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"raced","ts":100,"dur":90000000,"args":{"run":"r","stages":{"input_upload_us":71000000},"re":{"iu_materialize_us":69000000,"iu_digests_queried":1,"cancelled_stages":[{"name":"Local.MaterializeInputs","ts":101,"dur":83000000}]}}}"#,
                "\n]\n",
            ),
        )
        .unwrap();
        let tr = parse::parse(path.to_str().unwrap()).unwrap();
        let (app, _source) = (Ready::new(tr, None), AnalysisSource(path));
        let an = compute_analysis_cancellable(
            &app.tr,
            None,
            None,
            None,
            AnalysisReports::default(),
            &|| false,
        )
        .unwrap();
        let (c, acts) = an
            .cancelled
            .as_ref()
            .expect("the lost local attempt is listed");
        assert_eq!((c.actions, c.local), (1, 1));
        let act = acts[0].expect("the row opens its action");
        assert_eq!(app.tr.names[app.tr.acts[act].name as usize], "raced");
        let (_, _, race) = an.upload.as_ref().expect("the upload has a breakdown");
        assert_eq!(race, &vec![("raced".to_owned(), Some(act))]);
    }

    #[test]
    fn overview_uses_the_displayed_chain_when_longest_paths_tie() {
        let trace_path = crate::temp_path("overview-tied-chain.json");
        let graph_path = crate::temp_path("overview-tied-chain.jsonl");
        let _trace_source = AnalysisSource(trace_path.clone());
        let _graph_source = AnalysisSource(graph_path.clone());
        std::fs::write(
            &trace_path,
            concat!(
                "[\n",
                "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a2\",\"ts\":0,\"dur\":5},\n",
                "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":5,\"dur\":5},\n",
                "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"b\",\"ts\":0,\"dur\":10},\n",
                "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"c\",\"ts\":10,\"dur\":1}\n]\n",
            ),
        )
        .expect("should write tied-path trace");
        std::fs::write(
            &graph_path,
            concat!(
                "{\"graph\":{\"enc\":2}}\n",
                "{\"t\":\"a2\",\"d\":[]}\n",
                "{\"t\":\"a\",\"d\":[\"a2\"]}\n",
                "{\"t\":\"b\",\"d\":[]}\n",
                "{\"t\":\"c\",\"d\":[\"a\",\"b\"]}\n",
            ),
        )
        .expect("should write tied-path graph");
        let tr = parse::parse(trace_path.to_str().expect("should have a UTF-8 test path"))
            .expect("should parse tied-path trace");
        let g = graph::load_graph(graph_path.to_str().expect("should have a UTF-8 test path"))
            .expect("should parse tied-path graph");
        let an = compute_analysis(&tr, Some(&g), None, None).expect("should compute overview");
        let cp = an
            .ov_rows
            .iter()
            .find(|row| row.key == "Action model CP")
            .expect("should report the action-model critical path");
        assert!(cp.value.contains("over 3 steps"), "{}", cp.value);
    }

    #[test]
    fn scenario_worker_routes_node_and_admission_marginals_to_original_rows() {
        let (mut app, _source) = analysis_app();
        let mut an = compute_analysis(&app.tr, None, None, None).unwrap();
        an.admission = Some(Arc::new(AdmissionWeights {
            weights: vec![80, 150, 200],
            n_recorded: 3,
            n_actions: 3,
            n_modified: 3,
        }));
        app.an = Some(Box::new(an));
        Arc::get_mut(&mut app.tr).unwrap().acts = vec![act_of(100), act_of(200), act_of(300)];
        let b = dag::DagBundle {
            dag: dag::Dag {
                act_of_node: vec![0, 1, 2],
                weight_us: vec![100, 200, 300],
                pred_off: vec![0, 0, 1, 2],
                preds: vec![0, 1],
                succ_off: vec![0, 1, 2, 2],
                succs: vec![1, 2],
                t1: 600,
                cp: 600,
                up: vec![100, 300, 600],
                down: vec![600, 500, 300],
                cp_path: vec![0, 1, 2],
            },
            node_ev: vec![Some(0), Some(1), Some(2)],
            events: (0..3)
                .map(|_| dag::Ev {
                    name: "action".into(),
                    ts: 0,
                    has_cache: false,
                })
                .collect(),
        };
        app.gstate = GState::Ready {
            g: Arc::new(GraphSummary {
                meta: Default::default(),
                n_rows: 3,
                n_edges: 2,
                n_joins: 0,
                chain: None,
            }),
            dag: Some(Arc::new(b)),
            quality: Box::new(quality::Quality::assess(&app.tr, None, None)),
        };
        app.cp_overrides = vec![
            CpOverride::Node {
                act: 2,
                w_us: 100,
                label: "first".into(),
            },
            CpOverride::FreeAdmission,
            CpOverride::Node {
                act: 0,
                w_us: 50,
                label: "last".into(),
            },
        ];
        app.respawn_cp_sim();
        let rx = app.cp_sim_rx.take().unwrap();
        loop {
            match rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("scenario completion")
            {
                CpSimMsg::View(v) => app.cp_sim = Some(v),
                CpSimMsg::Margin(k, m) => app.cp_margins[k] = Some(m),
                CpSimMsg::Complete => break,
            }
        }
        assert_eq!(app.cp_sim.as_ref().unwrap().cp_us, 300);
        assert_eq!(app.cp_margins, vec![Some(100), Some(50), Some(30)]);
    }

    /// A build that is open stays open. Typing a path that does not
    /// exist used to swap the whole window for the error text, so a
    /// misremembered build id cost the analysis on screen and there was
    /// no way back to it.
    #[test]
    fn a_failed_open_keeps_the_build_that_was_already_on_screen() {
        match restore(Some(Box::new(one_bar_app(10))), "no such file".to_owned()) {
            State::Ready(r) => assert_eq!(r.open.err.as_deref(), Some("no such file")),
            _ => panic!("a failed Open discarded the build that was already open"),
        }
        // ...but the first load has nothing to keep, so it still gets
        // the full-window message
        assert!(matches!(
            restore(None, "no such file".to_owned()),
            State::Failed(_)
        ));
    }

    /// The opening view has to show the whole trace. Seeding `upp` from
    /// the window width rather than the plot width left the last gutter
    /// plus scrollbar of the span off the right edge — 4.4% of it, ten
    /// seconds on a large capture — before the user touched anything.
    #[test]
    fn the_opening_zoom_fits_the_plot_not_the_window() {
        let app = one_bar_app(232_911_256);
        let shown = app.upp * f64::from(canvas::DEFAULT_CANVAS_W);
        assert!(
            shown >= app.tr.span as f64,
            "opening view shows {shown} us of a {} us trace",
            app.tr.span
        );
        // and it does not overshoot so far that the trace floats in
        // whitespace: one plot width, to the point
        assert!(shown < app.tr.span as f64 * 1.001);
    }

    /// Vertical zoom is the counterpart of `zoom_at`: the lane under the
    /// cursor is the one that must not move.
    #[test]
    fn row_zoom_holds_the_lane_under_the_cursor() {
        let mut app = one_bar_app(1_000_000);
        app.row_h = 11.0;
        app.scroll = 110.0; // lane 10 at the top of the lane area
        app.zoom_rows_at(2.0, 0.0);
        assert_eq!(app.row_h, 22.0);
        assert_eq!(app.scroll, 220.0, "lane 10 is still the top lane");

        // a lane halfway down a 300 pt viewport stays halfway down
        app.row_h = 10.0;
        app.scroll = 100.0;
        let lane_at_py = (app.scroll + 150.0) / app.row_h;
        app.zoom_rows_at(1.5, 150.0);
        assert!(((app.scroll + 150.0) / app.row_h - lane_at_py).abs() < 1e-3);
    }

    #[test]
    fn row_zoom_cannot_collapse_or_run_away() {
        let mut app = one_bar_app(1_000_000);
        for _ in 0..50 {
            app.zoom_rows_at(0.5, 0.0);
        }
        assert_eq!(app.row_h, canvas::ROW_H_MIN);
        for _ in 0..50 {
            app.zoom_rows_at(2.0, 0.0);
        }
        assert_eq!(app.row_h, canvas::ROW_H_MAX);
        assert!(app.scroll.is_finite() && app.scroll >= 0.0);
    }

    /// Degenerate inputs (empty trace, zero/near-zero span) must never
    /// panic in the viewport math: `f64::clamp` aborts when the
    /// data-derived max falls below the fixed min (seen live when a
    /// graph sidecar was passed as the trace).
    #[test]
    fn viewport_clamps_survive_degenerate_span() {
        let path = crate::temp_path("gui-degenerate.json");
        std::fs::write(
            &path,
            "[\n{\"ph\":\"M\",\"cat\":\"run\",\"name\":\"run_start\",\
             \"ts\":1000,\"pid\":1,\"args\":{\"run\":\"r1\"}},\n",
        )
        .unwrap();
        let tr = parse::parse(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut app = Ready::new(tr, None);
        app.clamp_view(1434.0, 800.0);
        app.zoom_at(2.0, 700.0);
        app.zoom_at(0.5, 700.0);
        app.clamp_view(1.0, 1.0);
        assert!(app.upp >= 0.05, "upp stays at or above the min bound");
    }

    /// Aaron's ask: the titlebar names the build, not the artifact. The
    /// path moves to the Build overview, so the fallback here is the
    /// only place it can still surface.
    #[test]
    fn titlebar_names_the_build_and_falls_back_to_the_path() {
        let dir = std::env::temp_dir();
        let write = |stem: &str, args: &str| -> Trace {
            let path = dir.join(format!("retrace-title-{}-{stem}.json", std::process::id()));
            std::fs::write(
                &path,
                format!(
                    "[\n{{\"ph\":\"M\",\"cat\":\"run\",\"name\":\"run_start\",                     \"ts\":1000,\"pid\":1,\"args\":{{\"run\":\"r1\",{args}}}}},\n"
                ),
            )
            .unwrap();
            let tr = parse::parse(path.to_str().unwrap()).unwrap();
            std::fs::remove_file(&path).unwrap();
            tr
        };

        // recorded outright by the ingester
        let tr = write("recorded", "\"targets\":[\"igios-no-extensions\"]");
        assert_eq!(
            build_label(&tr, None).as_deref(),
            Some("igios-no-extensions")
        );
        assert!(
            window_title(build_label(&tr, None).as_deref(), &tr.path)
                .ends_with(" \u{2014} igios-no-extensions"),
            "{}",
            window_title(build_label(&tr, None).as_deref(), &tr.path)
        );

        // recovered from a pre-existing capture's argv
        let tr = write(
            "argv",
            "\"argv\":[\"buck2\",\"build\",\"--log-action-deps\",\"igios-no-extensions\"]",
        );
        assert_eq!(
            build_label(&tr, None).as_deref(),
            Some("igios-no-extensions")
        );

        // three or more targets elide; the overview keeps the full set
        let tr = write(
            "many",
            "\"targets\":[\"//a:a\",\"//b:b\",\"//c:c\",\"//d:d\"]",
        );
        assert_eq!(
            build_label(&tr, None).as_deref(),
            Some("//a:a, //b:b +2 more")
        );

        // nothing recoverable: the window still says which file it holds
        let tr = write("none", "\"argv\":[\"buck2\",\"build\"]");
        assert_eq!(build_label(&tr, None), None);
        assert!(window_title(None, &tr.path).ends_with(&tr.path));
    }

    /// The frontier is both the button count and the list behind it, so
    /// a duplicated edge must count once and a join node must resolve to
    /// the action behind it.
    #[test]
    fn direct_frontier_dedups_and_resolves_joins() {
        let p = crate::testdata("smoke-trace.json");
        let tr = parse::parse(&p).unwrap();
        let g = crate::graph::load_graph(&crate::testdata("smoke-graph.jsonl")).unwrap();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let act = |n: usize| b.dag.act_of_node[n] as usize;
        let (y, z, w) = (act(0), act(1), act(2));
        assert_eq!(detail_ui::direct_neighbors(&b, z, true), vec![y]);
        // w's pred list carries y twice: direct, and through the tset row
        assert_eq!(detail_ui::direct_neighbors(&b, w, true), vec![y]);
        let mut want = vec![z, w];
        want.sort_unstable();
        // the exit node is a pure join: it contributes nothing of its own
        assert_eq!(detail_ui::direct_neighbors(&b, y, false), want);
    }

    /// Back unwinds the history rather than extending it, and one
    /// re-clicking the selected action is still one history entry.
    #[test]
    fn select_records_one_history_entry_per_navigation() {
        let p = crate::testdata("smoke-trace.json");
        let tr = parse::parse(&p).unwrap();
        let mut app = Ready::new(tr, None);
        app.select(0);
        app.select(1);
        app.select(1);
        assert_eq!(app.hist, vec![0]);
        app.go_back(1434.0, 800.0);
        assert_eq!(app.selected, Some(0));
        assert!(app.hist.is_empty());
        // and the Back jump itself is not a place to come back to
        app.select(0);
        assert!(app.hist.is_empty());
    }

    fn act_of(dur: i64) -> crate::types::Act {
        crate::types::Act {
            ts: 0,
            dur,
            lane: 0,
            color: 0,
            flags: 0,
            name: 0,
            off: 0,
            len: 0,
            seq: 0,
            dir: crate::types::NO_STR,
            why: crate::types::NO_STR,
            run: crate::types::NO_STR,
            cache_bytes: None,
        }
    }

    /// The walker's header total is only checked against `cp_us` by a
    /// `debug_assert!`, so a release build would show a wrong total in
    /// silence.
    #[test]
    fn the_walker_totals_the_simulated_weights_not_the_measured_ones() {
        let acts = [act_of(500), act_of(800), act_of(300)];
        let (list, w) = cp_walk_steps(None, &[0, 2], &acts);
        assert_eq!(list, vec![0, 2]);
        assert_eq!(w.iter().sum::<i64>(), 800);

        let sim = CpSimView {
            act: Some(1),
            label: "at 50%".to_owned(),
            base_cp_us: 1300,
            cp_us: 900,
            path: vec![0, 1],
            w_us: vec![500, 400],
        };
        let (list, w) = cp_walk_steps(Some(&sim), &[0, 2], &acts);
        assert_eq!(list, sim.path);
        assert_eq!(w.iter().sum::<i64>(), sim.cp_us);
        // the measured durations of those same steps do not
        assert_ne!(
            list.iter().map(|&i| acts[i].dur).sum::<i64>(),
            sim.cp_us,
            "the fixture has to make the two totals differ"
        );
    }

    /// The scenario rows address overrides by position, so removing one
    /// has to drop exactly that entry and leave the rest of the scenario
    /// standing.
    #[test]
    fn removing_one_override_leaves_the_rest_of_the_scenario() {
        let p = crate::testdata("smoke-trace.json");
        let tr = parse::parse(&p).unwrap();
        let mut app = Ready::new(tr, None);
        app.set_cp_override(0, 0, "removed".to_owned());
        app.set_cp_override(1, 50, "at 50%".to_owned());
        app.set_cp_override(2, 0, "removed".to_owned());
        app.remove_cp_override(1);
        let acts: Vec<usize> = app
            .cp_overrides
            .iter()
            .filter_map(|o| match o {
                CpOverride::Node { act, .. } => Some(*act),
                CpOverride::FreeAdmission => None,
            })
            .collect();
        assert_eq!(acts, vec![0, 2]);
        // the chips are built one frame and clicked the next, so an index
        // past the end is a no-op rather than a panic
        app.remove_cp_override(9);
        assert_eq!(app.cp_overrides.len(), 2);
    }

    /// Free admission is a config-level replay and the step overrides are
    /// per-action, so the two have to compose instead of replacing each
    /// other.
    #[test]
    fn free_admission_stacks_with_step_overrides_and_toggles_off() {
        let p = crate::testdata("smoke-trace.json");
        let tr = parse::parse(&p).unwrap();
        let mut app = Ready::new(tr, None);
        app.set_cp_override(0, 0, "removed".to_owned());
        app.toggle_cp_free_admission();
        assert_eq!(app.cp_overrides.len(), 2);
        assert!(app.cp_overrides.contains(&CpOverride::FreeAdmission));
        app.toggle_cp_free_admission();
        assert_eq!(app.cp_overrides.len(), 1);
        assert!(!app.cp_overrides.contains(&CpOverride::FreeAdmission));
    }

    /// Modules whose symbols never reach the screen: `graph.rs` spells
    /// JSON unescaping with `\u{fffd}` and `\u{000c}`, `ingest` writes
    /// files and terminal status lines only, `config` hands its
    /// complaints about a bad file to stderr before the window opens,
    /// and `help` prints to the terminal.
    const NO_DRAWN_TEXT: &[&str] = &["config", "graph", "help", "ingest", "interrupt"];

    /// Every source that produces text the window draws. A symbol is
    /// only ever a `\u{...}` escape or a literal in one of these, so
    /// scanning them catches an uncovered one the day it is written
    /// rather than the day someone looks at a screenshot.
    const SOURCES: &[(&str, &str)] = &[
        ("activity.rs", include_str!("activity.rs")),
        ("command_profile.rs", include_str!("command_profile.rs")),
        ("gui.rs", include_str!("gui.rs")),
        ("gui/canvas.rs", include_str!("gui/canvas.rs")),
        ("gui/cascade.rs", include_str!("gui/cascade.rs")),
        ("gui/command_ui.rs", include_str!("gui/command_ui.rs")),
        ("gui/detail_ui.rs", include_str!("gui/detail_ui.rs")),
        ("gui/execution_ui.rs", include_str!("gui/execution_ui.rs")),
        ("gui/reported_path.rs", include_str!("gui/reported_path.rs")),
        ("gui/open.rs", include_str!("gui/open.rs")),
        ("gui/panels.rs", include_str!("gui/panels.rs")),
        ("gui/stagebar.rs", include_str!("gui/stagebar.rs")),
        ("gui/widgets.rs", include_str!("gui/widgets.rs")),
        ("analyses.rs", include_str!("analyses.rs")),
        ("blast.rs", include_str!("blast.rs")),
        ("dag.rs", include_str!("dag.rs")),
        ("detail.rs", include_str!("detail.rs")),
        ("execution.rs", include_str!("execution.rs")),
        ("execution_fetch.rs", include_str!("execution_fetch.rs")),
        ("export.rs", include_str!("export.rs")),
        ("findings.rs", include_str!("findings.rs")),
        ("humanize.rs", include_str!("humanize.rs")),
        ("lanes.rs", include_str!("lanes.rs")),
        ("parse.rs", include_str!("parse.rs")),
        ("quality.rs", include_str!("quality.rs")),
        ("sim.rs", include_str!("sim.rs")),
        ("transfer.rs", include_str!("transfer.rs")),
        ("types.rs", include_str!("types.rs")),
        ("upload.rs", include_str!("upload.rs")),
        // CLASS_NAMES feeds the menu-bar legend; health warnings are
        // drawn in the banner and the Capture health panel
        ("types.rs", include_str!("types.rs")),
        ("parse.rs", include_str!("parse.rs")),
    ];

    fn symbols() -> Vec<(&'static str, char)> {
        let mut out = Vec::new();
        for (file, src) in SOURCES {
            for line in src.lines() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let bytes = line.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    // the escape opener is ASCII, so `i + 3` is a char
                    // boundary and the hex slice below cannot split one
                    if bytes[i..].starts_with(br"\u{") {
                        if let Some(end) = line[i + 3..].find('}') {
                            if let Some(c) = u32::from_str_radix(&line[i + 3..i + 3 + end], 16)
                                .ok()
                                .and_then(char::from_u32)
                            {
                                out.push((*file, c));
                            }
                            i += 3 + end + 1;
                            continue;
                        }
                    }
                    i += 1;
                }
                out.extend(line.chars().filter(|c| !c.is_ascii()).map(|c| (*file, c)));
            }
        }
        out
    }

    /// `SOURCES` is written by hand, so a module added without an entry
    /// would never be scanned and the guard below would quietly stop
    /// covering it. Both crate roots declare their modules and both are
    /// already included, so the list can check itself.
    #[test]
    fn the_scan_list_covers_every_module() {
        let scanned: Vec<&str> = SOURCES
            .iter()
            .map(|(f, _)| f.rsplit('/').next().unwrap().trim_end_matches(".rs"))
            .collect();
        let missing: Vec<&str> = [include_str!("lib.rs"), include_str!("gui.rs")]
            .iter()
            .flat_map(|src| src.lines())
            .filter_map(|l| l.strip_prefix("pub mod ")?.strip_suffix(';'))
            .filter(|m| !NO_DRAWN_TEXT.contains(m) && !scanned.contains(m))
            .collect();
        assert!(
            missing.is_empty(),
            "these modules are never scanned for uncovered glyphs; add them to \
             SOURCES, or to NO_DRAWN_TEXT with the reason: {missing:?}"
        );
    }

    #[test]
    fn every_symbol_the_ui_draws_has_a_glyph() {
        let syms = symbols();
        assert!(syms.len() > 8, "the source scan found nothing to check");

        let ctx = egui::Context::default();
        super::install_fonts(&ctx);
        // fonts do not exist until a pass has run; the pass's texture
        // deltas must be consumed before drop (epaint asserts on it)
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();

        // Monospace carries the same four faces in a different order,
        // so the proportional family answers for both.
        let id = egui::FontId::proportional(12.0);
        let mut missing: Vec<String> = ctx.fonts_mut(|f| {
            syms.iter()
                .filter(|(_, c)| !f.has_glyph(&id, *c))
                .map(|(file, c)| format!("{file}: U+{:04X} {c}", *c as u32))
                .collect()
        });
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "no bundled font covers these, so egui draws its replacement square instead:\n  {}",
            missing.join("\n  ")
        );
    }
}
