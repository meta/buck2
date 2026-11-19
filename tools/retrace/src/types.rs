/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared data model. Interfaces here are pinned so the graph/dag/analyses
//! modules can be implemented against them independently; fields may grow,
//! existing meanings must not change.
//!
//! Lean relative to the viewer-proto original: buck2 traces are the only
//! input, so the fields the GNU-Make importer populated and the buck2
//! importer emits as constants are gone — `pid` (always 1, the daemon),
//! `makelevel` (always 0), `slot` (the importer's own lane packing, which
//! we redo), `serial` (never emitted), per-act `host` (always "buck2";
//! the RE worker lives in `args.re`, read lazily from the source line),
//! and the recursive-wrapper flag. Row layout is always packed
//! (lanes::assign); the slots/level modes packed make processes.

use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
#[cfg(unix)]
use std::os::unix::fs::FileExt as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
#[cfg(windows)]
use std::os::windows::fs::FileExt as _;

/// `Act::flags` bits.
pub const F_FAILED: u8 = 1;
pub const F_CACHE_HIT: u8 = 2;
/// `cat` is `"cache"` or `"phase"`: context, NOT an action. The timeline
/// still shows these rows, so they are kept as acts and flagged instead;
/// graph/dag/analyses skip them.
pub const F_NOT_RECIPE: u8 = 4;

pub const NO_STR: u32 = u32::MAX;

/// Wire microseconds, including the legacy protobuf [seconds, nanos] shape.
pub fn micros_value(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::Array(xs) if xs.len() == 2 => {
            let (s, nanos) = (xs[0].as_i64()?, xs[1].as_i64()?);
            if !(0..1_000_000_000).contains(&nanos) {
                return None;
            }
            s.checked_mul(1_000_000)?.checked_add(nanos / 1_000)
        }
        _ => None,
    }
}

/// A labelled group of rows: the buck2 daemon's packed concurrency lanes,
/// or the trailing cache/phase rows. Content occupies `base..base+nrows`
/// with no row reserved for the label, so a row index is a lane number.
#[derive(Debug)]
pub struct Band {
    pub base: u32,
    pub nrows: u32,
    /// The band's name, shown in the gutter beside its first row.
    pub gutter: String,
}

#[derive(Clone, Copy, Debug)]
pub struct Act {
    pub ts: i64,  // us, rebased to trace start
    pub dur: i64, // us
    pub lane: u32,
    pub color: u8, // index into COLORS/CLASS_NAMES
    pub flags: u8, // F_* bits above
    pub name: u32, // index into Trace::names
    pub off: u64,  // byte offset of source line (lazy full-record load)
    pub len: u32,
    /// file order of this act (acts are re-sorted by lane); ts-sorted
    /// views must break ts ties on this so ordering matches the source.
    pub seq: u32,
    /// index into `Trace::dirs` (`args.dir`), `NO_STR` when absent
    pub dir: u32,
    /// index into `Trace::whys` (`args.why`), `NO_STR` when absent
    pub why: u32,
    /// index into `Trace::run_ids` (`args.run`), `NO_STR` when absent
    pub run: u32,
    /// `args.cache.bytes`
    pub cache_bytes: Option<f64>,
}

#[derive(Debug)]
pub struct Lane {
    pub start: u32,
    pub end: u32,
    pub max_dur: i64,
}

/// One build invocation's metadata, from the `run_start`/`run_end`
/// records trace-buck.py synthesizes out of the buck2 Invocation header,
/// SystemInfo, the Command span and the Result line.
#[derive(Default, Clone, Debug)]
pub struct RunInfo {
    pub run: String,
    /// Exact RE correlation ID, when recorded; distinct from the build UUID.
    pub re_session: Option<String>,
    pub ts: i64,
    /// job limit; buck2 runs unbounded, shown as "default (parallel)"
    pub jobs: Option<i64>,
    pub ncpu: Option<i64>,
    /// the trace format's build-tool tag, always the literal "buck2" on
    /// buck2 captures.  For the machine, see `hostname`.
    pub host: Option<String>,
    /// the machine the build ran on, when the log recorded it
    pub hostname: Option<String>,
    /// who (or which agent) ran the build
    pub user: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub mem: Option<i64>,
    pub load: Option<String>,
    pub version: Option<String>,
    pub tool: Option<String>,
    /// the workspace root; popups confine trace-named paths to it
    pub cwd: Option<String>,
    pub argv: Option<String>,
    /// stamp of the capture this trace was written from; the graph
    /// beside it must carry the same one (`graph::check_pair`)
    pub capture: Option<String>,
    /// Lazily read command/output evidence, bound to this capture.
    pub execution_store: Option<crate::execution::StoreInfo>,
    /// Validated whole-command evidence, available during the initial parse.
    /// Absent for old captures or ambiguous/malformed profile metadata.
    pub command_profile: Option<crate::command_profile::CommandProfile>,
    /// what this build was called on the command line, e.g.
    /// `igios-no-extensions` — the short name a human uses for the
    /// capture, as opposed to the whole `argv`
    pub targets: Option<Vec<String>>,
    pub git: Option<String>,
    pub result: Option<i64>,
    /// how the build ended, as the `run_end` record says; a record with
    /// no result is `Unknown`, never success
    pub outcome: Outcome,
    /// the dependency capture's state as the `run_end` record says;
    /// `None` on traces written before the record carried one
    pub dep_capture: Option<CaptureState>,
    pub remade: Option<i64>,
    pub uptodate: Option<i64>,
    pub failed: Option<i64>,
    pub cache_hits: Option<i64>,
    pub cache_misses: Option<i64>,
    pub cache_stores: Option<i64>,
    /// watcher-reported changed files (first 50) — the recorded root
    /// cause of an incremental rebuild — plus the untruncated count.
    pub file_changes: Option<Vec<String>>,
    pub file_changes_total: Option<i64>,
    /// a `run_start` header was seen (a duplicated header must not
    /// overwrite the first one's fields)
    pub started: bool,
    /// a `run_end` footer was seen; a run without one is torn and the
    /// capture warns per such run
    pub ended: bool,
    pub duplicate_records: usize,
    pub end_ts: Option<i64>,
}

