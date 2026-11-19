/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The action-detail data layer: everything the detail popup shows,
//! computed as pure data (the paint loop never recomputes it). The
//! record is re-read from the trace file by byte offset at open time —
//! the parsed `Act` is lean on purpose, the popup is where the full
//! row comes back.

use serde_json::Value;

use crate::analyses::fmt_bytes_f;
use crate::analyses::fmt_dur;
use crate::analyses::fmt_dur_f;

/// Bound a full-record preview consistently in the GUI and headless inspector.
pub(crate) const MAX_DETAIL_RECORD_BYTES: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------- stages

/// Who measured a stage's duration. The first line of every stage
/// hover: a reader looking at a number must never have to guess whether
/// it was counted on the worker or inferred on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageProv {
    /// counted on the remote worker, returned in its response
    Worker,
    /// a span this machine's buck2 daemon opened and closed
    Client,
    /// a residual from recorded coverage, not a measured operation
    Derived,
    /// this machine's materializer, for actions that ran locally
    LocalMaterializer,
    /// Elapsed client observation of a state in the remote service.
    RemoteState,
    /// Sum of durations which may include parallel operations.
    DurationSum,
    /// An older conversion used one key for different measurement sources.
    Legacy,
}

impl StageProv {
    pub fn text(self) -> &'static str {
        match self {
            Self::Worker => "measured by the worker",
            Self::Client => "client-side span",
            Self::Derived => "derived \u{2014} residual elapsed time, not measured CPU",
            Self::LocalMaterializer => "measured by this machine's materializer",
            Self::RemoteState => "client-observed remote state (not worker service time)",
            Self::DurationSum => "sum of recorded durations (may overlap)",
            Self::Legacy => "legacy counter; measurement source is not disambiguated",
        }
    }
}

/// Everything that describes one stage band.
///
/// The taxonomy is defined here once and the flat `STAGE_*` arrays below
/// are derived from it, so a band cannot exist in some of them and not
/// the others.
pub struct Stage {
    /// the capture's key for this stage, and the name it is configured
    /// under in `[colors.stage]`
    pub key: &'static str,
    /// the full display name. Never a bare "upload" \u{2014} inputs and outputs
    /// move at different times, through different machinery, and
    /// conflating them hid a queueing pathology.
    pub label: &'static str,
    /// short form for tight chips and bar-segment text; an abbreviation
    /// of [`Stage::label`], same rule
    pub chip: &'static str,
    /// the one-line glossary shown as legend tooltips and in
    /// Help \u{25b8} Stage glossary
    pub help: &'static str,
    /// where this stage's number comes from
    pub prov: StageProv,
    /// the warning that belongs with the number but does not fit the
    /// one-line definition; `None` wherever [`Stage::help`] carries it
    pub caveat: Option<&'static str>,
    /// `None` is `execute_us`, painted in the action's own build-type
    /// colour
    pub color: Option<[u8; 3]>,
}

/// The stage taxonomy, in stable counter order (not chronological order).
/// A new band is one row \u{2014} the
/// arrays below, the config file's accepted keys and the glossary all
/// follow from it.
pub const STAGES: &[Stage] = &[
    Stage {
        key: "cache_query_us",
        label: "cache lookup",
        chip: "cache lookup",
        help: "checking for an existing result: the client span includes Buck admission \
               and the awaited SDK operation, including its queues and response handling",
        prov: StageProv::Client,
        caveat: Some(
            "This is not network RTT or backend-only latency. Optional sub-rows distinguish admission from the admitted client/SDK operation.",
        ),
        color: Some([0xf1, 0xce, 0x63]),
    },
    Stage {
        key: "input_upload_us",
        label: "inputs upload (to CAS)",
        chip: "inputs upload",
        help: "before running remotely, the client asks CAS which input blobs it is \
               missing and uploads only those; elapsed time may include admission, \
               preparation and storage calls",
        prov: StageProv::Client,
        caveat: Some(
            "Sub-rows split this stage: waiting for an upload slot is pure queueing \u{2014} \
             only writing new bytes moves data.",
        ),
        color: Some([0xd4, 0xa6, 0xc8]),
    },
    Stage {
        key: "exec_slot_wait_us",
        label: "waiting for an execution slot (client-side limit)",
        chip: "exec slot wait",
        help: "buck2 caps how many actions may be in remote execution at once \
               (execution_concurrency_limit); time here is queueing inside the \
               client before submission \u{2014} no work is happening",
        prov: StageProv::Client,
        caveat: None,
        color: Some([0x9d, 0x93, 0xc8]),
    },
    Stage {
        key: "queue_us",
        label: "RE queue",
        chip: "RE queue",
        help: "recorded remote queue wait, distinct from worker input download",
        prov: StageProv::Client,
        caveat: Some(
            "Older converted captures may include a wider first-enqueue-to-worker interval \
             here. New conversions report that interval separately under RE observations; \
             it is not a queue duration.",
        ),
        color: Some([0xc8, 0xc8, 0xc8]),
    },
    Stage {
        key: "materialize_us",
        label: "worker input preparation (CAS)",
        chip: "worker input prep",
        help: "preparing the assigned worker's inputs, including directory and cache work, \
               shared-download handling, and materializing files",
        prov: StageProv::Worker,
        caveat: Some(
            "Logical input bytes may already be cache-resident. Bytes divided by input \
             preparation time are an effective preparation rate, not network bandwidth. \
             A long interval does not by itself identify transfer or queueing as its cause.",
        ),
        color: Some([0x6f, 0xb8, 0xe8]),
    },
    Stage {
        key: "hashing_us",
        label: "hashing",
        chip: "hashing",
        help: "reported content hashing; local command metadata measures output-file hashing, while legacy or other sources may not identify inputs versus outputs",
        prov: StageProv::DurationSum,
        caveat: None,
        color: Some([0xa5, 0xd8, 0xa5]),
    },
    Stage {
        key: "execute_us",
        label: "execution",
        chip: "execution",
        help: "the command itself, on the worker (or locally)",
        prov: StageProv::Worker,
        caveat: None,
        color: None,
    },
    Stage {
        key: "upload_us",
        label: "outputs upload (worker to CAS)",
        chip: "outputs upload",
        help: "worker output preparation/upload, measured on the worker when available; \
               otherwise the client's observed worker-upload state. An observed state \
               includes notification delays and is not a network-throughput measurement",
        prov: StageProv::Worker,
        caveat: None,
        color: Some([0xd4, 0xa6, 0xc8]),
    },
    Stage {
        key: "result_wait_us",
        label: "result stage (legacy)",
        chip: "result stage",
        help: "legacy result-stage counter: older native conversions put the client's \
               observed worker-upload state here, not client queueing",
        prov: StageProv::Legacy,
        caveat: Some(
            "Reopen the original event log to separate worker upload from other result handling. This counter alone does not establish an idle wait or its cause.",
        ),
        color: Some([0xe0, 0x5c, 0x3c]),
    },
    Stage {
        key: "download_us",
        label: "download / materialize (from CAS)",
        chip: "download",
        help: "the client lazily pulls output bytes from CAS only when something \
               local needs them (deferred materialization)",
        prov: StageProv::Client,
        caveat: None,
        color: Some([0x5c, 0xc8, 0xc0]),
    },
    Stage {
        key: "rtt_us",
        label: "round trips + retries",
        chip: "round trips",
        help: "RE client round trips and retries (legacy captures)",
        prov: StageProv::Derived,
        caveat: None,
        color: Some([0xd9, 0x6a, 0x3a]),
    },
    Stage {
        key: "daemon_us",
        label: "uncovered time (client-span residual)",
        chip: "client residual",
        help: "elapsed time outside observed client-span coverage; the cause is \
               not measured",
        prov: StageProv::Derived,
        caveat: Some(
            "Sub-rows are positioned from client-side spans only \u{2014} worker counters \
             and hashing duration sums can overlap them. This residual does not establish daemon \
             CPU work, executor queueing, or idleness.",
        ),
        color: Some([0x8b, 0x8f, 0x98]),
    },
    Stage {
        key: "cache_hit_us",
        label: "cache-hit result handling",
        chip: "cached result",
        help: "accepting cached outputs: output-tree metadata retrieval and construction, \
               output ownership, and materializer declaration; file bytes may be materialized later",
        prov: StageProv::Client,
        caveat: Some(
            "Inclusive client elapsed time, not worker execution, CPU time, or necessarily output-file download. Subphases are parts of this stage, not additional work.",
        ),
        // Reuse the existing client-materialization colour.
        color: Some([0x5c, 0xc8, 0xc0]),
    },
    Stage {
        key: "prepare_us",
        label: "action request preparation",
        chip: "request prep",
        help: "constructing and serializing the action request, command arguments, \
               platform, and related metadata before lookup or execution",
        prov: StageProv::Client,
        caveat: Some(
            "This is elapsed request construction, not all prerequisite preparation or all input hashing, and is distinct from remote execution submission preparation.",
        ),
        // Reuse the existing local input-preparation colour.
        color: Some([0xa5, 0xd8, 0xa5]),
    },
    Stage {
        key: "cas_artifact_us",
        label: "CAS artifact preparation",
        chip: "CAS artifact",
        help: "client-observed expiration checks, optional TTL extension, tree metadata preparation, and output declaration",
        prov: StageProv::Client,
        caveat: Some(
            "This is not artifact-content download or backend-only latency. Chronological activity retains the separately recorded phases.",
        ),
        color: Some([0xf1, 0xce, 0x63]),
    },
    Stage {
        key: "local_finalize_outputs_us",
        label: "local output finalization",
        chip: "finalize outputs",
        help: "client-observed collection, hashing, metadata preparation, and declaration of local command outputs",
        prov: StageProv::Client,
        caveat: Some(
            "Includes local output hashing. The subordinate hash counter sums file durations and is not additional wall time.",
        ),
        color: Some([0xa5, 0xd8, 0xa5]),
    },
    Stage {
        key: "local_cache_check_us",
        label: "local cache check",
        chip: "local cache",
        help: "client validation of an identical action or filtered inputs against the local cache, including confirming outputs with the materializer",
        prov: StageProv::Client,
        caveat: Some(
            "An in-memory hit identifies entry provenance, not negligible latency. Persisted-store sub-timers are optional and do not partition the entire check. Remote dep-file checks are not added to this band.",
        ),
        color: Some([0xf1, 0xce, 0x63]),
    },
];

/// Number of stage slots, including retained legacy counters.
pub const STAGE_N: usize = STAGES.len();

const fn stage_keys() -> [&'static str; STAGE_N] {
    let mut out = [""; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].key;
        i += 1;
    }
    out
}

const fn stage_labels() -> [&'static str; STAGE_N] {
    let mut out = [""; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].label;
        i += 1;
    }
    out
}

const fn stage_chips() -> [&'static str; STAGE_N] {
    let mut out = [""; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].chip;
        i += 1;
    }
    out
}

const fn stage_helps() -> [&'static str; STAGE_N] {
    let mut out = [""; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].help;
        i += 1;
    }
    out
}

const fn stage_provs() -> [StageProv; STAGE_N] {
    let mut out = [StageProv::Client; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].prov;
        i += 1;
    }
    out
}

const fn stage_caveats() -> [Option<&'static str>; STAGE_N] {
    let mut out = [None; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].caveat;
        i += 1;
    }
    out
}

const fn stage_colors() -> [Option<[u8; 3]>; STAGE_N] {
    let mut out = [None; STAGE_N];
    let mut i = 0;
    while i < STAGE_N {
        out[i] = STAGES[i].color;
        i += 1;
    }
    out
}

/// The capture keys, in display order.
pub const STAGE_ORDER: [&str; STAGE_N] = stage_keys();

/// Parallel to [`STAGE_ORDER`]: the full display names.
pub const STAGE_LABEL: [&str; STAGE_N] = stage_labels();

/// Parallel to [`STAGE_ORDER`]: short forms for tight chips and
/// bar-segment text.
pub const STAGE_CHIP: [&str; STAGE_N] = stage_chips();

/// Parallel to [`STAGE_ORDER`]: the one-line glossary.
pub const STAGE_HELP: [&str; STAGE_N] = stage_helps();

/// Parallel to [`STAGE_ORDER`]: where each stage's number comes from.
pub const STAGE_PROV: [StageProv; STAGE_N] = stage_provs();

/// Parallel to [`STAGE_ORDER`]: the warning that belongs with the
/// number but does not fit the one-line definition. `None` wherever
/// [`STAGE_HELP`] already carries it.
pub const STAGE_CAVEAT: [Option<&str>; STAGE_N] = stage_caveats();

/// Parallel to [`STAGE_ORDER`]: the built-in stage colours the config
/// file's `[colors.stage]` overrides. `None` is `execute_us`, painted in
/// the action's own build-type colour.
pub const STAGE_COLOR: [Option<[u8; 3]>; STAGE_N] = stage_colors();

/// [`STAGE_PROV`], with the one entry that depends on where the action
/// ran: a local action's input fetch is this machine's materializer
/// writing to disk, not a worker filling a sandbox.
pub fn stage_prov(si: usize, local_fetch: bool) -> StageProv {
    if si == STAGE_FETCH_IDX && local_fetch {
        return StageProv::LocalMaterializer;
    }
    if si == STAGE_EXEC_IDX && local_fetch {
        return StageProv::Client;
    }
    STAGE_PROV[si]
}

/// The provenance table folded onto one line, naming only the stages
/// `lines` actually has. Text surfaces — the copied popup, the
/// analysis prompt — have nothing to hang [`stage_hover`] off, and
/// numbers with no measurement source beside them are what the
/// provenance table exists to prevent.
pub fn stages_prov_line(lines: &[StageLine], local_fetch: bool) -> String {
    let mut groups: Vec<(StageProv, Vec<&str>)> = Vec::new();
    for line in lines {
        let Some(si) = line.stage else { continue };
        let p = line
            .provenance
            .unwrap_or_else(|| stage_prov(si, local_fetch));
        match groups.iter_mut().find(|(g, _)| *g == p) {
            Some((_, v)) => v.push(STAGE_CHIP[si]),
            None => groups.push((p, vec![STAGE_CHIP[si]])),
        }
    }
    let body = groups
        .iter()
        .map(|(p, v)| format!("{}: {}", v.join(", "), p.text()))
        .collect::<Vec<_>>()
        .join("; ");
    format!("({body})")
}

/// Every stage line as plain text, provenance last. The bar-first
/// layouts put most of these numbers one hover away, and a screenshot of
/// a hover is not a table — this is what the escalation doc pastes.
pub fn stage_table_text(lines: &[StageLine], local_fetch: bool) -> String {
    let mut s = String::new();
    for l in lines {
        s.push_str(&l.text);
        s.push('\n');
    }
    s.push_str(&stages_prov_line(lines, local_fetch));
    s.push('\n');
    s
}

/// The hover for one stage: provenance, then the definition, then the
/// caveat if it has one. The single composition point — the glossary
/// window, the legend chips and the popup rows all render this, so no
/// wording lives in two places.
pub fn stage_hover(si: usize, local_fetch: bool) -> String {
    stage_hover_with_provenance(si, stage_prov(si, local_fetch))
}

fn stage_prov_for_args(si: usize, args: &Value) -> StageProv {
    if si == STAGE_FETCH_IDX
        && args
            .pointer("/stages/materialize_source")
            .and_then(Value::as_str)
            == Some("client_span")
    {
        return StageProv::Client;
    }
    if STAGE_ORDER.get(si) == Some(&"upload_us")
        && args
            .pointer("/stages/upload_source")
            .and_then(Value::as_str)
            == Some("client_state")
    {
        return StageProv::RemoteState;
    }
    if si == STAGE_EXEC_IDX
        && args
            .get("stages")
            .and_then(|s| s.get("execute_source"))
            .and_then(Value::as_str)
            == Some("client_span")
    {
        StageProv::Client
    } else {
        stage_prov(
            si,
            args.get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| k.starts_with("local")),
        )
    }
}

/// The row and its bar segment share the record's measurement source;
/// the generic glossary has no record and continues to use `stage_hover`.
pub fn stage_hover_for_args(si: usize, args: &Value) -> String {
    if si == STAGE_HASH_IDX
        && args
            .pointer("/stages/hashing_source")
            .and_then(Value::as_str)
            == Some("local_outputs")
    {
        return "Local output hashing: sum of reported output-file hashing durations. File operations can overlap; this is not a positioned interval or input hashing.".to_owned();
    }
    stage_hover_with_provenance(si, stage_prov_for_args(si, args))
}

fn stage_hover_with_provenance(si: usize, provenance: StageProv) -> String {
    let mut s = format!(
        "{}\n\n{}: {}",
        provenance.text(),
        STAGE_LABEL[si],
        STAGE_HELP[si]
    );
    if let Some(c) = STAGE_CAVEAT[si] {
        s.push_str("\n\n");
        s.push_str(c);
    }
    s
}

/// Counter residuals do not locate an interval or identify its cause.
pub const UNACCOUNTED_HELP: &str = "positive action-duration minus stage-counter residual; \
    not proof of idleness or daemon CPU work. Stage counters can overlap.";

/// Non-stage terms for Help ▸ Stage glossary.
pub const GLOSSARY_TERMS: [(&str, &str); 9] = [
    (
        "upload permit wait",
        "time an upload spent queued for one of the client's upload slots \
         (buck2's upload semaphore) before any work started; dead time, \
         not transfer — instrumented captures break it out under the \
         inputs-upload stage",
    ),
    (
        "execution slot",
        "buck2 caps how many actions may be in remote execution at once \
         (execution_concurrency_limit); an action holds a slot from \
         submission to completion, and the exec-slot-wait stage is the \
         time spent parked before submission when no slot was free",
    ),
    (
        "presence check",
        "before uploading, the client checks which input blobs the CAS \
         already has; the ones it lacks are the ones actually uploaded",
    ),
    (
        "CAS",
        "the remote-execution content-addressed store: every file is \
         identified by its digest (hash + size); a deduplicated blob \
         store for build artifacts that workers and clients both use",
    ),
    (
        "RE session",
        "one build's authenticated connection to remote execution; its \
         ID keys all server-side telemetry",
    ),
    (
        "span / span id",
        "every daemon operation is an event pair with a unique per-build \
         id; dependency edges join on these ids, not names",
    ),
    (
        "ActionKey",
        "the human-readable action name; order-dependent for dynamic \
         actions, which is why edges join on span ids",
    ),
    (
        "critical path (dependency floor)",
        "the longest must-run-in-order chain; the theoretical minimum \
         wall time for the build",
    ),
    ("unaccounted", UNACCOUNTED_HELP),
];

/// Index of `cache_query_us` in [`STAGE_ORDER`]. "Simulate as cache hit"
/// deliberately uses a simplifying lookup-only replacement, excluding
/// request preparation and cached-result handling. It requires the recorded
/// key ([`recorded_lookup_us`]); an absent key is not a measured zero.
pub const STAGE_LOOKUP_IDX: usize = 0;

/// The step's recorded cache-lookup time: `Some` only when the record
/// carries a `stages.cache_query_us` key. An explicit zero is a
/// measured zero and stays `Some(0)`; an absent key is `None`.
pub fn recorded_lookup_us(args: &Value) -> Option<i64> {
    opt_stage_us(args, STAGE_ORDER[STAGE_LOOKUP_IDX])
}

/// Index of `execute_us` in [`STAGE_ORDER`] — the one stage that is the
/// action doing its own work rather than waiting on machinery. A test
/// pins it to the array so a reordering cannot silently skew every
/// consumer.
pub const STAGE_EXEC_IDX: usize = 6;

/// Index of `materialize_us` in [`STAGE_ORDER`] — the input-fetch
/// stage, whose label and child rows depend on where the fetch actually
/// happened (a worker's sandbox vs this machine's disk). Pinned by the
/// same test as [`STAGE_EXEC_IDX`].
pub const STAGE_FETCH_IDX: usize = 4;

