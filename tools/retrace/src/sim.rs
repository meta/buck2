/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What-if scenarios by critical-path replay.
//!
//! A scenario edits per-node durations from the
//! recorded stage counters (queue, fetch, exec, upload, …) and
//! replays the longest path over the SAME declared-dependency DAG the
//! critical-path panel uses.
//!
//! Every estimate this module emits is labeled `cp-replay`: the change
//! to the dependency floor, assuming action durations otherwise as
//! observed and enough parallelism to run at that floor. It is not a
//! span prediction: resource limits and scheduling are not modeled.

use fxhash::FxHashMap;
use fxhash::FxHashSet;

use crate::analyses::Extras;
use crate::analyses::PEER_MIN_N;
use crate::analyses::Stages;
use crate::analyses::UploadSub;
use crate::analyses::fmt_dur;
use crate::dag::Dag;
use crate::dag::DagBundle;
use crate::transfer::TransferBytes;
use crate::transfer::transfer_delta;

/// The fixed bandwidth scenarios replayed over the dependency DAG.
pub const NETWORK_SCENARIOS: [(&str, i64); 7] = [
    ("hotel wifi 10 MiB/s", 10 * 1024 * 1024),
    ("VPN 25 MiB/s", 25 * 1024 * 1024),
    ("office wifi 50 MiB/s", 50 * 1024 * 1024),
    ("wired 100 MiB/s", 100 * 1024 * 1024),
    ("wired 250 MiB/s", 250 * 1024 * 1024),
    ("local-ish 1 GiB/s", 1024 * 1024 * 1024),
    ("infinite", 1i64 << 60),
];

/// Per-node cost inputs, parallel to the DAG's node arrays. Join/exit
/// nodes carry defaults (zero weight, no stages) and are never
/// modified by a scenario.
#[derive(Clone)]
pub struct SimInput {
    /// baseline weights — a copy of `Dag::weight_us`
    pub base: Vec<i64>,
    pub stages: Vec<Option<Stages>>,
    pub bytes_in: Vec<i64>,
    pub bytes_out: Vec<i64>,
    /// per-node input-upload permit wait (0 = not recorded); from the
    /// instrumented producer's `args.re.iu_sem_wait_us`
    pub iu_sem_wait: Vec<i64>,
    /// per-node CAS upload call inside the input-upload band
    /// (`args.re.iu_cas_call_us`): `None` when the capture has no upload
    /// sub-stages, `Some(0)` when the producer measured no transfer at
    /// all. This is the part repriced by the logical-bytes/rate model,
    /// not a measurement of bytes moving on the network.
    pub iu_cas_call: Vec<Option<i64>>,
    /// bytes the client's upload batch declared
    /// (`args.re.iu_bytes_uploaded`, the nominal batch size before
    /// retries); `None` on captures that did not record it
    pub iu_bytes: Vec<Option<i64>>,
    /// Explicitly rejected upload fields must not activate legacy fallbacks.
    pub iu_invalid: Vec<u16>,
    /// rung-2 `args.re.rs_exec_sem_wait_us` — time parked on the
    /// execution_concurrency_limit before submission; zero on
    /// pre-rung-2 captures
    pub exec_sem_wait: Vec<i64>,
    /// Presence of a submit band or explicit admission-wait counter;
    /// zero-suppressed waits alone cannot establish instrumentation.
    pub admission_recorded: Vec<bool>,
    /// the whole rung-2 submit window (`args.re.submit_us`): permit
    /// wait plus stream open, all before the worker; newer producers
    /// also include request preparation (explicitly marked in args.re)
    pub submit: Vec<i64>,
    /// `Extras::srcs` interner id of the node's category
    pub cat: Vec<Option<u32>>,
    pub cache_hit: Vec<bool>,
    /// Known non-command actions cannot be execution/sharding candidates.
    pub cache_ineligible: Vec<bool>,
    /// nodes that come from a traced action (weight-carrying)
    pub n_action_nodes: usize,
    /// of those, how many carry `args.stages`
    pub n_with_stages: usize,
}

pub fn sim_input(b: &DagBundle, x: &Extras) -> SimInput {
    let n = b.dag.weight_us.len();
    let mut si = SimInput {
        base: b.dag.weight_us.clone(),
        stages: vec![None; n],
        bytes_in: vec![0; n],
        bytes_out: vec![0; n],
        iu_sem_wait: vec![0; n],
        iu_cas_call: vec![None; n],
        iu_bytes: vec![None; n],
        iu_invalid: vec![0; n],
        exec_sem_wait: vec![0; n],
        admission_recorded: vec![false; n],
        submit: vec![0; n],
        cat: vec![None; n],
        cache_hit: vec![false; n],
        cache_ineligible: vec![false; n],
        n_action_nodes: 0,
        n_with_stages: 0,
    };
    for (i, &act) in b.dag.act_of_node.iter().enumerate() {
        if act == u32::MAX {
            continue;
        }
        si.n_action_nodes += 1;
        let a = x.action_args(act as usize);
        si.stages[i] = a.stages;
        if a.stages.is_some() {
            si.n_with_stages += 1;
        }
        si.bytes_in[i] = a.bytes_in;
        si.bytes_out[i] = a.bytes_out;
        si.iu_sem_wait[i] =
            a.iu.and_then(|s| s.phase(UploadSub::PHASE_PERMIT_WAIT))
                .unwrap_or(0);
        si.iu_cas_call[i] = a.iu.and_then(|s| s.phase(UploadSub::PHASE_CAS_CALL));
        si.iu_bytes[i] = a.iu_bytes_uploaded;
        si.iu_invalid[i] = a.iu.map_or(0, |s| s.invalid)
            | if a.iu_bytes_invalid {
                UploadSub::BYTES
            } else {
                0
            };
        si.exec_sem_wait[i] = a.exec_sem_wait_us;
        si.admission_recorded[i] = a.admission_recorded;
        si.submit[i] = a.submit_us;
        si.cat[i] = a.src;
        si.cache_hit[i] = a.cache.is_some();
        si.cache_ineligible[i] = a.cache_ineligible;
    }
    si
}

/// The longest-path recompute under weights `w` (same node/edge
/// structure as `dag`). One forward pass over the topological ids.
pub struct Replay {
    pub cp_us: i64,
    pub t1_us: i64,
}

pub fn replay(dag: &Dag, w: &[i64]) -> Replay {
    replay_cancellable(dag, w, &|| false).expect("an uncancelled replay completes")
}

pub fn replay_cancellable(dag: &Dag, w: &[i64], cancelled: &impl Fn() -> bool) -> Option<Replay> {
    if cancelled() {
        return None;
    }
    ReplayScratch::new(w.len()).replay(dag, w, cancelled)
}

// ------------------------------------------------------------ scenarios

/// Counters may overlap execution. Removing one must not remove any
/// recorded current execution, nor increase an already shorter action.
pub fn execution_floor(si: &SimInput, node: usize) -> i64 {
    si.stages[node]
        .map_or(0, |s| s.execute_us)
        .clamp(0, si.base[node].max(0))
}

fn cut_counter(si: &SimInput, node: usize, cut: i64) -> i64 {
    si.base[node]
        .saturating_sub(cut.max(0))
        .max(execution_floor(si, node))
}

/// The same intervention used by the standard scenario and findings.
/// Derived residuals are not recorded non-execution counters.
pub fn non_execution_weight(si: &SimInput, node: usize) -> i64 {
    cut_counter(
        si,
        node,
        si.stages[node]
            .map_or(0, |st| (st.hard_sum() - st.execute_us).max(0))
            .saturating_add(si.submit[node]),
    )
}

pub fn admission_coverage(si: &SimInput) -> (usize, usize) {
    (
        si.admission_recorded.iter().filter(|&&v| v).count(),
        si.n_action_nodes,
    )
}

/// Weights with one recorded stage removed from every node that has it.
/// Returns the weights and how many nodes changed.
pub fn w_zero_stage(si: &SimInput, pick: impl Fn(&Stages) -> i64) -> (Vec<i64>, usize) {
    w_zero_stage_cancellable(si, pick, &|| false).expect("an uncancelled weight scan completes")
}

/// Build scenario weights without retaining a partial result on cancellation.
fn weights_cancellable(
    si: &SimInput,
    cancelled: &impl Fn() -> bool,
    weight: impl Fn(usize) -> i64,
) -> Option<(Vec<i64>, usize)> {
    if cancelled() {
        return None;
    }
    let mut weights = Vec::with_capacity(si.base.len());
    let mut modified = 0;
    for i in 0..si.base.len() {
        if i.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let value = weight(i);
        modified += usize::from(value != si.base[i]);
        weights.push(value);
    }
    Some((weights, modified))
}

pub(crate) fn w_zero_stage_cancellable(
    si: &SimInput,
    pick: impl Fn(&Stages) -> i64,
    cancelled: &impl Fn() -> bool,
) -> Option<(Vec<i64>, usize)> {
    w_zero_stage_where_cancellable(si, |_| true, pick, cancelled)
}

/// [`w_zero_stage_cancellable`] for only the nodes `selected` accepts.
pub(crate) fn w_zero_stage_where_cancellable(
    si: &SimInput,
    selected: impl Fn(usize) -> bool,
    pick: impl Fn(&Stages) -> i64,
    cancelled: &impl Fn() -> bool,
) -> Option<(Vec<i64>, usize)> {
    weights_cancellable(si, cancelled, |i| {
        if !selected(i) {
            return si.base[i];
        }
        si.stages[i].map_or(si.base[i], |stage| {
            let cut = pick(&stage);
            if cut > 0 {
                cut_counter(si, i, cut)
            } else {
                si.base[i]
            }
        })
    })
}

/// Weights with every queue wait capped at `cap_us`.
pub fn w_queue_cap(si: &SimInput, cap_us: i64) -> (Vec<i64>, usize) {
    w_zero_stage(si, |st| (st.queue_us.unwrap_or(0) - cap_us).max(0))
}

/// Weights with the recorded input-upload permit wait removed from
/// every node that measured one (instrumented captures only).
pub fn w_zero_permit_wait(si: &SimInput) -> (Vec<i64>, usize) {
    w_zero_permit_wait_cancellable(si, &|| false).expect("an uncancelled weight scan completes")
}

pub(crate) fn w_zero_permit_wait_cancellable(
    si: &SimInput,
    cancelled: &impl Fn() -> bool,
) -> Option<(Vec<i64>, usize)> {
    weights_cancellable(si, cancelled, |i| {
        if si.iu_sem_wait[i] > 0 {
            cut_counter(si, i, si.iu_sem_wait[i])
        } else {
            si.base[i]
        }
    })
}

/// Weights with every execution-admission wait removed; request
/// preparation and connection opening remain.
pub fn w_free_admission(si: &SimInput) -> (Vec<i64>, usize) {
    w_free_admission_cancellable(si, &|| false).expect("an uncancelled weight scan completes")
}

fn w_free_admission_cancellable(
    si: &SimInput,
    cancelled: &impl Fn() -> bool,
) -> Option<(Vec<i64>, usize)> {
    weights_cancellable(si, cancelled, |i| {
        if si.exec_sem_wait[i] > 0 {
            cut_counter(si, i, si.exec_sem_wait[i])
        } else {
            si.base[i]
        }
    })
}

/// Weights with recorded fetch/upload stages replaced by a modeled
/// transfer at `bps`, replayed over the recorded dependency DAG.
pub fn w_bandwidth(si: &SimInput, bps: i64) -> (Vec<i64>, usize) {
    w_bandwidth_cancellable(si, bps, &|| false).expect("an uncancelled weight scan completes")
}

fn w_bandwidth_cancellable(
    si: &SimInput,
    bps: i64,
    cancelled: &impl Fn() -> bool,
) -> Option<(Vec<i64>, usize)> {
    weights_cancellable(si, cancelled, |i| {
        let Some(stage) = si.stages[i] else {
            return si.base[i];
        };
        let bytes = TransferBytes {
            bytes_in: si.bytes_in[i],
            bytes_out: si.bytes_out[i],
            iu_cas_call_us: si.iu_cas_call[i],
            iu_bytes: si.iu_bytes[i],
            iu_invalid: si.iu_invalid[i] & (UploadSub::CAS_CALL | UploadSub::BYTES) != 0,
        };
        transfer_delta(&stage, bytes, bps as f64).map_or(si.base[i], |delta| {
            si.base[i].saturating_add(delta).max(execution_floor(si, i))
        })
    })
}

/// Weights with every recorded non-execution band removed, retaining
/// recorded current execution as a floor when counters overlap.
pub fn w_execute_only(si: &SimInput) -> (Vec<i64>, usize) {
    weights_cancellable(si, &|| false, |i| non_execution_weight(si, i))
        .expect("an uncancelled weight scan completes")
}

