/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The analyses behind every panel and `--json` topic. ONE
//! implementation each — the CLI and the GUI call the same functions,
//! so the two surfaces share the same calculations.
//!
//! Inputs come in two layers: the lean [`crate::types::Trace`] from the
//! first parse, and [`Extras`] — a second sequential pass over the same
//! file for the per-action fields analyses need (stage timings, cache
//! hit details, peak RSS, transfer bytes, the action category). Rows
//! are matched to acts by the byte offset the first pass recorded, so
//! the passes cannot drift.
//!
//! Number formatting is kept exactly as the certified proto rendered it
//! (format-then-parse rounding, half-to-even), because these strings
//! were cross-checked between two implementations for weeks — do not
//! "simplify" them.

use std::borrow::Cow;
use std::io::BufRead;

use fxhash::FxHashMap;

use crate::types::F_FAILED;
use crate::types::F_NOT_RECIPE;
use crate::types::NO_STR;
use crate::types::Outcome;
use crate::types::Trace;

// ===================================================================
// §0  rounding + number formatting (kept parity-exact with the proto)
// ===================================================================

/// Python-`round(x, n)` semantics: correctly rounded on the exact
/// binary value, half-to-even (format-then-parse has both properties).
fn round_n(x: f64, n: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{:.*}", n, x).parse::<f64>().unwrap_or(x)
}

pub fn round3(x: f64) -> f64 {
    round_n(x, 3)
}

/// Python `str(float)` (shortest round-trip, always with a point).
pub fn py_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

pub fn fmt_dur(us: i64) -> String {
    // negatives (what-if regressions) format as the signed magnitude,
    // never as a raw microsecond dump ("-617749368us")
    if us < 0 {
        // saturating: i64::MIN has no negation, and a display helper
        // must never be the thing that panics on a hostile value
        return format!("-{}", fmt_dur(us.saturating_neg()));
    }
    if us >= 1_000_000 {
        format!("{:.2}s", us as f64 / 1e6)
    } else if us >= 1_000 {
        format!("{:.1}ms", us as f64 / 1e3)
    } else {
        format!("{us}us")
    }
}

/// `fmt_dur` on a float argument: the sub-millisecond branch leaks the
/// decimals, exactly as the original did.
pub fn fmt_dur_f(us: f64) -> String {
    if us < 0.0 {
        return format!("-{}", fmt_dur_f(-us));
    }
    if us >= 1e6 {
        format!("{:.2}s", us / 1e6)
    } else if us >= 1e3 {
        format!("{:.1}ms", us / 1e3)
    } else {
        format!("{}us", py_float(us))
    }
}

pub fn fmt_bytes(n: i64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    for unit in ["KiB", "MiB", "GiB"] {
        if v < 1024.0 || unit == "GiB" {
            return format!("{v:.1} {unit}");
        }
        v /= 1024.0;
    }
    unreachable!()
}

pub fn fmt_bytes_f(n: f64) -> String {
    if n < 1024.0 {
        return format!("{} B", py_float(n));
    }
    let mut v = n / 1024.0;
    for unit in ["KiB", "MiB", "GiB"] {
        if v < 1024.0 || unit == "GiB" {
            return format!("{v:.1} {unit}");
        }
        v /= 1024.0;
    }
    unreachable!()
}

/// Truncate toward zero, group in threes.
pub fn fmt_n(n: f64) -> String {
    let i = n.trunc() as i64;
    let neg = i < 0;
    let digits = i.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if neg {
        out.push('-');
    }
    let b = digits.as_bytes();
    for (k, c) in b.iter().enumerate() {
        if k > 0 && (b.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

fn group3(s: &str) -> String {
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let mut out = String::from(sign);
    let b = int.as_bytes();
    for (k, c) in b.iter().enumerate() {
        if k > 0 && (b.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

pub fn fmt_secs(us: f64) -> String {
    let s = us / 1e6;
    let mut txt = group3(&format!("{s:.2}"));
    txt.push('s');
    if s >= 7200.0 {
        txt.push_str(&format!(" ({}h)", group3(&format!("{:.1}", s / 3600.0))));
    }
    txt
}

/// Wall-clock rendering in UTC ("Thu 2026-08-27 16:00:00 UTC"). The GUI
/// passes a local-timezone formatter instead; this is the headless one.
pub fn fmt_when_utc(us: i64) -> String {
    const DOW: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let secs = us.div_euclid(1_000_000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{} {y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        DOW[((days + 3).rem_euclid(7)) as usize],
        sod / 3600,
        (sod / 60) % 60,
        sod % 60
    )
}

/// `shlex.quote` semantics.
pub fn shlex_quote(w: &str) -> String {
    let safe = !w.is_empty()
        && w.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c));
    if safe {
        w.to_owned()
    } else {
        format!("'{}'", w.replace('\'', "'\"'\"'"))
    }
}

pub fn shlex_join(words: &[String]) -> String {
    words
        .iter()
        .map(|w| shlex_quote(w))
        .collect::<Vec<_>>()
        .join(" ")
}

// ===================================================================
// §1  build types (the taxonomy the breakdown and the legend share)
// ===================================================================

/// The build types, legend order, with the chart palette.
pub const BUILD_TYPES: [(&str, [u8; 3]); 7] = [
    ("object", [0x4e, 0x79, 0xa7]),
    ("archive", [0xf2, 0x8e, 0x2b]),
    ("library", [0xb0, 0x7a, 0xa1]),
    ("binary", [0x59, 0xa1, 0x4f]),
    ("generated", [0xed, 0xc9, 0x48]),
    ("script", [0x9c, 0x75, 0x5f]),
    ("other", [0xba, 0xb0, 0xac]),
];

pub fn type_color(t: &str) -> [u8; 3] {
    BUILD_TYPES
        .iter()
        .find(|(n, _)| *n == t)
        .map(|(_, c)| *c)
        .unwrap_or([0xba, 0xb0, 0xac])
}

/// Exact category names whose type is fixed. Checked BEFORE the
/// substring heuristics — the proto checked "link" first, which sent
/// `symlinked_dir` to "binary" (a documented parity bug, fixed here now
/// that parity with the Python original is no longer the contract).
const BUCK_CATEGORY_TYPES: [(&str, &str); 10] = [
    ("genrule", "generated"),
    ("download_file", "generated"),
    ("http_archive", "generated"),
    ("thrift", "generated"),
    ("write", "script"),
    ("write_json", "script"),
    ("write_file", "script"),
    ("copy", "script"),
    ("symlinked_dir", "script"),
    ("mkdir", "script"),
];

/// Build type from the action's category (`args.src`) and its display
/// name.
pub fn classify_type(src: &str, name: &str) -> &'static str {
    let cat = src.to_lowercase();
    for (k, v) in BUCK_CATEGORY_TYPES {
        if cat == k {
            return v;
        }
    }
    let par = match name.rfind('(') {
        Some(i) => name[i..].to_lowercase(),
        None => String::new(),
    };
    if cat.contains("link") || par.contains("link") {
        let name = name.to_ascii_lowercase();
        if name.contains(".so") || name.contains(".dylib") || name.contains(".dll") {
            return "library";
        }
        return "binary";
    }
    if cat.contains("archive") {
        return "archive";
    }
    if cat.contains("compile")
        || cat.contains("cxx")
        || matches!(
            cat.as_str(),
            "rustc" | "javacd_jar" | "kotlincd_jar" | "javac" | "kotlinc" | "dex" | "d8" | "aapt2"
        )
    {
        return "object";
    }
    "other"
}

// ===================================================================
// §2  Extras: the analysis-only per-action fields
// ===================================================================

/// `args.stages` — the pipeline-stage counters the importer records.
/// These are recorded counters, not necessarily disjoint intervals; `rtt_us`/`daemon_us`
/// are derived remainders and are excluded from instrumentation-
/// coverage sums (counting them would make coverage a tautology).
/// Modern captures separate client input upload from worker output upload.
/// Legacy result-stage and upload counters retain their ambiguous provenance;
/// a name alone does not establish a client queue or network transfer.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stages {
    pub cache_query_us: i64,
    /// Client-observed acceptance of cached outputs, not historical execution.
    pub cache_hit_us: i64,
    /// Constructing the action request before cache lookup or execution.
    pub prepare_us: i64,
    /// `None` when the producer recorded no queue stage — absent is not
    /// zero; coverage counts depend on the distinction
    pub queue_us: Option<i64>,
    pub materialize_us: i64,
    pub hashing_us: i64,
    pub cas_artifact_us: i64,
    pub local_finalize_outputs_us: i64,
    pub local_cache_check_us: i64,
    /// Preserve the raw hash sum while excluding it from its inclusive parent.
    pub hashing_in_finalize_outputs: bool,
    /// Presence after field validation and removal of historical execution.
    /// Zero duration is measured only when the corresponding bit is present.
    pub measured_mask: u16,
    pub execute_us: i64,
    /// Worker output upload; older pre-split captures may aggregate uploads.
    pub upload_us: i64,
    /// client input upload to CAS (split converter)
    pub input_upload_us: i64,
    /// Legacy result-stage counter, not established client processing time.
    pub result_wait_us: i64,
    /// client result download (split converter)
    pub download_us: i64,
    /// legacy pre-split captures only (wall-based derivation)
    pub rtt_us: i64,
    pub daemon_us: i64,
}

impl Stages {
    // Bits follow the normalized source counters used by load_extras below.
    pub const CACHE_QUERY: u16 = 1 << 0;
    pub const CACHE_HIT: u16 = 1 << 1;
    pub const PREPARE: u16 = 1 << 2;
    pub const QUEUE: u16 = 1 << 3;
    pub const MATERIALIZE: u16 = 1 << 4;
    pub const HASHING: u16 = 1 << 5;
    pub const CAS_ARTIFACT: u16 = 1 << 6;
    pub const FINALIZE_OUTPUTS: u16 = 1 << 7;
    pub const LOCAL_CACHE: u16 = 1 << 8;
    pub const EXECUTE: u16 = 1 << 9;
    pub const UPLOAD: u16 = 1 << 10;
    pub const INPUT_UPLOAD: u16 = 1 << 11;
    pub const RESULT_WAIT: u16 = 1 << 12;
    pub const DOWNLOAD: u16 = 1 << 13;
    pub const RTT: u16 = 1 << 14;
    pub const DAEMON: u16 = 1 << 15;
    pub const OUTPUT_RESULT: u16 = Self::UPLOAD | Self::RESULT_WAIT | Self::DOWNLOAD;
    pub const DERIVED: u16 = Self::RTT | Self::DAEMON;
    pub const HARD_NON_EXECUTION: u16 = !(Self::EXECUTE | Self::DERIVED);

    /// Whether any requested counter is measured, including explicit zero.
    /// Hand-constructed stages without a mask can establish only positive
    /// counters (or `queue_us: Some(0)`), never a zero inferred from Default.
    pub fn has_any(&self, requested: u16) -> bool {
        if self.measured_mask != 0 {
            return self.measured_mask & requested != 0;
        }
        [
            self.cache_query_us > 0,
            self.cache_hit_us > 0,
            self.prepare_us > 0,
            self.queue_us.is_some(),
            self.materialize_us > 0,
            self.hashing_us > 0,
            self.cas_artifact_us > 0,
            self.local_finalize_outputs_us > 0,
            self.local_cache_check_us > 0,
            self.execute_us > 0,
            self.upload_us > 0,
            self.input_upload_us > 0,
            self.result_wait_us > 0,
            self.download_us > 0,
            self.rtt_us > 0,
            self.daemon_us > 0,
        ]
        .into_iter()
        .enumerate()
        .any(|(i, present)| present && requested & (1 << i) != 0)
    }

    /// Sum of the hard (recorded-span) counters only — the derived
    /// remainders (`rtt_us`, `daemon_us`) stay out so coverage never
    /// becomes a tautology.
    pub fn hard_sum(&self) -> i64 {
        self.cache_query_us
            + self.cache_hit_us
            + self.prepare_us
            + self.queue_us.unwrap_or(0)
            + self.materialize_us
            + hashing_composition_us(self.hashing_us, self.hashing_in_finalize_outputs)
            + self.cas_artifact_us
            + self.local_finalize_outputs_us
            + self.local_cache_check_us
            + self.execute_us
            + self.upload_us
            + self.input_upload_us
            + self.result_wait_us
            + self.download_us
    }
}

/// A local attempt alone does not identify the final report's hash counter.
pub fn hashing_in_finalize_outputs(source: Option<&str>, finalize_us: Option<i64>) -> bool {
    source == Some("local_outputs") && finalize_us.is_some_and(|us| us >= 0)
}

/// File-hash durations can overlap one another and their enclosing finalization.
pub fn hashing_composition_us(raw_us: i64, included_in_finalize: bool) -> i64 {
    if included_in_finalize { 0 } else { raw_us }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct CacheHit {
    pub bytes: Option<i64>,
    pub restore_us: Option<i64>,
    pub lookup_us: Option<i64>,
}

/// `args.re.iu_*` — the input-upload sub-stage breakdown the
/// instrumented producer records inside the upload span, plus the
/// input-digest classification. Present only on captures made with the
/// round-3 instrumentation (2026-08-29).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadSub {
    /// waiting for the client-side upload permit (buck2's `cas_semaphore`)
    pub sem_wait_us: i64,
    /// the presence check: which inputs the CAS already has
    pub find_missing_us: i64,
    /// materializing local files needed for the upload
    pub materialize_us: i64,
    /// the RE client upload call itself
    pub cas_call_us: i64,
    /// input digests the CAS did not have at all
    pub absent: i64,
    /// Fields explicitly rejected by the producer or decoder. Missing fields
    /// in an otherwise recorded breakdown are zero-suppressed measurements;
    /// rejected fields are not. This mask fits the counter struct's padding.
    pub invalid: u16,
}

impl UploadSub {
    // Phase indexes shared by reports and scenarios; distinct from validity masks.
    pub const PHASE_PERMIT_WAIT: usize = 0;
    pub const PHASE_PRESENCE_CHECK: usize = 1;
    pub const PHASE_MATERIALIZE: usize = 2;
    pub const PHASE_CAS_CALL: usize = 3;

    pub const SEM_WAIT: u16 = 1 << 0;
    pub const FIND_MISSING: u16 = 1 << 1;
    pub const MATERIALIZE: u16 = 1 << 2;
    pub const CAS_CALL: u16 = 1 << 3;
    pub const ABSENT: u16 = 1 << 4;
    pub const BYTES: u16 = 1 << 5;

    pub fn valid(&self, field: u16) -> bool {
        self.invalid & field == 0
    }

    /// Permit wait, presence check, local materialization, and CAS call, in order.
    pub fn phase(&self, i: usize) -> Option<i64> {
        let value = match i {
            Self::PHASE_PERMIT_WAIT => self.sem_wait_us,
            Self::PHASE_PRESENCE_CHECK => self.find_missing_us,
            Self::PHASE_MATERIALIZE => self.materialize_us,
            Self::PHASE_CAS_CALL => self.cas_call_us,
            _ => return None,
        };
        self.valid(1 << i).then_some(value)
    }
}

#[derive(Default, Clone, Debug)]
pub struct EvArgs {
    /// Whether this action's optional fields were successfully decoded.
    pub decoded: bool,
    pub maxrss: Option<i64>,
    /// index into `Extras::srcs` (the action category); `None` when
    /// `args.src` is absent
    pub src: Option<u32>,
    /// `Some` iff `"cache" in args` (the action was served by a cache)
    pub cache: Option<CacheHit>,
    pub bytes_in: i64,
    pub bytes_in_recorded: bool,
    /// A known non-command action kind must not enter execution scenarios.
    pub cache_ineligible: bool,
    pub bytes_out: i64,
    pub bytes_out_recorded: bool,
    /// `None` when the row carries no `args.stages` at all
    pub stages: Option<Stages>,
    /// `None` when the row carries no `args.re.iu_*` breakdown
    pub iu: Option<UploadSub>,
    /// `args.re.iu_bytes_uploaded`: the upload batch size the producer
    /// recorded (nominal, before retries). Written by producers with or
    /// without the sub-stage breakdown, so it says nothing about
    /// instrumentation presence; `None` when the capture lacks it
    pub iu_bytes_uploaded: Option<i64>,
    /// A rejected batch-size counter must not fall back to logical inputs.
    pub iu_bytes_invalid: bool,
    /// `args.re.worker_bytes_in` / `_out`: what the RE worker downloaded for
    /// inputs and uploaded for outputs; `None` unless RE reported it for
    /// an execution in this build
    pub worker_bytes_in: Option<i64>,
    pub worker_bytes_out: Option<i64>,
    /// rung-2 execution-admission band: the whole submit window and
    /// its dominant in-line-for-a-slot component (`args.re.submit_us`
    /// / `.rs_exec_sem_wait_us`); zero on pre-rung-2 captures
    pub submit_us: i64,
    pub exec_sem_wait_us: i64,
    pub admission_recorded: bool,
    pub exec_us: Option<i64>,
    pub wall_time_us: Option<i64>,
    /// `args.tries` — attempts including retries; absent when 1
    pub tries: Option<i64>,
    /// discarded attempts, newest last
    pub attempts: Vec<Attempt>,
    /// `args.winner.client_start`: the winning attempt's start on the
    /// buck2 host's clock, absolute µs; `None` on captures whose
    /// producer did not record it
    pub winner_client_start_us: Option<i64>,
    /// `args.winner.dur_us`: the same command's measured client span.
    /// Action-level `wall_time_us` is not a substitute for this interval.
    pub winner_dur_us: Option<i64>,
    /// reported execution CPU metadata (`args.cpu_us`), not a
    /// contention diagnosis or guaranteed current-attempt accounting:
    /// legacy remote reuse can retain the original execution's stats
    pub cpu_us: i64,
    /// reported execution metadata says elastic capacity; on legacy
    /// reused remote results this may describe the original execution
    pub elastic: bool,
    /// how each non-final attempt ended (`args.attempt_status`): buck2
    /// reports `Cancelled` when its hybrid executor drops the losing
    /// side of a local-vs-remote race, while a `Failure` is the
    /// action's own — a bare retry count conflates them. Interned into
    /// `Extras::srcs`; empty on older captures.
    pub attempt_status: Vec<u32>,
    /// index into `Extras::labels` — the full target label off
    /// `args.target` with the ` (category ident)` suffix stripped
    pub label: Option<u32>,
    /// The losing side of a hybrid local/remote race, from `args.re.cancelled_stages`.
    pub cancelled: Option<CancelledAttempt>,
}

/// The side of a hybrid race that buck2 cancelled when the other side finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelledAttempt {
    /// `true` for the local attempt (`Local.*` stages), `false` for the remote one (`Re.*`).
    pub local: bool,
    /// The stage it was in when cancelled, interned into `Extras::srcs`.
    pub phase: u32,
    /// How long that stage had run.
    pub dur_us: i64,
}

impl CancelledAttempt {
    /// Whether the local attempt was cancelled while still getting its inputs onto this machine.
    pub fn preparing_local_inputs(&self, srcs: &[String]) -> bool {
        self.local
            && srcs.get(self.phase as usize).map(String::as_str) == Some("Local.MaterializeInputs")
    }
}

/// One discarded attempt of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub dur_recorded: bool,
    pub start_recorded: bool,
    /// `args.attempts[].status`, interned into `Extras::srcs`
    pub status: u32,
    /// `args.attempts[].dur_us`: the attempt's client span; 0 when the
    /// capture did not record one
    pub dur_us: i64,
    /// `args.attempts[].start`: the executor's own start stamp, which
    /// for a remote attempt is the RE worker's clock; informational,
    /// never compared with another attempt
    pub start_us: i64,
    /// `args.attempts[].client_start`: absolute µs on the buck2 host's
    /// clock, the one start two attempts of one action share
    pub client_start_us: Option<i64>,
    pub elastic: bool,
}

/// A `ph:"M"` record in the run or command-profile metadata category.
#[derive(Clone, Debug)]
pub struct MetaRec {
    pub name: String,
    pub ts: i64,
    pub args: serde_json::Value,
}

impl MetaRec {
    pub fn get(&self, k: &str) -> Option<&serde_json::Value> {
        self.args.get(k)
    }
    pub fn i(&self, k: &str) -> Option<i64> {
        self.args.get(k).and_then(|v| v.as_i64())
    }
    pub fn s(&self, k: &str) -> Option<&str> {
        self.args.get(k).and_then(|v| v.as_str())
    }
}

/// The second pass over the trace file: everything `parse()` drops.
#[derive(Default)]
pub struct Extras {
    /// per `Trace::acts` index
    pub args: Vec<EvArgs>,
    /// interned `args.src` (category) strings
    pub srcs: Vec<String>,
    /// interned target labels (`EvArgs::label` indexes this)
    pub labels: Vec<String>,
    /// build type per act index
    pub ty: Vec<&'static str>,
    pub meta: Vec<MetaRec>,
    /// acts whose source line did not decode on the second pass
    pub unmatched: usize,
    pub rejected_fields: usize,
    /// Source changed or became unreadable between passes. No extras
    /// from that failed pass are retained, even when a prefix decoded.
    pub source_error: Option<String>,
    pub load_ms: u128,
}

impl Extras {
    /// Missing second-pass data remains unknown, including for callers that
    /// deliberately build an action-only view without loading extras.
    pub(crate) fn action_args(&self, act: usize) -> &EvArgs {
        static EMPTY: std::sync::OnceLock<EvArgs> = std::sync::OnceLock::new();
        self.args
            .get(act)
            .unwrap_or_else(|| EMPTY.get_or_init(EvArgs::default))
    }
}

#[derive(serde::Deserialize)]
struct JStages {
    cache_query_us: Option<i64>,
    cache_hit_us: Option<i64>,
    prepare_us: Option<i64>,
    queue_us: Option<i64>,
    materialize_us: Option<i64>,
    hashing_us: Option<i64>,
    hashing_source: Option<String>,
    cas_artifact_us: Option<i64>,
    local_finalize_outputs_us: Option<i64>,
    local_cache_check_us: Option<i64>,
    execute_us: Option<i64>,
    execute_source: Option<String>,
    upload_us: Option<i64>,
    input_upload_us: Option<i64>,
    result_wait_us: Option<i64>,
    download_us: Option<i64>,
    rtt_us: Option<i64>,
    daemon_us: Option<i64>,
}

/// Execution attributable to this capture, rather than the original
/// execution whose result a cache returned. Older converted traces put
/// that historical duration in both `exec_us` and `stages.execute_us`.
/// A cache result can still follow real local execution (for example a
/// losing hybrid attempt); explicitly positioned client spans retain
/// that work. Missing worker counters do not establish result reuse.
pub fn current_execution_us(
    reported: Option<i64>,
    cache_hit: bool,
    execution_metadata: Option<&str>,
    execute_source: Option<&str>,
) -> Option<i64> {
    if execute_source == Some("client_span") || (!cache_hit && execution_metadata != Some("reused"))
    {
        reported
    } else {
        None
    }
}

/// One DISCARDED attempt: why it ended and how long it burned first.
///
/// Everything here is best-effort. An attempt record is a decoration on
/// a row that already carries the timings people actually rank by, so a
/// converter that writes one field in an unexpected shape must cost us
/// that field and nothing more — never the row's whole extras block.
#[derive(serde::Deserialize, Default)]
struct JAttempt {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    dur_us: Option<i64>,
    #[serde(default, deserialize_with = "de_micros")]
    start: Option<i64>,
    #[serde(default, deserialize_with = "de_micros")]
    client_start: Option<i64>,
    #[serde(default)]
    elastic: bool,
}

/// The winning attempt's own timing (`args.winner`). The enclosing
/// action's `wall_time_us` may include other work and is never paired
/// with this start.
#[derive(serde::Deserialize, Default)]
struct JWinner {
    #[serde(default, deserialize_with = "de_micros")]
    client_start: Option<i64>,
    #[serde(default)]
    dur_us: Option<i64>,
}

/// Microseconds, also accepting a raw protobuf `[seconds, nanos]` pair.
/// Captures written by a converter that leaked the unconverted pair are
/// still readable; the timestamp is worth far less than the row it is
/// attached to.
/// A microsecond field as the converters write it: a plain number, or
/// a `[seconds, nanos]` pair.  ONE reader for every surface — the
/// census and the popup must never classify the same attempt
/// differently because they parsed its timestamp differently.
pub fn micros_value(v: &serde_json::Value) -> Option<i64> {
    crate::types::micros_value(v)
}

fn de_micros<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Option<serde_json::Value> as serde::Deserialize>::deserialize(d)?;
    match v {
        None => Ok(None),
        Some(v) => micros_value(&v)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("invalid microsecond value")),
    }
}