/// Index of `input_upload_us` in [`STAGE_ORDER`]. Pinned by the same
/// test as [`STAGE_EXEC_IDX`].
pub const STAGE_IU_IDX: usize = 1;

/// Index of the synthetic `exec_slot_wait_us` in [`STAGE_ORDER`] — the
/// execution-admission band, sourced from `args.re.rs_exec_sem_wait_us`
/// rather than `args.stages` (instrumented captures only; zero and
/// therefore invisible elsewhere). Pinned by the same test as
/// [`STAGE_EXEC_IDX`].
pub const STAGE_SLOT_IDX: usize = 2;

/// Index of `daemon_us` in [`STAGE_ORDER`] — the residual band. Pinned
/// by the same test as [`STAGE_EXEC_IDX`].
pub const STAGE_DAEMON_IDX: usize = 11;

/// Existing slot indices stay stable as newly classified client spans are added.
pub const STAGE_CACHE_HIT_IDX: usize = 12;
pub const STAGE_PREPARE_IDX: usize = 13;
pub const STAGE_HASH_IDX: usize = 5;
pub const STAGE_CAS_ARTIFACT_IDX: usize = 14;
pub const STAGE_FINALIZE_IDX: usize = 15;
pub const STAGE_LOCAL_CACHE_IDX: usize = 16;

/// Stage-counter accounting, reconciled per action before aggregation.
/// Residual in one action and excess counters in another must both
/// survive: neither can account for time in the other action.
#[derive(Default)]
pub struct CpAccounting {
    pub total: i64,
    pub exec: i64,
    /// summed non-exec stage counters, not necessarily disjoint waits
    pub waits: i64,
    /// Sum of positive per-action wall-minus-counter residuals. This
    /// is not measured idleness: duration counters do not locate work.
    pub unaccounted: i64,
    /// wall time belonging to steps that carry no stage counters at all.
    /// Held apart from [`Self::unaccounted`]: no counter measurement is
    /// available for these steps, so no residual was calculated.
    pub unmeasured: i64,
    /// how many steps contributed [`Self::unmeasured`]
    pub n_unmeasured: usize,
    /// Sum of positive per-action counter-minus-wall excess. It may
    /// reflect overlapping or inconsistent counters, not an exact
    /// measure of temporal overlap.
    pub overlap: i64,
}

impl CpAccounting {
    /// Reconcile this step before adding it to a path. `None` means
    /// absent measurements; an all-zero counter array is still measured.
    pub fn add_step(&mut self, total: i64, stages: Option<&[i64]>) {
        let step = match stages {
            Some(stages) => cp_accounting(total, stages, 0, 0),
            None => cp_accounting(total, &[], total, 1),
        };
        self.total = self.total.saturating_add(step.total);
        self.exec = self.exec.saturating_add(step.exec);
        self.waits = self.waits.saturating_add(step.waits);
        self.unaccounted = self.unaccounted.saturating_add(step.unaccounted);
        self.unmeasured = self.unmeasured.saturating_add(step.unmeasured);
        self.n_unmeasured = self.n_unmeasured.saturating_add(step.n_unmeasured);
        self.overlap = self.overlap.saturating_add(step.overlap);
    }
}

/// Reconcile one window. `unmeasured` explicitly excludes wall without
/// stage measurements. For a path of several actions, use
/// [`CpAccounting::add_step`] rather than passing aggregated counters:
/// clamping after aggregation would cancel unrelated residual/excess.
pub fn cp_accounting(
    total: i64,
    totals: &[i64],
    unmeasured: i64,
    n_unmeasured: usize,
) -> CpAccounting {
    let staged = totals
        .iter()
        .fold(0i64, |sum, &value| sum.saturating_add(value.max(0)));
    let exec = totals.get(STAGE_EXEC_IDX).copied().unwrap_or(0);
    CpAccounting {
        total,
        exec,
        waits: staged.saturating_sub(exec),
        unaccounted: total
            .saturating_sub(staged)
            .saturating_sub(unmeasured)
            .max(0),
        unmeasured,
        n_unmeasured,
        overlap: staged
            .saturating_add(unmeasured)
            .saturating_sub(total)
            .max(0),
    }
}

/// Which stages deserve a slow highlight in the popup. Execution never
/// qualifies — that is the productive work; the highlight exists to make
/// machinery waits pop. Nothing highlights on fast actions:
/// - eligible only when the wall is >= 2s;
/// - candidates are non-exec stages taking >= max(25% of wall, 1s);
/// - the single LARGEST candidate paints red; any other candidate
///   bolds — two red "% of wall" claims side by side read as a >100%
///   action, so exactly one thing pops;
/// - if none qualifies but RECORDED non-exec stage time is >= half the
///   wall, the single largest non-exec stage paints red (recorded
///   stages, not wall-minus-exec: unaccounted time is nobody's wait,
///   and blaming a tiny stage for it painted noise red).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageMark {
    None,
    Bold,
    Red,
}

pub fn slow_stage_marks(dur: i64, st: &[i64]) -> Vec<StageMark> {
    let mut out = vec![StageMark::None; st.len()];
    if dur < 2_000_000 {
        return out;
    }
    let thr = (dur / 4).max(1_000_000);
    let measured = |i| {
        !matches!(
            STAGE_PROV.get(i),
            Some(StageProv::Derived | StageProv::Legacy)
        )
    };
    let candidates: Vec<usize> = (0..st.len())
        .filter(|&i| i != STAGE_EXEC_IDX && measured(i) && st[i] >= thr)
        .collect();
    if let Some(&worst) = candidates.iter().max_by_key(|&&i| st[i]) {
        for &i in &candidates {
            out[i] = if i == worst {
                StageMark::Red
            } else {
                StageMark::Bold
            };
        }
        return out;
    }
    let nonexec = (0..st.len())
        .filter(|&i| i != STAGE_EXEC_IDX && measured(i))
        .fold(0i64, |a, i| a.saturating_add(st[i]));
    if nonexec.saturating_mul(2) >= dur {
        let worst = (0..st.len())
            .filter(|&i| i != STAGE_EXEC_IDX && measured(i) && st[i] > 0)
            .max_by_key(|&i| st[i]);
        if let Some(i) = worst {
            out[i] = StageMark::Red;
        }
    }
    out
}

/// One line of the popup's Stages block, kept structured so the GUI can
/// front each line with its bar-color chip and paint slow lines red;
/// text-only surfaces (analysis prompt, tests) join the `text`s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageLine {
    /// [`STAGE_ORDER`] index; `None` for the unaccounted, transfer, and
    /// nested child lines
    pub stage: Option<usize>,
    /// A record-specific measurement source. `None` uses the generic
    /// stage provenance; kept typed so copied text and hovers agree.
    pub provenance: Option<StageProv>,
    pub text: String,
    /// red: THE biggest machinery wait (at most one per popup)
    pub slow: bool,
    /// bold without red: a secondary over-threshold wait, the dominant
    /// sub-stage under a red parent, or a red-flagged child detail
    pub bold: bool,
    /// front the line with its bar-color chip (footnotes and child
    /// lines go bare)
    pub chip: bool,
    /// composed glossary hover ([`stage_hover`]); child rows carry
    /// their parent stage's, footnote rows carry none
    pub hover: Option<String>,
    /// the stage this line breaks down, for the child rows that explain
    /// part of a parent's number. A reader of `text` sees the nesting in
    /// the leading spaces; a layout that does not print the lines in
    /// order has nothing to read, so the link is a field.
    pub parent: Option<usize>,
}

/// The sub-rows nested under the execution-admission band
/// (`args.re.submit_us` and friends): where the submit window (permit
/// wait, request preparation and connection open) went, and the
/// slots-free context line. Empty when the capture has no rung-2
/// submit span.
fn exec_slot_child_lines(args: &Value) -> Vec<StageLine> {
    let Some(submit) = re_i64(args, "submit_us") else {
        return Vec::new();
    };
    let sem = re_i64(args, "rs_exec_sem_wait_us").unwrap_or(0);
    let prep = re_i64(args, "rs_request_prep_us").unwrap_or(0);
    let rpc = re_i64(args, "rs_rpc_open_us").unwrap_or(0);
    let mut out: Vec<StageLine> = Vec::new();
    let mut child = |text: String| {
        out.push(StageLine {
            stage: None,
            provenance: None,
            text,
            slow: false,
            bold: false,
            chip: false,
            hover: None,
            parent: None,
        });
    };
    child(format!(
        "    inside a submit window of {} (permit wait + request preparation + connection open)",
        fmt_dur(submit),
    ));
    if re_i64(args, "rs_rpc_open_us").is_some() {
        child(format!("    opening the connection {}", fmt_dur(rpc)));
    }
    let other = submit
        .saturating_sub(sem)
        .saturating_sub(rpc)
        .saturating_sub(prep);
    let complete = [
        "rs_exec_sem_wait_us",
        "rs_rpc_open_us",
        "rs_request_prep_us",
    ]
    .iter()
    .all(|key| !invalid_re_counter(args, key));
    if !complete {
        child(
            "    submit-window remainder unavailable: a component counter was invalid".to_owned(),
        );
    } else if other >= 1_000 {
        child(format!("    other {}", fmt_dur(other)));
    } else if other <= -1_000 {
        child(format!(
            "    other {} (sub-stages exceed the window)",
            fmt_dur(other)
        ));
    }
    if re_i64(args, "rs_request_prep_us").is_some() {
        child(format!("    preparing the request {}", fmt_dur(prep)));
    }
    if let Some(free) = re_i64(args, "rs_exec_sem_permits_available") {
        child(format!("    execution slots free when it asked: {free}"));
    }
    out
}

/// Optional phase observations within an existing cache stage. These are
/// inclusive client elapsed measurements, never extra additive stage work.
/// Counts preserve partial handled-failure/cancellation coverage; a missing
/// phase is not inferred to be zero or priced from the parent's remainder.
fn cache_child_lines(args: &Value, hit: bool) -> Vec<StageLine> {
    cache_diagnostic_lines(args, if hit { "cache_hit" } else { "cache_query" })
}

fn cache_diagnostic_lines(args: &Value, key: &str) -> Vec<StageLine> {
    let Some(data) = args
        .get("re")
        .and_then(|r| r.get(key))
        .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
    else {
        return Vec::new();
    };
    let invalid = |key: &str| {
        data.get("invalid_fields")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields.iter().any(|v| v.as_str() == Some(key)))
    };
    let measured = data.get("measured").and_then(Value::as_u64);
    let records = data.get("records").and_then(Value::as_u64);
    let mut out = Vec::new();
    let mut child = |text| {
        out.push(StageLine {
            stage: None,
            provenance: Some(StageProv::Client),
            text,
            slow: false,
            bold: false,
            chip: false,
            hover: None,
            parent: None,
        })
    };
    if let (Some(measured), Some(records)) = (measured, records) {
        if measured < records {
            child(format!(
                "    phase detail recorded for {measured}/{records} stage observations; totals below cover that subset"
            ));
        }
    }
    let fields: &[(&str, &str)] = match key {
        "cache_hit" => &[
            ("tree_admission_us", "output-tree admission"),
            ("tree_fetch_us", "output-tree fetch and typed decode"),
            (
                "local_bookkeeping_us",
                "output metadata construction and ownership claim",
            ),
            ("declare_us", "materializer declaration"),
        ],
        "local_cache" => &[
            ("persisted_probe_us", "persisted-store candidate probe"),
            ("persisted_fetch_us", "persisted-store entry fetch"),
        ],
        _ => &[
            ("permit_wait_us", "waiting for a cache lookup slot"),
            ("sdk_us", "admitted client/SDK operation (not network RTT)"),
        ],
    };
    let mut unavailable = Vec::new();
    for &(key, label) in fields {
        let us = data.get(key).and_then(Value::as_i64).filter(|v| *v >= 0);
        if invalid(key) {
            unavailable.push(format!("{label} (invalid)"));
        } else if let Some(us) = us {
            let coverage = match (
                data.get("field_records")
                    .and_then(|v| v.get(key))
                    .and_then(Value::as_u64),
                measured,
            ) {
                (Some(count), Some(total)) if count < total => {
                    format!(" ({count}/{total} measured records)")
                }
                _ => String::new(),
            };
            child(format!("    {label} {}{coverage}", fmt_dur(us)));
        } else {
            unavailable.push(label.to_owned());
        }
    }
    let count = match key {
        "cache_hit" => Some(("tree_count", "output trees")),
        "local_cache" => Some(("persisted_fetches", "persisted-store candidates fetched")),
        _ => None,
    };
    if let Some((count_key, count_label)) = count {
        if invalid(count_key) {
            unavailable.push(format!("{count_label} (invalid or inconsistent)"));
        } else if let Some(count) = data.get(count_key).and_then(Value::as_u64) {
            let suffix = match (
                data.get("field_records")
                    .and_then(|v| v.get(count_key))
                    .and_then(Value::as_u64),
                measured,
            ) {
                (Some(count), Some(total)) if count < total => {
                    format!(" ({count}/{total} measured records)")
                }
                _ => String::new(),
            };
            child(format!("    {count_label}: {count}{suffix}"));
        } else {
            unavailable.push(count_label.to_owned());
        }
    }
    if !unavailable.is_empty() {
        child(format!("    not recorded: {}", unavailable.join(", ")));
    }
    if key == "local_cache" {
        let mut outcomes = Vec::new();
        if let Some(records) = data.get("outcomes").and_then(Value::as_array) {
            for record in records {
                let Some(pair) = record.as_array().filter(|pair| pair.len() == 2) else {
                    continue;
                };
                let (Some(outcome), Some(count)) = (pair[0].as_i64(), pair[1].as_u64()) else {
                    continue;
                };
                let name = match outcome {
                    0 => "outcome not recorded".to_owned(),
                    1 => "miss".to_owned(),
                    2 => "in-memory hit".to_owned(),
                    3 => "persisted hit".to_owned(),
                    4 => "error".to_owned(),
                    other => format!("unknown outcome {other}"),
                };
                outcomes.push(format!("{count} {name}"));
            }
        }
        if !outcomes.is_empty() {
            child(format!(
                "    check outcomes (observations): {}",
                outcomes.join(", ")
            ));
        }
        child(
            "    in-memory describes the cache entry, not the elapsed validation cost".to_owned(),
        );
    }
    child(
        "    inclusive client phase observations; not additional work or a complete partition"
            .to_owned(),
    );
    out
}

/// Whether this record carries a valid input-upload phase or digest counter.
/// Legacy byte totals alone do not establish phase instrumentation.
pub(crate) fn iu_instrumented(args: &Value) -> bool {
    // Older emitters dropped zero timings. Current emitters preserve
    // measured zeros and explicitly identify invalid counters.
    [
        "iu_sem_wait_us",
        "iu_cas_call_us",
        "iu_find_missing_us",
        "iu_materialize_us",
        "iu_digests_absent",
        "iu_digests_queried",
    ]
    .iter()
    .any(|k| re_i64(args, k).is_some())
}

/// The recorded activity intervals, `[name, offset_us, duration_us, cancelled]` each.
fn activity_spans(args: &Value) -> impl Iterator<Item = (&str, i64, i64, bool)> {
    args.pointer("/activity/spans")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|span| {
            let span = span.as_array()?;
            Some((
                span.first()?.as_str()?,
                span.get(1)?.as_i64()?,
                span.get(2)?.as_i64()?.max(0),
                span.get(3).and_then(Value::as_bool).unwrap_or(false),
            ))
        })
}

/// The side of a hybrid race that buck2 cancelled when the other side finished: whether it was
/// the local attempt, and the stage it was in (its name, offset and duration).
fn cancelled_attempt(args: &Value) -> Option<(bool, &str, i64, i64)> {
    activity_spans(args)
        .filter(|(name, _, _, cancelled)| {
            *cancelled && (name.starts_with("Local.") || name.starts_with("Re."))
        })
        .max_by_key(|(_, offset, dur, _)| offset.saturating_add(*dur))
        .map(|(name, offset, dur, _)| (name.starts_with("Local."), name, offset, dur))
}

/// A cancelled attempt's stage, in words.
/// A plain-language name for the stage a hybrid attempt was in.
pub fn attempt_phase(stage: &str) -> &str {
    match stage {
        "Local.MaterializeInputs" => "preparing inputs on this machine",
        "Local.Execute" | "Local.WorkerExecute" => "running the command",
        "Local.Queued" | "Local.AcquireLocalResource" => "waiting for a local slot",
        "Re.Queue" => "queued in RE",
        "Re.WorkerDownload" => "the worker was preparing inputs",
        "Re.Execute" => "running on the worker",
        "Re.WorkerUpload" => "the worker was uploading outputs",
        "Re.Download" => "downloading outputs",
        other => other,
    }
}

const CANCELLED_ATTEMPT_HELP: &str = "buck2 ran this action locally and remotely at once and kept \
    whichever finished first; this is the attempt it cancelled. Its time ran alongside the stages \
    above and is not part of them.";

const CANCELLED_LOCAL_INPUTS_HELP: &str = "buck2 ran this action locally and remotely at once and \
    kept whichever finished first; this is the attempt it cancelled. Its time ran alongside the \
    stages above and is not part of them. Inputs it was fetching keep arriving in the background \
    after the cancel, and use this machine's network and disk.";

/// How long the upload overlapped the cancelled local attempt preparing the same inputs, if it did.
fn upload_overlap_with_local_inputs(args: &Value) -> Option<i64> {
    let (local, stage, offset, dur) = cancelled_attempt(args)?;
    if !local || stage != "Local.MaterializeInputs" {
        return None;
    }
    let overlap: i64 = activity_spans(args)
        .filter(|(name, _, _, cancelled)| *name == "ReUpload" && !cancelled)
        .map(|(_, up_offset, up_dur, _)| {
            (up_offset.saturating_add(up_dur)).min(offset.saturating_add(dur))
                - up_offset.max(offset)
        })
        .filter(|&o| o > 0)
        .sum();
    (overlap > 0).then_some(overlap)
}

/// The upload stage's instrumented sub-rows. `iu_inflight` is the modeled
/// admission depth supplied by the global upload-window pass.
fn upload_child_lines(args: &Value, parent_slow: bool, iu_inflight: Option<i64>) -> Vec<StageLine> {
    if !iu_instrumented(args) {
        return Vec::new();
    }
    let iu = |k: &str| re_i64(args, k);
    let cas_label = if iu("iu_bytes_uploaded") == Some(0) {
        "CAS call (zero-byte batch; no transfer priced)"
    } else {
        "writing new bytes"
    };
    let subs = [
        ("iu_sem_wait_us", "waiting for an upload slot"),
        ("iu_find_missing_us", "checking what storage has"),
        ("iu_materialize_us", "gathering local files to send"),
        ("iu_cas_call_us", cas_label),
    ];
    let dominant = subs.iter().filter_map(|(k, _)| iu(k)).max().unwrap_or(0);
    let mut out: Vec<StageLine> = Vec::new();
    for (k, lbl) in subs {
        let Some(v) = iu(k) else { continue };
        out.push(StageLine {
            stage: None,
            provenance: None,
            text: format!("    {lbl} {}", fmt_dur(v)),
            slow: false,
            // one level deeper than the parent's red: the dominant
            // sub-row bolds, teaching "upload time = queue time" at
            // a glance without a second red line
            bold: parent_slow && v > 0 && v == dominant,
            chip: false,
            hover: None,
            parent: None,
        });
        if k == "iu_materialize_us"
            && let Some(query) = iu("iu_materializer_query_us")
        {
            out.push(StageLine {
                stage: None,
                provenance: None,
                text: format!(
                    "      of which asking the materializer where each file is {}",
                    fmt_dur(query)
                ),
                slow: false,
                bold: false,
                chip: false,
                hover: None,
                parent: None,
            });
        }
        if k == "iu_materialize_us"
            && v > 0
            && let Some(overlap) = upload_overlap_with_local_inputs(args)
        {
            out.push(StageLine {
                stage: None,
                provenance: None,
                text: format!(
                    "      while the local attempt was preparing this action's inputs on this \
                     machine (overlapping {})",
                    fmt_dur(overlap)
                ),
                slow: false,
                bold: false,
                chip: false,
                hover: None,
                parent: None,
            });
        }
    }
    // Native producers always emit this capture marker, but omit zero
    // digest counts. A flagged value is unknown, not a suppressed zero.
    let zero_suppressed_counts = iu("iu_digests_queried").is_some();
    let count = |key| {
        iu(key)
            .or_else(|| {
                (zero_suppressed_counts
                    && !invalid_re_counter(args, key)
                    && args.get("re").and_then(|re| re.get(key)).is_none())
                .then_some(0)
            })
            .map_or_else(|| "unavailable".to_owned(), |v| v.to_string())
    };
    let ab = count("iu_digests_absent");
    let checked = iu("iu_digests_queried")
        .filter(|_| ab != "unavailable")
        .map(|q| format!(" of {}", crate::analyses::fmt_n(q as f64)))
        .unwrap_or_default();
    let sent = iu("iu_bytes_uploaded")
        .map(|bytes| format!("sent {} \u{2014} ", fmt_bytes_f(bytes as f64)))
        .unwrap_or_default();
    out.push(StageLine {
        stage: None,
        provenance: None,
        text: format!("    {sent}{ab}{checked} digests not in storage"),
        slow: false,
        bold: false,
        chip: false,
        hover: None,
        parent: None,
    });
    if let Some(n) = iu_inflight {
        out.push(StageLine {
            stage: None,
            provenance: None,
            text: format!(
                "    other modeled upload bands at admission: ~{} (not queue depth)",
                crate::analyses::fmt_n(n as f64)
            ),
            slow: false,
            bold: false,
            chip: false,
            hover: None,
            parent: None,
        });
    }
    out
}