/// (preparation nodes priced from logical bytes, nodes with preparation).
pub fn fetch_pricing_coverage(si: &SimInput) -> (usize, usize) {
    let mut priced = 0;
    let mut preparation = 0;
    for (i, st) in si.stages.iter().enumerate() {
        if st.is_some_and(|s| s.materialize_us > 0) {
            preparation += 1;
            priced += usize::from(si.bytes_in[i] > 0);
        }
    }
    (priced, preparation)
}

/// How many staged nodes carry a client input upload, and how many of
/// those the capture priced with a recorded batch size rather than the
/// declared input size.
pub fn upload_pricing_coverage(si: &SimInput) -> (usize, usize) {
    let mut priced = 0usize;
    let mut recorded = 0usize;
    for (i, st) in si.stages.iter().enumerate() {
        if st.is_some_and(|s| s.input_upload_us > 0) {
            priced += 1;
            if si.iu_bytes[i].is_some() && si.iu_invalid[i] & UploadSub::BYTES == 0 {
                recorded += 1;
            }
        }
    }
    (priced, recorded)
}

/// The assumption every bandwidth row rests on, with this capture's
/// byte provenance counted in.
fn upload_pricing_note(
    (priced, recorded): (usize, usize),
    (fetch_priced, preparation): (usize, usize),
) -> String {
    format!(
        "hypothetical bytes/rate replacement, not a bandwidth measurement: input preparation \
         uses logical input size ({fetch_priced} of {preparation} preparation actions), \
         including files that may already be cache-resident on local or remote workers; \
         the client upload is priced \
         with the recorded upload batch size where the capture has it ({recorded} of {priced} \
         uploading actions) and with the declared input size otherwise, a proxy, not a bound; \
         rejected CAS-call or batch-size measurements are not replaced by fallbacks; \
         a measured zero CAS call is left alone; output upload uses declared logical output \
         size, not measured traffic, and legacy upload_us may mix input and output work"
    )
}

/// Nearest-rank p50 of the recorded queue waits (`None` when no node
/// carries a queue stage).
pub fn queue_p50(si: &SimInput) -> Option<i64> {
    let mut q: Vec<i64> = si
        .stages
        .iter()
        .filter_map(|st| st.and_then(|s| s.queue_us))
        .collect();
    nearest_rank_p50(&mut q)
}

fn nearest_rank_p50(values: &mut [i64]) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    // nearest rank: the ceil(N/2)-th smallest, index (N-1)/2
    let mid = (values.len() - 1) / 2;
    Some(*values.select_nth_unstable(mid).1)
}

// ------------------------------------------------------- standard set

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioBasis {
    RecordedCounters,
    DerivedResidual,
    LogicalBytesRate,
    StructuralHypothesis,
    PeerDuration,
}

#[derive(Clone, Debug)]
pub struct WhatIf {
    pub label: String,
    pub cp_us: i64,
    /// baseline CP minus this scenario's CP (negative = slower)
    pub saved_us: i64,
    pub n_modified: usize,
    /// Typed provenance, independent of display labels or explanatory prose.
    pub basis: ScenarioBasis,
    /// Relevant recorded observations / eligible observations. `None` denotes a purely
    /// hypothetical structural or peer-duration intervention.
    pub coverage: Option<(usize, usize)>,
    /// the assumption the number rests on
    pub note: String,
}

impl WhatIf {
    pub fn is_priced(&self) -> bool {
        self.coverage.is_none_or(|(measured, _)| measured > 0)
    }
}

pub struct WhatIfReport {
    pub base_cp_us: i64,
    pub t1_us: i64,
    pub n_action_nodes: usize,
    pub n_with_stages: usize,
    pub rows: Vec<WhatIf>,
}

/// The standard scenario set the panel and `--json=whatif` share.
pub fn standard_whatifs(b: &DagBundle, si: &SimInput) -> WhatIfReport {
    standard_whatifs_cancellable(b, si, &|| false)
        .expect("an uncancelled scenario report completes")
}

/// Cancellation reaches weight construction and every graph replay. No
/// partially computed report is returned. Allocations and standard-library
/// selection/sort operations are checked before and after, not interrupted.
pub fn standard_whatifs_cancellable(
    b: &DagBundle,
    si: &SimInput,
    cancelled: &impl Fn() -> bool,
) -> Option<WhatIfReport> {
    if cancelled() {
        return None;
    }
    let dag = &b.dag;
    let base_cp = dag.cp;
    let mut rows: Vec<WhatIf> = Vec::new();
    let mut scratch = ReplayScratch::new(dag.weight_us.len());
    let mut stage_measured = [0; 5];
    let mut queue_values = Vec::new();
    let mut upload_measured = 0;
    let (mut fetch_priced, mut preparation) = (0, 0);
    let (mut input_uploads, mut recorded_uploads, mut priced_uploads) = (0, 0, 0);
    let (mut output_uploads, mut priced_outputs) = (0, 0);
    let (mut non_execution_measured, mut admission_measured) = (0, 0);
    for (i, stage) in si.stages.iter().enumerate() {
        if i.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(value) = stage.and_then(|s| s.queue_us) {
            queue_values.push(value);
        }
        let scenario_fields: [&[u16]; 5] = [
            &[Stages::MATERIALIZE],
            &[Stages::INPUT_UPLOAD],
            &[Stages::UPLOAD, Stages::RESULT_WAIT, Stages::DOWNLOAD],
            &[Stages::CACHE_QUERY],
            &[Stages::RTT, Stages::DAEMON],
        ];
        for (count, fields) in stage_measured.iter_mut().zip(scenario_fields) {
            for &field in fields {
                *count += usize::from(stage.is_some_and(|s| s.has_any(field)));
            }
        }
        upload_measured += usize::from(
            (si.iu_cas_call[i].is_some() || si.iu_invalid[i] & UploadSub::CAS_CALL != 0)
                && si.iu_invalid[i] & UploadSub::SEM_WAIT == 0,
        );
        if stage.is_some_and(|s| s.materialize_us > 0) {
            preparation += 1;
            fetch_priced += usize::from(si.bytes_in[i] > 0);
        }
        if stage.is_some_and(|s| s.input_upload_us > 0) {
            input_uploads += 1;
            recorded_uploads +=
                usize::from(si.iu_bytes[i].is_some() && si.iu_invalid[i] & UploadSub::BYTES == 0);
            priced_uploads += usize::from(
                si.iu_invalid[i] & (UploadSub::CAS_CALL | UploadSub::BYTES) == 0
                    && (si.iu_bytes[i].is_some() || si.bytes_in[i] > 0),
            );
        }
        if stage.is_some_and(|s| s.upload_us > 0) {
            output_uploads += 1;
            priced_outputs += usize::from(si.bytes_out[i] > 0);
        }
        non_execution_measured += usize::from(
            stage.is_some_and(|s| s.has_any(Stages::HARD_NON_EXECUTION))
                || si.admission_recorded[i],
        );
        admission_measured += usize::from(si.admission_recorded[i]);
    }
    let queue = Some((queue_values.len(), si.n_action_nodes));
    let queue_median = nearest_rank_p50(&mut queue_values);
    drop(queue_values);
    if cancelled() {
        return None;
    }
    let upload_phases = Some((upload_measured, si.n_action_nodes));
    let transfer = Some((
        fetch_priced + priced_uploads + priced_outputs,
        preparation + input_uploads + output_uploads,
    ));
    let non_execution = Some((non_execution_measured, si.n_action_nodes));
    let [
        preparation_coverage,
        input_coverage,
        output_coverage,
        cache_coverage,
        residual_coverage,
    ] = std::array::from_fn::<_, 5, _>(|i| {
        Some((stage_measured[i], [1, 1, 3, 1, 2][i] * si.n_action_nodes))
    });
    let mut push = |label: String,
                    (w, n): (Vec<i64>, usize),
                    basis,
                    coverage: Option<(usize, usize)>,
                    note: &str|
     -> Option<()> {
        if cancelled() {
            return None;
        }
        let cp_us = if coverage.is_some_and(|(measured, _)| measured == 0) {
            base_cp
        } else {
            scratch.replay(dag, &w, cancelled)?.cp_us
        };
        rows.push(WhatIf {
            label,
            cp_us,
            saved_us: base_cp - cp_us,
            n_modified: n,
            basis,
            coverage,
            note: note.to_owned(),
        });
        Some(())
    };

    push(
        "RE queue eliminated".into(),
        w_zero_stage_cancellable(si, |st| st.queue_us.unwrap_or(0), cancelled)?,
        ScenarioBasis::RecordedCounters,
        queue,
        "every recorded stages.queue_us removed",
    )?;
    if let Some(p50) = queue_median {
        push(
            format!("RE queue capped at p50 ({})", fmt_dur(p50)),
            w_zero_stage_cancellable(si, |st| (st.queue_us.unwrap_or(0) - p50).max(0), cancelled)?,
            ScenarioBasis::RecordedCounters,
            queue,
            "waits above the median cut to the median",
        )?;
    }
    push(
        "input preparation counters removed".into(),
        w_zero_stage_cancellable(si, |st| st.materialize_us, cancelled)?,
        ScenarioBasis::RecordedCounters,
        preparation_coverage,
        "recorded input-preparation duration removed; the capture does not separate \
         network transfer, cache lookup, local materialization or other setup work",
    )?;
    push(
        "input uploads eliminated".into(),
        w_zero_stage_cancellable(si, |st| st.input_upload_us, cancelled)?,
        ScenarioBasis::RecordedCounters,
        input_coverage,
        "the explicit client input-upload counter removed; output upload and ambiguous \
         legacy upload_us remain unchanged",
    )?;
    if upload_measured > 0 {
        push(
            "input-upload permit wait eliminated".into(),
            w_zero_permit_wait_cancellable(si, cancelled)?,
            ScenarioBasis::RecordedCounters,
            upload_phases,
            "the client-side upload-permit wait (instrumented sub-stage) removed; \
             the transfer itself stays",
        )?;
    }
    if admission_measured > 0 {
        push(
            "execution admission waits removed".into(),
            w_free_admission_cancellable(si, cancelled)?,
            ScenarioBasis::RecordedCounters,
            Some((admission_measured, si.n_action_nodes)),
            "recorded execution-admission waits removed; request preparation and \
             connection opening remain; this does not predict a changed scheduling limit",
        )?;
    }
    push(
        "output and legacy result stages removed".into(),
        w_zero_stage_cancellable(
            si,
            |st| {
                st.upload_us
                    .saturating_add(st.result_wait_us)
                    .saturating_add(st.download_us)
            },
            cancelled,
        )?,
        ScenarioBasis::RecordedCounters,
        output_coverage,
        "output upload, legacy result-stage counter and result download removed; legacy \
         result_wait_us may be a worker-upload observation, not a client-processing wait, \
         and legacy upload_us may mix input and output work; coverage counts counter \
         observations across three fields, not actions (legacy/split schemas may omit fields)",
    )?;
    push(
        "cache lookups eliminated".into(),
        w_zero_stage_cancellable(si, |st| st.cache_query_us, cancelled)?,
        ScenarioBasis::RecordedCounters,
        cache_coverage,
        "every recorded stages.cache_query_us removed",
    )?;
    push(
        "derived client residual removed".into(),
        w_zero_stage_cancellable(si, |st| st.rtt_us.saturating_add(st.daemon_us), cancelled)?,
        ScenarioBasis::DerivedResidual,
        residual_coverage,
        "hypothetical removal of derived client-span residuals (including legacy rtt); \
         these counters establish neither waiting nor daemon CPU work; coverage counts \
         counter observations across two fields, not actions",
    )?;
    push(
        "recorded non-execution counters removed".into(),
        weights_cancellable(si, cancelled, |i| non_execution_weight(si, i))?,
        ScenarioBasis::RecordedCounters,
        non_execution,
        "recorded non-execution counters and the submit window removed; current \
         execution is preserved as a floor because counters can overlap; unclassified \
         residual and unmeasured actions remain, so this is not execution-only time",
    )?;
    let upload_pricing = upload_pricing_note(
        (input_uploads, recorded_uploads),
        (fetch_priced, preparation),
    );
    for &(label, bps) in &NETWORK_SCENARIOS {
        push(
            format!("logical input prep + upload at {label}"),
            w_bandwidth_cancellable(si, bps, cancelled)?,
            ScenarioBasis::LogicalBytesRate,
            transfer,
            &upload_pricing,
        )?;
    }

    // structural edits along the observed critical path; nameless nodes
    // are skipped (a "split 4x: " row with no name ranks nothing), and
    // same-named steps carry the node id so two rows cannot collide
    if cancelled() {
        return None;
    }
    let mut cp_nodes: Vec<u32> = dag.cp_path.clone();
    cp_nodes.sort_by_key(|&i| std::cmp::Reverse(dag.weight_us[i as usize]));
    let named: Vec<(u32, String)> = cp_nodes
        .iter()
        .filter_map(|&node| {
            if si.cache_hit[node as usize]
                || si.cache_ineligible[node as usize]
                || si.stages[node as usize]
                    .is_some_and(|s| s.has_any(Stages::EXECUTE) && s.execute_us == 0)
            {
                return None;
            }
            b.node_ev[node as usize]
                .map(|e| b.events[e].name.clone())
                .filter(|n| !n.is_empty())
                .map(|n| (node, n))
        })
        .take(3)
        .collect();
    for (node, name) in &named {
        if cancelled() {
            return None;
        }
        let mut w = si.base.clone();
        w[*node as usize] /= 4;
        let label = if named.iter().filter(|(_, n)| n == name).count() > 1 {
            format!("split 4x: {name} (node {node})")
        } else {
            format!("split 4x: {name}")
        };
        push(
            label,
            (w, 1),
            ScenarioBasis::StructuralHypothesis,
            None,
            "the action itself sharded 4 ways, repartition cost ignored",
        )?;
    }

    // shrink critical-path outliers to their category median
    let mut by_cat: FxHashMap<(u32, bool), Vec<i64>> = FxHashMap::default();
    for (i, c) in si.cat.iter().enumerate() {
        if i.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(c) = c {
            by_cat
                .entry((*c, si.cache_hit[i]))
                .or_default()
                .push(si.base[i]);
        }
    }
    for v in by_cat.values_mut() {
        if cancelled() {
            return None;
        }
        let mid = crate::analyses::percentile_index(v.len(), 50);
        v.select_nth_unstable(mid);
    }
    let mut w = si.base.clone();
    let mut n = 0usize;
    for (step, &node) in dag.cp_path.iter().enumerate() {
        if step.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let i = node as usize;
        let Some(c) = si.cat[i] else { continue };
        let Some(durs) = by_cat.get(&(c, si.cache_hit[i])) else {
            continue;
        };
        if durs.len() < PEER_MIN_N {
            continue;
        }
        let med = durs[crate::analyses::percentile_index(durs.len(), 50)];
        if si.base[i] > 2 * med {
            w[i] = med;
            n += 1;
        }
    }
    if n > 0 {
        push(
            format!("critical-path outliers shrunk to category median ({n})"),
            (w, n),
            ScenarioBasis::PeerDuration,
            None,
            "CP steps over 2x the median of their category and cache-hit/execution \
             population set to that median",
        )?;
    }

    if cancelled() {
        return None;
    }
    Some(WhatIfReport {
        base_cp_us: base_cp,
        t1_us: dag.t1,
        n_action_nodes: si.n_action_nodes,
        n_with_stages: si.n_with_stages,
        rows,
    })
}