#[derive(serde::Deserialize)]
struct JCache {
    bytes: Option<i64>,
    restore_us: Option<i64>,
    lookup_us: Option<i64>,
}

#[derive(serde::Deserialize)]
struct JRe {
    /// Native conversion omits poisoned fields but retains their identity so
    /// zero-suppression cannot turn overflow into a measured zero.
    #[serde(default)]
    invalid_counter_fields: Vec<String>,
    submit_us: Option<i64>,
    rs_exec_sem_wait_us: Option<i64>,
    iu_sem_wait_us: Option<i64>,
    iu_find_missing_us: Option<i64>,
    iu_materialize_us: Option<i64>,
    iu_cas_call_us: Option<i64>,
    iu_digests_absent: Option<i64>,
    // The emitter zero-suppresses the five fields above but always
    // writes this one, so on an upload with nothing to send it is the
    // only evidence that the producer was instrumented at all.
    iu_digests_queried: Option<i64>,
    // Predates the sub-stage breakdown, so it must stay out of `any()`:
    // its presence alone must not make an uninstrumented capture read
    // as instrumented with measured-zero timings.
    iu_bytes_uploaded: Option<i64>,
    worker_bytes_in: Option<i64>,
    worker_bytes_out: Option<i64>,
    #[serde(default)]
    cancelled_stages: Vec<JCancelledStage>,
}

/// A stage of the losing side of a hybrid race (`args.re.cancelled_stages`).
#[derive(serde::Deserialize, Default)]
struct JCancelledStage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    dur: Option<i64>,
}

impl JRe {
    fn upload_invalid(&self) -> u16 {
        self.invalid_counter_fields.iter().fold(0, |mask, key| {
            mask | match key.as_str() {
                "iu_sem_wait_us" => UploadSub::SEM_WAIT,
                "iu_find_missing_us" => UploadSub::FIND_MISSING,
                "iu_materialize_us" => UploadSub::MATERIALIZE,
                "iu_cas_call_us" => UploadSub::CAS_CALL,
                "iu_digests_absent" => UploadSub::ABSENT,
                "iu_bytes_uploaded" => UploadSub::BYTES,
                _ => 0,
            }
        })
    }

    fn any(&self) -> bool {
        // Rejected fields are unknown, not evidence of measured-zero phases.
        self.iu_sem_wait_us.is_some()
            || self.iu_find_missing_us.is_some()
            || self.iu_materialize_us.is_some()
            || self.iu_cas_call_us.is_some()
            || self.iu_digests_absent.is_some()
            || self.iu_digests_queried.is_some()
    }
}

#[derive(serde::Deserialize)]
struct JArgs<'a> {
    maxrss: Option<i64>,
    #[serde(borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    src: Option<Cow<'a, str>>,
    cache: Option<JCache>,
    bytes_in: Option<i64>,
    bytes_out: Option<i64>,
    stages: Option<JStages>,
    re: Option<JRe>,
    exec_us: Option<i64>,
    #[serde(borrow)]
    execution_metadata: Option<Cow<'a, str>>,
    wall_time_us: Option<i64>,
    tries: Option<i64>,
    #[serde(borrow, default)]
    attempt_status: Vec<Cow<'a, str>>,
    #[serde(default)]
    attempts: Vec<JAttempt>,
    #[serde(default)]
    winner: Option<JWinner>,
    cpu_us: Option<i64>,
    #[serde(default)]
    elastic: bool,
    #[serde(borrow)]
    target: Option<Cow<'a, str>>,
}

#[derive(serde::Deserialize)]
struct JRow<'a> {
    ph: Option<&'a str>,
    cat: Option<&'a str>,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    ts: Option<i64>,
    #[serde(borrow)]
    args: Option<JArgs<'a>>,
}

impl JArgs<'_> {
    /// Invalid counters cost their own field. A conservative aggregate budget
    /// then protects the integer sums and scaled scenario arithmetic shared by
    /// consumers, without sprinkling saturating arithmetic through hot loops.
    fn sanitize(&mut self, total: &mut i128) -> usize {
        let mut rejected = self
            .re
            .as_ref()
            .map_or(0, |r| r.invalid_counter_fields.len());
        if self.maxrss.is_some_and(|v| v < 0) {
            self.maxrss = None;
            rejected += 1;
        }
        let mut counter = |value: &mut Option<i64>, include: bool| {
            if value.is_some_and(|v| v < 0) {
                *value = None;
                rejected += 1;
            }
            if include {
                *total += i128::from(value.unwrap_or(0));
            }
        };
        for value in [
            &mut self.bytes_in,
            &mut self.bytes_out,
            &mut self.exec_us,
            &mut self.wall_time_us,
            &mut self.tries,
            &mut self.cpu_us,
        ] {
            counter(value, true);
        }
        if let Some(c) = &mut self.cache {
            for value in [&mut c.bytes, &mut c.restore_us, &mut c.lookup_us] {
                counter(value, true);
            }
        }
        if let Some(s) = &mut self.stages {
            for value in [
                &mut s.cache_query_us,
                &mut s.cache_hit_us,
                &mut s.prepare_us,
                &mut s.queue_us,
                &mut s.materialize_us,
                &mut s.cas_artifact_us,
                &mut s.local_finalize_outputs_us,
                &mut s.local_cache_check_us,
                &mut s.execute_us,
                &mut s.upload_us,
                &mut s.input_upload_us,
                &mut s.result_wait_us,
                &mut s.download_us,
                &mut s.rtt_us,
                &mut s.daemon_us,
            ] {
                counter(value, true);
            }
            let included = hashing_in_finalize_outputs(
                s.hashing_source.as_deref(),
                s.local_finalize_outputs_us,
            );
            counter(&mut s.hashing_us, !included);
        }
        if let Some(r) = &mut self.re {
            for (key, value) in [
                ("submit_us", &mut r.submit_us),
                ("rs_exec_sem_wait_us", &mut r.rs_exec_sem_wait_us),
                ("iu_sem_wait_us", &mut r.iu_sem_wait_us),
                ("iu_find_missing_us", &mut r.iu_find_missing_us),
                ("iu_materialize_us", &mut r.iu_materialize_us),
                ("iu_cas_call_us", &mut r.iu_cas_call_us),
                ("iu_digests_absent", &mut r.iu_digests_absent),
                ("iu_digests_queried", &mut r.iu_digests_queried),
                ("iu_bytes_uploaded", &mut r.iu_bytes_uploaded),
                ("worker_bytes_in", &mut r.worker_bytes_in),
                ("worker_bytes_out", &mut r.worker_bytes_out),
            ] {
                if r.invalid_counter_fields
                    .iter()
                    .any(|invalid| invalid == key)
                {
                    // The durable rejection wins over contradictory payload values.
                    *value = None;
                } else if value.is_some_and(|v| v < 0) {
                    r.invalid_counter_fields.push(key.to_owned());
                }
                counter(value, true);
            }
        }
        for a in &mut self.attempts {
            counter(&mut a.dur_us, true);
        }
        if let Some(winner) = &mut self.winner {
            counter(&mut winner.dur_us, true);
        }
        rejected
    }
}

/// Re-read the trace file for the analysis-only args. One sequential
/// pass with a narrow serde struct; rows are matched to `Trace::acts`
/// by the byte offset the first parse recorded.
pub fn load_extras(tr: &Trace) -> Extras {
    load_extras_impl(tr, &|| false)
}

pub fn load_extras_cancellable(tr: &Trace, cancel: &dyn Fn() -> bool) -> anyhow::Result<Extras> {
    anyhow::ensure!(!cancel(), "analysis cancelled");
    let extras = load_extras_impl(tr, cancel);
    if let Some(error) = &extras.source_error {
        anyhow::bail!("{error}");
    }
    Ok(extras)
}