/// The daemon band's positioned gap map (`args.re.gap_*`): where the
/// residual sits relative to observed client spans, plus how much of the
/// first and last gaps the recorder measured as preparing the command and
/// finishing the action. The rest of the residual stays unnamed. Empty when
/// the record has no gap fields.
fn daemon_child_lines(args: &Value, parent_red: bool) -> Vec<StageLine> {
    let Some(re) = args.get("re") else {
        return Vec::new();
    };
    let g = |k: &str| re.get(k).and_then(|v| v.as_i64());
    if g("gap_pre_us").is_none() {
        return Vec::new();
    }
    let subs = [
        ("gap_pre_us", "before the first observed client span"),
        ("gap_inter_us", "between observed client spans"),
        ("gap_post_us", "after the last observed client span"),
    ];
    // For `run` actions the recorder measures preparing the command, before
    // the cache check, and finishing the action, after the result arrived.
    let measured = |k: &str| match k {
        "gap_pre_us" => g("prepare_command_us").map(|us| ("preparing the command", us)),
        "gap_post_us" => g("finish_action_us").map(|us| ("finishing the action", us)),
        _ => None,
    };
    let dominant = subs.iter().filter_map(|(k, _)| g(k)).max().unwrap_or(0);
    let mut out = Vec::new();
    for (k, lbl) in subs {
        let Some(v) = g(k) else { continue };
        if v == 0 {
            continue;
        }
        let mut text = format!("    {lbl} {}", fmt_dur(v));
        if let Some((what, us)) = measured(k) {
            text.push_str(&format!(", of which {what} {}", fmt_dur(us.min(v))));
        }
        out.push(StageLine {
            stage: None,
            provenance: None,
            text,
            slow: false,
            bold: parent_red && v == dominant,
            chip: false,
            hover: None,
            parent: None,
        });
    }
    out
}

/// The Stages block, or `None` when the record has no `args.stages`.
/// `fetch_med_bps` is the build-median effective input-preparation rate for
/// logical input sets larger than 10 MiB (a global pass supplies it).
/// This uncontrolled comparison does not diagnose network throughput;
/// > `iu_inflight` is the admission-depth estimate for the nested
/// > queue-depth line on instrumented captures.
pub fn stage_lines(
    args: &Value,
    dur: i64,
    fetch_med_bps: Option<f64>,
    iu_inflight: Option<i64>,
) -> Option<Vec<StageLine>> {
    let st = stages_of(args)?;
    let marks = slow_stage_marks(dur, &st);
    let red_idx = marks.iter().position(|m| *m == StageMark::Red);
    let mut iu_children =
        upload_child_lines(args, marks[STAGE_IU_IDX] == StageMark::Red, iu_inflight);
    let mut slot_children = exec_slot_child_lines(args);
    let mut query_children = cache_child_lines(args, false);
    let mut hit_children = cache_child_lines(args, true);
    let mut local_cache_children = cache_diagnostic_lines(args, "local_cache");
    let included_hashing = hashing_in_finalize_outputs(args)
        .then(|| opt_stage_us(args, "hashing_us"))
        .flatten();
    let mut daemon_children = daemon_child_lines(args, marks[STAGE_DAEMON_IDX] == StageMark::Red);
    // A local action prepares inputs using this machine's materializer,
    // not a worker sandbox. Neither duration is pure network transfer.
    let local_fetch = args
        .get("kind")
        .and_then(|v| v.as_str())
        .is_some_and(|k| k.starts_with("local"));
    let bin = num(args, "bytes_in");
    let mut lines: Vec<StageLine> = Vec::new();
    if args
        .get("re")
        .and_then(|re| re.get("invalid_counter_fields"))
        .and_then(Value::as_array)
        .is_some_and(|keys| !keys.is_empty())
    {
        lines.push(StageLine {
            stage: None,
            provenance: None,
            text:
                "Invalid counters are unavailable, not zero; valid phase measurements are retained."
                    .to_owned(),
            slow: false,
            bold: false,
            chip: false,
            hover: None,
            parent: None,
        });
    }
    for (i, us) in st.iter().enumerate() {
        if *us == 0 {
            // zero admission wait on a record that still carries a
            // submit window is a signal, not an absence: the window's
            // decomposition (prep, rpc, other) must not vanish with it
            if !((i == STAGE_SLOT_IDX && !slot_children.is_empty())
                || (i == STAGE_LOOKUP_IDX && !query_children.is_empty())
                || (i == STAGE_CACHE_HIT_IDX && !hit_children.is_empty())
                || (i == STAGE_LOCAL_CACHE_IDX && !local_cache_children.is_empty())
                || (i == STAGE_FINALIZE_IDX && included_hashing.is_some()))
            {
                continue;
            }
        }
        let label = if i == STAGE_FETCH_IDX && local_fetch {
            "local input preparation (CAS)"
        } else if i == STAGE_HASH_IDX
            && args
                .pointer("/stages/hashing_source")
                .and_then(Value::as_str)
                == Some("local_outputs")
        {
            "local output hashing"
        } else {
            STAGE_LABEL[i]
        };
        let mut text = if matches!(
            i,
            STAGE_LOOKUP_IDX | STAGE_CACHE_HIT_IDX | STAGE_LOCAL_CACHE_IDX
        ) && opt_stage_us(args, STAGE_ORDER[i]).is_none()
        {
            format!("{label} not recorded (phase details below)")
        } else if i == STAGE_SLOT_IDX && invalid_re_counter(args, "rs_exec_sem_wait_us") {
            "execution admission not measured (invalid counter)".to_owned()
        } else {
            format!("{} {}", label, fmt_dur(*us))
        };
        match marks[i] {
            StageMark::Red if dur > 0 => {
                // not color-alone: the share suffix keeps the callout
                // readable for CVD users and in grayscale screenshots
                text.push_str(&format!(
                    "  ({}% of wall)",
                    (*us).saturating_mul(100) / dur.max(1)
                ));
            }
            StageMark::Bold if dur > 0 => {
                // a second independent "% of wall" next to the red one
                // reads as a >100% action. The sum establishes counter
                // excess, not proof that the stages ran concurrently.
                match red_idx {
                    Some(r) if st[r].saturating_add(*us) > dur => {
                        text.push_str(&format!("  (with {}, counters exceed wall)", STAGE_CHIP[r]));
                    }
                    _ => text.push_str(&format!(
                        "  ({}% of wall)",
                        (*us).saturating_mul(100) / dur.max(1)
                    )),
                }
            }
            _ => {}
        }
        let provenance = stage_prov_for_args(i, args);
        let hover = if i == STAGE_HASH_IDX {
            stage_hover_for_args(i, args)
        } else {
            stage_hover_with_provenance(i, provenance)
        };
        // a child row explains part of its parent's number, so it
        // answers to the parent's definition and caveat
        let adopt = |kids: &mut Vec<StageLine>| {
            for c in kids.iter_mut() {
                c.hover = Some(hover.clone());
                c.parent = Some(i);
            }
        };
        lines.push(StageLine {
            stage: Some(i),
            provenance: Some(provenance),
            text,
            slow: marks[i] == StageMark::Red,
            bold: marks[i] == StageMark::Bold,
            chip: true,
            hover: Some(hover.clone()),
            parent: None,
        });
        if i == STAGE_IU_IDX {
            adopt(&mut iu_children);
            lines.append(&mut iu_children);
        }
        if i == STAGE_LOOKUP_IDX {
            adopt(&mut query_children);
            lines.append(&mut query_children);
        }
        if i == STAGE_CACHE_HIT_IDX {
            adopt(&mut hit_children);
            lines.append(&mut hit_children);
        }
        if i == STAGE_SLOT_IDX {
            adopt(&mut slot_children);
            lines.append(&mut slot_children);
        }
        if i == STAGE_DAEMON_IDX {
            adopt(&mut daemon_children);
            lines.append(&mut daemon_children);
        }
        if i == STAGE_LOCAL_CACHE_IDX {
            adopt(&mut local_cache_children);
            lines.append(&mut local_cache_children);
        }
        if i == STAGE_FINALIZE_IDX
            && let Some(hash_us) = included_hashing
        {
            lines.push(StageLine {
                stage: None,
                provenance: Some(StageProv::DurationSum),
                text: format!("    local output hashing {} (sum of file durations; included above, may overlap)", fmt_dur(hash_us)),
                slow: false,
                bold: false,
                chip: false,
                hover: Some(stage_hover_for_args(STAGE_HASH_IDX, args)),
                parent: Some(i),
            });
        }
        if i == STAGE_FETCH_IDX && bin != 0.0 {
            // this stage's own data lives under it, not two screens
            // away: bytes, then the effective rate in build context
            lines.push(StageLine {
                stage: None,
                provenance: None,
                text: format!(
                    "    logical inputs {} (may be cache-resident)",
                    fmt_bytes_f(bin)
                ),
                slow: false,
                bold: false,
                chip: false,
                hover: Some(hover.clone()),
                parent: Some(i),
            });
            if !local_fetch && let Some(downloaded) = re_i64(args, "worker_bytes_in") {
                let downloaded = downloaded as f64;
                lines.push(StageLine {
                    stage: None,
                    provenance: None,
                    text: if downloaded < bin {
                        format!(
                            "    worker downloaded {}; the rest was already cached on the worker",
                            fmt_bytes_f(downloaded)
                        )
                    } else {
                        format!("    worker downloaded {}", fmt_bytes_f(downloaded))
                    },
                    slow: false,
                    bold: false,
                    chip: false,
                    hover: Some(hover.clone()),
                    parent: Some(i),
                });
            }
            let rate = if *us >= 1000 {
                Some(bin / (*us as f64 / 1e6))
            } else {
                None
            };
            if let Some(rate) = rate {
                let mut text = format!(
                    "    effective input preparation rate {}/s (not network bandwidth)",
                    fmt_bytes_f(rate)
                );
                if let Some(med) = fetch_med_bps {
                    if med.is_finite() && med > 0.0 && bin > 10.0 * 1024.0 * 1024.0 {
                        text.push_str(&format!(
                            " \u{2014} build median for >10 MiB logical input sets: {}/s; \
                             input trees, caches and workers can differ",
                            fmt_bytes_f(med)
                        ));
                    }
                }
                lines.push(StageLine {
                    stage: None,
                    provenance: None,
                    text,
                    slow: false,
                    bold: false,
                    chip: false,
                    hover: Some(hover.clone()),
                    parent: Some(i),
                });
            }
        }
    }
    if let Some((local, stage, _, cancelled_us)) = cancelled_attempt(args) {
        lines.push(StageLine {
            stage: None,
            provenance: None,
            text: format!(
                "{} attempt, cancelled when the {} result arrived: {} {} (alongside the stages \
                 above, not included in them)",
                if local { "local" } else { "remote" },
                if local { "remote" } else { "local" },
                attempt_phase(stage),
                fmt_dur(cancelled_us)
            ),
            slow: false,
            bold: false,
            chip: false,
            hover: Some(
                if local && stage == "Local.MaterializeInputs" {
                    CANCELLED_LOCAL_INPUTS_HELP
                } else {
                    CANCELLED_ATTEMPT_HELP
                }
                .to_owned(),
            ),
            parent: None,
        });
    }
    let known: i64 = st.iter().fold(0i64, |a, &v| a.saturating_add(v));
    if dur > known {
        lines.push(StageLine {
            stage: None,
            provenance: None,
            text: format!(
                "counter remainder {} (not a positioned wait)",
                fmt_dur(dur.saturating_sub(known))
            ),
            slow: false,
            bold: false,
            chip: true,
            hover: Some(UNACCOUNTED_HELP.to_owned()),
            parent: None,
        });
    }
    if bin != 0.0 && st[STAGE_FETCH_IDX] == 0 {
        // no fetch stage to nest under: keep the labeled standalone form
        lines.push(StageLine {
            stage: None,
            provenance: None,
            text: format!(
                "logical inputs {}{} (may be cache-resident; not network bandwidth)",
                fmt_bytes_f(bin),
                rate_str(
                    "effective input preparation rate",
                    bin,
                    opt_stage_us(args, "materialize_us")
                )
            ),
            slow: false,
            bold: false,
            chip: true,
            hover: None,
            parent: None,
        });
    }
    let bout = num(args, "bytes_out");
    if bout != 0.0 {
        lines.push(StageLine {
            stage: None,
            provenance: None,
            text: format!(
                "outputs {}{}",
                fmt_bytes_f(bout),
                rate_str(
                    "effective output preparation rate, not network bandwidth:",
                    bout,
                    opt_stage_us(args, "upload_us")
                )
            ),
            slow: false,
            bold: false,
            chip: true,
            hover: None,
            parent: None,
        });
    }
    if known as f64 > dur as f64 * 1.05 {
        // Excess counters may overlap or be inconsistent; durations
        // alone do not establish concurrency or chronological positions.
        lines.push(StageLine {
            stage: None,
            provenance: None,
            text: "(stage counters exceed wall time \u{2014} the bar shows shares of \
                total counters, not a timeline)"
                .to_owned(),
            slow: false,
            bold: false,
            chip: false,
            hover: None,
            parent: None,
        });
    }
    Some(lines)
}

fn opt_stage_us(args: &Value, key: &str) -> Option<i64> {
    args.get("stages")
        .and_then(|s| s.get(key))
        .and_then(|v| v.as_i64())
        .filter(|&v| v >= 0)
}

/// A value from the instrumented `args.re` object.
pub(crate) fn re_i64(args: &Value, key: &str) -> Option<i64> {
    if invalid_re_counter(args, key) {
        return None;
    }
    args.get("re")
        .and_then(|re| re.get(key))
        .and_then(|v| v.as_i64())
        .filter(|&v| v >= 0)
}

fn invalid_re_counter(args: &Value, key: &str) -> bool {
    args.get("re")
        .and_then(|re| re.get("invalid_counter_fields"))
        .and_then(Value::as_array)
        .is_some_and(|keys| keys.iter().any(|value| value.as_str() == Some(key)))
}