#[derive(Default, Debug)]
pub struct Health {
    pub warnings: Vec<String>, // human strings, INCOMPLETE banner when non-empty
    /// undecodable lines skipped; rows may be missing
    pub dropped: usize,
    pub rejected_fields: usize,
    pub graph_warning: Option<String>,
    /// runs with no `run_end` footer (killed build or torn capture)
    pub unended: usize,
}

/// What the log says about how the build ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failed,
    /// the log holds neither a command result nor a command end
    #[default]
    Unknown,
}

impl Outcome {
    /// The one reading of a `run_end` every consumer shares: the
    /// `outcome` string when the record carries one, else the
    /// `result`/`exit` integer. An absent or null value is unknown.
    pub fn from_run_end_args(a: &serde_json::Value) -> Outcome {
        match a.get("outcome").and_then(|v| v.as_str()) {
            Some("success") => return Outcome::Success,
            Some("failed") => return Outcome::Failed,
            Some(_) => return Outcome::Unknown,
            None => {}
        }
        let code = a
            .get("result")
            .and_then(|v| v.as_i64())
            .or_else(|| a.get("exit").and_then(|v| v.as_i64()));
        match code {
            Some(0) => Outcome::Success,
            Some(_) => Outcome::Failed,
            None => Outcome::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failed => "failed",
            Outcome::Unknown => "unknown",
        }
    }
}

/// Whether a dependency-capture footer's counts describe the whole
/// capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Finality {
    /// no footer, or one written before finality was recorded
    #[default]
    Unknown,
    Final,
    /// the command returned before the capture settled; the counts are
    /// a snapshot even when they agree
    Nonfinal,
}

impl Finality {
    pub fn as_str(self) -> &'static str {
        match self {
            Finality::Unknown => "unknown",
            Finality::Final => "final",
            Finality::Nonfinal => "nonfinal",
        }
    }

    fn parse(s: Option<&str>) -> Finality {
        match s {
            Some("final") => Finality::Final,
            Some("nonfinal") => Finality::Nonfinal,
            _ => Finality::Unknown,
        }
    }
}

/// The counts a capture declares in its footer, or the counts a reader
/// tallied; `None` where nothing was declared or nothing is observable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureCounts {
    pub actions_recorded: Option<u64>,
    pub tset_def_events: Option<u64>,
}

impl CaptureCounts {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "actions_recorded": self.actions_recorded,
            "tset_def_events": self.tset_def_events,
        })
    }

    fn from_json(v: Option<&serde_json::Value>) -> CaptureCounts {
        let n = |k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_u64());
        CaptureCounts {
            actions_recorded: n("actions_recorded"),
            tset_def_events: n("tset_def_events"),
        }
    }
}

/// Contradictions in recorded identities cannot establish a lower bound.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize
)]
#[serde(default)]
pub struct CaptureIntegrity {
    pub malformed_records: u64,
    pub conflicting_footers: u64,
    pub ambiguous_producer_spans: u64,
    pub invalid_span_ids: u64,
    pub invalid_tset_ids: u64,
    pub dangling_producer_spans: u64,
    pub self_references: u64,
    pub orphan_action_ends: u64,
    pub foreign_event_ids: u64,
    #[serde(skip_serializing_if = "zero_count")]
    pub conflicting_capture_schemas: u64,
    #[serde(skip_serializing_if = "zero_count")]
    pub conflicting_capture_scopes: u64,
    #[serde(skip_serializing_if = "zero_count")]
    pub structural_conflicts: u64,
    #[serde(skip_serializing_if = "zero_count")]
    pub alias_cycles: u64,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if callbacks receive a borrowed field"
)]
fn zero_count(n: &u64) -> bool {
    *n == 0
}