fn load_extras_impl(tr: &Trace, cancel: &dyn Fn() -> bool) -> Extras {
    let t0 = std::time::Instant::now();
    let n = tr.acts.len();
    let failed = |mut args: Vec<EvArgs>, mut ty: Vec<&'static str>, error: String| {
        // Reuse the spine on failure; allocating a second per-action
        // table while the partial pass is live can exceed memory on a
        // multi-million-action capture precisely when we need to refuse.
        args.fill(EvArgs::default());
        ty.fill("other");
        Extras {
            args,
            srcs: Vec::new(),
            labels: Vec::new(),
            ty,
            meta: Vec::new(),
            unmatched: n,
            rejected_fields: 0,
            source_error: Some(error),
            load_ms: t0.elapsed().as_millis(),
        }
    };
    let mut args: Vec<EvArgs> = vec![EvArgs::default(); n];
    let mut ty: Vec<&'static str> = vec!["other"; n];
    let f = match tr.source_reader() {
        Ok(f) => f,
        Err(e) => return failed(args, ty, format!("{e:#}")),
    };
    let mut src_ix: FxHashMap<String, u32> = FxHashMap::default();
    let mut label_ix: FxHashMap<String, u32> = FxHashMap::default();
    let mut meta: Vec<MetaRec> = Vec::new();
    let mut source_order = vec![0u32; n];
    for (i, a) in tr.acts.iter().enumerate() {
        source_order[a.seq as usize] = i as u32;
    }
    let mut next_action = 0;
    let mut seen = 0usize;
    let mut rejected_fields = 0usize;
    let mut numeric_total = 0i128;

    let mut rd = std::io::BufReader::with_capacity(1 << 20, f);
    let mut raw: Vec<u8> = Vec::with_capacity(4096);
    let mut offset: u64 = 0;
    loop {
        if cancel() {
            return failed(args, ty, "analysis cancelled".to_owned());
        }
        raw.clear();
        // Bytes, not read_line: one invalid-UTF-8 line must lose only
        // itself, and only a byte read reports how far to advance the
        // offset join past it.
        let k = match rd.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Err(e) => return failed(args, ty, e.to_string()),
            Ok(k) => k,
        };
        let line = String::from_utf8_lossy(&raw);
        let s = line
            .trim()
            .trim_start_matches('\u{feff}')
            .trim_end_matches(',');
        if s.len() > 2 && s.starts_with('{') {
            let mut repaired = None;
            let parsed = match serde_json::from_str::<JRow>(s) {
                Ok(row) => Ok(row),
                Err(error) => {
                    repaired = crate::parse::repair_optional_args(s);
                    if let Some(repaired) = &repaired {
                        // sanitize counts durable RE rejection markers, including
                        // those the repair just added. Count every field once.
                        rejected_fields += repaired.unmarked_rejections;
                        serde_json::from_str::<JRow>(&repaired.text)
                    } else {
                        Err(error)
                    }
                }
            };
            if let Ok(mut r) = parsed {
                if let Some(row_args) = &mut r.args {
                    rejected_fields += row_args.sanitize(&mut numeric_total);
                }
                if numeric_total > i128::from(i64::MAX / 1000) {
                    return failed(
                        args,
                        ty,
                        "recorded counters exceed the supported aggregate arithmetic range"
                            .to_owned(),
                    );
                }
                if r.ph == Some("X") && r.ts.is_some() {
                    while next_action < n
                        && tr.acts[source_order[next_action] as usize].off < offset
                    {
                        next_action += 1;
                    }
                    let indexed = source_order
                        .get(next_action)
                        .filter(|&&i| tr.acts[i as usize].off == offset);
                    if let (Some(&ai), Some(name)) = (indexed, r.name.as_deref()) {
                        seen += 1;
                        let i = ai as usize;
                        let a = r.args.as_ref();
                        let mut e = EvArgs {
                            decoded: true,
                            ..EvArgs::default()
                        };
                        if let Some(a) = a {
                            if a.cache.is_none()
                                && a.execution_metadata.as_deref() != Some("reused")
                            {
                                e.maxrss = a.maxrss.filter(|n| *n >= 0);
                            }
                            e.cache_ineligible = a.kind.as_deref().is_some_and(|kind| {
                                !matches!(
                                    kind,
                                    "local"
                                        | "remote"
                                        | "local_worker"
                                        | "remote_worker"
                                        | "remote_deduplicated"
                                )
                            });
                            if let Some(sv) = a.src.as_deref() {
                                // intern into the map only; the dense
                                // table is materialized once at the end
                                e.src = Some(match src_ix.get(sv) {
                                    Some(&k) => k,
                                    None => {
                                        let k = src_ix.len() as u32;
                                        src_ix.insert(sv.to_owned(), k);
                                        k
                                    }
                                });
                            }
                            if let Some(c) = &a.cache {
                                e.cache = Some(CacheHit {
                                    bytes: c.bytes,
                                    restore_us: c.restore_us,
                                    lookup_us: c.lookup_us,
                                });
                            }
                            e.bytes_in = a.bytes_in.unwrap_or(0);
                            e.bytes_in_recorded = a.bytes_in.is_some();
                            e.bytes_out = a.bytes_out.unwrap_or(0);
                            e.bytes_out_recorded = a.bytes_out.is_some();
                            e.stages = a.stages.as_ref().and_then(|st| {
                                let execute_us = current_execution_us(
                                    st.execute_us,
                                    a.cache.is_some(),
                                    a.execution_metadata.as_deref(),
                                    st.execute_source.as_deref(),
                                );
                                // Presence is checked after normalization:
                                // empty/unknown-only objects and discarded
                                // historical execution are not measurements.
                                // Explicit zero counters are measurements,
                                // including the permit wait stored under `re`.
                                let measured_mask = [
                                    st.cache_query_us,
                                    st.cache_hit_us,
                                    st.prepare_us,
                                    st.queue_us,
                                    st.materialize_us,
                                    st.hashing_us,
                                    st.cas_artifact_us,
                                    st.local_finalize_outputs_us,
                                    st.local_cache_check_us,
                                    execute_us,
                                    st.upload_us,
                                    st.input_upload_us,
                                    st.result_wait_us,
                                    st.download_us,
                                    st.rtt_us,
                                    st.daemon_us,
                                ]
                                .iter()
                                .enumerate()
                                .fold(0u16, |mask, (i, value)| {
                                    mask | (u16::from(value.is_some()) << i)
                                });
                                if measured_mask == 0
                                    && a.re
                                        .as_ref()
                                        .and_then(|re| re.rs_exec_sem_wait_us)
                                        .is_none()
                                {
                                    return None;
                                }
                                Some(Stages {
                                    measured_mask,
                                    cache_query_us: st.cache_query_us.unwrap_or(0),
                                    cache_hit_us: st.cache_hit_us.unwrap_or(0),
                                    prepare_us: st.prepare_us.unwrap_or(0),
                                    queue_us: st.queue_us,
                                    materialize_us: st.materialize_us.unwrap_or(0),
                                    hashing_us: st.hashing_us.unwrap_or(0),
                                    cas_artifact_us: st.cas_artifact_us.unwrap_or(0),
                                    local_finalize_outputs_us: st
                                        .local_finalize_outputs_us
                                        .unwrap_or(0),
                                    local_cache_check_us: st.local_cache_check_us.unwrap_or(0),
                                    hashing_in_finalize_outputs: hashing_in_finalize_outputs(
                                        st.hashing_source.as_deref(),
                                        st.local_finalize_outputs_us,
                                    ),
                                    execute_us: execute_us.unwrap_or(0),
                                    upload_us: st.upload_us.unwrap_or(0),
                                    input_upload_us: st.input_upload_us.unwrap_or(0),
                                    result_wait_us: st.result_wait_us.unwrap_or(0),
                                    download_us: st.download_us.unwrap_or(0),
                                    rtt_us: st.rtt_us.unwrap_or(0),
                                    daemon_us: st.daemon_us.unwrap_or(0),
                                })
                            });
                            if let Some(re) = &a.re {
                                e.admission_recorded = !re
                                    .invalid_counter_fields
                                    .iter()
                                    .any(|key| key == "rs_exec_sem_wait_us")
                                    && (re.submit_us.is_some() || re.rs_exec_sem_wait_us.is_some());
                                e.submit_us = re.submit_us.unwrap_or(0);
                                e.exec_sem_wait_us = re.rs_exec_sem_wait_us.unwrap_or(0);
                                let invalid = re.upload_invalid();
                                e.iu_bytes_invalid = invalid & UploadSub::BYTES != 0;
                                e.iu_bytes_uploaded =
                                    re.iu_bytes_uploaded.filter(|_| !e.iu_bytes_invalid);
                                e.worker_bytes_in = re.worker_bytes_in;
                                e.worker_bytes_out = re.worker_bytes_out;
                                if re.any() {
                                    e.iu = Some(UploadSub {
                                        sem_wait_us: re.iu_sem_wait_us.unwrap_or(0),
                                        find_missing_us: re.iu_find_missing_us.unwrap_or(0),
                                        materialize_us: re.iu_materialize_us.unwrap_or(0),
                                        cas_call_us: re.iu_cas_call_us.unwrap_or(0),
                                        absent: re.iu_digests_absent.unwrap_or(0),
                                        invalid,
                                    });
                                }
                            }
                            e.tries = a.tries;
                            // Older producers retained original execution
                            // resources on cache hits. They describe history,
                            // not CPU consumed by this capture's lookup.
                            if a.cache.is_none()
                                && a.execution_metadata.as_deref() != Some("reused")
                            {
                                e.cpu_us = a.cpu_us.unwrap_or(0).max(0);
                                e.elastic = a.elastic;
                            }
                            e.attempts = a
                                .attempts
                                .iter()
                                .map(|at| {
                                    let status = at.status.as_deref().unwrap_or("Unknown");
                                    let k = match src_ix.get(status) {
                                        Some(&k) => k,
                                        None => {
                                            let k = src_ix.len() as u32;
                                            src_ix.insert(status.to_owned(), k);
                                            k
                                        }
                                    };
                                    Attempt {
                                        dur_recorded: at.dur_us.is_some(),
                                        start_recorded: at.start.is_some(),
                                        status: k,
                                        dur_us: at.dur_us.unwrap_or(0),
                                        start_us: at.start.unwrap_or(0),
                                        client_start_us: at.client_start,
                                        elastic: at.elastic,
                                    }
                                })
                                .collect();
                            e.winner_client_start_us =
                                a.winner.as_ref().and_then(|w| w.client_start);
                            e.winner_dur_us = a.winner.as_ref().and_then(|w| w.dur_us);
                            // The stage that ended last is the one the losing attempt was in.
                            e.cancelled = a.re.as_ref().and_then(|re| {
                                let last = re
                                    .cancelled_stages
                                    .iter()
                                    .filter(|s| {
                                        s.name.starts_with("Local.") || s.name.starts_with("Re.")
                                    })
                                    .max_by_key(|s| {
                                        s.ts.unwrap_or(0).saturating_add(s.dur.unwrap_or(0))
                                    })?;
                                let phase = match src_ix.get(last.name.as_str()) {
                                    Some(&k) => k,
                                    None => {
                                        let k = src_ix.len() as u32;
                                        src_ix.insert(last.name.clone(), k);
                                        k
                                    }
                                };
                                Some(CancelledAttempt {
                                    local: last.name.starts_with("Local."),
                                    phase,
                                    dur_us: last.dur.unwrap_or(0).max(0),
                                })
                            });
                            e.attempt_status = a
                                .attempt_status
                                .iter()
                                .map(|sv| match src_ix.get(sv.as_ref()) {
                                    Some(&k) => k,
                                    None => {
                                        let k = src_ix.len() as u32;
                                        src_ix.insert(sv.as_ref().to_owned(), k);
                                        k
                                    }
                                })
                                .collect();
                            if let Some(t) = a.target.as_deref() {
                                // "cell//pkg:name (category ident)" -> the label;
                                // intern into the map only, the dense table is
                                // materialized once at the end
                                let lbl = t.rfind(" (").map(|i| &t[..i]).unwrap_or(t);
                                e.label = Some(match label_ix.get(lbl) {
                                    Some(&k) => k,
                                    None => {
                                        let k = label_ix.len() as u32;
                                        label_ix.insert(lbl.to_owned(), k);
                                        k
                                    }
                                });
                            }
                            e.exec_us = current_execution_us(
                                a.exec_us,
                                a.cache.is_some(),
                                a.execution_metadata.as_deref(),
                                None,
                            );
                            e.wall_time_us = a.wall_time_us;
                            if r.cat == Some("recipe") {
                                ty[i] = classify_type(a.src.as_deref().unwrap_or(""), name);
                            }
                        }
                        args[i] = e;
                    }
                } else if r.ph == Some("M") && matches!(r.cat, Some("run" | "command-profile")) {
                    // The same text the row was parsed from, repaired if it needed to be.
                    let text = repaired.as_ref().map_or(s, |fixed| fixed.text.as_str());
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                        meta.push(MetaRec {
                            name: r.name.as_deref().unwrap_or("").to_owned(),
                            ts: r.ts.unwrap_or(0),
                            args: v.get("args").cloned().unwrap_or(serde_json::Value::Null),
                        });
                    }
                }
            }
        }
        offset += k as u64;
    }
    if let Err(e) = tr.validate_source() {
        return failed(args, ty, format!("{e:#}"));
    }
    let mut srcs = vec![String::new(); src_ix.len()];
    for (s, k) in src_ix {
        srcs[k as usize] = s;
    }
    let mut labels = vec![String::new(); label_ix.len()];
    for (s, k) in label_ix {
        labels[k as usize] = s;
    }
    Extras {
        args,
        srcs,
        labels,
        ty,
        meta,
        unmatched: n.saturating_sub(seen),
        rejected_fields,
        source_error: None,
        load_ms: t0.elapsed().as_millis(),
    }
}

// ===================================================================
// §3  Facts: the analysis event list over a selected run
// ===================================================================

pub struct Facts<'a> {
    pub tr: &'a Trace,
    pub x: &'a Extras,
    /// event index -> `Trace::acts` index (ts ascending, file order ties)
    pub ev: Vec<u32>,
    /// event indices kept by run selection (indexes `ev`)
    pub sel: Vec<u32>,
    pub run: Option<String>,
}

impl<'a> Facts<'a> {
    /// `want`: the run id analyses scope to (from `parse::select_run`).
    /// `None` is only valid for a single-run trace; several runs with no
    /// selection — or an id matching nothing — is a loud error
    /// (docs/contract.md: guessing analyses the wrong build).
    pub fn for_run(tr: &'a Trace, x: &'a Extras, want: Option<&str>) -> anyhow::Result<Facts<'a>> {
        let mut ev: Vec<u32> = (0..tr.acts.len() as u32)
            .filter(|&i| tr.acts[i as usize].flags & F_NOT_RECIPE == 0)
            .collect();
        ev.sort_by_key(|&i| (tr.acts[i as usize].ts, tr.acts[i as usize].seq));

        // Invocation identity comes from metadata even when no action ran.
        let run = crate::parse::select_trace_run(tr, want)?;
        let sel = (0..ev.len() as u32)
            .filter(|&e| tr.action_in_run(&tr.acts[ev[e as usize] as usize], run.as_deref()))
            .collect();
        Ok(Facts {
            tr,
            x,
            ev,
            sel,
            run,
        })
    }

    #[inline]
    pub fn act(&self, e: u32) -> &crate::types::Act {
        &self.tr.acts[self.ev[e as usize] as usize]
    }
    #[inline]
    pub fn name(&self, e: u32) -> &str {
        &self.tr.names[self.act(e).name as usize]
    }
    #[inline]
    pub fn ts(&self, e: u32) -> i64 {
        self.act(e).ts
    }
    #[inline]
    pub fn dur(&self, e: u32) -> i64 {
        self.act(e).dur
    }
    #[inline]
    pub fn args(&self, e: u32) -> &EvArgs {
        self.x.action_args(self.ev[e as usize] as usize)
    }
    #[inline]
    pub fn src(&self, e: u32) -> Option<&str> {
        self.args(e).src.map(|i| self.x.srcs[i as usize].as_str())
    }
    #[inline]
    pub fn ty(&self, e: u32) -> &'static str {
        self.x
            .ty
            .get(self.ev[e as usize] as usize)
            .copied()
            .unwrap_or("other")
    }
    #[inline]
    pub fn why(&self, e: u32) -> Option<&str> {
        let w = self.act(e).why;
        if w == NO_STR {
            None
        } else {
            Some(&self.tr.whys[w as usize])
        }
    }
}

// ===================================================================
// §4  run records: bounds, totals, cache stats
// ===================================================================