/// Optional observations, deliberately outside the stage-counter taxonomy.
/// These intervals can overlap existing counters and the canceled subset must
/// not be added to its enclosing worker-download-state union.
fn re_observations(args: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(us) = re_i64(args, "worker_download_wait_us") {
        lines.push(format!(
            "observed remote worker-download state {} (client-side elapsed spans)",
            fmt_dur(us)
        ));
    }
    if let Some(us) = re_i64(args, "worker_download_cancelled_wait_us") {
        lines.push(format!(
            "canceled portion {} (subset of the worker-download state; does not identify cancellation cause)",
            fmt_dur(us)
        ));
    }
    if !lines.is_empty() {
        lines.push(
            "State waits may overlap local work; they are not physical network service or \
             additional stage-counter budget. Do not add the canceled subset again."
                .to_owned(),
        );
    }
    if let Some(us) = re_i64(args, "reported_first_enqueue_to_worker_us") {
        lines.push(format!(
            "reported first-enqueue to final-worker interval {} (legacy scheduling interval, \
             not queue wait; may include worker activity and retries)",
            fmt_dur(us)
        ));
    }
    if let Some(attempts) = re_i64(args, "execution_attempts") {
        let mut line = format!("RE attempted the action {attempts} times");
        if let Some(us) = re_i64(args, "final_queue_us") {
            line.push_str(&format!("; the final attempt queued {}", fmt_dur(us)));
        }
        lines.push(line);
    }
    if let Some(keys) = args
        .get("re")
        .and_then(|re| re.get("invalid_counter_fields"))
        .and_then(Value::as_array)
    {
        if !keys.is_empty() {
            lines.push(format!(
                "{} invalid or overflowed counters were omitted, not measured as zero. Valid phase measurements remain available; incomplete phase totals cannot explain the remaining interval.",
                keys.len()
            ));
        }
    }
    if let Some(count) = re_i64(args, "invalid_cache_timing_records").filter(|n| *n > 0) {
        lines.push(format!(
            "{count} contradictory cache phase record(s) were omitted; missing phase detail is not measured zero."
        ));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// The stage durations in [`STAGE_ORDER`], or `None` without a stages
/// object and at least one recognized current counter. Presence is
/// checked after historical execution is removed; an explicit zero
/// still counts. The synthetic execution-slot counter comes from
/// `args.re.rs_exec_sem_wait_us` but also requires the stages object.
pub fn stages_of(args: &Value) -> Option<[i64; STAGE_N]> {
    args.get("stages")?.as_object()?;
    let mut out = [0i64; STAGE_N];
    let mut measured = false;
    for (i, k) in STAGE_ORDER.iter().enumerate() {
        let value = if i == STAGE_EXEC_IDX {
            crate::analyses::current_execution_us(
                opt_stage_us(args, k),
                args.get("cache").is_some_and(Value::is_object),
                args.get("execution_metadata").and_then(Value::as_str),
                args.get("stages")
                    .and_then(|s| s.get("execute_source"))
                    .and_then(Value::as_str),
            )
        } else if i == STAGE_SLOT_IDX {
            re_i64(args, "rs_exec_sem_wait_us")
        } else {
            opt_stage_us(args, k)
        };
        if let Some(value) = value {
            measured = true;
            out[i] = value;
        }
    }
    out[STAGE_HASH_IDX] = crate::analyses::hashing_composition_us(
        out[STAGE_HASH_IDX],
        hashing_in_finalize_outputs(args),
    );
    measured.then_some(out)
}

fn hashing_in_finalize_outputs(args: &Value) -> bool {
    crate::analyses::hashing_in_finalize_outputs(
        args.pointer("/stages/hashing_source")
            .and_then(Value::as_str),
        opt_stage_us(args, "local_finalize_outputs_us"),
    )
}

fn num(args: &Value, key: &str) -> f64 {
    args.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

/// The data an action moved, from its record's `args`: the logical sizes of
/// its inputs and outputs, and the transfers RE measured when it executed the
/// action. `None` is "not recorded", never zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DataMoved {
    /// Every input the executor needed present (`bytes_in`). Some of it may
    /// already have been there, so this bounds the transfer rather than
    /// measuring it.
    pub inputs: Option<f64>,
    /// `bytes_out`
    pub outputs: Option<f64>,
    /// What the client uploaded to CAS for the inputs (`re.iu_bytes_uploaded`).
    pub uploaded: Option<f64>,
    /// What the RE worker downloaded for inputs (`re.worker_bytes_in`).
    pub worker_in: Option<f64>,
    /// What the RE worker uploaded for outputs (`re.worker_bytes_out`).
    pub worker_out: Option<f64>,
}

impl DataMoved {
    pub fn of(args: &Value) -> Self {
        let bytes = |p: &str| {
            args.pointer(p)
                .and_then(Value::as_f64)
                .filter(|b| b.is_finite() && *b >= 0.0)
        };
        Self {
            inputs: bytes("/bytes_in"),
            outputs: bytes("/bytes_out"),
            uploaded: bytes("/re/iu_bytes_uploaded"),
            worker_in: bytes("/re/worker_bytes_in"),
            worker_out: bytes("/re/worker_bytes_out"),
        }
    }

    /// One line per number recorded for the inputs, for a hover.
    pub fn inputs_note(&self) -> String {
        let mut lines = vec![match self.inputs {
            Some(b) => format!(
                "{} of inputs had to be present where the action ran (some may already have been)",
                fmt_bytes_f(b)
            ),
            None => "input size not recorded".to_owned(),
        }];
        if let Some(b) = self.uploaded {
            lines.push(format!("{} uploaded to CAS by this client", fmt_bytes_f(b)));
        }
        if let Some(b) = self.worker_in {
            lines.push(format!("{} downloaded by the RE worker", fmt_bytes_f(b)));
        }
        lines.join("\n")
    }

    /// One line per number recorded for the outputs, for a hover.
    pub fn outputs_note(&self) -> String {
        let mut lines = vec![match self.outputs {
            Some(b) => format!("{} of outputs", fmt_bytes_f(b)),
            None => "output size not recorded".to_owned(),
        }];
        if let Some(b) = self.worker_out {
            lines.push(format!("{} uploaded by the RE worker", fmt_bytes_f(b)));
        }
        lines.join("\n")
    }
}

fn int_of(args: &Value, key: &str) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

fn str_of<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn rate_str(verb: &str, nbytes: f64, us: Option<i64>) -> String {
    match us {
        Some(us) if nbytes != 0.0 && us >= 1000 => {
            format!(" ({verb} {}/s)", fmt_bytes_f(nbytes / (us as f64 / 1e6)))
        }
        _ => String::new(),
    }
}

// ------------------------------------------------------------ popup rows

/// One `key: value` line of the detail popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: &'static str,
    pub value: String,
    /// `Result` row of a failed action: the value cell renders red.
    pub failed: bool,
}

impl Row {
    fn plain(key: &'static str, value: String) -> Row {
        Row {
            key,
            value,
            failed: false,
        }
    }
}

/// Everything the popup needs that is not in the record itself.
#[derive(Clone, Debug, Default)]
pub struct PopupCtx {
    /// absolute µs of the first event
    pub t0: i64,
    /// build-median effective preparation rate for >10 MiB logical input sets
    /// (bytes/s); `None` until the analysis pass has run
    pub fetch_med_bps: Option<f64>,
    /// admission-depth estimate for the nested queue-depth line; `None`
    /// until the analysis pass has run (or on uninstrumented captures)
    pub iu_inflight: Option<i64>,
}

/// The graph-context keys the strip above the stage bar lifts out of the
/// "Graph & context" grid. One list, so the strip and the grid's filter
/// cannot drift into showing the same line twice or losing it entirely.
pub const CONTEXT_STRIP_KEYS: [&str; 2] = ["Critical path", "Among peers"];

/// Kept separate from `Stages` so overlapping state observations cannot be
/// mistaken for another additive band by either the popup or copied prompt.
pub const RE_OBSERVATIONS_KEY: &str = "RE observations";

/// Where this action sits relative to the critical path and to its
/// peers, on one line. Composed from the values the grid would show, so
/// the two cannot disagree. `None` when neither is known — the caller
/// prints nothing rather than a placeholder.
///
/// Rank by raw duration is deliberately not here: on a build whose
/// longest actions are off the critical path it reads as a list of
/// things to fix. Slack leads instead, because it states what the
/// reader would otherwise assume.
pub fn context_strip(extra: &[(String, String)]) -> Option<String> {
    let parts: Vec<&str> = CONTEXT_STRIP_KEYS
        .iter()
        .filter_map(|k| {
            extra
                .iter()
                .find(|(kk, _)| kk == k)
                .map(|(_, v)| v.as_str())
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" \u{2014} "))
}

/// The action's recorded command line, or `None` when the event log
/// carries no usable `argv` — routine on remote captures, where the
/// digest is the only way back to the command. Every surface that offers
/// the command has to agree with the Action row about whether there is
/// one, so they all ask here.
pub fn command_argv(rec: &Value) -> Option<Vec<String>> {
    let items = rec.get("args")?.get("argv")?.as_array()?;
    let argv: Vec<String> = items
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    // Older converters used this sentinel when no remote argv was present.
    // It is not a command and must never be offered as a reproducer.
    (argv.first().map(String::as_str) != Some("<remote>")).then_some(argv)
}

/// How buck2 built a local command's environment (`args.env_rule`), before
/// setting the variables the action declares.
fn environment_rule_text(rule: &str) -> String {
    match rule {
        "inherit_except_exclusions" => {
            "buck2's own environment, minus a fixed list of variables known to break commands \
             (such as LD_PRELOAD and PYTHONPATH), plus the variables the action sets"
                .to_owned()
        }
        "test_allowlist" => {
            "only the allow-listed variables buck2 had, plus the variables the action sets"
                .to_owned()
        }
        "cleared" => "only the variables the action sets".to_owned(),
        other => format!("recorded as `{other}`"),
    }
}

/// The popup's row list, top to bottom.
pub fn popup_rows(rec: &Value, ctx: &PopupCtx) -> Vec<Row> {
    let empty = Value::Object(Default::default());
    let a = rec.get("args").unwrap_or(&empty);
    let name = rec.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let ts = rec.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
    let dur = rec.get("dur").and_then(|v| v.as_i64()).unwrap_or(0);
    let exit = int_of(a, "exit");
    let sig = int_of(a, "sig");
    let failed = exit != 0 || sig != 0;
    let src = str_of(a, "src").to_owned();
    let action_txt = match command_argv(rec) {
        Some(argv) if argv.is_empty() => "(recorded command argument list is empty)".to_owned(),
        Some(argv) => crate::analyses::shlex_join(&argv),
        None => "(command arguments not recorded in this trace row)".to_owned(),
    };

    let mut rows: Vec<Row> = Vec::with_capacity(20);
    rows.push(Row::plain("Target", name.to_owned()));
    rows.push(Row::plain("Action", action_txt));
    if let Some(dir) = a.get("cwd").and_then(|v| v.as_str()) {
        rows.push(Row::plain(
            "Working directory",
            format!("{dir}/ (relative to the project root)"),
        ));
    }
    if let Some(rule) = a.get("env_rule").and_then(|v| v.as_str()) {
        rows.push(Row::plain("Environment", environment_rule_text(rule)));
    }
    rows.push(Row::plain(
        "Category",
        format!(
            "{} \u{2014} {}",
            if src.is_empty() { "(unrecorded)" } else { &src },
            crate::analyses::classify_type(&src, name)
        ),
    ));
    rows.push(Row::plain(
        "Started",
        format!(
            "+{} from selected run origin",
            fmt_dur(ts.saturating_sub(ctx.t0))
        ),
    ));
    rows.push(Row::plain("Wall time", fmt_dur(dur)));
    if let Some(k) = a.get("kind").and_then(|v| v.as_str()) {
        rows.push(Row::plain("Executor", k.to_owned()));
    }
    if failed {
        rows.push(Row {
            key: "Result",
            value: if exit != 0 {
                format!("FAILED exit {exit}")
            } else {
                format!("signal {sig}")
            },
            failed: true,
        });
    }
    rows.push(Row::plain(
        "Digest",
        a.get("digest")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned(),
    ));

    if let Some(lines) = stage_lines(a, dur, ctx.fetch_med_bps, ctx.iu_inflight) {
        // one stage per line: the eye scans a column, not a wrapped
        // sentence (the GUI re-renders these lines with color chips,
        // slow highlights and per-row hovers; this joined form feeds
        // text surfaces, which is why provenance is spelled out here
        // and nowhere else)
        let local_fetch = a
            .get("kind")
            .and_then(|v| v.as_str())
            .is_some_and(|k| k.starts_with("local"));
        let mut value = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        value.push('\n');
        value.push_str(&stages_prov_line(&lines, local_fetch));
        insert_after(&mut rows, "Wall time", Row::plain("Stages", value));
    }
    if let Some(value) = re_observations(a) {
        let after = if rows.iter().any(|row| row.key == "Stages") {
            "Stages"
        } else {
            "Wall time"
        };
        insert_after(&mut rows, after, Row::plain(RE_OBSERVATIONS_KEY, value));
    }
    if let Some(why) = a.get("why").and_then(|v| v.as_str()) {
        if !why.is_empty() {
            insert_after(&mut rows, "Action", Row::plain("Why run", why.to_owned()));
        }
    }
    // inserted immediately before Digest, in call order
    if let Some(ca) = a.get("cache") {
        let mut v = "HIT — reused cached outputs".to_owned();
        if a.pointer("/stages/execute_source").and_then(Value::as_str) == Some("client_span") {
            v.push_str(" after a recorded local attempt");
        }
        for (key, label) in [("lookup_us", "lookup"), ("restore_us", "restore")] {
            if let Some(us) = ca
                .get(key)
                .and_then(Value::as_f64)
                .filter(|us| us.is_finite() && *us >= 0.0)
            {
                v.push_str(&format!("; {label} {}", fmt_dur_f(us)));
            }
        }
        if let Some(bytes) = ca
            .get("bytes")
            .and_then(Value::as_f64)
            .filter(|bytes| bytes.is_finite() && *bytes >= 0.0)
        {
            v.push_str(&format!("; {} logical outputs", fmt_bytes_f(bytes)));
        }
        insert_before_last(&mut rows, Row::plain("Cache", v));
    }
    for (key, label) in [
        ("cache_upload_result", "Action cache write"),
        ("dep_file_cache_upload_result", "Dep-file cache write"),
    ] {
        if let Some(value) = a.get(key) {
            insert_before_last(&mut rows, Row::plain(label, cache_upload_outcome(value)));
        }
    }
    if a.get("maxrss").is_some() {
        insert_before_last(
            &mut rows,
            Row::plain(
                "Resources",
                format!(
                    "{}peak RSS {} MiB",
                    if a.get("cache").is_some() {
                        "historical reused result: "
                    } else {
                        ""
                    },
                    int_of(a, "maxrss").div_euclid(1 << 20)
                ),
            ),
        );
    }
    if let Some(re) = a.get("re") {
        // only fields the Stages block cannot show: numbers measured on
        // the server, not client-side spans of the same windows
        let mut parts: Vec<String> = Vec::new();
        for (k, lbl) in [
            ("server_upload_us", "server upload"),
            ("worker_wall_us", "worker wall"),
        ] {
            if let Some(v) = re.get(k).and_then(|v| v.as_i64()) {
                parts.push(format!("{lbl} {}", fmt_dur(v)));
            }
        }
        if !parts.is_empty() {
            insert_before_last(
                &mut rows,
                Row::plain("Remote", format!("server-reported: {}", parts.join(", "))),
            );
        }
        // The instrumented inputs-upload decomposition renders as nested
        // sub-rows of the Stages block ([`upload_child_lines`]), not as
        // a separate row down here — it IS that stage's breakdown.
    }
    // Resource accounting does not diagnose why elapsed time exceeded
    // CPU time. Cache metadata may belong to an earlier execution.
    if let (Some(cpu), Some(st)) = (
        a.get("cpu_us").and_then(|v| v.as_i64()),
        stages_of(a).map(|st| st[STAGE_EXEC_IDX]),
    ) && cpu > 0
        && st > 0
        && a.get("cache").is_none()
    {
        let pct = i128::from(cpu) * 100 / i128::from(st);
        let mut line = format!(
            "reported execution metadata: {} of CPU over {} of execution = {pct}% of one core",
            fmt_dur(cpu),
            fmt_dur(st)
        );
        // Neither scheduler contention nor any other cause of low
        // CPU share is established by these two counters alone.
        if pct < 80 {
            line.push_str(
                " — low CPU share; possible I/O, synchronization or scheduling waits \
                 (cause not recorded)",
            );
        } else if pct > 120 {
            line.push_str(" — ran on several cores");
        }
        line.push_str("; reused remote results may carry the original execution's metadata");
        insert_before_last(&mut rows, Row::plain("Worker CPU", line));
    }
    if a.get("elastic").and_then(|v| v.as_bool()) == Some(true) && a.get("cache").is_none() {
        insert_before_last(
            &mut rows,
            Row::plain(
                "Worker capacity",
                "reported execution on elastic (preemptible) capacity; reused remote results \
                 may retain historical metadata; cancellation cause is not established"
                    .to_owned(),
            ),
        );
    }
    if let Some(tries) = a.get("tries").and_then(|v| v.as_i64()) {
        if tries > 1 {
            // Report recorded outcomes and temporal ordering. Cancelled does
            // not by itself identify a hybrid race, its losing side, or a
            // cancellation cause. Ordering alone does not establish the
            // build-delay effect of an earlier command.
            if let Some(rows_json) = a.get("attempts").and_then(|v| v.as_array()) {
                let winner = crate::analyses::AttemptTiming {
                    client_start_us: a
                        .get("winner")
                        .and_then(|w| w.get("client_start"))
                        .and_then(crate::analyses::micros_value),
                    dur_us: a
                        .get("winner")
                        .and_then(|w| w.get("dur_us"))
                        .and_then(Value::as_i64),
                };
                let mut lines: Vec<String> = Vec::new();
                for (i, at) in rows_json.iter().enumerate() {
                    let st = at.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                    let d = at.get("dur_us").and_then(|v| v.as_i64()).unwrap_or(0);
                    let el = at.get("elastic").and_then(|v| v.as_bool()) == Some(true);
                    let loser = crate::analyses::AttemptTiming {
                        client_start_us: at
                            .get("client_start")
                            .and_then(crate::analyses::micros_value),
                        dur_us: Some(d),
                    };
                    let (fate, _) = crate::analyses::attempt_fate(loser, winner, ts, dur);
                    let mut why = match (st, fate) {
                        ("Cancelled", crate::analyses::AttemptFate::Concurrent) => {
                            "ran alongside the final attempt; canceled, cause not recorded; \
                             build-delay effect is not established by overlap"
                                .to_owned()
                        }
                        ("Cancelled", crate::analyses::AttemptFate::Rerun) => {
                            "cancelled before the re-run started; cancellation cause not \
                             recorded; build-delay effect is not established by ordering"
                                .to_owned()
                        }
                        ("Cancelled", crate::analyses::AttemptFate::Unknown) => {
                            "cancelled — whether it overlapped the final attempt is \
                             not recorded; cancellation cause not recorded"
                                .to_owned()
                        }
                        (other, _) => other.to_lowercase(),
                    };
                    if el {
                        why.push_str(" on elastic capacity");
                    }
                    lines.push(if d > 0 {
                        format!("attempt {} — {why} after {}", i + 1, fmt_dur(d))
                    } else {
                        format!("attempt {} — {why}", i + 1)
                    });
                }
                lines.push(format!("attempt {tries} — {}", final_attempt_outcome(a)));
                insert_before_last(
                    &mut rows,
                    Row::plain("Attempts", format!("{tries}\n{}", lines.join("\n"))),
                );
                return rows;
            }
            let kinds: Vec<&str> = a
                .get("attempt_status")
                .and_then(|v| v.as_array())
                .map(|xs| xs.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let why = if kinds.is_empty() {
                "earlier attempt outcomes were not recorded".to_owned()
            } else if kinds.iter().all(|k| *k == "Cancelled") {
                "earlier attempts were canceled; cancellation cause not recorded".to_owned()
            } else {
                format!("earlier attempts ended: {}", kinds.join(", "))
            };
            insert_before_last(
                &mut rows,
                Row::plain(
                    "Attempts",
                    format!(
                        "{tries} ({why}); final attempt: {}",
                        final_attempt_outcome(a)
                    ),
                ),
            );
        }
    }
    rows
}

/// This is Buck's client cache-upload outcome, not remote-worker caching
/// policy, a cache-lookup result, or evidence that the next build must miss.
/// Keep unknown producer values visible; false legacy booleans are not reasons.
fn cache_upload_outcome(value: &Value) -> String {
    use buck2_data::UploadResult;

    let Some(raw) = value.as_i64() else {
        return "invalid recorded outcome (expected an UploadResult integer)".to_owned();
    };
    let Some(outcome) = i32::try_from(raw)
        .ok()
        .and_then(|v| UploadResult::try_from(v).ok())
    else {
        return format!("unrecognized recorded outcome ({raw})");
    };
    match outcome {
        UploadResult::DidNotUploadUnspecified => "outcome unspecified or not recorded",
        UploadResult::Uploaded => "uploaded by client",
        UploadResult::NotAttempted => "not attempted by client (reason not distinguished)",
        UploadResult::NonCommandAction => "not applicable: not a command action",
        UploadResult::ActionNotSuccessful => "not uploaded by client: action did not succeed",
        UploadResult::RemoteDepFileCacheHit => {
            "not uploaded by client: reused a remote dep-file cache result"
        }
        UploadResult::ActionUploadNotAllowed => {
            "not uploaded by client: action policy disallows it"
        }
        UploadResult::ExecutorUploadDisabled => {
            "not uploaded by client: cache writes disabled by executor"
        }
        UploadResult::NonLocalExecution => "not uploaded by client: execution was not local",
        UploadResult::RejectedOutputExceedsLimit => {
            "client upload rejected: output exceeds size limit"
        }
        UploadResult::RejectedPermissionDenied => "client upload rejected: permission denied",
        UploadResult::RejectedSymlinkOutput => "client upload rejected: symlink output",
        UploadResult::FailedUploadActionBlobs => "client upload failed: action blobs",
        UploadResult::FailedUploadOutputs => "client upload failed: output blobs",
        UploadResult::FailedWriteActionResult => "client upload failed: writing action result",
        UploadResult::FailedOther => "client upload failed (other error)",
        UploadResult::HadDepFileBundle => {
            "not uploaded by client: non-local result had a dep-file bundle"
        }
        UploadResult::DepFileEntryExists => {
            "not uploaded by client: dep-file cache entry already existed"
        }
        UploadResult::DepFileUploadNotAllowed => {
            "not uploaded by client: action disallows dep-file cache uploads"
        }
        UploadResult::LocalInPlaceOfRemote => {
            "not uploaded by client: a local run stood in for a remote one"
        }
    }
    .to_owned()
}

fn insert_before_last(rows: &mut Vec<Row>, row: Row) {
    let at = rows.len().saturating_sub(1);
    rows.insert(at, row);
}

/// Final command outcome is independent of an observed stage ending, and an
/// old capture's missing status must not silently become success.
fn final_attempt_outcome(args: &Value) -> &'static str {
    match args
        .get("winner")
        .and_then(|w| w.get("status"))
        .and_then(Value::as_str)
    {
        Some("Success") => "succeeded (recorded command outcome)",
        Some("Failure") => "failed (recorded command outcome)",
        Some("Timeout") => "timed out (recorded command outcome)",
        Some("Error") => "error (recorded command outcome)",
        Some("Cancelled") => "canceled (recorded command outcome)",
        Some("WorkerFailure") => "worker failure (recorded command outcome)",
        _ => "outcome not recorded or unrecognized",
    }
}

/// Anchored on the row it follows, so a change to the identity block
/// cannot silently move a conditional row somewhere else.
fn insert_after(rows: &mut Vec<Row>, key: &str, row: Row) {
    let at = rows
        .iter()
        .position(|r| r.key == key)
        .map_or(rows.len(), |i| i + 1);
    rows.insert(at, row);
}

/// Keys the popup renders, plus importer constants and bookkeeping
/// hidden on purpose. Other `args` remain visible in the raw record.
pub fn covered_arg_keys() -> &'static [&'static str] {
    &[
        "argv",
        "src",
        "exit",
        "sig",
        "host",
        "digest",
        "stages",
        "bytes_in",
        "bytes_out",
        "why",
        "cache",
        "cache_upload_result",
        "dep_file_cache_upload_result",
        "maxrss",
        "re",
        "kind",
        "tries",
        "run",
        "v",
        "makelevel",
        "recurse",
        "slot",
        "activity",
        "span_id",
    ]
}