impl CaptureIntegrity {
    fn problems(&self) -> impl Iterator<Item = String> + '_ {
        [
            (self.malformed_records, "malformed integrity records"),
            (self.conflicting_footers, "contradictory capture footers"),
            (self.ambiguous_producer_spans, "ambiguous producer spans"),
            (self.invalid_span_ids, "invalid producer span IDs"),
            (
                self.invalid_tset_ids,
                "transitive-set IDs outside the assigned range",
            ),
            (
                self.dangling_producer_spans,
                "unresolved producer span references",
            ),
            (self.self_references, "self-referencing dependency records"),
            (
                self.orphan_action_ends,
                "action ends without a matching start",
            ),
            (self.foreign_event_ids, "events from another command"),
            (
                self.conflicting_capture_schemas,
                "contradictory capture schemas",
            ),
            (
                self.conflicting_capture_scopes,
                "contradictory capture scopes",
            ),
            (
                self.structural_conflicts,
                "contradictory structural identities or provenance",
            ),
            (self.alias_cycles, "cycles in action identity aliases"),
        ]
        .into_iter()
        .filter(|(n, _)| *n != 0)
        .map(|(n, what)| format!("dependency-capture integrity: {n} {what}"))
    }

    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Schema-2 structural capture is separate from measured action execution.
/// Old captures have no value here; absent counts never prove a completed walk.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize
)]
#[serde(default)]
pub struct StructureCapture {
    pub complete: Option<bool>,
    pub lookup_failures: Option<u64>,
    pub unexpanded_reused_roots: Option<u64>,
    pub declared: StructureCounts,
    pub observed: StructureCounts,
    #[serde(skip_serializing_if = "false_flag")]
    pub malformed: bool,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize
)]
#[serde(default)]
pub struct StructureCounts {
    pub definitions: Option<u64>,
    pub definition_events: Option<u64>,
    pub alias_events: Option<u64>,
    pub unproven: Option<u64>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if callbacks receive a borrowed field"
)]
fn false_flag(flag: &bool) -> bool {
    !*flag
}

impl StructureCapture {
    fn count_pairs(&self) -> [(&'static str, Option<u64>, Option<u64>); 4] {
        [
            (
                "structural definitions",
                self.declared.definitions,
                self.observed.definitions,
            ),
            (
                "structural definition events",
                self.declared.definition_events,
                self.observed.definition_events,
            ),
            (
                "alias events",
                self.declared.alias_events,
                self.observed.alias_events,
            ),
            (
                "unproven structural definitions",
                self.declared.unproven,
                self.observed.unproven,
            ),
        ]
    }

    pub fn is_complete(&self) -> bool {
        !self.malformed
            && self.complete == Some(true)
            && self.lookup_failures == Some(0)
            && self.unexpanded_reused_roots == Some(0)
            && self.observed.unproven == Some(0)
            && self.count_pairs().into_iter().all(|(_, d, o)| agrees(d, o))
    }

    fn omission_only(&self) -> bool {
        !self.malformed
            && !self
                .count_pairs()
                .into_iter()
                .any(|(_, d, o)| matches!((d, o), (Some(d), Some(o)) if o > d))
            && !matches!((self.declared.definition_events, self.declared.definitions),
                (Some(e), Some(d)) if d > e)
    }

    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.malformed {
            out.push("malformed structural-capture metadata".to_owned());
        }
        match self.unexpanded_reused_roots {
            Some(n) if n > 0 => out.push(format!(
                "{n} cached BXL command root(s) may hide internal ctx.build inputs; explicitly ensured outputs remain captured (not a count of missing actions)"
            )),
            None => out.push("unexpanded reused command roots were not recorded".to_owned()),
            _ => {}
        }
        if self.complete != Some(true) {
            out.push("the artifact-structure walk did not establish completion".to_owned());
        }
        if let Some(n) = self.lookup_failures.filter(|&n| n > 0) {
            out.push(format!(
                "{n} artifact-structure lookup(s) failed; some input paths remain unknown"
            ));
        } else if self.lookup_failures.is_none() {
            out.push("artifact-structure lookup failures were not recorded".to_owned());
        }
        if let Some(n) = self.observed.unproven.filter(|&n| n > 0) {
            out.push(format!(
                "{n} structural definition(s) lack positive availability or reuse evidence"
            ));
        }
        for (name, declared, observed) in self.count_pairs() {
            if !agrees(declared, observed) {
                out.push(format!(
                    "{name}: declared {declared:?}, observed {observed:?}"
                ));
            }
        }
        out
    }
}

/// Requested graph domain, independent of integrity within that domain.
/// Missing scope in old JSON preserves its historical behavior; it does not
/// establish new evidence that a legacy graph contains every reused producer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureScope {
    #[default]
    Full,
    CurrentActionAncestors,
    Unknown,
}