// ------------------------------------------------------- cp-rank / node sims

/// `replay`, but also returning the simulated critical path (action
/// nodes only, build order — the same convention as `Dag::cp_path`).
pub fn replay_path(dag: &Dag, w: &[i64]) -> (i64, Vec<u32>) {
    replay_path_cancellable(dag, w, &|| false).expect("an uncancelled replay completes")
}

pub fn replay_path_cancellable(
    dag: &Dag,
    w: &[i64],
    cancelled: &impl Fn() -> bool,
) -> Option<(i64, Vec<u32>)> {
    if cancelled() {
        return None;
    }
    ReplayScratch::new(w.len()).path(dag, w, cancelled)
}

/// Reused across a cascade: the DAG's edges are borrowed, never copied,
/// and each replay overwrites these two O(V) buffers in place.
pub(crate) struct ReplayScratch {
    longest: Vec<i64>,
    best_pred: Vec<u32>,
}

impl ReplayScratch {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            longest: vec![0; n],
            best_pred: Vec::new(),
        }
    }

    pub(crate) fn replay(
        &mut self,
        dag: &Dag,
        w: &[i64],
        cancelled: &impl Fn() -> bool,
    ) -> Option<Replay> {
        let mut cp = 0;
        let mut t1 = 0i64;
        for (i, &weight) in w.iter().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                return None;
            }
            let mut parent = 0;
            for (k, &p) in dag.preds[dag.pred_off[i] as usize..dag.pred_off[i + 1] as usize]
                .iter()
                .enumerate()
            {
                if k > 0 && k.is_multiple_of(16384) && cancelled() {
                    return None;
                }
                parent = parent.max(self.longest[p as usize]);
            }
            self.longest[i] = weight.saturating_add(parent);
            cp = cp.max(self.longest[i]);
            t1 = t1.saturating_add(weight);
        }
        Some(Replay {
            cp_us: cp,
            t1_us: t1,
        })
    }

    pub(crate) fn savings_at(
        &mut self,
        dag: &Dag,
        w: &mut [i64],
        base_cp: i64,
        node: u32,
        scaled: i64,
        cancelled: &impl Fn() -> bool,
    ) -> Option<i64> {
        let i = node as usize;
        let old = w[i];
        w[i] = scaled;
        let r = self.replay(dag, w, cancelled);
        w[i] = old;
        r.map(|r| base_cp - r.cp_us)
    }

    fn path(
        &mut self,
        dag: &Dag,
        w: &[i64],
        cancelled: &impl Fn() -> bool,
    ) -> Option<(i64, Vec<u32>)> {
        self.best_pred.resize(w.len(), u32::MAX);
        let mut cp = 0i64;
        let mut end = None;
        for (i, &weight) in w.iter().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                return None;
            }
            let ps = &dag.preds[dag.pred_off[i] as usize..dag.pred_off[i + 1] as usize];
            let mut m = 0i64;
            let mut bp = u32::MAX;
            for (k, &j) in ps.iter().enumerate() {
                // A single join can have millions of predecessors.
                if k > 0 && k.is_multiple_of(16384) && cancelled() {
                    return None;
                }
                if self.longest[j as usize] > m {
                    m = self.longest[j as usize];
                    bp = j;
                }
            }
            self.longest[i] = weight.saturating_add(m);
            self.best_pred[i] = bp;
            if self.longest[i] > cp {
                cp = self.longest[i];
                end = Some(i as u32);
            }
        }
        let mut path = Vec::new();
        let mut cur = end;
        let mut visited = 0usize;
        while let Some(node) = cur {
            if visited.is_multiple_of(4096) && cancelled() {
                return None;
            }
            visited += 1;
            if dag.act_of_node[node as usize] != u32::MAX {
                path.push(node);
            }
            let bp = self.best_pred[node as usize];
            cur = (bp != u32::MAX).then_some(bp);
        }
        path.reverse();
        Some((cp, path))
    }
}

/// A bounded exploration, not an optimal set of actions to change.
pub const MAX_CASCADE_STEPS: usize = 20;

/// One sequential removal, scored against both its immediately preceding
/// scenario and the scenario that started the cascade. These are changes
/// in a dependency critical-path replay, NOT measured build-wall savings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadeStep {
    /// One-based removal number.
    pub index: usize,
    pub node: u32,
    /// Index into `Trace::acts`, not the node id or bundle event index.
    pub act: u32,
    /// This chosen node's weight BEFORE removal, including initial overrides.
    pub duration_us: i64,
    pub cp_before_us: i64,
    pub cp_after_us: i64,
    /// CP before this removal minus CP after it. May be zero: another
    /// equally long path can take over, and the cascade must continue.
    pub marginal_saved_us: i64,
    /// Starting scenario CP minus CP after this removal. Unlike the
    /// leave-one-out `marginal_at` values, sequential marginals telescope.
    pub cumulative_saved_us: i64,
    /// Measured-weight DAG CP minus CP after this removal; initial
    /// overrides are included, and a slower starting scenario may be negative.
    pub total_saved_us: i64,
    /// Changes to the selected representative CP, not the union of every
    /// equally critical path. No per-step path/weight snapshots are retained.
    pub path_changed: bool,
    pub entered_actions: usize,
    pub exited_actions: usize,
}

/// Greedily remove the longest positive-duration action on the CURRENT
/// replayed CP, replay, then choose again. This is neither the globally
/// longest actions nor the actions with the largest standalone savings.
///
/// The caller supplies fully composed current weights (including any
/// existing per-action or whole-graph overrides). Every node mapped to
/// the chosen action is set to zero, preserving all dependency edges.
/// Tied durations choose the lower action index, then lower node id;
/// tied paths retain `replay_path`'s stable predecessor/topological order.
///
/// Run on a worker. There is one initial replay and at most twenty further
/// O(V+E) passes, with one weight vector, reusable O(V) scratch, and only
/// the current path retained. Iteration emits compact progress rows; it
/// does NOT recompute all prior overrides' leave-one-out marginals.
pub struct GreedyCascade<'a> {
    dag: &'a Dag,
    w: Vec<i64>,
    scratch: ReplayScratch,
    path: Vec<u32>,
    cp_us: i64,
    start_cp_us: i64,
    limit: usize,
    completed: usize,
    finished: bool,
}

impl<'a> GreedyCascade<'a> {
    pub fn new(dag: &'a Dag, w: Vec<i64>, limit: usize) -> Self {
        Self::new_with_cancel(dag, w, limit, || false).expect("an uncancelled replay completes")
    }

    /// Cancellation is checked within long scans. A cancelled constructor
    /// returns no partial result. `w` must match the DAG's node order and
    /// contain nonnegative durations, as the existing simulation APIs do.
    pub fn new_with_cancel(
        dag: &'a Dag,
        w: Vec<i64>,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        assert_eq!(w.len(), dag.weight_us.len());
        if cancelled() {
            return None;
        }
        let mut scratch = ReplayScratch::new(w.len());
        let (cp_us, path) = scratch.path(dag, &w, &cancelled)?;
        Some(Self {
            dag,
            w,
            scratch,
            path,
            cp_us,
            start_cp_us: cp_us,
            limit: limit.min(MAX_CASCADE_STEPS),
            completed: 0,
            finished: false,
        })
    }

    pub fn base_cp_us(&self) -> i64 {
        self.dag.cp
    }

    pub fn start_cp_us(&self) -> i64 {
        self.start_cp_us
    }

    pub fn cp_us(&self) -> i64 {
        self.cp_us
    }

    /// Action-node ids of the current representative critical path.
    pub fn path(&self) -> &[u32] {
        &self.path
    }

    pub fn weights(&self) -> &[i64] {
        &self.w
    }

    pub fn into_weights(self) -> Vec<i64> {
        self.w
    }

    /// Like `next`, but cancellation terminates the iterator. An interrupted
    /// step restores its changed weights and emits no partial row; getters
    /// still describe the last completed scenario.
    pub fn next_with_cancel(&mut self, cancelled: impl Fn() -> bool) -> Option<CascadeStep> {
        if self.finished || self.completed >= self.limit {
            return None;
        }
        self.finished = true;
        let mut chosen = None;
        for (i, &node) in self.path.iter().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                return None;
            }
            let act = self.dag.act_of_node[node as usize];
            let weight = self.w[node as usize];
            if act == u32::MAX || weight <= 0 {
                continue;
            }
            // Maximum weight; minimum action/node id on ties.
            let key = (weight, std::cmp::Reverse(act), std::cmp::Reverse(node));
            if chosen.is_none_or(|old| key > old) {
                chosen = Some(key);
            }
        }
        let (duration_us, std::cmp::Reverse(act), std::cmp::Reverse(node)) = chosen?;
        let mut removed = Vec::new();
        for (n, &a) in self.dag.act_of_node.iter().enumerate() {
            if n.is_multiple_of(4096) && cancelled() {
                for &(n, old) in &removed {
                    self.w[n] = old;
                }
                return None;
            }
            if a == act && self.w[n] != 0 {
                removed.push((n, self.w[n]));
                self.w[n] = 0;
            }
        }
        let next = self.scratch.path(self.dag, &self.w, &cancelled);
        let changes = next
            .as_ref()
            .and_then(|(_, path)| self.path_changes(path, &cancelled));
        let (Some((cp_after_us, path)), Some((entered_actions, exited_actions))) = (next, changes)
        else {
            for &(n, old) in &removed {
                self.w[n] = old;
            }
            return None;
        };
        self.completed += 1;
        let step = CascadeStep {
            index: self.completed,
            node,
            act,
            duration_us,
            cp_before_us: self.cp_us,
            cp_after_us,
            marginal_saved_us: self.cp_us - cp_after_us,
            cumulative_saved_us: self.start_cp_us - cp_after_us,
            total_saved_us: self.dag.cp - cp_after_us,
            path_changed: self.path != path,
            entered_actions,
            exited_actions,
        };
        self.cp_us = cp_after_us;
        self.path = path;
        self.finished = false;
        Some(step)
    }

    fn path_changes(&self, next: &[u32], cancelled: &impl Fn() -> bool) -> Option<(usize, usize)> {
        let mut before = FxHashSet::default();
        for (i, &node) in self.path.iter().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                return None;
            }
            before.insert(self.dag.act_of_node[node as usize]);
        }
        let mut after = FxHashSet::default();
        for (i, &node) in next.iter().enumerate() {
            if i.is_multiple_of(4096) && cancelled() {
                return None;
            }
            after.insert(self.dag.act_of_node[node as usize]);
        }
        Some((
            after.difference(&before).count(),
            before.difference(&after).count(),
        ))
    }
}