// ------------------------------------------------------- analysis prompt

pub const PROMPT_PRESETS: [&str; 5] = [
    "Why is this action slow?",
    "How could this action be made faster?",
    "Where did this action's time go, stage by stage?",
    "Why did this action miss the cache?",
    "Why might this action have failed?",
];

/// The Copy-analysis-prompt window's initial body text.
pub fn prompt_intro() -> String {
    "Pick a question (or type your own) and press Preview.\n\n\
     The prompt will include this action's data and overall build context.\n\
     Nothing is sent — Copy prompt puts the text on the clipboard for\n\
     pasting into whichever assistant you use."
        .to_owned()
}

/// Context the prompt composer reads off the viewer.
#[derive(Clone, Debug, Default)]
pub struct PromptCtx<'a> {
    pub t0: i64,
    /// 1-based rank of this action by descending duration
    pub rank: usize,
    pub n_events: usize,
    /// the status-bar summary line
    pub summary: &'a str,
    pub platform: &'a str,
}

pub fn sys_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

/// Compose the analysis prompt for one action record.
pub fn analysis_prompt(rec: &Value, question: &str, ctx: &PromptCtx) -> String {
    let empty = Value::Object(Default::default());
    let a = rec.get("args").unwrap_or(&empty);
    let name = rec.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let ts = rec.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
    let dur = rec.get("dur").and_then(|v| v.as_i64()).unwrap_or(0);
    let src = str_of(a, "src");
    let mut parts: Vec<String> = vec![
        "You are analyzing one action from a buck2 build trace.".to_owned(),
        format!("Question: {question}"),
        String::new(),
        "Action data:".to_owned(),
        format!("  target:      {name}"),
        format!("  full label:  {}", str_of(a, "target")),
        format!(
            "  build type:  {} (heuristic)",
            crate::analyses::classify_type(src, name)
        ),
        format!(
            "  category:    {}",
            if src.is_empty() { "(unrecorded)" } else { src }
        ),
        format!("  executor:    {}", str_of(a, "kind")),
        format!(
            "  wall time:   {} (rank {} of {} actions by duration)",
            fmt_dur(dur),
            ctx.rank,
            ctx.n_events
        ),
        format!(
            "  started:     +{} from selected run origin",
            fmt_dur(ts.saturating_sub(ctx.t0))
        ),
        format!(
            "  exit status: {}  signal: {}",
            int_of(a, "exit"),
            int_of(a, "sig")
        ),
    ];
    if let Some(st) = stages_of(a) {
        let mut ps: Vec<String> = Vec::new();
        for (i, us) in st.iter().enumerate() {
            if *us != 0 {
                ps.push(format!("{} {}", STAGE_LABEL[i], fmt_dur(*us)));
            }
        }
        if !ps.is_empty() {
            parts.push(format!("  stages:      {}", ps.join(", ")));
        }
    }
    if let Some(lines) = stage_lines(a, dur, None, None) {
        for line in lines
            .iter()
            .filter(|line| line.parent == Some(STAGE_FETCH_IDX))
        {
            parts.push(format!("  {}", line.text.trim()));
        }
    }
    if let Some(observations) = re_observations(a) {
        parts.push(format!(
            "  {RE_OBSERVATIONS_KEY}: {}",
            observations.replace('\n', "\n    ")
        ));
    }
    if a.get("activity").is_some()
        || a.get("re")
            .and_then(|re| re.get("cancelled_stages"))
            .is_some()
    {
        parts.push(crate::activity::Activity::from_record(rec).diagnostics_text(rec, None));
    }
    if let Some(why) = a.get("why").and_then(|v| v.as_str()) {
        if !why.is_empty() {
            parts.push(format!("  why it ran:  {why}"));
        }
    }
    if a.get("cache").is_some() {
        parts.push("  cache:       HIT (reused cached outputs; any recorded current execution is shown separately)".to_owned());
    }
    if let Some(dg) = a.get("digest").and_then(|v| v.as_str()) {
        if !dg.is_empty() {
            parts.push(format!(
                "  digest:      {dg}  (frecli cas print-action reads the action definition \
                 without downloading inputs; it does not establish this execution's output)"
            ));
        }
    }
    parts.push(String::new());
    parts.push("Build context:".to_owned());
    parts.push(format!("  {}", ctx.summary));
    parts.push(format!("  platform: {}", ctx.platform));
    parts.push(String::new());
    parts.push(
        "Answer concisely and concretely.  If the data is insufficient, say what measurement \
         would settle it."
            .to_owned(),
    );
    parts.join("\n")
}

// -------------------------------------------------------------- summary

/// The idle status-bar line, also quoted into the analysis prompt.
pub struct SummaryFacts {
    pub n_events: i64,
    pub span: i64,
    pub busy: i64,
    pub nlanes: i64,
    /// (cache-hit events, Σ args.cache.bytes)
    pub cache_events: Option<(i64, f64)>,
}