impl CaptureScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::CurrentActionAncestors => "current_action_ancestors",
            Self::Unknown => "unknown",
        }
    }

    fn from_json(value: Option<&serde_json::Value>) -> Self {
        match value {
            None => Self::Full,
            Some(value) => match value.as_str() {
                Some("full") => Self::Full,
                Some("current_action_ancestors") => Self::CurrentActionAncestors,
                _ => Self::Unknown,
            },
        }
    }
}

/// How complete a dependency capture is within its declared scope, from what the log declares
/// against what the reader saw. Independent of the build's outcome: a
/// failed build can have a complete capture and a successful one an
/// incomplete capture.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureStatus {
    /// The action-graph schema the log declared, if any.
    pub schema: Option<u32>,
    pub scope: CaptureScope,
    /// Producer's successful structural walk, not action time or client log drain.
    pub structure_wall_time_us: Option<u64>,
    pub footer_present: bool,
    pub finality: Finality,
    /// from the footer; all `None` without one
    pub declared: CaptureCounts,
    /// tallied by the scan
    pub observed: CaptureCounts,
    /// the decoder stopped on this error after `records_read` records
    pub decode_error: Option<String>,
    pub records_read: Option<u64>,
    pub command_result_seen: bool,
    pub command_end_seen: bool,
    pub integrity: CaptureIntegrity,
    pub actions_cancelled: u64,
    /// Trace ids of other commands the daemon ran at the same time. An action one of them ran
    /// that this command also needed has its span in that command's log, so here it looks
    /// reused and takes no time.
    pub concurrent_commands: Vec<String>,
    pub structure: Option<StructureCapture>,
}

fn agrees(declared: Option<u64>, observed: Option<u64>) -> bool {
    matches!((declared, observed), (Some(d), Some(o)) if d == o)
}

impl CaptureStatus {
    /// The footer is present and final, the log decoded to its end, and
    /// every declared count matches what was recorded. An absent
    /// declaration is never taken as satisfied.
    pub fn is_complete(&self) -> bool {
        self.scope != CaptureScope::Unknown
            && (self.scope != CaptureScope::CurrentActionAncestors || self.structure.is_some())
            && self.footer_present
            && self.finality == Finality::Final
            && self.decode_error.is_none()
            && self.omission_only()
            && agrees(
                self.declared.actions_recorded,
                self.observed.actions_recorded,
            )
            && agrees(self.declared.tset_def_events, self.observed.tset_def_events)
            && self.concurrent_commands.is_empty()
            && self
                .structure
                .as_ref()
                .is_none_or(StructureCapture::is_complete)
    }