/// First `run_start` (matching the run when given) and the matching
/// `run_end`, as indices into `Extras::meta`.
pub fn run_bounds(meta: &[MetaRec], run: Option<&str>) -> (Option<usize>, Option<usize>) {
    let start = meta
        .iter()
        .position(|m| m.name == "run_start" && run.is_none_or(|r| m.s("run") == Some(r)));
    let want = run.or_else(|| start.and_then(|i| meta[i].s("run")));
    let end = meta
        .iter()
        .position(|m| m.name == "run_end" && want.is_none_or(|r| m.s("run") == Some(r)));
    (start, end)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RunTotals {
    pub remade: i64,
    pub uptodate: i64,
    pub failed: i64,
}

pub fn run_totals(meta: &[MetaRec], run: Option<&str>) -> Option<RunTotals> {
    let end = &meta[run_bounds(meta, run).1?];
    Some(RunTotals {
        remade: end.i("remade")?,
        uptodate: end.i("uptodate").unwrap_or(0),
        failed: end.i("failed").unwrap_or(0),
    })
}

pub const CACHE_KEYS: [&str; 12] = [
    "hits",
    "misses",
    "miss_inputs",
    "miss_errors",
    "stores",
    "uncacheable",
    "restored_bytes",
    "stored_bytes",
    "dedup_bytes",
    "key_us",
    "restore_us",
    "store_us",
];

/// The 12 `run_end.cache` counters, in `CACHE_KEYS` order.
pub fn cache_stats(meta: &[MetaRec], run: Option<&str>) -> Option<[i64; 12]> {
    let cache = meta[run_bounds(meta, run).1?].args.get("cache")?;
    Some(CACHE_KEYS.map(|key| cache.get(key).and_then(|v| v.as_i64()).unwrap_or(0)))
}

// ===================================================================
// §5  overview
// ===================================================================

#[derive(Clone, Debug)]
pub struct OverviewFacts {
    /// indices into `Extras::meta`
    pub start: Option<usize>,
    pub end: Option<usize>,
    /// absolute µs of the first selected action
    pub t0: i64,
    /// `t1 - t0` over selected actions, including measured zero
    pub span: i64,
    pub busy: i64,
    pub nwork: usize,
    pub nfail: usize,
}

pub fn overview_facts(f: &Facts) -> OverviewFacts {
    let t0r = f.sel.iter().map(|&e| f.ts(e)).min().unwrap_or(0);
    let t1r = f.sel.iter().map(|&e| f.ts(e) + f.dur(e)).max().unwrap_or(0);
    let (start, end) = run_bounds(&f.x.meta, f.run.as_deref());
    let nfail = f
        .sel
        .iter()
        .filter(|&&e| f.act(e).flags & F_FAILED != 0)
        .count();
    OverviewFacts {
        start,
        end,
        t0: t0r + f.tr.t0_abs,
        span: t1r - t0r,
        busy: f.sel.iter().map(|&e| f.dur(e)).sum(),
        nwork: f.sel.len(),
        nfail,
    }
}

/// One Build-overview row: key, value, and the panel its button opens.
#[derive(Clone, Debug, PartialEq)]
pub struct OverviewRow {
    pub key: &'static str,
    pub value: String,
    /// `"critical_path" | "stall_audit" | "breakdown"`
    pub link: Option<&'static str>,
}

/// `re.sub(r"\.tmp\.\w+\.\w+$", "", p)` — buck2 atomic-save temp names.
fn strip_tmp_suffix(p: &str) -> String {
    let w = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    let mut it = p.rsplitn(3, '.');
    let (c, b, head) = (it.next(), it.next(), it.next());
    if let (Some(c), Some(b), Some(head)) = (c, b, head)
        && w(c)
        && w(b)
        && let Some(rest) = head.strip_suffix(".tmp")
    {
        return rest.to_owned();
    }
    p.to_owned()
}

/// The Build-overview rows. `cp` is the graph critical path when a
/// graph was stitched; `when` renders wall-clock (UTC headless, local
/// in the GUI).
pub fn overview_rows(
    f: &Facts,
    o: &OverviewFacts,
    cp: Option<&crate::dag::CpChain>,
    when: &dyn Fn(i64) -> String,
) -> Vec<OverviewRow> {
    let meta = &f.x.meta;
    let start = o.start.map(|i| &meta[i]);
    let end = o.end.map(|i| &meta[i]);
    let no_hdr = "(not recorded — capture carries no run header)".to_owned();
    let empty = serde_json::Value::Null;
    let sa = start.map(|m| &m.args).unwrap_or(&empty);
    let s = |k: &str| sa.get(k).and_then(|v| v.as_str());
    let i = |k: &str| sa.get(k).and_then(|v| v.as_i64());

    let mut when_s = when(start.map(|m| m.ts).unwrap_or(o.t0));
    if start.is_none() {
        when_s.push_str("  (from the first action; no run header)");
    }
    let (invocation, jobs_label, who, toolinfo, cwd) = if start.is_some() {
        let argv: Vec<String> = match sa.get("argv") {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().unwrap_or_default().to_owned())
                .collect(),
            _ => Vec::new(),
        };
        let jobs = i("jobs").unwrap_or(-1);
        let jl = if jobs == 0 {
            "unlimited (-j with no limit)".to_owned()
        } else if jobs > 0 {
            format!("{jobs} (-j{jobs})")
        } else {
            "default (parallel; buck2 schedules to available resources)".to_owned()
        };
        (
            shlex_join(&argv),
            jl,
            match s("user") {
                Some(u) => u.to_owned(),
                None => "(not recorded in the event log)".to_owned(),
            },
            format!("buck2 — run {}", s("run").unwrap_or("?")),
            s("cwd").unwrap_or("?").to_owned(),
        )
    } else {
        let dir = f
            .sel
            .first()
            .map(|&e| {
                let d = f.act(e).dir;
                if d == NO_STR {
                    String::new()
                } else {
                    f.tr.dirs[d as usize].clone()
                }
            })
            .unwrap_or_default();
        (
            no_hdr.clone(),
            no_hdr.clone(),
            no_hdr.clone(),
            no_hdr.clone(),
            dir,
        )
    };
    let machine = {
        let spec = i("ncpu").filter(|&n| n > 0).map(|n| {
            let mut parts = vec![format!("{n} cores")];
            if let Some(m) = i("mem").filter(|&m| m > 0) {
                parts.push(format!("{:.0} GiB RAM", m as f64 / 1073741824.0));
            }
            parts.join(", ")
        });
        match (s("hostname"), spec) {
            (Some(h), Some(sp)) => Some(format!("{h} — {sp}")),
            (Some(h), None) => Some(h.to_owned()),
            (None, sp) => sp,
        }
    };
    let totals = run_totals(meta, f.run.as_deref());

    let mut result;
    let wall;
    match end {
        Some(e) => {
            result = match Outcome::from_run_end_args(&e.args) {
                Outcome::Success => "exit 0 — success".to_owned(),
                Outcome::Failed => match e.i("exit").or_else(|| e.i("result")) {
                    Some(code) => format!("exit {code} — FAILED"),
                    None => "FAILED — exit code not recorded".to_owned(),
                },
                Outcome::Unknown => "unknown — no reliable command outcome was recorded".to_owned(),
            };
            wall = match start {
                Some(st0) if e.ts >= st0.ts => format!(
                    "{}  (first-to-last action: {}; remaining time is outside that action window)",
                    fmt_dur(e.ts - st0.ts),
                    fmt_dur(o.span),
                ),
                Some(_) => "unavailable: run_end precedes run_start".to_owned(),
                None => fmt_dur(o.span),
            };
        }
        None => {
            result = "(no run_end record — the build was killed mid-run, or the capture is torn)"
                .to_owned();
            wall = format!("{}  (first-to-last action)", fmt_dur(o.span));
        }
    }
    if o.nfail != 0 {
        result.push_str(&format!("; {} failed action(s)", o.nfail));
    }
    let par = if o.span != 0 {
        o.busy as f64 / o.span as f64
    } else {
        0.0
    };

    let mut rows: Vec<OverviewRow> = vec![
        OverviewRow {
            key: "Date",
            value: when_s,
            link: None,
        },
        OverviewRow {
            key: "Ran by",
            value: who,
            link: None,
        },
        OverviewRow {
            key: "Directory",
            value: cwd,
            link: None,
        },
        OverviewRow {
            key: "Invocation",
            value: invocation,
            link: None,
        },
        OverviewRow {
            key: "Job limit",
            value: jobs_label,
            link: None,
        },
        OverviewRow {
            key: "Build tool",
            value: toolinfo,
            link: None,
        },
        OverviewRow {
            key: "Result",
            value: result,
            link: None,
        },
        OverviewRow {
            key: "Actions run",
            value: format!(
                "{} actions (one buck2 daemon){}",
                fmt_n(o.nwork as f64),
                match totals {
                    Some(t) => format!(
                        "; targets: {} remade, {} already up to date, {} failed",
                        fmt_n(t.remade as f64),
                        fmt_n(t.uptodate as f64),
                        fmt_n(t.failed as f64)
                    ),
                    None => String::new(),
                }
            ),
            link: None,
        },
        OverviewRow {
            key: "Total time",
            value: wall,
            link: None,
        },
        OverviewRow {
            key: "Action wall sum",
            value: format!(
                "{} — average in-flight actions ×{par:.1}; includes cache lookups and waits, not CPU work",
                fmt_secs(o.busy as f64)
            ),
            link: Some("breakdown"),
        },
    ];
    // What the build was called on the command line. The titlebar shows
    // this (elided); here it is in full, because a 50-target build has no
    // short name and the user still needs to know which targets they are
    // looking at.
    let targets = f.tr.run_targets(f.run.as_deref());
    if !targets.is_empty() {
        let idx = rows
            .iter()
            .position(|r| r.key == "Invocation")
            .map(|i| i + 1)
            .unwrap_or(rows.len());
        rows.insert(
            idx,
            OverviewRow {
                key: "Targets",
                value: targets.join(" "),
                link: None,
            },
        );
    }
    if let Some(serde_json::Value::Array(fc)) = sa.get("file_changes") {
        let mut norm: Vec<String> = Vec::new();
        for p in fc {
            let p = p.as_str().unwrap_or_default();
            let last = p.rsplit("//").next().unwrap_or(p);
            let cleaned = strip_tmp_suffix(last);
            if !norm.iter().any(|x| x == &cleaned) {
                norm.push(cleaned);
            }
        }
        let mut shown = norm.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let total = i("file_changes_total").filter(|n| *n >= 0);
        if norm.len() > 3 {
            shown.push_str(", …");
        }
        shown.push_str(&match total {
            Some(total) => format!(
                " (producer retained {} paths; {} shown in this capture)",
                fmt_n(total as f64),
                norm.len()
            ),
            None => format!(" ({} retained paths; total not recorded)", norm.len()),
        });
        // anchored, not a fixed index: rows above this one are
        // conditional, and a hard 7 silently files this under whatever
        // happens to be seventh
        let idx = rows
            .iter()
            .position(|r| r.key == "Actions run")
            .unwrap_or(rows.len());
        rows.insert(
            idx,
            OverviewRow {
                key: "Changed files",
                value: format!(
                    "{shown} — watcher observations, not proof of which changes caused each action to run"
                ),
                link: None,
            },
        );
    }
    if let Some(watcher) = sa.get("file_watcher").filter(|v| v.is_object()) {
        rows.push(OverviewRow {
            key: "File watcher",
            value: watcher_summary(watcher),
            link: None,
        });
    }
    if let Some(c) = cache_stats(meta, f.run.as_deref()) {
        let n = c[0]
            .saturating_add(c[1])
            .saturating_add(c[2])
            .saturating_add(c[3]);
        let pct = if n != 0 {
            100.0 * c[0] as f64 / n as f64
        } else {
            0.0
        };
        let mut parts = vec![format!(
            "{} hits / {} misses ({pct:.0}% of {} cacheable actions)",
            fmt_n(c[0] as f64),
            fmt_n(n.saturating_sub(c[0]) as f64),
            fmt_n(n as f64)
        )];
        let recorded_cache = end.and_then(|e| e.args.get("cache"));
        let recorded = |key: &str| {
            recorded_cache
                .and_then(|c| c.get(key))
                .and_then(serde_json::Value::as_i64)
        };
        if let Some(bytes) = recorded("restored_bytes") {
            parts.push(format!(
                "cached output logical size {} (not transferred bytes)",
                fmt_bytes(bytes),
            ));
        }
        if c[2] != 0 {
            parts.push(format!(
                "{} misses from changed discovered inputs (depfile manifest)",
                fmt_n(c[2] as f64)
            ));
        }
        if c[5] != 0 {
            parts.push(format!(
                "{} actions outside the reported cache lookup population",
                fmt_n(c[5] as f64)
            ));
        }
        // `key_us` is the cache-query span end to end: wall from dispatch
        // to response, which on a large build is mostly time queued behind
        // the client's own concurrency limit.  Nothing in the log measures
        // hashing.
        if let Some(us) = recorded("key_us") {
            parts.push(format!("{} in reported cache lookups (wall, queueing included; population follows producer)", fmt_dur(us)));
        }
        rows.push(OverviewRow {
            key: "Action cache",
            value: parts.join("; "),
            link: None,
        });
    }
    rows.push(match cp {
        Some(c) => OverviewRow {
            key: "Action model CP",
            value: format!(
                "{} over {} steps — longest path over recorded action dependencies and \
                 action durations; not the command-wide critical path reported by Buck.",
                fmt_dur(c.cp_us),
                c.entries.len(),
            ),
            link: Some("critical_path"),
        },
        None => OverviewRow {
            key: "Action model CP",
            value: "needs the dependency graph (a sibling *-graph.json, or --graph=); \
                    action timestamps alone do not establish dependencies"
                .to_owned(),
            link: None,
        },
    });
    // The artifact this window is rendering. It used to be the titlebar,
    // which is where nobody needs a 100-character path; provenance
    // belongs here, next to the rest of the capture's identity.
    if !f.tr.path.is_empty() {
        rows.push(OverviewRow {
            key: "Trace file",
            value: f.tr.path.clone(),
            link: None,
        });
    }
    let idx = rows
        .iter()
        .position(|r| r.key == "Build tool")
        .map(|i| i + 1)
        .unwrap_or(0);
    if let Some(m) = machine {
        rows.insert(
            idx,
            OverviewRow {
                key: "Machine",
                value: m,
                link: None,
            },
        );
    }
    rows
}

fn watcher_summary(watcher: &serde_json::Value) -> String {
    let count = |key: &str| {
        watcher
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(crate::humanize::count)
            .unwrap_or_else(|| "not recorded".to_owned())
    };
    let mut parts = vec![format!(
        "events processed: {}; events reported: {} (event counts, not unique files)",
        count("events_processed"),
        count("events_total")
    )];
    if watcher
        .get("fresh_instance")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        parts.push(
            "fresh watcher instance; an incremental change list may be unavailable".to_owned(),
        );
    }
    if watcher
        .get("paths_truncated")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        parts.push("changed paths are a truncated sample".to_owned());
    }
    if let Some(reasons) = watcher
        .get("incomplete_events_reasons")
        .and_then(serde_json::Value::as_array)
    {
        for reason in reasons.iter().filter_map(serde_json::Value::as_str) {
            parts.push(format!("producer limitation: {reason}"));
        }
    }
    if watcher
        .get("reasons_truncated")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        parts.push("additional producer limitations omitted".to_owned());
    }
    parts.join("; ")
}

// ===================================================================
// §6  breakdown / rule sites / top memory
// ===================================================================

/// Table columns: `(heading, width px, anchor)`.
pub type Columns = &'static [(&'static str, u32, char)];

pub const BREAKDOWN_COLUMNS: Columns = &[
    ("type", 110, 'w'),
    ("actions", 70, 'e'),
    ("total", 90, 'e'),
    ("share of work", 100, 'e'),
    ("avg", 90, 'e'),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BreakdownRow {
    pub ty: &'static str,
    pub count: i64,
    pub total_us: i64,
    /// integer floor of `total * 100 / busy`
    pub share_pct: i64,
    pub avg_us: i64,
}

/// A row that holds time but rounds below one percent reads `<1%`, so it is
/// not confused with a row that holds none.
fn share_cell(share_pct: i64, total_us: i64) -> String {
    if share_pct == 0 && total_us > 0 {
        "<1%".to_owned()
    } else {
        format!("{share_pct}%")
    }
}

impl BreakdownRow {
    pub fn cells(&self) -> Vec<String> {
        vec![
            self.ty.to_owned(),
            self.count.to_string(),
            fmt_dur(self.total_us),
            share_cell(self.share_pct, self.total_us),
            fmt_dur(self.avg_us),
        ]
    }
}

/// Work by build type, longest first (ties keep first-seen order).
pub fn breakdown(f: &Facts) -> Vec<BreakdownRow> {
    let busy: i64 = f.sel.iter().map(|&e| f.dur(e)).sum();
    let mut agg: Vec<(&'static str, i64, i64)> = Vec::new();
    for &e in &f.sel {
        let t = f.ty(e);
        match agg.iter_mut().find(|r| r.0 == t) {
            Some(r) => {
                r.1 += f.dur(e);
                r.2 += 1;
            }
            None => agg.push((t, f.dur(e), 1)),
        }
    }
    agg.sort_by_key(|r| -r.1); // stable
    agg.into_iter()
        .map(|(ty, total_us, count)| BreakdownRow {
            ty,
            count,
            total_us,
            share_pct: total_us * 100 / busy.max(1),
            avg_us: total_us / count.max(1),
        })
        .collect()
}

pub const RULE_SITES_COLUMNS: Columns = &[
    ("category", 300, 'w'),
    ("actions", 80, 'e'),
    ("total", 100, 'e'),
    ("share of work", 100, 'e'),
];

pub const RULE_SITES_N: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleSiteRow {
    pub rule: String,
    pub count: i64,
    pub total_us: i64,
    pub share_pct: i64,
}

impl RuleSiteRow {
    pub fn cells(&self) -> Vec<String> {
        vec![
            self.rule.clone(),
            self.count.to_string(),
            fmt_dur(self.total_us),
            share_cell(self.share_pct, self.total_us),
        ]
    }
}

/// Work per action category (`args.src`), longest first.
pub fn rule_sites(f: &Facts, n: usize) -> Vec<RuleSiteRow> {
    let busy: i64 = f.sel.iter().map(|&e| f.dur(e)).sum();
    let mut ix: FxHashMap<&str, usize> = FxHashMap::default();
    let mut agg: Vec<(i64, i64)> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for &e in &f.sel {
        let src = match f.src(e) {
            Some(s) if !s.is_empty() => s,
            _ => "(unrecorded)",
        };
        match ix.get(src) {
            Some(&k) => {
                agg[k].0 += f.dur(e);
                agg[k].1 += 1;
            }
            None => {
                let k = agg.len();
                ix.insert(src, k);
                names.push(src.to_owned());
                agg.push((f.dur(e), 1));
            }
        }
    }
    let mut idx: Vec<usize> = (0..agg.len()).collect();
    idx.sort_by_key(|&k| -agg[k].0); // stable
    idx.truncate(n);
    idx.into_iter()
        .map(|k| RuleSiteRow {
            rule: names[k].clone(),
            count: agg[k].1,
            total_us: agg[k].0,
            share_pct: agg[k].0 * 100 / busy.max(1),
        })
        .collect()
}

pub const TOP_MEMORY_COLUMNS: Columns = &[
    ("peak RSS", 90, 'e'),
    ("target", 280, 'w'),
    ("type", 80, 'w'),
    ("wall", 80, 'e'),
    ("exec", 80, 'e'),
    ("category", 150, 'w'),
];

pub const TOP_MEMORY_N: usize = 10;

pub const TOP_MEMORY_REFUSAL: &str = "No selected action carries a positive current-execution peak-RSS measurement. Reused cache-result resource metadata is historical and is excluded.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemRow {
    /// `Facts` event index, for navigation
    pub ev: u32,
    pub maxrss: i64,
    pub target: String,
    pub ty: &'static str,
    pub dur_us: i64,
    /// executor time (`args.exec_us`); `None` when the capture did not record it
    pub exec_us: Option<i64>,
    pub rule: String,
}

impl MemRow {
    pub fn cells(&self) -> Vec<String> {
        vec![
            format!("{:.0} MiB", self.maxrss as f64 / (1u64 << 20) as f64),
            self.target.clone(),
            self.ty.to_owned(),
            fmt_dur(self.dur_us),
            self.exec_us.map_or_else(|| "\u{2014}".to_owned(), fmt_dur),
            self.rule.clone(),
        ]
    }
}

/// Build-median effective input-fetch rate (bytes/s) over actions that
/// moved more than 10 MiB of inputs — the denominator the popup uses to
/// put one action's fetch rate in context. `None` under 25 samples: a
/// median of a handful of transfers is noise wearing a number.
pub fn fetch_rate_median(f: &Facts) -> Option<f64> {
    let mut rates: Vec<f64> = Vec::new();
    for &e in &f.sel {
        let a = f.args(e);
        let Some(st) = &a.stages else { continue };
        if a.bytes_in > 10 * 1024 * 1024 && st.materialize_us >= 1_000 {
            rates.push(a.bytes_in as f64 / (st.materialize_us as f64 / 1e6));
        }
    }
    if rates.len() < 25 {
        return None;
    }
    rates.sort_by(|x, y| x.partial_cmp(y).expect("rates are finite"));
    Some(rates[rates.len() / 2])
}

/// Top `n` actions by peak RSS. Empty = no resource data; render the refusal.
pub fn top_memory(f: &Facts, n: usize) -> Vec<MemRow> {
    let mut have: Vec<u32> = f
        .sel
        .iter()
        .copied()
        .filter(|&e| f.args(e).maxrss.unwrap_or(0) != 0)
        .collect();
    have.sort_by_key(|&e| -f.args(e).maxrss.unwrap_or(0)); // stable
    have.truncate(n);
    have.into_iter()
        .map(|e| {
            let a = f.args(e);
            MemRow {
                ev: e,
                maxrss: a.maxrss.unwrap_or(0),
                target: f.name(e).to_owned(),
                ty: f.ty(e),
                dur_us: f.dur(e),
                exec_us: a.exec_us,
                rule: match f.src(e) {
                    Some(s) if !s.is_empty() => s.to_owned(),
                    _ => "(unrecorded)".to_owned(),
                },
            }
        })
        .collect()
}

// ===================================================================
// §7  RE queue
// ===================================================================

/// RE-queue-wait figures over the selected run, coverage first (absent
/// stages are not zeros).
pub struct QueueOverview {
    /// Positive queue durations; measured zero contributes only to coverage.
    pub waited: usize,
    pub covered: usize,
    pub total: usize,
    pub total_us: i64,
    pub mean_us: i64,
    /// nearest-rank, the codebase's percentile convention
    pub p90_us: i64,
    pub max_us: i64,
    /// event index of the worst wait, for navigation
    pub max_ev: u32,
}

/// Temporal relationship of a discarded attempt to the final one.
/// buck2 marks an attempt
/// `Cancelled` when its hybrid executor drops the losing side of a
/// local-vs-remote race (`CommandExecution.Cancelled` in
/// `buck2_data/data.proto` is empty and documented as hybrid-only), so
/// the status alone says nothing about worker health. Timing can
/// establish overlap or ordering, but not the counterfactual effect of
/// removing an attempt on action or whole-build latency.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttemptFate {
    /// overlapped the attempt that won; its causal delay is not measured
    Concurrent,
    /// ended before the winner started — the action re-ran after it
    Rerun,
    /// the capture gives no interval retrace can compare
    Unknown,
}

impl AttemptFate {
    pub fn as_str(self) -> &'static str {
        match self {
            AttemptFate::Concurrent => "concurrent",
            AttemptFate::Rerun => "rerun",
            AttemptFate::Unknown => "unknown",
        }
    }
}

/// How a fate was decided, carried beside it so a reader can tell an
/// exact placement from one inferred from durations alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FateBasis {
    /// both attempts carry a start on the buck2 host's clock
    ClientInterval,
    /// the two client durations cannot both fit inside the action's
    /// span, so they overlapped; nothing else is decidable this way
    DurationOverflow,
    /// no rule applied
    None,
}

impl FateBasis {
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            FateBasis::ClientInterval => Some("client-interval"),
            FateBasis::DurationOverflow => Some("duration-overflow"),
            FateBasis::None => None,
        }
    }
}

/// One attempt's timing as the capture recorded it. `dur_us` is the
/// attempt's client-clock span; `client_start_us` is absolute µs on the
/// buck2 host's clock, the only start two attempts of one action share.
#[derive(Clone, Copy, Debug, Default)]
pub struct AttemptTiming {
    pub client_start_us: Option<i64>,
    pub dur_us: Option<i64>,
}