pub fn summary_line(f: &SummaryFacts) -> String {
    let mut s = format!(
        "{} actions | span {} | work {} | avg parallelism {:.1}x | {} lanes",
        crate::analyses::fmt_n(f.n_events as f64),
        fmt_dur(f.span),
        fmt_dur(f.busy),
        if f.span != 0 {
            f.busy as f64 / f.span as f64
        } else {
            0.0
        },
        crate::analyses::fmt_n(f.nlanes as f64)
    );
    if let Some((nhits, rb)) = f.cache_events {
        if nhits > 0 {
            s.push_str(&format!(
                " | {} cache hits, restored {}",
                crate::analyses::fmt_n(nhits as f64),
                fmt_bytes_f(rb)
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn popup_names_measured_preparation_attempts_and_local_environment() {
        let local = json!({
            "ph": "X", "name": "t (test)", "ts": 0, "dur": 2_000,
            "args": {
                "kind": "local", "argv": ["t"], "cwd": "fbcode", "env_rule": "test_allowlist",
                "stages": {"daemon_us": 1_500, "execute_us": 500},
                "re": {"gap_pre_us": 1_000, "gap_post_us": 500,
                       "prepare_command_us": 400, "finish_action_us": 600}
            }
        });
        let ctx = PopupCtx {
            t0: 0,
            fetch_med_bps: None,
            iu_inflight: None,
        };
        let rows = popup_rows(&local, &ctx);
        let get = |k: &str| rows.iter().find(|r| r.key == k).map(|r| r.value.clone());
        assert_eq!(
            get("Working directory").unwrap(),
            "fbcode/ (relative to the project root)"
        );
        assert!(
            get("Environment")
                .unwrap()
                .starts_with("only the allow-listed variables")
        );
        let stages = get("Stages").unwrap();
        assert!(
            stages.contains("of which preparing the command 400us"),
            "{stages}"
        );
        assert!(
            stages.contains("of which finishing the action 500us"),
            "a measurement is shown no larger than its gap: {stages}"
        );

        let remote =
            json!({"re": {"kind": "remote", "execution_attempts": 3, "final_queue_us": 2_000}});
        let observations = re_observations(&remote).unwrap();
        assert!(
            observations.contains("RE attempted the action 3 times; the final attempt queued"),
            "{observations}"
        );
    }

    #[test]
    fn data_moved_keeps_logical_sizes_apart_from_measured_transfers() {
        let d = DataMoved::of(&json!({
            "bytes_in": 20_078_000_000.0,
            "bytes_out": 2048,
            "re": {"iu_bytes_uploaded": 0, "worker_bytes_in": 4096, "worker_bytes_out": -1}
        }));
        assert_eq!(d.inputs, Some(20_078_000_000.0));
        assert_eq!(d.outputs, Some(2048.0));
        assert_eq!(
            d.uploaded,
            Some(0.0),
            "a recorded zero upload is data, not absence"
        );
        assert_eq!(d.worker_in, Some(4096.0));
        assert_eq!(
            d.worker_out, None,
            "a negative count is rejected, not shown"
        );
        assert!(
            d.inputs_note()
                .contains("18.7 GiB of inputs had to be present")
        );
        assert!(
            d.inputs_note()
                .contains("4.0 KiB downloaded by the RE worker")
        );

        let local = DataMoved::of(&json!({"bytes_in": 1024}));
        assert_eq!((local.worker_in, local.outputs), (None, None));
        assert_eq!(local.outputs_note(), "output size not recorded");
    }

    #[test]
    fn upload_instrumentation_requires_a_valid_phase_or_digest_counter() {
        assert!(!iu_instrumented(&json!({"re": {"iu_bytes_uploaded": 42}})));
        assert!(iu_instrumented(&json!({"re": {"iu_sem_wait_us": 0}})));
        assert!(!iu_instrumented(&json!({"re": {
            "iu_sem_wait_us": 42,
            "invalid_counter_fields": ["iu_sem_wait_us"]
        }})));
        assert!(!iu_instrumented(&json!({"re": {"iu_sem_wait_us": -1}})));
    }

    #[test]
    fn cached_result_classification_explains_recorded_bitflags_step_without_changing_wall() {
        // A recorded bitflags metadata action, not a synthetic speedup.
        let mut args = json!({"cache": {}, "stages": {"cache_query_us": 1_564_032}});
        let mut before = CpAccounting::default();
        before.add_step(2_404_085, stages_of(&args).as_ref().map(|s| s.as_slice()));
        assert_eq!(before.unaccounted, 840_053);
        args["stages"]["cache_hit_us"] = 839_433.into();
        args["stages"]["prepare_us"] = 8.into();
        let mut after = CpAccounting::default();
        after.add_step(2_404_085, stages_of(&args).as_ref().map(|s| s.as_slice()));
        assert_eq!(after.total, before.total);
        assert_eq!(after.exec, 0);
        assert_eq!(after.unaccounted, 612);
        assert_eq!(after.overlap, 0);
        assert_eq!(
            recorded_lookup_us(&args),
            Some(1_564_032),
            "cache-hit simulation remains lookup-only"
        );
    }

    #[test]
    fn local_cache_diagnostics_do_not_invent_a_missing_parent_duration() {
        for total in [None, Some(json!(-1)), Some(json!("bad")), Some(json!(0))] {
            let mut args = json!({
                "stages": {"execute_us": 1},
                "re": {"local_cache": {"records": 1, "measured": 1, "outcomes": [[2, 1]]}}
            });
            if let Some(total) = &total {
                args["stages"]["local_cache_check_us"] = total.clone();
            }
            let lines = stage_lines(&args, 100, None, None).unwrap();
            let parent = lines
                .iter()
                .find(|line| line.stage == Some(STAGE_LOCAL_CACHE_IDX))
                .unwrap();
            if total == Some(json!(0)) {
                assert_eq!(parent.text, "local cache check 0us");
            } else {
                assert_eq!(
                    parent.text,
                    "local cache check not recorded (phase details below)"
                );
            }
            assert!(lines.iter().any(|line| line.text.contains("in-memory hit")));
        }
    }

    #[test]
    fn local_cache_diagnostics_are_nested_and_outcome_provenance_is_not_latency() {
        let args = json!({"stages": {"local_cache_check_us": 100},
            "re": {"local_cache": {
                "records": 4, "measured": 3,
                "persisted_probe_us": 0, "persisted_fetch_us": 99, "persisted_fetches": 2,
                "invalid_fields": ["persisted_fetch_us"],
                "field_records": {"persisted_probe_us": 1},
                "outcomes": [[2,1], [3,1], [91,1]],
            }}
        });
        let lines = stage_lines(&args, 100, None, None).unwrap();
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "3/4 stage observations",
            "1/3 measured records",
            "in-memory hit",
            "persisted hit",
            "unknown outcome 91",
            "not the elapsed validation cost",
            "persisted-store entry fetch (invalid)",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(!text.contains("entry fetch 99"));
        assert_eq!(stages_of(&args).unwrap().iter().sum::<i64>(), 100);
        assert!(
            lines
                .iter()
                .filter(|line| line.stage.is_none())
                .all(|line| line.parent == Some(STAGE_LOCAL_CACHE_IDX))
        );
        assert!(cache_diagnostic_lines(&json!({}), "local_cache").is_empty());
    }

    #[test]
    fn cache_phase_details_are_nested_and_do_not_double_count_stage_totals() {
        let args = json!({"stages": {"prepare_us": 5, "cache_query_us": 30, "cache_hit_us": 60, "daemon_us": 5},
        "re": {
            "cache_query": {"records": 1, "measured": 1, "permit_wait_us": 20, "sdk_us": 10},
            "cache_hit": {"records": 1, "measured": 1, "tree_admission_us": 0, "tree_fetch_us": 10,
                "local_bookkeeping_us": 25, "declare_us": 25, "tree_count": 1}
        }});
        assert_eq!(stages_of(&args).unwrap().iter().sum::<i64>(), 100);
        let lines = stage_lines(&args, 100, None, None).unwrap();
        for (text, parent) in [
            ("waiting for a cache lookup slot 20", STAGE_LOOKUP_IDX),
            (
                "admitted client/SDK operation (not network RTT) 10",
                STAGE_LOOKUP_IDX,
            ),
            ("output-tree admission 0", STAGE_CACHE_HIT_IDX),
            (
                "output metadata construction and ownership claim 25",
                STAGE_CACHE_HIT_IDX,
            ),
            ("materializer declaration 25", STAGE_CACHE_HIT_IDX),
        ] {
            let line = lines.iter().find(|line| line.text.contains(text)).unwrap();
            assert_eq!(line.stage, None);
            assert_eq!(line.parent, Some(parent));
            assert!(!line.chip);
        }
        assert!(!lines.iter().any(|line| line.text.contains("not recorded")));
    }

    #[test]
    fn cache_phase_missing_invalid_and_partial_fields_are_not_measured_zeros() {
        let args = json!({"stages": {"cache_query_us": 30, "cache_hit_us": 60},
        "re": {
            "cache_query": {"records": 3, "measured": 2, "permit_wait_us": 20,
                "sdk_us": 123, "invalid_fields": ["sdk_us"],
                "field_records": {"permit_wait_us": 1}},
            "cache_hit": {"records": 1, "measured": 1, "tree_fetch_us": -1,
                "local_bookkeeping_us": 0, "tree_count": 0}
        }});
        let lines = stage_lines(&args, 100, None, None).unwrap();
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "2/3 stage observations",
            "1/2 measured records",
            "SDK operation (not network RTT) (invalid)",
            "not recorded:",
            "output metadata construction and ownership claim 0",
            "output trees: 0",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(
            !text.contains("123"),
            "an explicit invalid marker overrides the numeric value"
        );
        assert!(!text.contains("output-tree fetch and typed decode 0"));
        assert!(!text.contains("materializer declaration 0"));
        assert_eq!(stages_of(&args).unwrap().iter().sum::<i64>(), 90);
        assert!(cache_child_lines(&json!({}), false).is_empty());
        assert!(cache_child_lines(&json!({"re": {"cache_query": {}}}), false).is_empty());
        assert!(
            re_observations(&json!({"re": {"invalid_cache_timing_records": 1}}))
                .unwrap()
                .contains("contradictory cache phase record")
        );
    }

    #[test]
    fn final_attempt_outcome_is_recorded_not_assumed_from_stage_end() {
        for (status, expected) in [
            ("Success", "succeeded"),
            ("Failure", "failed"),
            ("Timeout", "timed out"),
            ("Cancelled", "canceled"),
            ("Error", "error"),
            ("WorkerFailure", "worker failure"),
        ] {
            let args = json!({"winner": {"status": status}});
            assert!(final_attempt_outcome(&args).starts_with(expected));
        }
        for args in [
            json!({}),
            json!({"exit": 0}),
            json!({"winner": {"status": "Unknown"}}),
        ] {
            assert_eq!(
                final_attempt_outcome(&args),
                "outcome not recorded or unrecognized"
            );
        }
        let rec = json!({"ts": 0, "dur": 100, "args": {"tries": 2,
            "attempts": [{"status": "Cancelled", "dur_us": 50}],
            "winner": {"status": "Failure"}}});
        let rows = popup_rows(&rec, &PopupCtx::default());
        let attempts = &rows.iter().find(|r| r.key == "Attempts").unwrap().value;
        assert!(attempts.contains("attempt 2 — failed"), "{attempts}");
        assert!(!attempts.contains("succeeded"), "{attempts}");
    }

    #[test]
    fn counter_remainder_is_not_labeled_as_a_positioned_wait() {
        let args = json!({"stages": {"execute_us": 1_000_000}});
        let lines = stage_lines(&args, 10_000_000, None, None).unwrap();
        let remainder = lines
            .iter()
            .find(|r| r.text.starts_with("counter remainder"))
            .unwrap();
        assert!(remainder.text.contains("not a positioned wait"));
        assert_eq!(remainder.hover.as_deref(), Some(UNACCOUNTED_HELP));
    }

    #[test]
    fn legacy_attempt_count_without_outcomes_does_not_invent_failures() {
        let rec = json!({"ts": 0, "dur": 100, "args": {"tries": 3}});
        let rows = popup_rows(&rec, &PopupCtx::default());
        let attempts = &rows.iter().find(|r| r.key == "Attempts").unwrap().value;
        assert!(
            attempts.contains("earlier attempt outcomes were not recorded"),
            "{attempts}"
        );
        assert!(
            attempts.contains("final attempt: outcome not recorded"),
            "{attempts}"
        );
        for invented in ["failed", "timed out", "succeeded", "hybrid"] {
            assert!(!attempts.contains(invented), "{attempts}");
        }
    }

    #[test]
    fn legacy_cancellation_does_not_identify_a_hybrid_race_or_final_success() {
        let rec = json!({"ts": 0, "dur": 100, "args": {"tries": 2,
            "attempt_status": ["Cancelled"], "winner": {"status": "Failure"}}});
        let rows = popup_rows(&rec, &PopupCtx::default());
        let attempts = &rows.iter().find(|r| r.key == "Attempts").unwrap().value;
        assert!(
            attempts.contains("cancellation cause not recorded"),
            "{attempts}"
        );
        assert!(attempts.contains("final attempt: failed"), "{attempts}");
        for invented in [
            "hybrid",
            "losing side",
            "race",
            "succeeded",
            "not this action failing",
        ] {
            assert!(!attempts.contains(invented), "{attempts}");
        }
    }

    #[test]
    fn popup_remote_row_is_server_reported_only() {
        // client-side spans of the same windows already render as
        // Stages rows; repeating them here under "Remote" invites
        // double-reading one interval as two
        let rec = json!({
            "name": "r (cxx_compile)", "ts": 0, "dur": 2_000_000,
            "args": {"src": "cxx_compile", "exit": 0, "sig": 0,
                     "re": {"kind": "remote", "queue_us": 90_000,
                            "exec_us": 1_200_000,
                            "server_upload_us": 434_000,
                            "worker_wall_us": 1_500_000}}
        });
        let rows = popup_rows(&rec, &PopupCtx::default());
        let v = &rows.iter().find(|r| r.key == "Remote").unwrap().value;
        assert!(v.starts_with("server-reported:"), "{v}");
        assert!(v.contains("server upload 434.0ms"), "{v}");
        assert!(v.contains("worker wall 1.50s"), "{v}");
        assert!(!v.contains("queue"), "client spans stay out: {v}");
        // no server-side numbers recorded: no row at all
        let rec = json!({
            "name": "r (cxx_compile)", "ts": 0, "dur": 5,
            "args": {"src": "cxx_compile", "exit": 0, "sig": 0,
                     "re": {"kind": "remote", "queue_us": 90_000}}
        });
        let rows = popup_rows(&rec, &PopupCtx::default());
        assert!(!rows.iter().any(|r| r.key == "Remote"));
    }

    #[test]
    fn re_state_observations_do_not_change_stage_counters_or_existing_rows() {
        let old = json!({
            "name": "fallback (rustc)", "ts": 0, "dur": 100_000_000,
            "args": {"kind": "local", "re": {"queue_us": 1_000_000},
                "stages": {"queue_us": 1_000_000, "materialize_us": 20_000_000,
                    "execute_us": 30_000_000, "daemon_us": 10_000_000}}
        });
        let mut observed = old.clone();
        observed["args"]["re"]["worker_download_wait_us"] = json!(90_000_000);
        observed["args"]["re"]["worker_download_cancelled_wait_us"] = json!(80_000_000);
        observed["args"]["re"]["reported_first_enqueue_to_worker_us"] = json!(95_000_000);
        assert_eq!(stages_of(&old["args"]), stages_of(&observed["args"]));
        assert_eq!(
            stage_lines(&old["args"], 100_000_000, None, None),
            stage_lines(&observed["args"], 100_000_000, None, None),
            "overlapping observations must not enter the counter composition"
        );
        let before = popup_rows(&old, &PopupCtx::default());
        let after = popup_rows(&observed, &PopupCtx::default());
        assert_eq!(
            before,
            after
                .iter()
                .filter(|r| r.key != RE_OBSERVATIONS_KEY)
                .cloned()
                .collect::<Vec<_>>(),
            "old captures and all pre-existing rows retain their values"
        );
        let at = |key| after.iter().position(|r| r.key == key).unwrap();
        assert_eq!(at(RE_OBSERVATIONS_KEY), at("Stages") + 1);
        let text = &after[at(RE_OBSERVATIONS_KEY)].value;
        for expected in [
            "observed remote worker-download state",
            "client-side elapsed spans",
            "canceled portion",
            "subset",
            "may overlap local work",
            "not physical network service",
            "not queue wait",
            "worker activity and retries",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        for us in [90_000_000, 80_000_000, 95_000_000] {
            assert!(text.contains(&fmt_dur(us)), "{text}");
        }
        let prompt = analysis_prompt(&observed, "Why?", &PromptCtx::default());
        for line in text.lines() {
            assert!(prompt.contains(line), "prompt lost observation: {line}");
        }
    }

    #[test]
    fn re_observations_preserve_zero_and_do_not_invent_missing_fields() {
        for absent in [
            json!({}),
            json!({"re": {"queue_us": 50}}),
            json!({"re": {"worker_download_wait_us": null}}),
            json!({"re": {"worker_download_wait_us": "unknown"}}),
        ] {
            assert_eq!(re_observations(&absent), None, "{absent}");
        }
        let only_zero = json!({"re": {"worker_download_wait_us": 0}});
        let text = re_observations(&only_zero).unwrap();
        assert!(text.contains("state 0us"), "{text}");
        assert!(!text.contains("canceled portion") && !text.contains("first-enqueue"));

        let only_legacy = json!({"re": {"reported_first_enqueue_to_worker_us": 0}});
        let text = re_observations(&only_legacy).unwrap();
        assert!(text.contains("interval 0us") && text.contains("not queue wait"));
        assert!(!text.contains("worker-download state"));

        let rec = json!({"name": "no stage data", "args": only_zero});
        let rows = popup_rows(&rec, &PopupCtx::default());
        assert!(rows.iter().any(|r| r.key == RE_OBSERVATIONS_KEY));
        assert!(!rows.iter().any(|r| r.key == "Stages"));
        assert_eq!(rows.last().unwrap().key, "Digest");
        assert_eq!(stages_of(&rec["args"]), None);
    }

    #[test]
    fn queue_hover_warns_about_legacy_interval_without_relabeling_it_as_transfer() {
        let queue = STAGE_ORDER.iter().position(|k| *k == "queue_us").unwrap();
        let hover = stage_hover(queue, false);
        assert!(hover.contains("recorded remote queue wait"));
        assert!(hover.contains("Older converted captures"));
        assert!(hover.contains("first-enqueue-to-worker interval"));
        assert!(hover.contains("not a queue duration"));
    }

    #[test]
    fn exec_index_pins_to_execute_stage() {
        assert_eq!(
            STAGE_ORDER[STAGE_EXEC_IDX], "execute_us",
            "STAGE_EXEC_IDX must track execute_us when stages are \
             inserted or reordered"
        );
        assert_eq!(
            STAGE_ORDER[STAGE_FETCH_IDX], "materialize_us",
            "STAGE_FETCH_IDX must track materialize_us when stages are \
             inserted or reordered"
        );
        assert_eq!(
            STAGE_ORDER[STAGE_SLOT_IDX], "exec_slot_wait_us",
            "STAGE_SLOT_IDX must track the synthetic exec_slot_wait_us slot"
        );
        assert_eq!(
            STAGE_ORDER[STAGE_IU_IDX], "input_upload_us",
            "STAGE_IU_IDX must track input_upload_us when stages are \
             inserted or reordered"
        );
        assert_eq!(
            STAGE_ORDER[STAGE_LOOKUP_IDX], "cache_query_us",
            "STAGE_LOOKUP_IDX must track cache_query_us when stages are \
             inserted or reordered"
        );
        assert_eq!(
            STAGE_ORDER[STAGE_DAEMON_IDX], "daemon_us",
            "STAGE_DAEMON_IDX must track daemon_us when stages are \
             inserted or reordered"
        );
        assert_eq!(STAGE_ORDER[STAGE_HASH_IDX], "hashing_us");
        assert_eq!(STAGE_ORDER[STAGE_CAS_ARTIFACT_IDX], "cas_artifact_us");
        assert_eq!(STAGE_ORDER[STAGE_FINALIZE_IDX], "local_finalize_outputs_us");
        assert_eq!(STAGE_ORDER[STAGE_LOCAL_CACHE_IDX], "local_cache_check_us");
    }

    /// `docs/stages.md` is the taxonomy's prose definition, and a reader
    /// who finds it disagreeing with the code has no way to tell which
    /// one is lying. Its band table is therefore checked against the
    /// arrays rather than maintained by hand.
    #[test]
    fn stage_doc_table_matches_the_arrays() {
        // The buck test target hands the built docs over in
        // `RETRACE_DOCS`; the OSS cargo build has no such target and
        // reads the sources where they sit. Neither is available to a
        // bare `rustc --test`, which skips rather than fails.
        let Some(dir) = std::env::var("RETRACE_DOCS")
            .ok()
            .or_else(|| option_env!("CARGO_MANIFEST_DIR").map(|d| format!("{d}/docs")))
        else {
            return;
        };
        let path = std::path::Path::new(&dir).join("stages.md");
        let doc = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let rows: Vec<(usize, &str, &str)> = doc
            .lines()
            .filter_map(|l| {
                let mut c = l.strip_prefix('|')?.split('|').map(str::trim);
                let n: usize = c.next()?.parse().ok()?;
                let key = c.next()?.strip_prefix('`')?.strip_suffix('`')?;
                Some((n, key, c.next()?))
            })
            .collect();
        assert_eq!(
            rows.len(),
            STAGE_N,
            "docs/stages.md must document every band exactly once"
        );
        for (i, (n, key, label)) in rows.iter().enumerate() {
            assert_eq!(*n, i, "docs/stages.md band rows must be in display order");
            assert_eq!(*key, STAGE_ORDER[i], "docs/stages.md band {i} key");
            assert_eq!(*label, STAGE_LABEL[i], "docs/stages.md band {i} label");
        }
    }

    #[test]
    fn slow_stage_marks_follow_the_intents() {
        let n = STAGE_ORDER.len();
        let up = STAGE_ORDER.iter().position(|k| *k == "upload_us").unwrap();
        let fe = STAGE_FETCH_IDX;
        let lk = STAGE_ORDER
            .iter()
            .position(|k| *k == "cache_query_us")
            .unwrap();
        let reds = |m: &[StageMark]| m.iter().filter(|&&x| x == StageMark::Red).count();
        let bolds = |m: &[StageMark]| m.iter().filter(|&&x| x == StageMark::Bold).count();

        // Aaron's jemalloc screenshot: wall 15.53s, upload 13.43s,
        // exec 552.5ms — exactly the upload row pops red
        let mut st = vec![0i64; n];
        st[up] = 13_430_000;
        st[STAGE_EXEC_IDX] = 552_500;
        let m = slow_stage_marks(15_530_000, &st);
        assert_eq!(m[up], StageMark::Red);
        assert_eq!((reds(&m), bolds(&m)), (1, 0));

        // Aaron's 97%+93% popup: two over-threshold waits — ONE red
        // (the larger), the other bolds; never two reds
        let mut st = vec![0i64; n];
        st[up] = 9_700_000;
        st[fe] = 9_300_000;
        let m = slow_stage_marks(10_000_000, &st);
        assert_eq!(m[up], StageMark::Red);
        assert_eq!(m[fe], StageMark::Bold);
        assert_eq!(reds(&m), 1);

        // unaccounted time is nobody's wait: exec 4s of a 10s wall
        // with only a 400ms lookup recorded must NOT paint the lookup
        // red just because 5.6s went unaccounted
        let mut st = vec![0i64; n];
        st[STAGE_EXEC_IDX] = 4_000_000;
        st[lk] = 400_000;
        assert!(
            slow_stage_marks(10_000_000, &st)
                .iter()
                .all(|&x| x == StageMark::None)
        );

        // healthy action: exec 90% of wall — nothing marks, even with
        // a 300ms lookup
        let mut st = vec![0i64; n];
        st[STAGE_EXEC_IDX] = 9_000_000;
        st[lk] = 300_000;
        assert!(
            slow_stage_marks(10_000_000, &st)
                .iter()
                .all(|&x| x == StageMark::None)
        );

        // all-sub-second action: nothing
        let mut st = vec![0i64; n];
        st[up] = 700_000;
        assert!(
            slow_stage_marks(800_000, &st)
                .iter()
                .all(|&x| x == StageMark::None)
        );

        // execution is never marked, no matter how large
        let mut st = vec![0i64; n];
        st[STAGE_EXEC_IDX] = 9_500_000;
        assert!(
            slow_stage_marks(10_000_000, &st)
                .iter()
                .all(|&x| x == StageMark::None)
        );

        // fallback: no single stage reaches 25%, but RECORDED waits
        // are over half the wall — exactly the largest paints red
        let mut st = vec![0i64; n];
        st[STAGE_EXEC_IDX] = 2_000_000;
        st[lk] = 2_000_000;
        st[up] = 2_400_000;
        st[fe] = 800_000;
        let m = slow_stage_marks(10_000_000, &st);
        assert_eq!(m[up], StageMark::Red);
        assert_eq!((reds(&m), bolds(&m)), (1, 0));
    }

    #[test]
    fn stage_lines_suffix_and_rate_labels() {
        let a = json!({
            "stages": {"upload_us": 13_430_000, "execute_us": 552_500,
                       "materialize_us": 31_600},
            "bytes_in": 14_540.8,
        });
        let lines = stage_lines(&a, 15_530_000, None, None).unwrap();
        let slow: Vec<_> = lines.iter().filter(|l| l.slow).collect();
        assert_eq!(slow.len(), 1);
        assert!(slow[0].text.contains("(86% of wall)"), "{}", slow[0].text);
        // the byte line nests under the fetch stage, right after it
        let fi = lines
            .iter()
            .position(|l| l.stage == Some(STAGE_FETCH_IDX))
            .unwrap();
        assert!(
            lines[fi + 1].text.starts_with("    logical inputs"),
            "bytes are the fetch stage's data: {}",
            lines[fi + 1].text
        );
        assert!(
            lines[fi + 2]
                .text
                .contains("effective input preparation rate"),
            "the rate must say whose rate it is: {}",
            lines[fi + 2].text
        );
        assert!(!lines[fi + 2].chip && !lines[fi + 2].slow);
    }

    #[test]
    fn every_stage_has_a_composed_hover() {
        for si in 0..STAGE_N {
            assert!(
                !STAGE_LABEL[si].is_empty() && !STAGE_HELP[si].is_empty(),
                "{si}"
            );
            let h = stage_hover(si, false);
            let prov = STAGE_PROV[si].text();
            assert!(h.starts_with(prov), "provenance leads the hover: {h}");
            let def = h
                .find(STAGE_HELP[si])
                .unwrap_or_else(|| panic!("{si}: {h}"));
            assert!(def > prov.len(), "the definition follows it: {h}");
            match STAGE_CAVEAT[si] {
                Some(c) => assert!(h.ends_with(c), "the caveat trails both: {h}"),
                None => assert!(h.ends_with(STAGE_HELP[si]), "{h}"),
            }
        }
        // the one entry that depends on where the action ran
        assert_eq!(
            stage_prov(STAGE_FETCH_IDX, true),
            StageProv::LocalMaterializer
        );
        assert_eq!(stage_prov(STAGE_FETCH_IDX, false), StageProv::Worker);
        assert_eq!(stage_prov(STAGE_EXEC_IDX, true), StageProv::Client);
        for si in (0..STAGE_N).filter(|si| *si != STAGE_FETCH_IDX && *si != STAGE_EXEC_IDX) {
            assert_eq!(stage_prov(si, true), STAGE_PROV[si], "{si}");
        }
    }

    #[test]
    fn text_surfaces_keep_provenance() {
        // the GUI hangs provenance off hovers; the copied popup and the
        // analysis prompt see only this joined string
        let rec = json!({
            "name": "t (cxx_compile)", "ts": 0, "dur": 10_000_000,
            "args": {"src": "cxx_compile", "exit": 0, "sig": 0, "kind": "remote",
                     "stages": {"cache_query_us": 100, "execute_us": 3_000_000,
                                "daemon_us": 2_000_000}}
        });
        let rows = popup_rows(&rec, &PopupCtx::default());
        let v = &rows.iter().find(|r| r.key == "Stages").unwrap().value;
        let last = v.lines().last().unwrap();
        assert_eq!(
            last,
            "(cache lookup: client-side span; execution: measured by the worker; \
             client residual: derived \u{2014} residual elapsed time, not measured CPU)",
            "{v}"
        );
        // a stage the record does not have is never named
        assert!(!last.contains("worker input prep"), "{last}");
        // and local execution moves the fetch entry here too
        let a = json!({
            "kind": "local",
            "stages": {"materialize_us": 1_000_000, "execute_us": 3_000_000},
        });
        let lines = stage_lines(&a, 5_000_000, None, None).unwrap();
        assert_eq!(
            stages_prov_line(&lines, true),
            "(worker input prep: measured by this machine's materializer; \
             execution: client-side span)"
        );
    }

    #[test]
    fn upload_state_and_local_materialization_keep_their_recorded_sources() {
        let mut args = json!({"kind": "remote", "stages": {
            "upload_us": 100, "upload_source": "client_state",
            "materialize_us": 200, "materialize_source": "client_span"
        }});
        let upload = STAGE_ORDER
            .iter()
            .position(|&key| key == "upload_us")
            .unwrap();
        assert_eq!(stage_prov_for_args(upload, &args), StageProv::RemoteState);
        assert_eq!(
            stage_prov_for_args(STAGE_FETCH_IDX, &args),
            StageProv::Client
        );
        args["stages"]["upload_source"] = json!("worker");
        assert_eq!(stage_prov_for_args(upload, &args), StageProv::Worker);
        let result = STAGE_ORDER
            .iter()
            .position(|&key| key == "result_wait_us")
            .unwrap();
        assert_eq!(stage_prov_for_args(result, &args), StageProv::Legacy);
        assert!(!stage_hover(result, false).contains("queueing inside the client"));
    }

    #[test]
    fn local_fetch_is_not_a_worker() {
        // Aaron's 7.1 GiB local link: the fetch was THIS machine's
        // materializer, and both the label and the hover must say so
        let a = json!({
            "kind": "local",
            "stages": {"materialize_us": 188_000_000, "execute_us": 3_000_000},
            "bytes_in": 7_624_000_000.0,
        });
        let lines = stage_lines(&a, 200_000_000, None, None).unwrap();
        let fetch = lines
            .iter()
            .find(|l| l.stage == Some(STAGE_FETCH_IDX))
            .unwrap();
        assert!(
            fetch.text.starts_with("local input preparation"),
            "{}",
            fetch.text
        );
        let prov = fetch.hover.as_deref().unwrap();
        assert!(prov.contains("this machine's materializer"), "{prov}");
        // the fetch row's own byte and rate children answer to it too
        assert_eq!(
            lines
                .iter()
                .find(|l| l.text.contains("inputs 7"))
                .unwrap()
                .hover
                .as_deref(),
            Some(prov)
        );
        // remote stays a worker
        let a = json!({
            "kind": "remote",
            "stages": {"materialize_us": 1_000_000, "execute_us": 3_000_000},
        });
        let lines = stage_lines(&a, 5_000_000, None, None).unwrap();
        let fetch = lines
            .iter()
            .find(|l| l.stage == Some(STAGE_FETCH_IDX))
            .unwrap();
        assert!(
            fetch.text.starts_with("worker input preparation"),
            "{}",
            fetch.text
        );
        assert!(
            fetch
                .hover
                .as_deref()
                .unwrap()
                .starts_with("measured by the worker"),
            "{:?}",
            fetch.hover
        );
    }

    #[test]
    fn input_preparation_rate_context_is_not_a_network_comparison() {
        // 276 MiB at ~5.5 MiB/s against a 157 MiB/s build median: the
        // context sentence appears without claiming a network slowdown.
        let a = json!({
            "kind": "local",
            "stages": {"materialize_us": 50_540_000, "execute_us": 3_000_000},
            "bytes_in": 289_500_000.0,
        });
        let med = 157.0 * 1024.0 * 1024.0;
        let lines = stage_lines(&a, 53_780_000, Some(med), None).unwrap();
        let rate = lines
            .iter()
            .find(|l| l.text.contains("effective input preparation rate"))
            .unwrap();
        assert!(rate.text.contains("build median"), "{}", rate.text);
        assert!(rate.text.contains("logical input sets"), "{}", rate.text);
        assert!(rate.text.contains("not network bandwidth"), "{}", rate.text);
        assert!(
            rate.text
                .contains("input trees, caches and workers can differ"),
            "{}",
            rate.text
        );
        assert!(!rate.text.contains("slower"), "{}", rate.text);
        assert!(
            !rate.slow,
            "an uncontrolled rate comparison does not diagnose a fault"
        );
        // Small logical input sets never get the median context.
        let a = json!({
            "stages": {"materialize_us": 31_600, "execute_us": 552_500},
            "bytes_in": 14_540.8,
        });
        let lines = stage_lines(&a, 15_530_000, Some(med), None).unwrap();
        let rate = lines
            .iter()
            .find(|l| l.text.contains("effective input preparation rate"))
            .unwrap();
        assert!(!rate.text.contains("build median"), "{}", rate.text);
        assert!(!rate.slow);
    }

    #[test]
    fn local_output_hashing_is_a_nested_counter_not_a_second_band() {
        let args = json!({"stages": {
            "hashing_us": 1_800_000, "hashing_source": "local_outputs",
            "local_finalize_outputs_us": 1_200_000, "cas_artifact_us": 100,
        }});
        let composition = stages_of(&args).unwrap();
        assert_eq!(composition[STAGE_HASH_IDX], 0);
        assert_eq!(composition[STAGE_FINALIZE_IDX], 1_200_000);
        assert_eq!(composition[STAGE_CAS_ARTIFACT_IDX], 100);
        assert_eq!(args["stages"]["hashing_us"], 1_800_000);
        let lines = stage_lines(&args, 1_200_100, None, None).unwrap();
        assert!(!lines.iter().any(|l| l.stage == Some(STAGE_HASH_IDX)));
        let hash = lines
            .iter()
            .find(|l| l.text.contains("local output hashing"))
            .unwrap();
        assert_eq!(hash.parent, Some(STAGE_FINALIZE_IDX));
        assert!(hash.text.contains("included above, may overlap"));
        assert!(!hash.slow && !hash.bold);
        let accounting = cp_accounting(1_200_100, &composition, 0, 0);
        assert_eq!(accounting.unaccounted, 0);
        assert_eq!(accounting.overlap, 0);
    }

    #[test]
    fn hash_parent_requires_explicit_source_and_a_valid_present_counter() {
        for (source, parent, contribution) in [
            (json!("local_outputs"), json!(12), 0),
            (json!("local_outputs"), json!(0), 0),
            (json!("local_outputs"), Value::Null, 18),
            (json!("local_outputs"), json!(-1), 18),
            (json!("local_outputs"), json!("12"), 18),
            (json!("worker"), json!(12), 18),
            (Value::Null, json!(12), 18),
            (json!(true), json!(12), 18),
        ] {
            let args = json!({"stages": {
                "hashing_us": 18, "hashing_source": source,
                "local_finalize_outputs_us": parent,
            }});
            assert_eq!(
                stages_of(&args).unwrap()[STAGE_HASH_IDX],
                contribution,
                "{args}"
            );
        }
        assert!(stages_of(&json!({"stages": {"hashing_source": "local_outputs"}})).is_none());
        let zero = json!({"stages": {"hashing_source": "local_outputs", "hashing_us": 18, "local_finalize_outputs_us": 0}});
        let lines = stage_lines(&zero, 18, None, None).unwrap();
        assert!(lines.iter().any(|l| l.stage == Some(STAGE_FINALIZE_IDX)));
        assert!(lines.iter().any(|l| l.parent == Some(STAGE_FINALIZE_IDX)));
    }

    #[test]
    fn concurrent_stages_are_honest() {
        // upload 97% and fetch 93% of the same wall: one red with the
        // share, the other bold with counter excess, plus the bar
        // footnote — no unsupported concurrency claim
        let up = STAGE_ORDER.iter().position(|k| *k == "upload_us").unwrap();
        let a = json!({
            "stages": {"upload_us": 9_700_000, "materialize_us": 9_300_000,
                       "execute_us": 400_000},
        });
        let lines = stage_lines(&a, 10_000_000, None, None).unwrap();
        let red: Vec<_> = lines.iter().filter(|l| l.slow).collect();
        assert_eq!(red.len(), 1);
        assert_eq!(red[0].stage, Some(up));
        assert!(red[0].text.contains("(97% of wall)"), "{}", red[0].text);
        let bold = lines.iter().find(|l| l.bold).unwrap();
        assert_eq!(bold.stage, Some(STAGE_FETCH_IDX));
        assert!(
            bold.text
                .contains("(with outputs upload, counters exceed wall)"),
            "{}",
            bold.text
        );
        assert!(!bold.text.contains("% of wall"), "{}", bold.text);
        assert!(
            lines
                .iter()
                .any(|l| l.text.contains("stage counters exceed wall time")),
            "excess counters get the bar footnote"
        );
        assert!(
            lines
                .iter()
                .all(|l| !l.text.contains("ran at the same time"))
        );
        // A second candidate whose counters fit keeps its share suffix.
        let a = json!({
            "stages": {"upload_us": 4_000_000, "materialize_us": 3_000_000,
                       "execute_us": 400_000},
        });
        let lines = stage_lines(&a, 10_000_000, None, None).unwrap();
        let bold = lines.iter().find(|l| l.bold).unwrap();
        assert!(bold.text.contains("(30% of wall)"), "{}", bold.text);
        assert!(
            !lines
                .iter()
                .any(|l| l.text.contains("stage counters exceed wall time")),
            "no footnote when stages fit inside the wall"
        );
    }

    #[test]
    fn cp_accounting_reconciles_exactly() {
        // stages fit inside the wall: exec + waits + unaccounted == total
        let mut totals = [0i64; STAGE_ORDER.len()];
        totals[STAGE_EXEC_IDX] = 400;
        totals[0] = 100;
        totals[1] = 200;
        let acc = cp_accounting(1000, &totals, 0, 0);
        assert_eq!(acc.exec, 400, "exec reads the execute stage");
        assert_eq!(acc.waits, 300, "waits are the non-exec stage time");
        assert_eq!(acc.unaccounted, 300);
        assert_eq!(acc.overlap, 0);
        assert_eq!(
            acc.exec + acc.waits + acc.unaccounted + acc.unmeasured,
            acc.total,
            "no clamping: the parts tile the wall exactly"
        );

        // steps carrying no stage counters come out of unaccounted, not
        // out of the wall: the tool must not read a missing counter as
        // measured idleness
        let acc = cp_accounting(1000, &totals, 250, 3);
        assert_eq!(acc.unmeasured, 250);
        assert_eq!(acc.n_unmeasured, 3);
        assert_eq!(
            acc.unaccounted, 50,
            "unaccounted is what is left after both"
        );
        assert_eq!(
            acc.exec + acc.waits + acc.unaccounted + acc.unmeasured,
            acc.total
        );

        // counters exceed the wall: retain the excess rather than
        // silently clamping a negative remainder to zero. Durations
        // alone do not establish an exact temporal overlap.
        let acc = cp_accounting(500, &totals, 0, 0);
        assert_eq!(acc.unaccounted, 0);
        assert_eq!(acc.overlap, 200, "overlap = staged - total");
        assert_eq!(acc.exec + acc.waits, acc.total + acc.overlap);

        // an unmeasured step is wall the stages cannot also claim
        let acc = cp_accounting(800, &totals, 200, 1);
        assert_eq!(acc.unaccounted, 0);
        assert_eq!(acc.overlap, 100, "overlap counts unmeasured wall too");
    }

    #[test]
    fn cp_accounting_keeps_each_actions_residual_and_excess() {
        let mut first = [0i64; STAGE_N];
        first[STAGE_EXEC_IDX] = 5_000_000;
        first[STAGE_FETCH_IDX] = 10_000_000;
        let mut second = [0i64; STAGE_N];
        second[STAGE_EXEC_IDX] = 1_000_000;
        let mut acc = CpAccounting::default();
        acc.add_step(10_000_000, Some(&first));
        acc.add_step(10_000_000, Some(&second));
        assert_eq!(acc.total, 20_000_000);
        assert_eq!(acc.exec, 6_000_000);
        assert_eq!(acc.waits, 10_000_000);
        assert_eq!(acc.unaccounted, 9_000_000);
        assert_eq!(acc.overlap, 5_000_000);
        assert_eq!(
            acc.exec + acc.waits + acc.unaccounted + acc.unmeasured,
            acc.total + acc.overlap,
            "the counter composition exceeds wall by the retained per-action excess"
        );
    }

    #[test]
    fn cp_accounting_separates_missing_zero_and_normal_measurements() {
        let mut acc = CpAccounting::default();
        acc.add_step(7, None);
        acc.add_step(3, Some(&[0; STAGE_N]));
        acc.add_step(0, Some(&[0; STAGE_N]));
        let mut normal = [0i64; STAGE_N];
        normal[STAGE_EXEC_IDX] = 6;
        normal[STAGE_LOOKUP_IDX] = 2;
        acc.add_step(10, Some(&normal));
        assert_eq!(acc.total, 20);
        assert_eq!(acc.exec, 6);
        assert_eq!(acc.waits, 2);
        assert_eq!(acc.unaccounted, 5);
        assert_eq!(acc.unmeasured, 7);
        assert_eq!(acc.n_unmeasured, 1);
        assert_eq!(acc.overlap, 0);
        assert_eq!(
            acc.exec + acc.waits + acc.unaccounted + acc.unmeasured,
            acc.total
        );
    }

    #[test]
    fn invalid_phase_counters_are_unknown_without_discarding_valid_measurements() {
        let args = json!({
            "stages": {"input_upload_us": 100},
            "re": {
                "submit_us": 50,
                "rs_rpc_open_us": 0,
                "rs_request_prep_us": 3,
                "iu_find_missing_us": 7,
                "iu_cas_call_us": 0,
                "invalid_counter_fields": ["iu_sem_wait_us", "iu_digests_absent", "rs_exec_sem_wait_us"]
            }
        });
        let text = stage_lines(&args, 150, None, None)
            .unwrap()
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Invalid counters are unavailable, not zero"));
        assert!(text.contains("checking what storage has 7us"), "{text}");
        assert!(text.contains("writing new bytes 0us"), "{text}");
        assert!(
            text.contains("unavailable digests not in storage"),
            "{text}"
        );
        assert!(text.contains("execution admission not measured"), "{text}");
        assert!(
            text.contains("submit-window remainder unavailable"),
            "{text}"
        );
        assert!(!text.contains("waiting for an upload slot 0"));
        assert!(!text.contains("other 47"));
        assert!(
            re_observations(&args)
                .unwrap()
                .contains("3 invalid or overflowed counters")
        );
        let mut contradictory = args;
        contradictory["re"]["rs_exec_sem_wait_us"] = json!(999);
        assert_eq!(re_i64(&contradictory, "rs_exec_sem_wait_us"), None);
        contradictory["re"]["iu_digests_queried"] = json!(12);
        let lines = upload_child_lines(&contradictory, false, None);
        assert!(
            lines
                .iter()
                .any(|line| line.text.contains("unavailable digests not in storage")),
            "capture markers must not turn a poisoned count into zero"
        );
    }

    #[test]
    fn cached_execution_requires_current_span_and_preserves_its_provenance() {
        let legacy = json!({
            "kind": "action_cache",
            "cache": {"hit": true},
            "stages": {"cache_query_us": 100, "materialize_us": 300,
                       "execute_us": 5_000_000}
        });
        let stages = stages_of(&legacy).unwrap();
        assert_eq!(stages[STAGE_EXEC_IDX], 0);
        assert_eq!(stages[STAGE_LOOKUP_IDX], 100);
        assert_eq!(stages[STAGE_FETCH_IDX], 300);
        let lines = stage_lines(&legacy, 500, None, None).unwrap();
        assert!(!lines.iter().any(|l| l.stage == Some(STAGE_EXEC_IDX)));
        assert!(lines.iter().any(|l| l.stage == Some(STAGE_LOOKUP_IDX)));
        assert!(lines.iter().any(|l| l.stage == Some(STAGE_FETCH_IDX)));

        let mut current = legacy;
        current["stages"]["execute_us"] = json!(200);
        current["stages"]["execute_source"] = json!("client_span");
        assert_eq!(stages_of(&current).unwrap()[STAGE_EXEC_IDX], 200);
        let lines = stage_lines(&current, 700, None, None).unwrap();
        let execution = lines
            .iter()
            .find(|l| l.stage == Some(STAGE_EXEC_IDX))
            .unwrap();
        assert_eq!(execution.provenance, Some(StageProv::Client));
        assert!(
            execution
                .hover
                .as_deref()
                .unwrap()
                .starts_with("client-side span")
        );
        assert!(
            !execution
                .hover
                .as_deref()
                .unwrap()
                .contains("measured by the worker")
        );
        assert!(stages_prov_line(&lines, false).contains("execution: client-side span"));
        assert!(stage_table_text(&lines, false).contains("execution: client-side span"));
        assert_eq!(
            stage_hover_for_args(STAGE_EXEC_IDX, &current),
            execution.hover.as_deref().unwrap(),
            "the execution bar segment and row must agree on provenance"
        );
        // The generic glossary has no record marker and keeps its API.
        assert!(stage_hover(STAGE_EXEC_IDX, false).starts_with("measured by the worker"));

        let absent = json!({"cache": {"hit": true}});
        assert!(stages_of(&absent).is_none());
        assert!(stage_lines(&absent, 700, None, None).is_none());

        for unmeasured in [
            json!({"cache": {}, "stages": {"execute_us": 900}}),
            json!({"cache": {}, "stages": {"execute_us": 0}}),
            json!({"execution_metadata": "reused", "stages": {"execute_us": 900}}),
            json!({"stages": {}}),
            json!({"stages": {"execute_source": "client_span"}}),
            json!({"stages": {"cache_query_us": null}}),
            json!({"re": {"rs_exec_sem_wait_us": 10}}),
        ] {
            let stages = stages_of(&unmeasured);
            assert!(stages.is_none(), "{unmeasured}");
            assert!(stage_lines(&unmeasured, 700, None, None).is_none());
            let mut accounting = CpAccounting::default();
            accounting.add_step(700, stages.as_ref().map(|s| s.as_slice()));
            assert_eq!(accounting.unmeasured, 700);
            assert_eq!(accounting.unaccounted, 0);
        }

        for measured_zero in [
            json!({"stages": {"execute_us": 0}}),
            json!({"cache": {}, "stages": {"cache_query_us": 0, "execute_us": 900}}),
            json!({"cache": {}, "stages": {"execute_us": 0, "execute_source": "client_span"}}),
            json!({"stages": {}, "re": {"rs_exec_sem_wait_us": 0}}),
        ] {
            assert_eq!(
                stages_of(&measured_zero),
                Some([0; STAGE_N]),
                "{measured_zero}"
            );
        }
        let slot_only = json!({
            "cache": {}, "stages": {"execute_us": 900},
            "re": {"rs_exec_sem_wait_us": 30}
        });
        let stages = stages_of(&slot_only).unwrap();
        assert_eq!(stages[STAGE_EXEC_IDX], 0);
        assert_eq!(stages[STAGE_SLOT_IDX], 30);
        assert_eq!(stages.iter().sum::<i64>(), 30);
    }

    #[test]
    fn popup_rows_buck_record() {
        let rec = json!({
            "ph": "X", "cat": "recipe", "name": "z (cxx_link)", "ts": 1_000_200, "dur": 800,
            "args": {
                "target": "fbcode//x:z (cxx_link)", "src": "cxx_link", "kind": "action_cache",
                "exit": 0, "sig": 0, "dir": "/repo", "run": "aaaa-bbbb",
                "cache": {"hit": true, "bytes": 2048, "lookup_us": 100},
                "stages": {"cache_query_us": 100}, "digest": "abc123:42"
            }
        });
        let rows = popup_rows(
            &rec,
            &PopupCtx {
                t0: 1_000_100,
                fetch_med_bps: None,
                iu_inflight: None,
            },
        );
        let get = |k: &str| rows.iter().find(|r| r.key == k).map(|r| r.value.clone());
        assert_eq!(get("Target").unwrap(), "z (cxx_link)");
        assert_eq!(get("Category").unwrap(), "cxx_link \u{2014} binary");
        assert_eq!(get("Started").unwrap(), "+100us from selected run origin");
        assert_eq!(get("Wall time").unwrap(), "800us");
        assert!(get("Stages").unwrap().contains("lookup 100us"));
        assert!(get("Cache").unwrap().contains("HIT"));
        assert!(
            !get("Cache").unwrap().contains("restore"),
            "missing restore time is not measured zero"
        );
        assert!(!get("Cache").unwrap().contains("not executed"));
        assert_eq!(get("Digest").unwrap(), "abc123:42");
        assert_eq!(get("Executor").unwrap(), "action_cache");
        // Digest stays last; Cache inserted before it
        assert_eq!(rows.last().unwrap().key, "Digest");
        // Stages follows the wall time it decomposes
        let at = |k: &str| rows.iter().position(|r| r.key == k).unwrap();
        assert_eq!(at("Stages"), at("Wall time") + 1);
    }

    #[test]
    fn popup_keeps_client_upload_policy_separate_from_cache_hits_and_action_success() {
        let rec = json!({
            "args": {
                "kind": "remote", "exit": 0, "cache": {"hit": true},
                "cache_upload_result": 6, "dep_file_cache_upload_result": 1
            }
        });
        let rows = popup_rows(&rec, &PopupCtx::default());
        let get = |key| rows.iter().find(|row| row.key == key).unwrap();
        assert_eq!(
            get("Action cache write").value,
            "not uploaded by client: action policy disallows it",
        );
        assert_eq!(get("Dep-file cache write").value, "uploaded by client");
        assert!(get("Cache").value.starts_with("HIT"));
        assert!(!get("Action cache write").failed);
        assert!(!rows.iter().any(|row| row.key == "Result"));
        assert_eq!(rows.last().unwrap().key, "Digest");
        assert!(covered_arg_keys().contains(&"cache_upload_result"));
        assert!(covered_arg_keys().contains(&"dep_file_cache_upload_result"));
    }

    #[test]
    fn upload_outcome_missingness_unknown_values_and_legacy_flags_are_not_policy() {
        for args in [json!({}), json!({"allows_cache_upload": false})] {
            let rows = popup_rows(&json!({"args": args}), &PopupCtx::default());
            assert!(!rows.iter().any(|row| row.key.ends_with("cache write")));
        }
        assert_eq!(
            cache_upload_outcome(&json!(0)),
            "outcome unspecified or not recorded"
        );
        assert_eq!(
            cache_upload_outcome(&json!(i32::MAX)),
            "unrecognized recorded outcome (2147483647)",
        );
        assert_eq!(
            cache_upload_outcome(&json!(-1)),
            "unrecognized recorded outcome (-1)"
        );
        assert_eq!(
            cache_upload_outcome(&json!(false)),
            "invalid recorded outcome (expected an UploadResult integer)",
        );
        // Enum 2 deliberately folds multiple causes; do not invent one.
        assert_eq!(
            cache_upload_outcome(&json!(2)),
            "not attempted by client (reason not distinguished)",
        );
        assert_eq!(
            cache_upload_outcome(&json!(14)),
            "client upload failed: writing action result",
        );
        assert_eq!(
            cache_upload_outcome(&json!(16)),
            "not uploaded by client: non-local result had a dep-file bundle",
        );
        assert_eq!(
            cache_upload_outcome(&json!(19)),
            "not uploaded by client: a local run stood in for a remote one",
        );
    }

    #[test]
    fn popup_omits_rows_the_reader_can_already_get_elsewhere() {
        let rec = json!({
            "name": "z (cxx_link)", "ts": 1_000_200, "dur": 800,
            "args": {"src": "cxx_link", "exit": 0, "sig": 0, "dir": "/repo"}
        });
        let rows = popup_rows(
            &rec,
            &PopupCtx {
                t0: 1_000_100,
                fetch_med_bps: None,
                iu_inflight: None,
            },
        );
        let has = |k: &str| rows.iter().any(|r| r.key == k);
        // Build overview carries the directory; Started plus Wall time is
        // the finish time; the lane is the viewer's own row packing
        assert!(!has("Directory"));
        assert!(!has("Finished"));
        assert!(!has("Lane"));
        // a successful action has nothing to report
        assert!(!has("Result"));
        // dropped from the popup, so the raw-record section has to show it
        assert!(!covered_arg_keys().contains(&"dir"));
        for key in ["target", "cfg", "rule", "wall_time_us", "exec_us"] {
            assert!(
                !covered_arg_keys().contains(&key),
                "raw record must retain {key}"
            );
        }
    }

    #[test]
    fn daemon_band_names_its_position() {
        // gap-mapped capture: the daemon row gets positioned children,
        // the dominant one bolds under a red parent, and the hover
        // carries the positioning caveat
        let a = json!({
            "kind": "remote",
            "stages": {"daemon_us": 54_540_000, "execute_us": 30_000_000},
            "re": {"kind": "remote", "gap_pre_us": 1_000_000,
                   "gap_inter_us": 3_540_000, "gap_post_us": 50_000_000}
        });
        let lines = stage_lines(&a, 158_540_000, None, None).unwrap();
        let di = lines
            .iter()
            .position(|l| l.stage == Some(STAGE_DAEMON_IDX))
            .unwrap();
        assert!(
            !lines[di].slow,
            "a derived residual is not a measured machinery wait"
        );
        assert!(
            lines[di]
                .text
                .starts_with("uncovered time (client-span residual)")
        );
        for line in &lines {
            assert!(!line.text.contains("internal executors"));
            assert!(!line.text.contains("prep + dispatch"));
            assert!(!line.text.contains("result processing"));
        }
        assert!(
            lines[di + 1]
                .text
                .contains("before the first observed client span"),
            "{}",
            lines[di + 1].text
        );
        let post = lines
            .iter()
            .find(|l| l.text.contains("after the last observed client span"))
            .unwrap();
        assert!(
            !post.slow,
            "unexplained positioned remainder is not a measured wait"
        );
        // the caveat rides the parent's hover and every child inherits it
        for l in [&lines[di], post] {
            assert!(
                l.hover
                    .as_deref()
                    .unwrap()
                    .contains("positioned from client-side spans only"),
                "{}",
                l.text
            );
        }
        // pre-gap-map capture: plain daemon row, no children
        let a = json!({
            "kind": "remote",
            "stages": {"daemon_us": 54_540_000, "execute_us": 30_000_000},
            "re": {"kind": "remote"}
        });
        let lines = stage_lines(&a, 158_540_000, None, None).unwrap();
        assert!(
            !lines
                .iter()
                .any(|l| l.text.contains("after the last observed client span"))
        );
    }

    #[test]
    fn exec_slot_band_prints_the_wait_its_bar_is_drawn_from() {
        let a = json!({
            "kind": "remote",
            "stages": {"execute_us": 30_000_000},
            "re": {"kind": "remote", "submit_us": 66_770_000,
                   "rs_exec_sem_wait_us": 24_000_000,
                   "rs_request_prep_us": 400_000,
                   "rs_rpc_open_us": 370_000}
        });
        let st = stages_of(&a).unwrap();
        let lines = stage_lines(&a, 100_000_000, None, None).unwrap();
        let bi = lines
            .iter()
            .position(|l| l.stage == Some(STAGE_SLOT_IDX))
            .unwrap();
        // the row, its percentage and the bar all read one number
        assert!(
            lines[bi].text.contains(&fmt_dur(st[STAGE_SLOT_IDX])),
            "{}",
            lines[bi].text
        );
        assert!(!lines[bi].text.contains(&fmt_dur(66_770_000)));
        // the enclosing window is the band's first child, not its headline
        assert!(
            lines[bi + 1].text.contains(&format!(
                "inside a submit window of {}",
                fmt_dur(66_770_000)
            )),
            "{}",
            lines[bi + 1].text
        );
        // 66.77 - 24.0 - 0.4 - 0.37
        assert!(
            lines.iter().any(|l| l.text.contains("other 42.00s")),
            "{:?}",
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>()
        );
        assert!(
            lines
                .iter()
                .any(|l| l.text.contains("preparing the request 400.0ms"))
        );
    }

    #[test]
    fn request_preparation_is_inside_the_submit_window() {
        let a = json!({"re": {
            "submit_us": 6_000_000, "rs_exec_sem_wait_us": 600_000,
            "rs_request_prep_us": 5_000_000, "rs_rpc_open_us": 300_000
        }});
        let lines = exec_slot_child_lines(&a);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.trim()).collect();
        assert!(texts.contains(&"other 100.0ms"), "{texts:?}");
        assert!(texts.contains(&"preparing the request 5.00s"), "{texts:?}");
    }

    /// The shape of a real hybrid race: the remote attempt won while the local one was still
    /// preparing inputs, and the upload's file gathering ran inside that window.
    #[test]
    fn a_lost_local_attempt_and_its_overlap_with_the_upload_are_shown() {
        let a = json!({
            "kind": "remote",
            "bytes_in": 6_770_684_465_i64,
            "stages": {"input_upload_us": 71_156_297, "materialize_us": 4_623_958,
                       "execute_us": 5_025_215},
            "activity": {"version": 1, "spans": [
                ["ReUpload", 310_547, 71_156_297, false],
                ["Local.Queued", 321_313, 1, false],
                ["Local.MaterializeInputs", 321_317, 83_356_256, true],
                ["Re.Execute", 76_355_130, 5_028_739, false]
            ]},
            "re": {"kind": "remote", "iu_materialize_us": 69_170_000,
                   "iu_materializer_query_us": 170_000, "iu_digests_absent": 389,
                   "iu_digests_queried": 40_030, "iu_bytes_uploaded": 1_103_232,
                   "worker_bytes_in": 920_308_210}
        });
        let texts: Vec<String> = stage_lines(&a, 83_681_985, None, None)
            .unwrap()
            .into_iter()
            .map(|l| l.text)
            .collect();
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has(
                "local attempt, cancelled when the remote result arrived: preparing inputs on \
                 this machine 83.36s"
            ),
            "{texts:?}"
        );
        assert!(
            has(
                "while the local attempt was preparing this action's inputs on this machine (overlapping 71.15s)"
            ),
            "{texts:?}"
        );
        assert!(
            has("sent 1.1 MiB \u{2014} 389 of 40,030 digests not in storage"),
            "{texts:?}"
        );
        assert!(
            has("worker downloaded 877.7 MiB; the rest was already cached on the worker"),
            "{texts:?}"
        );
    }

    #[test]
    fn the_materializer_query_is_shown_inside_gathering_files() {
        let a = json!({"re": {"iu_materialize_us": 3_000, "iu_materializer_query_us": 1_000}});
        let texts: Vec<String> = upload_child_lines(&a, false, None)
            .into_iter()
            .map(|l| l.text)
            .collect();
        let gathering = texts
            .iter()
            .position(|t| t.contains("gathering local files to send"))
            .expect("the materialize phase is shown");
        assert!(
            texts[gathering + 1].contains("of which asking the materializer where each file is"),
            "{texts:?}"
        );
    }

    #[test]
    fn zero_byte_cas_calls_do_not_claim_a_transfer_or_its_wait_cause() {
        let a = json!({"re": {"iu_cas_call_us": 2_000_000, "iu_bytes_uploaded": 0}});
        let text = upload_child_lines(&a, false, None)
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("zero-byte batch; no transfer priced"),
            "{text}"
        );
        assert!(!text.contains("writing new bytes") && !text.contains("another action"));
    }

    #[test]
    fn popup_does_not_classify_a_synthetic_winner_or_show_cached_cpu_as_current() {
        let rec = json!({"ts": 100, "dur": 100, "args": {
            "cache": {"hit": true}, "cpu_us": 40, "elastic": true,
            "stages": {"execute_us": 40}, "tries": 2,
            "winner": {"client_start": 150, "dur_us": 0}, "wall_time_us": 100,
            "attempts": [{"status": "Cancelled", "client_start": 100, "dur_us": 40}]
        }});
        let rows = popup_rows(&rec, &PopupCtx::default());
        assert!(
            !rows
                .iter()
                .any(|r| r.key == "Worker CPU" || r.key == "Worker capacity")
        );
        let attempts = &rows.iter().find(|r| r.key == "Attempts").unwrap().value;
        assert!(attempts.contains("not recorded"), "{attempts}");
        assert!(!attempts.contains("before the re-run") && !attempts.contains("no delay"));
    }

    /// A remainder that goes negative means the recorded parts exceed
    /// the window; it is shown signed rather than silently dropped.
    #[test]
    fn a_negative_submit_remainder_is_shown_signed_not_dropped() {
        let mk = |rpc: i64| {
            json!({
                "kind": "remote",
                "stages": {"execute_us": 30_000_000},
                "re": {"kind": "remote", "submit_us": 1_000_000,
                       "rs_exec_sem_wait_us": 900_000,
                       "rs_rpc_open_us": rpc}
            })
        };
        let texts = |a: &Value| -> Vec<String> {
            stage_lines(a, 40_000_000, None, None)
                .unwrap()
                .iter()
                .map(|l| l.text.trim().to_owned())
                .collect()
        };
        let over = texts(&mk(300_000));
        assert!(
            over.iter()
                .any(|t| t == "other -200.0ms (sub-stages exceed the window)"),
            "{over:?}"
        );
        // exactly zero and under a millisecond: nothing to report
        for rpc in [100_000, 99_500] {
            let t = texts(&mk(rpc));
            assert!(!t.iter().any(|t| t.starts_with("other ")), "{t:?}");
        }
    }

    #[test]
    fn stages_nest_upload_children() {
        let rec = json!({
            "name": "u (cxx_compile)", "ts": 0, "dur": 30_000_000,
            "args": {"src": "cxx_compile", "exit": 0, "sig": 0,
                     "stages": {"input_upload_us": 24_000_000, "execute_us": 552_500},
                     "re": {"kind": "remote",
                            "iu_sem_wait_us": 23_000_000,
                            "iu_find_missing_us": 50_000,
                            "iu_cas_call_us": 640_000,
                            "iu_digests_absent": 12}}
        });
        let a = rec.get("args").unwrap();
        let lines = stage_lines(a, 30_000_000, None, Some(28_969)).unwrap();
        let ix = |needle: &str| {
            lines
                .iter()
                .position(|l| l.text.contains(needle))
                .unwrap_or_else(|| panic!("no line contains {needle:?}"))
        };
        let parent = ix("inputs upload (to CAS)");
        let slot = ix("waiting for an upload slot 23");
        assert_eq!(slot, parent + 1, "children nest directly under the parent");
        assert!(lines[slot].text.starts_with("    "), "children indent");
        assert!(lines[slot].bold, "dominant child of a slow parent bolds");
        assert!(!lines[slot].slow && !lines[slot].chip);
        assert!(!lines[ix("writing new bytes 640")].bold);
        assert_eq!(
            lines[ix("12 digests not in storage")].text.trim(),
            "12 digests not in storage"
        );
        assert!(
            lines[ix("other modeled upload bands at admission")]
                .text
                .contains("~28,969")
        );
        // the explainer rides the parent's hover, inherited by the children
        for l in [parent, slot] {
            assert!(
                lines[l]
                    .hover
                    .as_deref()
                    .unwrap()
                    .contains("only writing new bytes moves data"),
                "{}",
                lines[l].text
            );
        }
        // and the popup carries no separate Execution-section row anymore
        let rows = popup_rows(&rec, &PopupCtx::default());
        assert!(!rows.iter().any(|r| r.key == "Upload breakdown"));
        let stages_val = &rows.iter().find(|r| r.key == "Stages").unwrap().value;
        assert!(
            stages_val.contains("waiting for an upload slot"),
            "{stages_val}"
        );

        // old records: parent row only, no sub-rows to explain
        let old = json!({"stages": {"input_upload_us": 4},
                         "re": {"kind": "remote"}});
        let lines = stage_lines(&old, 5, None, None).unwrap();
        assert!(!lines.iter().any(|l| l.text.contains("upload slot")));

        // an upload with nothing to send: every timing zero-suppressed,
        // only the always-written counter present. It is instrumented,
        // and says so, rather than rendering like the uninstrumented row
        let idle = json!({"stages": {"input_upload_us": 40},
                          "re": {"kind": "remote", "iu_digests_queried": 0}});
        let lines = stage_lines(&idle, 50, None, None).unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.text.contains("0 digests not in storage")),
            "{:?}",
            lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    /// The bar-first layouts place a line by which bucket it falls in:
    /// a stage owns a bar segment, a child hangs off its parent, and
    /// whatever is left is printed as a footer. Nothing may fall in two
    /// buckets or none, or a layout silently drops a number that the
    /// stage list shows.
    #[test]
    fn every_stage_line_lands_in_exactly_one_layout_bucket() {
        let a = json!({
            "kind": "remote", "bytes_in": 41_943_040.0, "bytes_out": 8_388_608.0,
            "stages": {"input_upload_us": 24_000_000, "materialize_us": 9_000_000,
                       "execute_us": 30_000_000, "upload_us": 2_000_000,
                       "daemon_us": 54_540_000},
            "re": {"kind": "remote", "iu_sem_wait_us": 20_000_000,
                   "rs_exec_sem_wait_us": 880_000, "gap_pre_us": 1_000_000,
                   "gap_inter_us": 3_540_000, "gap_post_us": 50_000_000}
        });
        let lines = stage_lines(&a, 158_540_000, Some(50_000_000.0), Some(28_969)).unwrap();
        for l in &lines {
            assert!(
                !(l.stage.is_some() && l.parent.is_some()),
                "{:?} is both a stage and a child",
                l.text
            );
        }
        for (bucket, n) in [
            ("stage", lines.iter().filter(|l| l.stage.is_some()).count()),
            ("child", lines.iter().filter(|l| l.parent.is_some()).count()),
            (
                "footer",
                lines
                    .iter()
                    .filter(|l| l.stage.is_none() && l.parent.is_none())
                    .count(),
            ),
        ] {
            assert!(n > 0, "the fixture must exercise the {bucket} bucket");
        }
        // a child's parent is a real stage line, not a dangling index
        for l in lines.iter().filter(|l| l.parent.is_some()) {
            assert!(
                lines.iter().any(|p| p.stage == l.parent),
                "{:?} points at no stage line",
                l.text
            );
        }
        // the clipboard table is the whole list, so a screenshot of any
        // layout can be replaced by text that lost nothing
        let table = stage_table_text(&lines, false);
        for l in &lines {
            assert!(table.contains(l.text.as_str()), "{:?} missing", l.text);
        }
        assert!(table.contains("measured by"), "{table}");
    }

    #[test]
    fn popup_rows_failed_and_why() {
        let rec = json!({
            "name": "w (genrule)", "ts": 0, "dur": 5,
            "args": {"src": "genrule", "exit": 1, "sig": 0,
                     "why": "inputs changed (buck2 invalidation tracking)"}
        });
        let rows = popup_rows(&rec, &PopupCtx::default());
        let res = rows.iter().find(|r| r.key == "Result").unwrap();
        assert_eq!(res.value, "FAILED exit 1");
        assert!(res.failed);
        let at = |k: &str| rows.iter().position(|r| r.key == k).unwrap();
        assert_eq!(at("Why run"), at("Action") + 1);
    }

    #[test]
    fn every_stage_slot_is_pinned_across_all_parallel_arrays() {
        // The full slot order, not just the named indices: a swap of
        // two unpinned rows in any ONE parallel array silently mislabels
        // a stage everywhere it is drawn.
        assert_eq!(
            STAGE_ORDER,
            [
                "cache_query_us",
                "input_upload_us",
                "exec_slot_wait_us",
                "queue_us",
                "materialize_us",
                "hashing_us",
                "execute_us",
                "upload_us",
                "result_wait_us",
                "download_us",
                "rtt_us",
                "daemon_us",
                "cache_hit_us",
                "prepare_us",
                "cas_artifact_us",
                "local_finalize_outputs_us",
                "local_cache_check_us",
            ]
        );
        // Each parallel array's row must speak about the same stage as
        // its STAGE_ORDER slot; pin one distinctive word per row.
        let words = [
            "lookup",
            "upload",
            "slot",
            "queue",
            "input preparation",
            "hashing",
            "execution",
            "upload",
            "result stage",
            "download",
            "trips",
            "client-span residual",
            "cache-hit result handling",
            "request preparation",
            "CAS artifact",
            "local output finalization",
            "local cache check",
        ];
        for (i, w) in words.iter().enumerate() {
            assert!(
                STAGE_LABEL[i].contains(w),
                "STAGE_LABEL[{i}] ({:?}) does not describe {:?}",
                STAGE_LABEL[i],
                STAGE_ORDER[i]
            );
            assert!(!STAGE_HELP[i].is_empty(), "STAGE_HELP[{i}] empty");
        }
        let chip_words = [
            "lookup",
            "upload",
            "slot",
            "queue",
            "input prep",
            "hashing",
            "execution",
            "upload",
            "result stage",
            "download",
            "trips",
            "client residual",
            "cached result",
            "request prep",
            "CAS artifact",
            "finalize outputs",
            "local cache",
        ];
        for (i, w) in chip_words.iter().enumerate() {
            assert!(
                STAGE_CHIP[i].contains(w),
                "STAGE_CHIP[{i}] ({:?}) does not describe {:?}",
                STAGE_CHIP[i],
                STAGE_ORDER[i]
            );
        }
        // New classifications append slots without shifting legacy indices.
        assert_eq!(STAGE_ORDER[STAGE_DAEMON_IDX], "daemon_us");
        assert_eq!(STAGE_ORDER[STAGE_CACHE_HIT_IDX], "cache_hit_us");
        assert_eq!(STAGE_ORDER[STAGE_PREPARE_IDX], "prepare_us");
    }

    #[test]
    fn stages_and_summary() {
        let a = json!({"stages": {"queue_us": 50, "execute_us": 400}});
        let st = stages_of(&a).unwrap();
        assert_eq!(st[3], 50, "queue lives in slot 3 of the split order");
        assert_eq!(st[6], 400, "exec lives in slot 6");
        assert_eq!(
            st[STAGE_SLOT_IDX], 0,
            "no args.re: the synthetic slot is zero"
        );
        let b = json!({"stages": {"execute_us": 1},
                       "re": {"rs_exec_sem_wait_us": 880}});
        assert_eq!(stages_of(&b).unwrap()[STAGE_SLOT_IDX], 880);
        assert!(stages_of(&json!({})).is_none());
        let s = summary_line(&SummaryFacts {
            n_events: 404_094,
            span: 707_864_107,
            busy: 2_049_088_194_705,
            nlanes: 22_082,
            cache_events: Some((191, 34_000_000.0)),
        });
        assert!(s.contains("404,094 actions"), "{s}");
        assert!(s.contains("avg parallelism 2894.7x"), "{s}");
        assert!(s.contains("191 cache hits"), "{s}");
    }

    #[test]
    fn analysis_prompt_is_buck_flavored() {
        let rec = json!({
            "name": "y (rustc emit)", "ts": 100, "dur": 500,
            "args": {"src": "rustc", "kind": "remote", "exit": 0, "sig": 0,
                     "digest": "deadbeef:1", "stages": {"queue_us": 50}}
        });
        let p = analysis_prompt(
            &rec,
            PROMPT_PRESETS[0],
            &PromptCtx {
                t0: 0,
                rank: 3,
                n_events: 10,
                summary: "SUM",
                platform: "linux",
            },
        );
        assert!(p.starts_with("You are analyzing one action from a buck2 build trace."));
        assert!(p.contains("rank 3 of 10"));
        assert!(p.contains("stages:      RE queue 50us"));
        assert!(p.contains("frecli cas print-action"));
        assert!(!p.contains("download-action"));
        assert!(p.contains("does not establish this execution's output"));
        assert!(!p.contains("makelevel"), "make-era fields must be gone");
    }

    fn extra(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn context_strip_reads_the_same_values_the_grid_would_show() {
        let both = extra(&[
            ("Among peers", "rank 3 of 10"),
            ("Critical path", "on it (step 2)"),
            ("Deps", "4"),
        ]);
        // strip order is CONTEXT_STRIP_KEYS, not the order on `d.extra`
        assert_eq!(
            context_strip(&both).unwrap(),
            "on it (step 2) \u{2014} rank 3 of 10"
        );

        let one = extra(&[("Critical path", "off the critical path \u{2014} slack 2.0s")]);
        assert_eq!(
            context_strip(&one).unwrap(),
            "off the critical path \u{2014} slack 2.0s"
        );

        assert_eq!(context_strip(&extra(&[("Deps", "4")])), None);
        assert_eq!(context_strip(&[]), None);
    }

    #[test]
    fn command_argv_preserves_recorded_empty_and_rejects_invented_arguments() {
        let argv = command_argv(&json!({"args": {"argv": ["cc", "-c", "a.c"]}})).unwrap();
        assert_eq!(argv, ["cc", "-c", "a.c"]);

        assert_eq!(
            command_argv(&json!({"args": {"argv": []}})),
            Some(Vec::new())
        );
        assert_eq!(
            command_argv(&json!({"args": {"argv": ["", ""]}})),
            Some(vec![String::new(), String::new()])
        );

        for rec in [
            json!({}),
            json!({"args": {}}),
            json!({"args": {"argv": "cc -c a.c"}}),
            json!({"args": {"argv": ["cc", 7]}}),
            json!({"args": {"argv": ["cc", null]}}),
            json!({"args": {"argv": ["<remote>"]}}),
        ] {
            assert_eq!(command_argv(&rec), None, "{rec}");
        }
    }
}