    /// Every way this capture falls short, in plain language; empty
    /// when complete.
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self.integrity.problems().collect();
        if self.scope == CaptureScope::Unknown {
            out.push("the dependency capture declares an unrecognized graph scope".to_owned());
        }
        if self.scope == CaptureScope::CurrentActionAncestors && self.structure.is_none() {
            out.push(
                "the scoped dependency capture has no structural completeness accounting"
                    .to_owned(),
            );
        }
        if let Some(e) = &self.decode_error {
            out.push(format!(
                "the log stopped decoding after {} record(s): {e}",
                self.records_read.unwrap_or(0)
            ));
        }
        if !self.footer_present {
            out.push("no dependency-capture footer: graph finality is unverified".to_owned());
        } else if self.finality == Finality::Nonfinal {
            out.push(
                "the dependency-capture footer is nonfinal: the command returned before the \
                 capture settled"
                    .to_owned(),
            );
        } else if self.finality == Finality::Unknown {
            out.push(
                "the dependency-capture footer does not declare finality: settled counts are unverified"
                    .to_owned(),
            );
        }
        let mut mismatch =
            |what: &str, declared: Option<u64>, observed: Option<u64>| match (declared, observed) {
                (Some(d), Some(o)) if d != o => out.push(format!(
                    "the dependency capture declares {d} {what}, {o} were recorded"
                )),
                (Some(d), None) => out.push(format!(
                    "the dependency capture declares {d} {what}, none were counted"
                )),
                (None, _) if self.footer_present => out.push(format!(
                    "the dependency-capture footer does not declare {what}"
                )),
                _ => {}
            };
        mismatch(
            "action(s)",
            self.declared.actions_recorded,
            self.observed.actions_recorded,
        );
        mismatch(
            "definition event(s)",
            self.declared.tset_def_events,
            self.observed.tset_def_events,
        );
        if let Some(structure) = &self.structure {
            out.extend(structure.problems());
        }
        if !self.concurrent_commands.is_empty() {
            out.push(format!(
                "the command ran alongside {} other command(s) ({}): actions they ran for it are \
                 recorded as reused and take no time here",
                self.concurrent_commands.len(),
                self.concurrent_commands.join(", ")
            ));
        }
        out
    }

    /// Surplus observations are not evidence of omissions alone, so
    /// they cannot justify a bound.
    pub fn omission_only(&self) -> bool {
        let no_surplus =
            |d: Option<u64>, o: Option<u64>| !matches!((d, o), (Some(d), Some(o)) if o > d);
        self.scope != CaptureScope::Unknown
            && self.integrity.is_empty()
            && no_surplus(
                self.declared.actions_recorded,
                self.observed.actions_recorded,
            )
            && no_surplus(self.declared.tset_def_events, self.observed.tset_def_events)
            && self
                .structure
                .as_ref()
                .is_none_or(StructureCapture::omission_only)
    }

    fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "state": "started",
            "footer_present": self.footer_present,
            "finality": self.finality.as_str(),
            "declared": self.declared.to_json(),
            "observed": self.observed.to_json(),
            "decode_error": self.decode_error,
            "records_read": self.records_read,
            "command_result_seen": self.command_result_seen,
            "command_end_seen": self.command_end_seen,
            "integrity": self.integrity,
            "actions_cancelled": self.actions_cancelled,
        });
        if !self.concurrent_commands.is_empty() {
            value["concurrent_commands"] = serde_json::json!(self.concurrent_commands);
        }
        if let Some(structure) = &self.structure {
            value["structure"] = serde_json::to_value(structure).expect("capture counts serialize");
        }
        if self.scope != CaptureScope::Full {
            value["scope"] = serde_json::json!(self.scope.as_str());
        }
        if let Some(schema) = self.schema {
            value["schema"] = serde_json::json!(schema);
        }
        if let Some(us) = self.structure_wall_time_us {
            value["structure_wall_time_us"] = serde_json::json!(us);
        }
        value
    }

    fn from_json(v: &serde_json::Value) -> CaptureStatus {
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        CaptureStatus {
            schema: v
                .get("schema")
                .and_then(|n| n.as_u64())
                .and_then(|n| u32::try_from(n).ok()),
            scope: CaptureScope::from_json(v.get("scope")),
            structure_wall_time_us: v.get("structure_wall_time_us").and_then(|n| n.as_u64()),
            footer_present: b("footer_present"),
            finality: Finality::parse(v.get("finality").and_then(|x| x.as_str())),
            declared: CaptureCounts::from_json(v.get("declared")),
            observed: CaptureCounts::from_json(v.get("observed")),
            decode_error: v
                .get("decode_error")
                .and_then(|x| x.as_str())
                .map(|x| x.to_owned()),
            records_read: v.get("records_read").and_then(|x| x.as_u64()),
            command_result_seen: b("command_result_seen"),
            command_end_seen: b("command_end_seen"),
            integrity: match v.get("integrity") {
                None => CaptureIntegrity::default(),
                Some(i) => serde_json::from_value(i.clone()).unwrap_or_else(|_| CaptureIntegrity {
                    malformed_records: 1,
                    ..Default::default()
                }),
            },
            actions_cancelled: v
                .get("actions_cancelled")
                .and_then(|n| n.as_u64())
                .unwrap_or(0),
            concurrent_commands: v
                .get("concurrent_commands")
                .and_then(|ids| ids.as_array())
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            structure: v.get("structure").map(|s| {
                serde_json::from_value(s.clone()).unwrap_or_else(|_| StructureCapture {
                    malformed: true,
                    ..Default::default()
                })
            }),
        }
    }
}

/// Whether the log carried a dependency capture at all, and if so how
/// complete it is. A build run without `-c buck2.log_action_graph=true`
/// has no capture, which is not a gap in one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "one value per command, not per action: boxing would add an allocation without \
              shrinking a per-action structure"
)]
pub enum CaptureState {
    NotEnabled,
    Started(CaptureStatus),
}

impl CaptureState {
    pub fn scope(&self) -> Option<CaptureScope> {
        match self {
            Self::NotEnabled => None,
            Self::Started(status) => Some(status.scope),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            CaptureState::NotEnabled => serde_json::json!({"state": "not-enabled"}),
            CaptureState::Started(s) => s.to_json(),
        }
    }

    /// `None` for anything that is not a capture-state object.
    pub fn from_json(v: &serde_json::Value) -> Option<CaptureState> {
        match v.get("state").and_then(|x| x.as_str())? {
            "not-enabled" => Some(CaptureState::NotEnabled),
            "started" => Some(CaptureState::Started(CaptureStatus::from_json(v))),
            _ => None,
        }
    }

    /// `None` when no capture was enabled: there is nothing to be
    /// complete or incomplete.
    pub fn is_complete(&self) -> Option<bool> {
        match self {
            CaptureState::NotEnabled => None,
            CaptureState::Started(s) => Some(s.is_complete()),
        }
    }

    pub fn problems(&self) -> Vec<String> {
        match self {
            CaptureState::NotEnabled => Vec::new(),
            CaptureState::Started(s) => s.problems(),
        }
    }
}