/// Classify one discarded attempt against the attempt that won.
///
/// With a client start on both sides the comparison is exact: same
/// host clock, same boundaries. Without one, overlap is certain only
/// when the two client durations together exceed the action's span
/// (both lie inside it); anything else is undecidable from durations
/// and reads `Unknown`. A zero-length span is a record buck2 wrote
/// without measuring the attempt (an attempt cancelled in the queue),
/// so its start is not the attempt's start and it is `Unknown` even
/// with client starts. A missing winner duration is `Unknown`, never a
/// zero-length winner at the action's end. The comparison assumes the
/// host's clock did not step between the two attempts.
pub fn attempt_fate(
    loser: AttemptTiming,
    winner: AttemptTiming,
    act_start_us: i64,
    act_dur_us: i64,
) -> (AttemptFate, FateBasis) {
    let unknown = (AttemptFate::Unknown, FateBasis::None);
    let (Some(ld), Some(wd)) = (
        loser.dur_us.filter(|&d| d > 0 && d <= act_dur_us),
        winner.dur_us.filter(|&d| d > 0 && d <= act_dur_us),
    ) else {
        return unknown;
    };
    let Some(act_end) = act_start_us.checked_add(act_dur_us) else {
        return unknown;
    };
    // Starts and durations are one interval. Reject contradictory
    // recorded starts instead of falling through to duration inference.
    for (start, duration) in [(loser.client_start_us, ld), (winner.client_start_us, wd)] {
        if let Some(start) = start {
            if start < act_start_us || start.checked_add(duration).is_none_or(|e| e > act_end) {
                return unknown;
            }
        }
    }
    if let (Some(ls), Some(ws)) = (loser.client_start_us, winner.client_start_us) {
        let fate = if ls + ld <= ws {
            AttemptFate::Rerun
        } else if ls < ws + wd {
            AttemptFate::Concurrent
        } else {
            // A discarded attempt starting after the winner finished
            // does not fit either causal account this census reports.
            return unknown;
        };
        return (fate, FateBasis::ClientInterval);
    }
    if i128::from(ld) + i128::from(wd) > i128::from(act_dur_us) {
        (AttemptFate::Concurrent, FateBasis::DurationOverflow)
    } else {
        unknown
    }
}

/// Hybrid races whose losing side buck2 cancelled: what that attempt was doing and for how
/// long. Its time ran alongside the winning attempt, so it is not added to the action's time.
pub struct CancelledCensus {
    pub actions: usize,
    /// of those, local attempts (the rest were remote)
    pub local: usize,
    /// summed time of the stage each attempt was cancelled in
    pub cancelled_us: i64,
    /// (stage, actions), descending
    pub by_phase: Vec<(String, usize)>,
    /// actions whose upload spent at least [`SLOW_GATHER_US`] gathering local files while their
    /// local attempt was getting the same inputs onto this machine, and that time summed
    pub slow_gathers: usize,
    pub slow_gather_us: i64,
    /// every row, sorted by cancelled time descending
    pub rows: Vec<CancelledRow>,
}

/// An upload's "gathering local files" time worth calling out.
pub const SLOW_GATHER_US: i64 = 1_000_000;

#[derive(Clone, Debug)]
pub struct CancelledRow {
    pub ev: u32,
    pub local: bool,
    pub phase: String,
    pub cancelled_us: i64,
    /// the action's own upload time gathering local files, when it uploaded inputs
    pub gather_us: Option<i64>,
    pub bytes_in: i64,
}

pub fn cancelled_census(f: &Facts) -> Option<CancelledCensus> {
    let mut c = CancelledCensus {
        actions: 0,
        local: 0,
        cancelled_us: 0,
        by_phase: Vec::new(),
        slow_gathers: 0,
        slow_gather_us: 0,
        rows: Vec::new(),
    };
    let mut phases: FxHashMap<u32, usize> = FxHashMap::default();
    for &e in &f.sel {
        let a = f.args(e);
        let Some(cancelled) = a.cancelled else {
            continue;
        };
        c.actions += 1;
        c.local += usize::from(cancelled.local);
        c.cancelled_us = c.cancelled_us.saturating_add(cancelled.dur_us);
        *phases.entry(cancelled.phase).or_default() += 1;
        let gather_us =
            a.iu.filter(|iu| iu.valid(UploadSub::MATERIALIZE))
                .map(|iu| iu.materialize_us);
        if cancelled.preparing_local_inputs(&f.x.srcs)
            && let Some(gather) = gather_us.filter(|&g| g >= SLOW_GATHER_US)
        {
            c.slow_gathers += 1;
            c.slow_gather_us = c.slow_gather_us.saturating_add(gather);
        }
        c.rows.push(CancelledRow {
            ev: e,
            local: cancelled.local,
            phase: f
                .x
                .srcs
                .get(cancelled.phase as usize)
                .cloned()
                .unwrap_or_default(),
            cancelled_us: cancelled.dur_us,
            gather_us,
            bytes_in: a.bytes_in,
        });
    }
    if c.actions == 0 {
        return None;
    }
    let name = |k: u32| f.x.srcs.get(k as usize).cloned().unwrap_or_default();
    c.by_phase = phases.into_iter().map(|(k, n)| (name(k), n)).collect();
    c.by_phase
        .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    c.rows
        .sort_by_key(|r| (std::cmp::Reverse(r.cancelled_us), r.ev));
    Some(c)
}

/// Retry census: how many actions retried, the discarded attempts'
/// recorded command elapsed time, and their timing relative to the winner.
/// Overlap and ordering do not establish the causal build impact of
/// removing an attempt.
pub struct RetryCensus {
    /// actions that ran more than once
    pub actions: usize,
    /// extra attempts beyond the first, summed
    pub extra_attempts: usize,
    /// command elapsed time of the discarded attempts, as buck2
    /// measured each (it can include client queueing and download, so
    /// it is not worker occupancy or a measured build-delay counter)
    pub lost_us: i64,
    /// discarded attempts that overlapped the attempt that won
    pub concurrent: usize,
    /// discarded attempts that ended before the winner started
    pub rerun: usize,
    /// discarded attempts with a record but no interval retrace can
    /// compare; counted in `lost_us`, never in `delay_us`
    pub unknown: usize,
    /// of the classified attempts, how many were placed by an exact
    /// client interval rather than by duration overflow
    pub exact_fates: usize,
    /// summed duration of the classified pre-winner attempts: candidate
    /// removable command time, not proven action or whole-build savings
    pub delay_us: i64,
    /// action-span wall time of the actions that retried, not the
    /// winning attempt's command duration
    pub retried_dur_us: i64,
    /// (status, actions, extra attempts), descending by attempts
    pub by_status: Vec<(String, usize, usize)>,
    /// per category: (category, actions, extra attempts), descending
    pub by_category: Vec<(String, usize, usize)>,
    /// Distinct categories before the display limit is applied.
    pub n_categories: usize,
    /// every retried action, sorted by lost time descending
    pub rows: Vec<RetryRow>,
}

/// One discarded attempt as the census reports it.
#[derive(Clone, Debug)]
pub struct AttemptRow {
    pub dur_recorded: bool,
    pub start_recorded: bool,
    pub status: String,
    pub dur_us: i64,
    pub start_us: i64,
    pub client_start_us: Option<i64>,
    pub elastic: bool,
}

#[derive(Clone)]
pub struct RetryRow {
    pub ev: u32,
    pub tries: i64,
    pub lost_us: i64,
    pub attempts: Vec<AttemptRow>,
    /// parallel to `attempts`: the fate and how it was decided
    pub fates: Vec<(AttemptFate, FateBasis)>,
    /// summed duration of this row's `Rerun` attempts
    pub delay_us: i64,
}

pub fn retry_census(f: &Facts, topk: usize) -> Option<RetryCensus> {
    let mut c = RetryCensus {
        actions: 0,
        extra_attempts: 0,
        concurrent: 0,
        rerun: 0,
        unknown: 0,
        exact_fates: 0,
        delay_us: 0,
        lost_us: 0,
        retried_dur_us: 0,
        by_status: Vec::new(),
        by_category: Vec::new(),
        n_categories: 0,
        rows: Vec::new(),
    };
    let mut status: FxHashMap<u32, (usize, usize)> = FxHashMap::default();
    let mut cat: FxHashMap<Option<u32>, (usize, usize)> = FxHashMap::default();
    for &e in &f.sel {
        let a = f.args(e);
        let Some(tries) = a.tries.filter(|&t| t > 1) else {
            continue;
        };
        let extra = (tries - 1) as usize;
        c.actions += 1;
        c.extra_attempts += extra;
        let act = f.act(e);
        c.retried_dur_us += act.dur;
        let lost: i64 = a.attempts.iter().map(|at| at.dur_us.max(0)).sum();
        c.lost_us += lost;
        // Only the winning command's own span can be paired with its
        // client start; action-level wall time has different boundaries.
        let winner = AttemptTiming {
            client_start_us: a.winner_client_start_us,
            dur_us: a.winner_dur_us,
        };
        let fates: Vec<(AttemptFate, FateBasis)> = a
            .attempts
            .iter()
            .map(|at| {
                attempt_fate(
                    AttemptTiming {
                        client_start_us: at.client_start_us,
                        dur_us: Some(at.dur_us),
                    },
                    winner,
                    f.tr.t0_abs.saturating_add(act.ts),
                    act.dur,
                )
            })
            .collect();
        let delay: i64 = a
            .attempts
            .iter()
            .zip(&fates)
            .filter(|(_, (ft, _))| *ft == AttemptFate::Rerun)
            .map(|(at, _)| at.dur_us)
            .sum();
        for (ft, basis) in &fates {
            match ft {
                AttemptFate::Concurrent => c.concurrent += 1,
                AttemptFate::Rerun => c.rerun += 1,
                AttemptFate::Unknown => c.unknown += 1,
            }
            if *basis == FateBasis::ClientInterval {
                c.exact_fates += 1;
            }
        }
        c.delay_us += delay;
        c.rows.push(RetryRow {
            ev: e,
            tries,
            lost_us: lost,
            fates,
            delay_us: delay,
            attempts: a
                .attempts
                .iter()
                .map(|at| AttemptRow {
                    dur_recorded: at.dur_recorded,
                    start_recorded: at.start_recorded,
                    status: f
                        .x
                        .srcs
                        .get(at.status as usize)
                        .cloned()
                        .unwrap_or_default(),
                    dur_us: at.dur_us,
                    start_us: at.start_us,
                    client_start_us: at.client_start_us,
                    elastic: at.elastic,
                })
                .collect(),
        });
        let ce = cat.entry(a.src).or_default();
        ce.0 += 1;
        ce.1 += extra;
        // one action can fail the same way several times: the actions
        // column counts it once, the attempts column counts each one
        let mut statuses_seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for &k in &a.attempt_status {
            let se = status.entry(k).or_default();
            if statuses_seen.insert(k) {
                se.0 += 1;
            }
            se.1 += 1;
        }
    }
    if c.actions == 0 {
        return None;
    }
    let name = |k: u32| f.x.srcs.get(k as usize).cloned().unwrap_or_default();
    c.by_status = status
        .into_iter()
        .map(|(k, (n, ex))| (name(k), n, ex))
        .collect();
    c.by_status
        .sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    c.by_category = cat
        .into_iter()
        .map(|(k, (n, ex))| (k.map(name).unwrap_or_else(|| "(none)".to_owned()), n, ex))
        .collect();
    c.by_category
        .sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    c.n_categories = c.by_category.len();
    c.by_category.truncate(topk);
    c.rows
        .sort_by_key(|r| std::cmp::Reverse((r.lost_us, r.tries)));
    Some(c)
}

/// Execution-admission band figures over the selected run (rung-2
/// captures; `None` when no action carries a submit span).
pub struct ExecSlotOverview {
    /// actions carrying the band
    pub n: usize,
    pub submit_us: i64,
    pub sem_us: i64,
    pub over_1s: usize,
    pub over_10s: usize,
    pub max_us: i64,
}

pub fn exec_slot_overview(f: &Facts) -> Option<ExecSlotOverview> {
    let mut o = ExecSlotOverview {
        n: 0,
        submit_us: 0,
        sem_us: 0,
        over_1s: 0,
        over_10s: 0,
        max_us: 0,
    };
    for &e in &f.sel {
        let a = f.args(e);
        if !a.admission_recorded {
            continue;
        }
        o.n += 1;
        o.submit_us += a.submit_us;
        o.sem_us += a.exec_sem_wait_us;
        if a.exec_sem_wait_us > 1_000_000 {
            o.over_1s += 1;
        }
        if a.exec_sem_wait_us > 10_000_000 {
            o.over_10s += 1;
        }
        o.max_us = o.max_us.max(a.exec_sem_wait_us);
    }
    if o.n == 0 { None } else { Some(o) }
}

pub fn queue_overview(f: &Facts) -> Option<QueueOverview> {
    let total = f.sel.len();
    let mut vals: Vec<(i64, u32)> = f
        .sel
        .iter()
        .filter_map(|&e| {
            f.args(e)
                .stages
                .and_then(|st| st.queue_us)
                .filter(|&q| q >= 0)
                .map(|q| (q, e))
        })
        .collect();
    if vals.is_empty() {
        return None;
    }
    vals.sort_unstable();
    let covered = vals.len();
    let total_us: i64 = vals.iter().map(|(q, _)| q).sum();
    let (max_us, max_ev) = *vals.last().expect("non-empty");
    Some(QueueOverview {
        waited: vals.iter().filter(|(q, _)| *q > 0).count(),
        covered,
        total,
        total_us,
        mean_us: total_us / covered as i64,
        p90_us: vals[percentile_index(covered, 90)].0,
        max_us,
        max_ev,
    })
}

// ===================================================================
// §8  stall audit + exec-peer percentiles
// ===================================================================

/// Similar-duration actions with little recorded stage time. This is a
/// diagnostic grouping, not evidence of synchronization or a shared cause.
#[derive(Clone, Debug)]
pub struct StallCohort {
    pub n: usize,
    pub dur_typ: i64,
    pub t0: i64,
    pub t1: i64,
    pub staged_frac: f64,
    pub lost_us: i64,
    /// longest member (event index), for click-through
    pub sample: u32,
}

#[derive(Clone, Debug)]
pub struct StallReport {
    pub nacts: usize,
    pub with_stages: usize,
    pub total_dur: i64,
    pub staged: i64,
    pub cohorts: Vec<StallCohort>,
}

/// Instrumentation coverage plus stall-cohort detection (dur >= 30s,
/// hard stages < 5% of span, grouped by 5s duration bucket, cohorts of
/// >= 25, top 40 by lost time).
pub fn stall_audit(f: &Facts) -> StallReport {
    let mut total_dur = 0i64;
    let mut staged = 0i64;
    let mut with_stages = 0usize;
    let mut groups: FxHashMap<i64, Vec<(u32, i64, i64)>> = FxHashMap::default();
    for &e in &f.sel {
        let dur = f.dur(e);
        total_dur += dur;
        let st = f.args(e).stages;
        let ssum = st.map(|s| s.hard_sum()).unwrap_or(0) + f.args(e).exec_sem_wait_us;
        if st.is_some() || f.args(e).admission_recorded {
            with_stages += 1;
        }
        staged += ssum.min(dur);
        if dur >= 30_000_000 && ssum < dur / 20 {
            groups
                .entry(dur / 5_000_000)
                .or_default()
                .push((e, dur, ssum));
        }
    }
    let mut cohorts: Vec<StallCohort> = groups
        .into_values()
        .filter(|g| g.len() >= 25)
        .map(|g| {
            let n = g.len();
            let mut durs: Vec<i64> = g.iter().map(|x| x.1).collect();
            durs.sort_unstable();
            let t0 = g.iter().map(|&(e, _, _)| f.ts(e)).min().unwrap_or(0);
            let t1 = g.iter().map(|&(e, d, _)| f.ts(e) + d).max().unwrap_or(0);
            let s: i64 = g.iter().map(|x| x.2).sum();
            let d: i64 = g.iter().map(|x| x.1).sum();
            let sample = g.iter().max_by_key(|x| x.1).map(|x| x.0).unwrap_or(0);
            StallCohort {
                n,
                dur_typ: durs[n / 2],
                t0,
                t1,
                staged_frac: s as f64 / d.max(1) as f64,
                lost_us: d - s,
                sample,
            }
        })
        .collect();
    cohorts.sort_unstable_by_key(|c| std::cmp::Reverse(c.lost_us));
    cohorts.truncate(40);
    StallReport {
        nacts: f.sel.len(),
        with_stages,
        total_dur,
        staged,
        cohorts,
    }
}