impl Iterator for GreedyCascade<'_> {
    type Item = CascadeStep;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_with_cancel(|| false)
    }
}

/// Simulated-baseline CP minus the CP with `node`'s weight set to
/// `scaled`. Mutates `w` in place and restores it (per-node loops over
/// a multi-million-node DAG must not clone the weight vector each
/// step). Sim-vs-sim: `base_cp` must be `replay(dag, w).cp_us`, never
/// the measured wall.
pub fn savings_at(dag: &Dag, w: &mut [i64], base_cp: i64, node: u32, scaled: i64) -> i64 {
    savings_at_cancellable(dag, w, base_cp, node, scaled, &|| false)
        .expect("an uncancelled replay completes")
}

pub fn savings_at_cancellable(
    dag: &Dag,
    w: &mut [i64],
    base_cp: i64,
    node: u32,
    scaled: i64,
    cancelled: &impl Fn() -> bool,
) -> Option<i64> {
    if cancelled() {
        return None;
    }
    ReplayScratch::new(w.len()).savings_at(dag, w, base_cp, node, scaled, cancelled)
}

/// What one override in a stacked scenario is still buying, given all
/// the others: the CP with `group` reverted, minus the scenario CP.
/// `group` is the `(node, base_us)` pairs that override edited, so
/// reverting restores the weights the rest of the scenario stands on.
/// Mutates `w` in place and restores it, as [`savings_at`] does.
///
/// Marginal rather than standalone because overlapping steps share
/// their savings: two overrides on one chain each worth 66s alone are
/// not worth 132s together, so standalone numbers do not add up to the
/// scenario total. Zero means reverting this override would not lengthen
/// the scenario CP. Path membership alone cannot decide that: reverting
/// an override on a currently off-path action may make its branch critical.
pub fn marginal_at(dag: &Dag, w: &mut [i64], cp: i64, group: &[(u32, i64)]) -> i64 {
    marginal_at_cancellable(dag, w, cp, group, &|| false).expect("an uncancelled replay completes")
}

pub fn marginal_at_cancellable(
    dag: &Dag,
    w: &mut [i64],
    cp: i64,
    group: &[(u32, i64)],
    cancelled: &impl Fn() -> bool,
) -> Option<i64> {
    let held: Vec<i64> = group.iter().map(|&(n, _)| w[n as usize]).collect();
    for &(n, base) in group {
        w[n as usize] = base;
    }
    let r = replay_cancellable(dag, w, cancelled);
    for (&(n, _), &old) in group.iter().zip(&held) {
        w[n as usize] = old;
    }
    r.map(|r| r.cp_us - cp)
}

/// The [`marginal_at`] group for an override that rewrote every weight
/// rather than one action's: the nodes whose scenario weight `w` still
/// differs from `measured`, minus the ones `pinned` by a per-step
/// override. Those belong to their own row, and reverting them here
/// would bill their savings to both.
pub fn whole_graph_group(measured: &[i64], w: &[i64], pinned: &[(u32, i64)]) -> Vec<(u32, i64)> {
    let pinned: FxHashSet<u32> = pinned.iter().map(|&(n, _)| n).collect();
    (0..w.len() as u32)
        .filter(|n| !pinned.contains(n))
        .filter(|&n| w[n as usize] != measured[n as usize])
        .map(|n| (n, measured[n as usize]))
        .collect()
}

#[derive(Clone, Debug)]
pub struct CpRankRow {
    pub node: u32,
    /// `Trace::acts` index
    pub act: u32,
    pub dur_us: i64,
    /// simulated-baseline CP minus CP with this node removed (dur 0;
    /// node and edges stay, so ordering survives)
    pub save_elim_us: i64,
    /// same at 50% of the node's duration
    pub save_half_us: i64,
}

pub struct CpRankReport {
    pub base_cp_us: i64,
    pub cp_steps: usize,
    /// every step of the observed critical path, sorted by
    /// `save_elim_us` descending
    pub rows: Vec<CpRankRow>,
}

/// True per-step sensitivity of the critical path: for each CP step,
/// the wall the dependency floor sheds if that step were free (or
/// halved). Share-of-CP is NOT savings — removing a step only helps
/// until the longest path that avoids it takes over, and that shadow
/// path cannot be read off any duration table. Full recompute per step
/// costs O(|CP| * (V+E)); scratch is reused between replays.
pub fn cp_rank(b: &DagBundle) -> CpRankReport {
    cp_rank_topk(b, usize::MAX)
}

/// Elimination savings determine the rank; only retained rows need a
/// second replay for their halved duration.
pub fn cp_rank_topk(b: &DagBundle, topk: usize) -> CpRankReport {
    let dag = &b.dag;
    let mut w = dag.weight_us.clone();
    let base_cp = dag.cp;
    let mut scratch = ReplayScratch::new(w.len());
    let mut rows: Vec<CpRankRow> = dag
        .cp_path
        .iter()
        .map(|&node| {
            let i = node as usize;
            CpRankRow {
                node,
                act: dag.act_of_node[i],
                dur_us: dag.weight_us[i],
                save_elim_us: scratch
                    .savings_at(dag, &mut w, base_cp, node, 0, &|| false)
                    .expect("an uncancelled replay completes"),
                save_half_us: 0,
            }
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.save_elim_us));
    rows.truncate(topk);
    for row in &mut rows {
        row.save_half_us = scratch
            .savings_at(dag, &mut w, base_cp, row.node, row.dur_us / 2, &|| false)
            .expect("an uncancelled replay completes");
    }
    CpRankReport {
        base_cp_us: base_cp,
        cp_steps: dag.cp_path.len(),
        rows,
    }
}

#[derive(Clone, Debug)]
pub struct NodeSim {
    pub label: String,
    pub base_cp_us: i64,
    pub cp_us: i64,
    /// the simulated critical path (action nodes, build order)
    pub path: Vec<u32>,
}

/// `edits` applied to `w` in place, then one path recompute. `w` is the
/// caller's buffer so it can read the weights the result was drawn
/// with; a scenario that rewrites every node hands its own vector in
/// here rather than the measured one.
///
/// The score is always against `Dag::cp`, the *measured* critical path,
/// so stacking a second override cannot re-baseline the first.
pub fn simulate_edits_from(
    b: &DagBundle,
    w: &mut [i64],
    edits: &[(u32, i64)],
    label: &str,
) -> NodeSim {
    for &(node, w_node) in edits {
        w[node as usize] = w_node.max(0);
    }
    let (cp, path) = replay_path(&b.dag, w);
    NodeSim {
        label: label.to_owned(),
        base_cp_us: b.dag.cp,
        cp_us: cp,
        path,
    }
}

/// Per-step overrides on the measured weights.
pub fn simulate_edits(b: &DagBundle, edits: &[(u32, i64)], label: &str) -> NodeSim {
    let mut w = b.dag.weight_us.clone();
    simulate_edits_from(b, &mut w, edits, label)
}

/// One node's weight replaced by `w_node`, full path recompute. The
/// scale menu and `--simulate` both land here.
pub fn simulate_node(b: &DagBundle, node: u32, w_node: i64, label: &str) -> NodeSim {
    simulate_edits(b, &[(node, w_node)], label)
}

/// One stacked scenario: `base` is the weight vector every per-action
/// override is applied to (the measured weights, or a whole-graph
/// transform of them such as free admission). `groups[k]` is override
/// `k`'s `(node, base_us)` pairs in the shape [`marginal_at`] reverts,
/// so a row's marginal restores the weights the rest of the scenario
/// stands on, never the measured ones. Building the groups and applying
/// the edits from one vector is what keeps the two from disagreeing.
pub struct Stacked {
    pub sim: NodeSim,
    /// the weights `sim` was drawn with
    pub w: Vec<i64>,
    pub groups: Vec<Vec<(u32, i64)>>,
    pub edits: Vec<(u32, i64)>,
}

/// `overrides` is `(act index, weight)`. `build_full` creates one node per
/// action; defensive support for hand-built DAGs moves all associated nodes.
pub fn stack_overrides(
    b: &DagBundle,
    base: Vec<i64>,
    overrides: &[(usize, i64)],
    label: &str,
) -> Stacked {
    stack_overrides_cancellable(b, base, overrides, label, &|| false)
        .expect("an uncancelled replay completes")
}