#[cfg(test)]
mod capture_tests {
    #[test]
    fn structural_capture_requires_a_completed_reconciled_walk() {
        use super::*;
        let counts = StructureCounts {
            definitions: Some(1),
            definition_events: Some(2),
            alias_events: Some(1),
            unproven: Some(0),
        };
        let complete = StructureCapture {
            complete: Some(true),
            lookup_failures: Some(0),
            unexpanded_reused_roots: Some(0),
            declared: counts.clone(),
            observed: counts,
            malformed: false,
        };
        assert!(complete.is_complete());
        assert!(complete.problems().is_empty());
        let mut opaque_root = complete.clone();
        opaque_root.unexpanded_reused_roots = Some(1);
        assert!(!opaque_root.is_complete());
        assert!(opaque_root.omission_only());
        assert!(opaque_root.problems().iter().any(|p| p.contains("BXL")));
        let mut missing = complete.clone();
        missing.observed.definition_events = Some(1);
        assert!(!missing.is_complete());
        assert!(missing.omission_only());
        let mut contradictory = complete.clone();
        contradictory.observed.definition_events = Some(3);
        assert!(!contradictory.omission_only());
        let mut unavailable = complete.clone();
        unavailable.complete = None;
        assert!(!unavailable.is_complete());
        let state = CaptureState::Started(CaptureStatus {
            structure: Some(complete),
            ..Default::default()
        });
        assert_eq!(CaptureState::from_json(&state.to_json()), Some(state));
        let legacy = CaptureState::Started(CaptureStatus::default()).to_json();
        assert!(
            legacy.get("structure").is_none(),
            "legacy artifact payload stays unchanged"
        );
        let malformed = CaptureState::from_json(&serde_json::json!({
            "state": "started", "structure": {"complete": "yes"}
        }))
        .unwrap();
        let CaptureState::Started(malformed) = malformed else {
            unreachable!()
        };
        assert!(!malformed.omission_only());
    }

    use super::*;

    #[test]
    fn scope_metadata_roundtrips_without_changing_legacy_full_json() {
        let legacy = CaptureState::Started(CaptureStatus::default());
        assert!(legacy.to_json().get("scope").is_none());
        assert_eq!(
            CaptureState::from_json(&legacy.to_json()),
            Some(legacy.clone())
        );
        let mut json = legacy.to_json();
        json["scope"] = serde_json::json!("current_action_ancestors");
        json["structure_wall_time_us"] = serde_json::json!(0);
        let scoped = CaptureState::from_json(&json).unwrap();
        assert_eq!(scoped.scope(), Some(CaptureScope::CurrentActionAncestors));
        assert_eq!(scoped.to_json(), json);
        assert_eq!(
            scoped.is_complete(),
            Some(false),
            "scope is not a completeness claim"
        );
        json["scope"] = serde_json::json!("future_scope");
        let CaptureState::Started(unknown) = CaptureState::from_json(&json).unwrap() else {
            unreachable!()
        };
        assert!(!unknown.is_complete() && !unknown.omission_only());
    }

    #[test]
    fn finality_and_declared_counts_are_required_not_inferred() {
        let counts = CaptureCounts {
            actions_recorded: Some(1),
            tset_def_events: Some(2),
        };
        let mut s = CaptureStatus {
            footer_present: true,
            finality: Finality::Final,
            declared: counts.clone(),
            observed: counts,
            ..Default::default()
        };
        assert!(s.is_complete());
        s.finality = Finality::Unknown;
        assert!(!s.is_complete());
        assert!(
            s.problems()
                .iter()
                .any(|p| p.contains("does not declare finality"))
        );
        s.finality = Finality::Final;
        s.declared.tset_def_events = Some(3);
        assert!(!s.is_complete());
        assert!(
            s.omission_only(),
            "fewer events than declared is an omission"
        );
        s.declared.tset_def_events = Some(1);
        assert!(!s.is_complete());
        assert!(
            !s.omission_only(),
            "more events than declared is contradictory"
        );
        s.declared.tset_def_events = None;
        assert!(
            !s.is_complete(),
            "an absent declaration is never taken as satisfied"
        );
    }