/// Per-category duration distributions over the selected run — ONE
/// structure backing both the peers table and the detail pane's
/// "among peers" line.
pub struct PeerStats {
    /// category (src interner id) -> sorted durations
    by_cat: FxHashMap<(u32, bool), Vec<i64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeerRank {
    pub pct: usize,
    pub n: usize,
    pub median_us: i64,
    pub cat: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeerRow {
    pub cat: String,
    pub n: usize,
    pub p50_us: i64,
    pub p90_us: i64,
    pub max_us: i64,
    pub total_us: i64,
}

pub const PEER_MIN_N: usize = 5;

/// Nearest-rank quantile of a sorted nonempty population (1..=100).
pub fn percentile_index(n: usize, percent: usize) -> usize {
    assert!(n > 0 && (1..=100).contains(&percent));
    (n * percent).div_ceil(100) - 1
}

pub fn peer_category(src: &str, cache_hit: bool) -> String {
    if cache_hit {
        format!("{src} (cache hit)")
    } else {
        src.to_owned()
    }
}

pub fn peer_stats(f: &Facts) -> PeerStats {
    let mut by_cat: FxHashMap<(u32, bool), Vec<i64>> = FxHashMap::default();
    for &e in &f.sel {
        if let Some(c) = f.args(e).src {
            by_cat
                .entry((c, f.args(e).cache.is_some()))
                .or_default()
                .push(f.dur(e));
        }
    }
    for v in by_cat.values_mut() {
        v.sort_unstable();
    }
    PeerStats { by_cat }
}

/// Where `dur` sits in a SORTED duration list: (percentile, n, median).
/// `None` under [`PEER_MIN_N`] entries (a percentile over 3 actions is
/// noise). The one rank implementation — the peers table, the detail
/// popup and any future surface all call this.
pub fn rank_among(durs: &[i64], dur: i64) -> Option<(usize, usize, i64)> {
    if durs.len() < PEER_MIN_N {
        return None;
    }
    let pos = durs.partition_point(|&x| x < dur);
    Some((
        100 * pos / durs.len(),
        durs.len(),
        durs[percentile_index(durs.len(), 50)],
    ))
}

impl PeerStats {
    /// Where event `e` sits among same-category actions.
    pub fn rank(&self, f: &Facts, e: u32) -> Option<PeerRank> {
        let c = f.args(e).src?;
        let cache_hit = f.args(e).cache.is_some();
        let durs = self.by_cat.get(&(c, cache_hit))?;
        let (pct, n, median_us) = rank_among(durs, f.dur(e))?;
        Some(PeerRank {
            pct,
            n,
            median_us,
            cat: peer_category(&f.x.srcs[c as usize], cache_hit),
        })
    }

    /// The sorted per-category distributions keyed by category name —
    /// what the GUI keeps once `Facts` is gone.
    pub fn into_named(self, f: &Facts) -> FxHashMap<String, Vec<i64>> {
        self.by_cat
            .into_iter()
            .map(|((c, cache_hit), durs)| (peer_category(&f.x.srcs[c as usize], cache_hit), durs))
            .collect()
    }

    /// The percentile table, biggest total first.
    pub fn table(&self, f: &Facts) -> Vec<PeerRow> {
        let mut rows: Vec<PeerRow> = self
            .by_cat
            .iter()
            .map(|(&(c, cache_hit), durs)| {
                let n = durs.len();
                PeerRow {
                    cat: peer_category(&f.x.srcs[c as usize], cache_hit),
                    n,
                    p50_us: durs[percentile_index(n, 50)],
                    p90_us: durs[percentile_index(n, 90)],
                    max_us: durs[n - 1],
                    total_us: durs.iter().sum(),
                }
            })
            .collect();
        rows.sort_by(|a, b| b.total_us.cmp(&a.total_us).then(a.cat.cmp(&b.cat)));
        rows
    }
}

// ===================================================================
// tests
// ===================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn stage_presence_tracks_each_valid_counter_without_growing_the_layout() {
        use serde_json::json;

        assert_eq!(
            std::mem::size_of::<Stages>(),
            std::mem::size_of::<([i64; 17], bool)>()
        );
        for (key, bit) in [
            ("cache_query_us", Stages::CACHE_QUERY),
            ("cache_hit_us", Stages::CACHE_HIT),
            ("prepare_us", Stages::PREPARE),
            ("queue_us", Stages::QUEUE),
            ("materialize_us", Stages::MATERIALIZE),
            ("hashing_us", Stages::HASHING),
            ("cas_artifact_us", Stages::CAS_ARTIFACT),
            ("local_finalize_outputs_us", Stages::FINALIZE_OUTPUTS),
            ("local_cache_check_us", Stages::LOCAL_CACHE),
            ("execute_us", Stages::EXECUTE),
            ("upload_us", Stages::UPLOAD),
            ("input_upload_us", Stages::INPUT_UPLOAD),
            ("result_wait_us", Stages::RESULT_WAIT),
            ("download_us", Stages::DOWNLOAD),
            ("rtt_us", Stages::RTT),
            ("daemon_us", Stages::DAEMON),
        ] {
            for value in [json!(0), json!(7), json!(-1), json!("bad"), json!(null)] {
                let row = json!({"ph":"X","cat":"recipe","name":"job","ts":0,"dur":100,
                    "args":{"stages":{key:value}}});
                let (_, extras) = census_of(&format!("[\n{row}\n]\n"), "stage-presence");
                let valid = value.as_i64().is_some_and(|v| v >= 0);
                let stages = extras.args[0].stages;
                assert_eq!(stages.is_some(), valid, "{key}={value}");
                if let Some(stages) = stages {
                    assert_eq!(stages.measured_mask, bit, "{key}={value}");
                    assert!(stages.has_any(bit));
                    assert!(!stages.has_any(!bit));
                }
            }
        }
        for cache in [json!({}), json!({"bytes":1})] {
            let row = json!({"ph":"X","cat":"recipe","name":"job","ts":0,"dur":100,
                "args":{"cache":cache,"stages":{"execute_us":42,"cache_query_us":0}}});
            let (_, extras) = census_of(&format!("[\n{row}\n]\n"), "historical-stage-presence");
            let stages = extras.args[0].stages.unwrap();
            assert!(stages.has_any(Stages::CACHE_QUERY));
            assert!(!stages.has_any(Stages::EXECUTE));
        }
        assert!(!Stages::default().has_any(u16::MAX));
        assert!(
            Stages {
                execute_us: 7,
                ..Stages::default()
            }
            .has_any(Stages::EXECUTE)
        );
        assert!(
            Stages {
                queue_us: Some(0),
                ..Stages::default()
            }
            .has_any(Stages::QUEUE)
        );
    }

    #[test]
    fn optional_repairs_count_each_rejected_field_once() {
        use serde_json::json;

        for (args, expected) in [
            (
                json!({"re":{"iu_sem_wait_us":"bad","iu_cas_call_us":12}}),
                1,
            ),
            (
                json!({"re":{"iu_sem_wait_us":-1,"rs_exec_sem_wait_us":"bad","iu_cas_call_us":12}}),
                2,
            ),
            (
                json!({"bytes_in":"bad","re":{
                    "invalid_counter_fields":["iu_sem_wait_us"],
                    "rs_exec_sem_wait_us":"bad","iu_cas_call_us":12
                }}),
                3,
            ),
            (
                json!({"maxrss":"bad","re":{
                    "invalid_counter_fields":["iu_sem_wait_us"],"iu_cas_call_us":12
                }}),
                2,
            ),
            (
                json!({"re":{"invalid_counter_fields":null,"iu_sem_wait_us":"bad","iu_cas_call_us":12}}),
                2,
            ),
        ] {
            let row = json!({"ph":"X","cat":"recipe","name":"job","ts":0,"dur":100,"args":args});
            let (_, extras) = census_of(&format!("[\n{row}\n]\n"), "repair-counts");
            assert!(extras.source_error.is_none(), "{:?}", extras.source_error);
            assert_eq!(extras.unmatched, 0);
            assert_eq!(extras.rejected_fields, expected, "{args}");
            let upload = extras.args[0]
                .iu
                .expect("valid CAS duration survives repair");
            assert_eq!(upload.phase(UploadSub::PHASE_CAS_CALL), Some(12));
            assert_eq!(upload.phase(UploadSub::PHASE_PERMIT_WAIT), None);
        }
    }

    use super::*;
    use crate::parse;
    use crate::testdata as fixture;