pub fn stack_overrides_cancellable(
    b: &DagBundle,
    mut base: Vec<i64>,
    overrides: &[(usize, i64)],
    label: &str,
    cancelled: &impl Fn() -> bool,
) -> Option<Stacked> {
    let mut groups: Vec<Vec<(u32, i64)>> = vec![Vec::new(); overrides.len()];
    for (n, &a) in b.dag.act_of_node.iter().enumerate() {
        if n.is_multiple_of(4096) && cancelled() {
            return None;
        }
        if let Some(i) = overrides.iter().position(|&(act, _)| act as u32 == a) {
            groups[i].push((n as u32, base[n]));
        }
    }
    let edits: Vec<(u32, i64)> = groups
        .iter()
        .zip(overrides)
        .flat_map(|(g, &(_, w))| g.iter().map(move |&(n, _)| (n, w)))
        .collect();
    for &(node, weight) in &edits {
        base[node as usize] = weight.max(0);
    }
    let (cp_us, path) = replay_path_cancellable(&b.dag, &base, cancelled)?;
    let sim = NodeSim {
        label: label.to_owned(),
        base_cp_us: b.dag.cp,
        cp_us,
        path,
    };
    Some(Stacked {
        sim,
        w: base,
        groups,
        edits,
    })
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    #[test]
    fn each_counter_scenario_preserves_missing_versus_measured_zero() {
        let (b, mut si) = bundle();
        si.stages.fill(None);
        si.admission_recorded.fill(false);
        let node = b.dag.cp_path[0] as usize;
        let cases = [
            (Stages::MATERIALIZE, "input preparation counters removed"),
            (Stages::INPUT_UPLOAD, "input uploads eliminated"),
            (
                Stages::OUTPUT_RESULT,
                "output and legacy result stages removed",
            ),
            (Stages::CACHE_QUERY, "cache lookups eliminated"),
            (Stages::DERIVED, "derived client residual removed"),
        ];
        // Execution alone says nothing about unrelated counters, even zero costs.
        si.stages[node] = Some(Stages {
            measured_mask: Stages::EXECUTE,
            ..Default::default()
        });
        let report = standard_whatifs(&b, &si);
        for &(mask, label) in &cases {
            let row = report.rows.iter().find(|r| r.label == label).unwrap();
            let fields = mask.count_ones() as usize;
            assert_eq!(
                row.coverage,
                Some((0, fields * si.n_action_nodes)),
                "{label}"
            );
            assert!(!row.is_priced(), "{label}");
        }
        for &(mask, label) in &cases {
            si.stages[node] = Some(Stages {
                measured_mask: mask,
                ..Default::default()
            });
            let report = standard_whatifs(&b, &si);
            let row = report.rows.iter().find(|r| r.label == label).unwrap();
            let fields = mask.count_ones() as usize;
            assert_eq!(
                row.coverage,
                Some((fields, fields * si.n_action_nodes)),
                "{label}"
            );
            assert!(row.is_priced(), "{label}");
            assert_eq!((row.saved_us, row.n_modified), (0, 0), "{label}");
        }
    }

    #[test]
    fn compound_coverage_counts_only_present_counter_observations() {
        let (b, mut si) = bundle();
        si.stages.fill(None);
        let node = b.dag.cp_path[0] as usize;
        si.stages[node] = Some(Stages {
            measured_mask: Stages::UPLOAD,
            ..Default::default()
        });
        let report = standard_whatifs(&b, &si);
        let row = report
            .rows
            .iter()
            .find(|r| r.label == "output and legacy result stages removed")
            .unwrap();
        assert_eq!(row.coverage, Some((1, 3 * si.n_action_nodes)));
        assert_eq!((row.saved_us, row.n_modified), (0, 0));
        assert!(row.note.contains("counter observations"));
    }

    #[test]
    fn split_candidates_exclude_known_non_commands_not_unknown_execution() {
        let (b, mut si) = bundle();
        si.cache_hit.fill(false);
        si.cache_ineligible.fill(true);
        si.stages.fill(None);
        let no_candidates = standard_whatifs(&b, &si);
        assert!(
            !no_candidates
                .rows
                .iter()
                .any(|r| r.label.starts_with("split 4x:"))
        );
        let node = b.dag.cp_path[0] as usize;
        si.cache_ineligible[node] = false;
        // A partial stage record with no execution counter is still unknown.
        si.stages[node] = Some(Stages {
            measured_mask: Stages::CACHE_QUERY,
            ..Default::default()
        });
        assert!(
            standard_whatifs(&b, &si)
                .rows
                .iter()
                .any(|r| r.label.starts_with("split 4x:"))
        );
        si.stages[node].as_mut().unwrap().measured_mask |= Stages::EXECUTE;
        assert!(
            !standard_whatifs(&b, &si)
                .rows
                .iter()
                .any(|r| r.label.starts_with("split 4x:"))
        );
        let extras = Extras {
            args: vec![
                crate::analyses::EvArgs {
                    cache_ineligible: true,
                    ..Default::default()
                };
                4
            ],
            ..Default::default()
        };
        let actual = sim_input(&b, &extras);
        for (node, &act) in b.dag.act_of_node.iter().enumerate() {
            assert_eq!(actual.cache_ineligible[node], act != u32::MAX);
        }
    }

    use super::*;
    use crate::analyses::load_extras;
    use crate::graph::load_graph;
    use crate::parse;
    use crate::types::Trace;

    #[test]
    fn measured_zero_upload_wait_still_has_a_scenario() {
        let (b, mut si) = bundle();
        si.iu_sem_wait.fill(0);
        si.iu_cas_call.fill(None);
        si.iu_invalid.fill(0);
        si.iu_cas_call[0] = Some(0);
        let report = standard_whatifs(&b, &si);
        let row = report
            .rows
            .iter()
            .find(|r| r.label == "input-upload permit wait eliminated")
            .unwrap();
        assert_eq!(row.n_modified, 0);
        assert_eq!(row.saved_us, 0);
        assert_eq!(row.coverage, Some((1, si.n_action_nodes)));
        si.iu_invalid[0] = UploadSub::SEM_WAIT;
        assert!(
            !standard_whatifs(&b, &si)
                .rows
                .iter()
                .any(|r| r.label == "input-upload permit wait eliminated")
        );
        si.iu_cas_call[0] = None;
        si.iu_invalid[0] = UploadSub::CAS_CALL;
        assert!(
            standard_whatifs(&b, &si)
                .rows
                .iter()
                .any(|r| r.label == "input-upload permit wait eliminated")
        );
        si.iu_invalid.fill(0);
        si.iu_cas_call.fill(None);
        assert!(
            !standard_whatifs(&b, &si)
                .rows
                .iter()
                .any(|r| r.label == "input-upload permit wait eliminated")
        );
    }

    #[test]
    fn negative_edits_are_zero_and_missing_extras_are_unknown() {
        let (b, _) = bundle();
        let empty = Extras::default();
        let si = sim_input(&b, &empty);
        assert_eq!(si.n_with_stages, 0);
        assert!(si.iu_cas_call.iter().all(Option::is_none));
        let node = b.dag.cp_path[0];
        let negative = simulate_edits(&b, &[(node, -100)], "negative");
        let zero = simulate_edits(&b, &[(node, 0)], "zero");
        assert_eq!(negative.cp_us, zero.cp_us);
        assert_eq!(negative.path, zero.path);
    }

    #[test]
    fn standard_report_cancels_without_publishing_partial_scenarios() {
        let (b, si) = bundle();
        let original = si.base.clone();
        let checks = std::cell::Cell::new(0usize);
        let report = standard_whatifs_cancellable(&b, &si, &|| {
            checks.set(checks.get() + 1);
            false
        })
        .unwrap();
        let total_checks = checks.get();
        assert!(
            total_checks > 20,
            "checks reach the individual scenarios and replays"
        );
        for stop_at in [1, 2, total_checks / 2, total_checks] {
            checks.set(0);
            assert!(
                standard_whatifs_cancellable(&b, &si, &|| {
                    checks.set(checks.get() + 1);
                    checks.get() >= stop_at
                })
                .is_none()
            );
            assert_eq!(checks.get(), stop_at);
            assert_eq!(si.base, original);
        }
        let resumed = standard_whatifs(&b, &si);
        assert_eq!(resumed.rows.len(), report.rows.len());
        for (a, b) in resumed.rows.iter().zip(&report.rows) {
            assert_eq!(
                (&a.label, a.cp_us, a.saved_us, a.n_modified, a.coverage),
                (&b.label, b.cp_us, b.saved_us, b.n_modified, b.coverage)
            );
        }
    }

    #[test]
    fn weight_scan_checks_cancellation_within_a_large_scenario() {
        let (_, mut si) = bundle();
        si.base.resize(16_384, 1);
        let visited = std::cell::Cell::new(0usize);
        assert!(
            weights_cancellable(&si, &|| visited.get() >= 4096, |i| {
                visited.set(visited.get() + 1);
                si.base[i]
            })
            .is_none()
        );
        assert_eq!(
            visited.get(),
            4096,
            "the rest of the node scan was not performed"
        );
    }

    #[test]
    fn counter_removal_preserves_execution_and_unclassified_residual() {
        let (_, mut si) = bundle();
        let node = si.stages.iter().position(Option::is_some).unwrap();
        si.base[node] = 1000;
        si.stages[node] = Some(Stages {
            execute_us: 800,
            materialize_us: 900,
            ..Default::default()
        });
        si.bytes_in[node] = 100;
        assert_eq!(w_zero_stage(&si, |s| s.materialize_us).0[node], 800);
        assert_eq!(w_execute_only(&si).0[node], 800);
        assert_eq!(w_bandwidth(&si, i64::MAX).0[node], 800);
        si.stages[node] = Some(Stages {
            execute_us: 100,
            daemon_us: 900,
            ..Default::default()
        });
        assert_eq!(
            w_execute_only(&si).0[node],
            1000,
            "derived residual is not a recorded stage"
        );
    }

    #[test]
    fn local_finalization_and_cas_are_non_execution_without_double_counting_hashing() {
        let (_, mut si) = bundle();
        let node = si.stages.iter().position(Option::is_some).unwrap();
        si.base[node] = 100;
        si.submit[node] = 0;
        si.stages[node] = Some(Stages {
            execute_us: 20,
            cas_artifact_us: 10,
            local_finalize_outputs_us: 50,
            hashing_us: i64::MAX,
            hashing_in_finalize_outputs: true,
            ..Default::default()
        });
        assert_eq!(non_execution_weight(&si, node), 40);
        assert_eq!(w_execute_only(&si).0[node], 40);
        assert_eq!(
            si.base[node], 100,
            "classification does not mutate action duration"
        );
    }

    #[test]
    fn local_cache_check_is_measured_non_execution_not_residual() {
        let (_, mut si) = bundle();
        let node = si.stages.iter().position(Option::is_some).unwrap();
        si.base[node] = 100;
        si.submit[node] = 0;
        si.stages[node] = Some(Stages {
            local_cache_check_us: 90,
            ..Default::default()
        });
        assert_eq!(non_execution_weight(&si, node), 10);
        assert_eq!(w_execute_only(&si).0[node], 10);
        assert_eq!(si.base[node], 100);
    }

    #[test]
    fn current_cached_result_and_preparation_are_recorded_non_execution_costs() {
        let (_, mut si) = bundle();
        let node = si.stages.iter().position(Option::is_some).unwrap();
        si.base[node] = 100;
        si.submit[node] = 0;
        si.stages[node] = Some(Stages {
            cache_query_us: 30,
            cache_hit_us: 60,
            prepare_us: 5,
            daemon_us: 5,
            ..Default::default()
        });
        assert_eq!(non_execution_weight(&si, node), 5);
        assert_eq!(
            w_zero_stage(&si, |s| s.cache_query_us).0[node],
            70,
            "lookup-only intervention does not remove cached-result handling"
        );
        assert_eq!(
            si.base[node], 100,
            "classification never mutates recorded duration"
        );
    }

    #[test]
    fn input_and_output_scenarios_do_not_reassign_legacy_result_time() {
        let (b, mut si) = bundle();
        let node = group_of(&b, "a (x)")[0].0 as usize;
        si.stages.fill(None);
        si.stages[node] = Some(Stages {
            input_upload_us: 20,
            upload_us: 30,
            result_wait_us: 10,
            download_us: 5,
            execute_us: 5,
            ..Default::default()
        });
        si.n_with_stages = 1;
        let report = standard_whatifs(&b, &si);
        let input = report
            .rows
            .iter()
            .find(|r| r.label == "input uploads eliminated")
            .unwrap();
        assert_eq!(input.saved_us, 20);
        let output = report
            .rows
            .iter()
            .find(|r| r.label == "output and legacy result stages removed")
            .unwrap();
        assert_eq!(output.saved_us, 45);
        assert!(output.note.contains("not a client-processing wait"));
        assert!(output.note.contains("legacy upload_us may mix"));

        // Counter sums are conditional cuts, never wrapping negative or
        // erasing the retained current-execution floor on extreme input.
        let stage = si.stages[node].as_mut().unwrap();
        stage.upload_us = i64::MAX;
        stage.result_wait_us = i64::MAX;
        stage.download_us = i64::MAX;
        let (weights, _) = w_zero_stage(&si, |s| {
            s.upload_us
                .saturating_add(s.result_wait_us)
                .saturating_add(s.download_us)
        });
        assert_eq!(weights[node], 5);
    }

    #[test]
    fn logical_rate_coverage_includes_output_only_uploads() {
        let (b, mut si) = bundle();
        let node = group_of(&b, "a (x)")[0].0 as usize;
        si.stages.fill(None);
        si.stages[node] = Some(Stages {
            upload_us: 30,
            execute_us: 5,
            ..Default::default()
        });
        si.n_with_stages = 1;
        for (bytes, expected) in [(10, (1, 1)), (0, (0, 1))] {
            si.bytes_out[node] = bytes;
            let report = standard_whatifs(&b, &si);
            for row in report
                .rows
                .iter()
                .filter(|r| r.basis == ScenarioBasis::LogicalBytesRate)
            {
                assert_eq!(row.coverage, Some(expected));
                assert_eq!(row.is_priced(), bytes > 0);
                assert!(
                    row.note
                        .contains("output upload uses declared logical output")
                );
            }
        }
    }

    #[test]
    fn admission_presence_preserves_measured_zero_and_hits_are_not_shard_candidates() {
        let (b, mut si) = bundle();
        assert_eq!(admission_coverage(&si), (0, 4));
        let node = b.dag.cp_path[0] as usize;
        si.admission_recorded[node] = true;
        assert_eq!(admission_coverage(&si), (1, 4));
        si.cache_hit.fill(true);
        let report = standard_whatifs(&b, &si);
        assert!(!report.rows.iter().any(|r| r.label.starts_with("split 4x:")));
        let admission = report
            .rows
            .iter()
            .find(|r| r.label == "execution admission waits removed")
            .unwrap();
        assert_eq!((admission.n_modified, admission.saved_us), (0, 0));
    }

    #[test]
    fn ranked_prefix_matches_full_rank_and_cancelled_replays_restore_weights() {
        let (b, _) = bundle();
        let full = cp_rank(&b);
        let short = cp_rank_topk(&b, 1);
        assert_eq!(short.cp_steps, full.cp_steps);
        assert_eq!(short.rows.len(), 1);
        assert_eq!(
            (short.rows[0].node, short.rows[0].save_half_us),
            (full.rows[0].node, full.rows[0].save_half_us)
        );
        let mut weights = b.dag.weight_us.clone();
        let node = b.dag.cp_path[0];
        assert!(
            savings_at_cancellable(&b.dag, &mut weights, b.dag.cp, node, 0, &|| true).is_none()
        );
        assert_eq!(weights, b.dag.weight_us);
        assert!(
            marginal_at_cancellable(&b.dag, &mut weights, b.dag.cp, &[(node, 0)], &|| true)
                .is_none()
        );
        assert_eq!(weights, b.dag.weight_us);
        assert!(stack_overrides_cancellable(&b, weights, &[], "cancelled", &|| true).is_none());
    }

    #[test]
    fn category_median_does_not_price_the_only_execution_as_a_cache_hit() {
        let trace_path = crate::temp_path("sim-mixed-cache-trace.json");
        let graph_path = crate::temp_path("sim-mixed-cache-graph.jsonl");
        let mut trace = String::from("[\n");
        let mut graph = String::from("{\"graph\":{\"enc\":2}}\n");
        for i in 0..6 {
            let args = if i < 5 {
                serde_json::json!({"src":"compile", "cache":{"lookup_us":10}})
            } else {
                serde_json::json!({"src":"compile", "kind":"remote", "stages":{"execute_us":1000}})
            };
            trace.push_str(&format!(
                "{},\n",
                serde_json::json!({
                    "ph":"X", "cat":"recipe", "name":format!("action{i}"), "ts":0,
                    "dur":if i < 5 {10} else {1000}, "args":args,
                })
            ));
            graph.push_str(&format!(
                "{}\n",
                serde_json::json!({"t":format!("action{i}"), "d":[]})
            ));
        }
        std::fs::write(&trace_path, trace).unwrap();
        std::fs::write(&graph_path, graph).unwrap();
        let tr = parse::parse(trace_path.to_str().unwrap()).unwrap();
        let graph = load_graph(graph_path.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        let (b, _) = crate::dag::build_full(&tr, &graph, None).unwrap();
        let si = sim_input(&b, &x);
        let report = standard_whatifs(&b, &si);
        assert!(
            !report
                .rows
                .iter()
                .any(|r| r.label.contains("category median"))
        );
        std::fs::remove_file(trace_path).unwrap();
        std::fs::remove_file(graph_path).unwrap();
    }

    /// diamond: a -> b, a -> c, {b,c} -> d; b carries a queue stage,
    /// c carries a fetch stage with bytes.
    fn diamond() -> (Trace, crate::graph::Graph) {
        // unique per call: tests run in parallel threads of one process
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-sim-{}-{seq}-trace.json",
            std::process::id()
        ));
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a (x)","ts":0,"dur":100,"args":{"src":"x","run":"r","stages":{"execute_us":100}}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"b (x)","ts":100,"dur":1000,"args":{"src":"x","run":"r","stages":{"queue_us":600,"execute_us":400}}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"c (x)","ts":100,"dur":900,"args":{"src":"x","run":"r","bytes_in":9000000,"stages":{"materialize_us":800,"execute_us":100}}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"d (x)","ts":1100,"dur":50,"args":{"src":"x","run":"r","stages":{"execute_us":50}}},"#,
                "\n"
            ),
        )
        .unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!(
            "retrace-sim-{}-{seq}-graph.json",
            std::process::id()
        ));
        std::fs::write(
            &gp,
            concat!(
                "{\"graph\":{\"enc\":2}}\n",
                "{\"t\":\"a (x)\",\"d\":[]}\n",
                "{\"t\":\"b (x)\",\"d\":[\"a (x)\"]}\n",
                "{\"t\":\"c (x)\",\"d\":[\"a (x)\"]}\n",
                "{\"t\":\"d (x)\",\"d\":[\"b (x)\",\"c (x)\"]}\n",
            ),
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        std::fs::remove_file(&gp).ok();
        (tr, g)
    }

    fn bundle() -> (DagBundle, SimInput) {
        let (tr, g) = diamond();
        // extras re-read tr.path, so the trace file must outlive this
        let x = load_extras(&tr);
        std::fs::remove_file(&tr.path).ok();
        let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
        let si = sim_input(&b, &x);
        (b, si)
    }

    /// Minimal topological DAG for replay tests. Reverse edges and slack
    /// arrays are unused by replay; the measured CP is independently
    /// computed with the scalar-only `replay` implementation.
    fn replay_dag(w: &[i64], ps: &[&[u32]], acts: &[u32]) -> Dag {
        assert_eq!(w.len(), ps.len());
        assert_eq!(w.len(), acts.len());
        let mut pred_off = vec![0];
        let mut preds = Vec::new();
        for (i, p) in ps.iter().enumerate() {
            assert!(p.iter().all(|&n| (n as usize) < i));
            preds.extend_from_slice(p);
            pred_off.push(preds.len() as u32);
        }
        let mut dag = Dag {
            act_of_node: acts.to_vec(),
            weight_us: w.to_vec(),
            pred_off,
            preds,
            succ_off: vec![0; w.len() + 1],
            succs: Vec::new(),
            t1: w.iter().sum(),
            cp: 0,
            up: Vec::new(),
            down: Vec::new(),
            cp_path: Vec::new(),
        };
        dag.cp = replay(&dag, w).cp_us;
        dag
    }

    #[test]
    fn cascade_follows_each_new_path_not_the_original_longest_list() {
        let (b, _) = bundle();
        let mut cascade = GreedyCascade::new(&b.dag, b.dag.weight_us.clone(), 20);
        let mut steps = Vec::new();
        while let Some(step) = cascade.next() {
            assert_eq!(step.cp_after_us, replay(&b.dag, cascade.weights()).cp_us);
            assert_eq!(
                cascade
                    .path()
                    .iter()
                    .map(|&n| cascade.weights()[n as usize])
                    .sum::<i64>(),
                step.cp_after_us,
            );
            assert!(step.cp_after_us <= step.cp_before_us);
            steps.push(step);
        }
        let names: Vec<&str> = steps
            .iter()
            .map(|s| b.events[b.node_ev[s.node as usize].unwrap()].name.as_str())
            .collect();
        assert_eq!(names, ["b (x)", "c (x)", "a (x)", "d (x)"]);
        assert_eq!(
            steps.iter().map(|s| s.cp_after_us).collect::<Vec<_>>(),
            [1050, 150, 50, 0]
        );
        assert_eq!(
            steps
                .iter()
                .map(|s| s.marginal_saved_us)
                .collect::<Vec<_>>(),
            [100, 900, 100, 50]
        );
        assert_eq!(steps[0].duration_us, 1000);
        assert_eq!(steps[0].entered_actions, 1);
        assert_eq!(steps[0].exited_actions, 1);
        assert!(steps[0].path_changed);
        assert_eq!(steps.last().unwrap().cumulative_saved_us, 1150);
        assert_eq!(steps.iter().map(|s| s.marginal_saved_us).sum::<i64>(), 1150);
        assert!(cascade.path().is_empty());
        assert!(cascade.next().is_none());
        assert_eq!(b.dag.cp, 1150, "the measured DAG was not mutated");
    }

    #[test]
    fn cascade_continues_after_zero_gain_on_a_tied_parallel_path() {
        let dag = replay_dag(&[10, 10, 0], &[&[], &[], &[1, 0]], &[99, 7, u32::MAX]);
        let steps: Vec<_> = GreedyCascade::new(&dag, dag.weight_us.clone(), 20).collect();
        // The first topological maximum is the representative CP, even
        // though another tied path has a lower action id. Only the
        // current path is eligible; the second path is discovered next.
        assert_eq!(steps.iter().map(|s| s.act).collect::<Vec<_>>(), [99, 7]);
        assert_eq!(
            steps
                .iter()
                .map(|s| s.marginal_saved_us)
                .collect::<Vec<_>>(),
            [0, 10]
        );
        assert_eq!(steps[0].cp_before_us, steps[0].cp_after_us);
        assert_eq!(steps[0].entered_actions, 1);
        assert_eq!(steps[0].exited_actions, 1);
        assert_eq!(steps[1].cumulative_saved_us, 10);
    }

    #[test]
    fn cascade_selects_longest_duration_not_largest_marginal_saving() {
        let dag = replay_dag(
            &[200, 1000, 999, 50],
            &[&[], &[0], &[0], &[1, 2]],
            &[0, 1, 2, 3],
        );
        let mut weights = dag.weight_us.clone();
        assert_eq!(savings_at(&dag, &mut weights, dag.cp, 0, 0), 200);
        assert_eq!(savings_at(&dag, &mut weights, dag.cp, 1, 0), 1);
        let mut cascade = GreedyCascade::new(&dag, weights, 20);
        let first = cascade.next().unwrap();
        assert_eq!(
            (first.node, first.duration_us, first.marginal_saved_us),
            (1, 1000, 1)
        );
        let second = cascade.next().unwrap();
        assert_eq!(
            (second.node, second.duration_us, second.marginal_saved_us),
            (2, 999, 999)
        );
    }

    #[test]
    fn cascade_selects_from_the_serial_cp_not_the_globally_longest_actions() {
        let dag = replay_dag(&[600, 600, 1000], &[&[], &[0], &[]], &[0, 1, 2]);
        let steps: Vec<_> = GreedyCascade::new(&dag, dag.weight_us.clone(), 20).collect();
        assert_eq!(steps.iter().map(|s| s.act).collect::<Vec<_>>(), [0, 2, 1]);
        assert_eq!(
            steps.iter().map(|s| s.duration_us).collect::<Vec<_>>(),
            [600, 1000, 600]
        );
        assert_eq!(
            steps.iter().map(|s| s.cp_after_us).collect::<Vec<_>>(),
            [1000, 600, 0]
        );
    }

    #[test]
    fn cascade_ties_use_action_ids_and_never_select_zero_or_join_nodes() {
        let dag = replay_dag(
            &[5, 5, 0, 0],
            &[&[], &[0], &[1], &[2]],
            &[9, 3, u32::MAX, 1],
        );
        let first: Vec<_> = GreedyCascade::new(&dag, dag.weight_us.clone(), 20).collect();
        let again: Vec<_> = GreedyCascade::new(&dag, dag.weight_us.clone(), 20).collect();
        assert_eq!(first, again);
        assert_eq!(first.iter().map(|s| s.act).collect::<Vec<_>>(), [3, 9]);
        assert!(first.iter().all(|s| s.duration_us > 0));
        assert_eq!(first.last().unwrap().cp_after_us, 0);
    }

    #[test]
    fn cascade_starts_from_composed_overrides_and_keeps_both_baselines() {
        let (b, _) = bundle();
        let gb = group_of(&b, "b (x)");
        let act_b = b.dag.act_of_node[gb[0].0 as usize] as usize;
        let half: Vec<i64> = b.dag.weight_us.iter().map(|w| w / 2).collect();
        let sc = stack_overrides(&b, half, &[(act_b, 0)], "half + b removed");
        let mut cascade = GreedyCascade::new(&b.dag, sc.w, 20);
        assert_eq!(cascade.base_cp_us(), 1150);
        assert_eq!(cascade.start_cp_us(), 525);
        let first = cascade.next().unwrap();
        assert_eq!(
            b.events[b.node_ev[first.node as usize].unwrap()].name,
            "c (x)"
        );
        assert_eq!(
            first.duration_us, 450,
            "uses the current, not recorded, duration"
        );
        assert_eq!((first.cp_before_us, first.cp_after_us), (525, 75));
        assert_eq!(first.marginal_saved_us, 450);
        assert_eq!(first.cumulative_saved_us, 450);
        assert_eq!(
            first.total_saved_us, 1075,
            "includes initial overrides only in this total"
        );
        let rest: Vec<_> = cascade.collect();
        assert!(rest.iter().all(|s| s.act as usize != act_b));
        assert_eq!(rest.last().unwrap().cumulative_saved_us, 525);
        assert_eq!(rest.last().unwrap().total_saved_us, 1150);
    }

    #[test]
    fn cascade_preserves_negative_total_savings_for_a_slower_start() {
        let dag = replay_dag(&[5, 5, 5], &[&[], &[0], &[1]], &[0, 1, 2]);
        let mut cascade = GreedyCascade::new(&dag, vec![50, 50, 50], 1);
        let step = cascade.next().unwrap();
        assert_eq!((cascade.base_cp_us(), cascade.start_cp_us()), (15, 150));
        assert_eq!(step.cp_after_us, 100);
        assert_eq!(step.cumulative_saved_us, 50);
        assert_eq!(step.total_saved_us, -85);
    }

    #[test]
    fn cascade_removes_all_nodes_of_one_action_without_confusing_node_and_act_ids() {
        let dag = replay_dag(
            &[4, 7, 3, 0],
            &[&[], &[0], &[1], &[2]],
            &[40, 9, 40, u32::MAX],
        );
        let mut cascade = GreedyCascade::new(&dag, dag.weight_us.clone(), 20);
        let first = cascade.next().unwrap();
        assert_eq!((first.node, first.act), (1, 9));
        let second = cascade.next().unwrap();
        assert_eq!((second.node, second.act), (0, 40));
        assert_eq!(second.duration_us, 4, "duration belongs to the chosen node");
        assert_eq!(
            second.marginal_saved_us, 7,
            "both nodes mapping to action40 are removed"
        );
        assert_eq!(cascade.weights(), &[0, 0, 0, 0]);
        assert!(cascade.next().is_none());
    }

    #[test]
    fn cascade_limit_is_bounded_and_an_empty_or_zero_graph_has_no_removals() {
        let pred_lists: Vec<Vec<u32>> = (0..24u32)
            .map(|n| n.checked_sub(1).into_iter().collect())
            .collect();
        let preds: Vec<&[u32]> = pred_lists.iter().map(Vec::as_slice).collect();
        let dag = replay_dag(&[1; 24], &preds, &(0..24).collect::<Vec<_>>());
        let steps: Vec<_> = GreedyCascade::new(&dag, dag.weight_us.clone(), usize::MAX).collect();
        assert_eq!(steps.len(), MAX_CASCADE_STEPS);
        assert_eq!(steps.last().unwrap().cp_after_us, 4);
        assert_eq!(steps.last().unwrap().cumulative_saved_us, 20);
        assert!(
            GreedyCascade::new(&dag, dag.weight_us.clone(), 0)
                .next()
                .is_none()
        );
        for empty in [
            replay_dag(&[], &[], &[]),
            replay_dag(&[0, 0], &[&[], &[0]], &[3, u32::MAX]),
        ] {
            assert_eq!(replay_path(&empty, &empty.weight_us), (0, Vec::new()));
            let mut cascade = GreedyCascade::new(&empty, empty.weight_us.clone(), 20);
            assert_eq!(cascade.cp_us(), 0);
            assert!(cascade.path().is_empty());
            assert!(cascade.next().is_none());
            assert_eq!(cascade.into_weights(), empty.weight_us);
        }
    }

    #[test]
    fn cascade_cancellation_is_terminal_and_does_not_publish_a_partial_removal() {
        let dag = replay_dag(&[5, 7], &[&[], &[0]], &[20, 30]);
        assert!(GreedyCascade::new_with_cancel(&dag, dag.weight_us.clone(), 20, || true).is_none());
        let mut cascade = GreedyCascade::new(&dag, dag.weight_us.clone(), 20);
        let original_path = cascade.path().to_vec();
        let checks = std::cell::Cell::new(0usize);
        // Check1: selection; check2: weight scan; check3: replay, after
        // the selected weight has already changed. It must be restored.
        assert!(
            cascade
                .next_with_cancel(|| {
                    checks.set(checks.get() + 1);
                    checks.get() == 3
                })
                .is_none()
        );
        assert_eq!(checks.get(), 3);
        assert_eq!(cascade.weights(), dag.weight_us);
        assert_eq!(cascade.path(), original_path);
        assert_eq!(cascade.cp_us(), dag.cp);
        assert!(cascade.next().is_none());
    }

    #[test]
    fn cascade_stays_inside_the_selected_trace_run() {
        let tp = crate::temp_path("cascade-runs-trace.json");
        let gp = crate::temp_path("cascade-runs-graph.json");
        std::fs::write(&tp, concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"a (x)","ts":0,"dur":100,"args":{"run":"one"}},"#,
            "\n",
            r#"{"ph":"X","cat":"recipe","name":"a (x)","ts":1000,"dur":10000,"args":{"run":"two"}},"#,
            "\n",
        )).unwrap();
        std::fs::write(&gp, "{\"graph\":{\"enc\":2}}\n{\"t\":\"a (x)\",\"d\":[]}\n").unwrap();
        let tr = parse::parse(tp.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        assert!(crate::dag::build_full(&tr, &g, None).is_none());
        for (run, expected) in [("one", 100), ("two", 10000)] {
            let (b, _) = crate::dag::build_full(&tr, &g, Some(run)).unwrap();
            let steps: Vec<_> = GreedyCascade::new(&b.dag, b.dag.weight_us.clone(), 20).collect();
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[0].duration_us, expected);
            let act = &tr.acts[steps[0].act as usize];
            assert_eq!(tr.run_ids[act.run as usize], run);
            assert_eq!(steps[0].cp_after_us, 0);
        }
        std::fs::remove_file(tp).unwrap();
        std::fs::remove_file(gp).unwrap();
    }

    #[test]
    fn replay_on_unmodified_weights_matches_the_dag() {
        let (b, si) = bundle();
        // CP = a(100) + b(1000) + d(50)
        assert_eq!(b.dag.cp, 1150, "baseline CP is the a->b->d chain");
        let r = replay(&b.dag, &si.base);
        assert_eq!(r.cp_us, b.dag.cp);
        assert_eq!(r.t1_us, b.dag.t1);
    }

    #[test]
    fn zero_queue_moves_the_critical_path() {
        let (b, si) = bundle();
        let (w, n) = w_zero_stage(&si, |st| st.queue_us.unwrap_or(0));
        assert_eq!(n, 1, "only b carries a queue stage");
        let r = replay(&b.dag, &w);
        // b drops to 400 -> the c branch (900) now gates: 100+900+50
        assert_eq!(r.cp_us, 1050);
    }

    #[test]
    fn bandwidth_rescales_the_fetch_branch() {
        let (b, si) = bundle();
        // 9 MB at 9 MB/s = 1s modeled fetch, replacing the observed
        // 800us: c grows to 900 - 800 + 1_000_000
        let (w, n) = w_bandwidth(&si, 9_000_000);
        assert_eq!(n, 1);
        let r = replay(&b.dag, &w);
        assert_eq!(r.cp_us, 100 + (900 - 800 + 1_000_000) + 50);
        // and at infinite bandwidth the fetch cost vanishes
        let (w, _) = w_bandwidth(&si, i64::MAX / 4);
        let r = replay(&b.dag, &w);
        assert_eq!(r.cp_us, 1150, "b still gates");
    }

    #[test]
    fn standard_set_reports_against_the_baseline() {
        let (b, si) = bundle();
        let rep = standard_whatifs(&b, &si);
        assert_eq!(rep.base_cp_us, 1150);
        assert_eq!(rep.n_action_nodes, 4);
        assert_eq!(rep.n_with_stages, 4);
        let get = |label: &str| {
            rep.rows
                .iter()
                .find(|r| r.label.starts_with(label))
                .unwrap_or_else(|| panic!("scenario {label} should exist"))
        };
        assert_eq!(get("RE queue eliminated").saved_us, 100);
        assert_eq!(get("input preparation counters removed").n_modified, 1);
        // "execution time only": a=100, b=400, c=100, d=50 -> CP 550
        assert_eq!(get("recorded non-execution counters removed").cp_us, 550);
        assert_eq!(get("recorded non-execution counters removed").saved_us, 600);
        // the biggest CP step (b) gets a split scenario
        assert!(rep.rows.iter().any(|r| r.label == "split 4x: b (x)"));
        // scenarios never mutate the baseline
        let r = replay(&b.dag, &si.base);
        assert_eq!(r.cp_us, 1150);
    }

    #[test]
    fn cp_rank_savings_are_shadow_path_bounded_not_duration_share() {
        let (b, _si) = bundle();
        // CP = a(100)+b(1000)+d(50) = 1150; the b-avoiding shadow path
        // is a+c+d = 1050.
        let rep = cp_rank(&b);
        assert_eq!(rep.base_cp_us, 1150);
        assert_eq!(rep.rows.len(), 3, "one row per CP step");
        let by_node = |node_name: &str| {
            rep.rows
                .iter()
                .find(|r| b.events[b.node_ev[r.node as usize].unwrap()].name == node_name)
                .unwrap()
        };
        // b is 87% of the CP but eliminating it saves only 100us: the
        // shadow path through c takes over. THE point of this rank.
        assert_eq!(by_node("b (x)").save_elim_us, 100);
        // halving b (w=500): a+b+d = 650 < 1050, c-path still binds.
        assert_eq!(by_node("b (x)").save_half_us, 100);
        // a is on EVERY path: eliminating it saves its full duration.
        assert_eq!(by_node("a (x)").save_elim_us, 100);
        // d likewise terminal on both paths.
        assert_eq!(by_node("d (x)").save_elim_us, 50);
        // rows sorted by save_elim descending
        assert!(
            rep.rows
                .windows(2)
                .all(|p| p[0].save_elim_us >= p[1].save_elim_us)
        );
    }

    /// Every node backing an action of this name, paired with its
    /// measured weight — the group shape [`marginal_at`] reverts, and
    /// what the walker builds per override.
    fn group_of(b: &DagBundle, name: &str) -> Vec<(u32, i64)> {
        (0..b.dag.weight_us.len() as u32)
            .filter(|&n| b.node_ev[n as usize].is_some_and(|e| b.events[e].name == name))
            .map(|n| (n, b.dag.weight_us[n as usize]))
            .collect()
    }

    #[test]
    fn marginals_are_relative_to_the_rest_of_the_scenario() {
        let (b, _si) = bundle();
        // Two overrides, both "removed". c is NOT on the measured CP —
        // it only gates once b is gone, so the second override is
        // worthless standing alone and decisive in this stack.
        let gb = group_of(&b, "b (x)");
        let gc = group_of(&b, "c (x)");
        assert!(!gb.is_empty() && !gc.is_empty());
        let edits: Vec<(u32, i64)> = gb.iter().chain(&gc).map(|&(n, _)| (n, 0)).collect();
        let mut w = b.dag.weight_us.clone();
        let s = simulate_edits_from(&b, &mut w, &edits, "removed");
        assert_eq!(s.base_cp_us, 1150);
        assert_eq!(s.cp_us, 150, "a(100) + d(50) once both branches are gone");
        let total = s.base_cp_us - s.cp_us;
        assert_eq!(total, 1000);

        // b reverted, c still gone: 100 + 1000 + 50
        let mb = marginal_at(&b.dag, &mut w, s.cp_us, &gb);
        assert_eq!(mb, 1000);
        // c reverted, b still gone: 100 + 900 + 50
        let mc = marginal_at(&b.dag, &mut w, s.cp_us, &gc);
        assert_eq!(mc, 900);
        // THE reason the box carries a footer: 1900 != 1000. Both
        // branches gate the same join, so each one's marginal counts
        // savings the other is also claiming.
        assert_ne!(mb + mc, total);

        // and the scenario weights survive both probes
        assert_eq!(replay(&b.dag, &w).cp_us, s.cp_us);
    }

    #[test]
    fn an_override_off_the_simulated_path_is_worth_exactly_zero() {
        let (b, _si) = bundle();
        // c alone: b (1000) still gates the join, so removing the 900us
        // shadow branch buys nothing at all.
        let gc = group_of(&b, "c (x)");
        let edits: Vec<(u32, i64)> = gc.iter().map(|&(n, _)| (n, 0)).collect();
        let mut w = b.dag.weight_us.clone();
        let s = simulate_edits_from(&b, &mut w, &edits, "removed");
        assert_eq!(s.cp_us, 1150);
        assert_eq!(marginal_at(&b.dag, &mut w, s.cp_us, &gc), 0);
    }

    #[test]
    fn a_whole_graph_override_is_not_billed_for_the_steps_stacked_on_it() {
        let (b, _si) = bundle();
        // Stand in for free admission with a rewrite every node shares:
        // halve the graph. Then stack "b removed" on top of it.
        let measured = b.dag.weight_us.clone();
        let half: Vec<i64> = measured.iter().map(|x| x / 2).collect();
        let gb = group_of(&b, "b (x)");
        let act_b = b.dag.act_of_node[gb[0].0 as usize] as usize;
        let sc = stack_overrides(&b, half, &[(act_b, 0)], "removed");
        let mut w = sc.w;
        assert_eq!(
            sc.sim.base_cp_us, 1150,
            "the baseline is always the measured CP"
        );
        assert_eq!(
            sc.sim.cp_us, 525,
            "50 + 450 + 25 once b is gone from a halved graph"
        );

        let g = whole_graph_group(&measured, &w, &sc.edits);
        // b's nodes are pinned to 0 by the step override, so the halving
        // no longer owns them
        assert!(gb.iter().all(|&(n, _)| !g.iter().any(|&(m, _)| m == n)));
        // 100 + 900 + 50 with the halving reverted everywhere else
        let m_half = marginal_at(&b.dag, &mut w, sc.sim.cp_us, &g);
        assert_eq!(m_half, 525);
        // b's own marginal restores b to the HALVED base it stands on:
        // 50 + 500 + 25 = 575, so 50 — not the 550 that restoring the
        // measured 1000 would claim, which would also undo the halving
        let m_b = marginal_at(&b.dag, &mut w, sc.sim.cp_us, &sc.groups[0]);
        assert_eq!(m_b, 50);
        // 575 != 625, as with any two overlapping rows
        assert_ne!(m_half + m_b, sc.sim.base_cp_us - sc.sim.cp_us);
        assert_eq!(replay(&b.dag, &w).cp_us, sc.sim.cp_us);
    }

    /// The GUI's stack: free admission rewrites every weight, a per-step
    /// override sits on top. That row's marginal must restore the
    /// admission-free weight, not the measured one.
    #[test]
    fn a_per_action_marginal_restores_the_admission_free_base_not_the_measured_weight() {
        let (b, si) = bundle();
        let measured = b.dag.weight_us.clone();
        let gb = group_of(&b, "b (x)");
        let b_node = gb[0].0 as usize;
        let act_b = b.dag.act_of_node[b_node] as usize;
        let mut si2 = SimInput {
            base: si.base.clone(),
            stages: si.stages.clone(),
            bytes_in: si.bytes_in.clone(),
            bytes_out: si.bytes_out.clone(),
            iu_sem_wait: si.iu_sem_wait.clone(),
            iu_cas_call: si.iu_cas_call.clone(),
            iu_bytes: si.iu_bytes.clone(),
            iu_invalid: si.iu_invalid.clone(),
            exec_sem_wait: si.exec_sem_wait.clone(),
            admission_recorded: si.admission_recorded.clone(),
            submit: si.submit.clone(),
            cat: si.cat.clone(),
            cache_hit: si.cache_hit.clone(),
            cache_ineligible: si.cache_ineligible.clone(),
            n_action_nodes: si.n_action_nodes,
            n_with_stages: si.n_with_stages,
        };
        // b waited 50us for an execution slot
        si2.exec_sem_wait[b_node] = 50;
        let (base, n) = w_free_admission(&si2);
        assert_eq!(n, 1);
        assert_eq!(base[b_node], 950);
        assert_eq!(replay(&b.dag, &base).cp_us, 1100);

        // "at 50%" of the measured 1000, the GUI's dur/2
        let sc = stack_overrides(&b, base, &[(act_b, 500)], "at 50%");
        let mut w = sc.w;
        assert_eq!(sc.sim.cp_us, 1050, "the a->c->d shadow path binds");
        let m_b = marginal_at(&b.dag, &mut w, sc.sim.cp_us, &sc.groups[0]);
        assert_eq!(
            m_b, 50,
            "b restored to its admission-free 950 gives 1100; the measured 1000 would claim 100"
        );
        let g = whole_graph_group(&measured, &w, &sc.edits);
        assert!(g.is_empty(), "b is pinned, and nothing else changed");
        assert_eq!(marginal_at(&b.dag, &mut w, sc.sim.cp_us, &g), 0);

        // a wait large enough that the override is worth nothing at all
        si2.exec_sem_wait[b_node] = 500;
        let (base, _) = w_free_admission(&si2);
        let sc = stack_overrides(&b, base, &[(act_b, 250)], "at 25%");
        let mut w = sc.w;
        assert_eq!(sc.sim.cp_us, 1050);
        assert_eq!(
            marginal_at(&b.dag, &mut w, sc.sim.cp_us, &sc.groups[0]),
            0,
            "restoring b to 500 leaves the shadow path binding"
        );
    }

    #[test]
    fn simulate_node_repaints_the_shadow_path() {
        let (b, si) = bundle();
        let bnode = b
            .dag
            .cp_path
            .iter()
            .copied()
            .find(|&n| b.events[b.node_ev[n as usize].unwrap()].name == "b (x)")
            .unwrap();
        let s = simulate_node(&b, bnode, 0, "removed");
        assert_eq!(s.base_cp_us, 1150);
        assert_eq!(s.cp_us, 1050);
        let names: Vec<&str> = s
            .path
            .iter()
            .map(|&n| b.events[b.node_ev[n as usize].unwrap()].name.as_str())
            .collect();
        assert_eq!(
            names,
            ["a (x)", "c (x)", "d (x)"],
            "simulated CP walks the shadow path"
        );
        // weights untouched after the sim
        assert_eq!(replay(&b.dag, &si.base).cp_us, 1150);
        // "as cache hit" weight for b = its cache_query_us (absent -> 0)
    }

    /// Why the walker stacks overrides instead of showing them one at a
    /// time: two steps on the same chain do not save the sum of what
    /// they save alone, in either direction.
    #[test]
    fn stacked_edits_are_not_the_sum_of_single_edits() {
        let (b, _) = bundle();
        let node = |name: &str| {
            (0..b.dag.weight_us.len() as u32)
                .find(|&n| b.node_ev[n as usize].is_some_and(|e| b.events[e].name == name))
                .unwrap()
        };
        let (bn, cn) = (node("b (x)"), node("c (x)"));
        let only_b = simulate_edits(&b, &[(bn, 0)], "b removed");
        let only_c = simulate_edits(&b, &[(cn, 0)], "c removed");
        let both = simulate_edits(&b, &[(bn, 0), (cn, 0)], "both removed");
        assert_eq!(
            (only_b.base_cp_us, only_c.base_cp_us, both.base_cp_us),
            (1150, 1150, 1150),
            "every replay scores against the measured CP"
        );
        // b alone uncovers the a->c->d shadow path; c alone is off the CP
        // and saves nothing; together they leave only a->d
        assert_eq!((only_b.cp_us, only_c.cp_us, both.cp_us), (1050, 1150, 150));
        let saved = |s: &NodeSim| s.base_cp_us - s.cp_us;
        assert_eq!(saved(&only_b) + saved(&only_c), 100);
        assert_eq!(
            saved(&both),
            1000,
            "the pair saves 10x the sum of the parts"
        );
    }

    /// What the CP walker's header total rests on: join and exit nodes are
    /// weight 0 and `replay_path` yields only action nodes, so the simulated
    /// path drawn step by step cannot disagree with the simulated CP.
    #[test]
    fn simulated_path_weights_sum_to_the_simulated_cp() {
        let (b, _) = bundle();
        let bnode = b
            .dag
            .cp_path
            .iter()
            .copied()
            .find(|&n| b.events[b.node_ev[n as usize].unwrap()].name == "b (x)")
            .unwrap();
        for w_node in [0, 250, 500, 1000, 4000] {
            let s = simulate_node(&b, bnode, w_node, "scaled");
            let sum: i64 = s
                .path
                .iter()
                .map(|&n| {
                    if n == bnode {
                        w_node
                    } else {
                        b.dag.weight_us[n as usize]
                    }
                })
                .sum();
            assert_eq!(sum, s.cp_us, "step weights vs cp_us at w_node={w_node}");
        }
    }

    #[test]
    fn queue_p50_is_nearest_rank_and_absent_without_data() {
        let (_, si) = bundle();
        assert_eq!(queue_p50(&si), Some(600));
        let empty = SimInput {
            base: vec![],
            stages: vec![],
            bytes_in: vec![],
            bytes_out: vec![],
            iu_sem_wait: vec![],
            iu_cas_call: vec![],
            iu_bytes: vec![],
            iu_invalid: vec![],
            exec_sem_wait: vec![],
            admission_recorded: vec![],
            submit: vec![],
            cat: vec![],
            cache_hit: vec![],
            cache_ineligible: vec![],
            n_action_nodes: 0,
            n_with_stages: 0,
        };
        assert_eq!(queue_p50(&empty), None);
    }

    /// The entire submit window precedes the worker, so it is not
    /// part of "execution time only".
    #[test]
    fn execution_only_removes_the_submit_window() {
        let (_, si) = bundle();
        let node = (0..si.base.len())
            .find(|&i| si.stages[i].is_some())
            .expect("fixture has staged nodes");
        let mut si2 = SimInput {
            base: si.base.clone(),
            stages: si.stages.clone(),
            bytes_in: si.bytes_in.clone(),
            bytes_out: si.bytes_out.clone(),
            iu_sem_wait: si.iu_sem_wait.clone(),
            iu_cas_call: si.iu_cas_call.clone(),
            iu_bytes: si.iu_bytes.clone(),
            iu_invalid: si.iu_invalid.clone(),
            exec_sem_wait: si.exec_sem_wait.clone(),
            admission_recorded: si.admission_recorded.clone(),
            submit: si.submit.clone(),
            cat: si.cat.clone(),
            cache_hit: si.cache_hit.clone(),
            cache_ineligible: si.cache_ineligible.clone(),
            n_action_nodes: si.n_action_nodes,
            n_with_stages: si.n_with_stages,
        };
        // 4s more wall, all of it the submit window
        si2.base[node] += 4_000_000;
        si2.submit[node] = 4_000_000;
        let (w, _) = w_execute_only(&si2);
        assert_eq!(
            w[node],
            si2.stages[node].expect("staged").execute_us,
            "execution-only strips the submit window too"
        );
    }

    #[test]
    fn poisoned_upload_fields_preserve_other_measurements_without_pricing_fallbacks() {
        for (re, expected_weight, measured_wait, measured_cas, measured_absent, has_breakdown) in [
            (
                serde_json::json!({"iu_sem_wait_us": -1, "iu_cas_call_us": 12,
                "iu_digests_absent": -1}),
                988,
                0,
                1,
                0,
                true,
            ),
            (
                serde_json::json!({"iu_sem_wait_us": 20, "iu_cas_call_us": 999,
                "invalid_counter_fields": ["iu_cas_call_us"]}),
                1000,
                1,
                0,
                1,
                true,
            ),
            (
                serde_json::json!({"iu_cas_call_us": 12, "iu_bytes_uploaded": -1}),
                1000,
                1,
                1,
                1,
                true,
            ),
            (
                serde_json::json!({"invalid_counter_fields": ["iu_bytes_uploaded"]}),
                1000,
                0,
                0,
                0,
                false,
            ),
        ] {
            let tp = crate::temp_path("upload-poison-trace.json");
            let gp = crate::temp_path("upload-poison-graph.jsonl");
            std::fs::write(
                &tp,
                format!(
                    "[\n{}\n]\n",
                    serde_json::json!({
                        "ph":"X", "cat":"recipe", "name":"job", "ts":0, "dur":1000,
                        "args":{"bytes_in":42, "re":re,
                            "stages":{"input_upload_us":900, "execute_us":10}}
                    })
                ),
            )
            .unwrap();
            std::fs::write(&gp, "{\"graph\":{\"enc\":2}}\n{\"t\":\"job\",\"d\":[]}\n").unwrap();
            let tr = parse::parse(tp.to_str().unwrap()).unwrap();
            let x = load_extras(&tr);
            assert_eq!(x.unmatched, 0);
            assert_eq!(x.args[0].iu.is_some(), has_breakdown);
            let graph = load_graph(gp.to_str().unwrap()).unwrap();
            let (b, _) = crate::dag::build_full(&tr, &graph, None).unwrap();
            let si = sim_input(&b, &x);
            let node = b
                .dag
                .act_of_node
                .iter()
                .position(|&a| a != u32::MAX)
                .unwrap();
            assert_eq!(w_bandwidth(&si, 1_000_000_000_000).0[node], expected_weight);
            let f = crate::analyses::Facts::for_run(&tr, &x, None).unwrap();
            let report = crate::upload::upload_report(&f).unwrap();
            assert_eq!(report.subs[0].n_measured, measured_wait);
            assert_eq!(report.subs[3].n_measured, measured_cas);
            assert_eq!(report.absent_measured, measured_absent);
            let exported = crate::export::export(
                &tr,
                &f,
                None,
                None,
                &["upload".into()],
                10,
                &crate::export::TopicArgs::default(),
            );
            if measured_wait == 0 {
                assert!(exported["upload"]["data"]["sub_stages"][0]["sum_us"].is_null());
            }
            if measured_absent == 0 {
                assert!(exported["upload"]["data"]["digests_absent"].is_null());
            }
            std::fs::remove_file(tp).unwrap();
            std::fs::remove_file(gp).unwrap();
        }
    }

    /// Replay the wire shape of a no-transfer upload: the zero CAS timing is
    /// suppressed on the wire and presence comes from always-written counters.
    #[test]
    fn bandwidth_replay_preserves_a_measured_zero_upload() {
        let instrumented = concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"z (compile)","ts":0,"dur":12000000,"args":{"src":"compile","run":"r","bytes_in":104857600,"bytes_out":1048576,"stages":{"materialize_us":1000000,"input_upload_us":10000000,"execute_us":1000000},"re":{"kind":"remote","iu_sem_wait_us":9000000,"iu_find_missing_us":1000000,"iu_digests_queried":40}}},"#,
            "\n"
        );
        let uninstrumented = concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"z (compile)","ts":0,"dur":12000000,"args":{"src":"compile","run":"r","bytes_in":104857600,"bytes_out":1048576,"stages":{"materialize_us":1000000,"input_upload_us":10000000,"execute_us":1000000}}},"#,
            "\n"
        );
        let graph = "{\"graph\":{\"enc\":2}}\n{\"t\":\"z (compile)\",\"d\":[]}\n";
        let run = |trace: &str, tag: &str| {
            let tp = crate::temp_path(&format!("{tag}-trace.json"));
            let gp = crate::temp_path(&format!("{tag}-graph.json"));
            std::fs::write(&tp, trace).unwrap();
            std::fs::write(&gp, graph).unwrap();
            let tr = parse::parse(tp.to_str().unwrap()).unwrap();
            let g = load_graph(gp.to_str().unwrap()).unwrap();
            let x = load_extras(&tr);
            let (b, _) = crate::dag::build_full(&tr, &g, None).unwrap();
            let si = sim_input(&b, &x);
            let rep = standard_whatifs(&b, &si);
            let row = |label: &str| {
                rep.rows
                    .iter()
                    .find(|r| r.label == label)
                    .unwrap_or_else(|| panic!("scenario {label} should exist"))
                    .clone()
            };
            let out = (
                row("logical input prep + upload at infinite"),
                row("logical input prep + upload at wired 100 MiB/s"),
            );
            std::fs::remove_file(&tp).ok();
            std::fs::remove_file(&gp).ok();
            out
        };

        let (inf, wired) = run(instrumented, "iu-zero");
        // only the 1 s fetch is transfer: infinite saves it, 100 MiB/s
        // reprices it to the same 1 s
        assert_eq!(inf.saved_us, 1_000_000, "{}", inf.note);
        assert_eq!(wired.saved_us, 0);
        assert!(
            inf.note.contains("0 of 1 uploading actions"),
            "{}",
            inf.note
        );

        let (inf, _) = run(uninstrumented, "iu-none");
        // no sub-stages: the whole 10 s band is treated as transfer
        assert_eq!(inf.saved_us, 11_000_000);
    }
}