    #[test]
    fn capture_state_round_trips_through_json_and_old_objects_default_to_unknown() {
        let started = CaptureState::Started(CaptureStatus {
            footer_present: true,
            finality: Finality::Final,
            declared: CaptureCounts {
                actions_recorded: Some(5),
                tset_def_events: Some(3),
            },
            observed: CaptureCounts {
                actions_recorded: Some(5),
                tset_def_events: Some(3),
            },
            decode_error: None,
            records_read: None,
            command_result_seen: true,
            command_end_seen: true,
            ..Default::default()
        });
        assert_eq!(
            CaptureState::from_json(&started.to_json()),
            Some(started.clone())
        );
        assert_eq!(started.is_complete(), Some(true));
        assert!(started.problems().is_empty());
        let off = CaptureState::NotEnabled;
        assert_eq!(CaptureState::from_json(&off.to_json()), Some(off.clone()));
        assert_eq!(off.is_complete(), None);

        // an object missing everything but the state
        let minimal = CaptureState::from_json(&serde_json::json!({"state": "started"})).unwrap();
        let CaptureState::Started(s) = &minimal else {
            panic!("started");
        };
        assert_eq!(s.finality, Finality::Unknown);
        assert_eq!(s.declared, CaptureCounts::default());
        assert!(!s.is_complete(), "an absent declaration is not satisfied");
        assert!(CaptureState::from_json(&serde_json::json!({"x": 1})).is_none());

        // a declared count the reader did not see
        let CaptureState::Started(mut s) = started else {
            panic!("started");
        };
        s.observed.actions_recorded = Some(4);
        assert!(!s.is_complete());
        assert_eq!(
            s.problems(),
            ["the dependency capture declares 5 action(s), 4 were recorded"]
        );
    }

    #[test]
    fn outcome_reads_null_exit_as_unknown_and_never_success() {
        use serde_json::json;
        assert_eq!(
            Outcome::from_run_end_args(&json!({"exit": 0})),
            Outcome::Success
        );
        assert_eq!(
            Outcome::from_run_end_args(&json!({"exit": 2})),
            Outcome::Failed
        );
        assert_eq!(
            Outcome::from_run_end_args(&json!({"result": 1, "exit": 0})),
            Outcome::Failed
        );
        assert_eq!(
            Outcome::from_run_end_args(&json!({"exit": null})),
            Outcome::Unknown
        );
        assert_eq!(Outcome::from_run_end_args(&json!({})), Outcome::Unknown);
        assert_eq!(
            Outcome::from_run_end_args(&json!({"exit": 0, "outcome": "unknown"})),
            Outcome::Unknown,
            "the outcome string wins over a numeric exit"
        );
        assert_eq!(
            Outcome::from_run_end_args(&json!({"exit": null, "outcome": "failed"})),
            Outcome::Failed
        );
    }

    #[test]
    fn a_concurrent_command_leaves_the_capture_a_lower_bound() {
        let counts = CaptureCounts {
            actions_recorded: Some(1),
            tset_def_events: Some(0),
        };
        let mut s = CaptureStatus {
            footer_present: true,
            finality: Finality::Final,
            declared: counts.clone(),
            observed: counts,
            ..Default::default()
        };
        assert!(s.is_complete());
        s.concurrent_commands = vec!["other-trace".to_owned()];
        assert!(!s.is_complete());
        assert!(
            s.omission_only(),
            "the other command's actions are only missing, so paths are lower bounds"
        );
        assert_eq!(
            s.problems(),
            [
                "the command ran alongside 1 other command(s) (other-trace): actions they ran \
                 for it are recorded as reused and take no time here"
            ]
        );
        assert_eq!(CaptureStatus::from_json(&s.to_json()), s, "round-trips");
    }

    #[test]
    fn missing_dependency_footer_does_not_claim_the_command_was_truncated() {
        let status = CaptureStatus {
            command_result_seen: true,
            command_end_seen: true,
            ..Default::default()
        };
        assert!(!status.is_complete());
        assert_eq!(
            status.problems(),
            ["no dependency-capture footer: graph finality is unverified"]
        );
    }
}

#[derive(Debug)]
pub struct Trace {
    pub path: String,
    /// The opened generation the offsets address. Never reopen `path`
    /// for a lazy read: another ingest may have replaced that name.
    pub(crate) source: TraceSource,
    /// Grouped by lane, timestamp-ascending within each lane. `Lane::start..end`,
    /// culling and `selected_summary`'s lane count rely on this order; do not reorder.
    pub acts: Vec<Act>,
    pub names: Vec<String>,
    pub lanes: Vec<Lane>,
    pub span: i64,
    pub t0_abs: i64, // original min ts (us since epoch) before rebase
    /// Sum of action durations in microseconds. Parsing rejects totals above
    /// `i64::MAX / 1000`; nonnegative durations bound every derived duration
    /// total (class time and DAG paths/work) by this value. Code that changes
    /// action durations after parsing must preserve the bound.
    pub busy: i64,
    pub n_failed: usize,
    pub n_hits: usize,
    pub class_count: [usize; CLASS_NAMES.len()],
    pub class_time: [i64; CLASS_NAMES.len()],
    pub runs: Vec<RunInfo>,
    pub health: Health,
    pub parse_ms: u128,
    pub index_ms: u128,
    /// distinct `args.dir` values
    pub dirs: Vec<String>,
    /// distinct `args.why` values
    pub whys: Vec<String>,
    /// distinct `args.run` values
    pub run_ids: Vec<String>,
    /// row bands; the daemon band plus, when present, cache/phase rows
    pub bands: Vec<Band>,
}