    #[test]
    fn optional_field_errors_preserve_other_measurements_and_zero() {
        let (tr, x) = census_of(
            r#"[
{"ph":"X","cat":"recipe","name":"current","ts":100,"dur":0,"args":{"src":"rustc","maxrss":17,"stages":{"queue_us":0,"execute_us":-8},"re":{"submit_us":0},"attempts":[{"status":"Cancelled","dur_us":"bad","client_start":120}],"bytes_in":"bad"}}
]
"#,
            "optional-local-errors",
        );
        assert_eq!(tr.acts[0].dur, 0);
        assert_eq!(x.unmatched, 0);
        assert!(x.rejected_fields >= 3);
        let a = &x.args[0];
        assert!(a.decoded);
        assert_eq!(a.maxrss, Some(17));
        assert!(a.admission_recorded);
        assert_eq!(a.stages.unwrap().queue_us, Some(0));
        assert_eq!(a.attempts.len(), 1);
        assert!(!a.attempts[0].dur_recorded);
        assert_eq!(a.attempts[0].client_start_us, Some(120));
        let f = Facts::for_run(&tr, &x, None).unwrap();
        assert_eq!(queue_overview(&f).unwrap().waited, 0);
        assert_eq!(exec_slot_overview(&f).unwrap().n, 1);
    }

    #[test]
    fn historical_peak_rss_is_not_current_memory() {
        let (tr, x) = census_of(
            r#"[
{"ph":"X","cat":"recipe","name":"cached","ts":100,"dur":10,"args":{"cache":{"hit":true},"maxrss":80000000000}}
{"ph":"X","cat":"recipe","name":"reused","ts":110,"dur":10,"args":{"execution_metadata":"reused","maxrss":80000000000}}
{"ph":"X","cat":"recipe","name":"current","ts":120,"dur":10,"args":{"maxrss":100}}
]
"#,
            "current-rss",
        );
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let rows = top_memory(&f, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].maxrss, 100);
    }

    #[test]
    fn runless_actions_and_duplicate_metadata_have_one_policy() {
        let (tr, x) = census_of(
            r#"[
{"ph":"M","cat":"run","name":"run_start","ts":100,"args":{"run":"r","user":"first"}}
{"ph":"M","cat":"run","name":"run_start","ts":101,"args":{"run":"r","user":"second"}}
{"ph":"X","cat":"recipe","name":"a","ts":102,"dur":10}
{"ph":"M","cat":"run","name":"run_end","ts":120,"args":{"run":"r","exit":0,"remade":1}}
{"ph":"M","cat":"run","name":"run_end","ts":130,"args":{"run":"r","exit":7,"remade":8}}
]
"#,
            "first-run-record",
        );
        let f = Facts::for_run(&tr, &x, Some("r")).unwrap();
        assert_eq!(f.sel.len(), 1);
        assert_eq!(tr.selected_summary(Some("r")).actions, 1);
        assert_eq!(run_totals(&x.meta, Some("r")).unwrap().remade, 1);
        assert_eq!(tr.runs[0].duplicate_records, 2);
        let rows = overview_rows(&f, &overview_facts(&f), None, &fmt_when_utc);
        assert_eq!(
            rows.iter().find(|r| r.key == "Result").unwrap().value,
            "exit 0 — success"
        );
        assert_eq!(
            rows.iter().find(|r| r.key == "Ran by").unwrap().value,
            "first"
        );
    }

    fn smoke() -> (Trace, Extras) {
        let tr = parse::parse(&fixture("smoke-trace.json")).unwrap();
        let x = load_extras(&tr);
        (tr, x)
    }

    #[test]
    fn formatting_matches_the_certified_shapes() {
        assert_eq!(fmt_dur(1_559_700_000), "1559.70s");
        assert_eq!(fmt_dur(1500), "1.5ms");
        assert_eq!(fmt_dur(900), "900us");
        // i64::MIN has no negation; the display helper must not panic
        assert!(fmt_dur(i64::MIN).starts_with('-'));
        assert_eq!(fmt_bytes(2048), "2.0 KiB");
        assert_eq!(fmt_bytes(500), "500 B");
        assert_eq!(fmt_bytes(3 << 30), "3.0 GiB");
        assert_eq!(fmt_n(1_601_203.9), "1,601,203");
        assert_eq!(fmt_secs(5_314_130_400_000.0), "5,314,130.40s (1,476.1h)");
        assert_eq!(round3(1.0005), 1.0); // half-to-even like Python
        assert_eq!(round3(1.0015), 1.002);
        assert_eq!(py_float(3.0), "3.0");
        assert_eq!(fmt_when_utc(0), "Thu 1970-01-01 00:00:00 UTC");
        assert_eq!(
            shlex_join(&["buck2".into(), "build".into(), "a b".into()]),
            "buck2 build 'a b'"
        );
    }

    #[test]
    fn classify_type_buck_categories() {
        assert_eq!(classify_type("rustc", "y (rustc emit)"), "object");
        assert_eq!(classify_type("cxx_link", "z (cxx_link)"), "binary");
        assert_eq!(classify_type("cxx_link", "libz.so (cxx_link)"), "library");
        assert_eq!(classify_type("cxx_link", "LIBZ.SO (cxx_link)"), "library");
        assert_eq!(classify_type("genrule", "w (genrule)"), "generated");
        // the proto's link-substring bug is fixed: exact table wins
        assert_eq!(
            classify_type("symlinked_dir", "out (symlinked_dir)"),
            "script"
        );
        assert_eq!(classify_type("archive", "libx.a (archive)"), "archive");
        assert_eq!(classify_type("", "mystery"), "other");
    }

    #[test]
    fn only_kinds_that_ran_a_command_can_be_simulated_as_cache_hits() {
        let row = |name: &str, kind: &str| {
            format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"{name}\",\"ts\":0,\"dur\":10,\
                 \"args\":{{\"run\":\"r\",\"kind\":\"{kind}\"}}}}"
            )
        };
        let body = format!(
            "[\n{},\n{}\n]\n",
            row("shared (link)", "remote_deduplicated"),
            row("served (link)", "remote_served_from_cache"),
        );
        let (tr, x) = census_of(&body, "cache-eligible-kinds");
        let ineligible = |n: &str| {
            let i = tr.names.iter().position(|s| s == n).unwrap();
            let ai = tr.acts.iter().position(|a| a.name as usize == i).unwrap();
            x.args[ai].cache_ineligible
        };
        assert!(
            !ineligible("shared (link)"),
            "a deduplicated execution ran, shared with another build"
        );
        assert!(
            ineligible("served (link)"),
            "RE served its own cache; nothing ran"
        );
    }

    #[test]
    fn extras_second_pass_matches_by_offset() {
        let (tr, x) = smoke();
        assert_eq!(x.unmatched, 0);
        let by_name = |n: &str| {
            let i = tr.names.iter().position(|s| s == n).unwrap();
            let ai = tr.acts.iter().position(|a| a.name as usize == i).unwrap();
            (&x.args[ai], x.ty[ai])
        };
        let (y, yty) = by_name("y (rustc emit)");
        assert_eq!(x.srcs[y.src.unwrap() as usize], "rustc");
        assert_eq!(yty, "object");
        assert_eq!(y.stages.unwrap().execute_us, 400);
        assert_eq!(y.stages.unwrap().queue_us, Some(50));
        assert_eq!(y.maxrss, Some(1_000_000));
        assert_eq!(y.bytes_in, 5_000_000);
        let (z, zty) = by_name("z (cxx_link)");
        assert_eq!(zty, "binary");
        let c = z.cache.unwrap();
        assert_eq!(c.bytes, Some(2048));
        assert_eq!(c.lookup_us, Some(100));
        let (w, wty) = by_name("w (genrule)");
        assert_eq!(wty, "generated");
        assert!(w.stages.is_none(), "absent stages stay None for coverage");
        // meta records: run_start + run_end
        assert_eq!(x.meta.len(), 2);
        assert_eq!(x.meta[0].name, "run_start");
        assert_eq!(x.meta[1].i("exit"), Some(1));
    }

    #[test]
    fn lazy_extras_never_switches_to_a_replacement_file() {
        let p = crate::temp_path("lazy-generation-trace.json");
        let replacement = crate::temp_path("lazy-generation-replacement.json");
        let row = |target: &str| {
            format!(
                "[\n{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":0,\"dur\":10,\"args\":{{\"src\":\"{target}\",\"target\":\"{target}\",\"cfg\":\"cfg\"}}}}\n]\n"
            )
        };
        std::fs::write(&p, row("old")).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        std::fs::write(&replacement, row("new")).unwrap();
        std::fs::rename(&replacement, &p).unwrap();
        let x = load_extras(&tr);
        if x.source_error.is_some() {
            assert_eq!(x.unmatched, tr.acts.len());
            assert!(x.srcs.is_empty());
        } else {
            assert_eq!(x.srcs, ["old"], "retained handle, never reopened path");
        }
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn lazy_extras_refuses_in_place_mutation() {
        let p = crate::temp_path("lazy-mutation-trace.json");
        let body = "[\n{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":0,\"dur\":10,\"args\":{\"src\":\"old\",\"target\":\"old\"}}\n]\n";
        std::fs::write(&p, body).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        std::fs::write(&p, format!("{body} ")).unwrap();
        let x = load_extras(&tr);
        assert!(x.source_error.is_some());
        assert_eq!(x.unmatched, tr.acts.len());
        assert!(x.args.iter().all(|a| a.src.is_none()));
        assert!(x.srcs.is_empty() && x.meta.is_empty());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn extras_survive_an_invalid_utf8_line_between_records() {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-extras-badutf8-{}.json",
            std::process::id()
        ));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"[\n");
        bytes.extend_from_slice(
            br#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"src":"rustc"}},"#,
        );
        bytes.push(b'\n');
        bytes.extend_from_slice(&[0xff, 0xfe, b'x', b'\n']);
        bytes.extend_from_slice(
            br#"{"ph":"X","cat":"recipe","name":"b","ts":20,"dur":10,"args":{"src":"genrule"}},"#,
        );
        bytes.push(b'\n');
        std::fs::write(&p, &bytes).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        std::fs::remove_file(&p).ok();
        assert_eq!(tr.acts.len(), 2);
        assert_eq!(
            x.unmatched, 0,
            "one bad line loses only itself; records after it still match by offset"
        );
    }

    /// `bytes_uploaded` predates the sub-stage breakdown, so a row that
    /// carries only it is still uninstrumented: the band is replaced
    /// whole, priced with the recorded bytes.
    #[test]
    fn upload_bytes_alone_do_not_make_a_capture_instrumented() {
        let p = crate::temp_path("upload-bytes-only-trace.json");
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"u (link)","ts":0,"dur":4000000,"args":{"src":"link","run":"r","bytes_in":104857600,"stages":{"input_upload_us":3000000,"execute_us":1000000},"re":{"kind":"remote","iu_bytes_uploaded":5242880}}},"#,
                "\n"
            ),
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        std::fs::remove_file(&p).ok();
        let a = &x.args[0];
        assert!(a.iu.is_none(), "no sub-stage counters, so not instrumented");
        assert_eq!(a.iu_bytes_uploaded, Some(5_242_880));
        let bytes = crate::transfer::TransferBytes {
            bytes_in: a.bytes_in,
            bytes_out: a.bytes_out,
            iu_cas_call_us: a.iu.map(|s| s.cas_call_us),
            iu_bytes: a.iu_bytes_uploaded,
            iu_invalid: false,
        };
        let st = a.stages.expect("staged");
        // 5 MiB at 100 MiB/s = 50 ms replaces the whole 3 s band
        assert_eq!(
            crate::transfer::transfer_delta(&st, bytes, (100i64 << 20) as f64),
            Some(50_000 - 3_000_000)
        );
    }

    #[test]
    fn facts_selects_the_single_run_and_orders_events() {
        let (tr, x) = smoke();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        assert_eq!(f.run.as_deref(), Some("aaaa-bbbb"));
        assert_eq!(f.sel.len(), 3);
        // ts order: y, z, w
        assert_eq!(f.name(f.sel[0]), "y (rustc emit)");
        assert_eq!(f.name(f.sel[2]), "w (genrule)");
        assert!(Facts::for_run(&tr, &x, Some("nope")).is_err());
    }

    #[test]
    fn overview_does_not_invent_an_exit_code() {
        let smoke = std::fs::read_to_string(fixture("smoke-trace.json")).unwrap();
        // The run ended as failed with no exit code recorded.
        let edited: String = smoke
            .split_inclusive('\n')
            .map(|line| {
                if line.contains(r#""name":"run_end""#) {
                    line.replace(r#""exit":1,"#, r#""exit":null,"outcome":"failed","#)
                } else {
                    line.to_owned()
                }
            })
            .collect();
        assert_ne!(edited, smoke);
        let path = crate::temp_path("smoke-failed-no-exit.json");
        std::fs::write(&path, edited).unwrap();
        let tr = parse::parse(&path.to_string_lossy()).unwrap();
        let x = load_extras(&tr);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let rows = overview_rows(&f, &overview_facts(&f), None, &fmt_when_utc);
        let result = rows.iter().find(|r| r.key == "Result").unwrap();
        assert!(
            result.value.contains("FAILED — exit code not recorded"),
            "{}",
            result.value
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn overview_without_a_graph_does_not_invent_a_dependency_path() {
        let (tr, x) = smoke();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let o = overview_facts(&f);
        assert_eq!(o.span, 900);
        assert_eq!(o.busy, 1600);
        assert_eq!(o.nwork, 3);
        assert_eq!(o.nfail, 1);
        let rows = overview_rows(&f, &o, None, &fmt_when_utc);
        let get = |k: &str| rows.iter().find(|r| r.key == k).map(|r| r.value.clone());
        assert!(
            get("Result")
                .unwrap()
                .contains("exit 1 — FAILED; 1 failed action(s)")
        );
        assert!(get("Job limit").unwrap().contains("default (parallel"));
        assert!(get("Changed files").unwrap().contains("fbcode/x/y.rs"));
        assert!(get("Action cache").unwrap().contains("1 hits / 2 misses"));
        assert!(
            get("Action model CP")
                .unwrap()
                .contains("needs the dependency graph")
        );
        assert!(get("Serial chain").is_none());
        assert!(get("Parallelism bound").is_none());
        assert!(!get("Total time").unwrap().contains("startup"));
        // The fixture names no machine, so Machine is the spec alone and
        // "Ran by" is a person — never the tool tag glued on as if it
        // were a host, which is what `user@buck2` used to render.
        assert_eq!(get("Machine").unwrap(), "64 cores, 251 GiB RAM");
        assert!(
            !get("Ran by").unwrap().contains("buck2"),
            "{}",
            get("Ran by").unwrap()
        );
    }

    #[test]
    fn breakdown_rule_sites_top_memory() {
        let (tr, x) = smoke();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let b = breakdown(&f);
        let row = |ty: &str| b.iter().find(|r| r.ty == ty).unwrap().clone();
        assert_eq!(row("binary").total_us, 800);
        assert_eq!(row("binary").share_pct, 50);
        assert_eq!(row("object").share_pct, 31); // floor(500*100/1600)
        assert_eq!(row("generated").count, 1);
        assert_eq!(b[0].ty, "binary"); // longest first

        let rs = rule_sites(&f, 40);
        assert_eq!(rs[0].rule, "cxx_link");
        assert_eq!(rs[0].total_us, 800);

        let tm = top_memory(&f, 10);
        assert_eq!(tm.len(), 1);
        assert_eq!(tm[0].maxrss, 1_000_000);
        assert_eq!(tm[0].exec_us, Some(400));
    }

    #[test]
    fn top_memory_leaves_unrecorded_execution_time_absent() {
        let (tr, x) = census_of(
            r#"[
{"ph":"X","cat":"recipe","name":"measured","ts":100,"dur":900,"args":{"src":"rustc","maxrss":2000,"exec_us":700}}
{"ph":"X","cat":"recipe","name":"unmeasured","ts":100,"dur":800,"args":{"src":"rustc","maxrss":1000}}
]
"#,
            "top-memory-exec-absent",
        );
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let tm = top_memory(&f, 10);
        assert_eq!(tm.len(), 2);
        assert_eq!(tm[0].exec_us, Some(700));
        assert_eq!(
            tm[1].exec_us, None,
            "an execution time the capture never recorded is unknown, not zero"
        );
        assert_eq!(tm[1].cells()[4], "\u{2014}");
    }

    #[test]
    fn a_share_that_rounds_away_is_not_reported_as_zero() {
        assert_eq!(share_cell(0, 0), "0%");
        assert_eq!(share_cell(0, 1), "<1%");
        assert_eq!(share_cell(31, 500), "31%");
    }

    #[test]
    fn queue_coverage_semantics() {
        let (tr, x) = smoke();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let q = queue_overview(&f).unwrap();
        assert_eq!((q.covered, q.total), (1, 3)); // absent stages are not zeros
        assert_eq!(q.mean_us, 50);
        assert_eq!(q.max_us, 50);
    }

    #[test]
    fn stall_audit_coverage_and_thresholds() {
        let (tr, x) = smoke();
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let r = stall_audit(&f);
        assert_eq!(r.nacts, 3);
        assert_eq!(r.with_stages, 2);
        assert_eq!(r.total_dur, 1600);
        assert_eq!(r.staged, 480 + 100); // y hard sum + z cache_query
        assert!(r.cohorts.is_empty(), "nothing is 30s long here");
    }

    #[test]
    fn stall_cohort_detection_fires_on_synchronized_stalls() {
        // 30 actions of ~40s with no stages = one cohort
        let mut body = String::from("[\n");
        for k in 0..30 {
            body.push_str(&format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"stall{k} (x)\",\"ts\":{},\
                 \"dur\":40000000,\"args\":{{\"src\":\"x\",\"run\":\"r\"}}}},\n",
                1000 + k
            ));
        }
        let mut p = std::env::temp_dir();
        p.push(format!("retrace-stall-{}.json", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let r = stall_audit(&f);
        assert_eq!(r.cohorts.len(), 1);
        assert_eq!(r.cohorts[0].n, 30);
        assert_eq!(r.cohorts[0].dur_typ, 40_000_000);
        assert_eq!(r.cohorts[0].staged_frac, 0.0);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn malformed_attempt_records_cost_only_themselves() {
        // A converter regression wrote `start` as the raw protobuf
        // [seconds, nanos] pair. Rejecting the whole row for that threw
        // away every extra on every retried action in the capture, which
        // is exactly the outcome this parse must never produce again.
        let mut body = String::from("[\n");
        body.push_str(
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"pair (rustc emit)\",\"ts\":100,\
             \"dur\":900,\"args\":{\"src\":\"rustc\",\"run\":\"r\",\"maxrss\":4096,\"tries\":3,\
             \"attempts\":[\
             {\"status\":\"Cancelled\",\"dur_us\":100,\"start\":[1788202933,40000000],\
             \"elastic\":true},\
             {\"status\":\"Cancelled\",\"dur_us\":200,\"start\":[1788202934,0]}]}},\n",
        );
        body.push_str(
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"micros (rustc emit)\",\"ts\":200,\
             \"dur\":800,\"args\":{\"src\":\"rustc\",\"run\":\"r\",\"tries\":2,\
             \"attempts\":[{\"status\":\"Timeout\",\"dur_us\":50,\"start\":1788202935000000}]}},\n",
        );
        body.push_str(
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"junk (rustc emit)\",\"ts\":300,\
             \"dur\":700,\"args\":{\"src\":\"rustc\",\"run\":\"r\",\"tries\":2,\
             \"attempts\":[\"nonsense\",{\"status\":\"Cancelled\",\"dur_us\":7}]}},\n",
        );
        let mut p = std::env::temp_dir();
        p.push(format!("retrace-attempts-{}.json", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        std::fs::remove_file(&p).ok();

        let arg = |n: &str| {
            let i = tr.names.iter().position(|s| s == n).unwrap();
            let ai = tr.acts.iter().position(|a| a.name as usize == i).unwrap();
            &x.args[ai]
        };

        let pair = arg("pair (rustc emit)");
        assert_eq!(
            pair.maxrss,
            Some(4096),
            "the rest of the row survives a pair-shaped timestamp"
        );
        assert_eq!(pair.tries, Some(3));
        assert_eq!(pair.attempts.len(), 2);
        // 1788202933s + 40ms, in microseconds
        assert_eq!(pair.attempts[0].start_us, 1_788_202_933_040_000);
        assert_eq!(pair.attempts[1].start_us, 1_788_202_934_000_000);
        assert!(pair.attempts[0].elastic, "elastic rides along");
        assert_eq!(pair.attempts[0].client_start_us, None);

        let micros = arg("micros (rustc emit)");
        assert_eq!(micros.attempts[0].start_us, 1_788_202_935_000_000);

        let junk = arg("junk (rustc emit)");
        assert_eq!(junk.attempts.len(), 1, "the bare string is dropped alone");
        assert_eq!(junk.attempts[0].dur_us, 7);
        assert_eq!(
            junk.attempts[0].start_us, 0,
            "a missing start reads as unknown"
        );

        let f = Facts::for_run(&tr, &x, None).unwrap();
        let c = retry_census(&f, 10).unwrap();
        assert_eq!(c.actions, 3);
        assert_eq!(c.extra_attempts, 4); // 2 + 1 + 1
        assert_eq!(c.rows.len(), 3);
    }

    #[test]
    fn peer_percentiles_share_one_implementation() {
        // 6 rustc actions with spread durations + 1 lonely genrule
        let mut body = String::from("[\n");
        for (k, d) in [10, 20, 30, 40, 50, 60].iter().enumerate() {
            body.push_str(&format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"r{k} (rustc emit)\",\"ts\":{},\
                 \"dur\":{d},\"args\":{{\"src\":\"rustc\",\"run\":\"r\"}}}},\n",
                k * 100
            ));
        }
        body.push_str(
            "{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"g (genrule)\",\"ts\":900,\
             \"dur\":5,\"args\":{\"src\":\"genrule\",\"run\":\"r\"}},\n",
        );
        let mut p = std::env::temp_dir();
        p.push(format!("retrace-peers-{}.json", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let ps = peer_stats(&f);
        // the 50-dur action: 4 of 6 durs below it
        let e50 = f.sel.iter().copied().find(|&e| f.dur(e) == 50).unwrap();
        let r = ps.rank(&f, e50).unwrap();
        assert_eq!((r.pct, r.n, r.median_us), (66, 6, 30));
        assert_eq!(r.cat, "rustc");
        // lonely category: below the n>=5 gate
        let eg = f.sel.iter().copied().find(|&e| f.dur(e) == 5).unwrap();
        assert!(ps.rank(&f, eg).is_none());
        // the table is the same distribution
        let t = ps.table(&f);
        assert_eq!(t[0].cat, "rustc");
        assert_eq!(t[0].n, 6);
        assert_eq!(t[0].p50_us, 30);
        assert_eq!(t[0].p90_us, 60);
        assert_eq!(t[0].total_us, 210);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn attempt_fate_reads_the_overlap_not_the_status() {
        use AttemptFate::*;
        let t = |client_start_us: Option<i64>, dur_us: Option<i64>| AttemptTiming {
            client_start_us,
            dur_us,
        };
        // xpv shape: 312 s action, winner's wall 311.6 s, the local
        // loser ran 200.5 s: the two cannot both fit, so they overlapped
        assert_eq!(
            attempt_fate(
                t(None, Some(200_541_110)),
                t(None, Some(311_605_344)),
                0,
                312_368_240
            ),
            (Concurrent, FateBasis::DurationOverflow)
        );
        // a short loser and a short winner in a long action: nothing
        // decidable from durations
        assert_eq!(
            attempt_fate(
                t(None, Some(420_000)),
                t(None, Some(1_351_055)),
                0,
                10_340_000
            ),
            (Unknown, FateBasis::None)
        );
        // 30 s loser + 59 s winner in a 90 s action: could be serial or
        // overlapping; without starts it is unknown, never a rerun
        assert_eq!(
            attempt_fate(
                t(None, Some(30_000_000)),
                t(None, Some(59_000_000)),
                0,
                90_000_000
            ),
            (Unknown, FateBasis::None)
        );
        // no winner duration: unknown, never a zero-length winner
        assert_eq!(
            attempt_fate(t(None, Some(30_000_000)), t(None, None), 0, 90_000_000),
            (Unknown, FateBasis::None)
        );
        // a loser with no recorded duration
        assert_eq!(
            attempt_fate(t(Some(100), None), t(Some(200), Some(10)), 100, 90),
            (Unknown, FateBasis::None)
        );
        // a queue-cancelled record: zero-length span stamped at
        // cancellation, not a real start
        assert_eq!(
            attempt_fate(t(Some(100), Some(0)), t(Some(50), Some(10)), 50, 90),
            (Unknown, FateBasis::None)
        );
        // client starts on both sides: exact
        let a0 = 1_000_000_000;
        assert_eq!(
            attempt_fate(
                t(Some(a0), Some(9_000_000)),
                t(Some(a0 + 10_000_000), Some(10_000_000)),
                a0,
                20_000_000
            ),
            (Rerun, FateBasis::ClientInterval)
        );
        assert_eq!(
            attempt_fate(
                t(Some(a0), Some(9_000_000)),
                t(Some(a0 + 5_000_000), Some(10_000_000)),
                a0,
                20_000_000
            ),
            (Concurrent, FateBasis::ClientInterval)
        );
        // touching: the loser ended exactly as the winner started
        assert_eq!(
            attempt_fate(
                t(Some(a0), Some(9_000_000)),
                t(Some(a0 + 9_000_000), Some(10_000_000)),
                a0,
                20_000_000
            ),
            (Rerun, FateBasis::ClientInterval)
        );
    }

    #[test]
    fn invalid_or_synthetic_attempt_intervals_are_unknown() {
        let t = |start, dur| AttemptTiming {
            client_start_us: start,
            dur_us: dur,
        };
        let unknown = (AttemptFate::Unknown, FateBasis::None);
        for winner in [
            t(Some(150), None),
            t(Some(150), Some(0)),
            t(Some(150), Some(-1)),
        ] {
            assert_eq!(
                attempt_fate(t(Some(100), Some(40)), winner, 100, 100),
                unknown
            );
        }
        for (loser, winner) in [
            (t(Some(90), Some(40)), t(Some(150), Some(40))), // before the action
            (t(Some(100), Some(40)), t(Some(180), Some(40))), // past action end
            (t(None, Some(110)), t(None, Some(40))),         // oversized duration alone
            (t(Some(180), Some(10)), t(Some(100), Some(40))), // after winner ended
            (t(Some(i64::MAX - 1), Some(10)), t(Some(100), Some(40))),
        ] {
            assert_eq!(
                attempt_fate(loser, winner, 100, 100),
                unknown,
                "{loser:?} {winner:?}"
            );
        }
    }

    fn census_of(body: &str, tag: &str) -> (Trace, Extras) {
        let p = crate::temp_path(&format!("{tag}-trace.json"));
        std::fs::write(&p, body).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        std::fs::remove_file(&p).ok();
        (tr, x)
    }

    #[test]
    fn cancelled_census_names_the_losing_attempt_and_slow_uploads_beside_it() {
        let row = |name: &str, cancelled: &str, gather_us: i64| {
            format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"{name}\",\"ts\":100,\"dur\":90000000,\
                 \"args\":{{\"run\":\"r\",\"src\":\"link\",\"bytes_in\":6000000000,\
                 \"stages\":{{\"input_upload_us\":71000000}},\
                 \"re\":{{\"iu_materialize_us\":{gather_us},\"iu_digests_queried\":40,\
                 \"cancelled_stages\":[{cancelled}]}}}}}}"
            )
        };
        let local = "{\"name\":\"Local.Queued\",\"ts\":101,\"dur\":1},\
                     {\"name\":\"Local.MaterializeInputs\",\"ts\":102,\"dur\":83000000}";
        let remote = "{\"name\":\"Re.Queue\",\"ts\":101,\"dur\":5000}";
        let body = format!(
            "[\n{},\n{},\n{}\n]\n",
            row("slow (link)", local, 69_000_000),
            row("quick (link)", local, 10_000),
            row("lost remote (link)", remote, 0),
        );
        let (tr, x) = census_of(&body, "cancelled-census");
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let c = cancelled_census(&f).unwrap();
        assert_eq!((c.actions, c.local), (3, 2));
        assert_eq!(c.cancelled_us, 2 * 83_000_000 + 5_000);
        assert_eq!(
            (c.slow_gathers, c.slow_gather_us),
            (1, 69_000_000),
            "only the upload that waited at least a second while its local attempt prepared inputs"
        );
        assert_eq!(c.by_phase[0], ("Local.MaterializeInputs".to_owned(), 2));
        assert_eq!(c.rows[0].phase, "Local.MaterializeInputs");
        assert!(!c.rows[2].local, "the remote attempt lost the third race");
    }

    /// The review's three race fixtures: a 10 s winner with an
    /// overlapping 9 s loser, the same without the winner's wall, and
    /// the same with a 90 s tail after the command. None may become a
    /// rerun with a delay.
    #[test]
    fn retry_census_never_invents_a_rerun() {
        let race = |dur: i64, wall: &str| {
            format!(
                "[\n{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"race (compile)\",\"ts\":100000000,\
                 \"dur\":{dur},\"args\":{{\"run\":\"r\",\"src\":\"compile\",{wall}\"tries\":2,\
                 \"attempt_status\":[\"Cancelled\"],\"attempts\":[{{\"status\":\"Cancelled\",\
                 \"dur_us\":9000000,\"start\":100000000,\"elastic\":false}}],\
                 \"stages\":{{\"execute_us\":10000000}}}}}},\n"
            )
        };
        let cases = [
            (
                "race",
                race(10_000_000, "\"winner\":{\"dur_us\":10000000},"),
                1,
                0,
            ),
            (
                "legacy-action-wall",
                race(10_000_000, "\"wall_time_us\":10000000,"),
                0,
                1,
            ),
            ("race-missing-winner", race(10_000_000, ""), 0, 1),
            (
                "race-tail",
                race(100_000_000, "\"winner\":{\"dur_us\":10000000},"),
                0,
                1,
            ),
        ];
        for (tag, body, concurrent, unknown) in cases {
            let (tr, x) = census_of(&body, tag);
            let f = Facts::for_run(&tr, &x, None).unwrap();
            let c = retry_census(&f, 10).unwrap();
            assert_eq!(
                (c.concurrent, c.rerun, c.unknown, c.delay_us, c.lost_us),
                (concurrent, 0, unknown, 0, 9_000_000),
                "{tag}"
            );
            assert_eq!(c.exact_fates, 0, "{tag}: no client starts on the wire");
        }
    }

    /// With a client start on both attempts the placement is exact: the
    /// 20 s action whose loser ended before the winner began is a rerun
    /// with the loser's duration as its delay; the same loser against a
    /// winner that started earlier is concurrent.
    #[test]
    fn retry_census_classifies_exactly_with_client_starts() {
        let row = |name: &str, winner_start: i64| {
            format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"{name} (compile)\",\"ts\":100000000,\
                 \"dur\":20000000,\"args\":{{\"run\":\"r\",\"src\":\"compile\",\
                 \"wall_time_us\":20000000,\"winner\":{{\"client_start\":{winner_start},\"dur_us\":10000000}},\
                 \"tries\":2,\"attempt_status\":[\"Cancelled\"],\"attempts\":[{{\"status\":\
                 \"Cancelled\",\"dur_us\":9000000,\"start\":100000000,\"client_start\":100000000,\
                 \"elastic\":false}}],\"stages\":{{\"execute_us\":10000000}}}}}},\n"
            )
        };
        let body = format!(
            "[\n{}{}",
            row("serial", 110_000_000),
            row("overlap", 105_000_000)
        );
        let (tr, x) = census_of(&body, "client-starts");
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let c = retry_census(&f, 10).unwrap();
        assert_eq!((c.concurrent, c.rerun, c.unknown), (1, 1, 0));
        assert_eq!(c.exact_fates, 2);
        assert_eq!(c.delay_us, 9_000_000);
        let serial = c
            .rows
            .iter()
            .find(|r| f.name(r.ev).starts_with("serial"))
            .unwrap();
        assert_eq!(
            serial.fates,
            [(AttemptFate::Rerun, FateBasis::ClientInterval)]
        );
        assert_eq!(serial.attempts[0].client_start_us, Some(100_000_000));
        assert_eq!(serial.delay_us, 9_000_000);
    }

    #[test]
    fn execution_accounting_distinguishes_reused_metadata_from_current_spans() {
        assert_eq!(current_execution_us(Some(900), true, None, None), None);
        assert_eq!(
            current_execution_us(Some(900), false, Some("reused"), None),
            None
        );
        assert_eq!(
            current_execution_us(Some(40), true, Some("reused"), Some("client_span")),
            Some(40)
        );
        assert_eq!(
            current_execution_us(Some(900), true, None, Some("unknown")),
            None
        );
        assert_eq!(current_execution_us(Some(40), false, None, None), Some(40));
        assert_eq!(current_execution_us(Some(0), false, None, None), Some(0));
        assert_eq!(current_execution_us(None, false, None, None), None);
    }

    #[test]
    fn extras_do_not_spend_cached_history_as_current_execution() {
        let cases = [
            ("legacy-cache", true, None, None, Some(0), None),
            ("reused-remote", false, Some("reused"), None, Some(0), None),
            (
                "local-work-before-cache",
                true,
                Some("reused"),
                Some("client_span"),
                Some(40),
                None,
            ),
            ("current-execution", false, None, None, Some(40), Some(40)),
            ("cache-no-stages", true, None, None, None, None),
        ];
        let presence_cases = [
            (
                "historical-only-cache",
                serde_json::json!({"cache": {"hit": true}, "stages": {"execute_us": 90_000}}),
                None,
            ),
            (
                "historical-only-reused-remote",
                serde_json::json!({"execution_metadata": "reused", "stages": {"execute_us": 90_000}}),
                None,
            ),
            (
                "zero-current-execution",
                serde_json::json!({"stages": {"execute_us": 0}}),
                Some(0),
            ),
            (
                "zero-client-execution-cache",
                serde_json::json!({"cache": {"hit": true}, "stages": {
                    "execute_us": 0, "execute_source": "client_span"
                }}),
                Some(0),
            ),
            (
                "zero-other-counter-cache",
                serde_json::json!({"cache": {"hit": true}, "stages": {
                    "execute_us": 90_000, "materialize_us": 0
                }}),
                Some(0),
            ),
            (
                "slot-only",
                serde_json::json!({"stages": {}, "re": {"rs_exec_sem_wait_us": 4}}),
                Some(0),
            ),
            (
                "zero-slot-only",
                serde_json::json!({"stages": {}, "re": {"rs_exec_sem_wait_us": 0}}),
                Some(0),
            ),
            (
                "slot-without-stages",
                serde_json::json!({"re": {"rs_exec_sem_wait_us": 4}}),
                None,
            ),
            ("empty-stages", serde_json::json!({"stages": {}}), None),
            (
                "unknown-only-stages",
                serde_json::json!({"stages": {"unknown_us": 10}}),
                None,
            ),
            (
                "source-only-stages",
                serde_json::json!({"stages": {"execute_source": "client_span"}}),
                None,
            ),
            (
                "null-counter-stages",
                serde_json::json!({"stages": {"execute_us": null, "materialize_us": null}}),
                None,
            ),
        ];
        let mut body = String::from("[\n");
        for (name, cache, metadata, source, stages, _) in cases {
            let mut args = serde_json::json!({"exec_us": 40, "cpu_us": 7, "elastic": true});
            if cache {
                args["cache"] = serde_json::json!({"hit": true});
            }
            if let Some(metadata) = metadata {
                args["execution_metadata"] = metadata.into();
            }
            if stages.is_some() {
                args["stages"] = serde_json::json!({
                    "cache_query_us": 10,
                    "materialize_us": 20,
                    "execute_us": if cache && source.is_none() { 90_000 } else { 40 }
                });
                if let Some(source) = source {
                    args["stages"]["execute_source"] = source.into();
                }
            }
            let row = serde_json::json!({
                "ph": "X", "cat": "recipe", "name": name,
                "ts": 0, "dur": 100, "args": args
            });
            body.push_str(&format!("{row},\n"));
        }
        for (name, args, _) in &presence_cases {
            let row = serde_json::json!({
                "ph": "X", "cat": "recipe", "name": name,
                "ts": 0, "dur": 100, "args": args
            });
            body.push_str(&format!("{row},\n"));
        }
        body.push_str("]\n");
        let p = std::env::temp_dir().join(format!(
            "retrace-current-execution-{}.json",
            std::process::id()
        ));
        std::fs::write(&p, body).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let x = load_extras(&tr);
        std::fs::remove_file(&p).ok();
        assert!(x.source_error.is_none());
        assert_eq!(x.unmatched, 0);
        for (name, _, _, _, execution, command_execution) in cases {
            let i = tr
                .acts
                .iter()
                .position(|a| tr.names[a.name as usize] == name)
                .unwrap();
            let e = &x.args[i];
            assert_eq!(e.stages.map(|st| st.execute_us), execution, "{name}");
            assert_eq!(e.exec_us, command_execution, "{name}");
            assert_eq!(
                e.cpu_us,
                if command_execution.is_some() { 7 } else { 0 },
                "{name}"
            );
            assert_eq!(e.elastic, command_execution.is_some(), "{name}");
            if let Some(stages) = e.stages {
                assert_eq!(stages.cache_query_us, 10, "{name}: actual lookup stays");
                assert_eq!(stages.materialize_us, 20, "{name}: actual fetch stays");
            }
        }
        for (name, args, execution) in presence_cases {
            let i = tr
                .acts
                .iter()
                .position(|a| tr.names[a.name as usize] == name)
                .unwrap();
            let e = &x.args[i];
            assert_eq!(e.stages.map(|st| st.execute_us), execution, "{name}");
            assert_eq!(
                e.exec_sem_wait_us,
                args["re"]["rs_exec_sem_wait_us"].as_i64().unwrap_or(0),
                "{name}: permit wait remains available independently"
            );
        }
    }

    #[test]
    fn local_cache_counter_preserves_optional_presence_and_repair() {
        for (counter, expected) in [
            (serde_json::Value::Null, None),
            (serde_json::json!(0), Some(0)),
            (serde_json::json!(21_550_264), Some(21_550_264)),
            (serde_json::json!(-1), None),
            (serde_json::json!("invalid"), None),
        ] {
            let path = crate::temp_path("local-cache-counter.json");
            let row = serde_json::json!({
                "ph":"X", "cat":"recipe", "name":"cached action", "ts":0, "dur":30_000_000,
                "args":{"cache":{},"stages":{"local_cache_check_us":counter}}
            });
            std::fs::write(&path, format!("[\n{row}\n]\n")).unwrap();
            let trace = parse::parse(path.to_str().unwrap()).unwrap();
            let extras = load_extras(&trace);
            std::fs::remove_file(path).unwrap();
            assert!(extras.source_error.is_none(), "{:?}", extras.source_error);
            assert_eq!(
                extras.args[0].stages.map(|s| s.local_cache_check_us),
                expected
            );
            if let Some(stages) = extras.args[0].stages {
                assert_eq!(stages.hard_sum(), expected.unwrap());
            }
        }
    }

    #[test]
    fn typed_hash_composition_matches_raw_detail_after_optional_field_repair() {
        use serde_json::Value;
        use serde_json::json;
        let cases = [
            (json!("local_outputs"), json!(12)),
            (json!("local_outputs"), json!(0)),
            (json!("local_outputs"), Value::Null),
            (json!("local_outputs"), json!(-1)),
            (json!("local_outputs"), json!("12")),
            (json!("worker"), json!(12)),
            (Value::Null, json!(12)),
            (json!(true), json!(12)),
        ];
        for (source, parent) in cases {
            let args = json!({"stages": {
                "hashing_us": 18, "hashing_source": source,
                "local_finalize_outputs_us": parent, "cas_artifact_us": 3,
            }});
            let p = crate::temp_path("hash-composition.json");
            let row = json!({"ph": "X", "cat": "recipe", "name": "job", "ts": 0, "dur": 100, "args": args});
            std::fs::write(&p, format!("[\n{row}\n]\n")).unwrap();
            let tr = parse::parse(p.to_str().unwrap()).unwrap();
            let extras = load_extras(&tr);
            std::fs::remove_file(&p).ok();
            assert!(extras.source_error.is_none(), "{:?}", extras.source_error);
            assert_eq!(extras.unmatched, 0);
            let typed = extras.args[0].stages.unwrap();
            let raw = crate::detail::stages_of(&args).unwrap();
            assert_eq!(typed.hashing_us, 18, "raw counter is retained");
            assert_eq!(typed.hard_sum(), raw.iter().sum::<i64>(), "{args}");
        }
    }

    #[test]
    fn inclusive_finalize_preserves_large_raw_hash_without_poisoning_composition() {
        for finalize_us in [0, 12] {
            let args = serde_json::json!({"stages": {
                "hashing_us": i64::MAX, "hashing_source": "local_outputs",
                "local_finalize_outputs_us": finalize_us, "cas_artifact_us": 3,
            }});
            let path = crate::temp_path("inclusive-finalize-overflow.json");
            let row = serde_json::json!({
                "ph": "X", "cat": "recipe", "name": "job", "ts": 0, "dur": 100,
                "args": args,
            });
            std::fs::write(&path, format!("[\n{row}\n]\n")).unwrap();
            let trace = parse::parse(path.to_str().unwrap()).unwrap();
            let extras = load_extras(&trace);
            std::fs::remove_file(path).unwrap();
            assert!(extras.source_error.is_none(), "{:?}", extras.source_error);
            assert_eq!(extras.unmatched, 0);
            assert_eq!(extras.rejected_fields, 0);
            let stages = extras.args[0].stages.unwrap();
            assert_eq!(
                stages.hashing_us,
                i64::MAX,
                "raw duration sum remains available"
            );
            assert!(stages.hashing_in_finalize_outputs);
            assert_eq!(stages.hard_sum(), finalize_us + 3);
            assert_eq!(
                crate::detail::stages_of(&args).unwrap().iter().sum::<i64>(),
                finalize_us + 3,
            );
        }
    }

    #[test]
    fn poisoned_upload_and_admission_fields_never_become_measured_zero() {
        assert_eq!(
            std::mem::size_of::<UploadSub>(),
            std::mem::size_of::<([i64; 5], bool)>(),
            "field-local validity must fit the old counter layout"
        );
        for (re, expected_diagnostics) in [
            (
                serde_json::json!({"submit_us": 20, "iu_cas_call_us": 12,
                "invalid_counter_fields": ["iu_sem_wait_us", "rs_exec_sem_wait_us"]}),
                2,
            ),
            (
                serde_json::json!({"submit_us": 20, "iu_cas_call_us": 12,
                "iu_sem_wait_us": -1, "rs_exec_sem_wait_us": -1}),
                2,
            ),
        ] {
            let path = crate::temp_path("poisoned-counters.json");
            std::fs::write(
                &path,
                format!(
                    "[\n{}\n]\n",
                    serde_json::json!({
                        "ph": "X", "cat": "recipe", "name": "job", "ts": 1, "dur": 100,
                        "args": {"re": re, "bytes_in": 42, "stages": {"execute_us": 7}}
                    })
                ),
            )
            .unwrap();
            let trace = parse::parse(path.to_str().unwrap()).unwrap();
            let extras = load_extras(&trace);
            assert_eq!(extras.unmatched, 0);
            assert_eq!(extras.rejected_fields, expected_diagnostics);
            let action = &extras.args[0];
            let upload = action.iu.unwrap();
            assert_eq!(upload.phase(0), None);
            assert_eq!(
                upload.phase(3),
                Some(12),
                "unrelated valid CAS duration survives"
            );
            assert!(!action.admission_recorded);
            assert_eq!(action.submit_us, 20);
            assert_eq!(action.bytes_in, 42);
            assert_eq!(action.stages.unwrap().execute_us, 7);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn rejected_upload_fields_alone_do_not_establish_measurements() {
        for re in [
            serde_json::json!({"invalid_counter_fields": ["iu_digests_queried"]}),
            serde_json::json!({"iu_digests_queried": -1}),
            serde_json::json!({"invalid_counter_fields": ["iu_sem_wait_us"]}),
        ] {
            let row = serde_json::json!({"ph":"X", "cat":"recipe", "name":"a", "ts":0, "dur":10, "args":{"re":re}});
            let (_, x) = census_of(&format!("[\n{row}\n]\n"), "invalid-only-upload");
            assert!(x.args[0].iu.is_none());
        }
    }

    #[test]
    fn overview_bounds_foreign_counters_and_machine_measurements() {
        let (tr, x) = census_of(
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":0,"args":{"ncpu":8,"mem":-1}}"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10}"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_end","ts":20,"args":{"exit":0,"cache":{"hits":9223372036854775807,"misses":1}}}"#,
                "\n]\n"
            ),
            "foreign-overview",
        );
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let rows = overview_rows(&f, &overview_facts(&f), None, &fmt_when_utc);
        assert!(rows.iter().any(|r| r.key == "Action cache"));
        assert_eq!(
            rows.iter().find(|r| r.key == "Machine").unwrap().value,
            "8 cores"
        );
        let empty = Extras::default();
        let f = Facts::for_run(&tr, &empty, None).unwrap();
        assert!(!f.args(0).decoded);
        assert!(f.args(0).src.is_none());
    }

    #[test]
    fn retry_rankings_break_ties_by_name_and_keep_total_categories() {
        let rows = ["zeta", "beta", "delta", "alpha"].map(|name| {
            serde_json::json!({"ph":"X", "cat":"recipe", "name":name, "ts":0, "dur":10,
                "args":{"src":name, "tries":2, "attempt_status":[name]}})
            .to_string()
        });
        let (tr, x) = census_of(&format!("[\n{}\n]\n", rows.join("\n")), "retry-ties");
        let f = Facts::for_run(&tr, &x, None).unwrap();
        let c = retry_census(&f, 2).unwrap();
        assert_eq!(c.n_categories, 4);
        assert_eq!(
            c.by_category
                .iter()
                .map(|r| r.0.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert_eq!(
            c.by_status.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta", "delta", "zeta"]
        );
    }

    #[test]
    fn cached_result_and_request_preparation_survive_lean_extras_decode() {
        for (stages, expected, rejected) in [
            (
                serde_json::json!({"cache_hit_us": 60, "prepare_us": 5, "cache_query_us": 30, "daemon_us": 5}),
                Some((60, 5, 95)),
                0,
            ),
            (
                serde_json::json!({"cache_hit_us": 0, "prepare_us": 0}),
                Some((0, 0, 0)),
                0,
            ),
            (
                serde_json::json!({"cache_hit_us": -1, "prepare_us": 5}),
                Some((0, 5, 5)),
                1,
            ),
            (
                serde_json::json!({"cache_hit_us": null, "prepare_us": null}),
                None,
                0,
            ),
        ] {
            let path = crate::temp_path("cache-stage-counters.json");
            std::fs::write(
                &path,
                format!(
                    "[\n{}\n]\n",
                    serde_json::json!({
                        "ph": "X", "cat": "recipe", "name": "cached action", "ts": 1, "dur": 100,
                        "args": {"cache": {}, "execution_metadata": "reused", "stages": stages}
                    })
                ),
            )
            .unwrap();
            let trace = parse::parse(path.to_str().unwrap()).unwrap();
            let extras = load_extras(&trace);
            assert_eq!(extras.unmatched, 0);
            assert_eq!(extras.rejected_fields, rejected);
            assert_eq!(
                extras.args[0]
                    .stages
                    .map(|s| (s.cache_hit_us, s.prepare_us, s.hard_sum())),
                expected
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn watcher_counts_are_not_file_counts_and_incomplete_samples_are_explicit() {
        let text = watcher_summary(&serde_json::json!({
            "events_processed": 40, "events_total": 50, "fresh_instance": true,
            "paths_truncated": true, "reasons_truncated": true,
            "incomplete_events_reasons": ["producer dropped events"]
        }));
        for expected in [
            "processed: 40",
            "reported: 50",
            "not unique files",
            "fresh watcher",
            "truncated sample",
            "producer dropped events",
            "limitations omitted",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(
            watcher_summary(&serde_json::json!({"events_total": null})).contains("not recorded")
        );
    }

    #[test]
    fn run_records_totals_and_cache() {
        let (_, x) = smoke();
        let (s, e) = run_bounds(&x.meta, Some("aaaa-bbbb"));
        assert!(s.is_some() && e.is_some());
        let t = run_totals(&x.meta, Some("aaaa-bbbb")).unwrap();
        assert_eq!((t.remade, t.uptodate, t.failed), (2, 0, 1));
        let c = cache_stats(&x.meta, None).unwrap();
        assert_eq!(c[0], 1); // hits
        assert_eq!(c[1], 2); // misses
        assert_eq!(c[6], 2048); // restored_bytes
        assert_eq!(c[9], 100); // key_us
    }
}