impl Trace {
    /// Unlabelled legacy actions belong to a run only when it is unambiguous.
    /// Keep file-order event indices intact; filter the selected view, not its join keys.
    pub fn action_in_run(&self, action: &Act, run: Option<&str>) -> bool {
        let Some(run) = run else { return true };
        if action.run != NO_STR {
            return self
                .run_ids
                .get(action.run as usize)
                .is_some_and(|id| id == run);
        }
        self.runs.iter().all(|r| r.run == run) && self.run_ids.iter().all(|id| id == run)
    }

    pub fn run_origin_us(&self, run: Option<&str>) -> i64 {
        self.runs
            .iter()
            .find(|r| run.is_none_or(|id| r.run == id))
            .map(|r| r.ts)
            .unwrap_or(self.t0_abs)
    }

    pub fn selected_summary(&self, run: Option<&str>) -> SelectedSummary {
        let mut summary = SelectedSummary::default();
        let mut last_lane = None;
        let mut first = None::<i64>;
        let mut end = None::<i64>;
        for action in &self.acts {
            if action.flags & F_NOT_RECIPE != 0 || !self.action_in_run(action, run) {
                continue;
            }
            summary.actions += 1;
            summary.cache_hits += usize::from(action.flags & F_CACHE_HIT != 0);
            summary.failed += usize::from(action.flags & F_FAILED != 0);
            if last_lane != Some(action.lane) {
                summary.lanes += 1;
                last_lane = Some(action.lane);
            }
            first = Some(first.map_or(action.ts, |ts| ts.min(action.ts)));
            let action_end = action.ts.saturating_add(action.dur);
            end = Some(end.map_or(action_end, |ts| ts.max(action_end)));
        }
        summary.span_us = first.zip(end).map_or(0, |(a, b)| b - a);
        summary
    }

    /// A separate positional cursor over the parsed generation. A lazy
    /// consumer must also validate after reading, before publishing data.
    pub fn source_reader(&self) -> anyhow::Result<TraceReader<'_>> {
        self.validate_source()?;
        Ok(TraceReader {
            source: &self.source,
            pos: 0,
        })
    }

    pub fn validate_source(&self) -> anyhow::Result<()> {
        self.source.validate().map_err(|e| {
            anyhow::anyhow!(
                "{}: {e}; reopen the trace before reading action details",
                self.path
            )
        })
    }

    /// The target patterns of the named run — what the build was called
    /// on the command line. Empty when the capture records none and the
    /// argv recovery found nothing.
    pub fn run_targets(&self, run_id: Option<&str>) -> &[String] {
        let r = match run_id {
            Some(id) => self.runs.iter().find(|r| r.run == id),
            None => self.runs.first(),
        };
        r.and_then(|r| r.targets.as_deref()).unwrap_or(&[])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectedSummary {
    pub actions: usize,
    pub failed: usize,
    pub cache_hits: usize,
    pub lanes: usize,
    pub span_us: i64,
}

/// A conservative generation fingerprint: in-place rewrites invalidate
/// all offsets, even if they happen to retain the file's length.
#[derive(Debug, PartialEq, Eq)]
struct SourceStamp {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    change: (u64, u64, i64, i64),
}

impl SourceStamp {
    fn read(file: &File) -> io::Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            len: m.len(),
            modified: m.modified()?,
            #[cfg(unix)]
            change: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
        })
    }
}

#[derive(Debug)]
pub(crate) struct TraceSource {
    file: File,
    stamp: SourceStamp,
}

impl TraceSource {
    pub(crate) fn new(file: File) -> io::Result<Self> {
        let stamp = SourceStamp::read(&file)?;
        Ok(Self { file, stamp })
    }

    pub(crate) fn reader(&self) -> TraceReader<'_> {
        TraceReader {
            source: self,
            pos: 0,
        }
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        if SourceStamp::read(&self.file)? != self.stamp {
            return Err(io::Error::other(
                "the parsed trace file changed; cached byte offsets are no longer valid",
            ));
        }
        Ok(())
    }
}

/// Unlike `File::try_clone`, this reader does not share a seek position
/// with concurrent detail/analysis readers.
pub struct TraceReader<'a> {
    source: &'a TraceSource,
    pos: u64,
}

impl Read for TraceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.source.validate()?;
        #[cfg(unix)]
        let n = self.source.file.read_at(buf, self.pos)?;
        #[cfg(windows)]
        let n = self.source.file.seek_read(buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for TraceReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.source.stamp.len) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
        };
        self.pos = u64::try_from(next)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid trace seek"))?;
        Ok(self.pos)
    }
}

pub const CLASS_NAMES: [&str; 7] = [
    "object",
    "library",
    "binary",
    "generated",
    "script",
    "other",
    "cache",
];

pub fn fmt_us(us: f64) -> String {
    if us >= 1e6 {
        format!("{:.2}s", us / 1e6)
    } else if us >= 1e3 {
        format!("{:.1}ms", us / 1e3)
    } else {
        format!("{us:.0}us")
    }
}
