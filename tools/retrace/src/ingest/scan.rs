/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! One pass over a buck2 event-log stream, keeping exactly what the
//! trace/graph emitters need.  This is the native-decode port of the
//! Python converter's `parse_log` (make-rust `trace-buck.py`); the
//! behavioral spec it preserves is archived at
//! `trace-runs/ingest-2026-09-05/recon/spec-converter.md`.
//!
//! Version contract: the action-graph encoding is declared twice —
//! `ActionGraphStart.schema` as soon as the build command starts
//! and `ActionGraphFooter.schema` in the end-of-build footer, so
//! even a log truncated short of the footer names its encoding.  This
//! reader speaks schema 1: integer-id tset projections, and reused
//! structure recorded only where an action that ran lies below it. The
//! rest of the reused graph is named without its inputs, which makes the
//! capture scoped to what the actions that ran depend on. It refuses any
//! other declared schema by name — never a shape heuristic — and checks
//! that start and footer declarations agree.  A log with neither
//! declaration was written without dep capture; absence downgrades
//! completeness (dangling refs stop being proof of corruption) but is not
//! a refusal.

use std::collections::HashMap;
use std::collections::HashSet;

use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::stream_value::StreamValue;
use futures::StreamExt as _;

use super::compact::InstantKind;
use super::compact::instant_kind;
use super::normalized;
use crate::types::CaptureCounts;
use crate::types::CaptureScope;
use crate::types::CaptureState;
use crate::types::CaptureStatus;
use crate::types::Finality;

/// The one action-graph encoding this reader speaks; compare against
/// `ActionGraphStart.schema` / `ActionGraphFooter.schema`.
pub const SUPPORTED_CAPTURE_SCHEMA: u32 = 1;

fn schema_mismatch(declared: u32) -> anyhow::Error {
    anyhow::anyhow!(
        "this log declares action-graph schema {declared} and this retrace reads schema \
         {SUPPORTED_CAPTURE_SCHEMA}; use a retrace built alongside the buck2 that wrote the log"
    )
}

fn check_capture_schema(schema: u32) -> anyhow::Result<()> {
    if schema != SUPPORTED_CAPTURE_SCHEMA {
        return Err(schema_mismatch(schema));
    }
    Ok(())
}

/// Microseconds since the epoch from a protobuf timestamp (nanos
/// floor-divided, matching the converter's `us()`).  Saturating: a
/// hostile seconds value clamps instead of panicking mid-decode.
pub fn ts_us(t: &::prost_types::Timestamp) -> i64 {
    t.seconds
        .saturating_mul(1_000_000)
        .saturating_add(i64::from(t.nanos).div_euclid(1_000))
}

/// Microseconds from a protobuf duration, same rounding and the same
/// saturation.
pub fn dur_us(d: &::prost_types::Duration) -> i64 {
    d.seconds
        .saturating_mul(1_000_000)
        .saturating_add(i64::from(d.nanos).div_euclid(1_000))
}

/// One command attempt, projected to the fields the emitter's
/// attempts table reads.
#[derive(allocative::Allocative)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct CmdRec {
    pub status: &'static str,
    pub wall_time_us: Option<i64>,
    pub start_us: Option<i64>,
    /// the start of the span `wall_time_us` measures, on the buck2
    /// host's clock; absent on logs from a producer that did not
    /// record it
    pub client_start_us: Option<i64>,
    pub elastic: bool,
}

/// The LAST command's detail fields — the only attempt whose details
/// the emitter reads.
#[derive(Default, allocative::Allocative)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct LastCmd {
    pub signed_exit_code: Option<i32>,
    pub exec_us: Option<i64>,
    /// Positive evidence that this result's execution metadata belongs
    /// to a reused execution, not this invocation. RE-side cache serves
    /// and deduplication on the execute path are not identified by the
    /// current command payload, so false does not prove fresh execution.
    pub reused: bool,
    /// For remote commands, this is first enqueue -> final worker start,
    /// including earlier attempts/backoff. It is NOT client QUEUED time.
    pub queue_us: Option<i64>,
    pub input_fetch_us: Option<i64>,
    pub upload_us: Option<i64>,
    pub worker_wall_us: Option<i64>,
    /// Bytes the RE worker downloaded for inputs and uploaded for outputs.
    pub worker_bytes_in: Option<u64>,
    pub worker_bytes_out: Option<u64>,
    /// How many times RE attempted the command, when more than once, and how long the final
    /// attempt queued.
    pub attempts: Option<u32>,
    pub final_queue_us: Option<u64>,
    pub hashing_us: Option<i64>,
    /// The same final command report identifies local output hashing.
    pub hashing_local_outputs: bool,
    pub mem_peak: Option<u64>,
    pub cpu_us: Option<u64>,
    pub cpu_user_us: Option<u64>,
    pub cpu_sys_us: Option<u64>,
    pub elastic: bool,
    pub remote: bool,
    pub remote_queue_us: Option<i64>,
    /// The selected report's argv (or the historical remote digest marker).
    /// Missing/omitted local argv may be recovered only from unambiguous
    /// same-digest Local.Execute observations, never the first arbitrary stage.
    pub argv: Option<Vec<String>>,
    pub local_digest: Option<String>,
    /// A local command's working directory when it is not the project root, and how buck2 built
    /// its environment (`LocalCommand.environment_rule`; zero when no rule applied).
    pub cwd: Option<String>,
    pub env_rule: i32,
}

/// One finished ActionExecution span, PROJECTED at scan time to
/// exactly what the emitters read.  Holding the intact prost payloads
/// instead cost ~8x the wire size in allocator shape alone — 11.2 GiB
/// on a 767k-action capture whose start+end payloads total 1.4 GB
/// (coord L-122 §B.3).
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct ActionRec {
    pub sid: u64,
    pub ts: i64,
    pub dur: i64,
    /// `pkg:tname (what)`, computed once here.  The short display name
    /// `tname (what)` is this same string from `short_at` on, so a
    /// record never holds two copies of it.
    pub full_target: String,
    pub short_at: u32,
    /// Indices into [`Scan::cfg_names`] / [`Scan::category_names`].
    pub cfg: u32,
    pub category: u32,
    /// 128-bit digest of the prost-encoded ActionKey, the graph join key
    pub key_hash: Option<u128>,
    pub kind: i32,
    pub failed: bool,
    pub error_kind: &'static str,
    pub execution_kind: i32,
    /// Raw UploadResult enums. Zero is unspecified; future values must survive.
    pub cache_upload_result: i32,
    pub dep_file_cache_upload_result: i32,
    pub wall_time_us: Option<i64>,
    /// For `run` actions: time spent preparing the command, before the cache check, and finishing
    /// the action, after the result arrived.
    pub prepare_us: Option<u64>,
    pub finish_us: Option<u64>,
    pub output_size: u64,
    pub input_files_bytes: Option<u64>,
    pub invalidated: bool,
    pub cmds: Vec<CmdRec>,
    pub last: LastCmd,
    /// first non-empty action digest across the commands
    pub digest: Option<String>,
    /// Small reference to the on-disk per-action execution evidence manifest.
    pub execution: Option<crate::execution::DetailRef>,
    pub dep_span_ids: Vec<u64>,
    /// Reused producers, by key digest. The log refers to them by per-command id, which this
    /// holds until `resolve_reused_ids` swaps in the digest of the key their definition names.
    pub dep_action_keys: Vec<u128>,
    pub dep_tset_ids: Vec<u64>,
}

impl ActionRec {
    /// `tname (what)` — the row's display name before deduplication.
    pub fn short(&self) -> &str {
        &self.full_target[self.short_at as usize..]
    }
}

/// A stage span attributed to an enclosing action: `ExecutorStage`
/// (named "Kind.Sub" like `Re.Queue`), `ReUpload`, or `ReSubmit`.
pub struct StageRec {
    pub name: &'static str,
    pub ts: i64,
    pub dur: i64,
    /// A recorded SpanCancelled closes an observed interval, not a normal
    /// stage completion. In particular it supplies no upload/submit totals.
    pub cancelled: bool,
    /// The start/end kinds make these payloads mutually exclusive. Plain
    /// stages allocate nothing; decoded timing boxes move here without cloning.
    pub details: Option<StageDetails>,
    /// Contradictory/wrong-stage timing payloads are not plausible measurements.
    pub cache_timings_invalid: bool,
}

pub enum StageDetails {
    Upload(Box<buck2_data::ReUploadEnd>),
    Submit(Box<buck2_data::ReSubmitEnd>),
    LocalCommand(Box<LocalCommandRec>),
    CacheQuery(Box<buck2_data::CacheQueryTimings>),
    CacheHit(Box<buck2_data::CacheHitTimings>),
    /// The common in-memory/miss result needs no extra allocation.
    LocalCacheOutcome(i32),
    LocalCacheTimings(Box<buck2_data::MatchDepFilesEnd>),
}

impl StageRec {
    pub fn local_command(&self) -> Option<&LocalCommandRec> {
        match self.details.as_ref()? {
            StageDetails::LocalCommand(command) => Some(command),
            _ => None,
        }
    }
}

/// Keep optional persisted-store diagnostics only when they were actually recorded.
fn local_cache_details(end: buck2_data::MatchDepFilesEnd) -> StageDetails {
    if end.persisted_probe_us.is_some()
        || end.persisted_fetch_us.is_some()
        || end.persisted_fetches.is_some()
    {
        StageDetails::LocalCacheTimings(Box::new(end))
    } else {
        StageDetails::LocalCacheOutcome(end.outcome)
    }
}

pub(super) fn is_local_cache_stage(name: &str) -> bool {
    matches!(
        name,
        "LocalCache.CheckIdenticalAction" | "LocalCache.CheckFilteredInputs"
    )
}

fn dep_file_stage_name(start: buck2_data::MatchDepFilesStart) -> &'static str {
    match (start.remote_cache, start.checking_filtered_inputs) {
        (false, false) => "LocalCache.CheckIdenticalAction",
        (false, true) => "LocalCache.CheckFilteredInputs",
        (true, false) => "RemoteCache.CheckIdenticalAction",
        (true, true) => "RemoteCache.CheckFilteredInputs",
    }
}

fn executor_details(
    name: &str,
    end: buck2_data::ExecutorStageEnd,
    local_command: Option<Box<LocalCommandRec>>,
) -> (Option<StageDetails>, bool) {
    let invalid = (end.cache_query.is_some() && name != "CacheQuery")
        || (end.cache_hit.is_some() && name != "CacheHit");
    if invalid {
        // Preserve independent local argv, but reject contradictory timing
        // messages instead of picking one or attributing it to another stage.
        return (local_command.map(StageDetails::LocalCommand), true);
    }
    let details = match name {
        "CacheQuery" => end
            .cache_query
            .map(|timings| StageDetails::CacheQuery(Box::new(timings))),
        "CacheHit" => end
            .cache_hit
            .map(|timings| StageDetails::CacheHit(Box::new(timings))),
        _ => local_command.map(StageDetails::LocalCommand),
    };
    (details, false)
}

pub struct LocalCommandRec {
    pub argv: Vec<String>,
    pub digest: Option<String>,
}

/// A build-phase span (`--phases` output): Load, LoadPackage,
/// Analysis, Materialization.
pub struct PhaseRec {
    pub kind: &'static str,
    pub ts: i64,
    pub dur: i64,
    pub detail: String,
    /// The rule that produced an `Analysis` span; empty for every
    /// other phase kind, which carry no rule.
    pub rule: String,
}

/// The open half of a `PhaseRec`, held from a phase span's start until
/// its end supplies the duration.
struct PhaseStart {
    ts: i64,
    kind: &'static str,
    detail: String,
    rule: String,
}

/// One `Snapshot` instant, reduced to the counters the trace carries.
pub struct SnapRec {
    pub ts: i64,
    pub heap: u64,
    pub rss_max: u64,
    pub dl_bytes: u64,
    pub ul_bytes: u64,
    pub cpu_us: u64,
}

/// One transitive-set projection definition, unioned across split
/// chunks and DICE re-emissions carrying the same id.
#[derive(Default)]
pub struct TsetDef {
    /// The wire `display_key` is not kept in any form: it is a
    /// truncated rendering that distinct projections can share
    /// (`data.proto`: "Display only, never a join key"), so nothing
    /// downstream may derive identity from it, and on a mobile capture
    /// the strings run to kilobytes apiece.  The integer id is the
    /// identity, in the graph as on the wire.
    ///
    /// Reused producers by key digest, held as their per-command ids
    /// until `resolve_reused_ids`, as in [`ActionRec::dep_action_keys`].
    pub own_keys: Vec<u128>,
    pub own_span_ids: Vec<u64>,
    pub child_ids: Vec<u64>,
    /// Nothing that ran lies below this set, so the recorder didn't record its contents.
    pub contents_omitted: bool,
}

/// Producer watcher counts and completeness, separate from the bounded path
/// sample. Counts describe events, not distinct paths or proof of invalidation.
#[derive(Default, serde::Serialize)]
pub struct FileWatcherSummary {
    pub updates: u64,
    pub events_total: Option<u64>,
    pub events_processed: Option<u64>,
    pub fresh_instance: bool,
    pub incomplete_events_reasons: Vec<String>,
    pub reasons_truncated: bool,
    pub paths_truncated: bool,
}

impl FileWatcherSummary {
    fn record(&mut self, stats: &buck2_data::FileWatcherStats) {
        if self.updates == 0 {
            self.events_total = Some(stats.events_total);
            self.events_processed = Some(stats.events_processed);
        } else {
            self.events_total = self
                .events_total
                .and_then(|n| n.checked_add(stats.events_total));
            self.events_processed = self
                .events_processed
                .and_then(|n| n.checked_add(stats.events_processed));
        }
        self.updates = self.updates.saturating_add(1);
        self.fresh_instance |= stats.fresh_instance || stats.fresh_instance_data.is_some();
        if let Some(reason) = &stats.incomplete_events_reason {
            // Retain the producer's explanation without making a command-level
            // summary grow with the log. Truncation remains explicit.
            let end = reason.floor_char_boundary(reason.len().min(512));
            self.reasons_truncated |= reason.len() > end;
            let reason = &reason[..end];
            if !self.incomplete_events_reasons.iter().any(|s| s == reason) {
                if self.incomplete_events_reasons.len() < 8 {
                    self.incomplete_events_reasons.push(reason.to_owned());
                } else {
                    self.reasons_truncated = true;
                }
            }
        }
    }
}

/// Everything one scan of the log yields.
pub struct Scan {
    pub trace_id: String,
    pub start_time_us: i64,
    pub working_dir: String,
    pub argv: Vec<String>,
    pub actions: Vec<ActionRec>,
    /// The distinct values of the two strings every action repeats;
    /// [`ActionRec::cfg`] and [`ActionRec::category`] index into these.
    /// A real capture has tens of each against millions of actions.
    pub cfg_names: Vec<String>,
    pub category_names: Vec<String>,
    pub stages: HashMap<u64, Vec<StageRec>>,
    pub phases: Vec<PhaseRec>,
    pub snapshots: Vec<SnapRec>,
    pub tset_defs: HashMap<u64, TsetDef>,
    pub structures: HashMap<u128, StructureDef>,
    /// Explicit identity evidence, flattened after scanning. Never guessed
    /// from display names, owners, or opaque index suffixes.
    pub aliases: HashMap<u128, AliasDef>,
    /// The key digest of each reused-action id, from the structures and
    /// aliases that define them.
    pub reused_keys: HashMap<u64, u128>,
    /// Aliases as carried, by key digest, until `resolve_reused_ids`
    /// resolves what they stand for into `aliases`.
    pub pending_aliases: Vec<(u128, buck2_data::action_graph_alias::Canonical)>,
    /// Trace ids of other commands the daemon ran while this one ran.
    pub concurrent_commands: Vec<String>,
    pub structure_def_events_seen: u64,
    pub alias_events_seen: u64,
    pub capture_schema: Option<u32>,
    pub capture_scope: CaptureScope,
    pub re_session: Option<String>,
    pub ncpu: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub file_changes: Vec<String>,
    pub file_watcher: FileWatcherSummary,
    /// (ts, is_success) of the Command span end, if the log has one.
    pub cmd_end: Option<(i64, bool)>,
    pub command_end_outcome: Option<bool>,
    pub integrity: crate::types::CaptureIntegrity,
    pub actions_cancelled: u64,
    pub ambiguous_spans: HashSet<u64>,
    /// From the client Result record's BuildResponse, if present.
    pub build_errors: Option<usize>,
    pub build_targets: Vec<String>,
    /// The target patterns exactly as they were typed on the command
    /// line, from `CommandEnd`. `build_targets` is the *resolved* set
    /// (one entry per built target, often thousands); this is the one
    /// or two words a human would use to name the build.
    pub target_patterns: Vec<String>,
    pub project_root: Option<String>,
    /// Hostname of the machine the buck2 daemon ran on, harvested from
    /// the first locally-executed action.  Only LOCAL and LOCAL_WORKER
    /// kinds are trusted: the proto also sets `hostname` on any *failed*
    /// action regardless of execution kind, and on a failed remote
    /// action that names the RE worker, not this machine.
    pub run_host: Option<String>,
    /// Critical-path chain as ActionExecution span-id candidates, in
    /// build order (each entry keeps every span id it named).
    pub critical_path_span_ids: Vec<Vec<u64>>,
    pub footer: Option<buck2_data::ActionGraphFooter>,
    /// Why the scan covers less than the whole build, when it does.
    /// The graph emitter uses this the way the converter used its
    /// dropped-line count: dangling definitions on a complete log
    /// prove corruption; on an incomplete one they are expected.
    pub incomplete: Option<Incomplete>,
    /// Every ActionExecution end the stream carried, orphans included:
    /// the recorder's `actions_recorded` counts each end that reached
    /// the stream.
    pub action_ends_seen: u64,
    /// TransitiveSetProjectionDef events as carried, before the by-id
    /// merge into `tset_defs`.
    pub tset_def_events_seen: u64,
    /// The client `Result` record was seen.
    pub command_result_seen: bool,
    /// The log carried a dependency capture: a start stamp, a footer, a
    /// definition event, or an action end with dependency ids. A build
    /// run without the recorder has none, which is not a gap in one.
    pub capture_started: bool,
    /// Bounded command-level observations; never contributes action DAG edges.
    pub command_profile: crate::command_profile::CommandProfile,
}

pub struct StructureDef {
    pub deps: TsetDef,
    /// Nothing that ran lies below this reused action, so the recorder named it without its
    /// inputs. It is a zero-duration leaf here.
    pub inputs_omitted: bool,
}

pub struct AliasDef {
    pub canonical: u128,
}

impl Scan {
    fn capture_declaration(&mut self, schema: u32) -> anyhow::Result<()> {
        check_capture_schema(schema)?;
        match self.capture_schema {
            Some(previous) if previous != schema => self.integrity.conflicting_capture_schemas += 1,
            Some(_) => {}
            None => self.capture_schema = Some(schema),
        }
        Ok(())
    }

    /// The other commands this one ran alongside, sorted, each once.
    fn other_commands(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .concurrent_commands
            .iter()
            .filter(|id| !id.eq_ignore_ascii_case(&self.trace_id))
            .cloned()
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn structure_capture(&self) -> Option<crate::types::StructureCapture> {
        use crate::types::StructureCapture;
        use crate::types::StructureCounts;
        if self.capture_schema != Some(SUPPORTED_CAPTURE_SCHEMA)
            && self.structure_def_events_seen == 0
            && self.alias_events_seen == 0
        {
            return None;
        }
        let footer = self
            .footer
            .as_ref()
            .filter(|f| f.schema == SUPPORTED_CAPTURE_SCHEMA);
        // The footer marks a walk that could not look something up; it doesn't count the
        // failures. Every reused producer an action that ran refers to is walked, so there are no
        // unexpanded roots, and each definition comes from a lookup, so none is unproven.
        let walk_finished = footer.map(|f| {
            f.incomplete != Some(buck2_data::action_graph_footer::Incomplete::UnfinishedWalk as i32)
        });
        Some(StructureCapture {
            complete: walk_finished,
            lookup_failures: walk_finished.and_then(|finished| finished.then_some(0)),
            unexpanded_reused_roots: footer.map(|_| 0),
            declared: StructureCounts {
                definitions: footer.map(|f| f.structure_defs),
                definition_events: footer.map(|f| f.structure_defs),
                alias_events: footer.map(|f| f.aliases),
                unproven: footer.map(|_| 0),
            },
            observed: StructureCounts {
                definitions: Some(self.structures.len() as u64),
                definition_events: Some(self.structure_def_events_seen),
                alias_events: Some(self.alias_events_seen),
                unproven: Some(0),
            },
            malformed: false,
        })
    }
    /// What the log declares about its dependency capture against what
    /// this scan saw.
    pub fn capture_state(&self) -> CaptureState {
        if !self.capture_started {
            return CaptureState::NotEnabled;
        }
        // The footer declares the action ends and set-definition events it wrote, and whether
        // the recorder vouches for the graph.
        let (declared, finality) = match &self.footer {
            Some(f) => (
                CaptureCounts {
                    actions_recorded: Some(f.action_ends),
                    tset_def_events: Some(f.tset_def_events),
                },
                if f.incomplete.is_none() {
                    Finality::Final
                } else {
                    Finality::Nonfinal
                },
            ),
            None => (CaptureCounts::default(), Finality::Unknown),
        };
        let (decode_error, records_read) = match &self.incomplete {
            Some(Incomplete::Undecodable { records, error }) => {
                (Some(error.clone()), Some(*records))
            }
            _ => (None, None),
        };
        CaptureState::Started(CaptureStatus {
            schema: self.capture_schema,
            scope: self.capture_scope,
            structure_wall_time_us: self.footer.as_ref().map(|f| f.definitions_walk_us),
            footer_present: self.footer.is_some(),
            finality,
            declared,
            observed: CaptureCounts {
                actions_recorded: Some(self.action_ends_seen),
                tset_def_events: Some(self.tset_def_events_seen),
            },
            decode_error,
            records_read,
            command_result_seen: self.command_result_seen,
            command_end_seen: self.cmd_end.is_some(),
            integrity: self.integrity.clone(),
            actions_cancelled: self.actions_cancelled,
            concurrent_commands: self.other_commands(),
            structure: self.structure_capture(),
        })
    }
}

/// Why a scan stopped short. A malformed frame can be corruption, an
/// unsupported encoding, or a writer interrupted inside a record; its cause
/// cannot always be established from the readable prefix.
#[derive(Clone, Debug)]
pub enum Incomplete {
    /// The stream decoded to its end and carried no client `Result`
    /// record: the build was cancelled, or is still running.
    NoResult,
    /// A frame failed to decode.  Whatever follows it is unreachable,
    /// so the scan holds only the records that came before.
    Undecodable { records: u64, error: String },
}

const FILE_CHANGES_CAP: usize = 10_000;
const PARENT_HOP_CAP: usize = 64;

// Bound the dense u64 payload, not total index memory: sparse entries, allocator
// bookkeeping and transient storage during growth are additional allocations.
const DENSE_PARENT_MAX_ENTRIES: usize = (256 * 1024 * 1024) / std::mem::size_of::<u64>();
const DENSE_PARENT_MAX_GAP: usize = 4_096;

/// Both an absent link and an explicit root link stop attribution, so zero
/// can represent either in the dense array. Other parent values keep all 64 bits.
/// Large or sparse IDs use the map; dense storage is only an indexing shortcut.
struct DenseParentIndex {
    base: Option<u64>,
    dense: Vec<u64>,
    sparse: HashMap<u64, u64>,
    max_entries: usize,
    max_gap: usize,
}

impl DenseParentIndex {
    fn new(max_entries: usize, max_gap: usize) -> Self {
        assert!(max_entries <= DENSE_PARENT_MAX_ENTRIES);
        Self {
            base: None,
            dense: Vec::new(),
            sparse: HashMap::new(),
            max_entries,
            max_gap,
        }
    }

    fn insert(&mut self, sid: u64, parent: u64) {
        let base = *self.base.get_or_insert(sid);
        // The u64 ID is untrusted. Only a representable, nearby, budgeted
        // position may grow the dense storage; all others remain sparse.
        if let Some(index) = sid.checked_sub(base).and_then(|n| usize::try_from(n).ok())
            && index < self.max_entries
            && index.saturating_sub(self.dense.len()) <= self.max_gap
            && let Some(required_len) = index.checked_add(1)
        {
            if required_len > self.dense.capacity() {
                let capacity = required_len
                    .max(self.dense.capacity().saturating_mul(2).max(4_096))
                    .min(self.max_entries);
                // Reserve before resize, with a hard ceiling on the requested
                // payload. Allocation failure may use the sparse path instead.
                if self
                    .dense
                    .try_reserve_exact(capacity - self.dense.len())
                    .is_err()
                {
                    self.sparse.insert(sid, parent);
                    return;
                }
            }
            if required_len > self.dense.len() {
                self.dense.resize(required_len, 0);
            }
            self.dense[index] = parent;
            // A previously sparse ID may now fit. In particular a duplicate
            // root (parent=0) must not resurrect the old sparse nonzero value.
            self.sparse.remove(&sid);
            return;
        }
        self.sparse.insert(sid, parent);
    }

    fn get(&self, sid: u64) -> u64 {
        if let Some(index) = self
            .base
            .and_then(|base| sid.checked_sub(base))
            .and_then(|n| usize::try_from(n).ok())
            && let Some(&parent) = self.dense.get(index)
            && parent != 0
        {
            return parent;
        }
        // Growth may cover an earlier sparse entry with a zero-filled hole.
        self.sparse.get(&sid).copied().unwrap_or(0)
    }
}

/// Decode and reduce the whole log.  Async because the decoder is;
/// the caller wraps it in a current-thread runtime.
pub async fn scan(log: &EventLogPathBuf) -> anyhow::Result<Scan> {
    scan_with_phases(log, true).await
}

/// Serial readers yield one normalized record without allocating a batch.
/// Parallel decoding delivers ordered owned batches; the reducer consumes or
/// drops each batch before asking for another.
enum ScanInput {
    Stream(futures::stream::BoxStream<'static, super::decode_pool::LogItem<normalized::Record>>),
    Pool(super::decode_pool::Pool<normalized::Record>),
    /// The old raw decoder is retained only as a parity oracle.
    #[cfg(test)]
    RawPool(super::decode_pool::Pool<StreamValue>),
}

impl ScanInput {
    async fn next_batch(&mut self) -> Option<ScanBatch> {
        match self {
            Self::Stream(stream) => stream.next().await.map(|item| ScanBatch::One(Some(item))),
            Self::Pool(pool) => pool.next_batch().map(ScanBatch::Many),
            #[cfg(test)]
            Self::RawPool(pool) => pool.next_batch().map(ScanBatch::RawMany),
        }
    }
}

enum ScanBatch {
    One(Option<super::decode_pool::LogItem<normalized::Record>>),
    Many(std::vec::IntoIter<super::decode_pool::LogItem<normalized::Record>>),
    #[cfg(test)]
    RawMany(std::vec::IntoIter<super::decode_pool::LogItem<StreamValue>>),
}

impl Iterator for ScanBatch {
    type Item = super::decode_pool::LogItem<normalized::Record>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::One(item) => item.take(),
            Self::Many(items) => items.next(),
            #[cfg(test)]
            Self::RawMany(items) => items.next().map(|item| item.map(normalized::Record::Raw)),
        }
    }
}

/// Decode the whole log, optionally retaining the display-only build phases.
/// Disabling their projection does not skip decoding or parent links: executor
/// stages may be nested below a phase and still belong to an enclosing action.
pub async fn scan_with_phases(log: &EventLogPathBuf, phases: bool) -> anyhow::Result<Scan> {
    scan_with_workers(log, phases, crate::parallelism::decode_workers()).await
}

/// Spill command/output evidence during ordered reduction. The writer's I/O
/// failures are fatal; unlike an undecodable input frame they must not publish
/// an apparently successful partial conversion.
pub async fn scan_with_details(
    log: &EventLogPathBuf,
    phases: bool,
    details: &mut crate::execution::DetailWriter,
) -> anyhow::Result<Scan> {
    scan_with_workers_and_details(
        log,
        phases,
        crate::parallelism::decode_workers(),
        Some(details),
    )
    .await
}

pub async fn scan_with_details_cancellable(
    log: &EventLogPathBuf,
    phases: bool,
    details: &mut crate::execution::DetailWriter,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Scan> {
    scan_with_workers_and_details_cancellable(
        log,
        phases,
        crate::parallelism::decode_workers(),
        Some(details),
        cancel,
    )
    .await
}

async fn scan_with_workers(
    log: &EventLogPathBuf,
    phases: bool,
    workers: usize,
) -> anyhow::Result<Scan> {
    scan_with_workers_and_details(log, phases, workers, None).await
}

async fn scan_with_workers_and_details(
    log: &EventLogPathBuf,
    phases: bool,
    workers: usize,
    details: Option<&mut crate::execution::DetailWriter>,
) -> anyhow::Result<Scan> {
    scan_with_workers_and_details_cancellable(log, phases, workers, details, None).await
}

async fn scan_with_workers_and_details_cancellable(
    log: &EventLogPathBuf,
    phases: bool,
    workers: usize,
    details: Option<&mut crate::execution::DetailWriter>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Scan> {
    if workers > 0 && log.is_protobuf() {
        let (invocation, frames) = log
            .unpack_protobuf_frames()
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))?;
        let pool = super::decode_pool::Pool::new_normalized(
            frames,
            super::decode_pool::Config {
                expected_run: invocation.trace_id.to_string().as_bytes().try_into().ok(),
                workers,
                phases,
                ..Default::default()
            },
        )?;
        return scan_input_with_details_cancellable(
            invocation,
            ScanInput::Pool(pool),
            phases,
            details,
            cancel,
        )
        .await;
    }
    // Low-core machines and JSON encodings keep the ordinary serial decoder.
    // They use the same post-validation compaction/projection as the workers,
    // not a second action model. The shared reader still owns truncation policy.
    let (invocation, stream) = log
        .unpack_stream()
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let expected_run: Option<[u8; 36]> = invocation.trace_id.to_string().as_bytes().try_into().ok();
    let stream = stream
        .map(move |item| {
            item.map(|value| {
                normalized::project(super::compact::clean_for_run(
                    value,
                    phases,
                    expected_run.as_ref(),
                ))
            })
        })
        .boxed();
    scan_input_with_details_cancellable(
        invocation,
        ScanInput::Stream(stream),
        phases,
        details,
        cancel,
    )
    .await
}

enum LiveActionStart {
    #[cfg(test)]
    Raw(i64, Box<buck2_data::ActionExecutionStart>),
    Normalized(Box<normalized::Start>),
}

fn commit_normalized(
    mut start: normalized::Start,
    end: normalized::End,
    interners: &mut Interners,
) -> anyhow::Result<ActionRec> {
    // Reject an unindexable name only once its action is paired, before
    // mutating the interners. Orphan starts do not become action records.
    anyhow::ensure!(
        start.short_at.is_some(),
        "package path for span {} exceeds the 32-bit short-name index limit",
        start.sid
    );
    let cfg = interners.cfg.id(std::mem::take(&mut start.cfg));
    let category = interners.category.id(std::mem::take(&mut start.category));
    Ok(end.into_action(start, cfg, category))
}

fn write_action_details(
    end: &mut normalized::End,
    observations: Vec<crate::execution::DetailRef>,
    writer: &mut crate::execution::DetailWriter,
) -> anyhow::Result<crate::execution::DetailRef> {
    use crate::execution::ActionManifest;
    use crate::execution::Record;

    let mut reports = Vec::with_capacity(end.reports.len());
    for (index, pending) in end.reports.iter_mut().enumerate() {
        pending.report.index = u32::try_from(index)
            .map_err(|_| anyhow::anyhow!("too many command reports in action {}", end.sid))?;
        if let Some(text) = pending.stdout.take() {
            pending.report.stdout.content = Some(writer.append_text(&text)?);
        }
        if let Some(text) = pending.stderr.take() {
            pending.report.stderr.content = Some(writer.append_text(&text)?);
        }
        // Borrow the owned record for serialization; move it back so the
        // selected compatibility argv can move out after this write, without
        // copying command strings into a second in-memory representation.
        let record = Record::Attempt(std::mem::take(&mut pending.report));
        reports.push(writer.append(&record)?);
        let Record::Attempt(report) = record else {
            unreachable!()
        };
        pending.report = report;
    }
    writer.append(&Record::Action(ActionManifest {
        span_id: end.sid,
        reports,
        observations,
    }))
}

#[cfg(test)]
async fn scan_input(
    invocation: buck2_event_log::utils::Invocation,
    input: ScanInput,
    phases: bool,
) -> anyhow::Result<Scan> {
    scan_input_with_details(invocation, input, phases, None).await
}

#[cfg(test)]
async fn scan_input_with_details(
    invocation: buck2_event_log::utils::Invocation,
    input: ScanInput,
    phases: bool,
    details: Option<&mut crate::execution::DetailWriter>,
) -> anyhow::Result<Scan> {
    scan_input_with_details_cancellable(invocation, input, phases, details, None).await
}

async fn scan_input_with_details_cancellable(
    invocation: buck2_event_log::utils::Invocation,
    mut input: ScanInput,
    phases: bool,
    mut details: Option<&mut crate::execution::DetailWriter>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Scan> {
    let profile_origin = invocation
        .start_time
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_micros()).ok());
    let start_time_us = profile_origin.unwrap_or(0);
    let mut command_profile =
        super::profile::Collector::new(profile_origin, invocation.trace_id.to_string());
    let argv = if invocation.expanded_command_line_args.is_empty() {
        invocation.command_line_args.clone()
    } else {
        invocation.expanded_command_line_args.clone()
    };

    let mut out = Scan {
        trace_id: invocation.trace_id.to_string(),
        start_time_us,
        working_dir: invocation.working_dir.clone(),
        argv,
        actions: Vec::new(),
        cfg_names: Vec::new(),
        category_names: Vec::new(),
        stages: HashMap::new(),
        phases: Vec::new(),
        snapshots: Vec::new(),
        tset_defs: HashMap::new(),
        structures: HashMap::new(),
        aliases: HashMap::new(),
        reused_keys: HashMap::new(),
        pending_aliases: Vec::new(),
        concurrent_commands: Vec::new(),
        structure_def_events_seen: 0,
        alias_events_seen: 0,
        capture_schema: None,
        capture_scope: CaptureScope::Full,
        re_session: None,
        ncpu: None,
        mem_bytes: None,
        file_changes: Vec::new(),
        file_watcher: FileWatcherSummary::default(),
        cmd_end: None,
        command_end_outcome: None,
        integrity: Default::default(),
        actions_cancelled: 0,
        ambiguous_spans: HashSet::new(),
        build_errors: None,
        build_targets: Vec::new(),
        target_patterns: Vec::new(),
        project_root: None,
        run_host: None,
        critical_path_span_ids: Vec::new(),
        footer: None,
        incomplete: None,
        action_ends_seen: 0,
        tset_def_events_seen: 0,
        command_result_seen: false,
        capture_started: false,
        command_profile: Default::default(),
    };

    // Span bookkeeping, exactly the converter's: every SpanStart's
    // parent is recorded so a stage can walk to its enclosing action;
    // starts of interesting spans keep (ts, payload) until their end.
    let mut interners = Interners::default();
    let mut parent = DenseParentIndex::new(DENSE_PARENT_MAX_ENTRIES, DENSE_PARENT_MAX_GAP);
    let mut act_start: HashMap<u64, LiveActionStart> = HashMap::new();
    let mut stage_start: HashMap<u64, (i64, StageStartKind)> = HashMap::new();
    let mut phase_start: HashMap<u64, PhaseStart> = HashMap::new();
    let mut command_observations: HashMap<u64, Vec<crate::execution::DetailRef>> = HashMap::new();
    let mut records: u64 = 0;

    'scan: while let Some(batch) = input.next_batch().await {
        super::check_cancel(cancel)?;
        for item in batch {
            let value = match item {
                Ok(v) => {
                    records += 1;
                    v
                }
                Err(e) => {
                    if e.has_tag(buck2_error::ErrorTag::Tier0) {
                        return Err(anyhow::anyhow!("internal ingestion failure: {e:#}"));
                    }
                    // Nothing after an undecodable frame is trustworthy, so
                    // the scan stops here.  The error and the record count
                    // travel out with it because they are the only evidence
                    // that separates a file this decoder cannot read from a
                    // build that simply has not finished.
                    out.incomplete = Some(Incomplete::Undecodable {
                        records,
                        error: format!("{e:#}"),
                    });
                    break 'scan;
                }
            };
            let value = match value {
                normalized::Record::Raw(value) => value,
                normalized::Record::Start(start) => {
                    command_profile.action_start(start.sid);
                    parent.insert(start.sid, start.parent);
                    command_observations.remove(&start.sid);
                    let sid = start.sid;
                    if act_start
                        .insert(sid, LiveActionStart::Normalized(start))
                        .is_some()
                    {
                        out.ambiguous_spans.insert(sid);
                    }
                    continue;
                }
                normalized::Record::End(mut end) => {
                    // This end consumes its SID even if it is the wrong kind
                    // for an open stage. Keep ancestry; reject only stale pairing.
                    stage_start.remove(&end.sid);
                    out.action_ends_seen += 1;
                    out.capture_started |= end.has_deps();
                    if out.run_host.is_none()
                        && ran_on_this_host(end.execution_kind)
                        && let Some(host) = end.hostname.as_deref().filter(|h| !h.is_empty())
                    {
                        out.run_host = Some(host.to_owned());
                    }
                    if let Some(start) = act_start.remove(&end.sid) {
                        let start = match start {
                            LiveActionStart::Normalized(start) => *start,
                            #[cfg(test)]
                            LiveActionStart::Raw(ts, start) => {
                                normalized::project_start(end.sid, 0, ts, *start)
                            }
                        };
                        let observations =
                            command_observations.remove(&end.sid).unwrap_or_default();
                        if start.key_hash.is_some()
                            && end.key_hash.is_some()
                            && start.key_hash != end.key_hash
                        {
                            out.ambiguous_spans.insert(end.sid);
                        }
                        let execution = details
                            .as_deref_mut()
                            .map(|writer| write_action_details(&mut end, observations, writer))
                            .transpose()?;
                        let mut action = commit_normalized(start, *end, &mut interners)?;
                        action.execution = execution;
                        out.actions.push(action);
                    } else {
                        out.integrity.orphan_action_ends += 1;
                    }
                    continue;
                }
            };
            command_profile.result(&value);
            let mut event = match value {
                StreamValue::Event(e) => e,
                StreamValue::Result(result) => {
                    out.command_result_seen = true;
                    if let Some(buck2_cli_proto::command_result::Result::BuildResponse(br)) =
                        result.result
                    {
                        out.build_errors = Some(br.errors.len());
                        out.build_targets =
                            br.build_targets.into_iter().map(|t| t.target).collect();
                        if !br.project_root.is_empty() {
                            out.project_root = Some(br.project_root);
                        }
                    }
                    continue;
                }
                StreamValue::PartialResult(..) => continue,
            };

            if !event.trace_id.is_empty() && !event.trace_id.eq_ignore_ascii_case(&out.trace_id) {
                out.integrity.foreign_event_ids += 1;
                continue;
            }
            command_profile.event(&event);
            // Capture identity is independent of timestamps. In particular a
            // missing timestamp must not hide an unsupported schema.
            if let Some(buck2_data::buck_event::Data::Instant(i)) = &mut event.data {
                match instant_kind(i.data.as_ref()) {
                    InstantKind::ActionGraphStart => {
                        let Some(buck2_data::instant_event::Data::ActionGraphStart(s)) =
                            &mut i.data
                        else {
                            unreachable!("instant classifier preserves the payload kind");
                        };
                        out.capture_declaration(s.schema)?;
                        out.capture_started = true;
                        continue;
                    }
                    InstantKind::ActionGraphFooter => {
                        let Some(buck2_data::instant_event::Data::ActionGraphFooter(s)) =
                            &mut i.data
                        else {
                            unreachable!("instant classifier preserves the payload kind");
                        };
                        out.capture_declaration(s.schema)?;
                        out.capture_started = true;
                        match &out.footer {
                            Some(first) if first != &*s => out.integrity.conflicting_footers += 1,
                            Some(_) => {}
                            None => out.footer = Some(std::mem::take(s)),
                        }
                        continue;
                    }
                    InstantKind::TransitiveSetProjectionDef => {
                        let Some(buck2_data::instant_event::Data::TransitiveSetProjectionDef(d)) =
                            &mut i.data
                        else {
                            unreachable!("instant classifier preserves the payload kind");
                        };
                        out.tset_def_events_seen += 1;
                        out.capture_started = true;
                        let def = out.tset_defs.entry(d.tset_id).or_default();
                        def.contents_omitted |= d.contents_omitted;
                        if let Some(p) = &mut d.producers {
                            def.own_keys
                                .extend(p.reused_ids.iter().map(|&id| u128::from(id)));
                            def.own_span_ids.append(&mut p.span_ids);
                            def.child_ids.append(&mut p.tset_ids);
                        }
                        continue;
                    }
                    InstantKind::ActionGraphStructureDef => {
                        let Some(buck2_data::instant_event::Data::ActionGraphStructureDef(d)) =
                            &mut i.data
                        else {
                            unreachable!("instant classifier preserves payload kind")
                        };
                        out.capture_started = true;
                        out.structure_def_events_seen += 1;
                        let Some(key) = &d.key else {
                            out.integrity.structural_conflicts += 1;
                            continue;
                        };
                        let key = key_hash(key);
                        if out
                            .reused_keys
                            .insert(d.reused_id, key)
                            .is_some_and(|k| k != key)
                        {
                            out.integrity.structural_conflicts += 1;
                        }
                        let def = out.structures.entry(key).or_insert_with(|| StructureDef {
                            deps: TsetDef::default(),
                            inputs_omitted: d.inputs_omitted,
                        });
                        if def.inputs_omitted != d.inputs_omitted {
                            out.integrity.structural_conflicts += 1;
                        }
                        if let Some(p) = &mut d.producers {
                            def.deps
                                .own_keys
                                .extend(p.reused_ids.iter().map(|&id| u128::from(id)));
                            def.deps.own_span_ids.append(&mut p.span_ids);
                            def.deps.child_ids.append(&mut p.tset_ids);
                        }
                        continue;
                    }
                    InstantKind::ActionGraphAlias => {
                        let Some(buck2_data::instant_event::Data::ActionGraphAlias(d)) =
                            &mut i.data
                        else {
                            unreachable!("instant classifier preserves payload kind")
                        };
                        out.capture_started = true;
                        out.alias_events_seen += 1;
                        let (Some(key), Some(canonical)) = (&d.key, d.canonical) else {
                            out.integrity.structural_conflicts += 1;
                            continue;
                        };
                        let key = key_hash(key);
                        if out.reused_keys.insert(d.id, key).is_some_and(|k| k != key) {
                            out.integrity.structural_conflicts += 1;
                        }
                        out.pending_aliases.push((key, canonical));
                        continue;
                    }
                    InstantKind::ConcurrentCommands => {
                        let Some(buck2_data::instant_event::Data::ConcurrentCommands(c)) =
                            &mut i.data
                        else {
                            unreachable!("instant classifier preserves payload kind")
                        };
                        out.concurrent_commands.append(&mut c.trace_ids);
                        continue;
                    }
                    InstantKind::ReSession
                    | InstantKind::SystemInfo
                    | InstantKind::Snapshot
                    | InstantKind::BuildGraphInfo
                    | InstantKind::InstallFinished
                    | InstantKind::Ignored => {}
                }
            }

            let ts = match &event.timestamp {
                Some(t) => ts_us(t),
                None => {
                    if matches!(&event.data, Some(buck2_data::buck_event::Data::SpanEnd(_))) {
                        stage_start.remove(&event.span_id);
                    }
                    continue;
                }
            };
            let sid = event.span_id;
            match event.data {
                Some(buck2_data::buck_event::Data::SpanStart(s)) => {
                    parent.insert(sid, event.parent_id);
                    match s.data {
                        #[cfg(test)]
                        Some(buck2_data::span_start_event::Data::ActionExecution(a)) => {
                            command_observations.remove(&sid);
                            if act_start
                                .insert(sid, LiveActionStart::Raw(ts, Box::new(a)))
                                .is_some()
                            {
                                out.ambiguous_spans.insert(sid);
                            }
                        }
                        Some(buck2_data::span_start_event::Data::ExecutorStage(st)) => {
                            let owner = enclosing_action(&parent, &act_start, sid);
                            let writer = owner.and(details.as_deref_mut());
                            let (kind, reference) =
                                StageStartKind::executor_observed(st, sid, ts, writer)?;
                            if let (Some(owner), Some(reference)) = (owner, reference) {
                                command_observations
                                    .entry(owner)
                                    .or_default()
                                    .push(reference);
                            }
                            stage_start.insert(sid, (ts, kind));
                        }
                        Some(buck2_data::span_start_event::Data::MatchDepFiles(start)) => {
                            stage_start.insert(
                                sid,
                                (
                                    ts,
                                    StageStartKind::MatchDepFiles {
                                        name: dep_file_stage_name(start),
                                    },
                                ),
                            );
                        }
                        Some(buck2_data::span_start_event::Data::ReUpload(..)) => {
                            stage_start.insert(sid, (ts, StageStartKind::ReUpload));
                        }
                        Some(buck2_data::span_start_event::Data::ReSubmit(..)) => {
                            stage_start.insert(sid, (ts, StageStartKind::ReSubmit));
                        }
                        Some(buck2_data::span_start_event::Data::Load(l)) if phases => {
                            phase_start.insert(sid, PhaseStart::new(ts, "Load", l.module_id));
                        }
                        Some(buck2_data::span_start_event::Data::LoadPackage(l)) if phases => {
                            phase_start.insert(sid, PhaseStart::new(ts, "LoadPackage", l.path));
                        }
                        Some(buck2_data::span_start_event::Data::Analysis(a)) if phases => {
                            phase_start.insert(
                                sid,
                                PhaseStart {
                                    rule: a.rule,
                                    ..PhaseStart::new(ts, "Analysis", analysis_detail(a.target))
                                },
                            );
                        }
                        Some(buck2_data::span_start_event::Data::Materialization(m)) if phases => {
                            phase_start.insert(
                                sid,
                                PhaseStart::new(
                                    ts,
                                    "Materialization",
                                    m.action_digest.unwrap_or_default(),
                                ),
                            );
                        }
                        _ => {}
                    }
                }
                Some(buck2_data::buck_event::Data::SpanEnd(e)) => {
                    // Any end consumes the open stage once. Wrong/unknown
                    // kinds must not leave a start for a later false match.
                    let stage = stage_start.remove(&sid);
                    let recorded_dur = e.duration.as_ref().map(dur_us);
                    #[cfg(test)]
                    let dur = recorded_dur.filter(|d| *d >= 0).unwrap_or(0);
                    match e.data {
                        #[cfg(test)]
                        Some(buck2_data::span_end_event::Data::ActionExecution(end)) => {
                            out.action_ends_seen += 1;
                            if end.graph_producers.is_some() {
                                out.capture_started = true;
                            }
                            if out.run_host.is_none()
                                && ran_on_this_host(end.execution_kind)
                                && let Some(h) = end.hostname.as_deref().filter(|h| !h.is_empty())
                            {
                                out.run_host = Some(h.to_owned());
                            }
                            // An end without a start is a torn log's
                            // orphan; drop it like the converter does.
                            if let Some(start) = act_start.remove(&sid) {
                                let start_key = match &start {
                                    LiveActionStart::Raw(_, start) => {
                                        start.key.as_ref().map(key_hash)
                                    }
                                    LiveActionStart::Normalized(start) => start.key_hash,
                                };
                                let end_key = end.key.as_ref().map(key_hash);
                                if start_key.is_some() && end_key.is_some() && start_key != end_key
                                {
                                    out.ambiguous_spans.insert(sid);
                                }
                                let action = if let Some(writer) = details.as_deref_mut() {
                                    let start = match start {
                                        LiveActionStart::Raw(ts, start) => {
                                            normalized::project_start(sid, 0, ts, *start)
                                        }
                                        LiveActionStart::Normalized(start) => *start,
                                    };
                                    let mut end = normalized::project_end(sid, dur, end);
                                    let reference = write_action_details(
                                        &mut end,
                                        command_observations.remove(&sid).unwrap_or_default(),
                                        writer,
                                    )?;
                                    let mut action = commit_normalized(start, end, &mut interners)?;
                                    action.execution = Some(reference);
                                    action
                                } else {
                                    match start {
                                        LiveActionStart::Raw(start_ts, start) => project_action(
                                            sid,
                                            start_ts,
                                            dur,
                                            &start,
                                            &end,
                                            &mut interners,
                                        ),
                                        LiveActionStart::Normalized(start) => commit_normalized(
                                            *start,
                                            normalized::project_end(sid, dur, end),
                                            &mut interners,
                                        )?,
                                    }
                                };
                                out.actions.push(action);
                            } else {
                                out.integrity.orphan_action_ends += 1;
                            }
                        }
                        Some(buck2_data::span_end_event::Data::Command(c)) => {
                            out.command_end_outcome = command_end_outcome(&c);
                            out.cmd_end = Some((ts, out.command_end_outcome == Some(true)));
                            out.target_patterns = command_end_patterns(&c);
                        }
                        Some(buck2_data::span_end_event::Data::FileWatcher(f)) => {
                            if let Some(stats) = f.stats {
                                out.file_watcher.record(&stats);
                                out.file_watcher.paths_truncated |= stats.events.len()
                                    > FILE_CHANGES_CAP.saturating_sub(out.file_changes.len());
                                for ev in stats.events {
                                    if out.file_changes.len() >= FILE_CHANGES_CAP {
                                        break;
                                    }
                                    out.file_changes.push(ev.path);
                                }
                            }
                        }
                        Some(buck2_data::span_end_event::Data::ReUpload(ru)) => {
                            if let Some((start_ts, StageStartKind::ReUpload)) = stage {
                                let dur =
                                    observed_duration(start_ts, ts, recorded_dur).unwrap_or(-1);
                                attribute_stage(
                                    &mut out.stages,
                                    &parent,
                                    &act_start,
                                    sid,
                                    StageRec {
                                        name: "ReUpload",
                                        ts: start_ts,
                                        dur,
                                        cancelled: false,
                                        details: Some(StageDetails::Upload(Box::new(ru))),
                                        cache_timings_invalid: false,
                                    },
                                );
                            }
                        }
                        Some(buck2_data::span_end_event::Data::ReSubmit(rs)) => {
                            if let Some((start_ts, StageStartKind::ReSubmit)) = stage {
                                let dur =
                                    observed_duration(start_ts, ts, recorded_dur).unwrap_or(-1);
                                attribute_stage(
                                    &mut out.stages,
                                    &parent,
                                    &act_start,
                                    sid,
                                    StageRec {
                                        name: "ReSubmit",
                                        ts: start_ts,
                                        dur,
                                        cancelled: false,
                                        details: Some(StageDetails::Submit(Box::new(rs))),
                                        cache_timings_invalid: false,
                                    },
                                );
                            }
                        }
                        Some(buck2_data::span_end_event::Data::MatchDepFiles(end)) => {
                            if let Some((start_ts, StageStartKind::MatchDepFiles { name })) = stage
                            {
                                let dur =
                                    observed_duration(start_ts, ts, recorded_dur).unwrap_or(-1);
                                attribute_stage(
                                    &mut out.stages,
                                    &parent,
                                    &act_start,
                                    sid,
                                    StageRec {
                                        name,
                                        ts: start_ts,
                                        dur,
                                        cancelled: false,
                                        details: is_local_cache_stage(name)
                                            .then(|| local_cache_details(end)),
                                        cache_timings_invalid: false,
                                    },
                                );
                            }
                        }
                        Some(buck2_data::span_end_event::Data::ExecutorStage(end)) => {
                            if let Some((
                                start_ts,
                                StageStartKind::Executor {
                                    name,
                                    local_command,
                                },
                            )) = stage
                            {
                                let (details, cache_timings_invalid) =
                                    executor_details(name, end, local_command);
                                // Keep an invalid marker for activity.rejected;
                                // negative durations never contribute totals or
                                // become positioned intervals in the artifact.
                                let dur =
                                    observed_duration(start_ts, ts, recorded_dur).unwrap_or(-1);
                                attribute_stage(
                                    &mut out.stages,
                                    &parent,
                                    &act_start,
                                    sid,
                                    StageRec {
                                        name,
                                        ts: start_ts,
                                        dur,
                                        cancelled: false,
                                        details,
                                        cache_timings_invalid,
                                    },
                                );
                            }
                        }
                        Some(buck2_data::span_end_event::Data::SpanCancelled(..)) => {
                            if let Some((start_ts, kind)) = stage
                                && let Some(rec) = cancelled_stage(start_ts, ts, recorded_dur, kind)
                            {
                                attribute_stage(&mut out.stages, &parent, &act_start, sid, rec);
                            }
                            if act_start.remove(&sid).is_some() {
                                command_observations.remove(&sid);
                                out.actions_cancelled += 1;
                            }
                            // Preserve the existing optional phase projection;
                            // executor-stage cancellation is independent of it.
                            if phases
                                && let Some(p) = phase_start.remove(&sid)
                                && let Some(dur) = observed_duration(p.ts, ts, recorded_dur)
                            {
                                out.phases.push(PhaseRec {
                                    kind: p.kind,
                                    ts: p.ts,
                                    dur,
                                    detail: p.detail,
                                    rule: p.rule,
                                });
                            }
                        }
                        _ => {
                            if phases
                                && let Some(p) = phase_start.remove(&sid)
                                && let Some(dur) = observed_duration(p.ts, ts, recorded_dur)
                            {
                                out.phases.push(PhaseRec {
                                    kind: p.kind,
                                    ts: p.ts,
                                    dur,
                                    detail: p.detail,
                                    rule: p.rule,
                                });
                            }
                        }
                    }
                }
                Some(buck2_data::buck_event::Data::Instant(i)) => {
                    match instant_kind(i.data.as_ref()) {
                        InstantKind::ReSession => {
                            let Some(buck2_data::instant_event::Data::ReSession(s)) = i.data else {
                                unreachable!("instant classifier preserves the payload kind");
                            };
                            out.re_session = Some(s.session_id);
                        }
                        InstantKind::SystemInfo => {
                            let Some(buck2_data::instant_event::Data::SystemInfo(s)) = i.data
                            else {
                                unreachable!("instant classifier preserves the payload kind");
                            };
                            if s.num_cores.unwrap_or(0) > 0 {
                                out.ncpu = s.num_cores;
                                out.mem_bytes = s.system_total_memory_bytes;
                            }
                        }
                        InstantKind::Snapshot => {
                            let Some(buck2_data::instant_event::Data::Snapshot(s)) = i.data else {
                                unreachable!("instant classifier preserves the payload kind");
                            };
                            let heap = s.malloc_bytes_active.unwrap_or(0);
                            out.snapshots.push(SnapRec {
                                ts,
                                heap: if heap != 0 {
                                    heap
                                } else {
                                    s.buck2_rss.unwrap_or(0)
                                },
                                rss_max: s.buck2_max_rss,
                                dl_bytes: s.re_download_bytes,
                                ul_bytes: s.re_upload_bytes,
                                cpu_us: s.buck2_user_cpu_us.saturating_add(s.buck2_system_cpu_us),
                            });
                        }
                        InstantKind::BuildGraphInfo => {
                            let Some(buck2_data::instant_event::Data::BuildGraphInfo(b)) = i.data
                            else {
                                unreachable!("instant classifier preserves the payload kind");
                            };
                            out.critical_path_span_ids = b
                                .critical_path2
                                .iter()
                                .filter(|e| {
                                    matches!(
                                e.entry,
                                Some(buck2_data::critical_path_entry2::Entry::ActionExecution(..))
                            )
                                })
                                .map(|e| e.span_ids.clone())
                                .collect();
                        }
                        // Capture kinds were handled before timestamp validation;
                        // installation evidence belongs to the command profile.
                        InstantKind::ActionGraphStart
                        | InstantKind::ActionGraphFooter
                        | InstantKind::TransitiveSetProjectionDef
                        | InstantKind::ActionGraphStructureDef
                        | InstantKind::ActionGraphAlias
                        | InstantKind::ConcurrentCommands
                        | InstantKind::InstallFinished
                        | InstantKind::Ignored => {}
                    }
                }
                _ => {}
            }
        }
    }

    drop(input);
    if !out.command_result_seen && out.incomplete.is_none() {
        out.incomplete = Some(Incomplete::NoResult);
    }
    out.command_profile = command_profile.finish(out.incomplete.is_some())?;
    out.cfg_names = interners.cfg.names;
    out.category_names = interners.category.names;

    validate_graph_identity(&mut out);

    Ok(out)
}

fn validate_graph_identity(scan: &mut Scan) {
    let mut producers = HashMap::new();
    let mut keys = HashMap::new();
    for (i, action) in scan.actions.iter().enumerate() {
        if action.sid == 0 {
            scan.integrity.invalid_span_ids += 1;
            scan.ambiguous_spans.insert(0);
        }
        if let Some(previous) = producers.insert(action.sid, i) {
            let first: &ActionRec = &scan.actions[previous];
            if first.key_hash != action.key_hash || first.full_target != action.full_target {
                scan.ambiguous_spans.insert(action.sid);
            }
        }
        if let Some(key) = action.key_hash
            && let Some(previous) = keys.insert(key, action.sid)
            && previous != action.sid
        {
            scan.ambiguous_spans.extend([previous, action.sid]);
        }
    }
    scan.integrity.ambiguous_producer_spans = scan.ambiguous_spans.len() as u64;
    // A stage's ancestry names a span, not an action key. It cannot be
    // attributed to either producer once that span's identity is ambiguous.
    for sid in &scan.ambiguous_spans {
        scan.stages.remove(sid);
    }
    resolve_reused_ids(scan, &producers);
    for key in scan.aliases.keys() {
        if keys.contains_key(key) || scan.structures.contains_key(key) {
            scan.integrity.structural_conflicts += 1;
        }
    }
    // The recorder writes reused structure only where an action that ran lies below it, and
    // nothing at all when no action ran, so such captures are scoped to what the actions that
    // ran depend on. When actions ran and no reused producer was defined, every producer ran,
    // and the graph is the command's whole graph.
    if scan.capture_schema.is_some()
        && (scan.action_ends_seen == 0
            || !scan.structures.is_empty()
            || scan.tset_defs.values().any(|d| d.contents_omitted))
    {
        scan.capture_scope = CaptureScope::CurrentActionAncestors;
    }
    flatten_aliases(&mut scan.aliases, &mut scan.integrity);
    for (&key, def) in &mut scan.structures {
        if keys.contains_key(&key) {
            scan.integrity.structural_conflicts += 1;
        }
        for &sid in &def.deps.own_span_ids {
            if sid == 0 {
                scan.integrity.invalid_span_ids += 1;
            }
            if !producers.contains_key(&sid) {
                scan.integrity.dangling_producer_spans += 1;
            }
        }
        def.deps.own_keys.retain(|dep| {
            let canonical = scan.aliases.get(dep).map_or(*dep, |a| a.canonical);
            if canonical == key {
                scan.integrity.self_references += 1;
                false
            } else {
                true
            }
        });
    }
    for action in &mut scan.actions {
        action.dep_span_ids.retain(|&sid| {
            if sid == action.sid {
                scan.integrity.self_references += 1;
                return false;
            }
            if sid == 0 {
                scan.integrity.invalid_span_ids += 1;
            }
            if !producers.contains_key(&sid) {
                scan.integrity.dangling_producer_spans += 1;
            }
            true
        });
        action.dep_action_keys.retain(|key| {
            if Some(*key) == action.key_hash {
                scan.integrity.self_references += 1;
                false
            } else {
                true
            }
        });
    }
    for (&id, def) in &mut scan.tset_defs {
        for &sid in &def.own_span_ids {
            if sid == 0 {
                scan.integrity.invalid_span_ids += 1;
            }
            if !producers.contains_key(&sid) {
                scan.integrity.dangling_producer_spans += 1;
            }
        }
        def.child_ids.retain(|&child| {
            if child == id {
                scan.integrity.self_references += 1;
                false
            } else {
                true
            }
        });
    }
}

/// Swaps the per-command reused ids the scan held for the key digests of the
/// definitions that name them, and resolves what each alias stands for. An id
/// nothing defines gets a digest that matches no key, so it stays dangling.
fn resolve_reused_ids(scan: &mut Scan, producers: &HashMap<u64, usize>) {
    let reused_keys = std::mem::take(&mut scan.reused_keys);
    let digest = |id: u64| {
        reused_keys
            .get(&id)
            .copied()
            .unwrap_or_else(|| undefined_reused(id))
    };
    for (key, canonical) in std::mem::take(&mut scan.pending_aliases) {
        let canonical = match canonical {
            buck2_data::action_graph_alias::Canonical::SpanId(sid) => {
                match producers.get(&sid).and_then(|&i| scan.actions[i].key_hash) {
                    Some(canonical) => canonical,
                    None => {
                        scan.integrity.dangling_producer_spans += 1;
                        continue;
                    }
                }
            }
            buck2_data::action_graph_alias::Canonical::ReusedId(id) => digest(id),
        };
        let alias = scan.aliases.entry(key).or_insert(AliasDef { canonical });
        if alias.canonical != canonical {
            scan.integrity.structural_conflicts += 1;
        }
    }
    let resolve = |ids: &mut Vec<u128>| {
        for id in ids {
            *id = digest(u64::try_from(*id).expect("the scan holds a u64 id here"));
        }
    };
    for action in &mut scan.actions {
        resolve(&mut action.dep_action_keys);
    }
    for def in scan.tset_defs.values_mut() {
        resolve(&mut def.own_keys);
    }
    for def in scan.structures.values_mut() {
        resolve(&mut def.deps.own_keys);
    }
}

/// The key digest of a reused-action id that nothing defines, distinct from every real key's.
fn undefined_reused(id: u64) -> u128 {
    key_hash(&buck2_data::ActionKey {
        key: format!("undefined reused-action id {id}"),
        ..Default::default()
    })
}

/// Resolve only producer-recorded aliases. Iterative path compression keeps a
/// long alias chain linear and avoids recursion on untrusted event-log data.
/// A cycle and aliases feeding it remain unresolved, never arbitrarily joined.
fn flatten_aliases(
    aliases: &mut HashMap<u128, AliasDef>,
    integrity: &mut crate::types::CaptureIntegrity,
) {
    let mut resolved: HashMap<u128, Option<u128>> = HashMap::new();
    let mut visiting = HashSet::new();
    for &start in aliases.keys() {
        if resolved.contains_key(&start) {
            continue;
        }
        let mut path = Vec::new();
        let mut key = start;
        let canonical = loop {
            if let Some(&resolved) = resolved.get(&key) {
                break resolved;
            }
            let Some(alias) = aliases.get(&key) else {
                break Some(key);
            };
            if !visiting.insert(key) {
                integrity.alias_cycles += 1;
                break None;
            }
            path.push(key);
            key = alias.canonical;
        };
        for key in path {
            visiting.remove(&key);
            resolved.insert(key, canonical);
        }
    }
    aliases.retain(|key, alias| {
        if let Some(Some(canonical)) = resolved.get(key) {
            alias.canonical = *canonical;
            true
        } else {
            false
        }
    });
}

fn command_end_outcome(end: &buck2_data::CommandEnd) -> Option<bool> {
    let completed = end.build_result.as_ref()?.build_completed;
    if !completed {
        return Some(false);
    }
    // A successful Test build says nothing about whether its tests passed.
    matches!(
        end.data,
        Some(buck2_data::command_end::Data::Build(_) | buck2_data::command_end::Data::Install(_))
    )
    .then_some(true)
}

fn observed_duration(start: i64, end: i64, recorded: Option<i64>) -> Option<i64> {
    let elapsed = end.checked_sub(start).filter(|v| *v >= 0)?;
    recorded
        .or(Some(elapsed))
        .filter(|duration| *duration >= 0 && start.checked_add(*duration).is_some())
}

pub(super) fn action_error_kind(end: &buck2_data::ActionExecutionEnd) -> &'static str {
    use buck2_data::action_execution_end::Error;
    match &end.error {
        Some(Error::MissingOutputs(_)) => "missing_outputs",
        Some(Error::CommandExecutionError(_)) => "command_execution_error",
        _ => "unknown",
    }
}

impl PhaseStart {
    fn new(ts: i64, kind: &'static str, detail: String) -> Self {
        Self {
            ts,
            kind,
            detail,
            rule: String::new(),
        }
    }
}

/// Names what an `Analysis` span analysed.  All three target kinds are
/// named, not just the standard one: anon targets and dynamic lambdas
/// are the majority of analysis spans on a large build, so leaving
/// them unnamed makes most of the phase unattributable.
pub(super) fn analysis_detail(target: Option<buck2_data::analysis_start::Target>) -> String {
    use buck2_data::analysis_start::Target;
    match target {
        Some(Target::StandardTarget(c)) => configured_label(&c),
        Some(Target::AnonTarget(a)) => format!("{} (anon)", anon_label(&a)),
        Some(Target::DynamicLambda(d)) => {
            use buck2_data::dynamic_lambda_owner::Owner;
            let owner = match d.owner {
                Some(Owner::TargetLabel(c)) => configured_label(&c),
                Some(Owner::BxlKey(k)) => match k.label {
                    Some(l) => format!("{}:{}", l.bxl_path, l.name),
                    None => String::new(),
                },
                Some(Owner::AnonTarget(a)) => anon_label(&a),
                None => String::new(),
            };
            format!("{owner} (dynamic)")
        }
        None => String::new(),
    }
}

fn configured_label(c: &buck2_data::ConfiguredTargetLabel) -> String {
    match &c.label {
        Some(l) => format!("{}:{}", l.package, l.name),
        None => String::new(),
    }
}

fn anon_label(a: &buck2_data::AnonTarget) -> String {
    match &a.name {
        Some(l) => format!("{}:{}", l.package, l.name),
        None => String::new(),
    }
}

/// (full target, offset of the short name within it, cfg) from the
/// action's owner, exactly the converter's `names_of`: a real
/// `TargetLabel` names the row; every other owner kind renders as
/// `("?", <variant name>, "")`.
#[cfg(test)]
fn names_of(start: &buck2_data::ActionExecutionStart) -> (String, u32, String) {
    let what = match &start.name {
        Some(n) => format!("{} {}", n.category, n.identifier).trim().to_owned(),
        None => String::new(),
    };
    let (pkg, tname, cfg) = target_of(start);
    // The short name is the full target past `pkg:`, so the offset
    // lands on the byte after an ASCII colon whatever `pkg` contains.
    let short_at = u32::try_from(pkg.len() + 1).expect("package paths are far below 4 GiB");
    (format!("{pkg}:{tname} ({what})"), short_at, cfg)
}

/// Ordinals for a string the records repeat: `Scan` keeps one copy of
/// each distinct value and the records keep 4-byte indices.
#[derive(Default)]
struct Interner {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    fn id(&mut self, s: String) -> u32 {
        if let Some(&i) = self.ids.get(&s) {
            return i;
        }
        let i = u32::try_from(self.names.len()).expect("fewer than 4G distinct values");
        self.ids.insert(s.clone(), i);
        self.names.push(s);
        i
    }
}

#[derive(Default)]
struct Interners {
    cfg: Interner,
    category: Interner,
}

#[cfg(test)]
fn target_of(start: &buck2_data::ActionExecutionStart) -> (String, String, String) {
    use buck2_data::action_key::Owner;
    let owner = start.key.as_ref().and_then(|k| k.owner.as_ref());
    match owner {
        Some(Owner::TargetLabel(c)) => {
            let (pkg, name) = match &c.label {
                Some(l) => (l.package.clone(), l.name.clone()),
                None => ("?".to_owned(), "?".to_owned()),
            };
            let cfg = c
                .configuration
                .as_ref()
                .map(|x| x.full_name.clone())
                .unwrap_or_default();
            (pkg, name, cfg)
        }
        Some(Owner::BxlKey(..)) => ("?".to_owned(), "BxlKey".to_owned(), String::new()),
        Some(Owner::TestTargetLabel(..)) => {
            ("?".to_owned(), "TestTargetLabel".to_owned(), String::new())
        }
        Some(Owner::AnonTarget(..)) => ("?".to_owned(), "AnonTarget".to_owned(), String::new()),
        Some(Owner::LocalResourceSetup(..)) => (
            "?".to_owned(),
            "LocalResourceSetup".to_owned(),
            String::new(),
        ),
        None => ("?".to_owned(), "?".to_owned(), String::new()),
    }
}

pub(super) fn status_name(c: &buck2_data::CommandExecution) -> &'static str {
    use buck2_data::command_execution::Status;
    match &c.status {
        Some(Status::Success(..)) => "Success",
        Some(Status::Failure(..)) => "Failure",
        Some(Status::Timeout(..)) => "Timeout",
        Some(Status::Error(..)) => "Error",
        Some(Status::Cancelled(..)) => "Cancelled",
        Some(Status::WorkerFailure(..)) => "WorkerFailure",
        None => "Unknown",
    }
}

/// Whether a remote command's recorded origin is a cache other than buck2's own action cache:
/// the remote dep-file cache, or RE serving its own action cache for a submitted execution.
pub(crate) fn remote_cache_origin(origin: i32) -> bool {
    origin == buck2_data::RemoteCommandOrigin::RemoteDepFileCache as i32
        || origin == buck2_data::RemoteCommandOrigin::ReServedFromCache as i32
}

/// Did an action of this kind execute on the buck2 daemon's own host?
/// `ActionExecutionEnd.hostname` is always set for locally-executed
/// actions, but is *also* set for any failed action whatever its kind —
/// so a failed remote action would otherwise hand us an RE worker's
/// name and we would report the build as having run there.
fn ran_on_this_host(execution_kind: i32) -> bool {
    execution_kind == buck2_data::ActionExecutionKind::Local as i32
        || execution_kind == buck2_data::ActionExecutionKind::LocalWorker as i32
}

/// The one place the prost payloads are read: everything the emitters
/// need is copied out here and the payloads drop with the event.
#[cfg(test)]
fn project_action(
    sid: u64,
    ts: i64,
    dur: i64,
    start: &buck2_data::ActionExecutionStart,
    end: &buck2_data::ActionExecutionEnd,
    interners: &mut Interners,
) -> ActionRec {
    let (full_target, short_at, cfg) = names_of(start);
    let no_producers = buck2_data::ActionGraphProducers::default();
    let producers = end.graph_producers.as_ref().unwrap_or(&no_producers);
    let category = start
        .name
        .as_ref()
        .map(|n| n.category.clone())
        .unwrap_or_default();

    let cmds: Vec<CmdRec> = end
        .commands
        .iter()
        .map(|c| {
            let md = c.details.as_ref().and_then(|d| d.metadata.as_ref());
            CmdRec {
                status: status_name(c),
                wall_time_us: md.and_then(|m| m.wall_time.as_ref()).map(dur_us),
                start_us: md.and_then(|m| m.start_time.as_ref()).map(ts_us),
                client_start_us: md.and_then(|m| m.client_start_time.as_ref()).map(ts_us),
                elastic: c
                    .details
                    .as_ref()
                    .and_then(re_worker)
                    .is_some_and(|w| w.elastic_capacity),
            }
        })
        .collect();

    let mut last = LastCmd::default();
    if let Some(d) = end.commands.last().and_then(|c| c.details.as_ref()) {
        let md = d.metadata.as_ref();
        let stats = md.and_then(|m| m.execution_stats.as_ref());
        last.signed_exit_code = d.signed_exit_code;
        last.exec_us = md.and_then(|m| m.execution_time.as_ref()).map(dur_us);
        last.queue_us = md.and_then(|m| m.queue_duration.as_ref()).map(dur_us);
        last.input_fetch_us = md
            .and_then(|m| m.input_materialization_duration.as_ref())
            .map(dur_us);
        let worker = re_worker(d);
        last.upload_us = worker.and_then(|w| w.output_upload_us).and_then(us_i64);
        last.worker_wall_us = worker.and_then(|w| w.worker_wall_time_us).and_then(us_i64);
        last.worker_bytes_in = worker.and_then(|w| w.input_download_bytes);
        last.worker_bytes_out = worker.and_then(|w| w.output_upload_bytes);
        last.attempts = worker.and_then(|w| w.execution_attempts).filter(|n| *n > 1);
        last.final_queue_us = worker.and_then(|w| w.final_queue_us);
        last.hashing_us = md.and_then(|m| m.hashing_duration.as_ref()).map(dur_us);
        last.mem_peak = stats.and_then(|s| s.memory_peak);
        last.cpu_us = worker.and_then(cpu_total_us);
        last.cpu_user_us = worker.and_then(|w| w.cpu_time_user_us);
        last.cpu_sys_us = worker.and_then(|w| w.cpu_time_system_us);
        last.elastic = worker.is_some_and(|w| w.elastic_capacity);
        if let Some(kind) = d.command_kind.as_ref() {
            use buck2_data::command_execution_kind::Command;
            last.hashing_local_outputs = matches!(
                &kind.command,
                Some(
                    Command::LocalCommand(_)
                        | Command::OmittedLocalCommand(_)
                        | Command::WorkerCommand(_)
                )
            );
            match &kind.command {
                Some(Command::LocalCommand(c)) => {
                    last.local_digest =
                        (!c.action_digest.is_empty()).then(|| c.action_digest.clone());
                    if !c.argv.is_empty() {
                        last.argv = Some(c.argv.clone());
                    }
                    last.cwd = c.working_directory.clone();
                    last.env_rule = c.environment_rule;
                }
                Some(Command::WorkerCommand(c)) if !c.argv.is_empty() => {
                    last.argv = Some(c.argv.clone());
                }
                Some(Command::WorkerInitCommand(c)) if !c.argv.is_empty() => {
                    last.argv = Some(c.argv.clone());
                }
                Some(Command::RemoteCommand(rc)) => {
                    last.remote = true;
                    // ActionCache is enum value zero, so an absent
                    // cache_hit_type in an older log is not evidence of
                    // reuse. The bool and the nondefault cache origins are.
                    last.reused = rc.cache_hit || remote_cache_origin(rc.cache_hit_type);
                    last.remote_queue_us = rc.queue_time.as_ref().map(dur_us);
                    last.argv = Some(vec!["<remote>".to_owned(), rc.action_digest.clone()]);
                }
                Some(Command::OmittedLocalCommand(c)) => {
                    last.local_digest =
                        (!c.action_digest.is_empty()).then(|| c.action_digest.clone());
                }
                _ => {}
            }
        }
    }

    let digest = end.commands.iter().find_map(|c| {
        use buck2_data::command_execution_kind::Command;
        let kind = c.details.as_ref().and_then(|d| d.command_kind.as_ref())?;
        let d = match &kind.command {
            Some(Command::LocalCommand(x)) => &x.action_digest,
            Some(Command::RemoteCommand(x)) => &x.action_digest,
            Some(Command::WorkerCommand(x)) => &x.action_digest,
            Some(Command::OmittedLocalCommand(x)) => &x.action_digest,
            _ => return None,
        };
        (!d.is_empty()).then(|| d.clone())
    });

    ActionRec {
        sid,
        ts,
        dur,
        full_target,
        short_at,
        cfg: interners.cfg.id(cfg),
        category: interners.category.id(category),
        key_hash: start.key.as_ref().map(key_hash),
        kind: start.kind,
        failed: end.failed,
        error_kind: action_error_kind(end),
        execution_kind: end.execution_kind,
        cache_upload_result: end.cache_upload_result,
        dep_file_cache_upload_result: end.dep_file_cache_upload_result,
        wall_time_us: end.wall_time.as_ref().map(dur_us),
        prepare_us: end.prepare_command_us,
        finish_us: end.finish_action_us,
        output_size: end.output_size,
        input_files_bytes: end.input_files_bytes,
        invalidated: end.invalidation_info.is_some(),
        cmds,
        last,
        digest,
        execution: None,
        dep_span_ids: producers.span_ids.clone(),
        dep_action_keys: producers
            .reused_ids
            .iter()
            .map(|&id| u128::from(id))
            .collect(),
        dep_tset_ids: producers.tset_ids.clone(),
    }
}

/// A worker-reported microsecond count as the signed microseconds the records hold.
pub(super) fn us_i64(us: u64) -> Option<i64> {
    i64::try_from(us).ok()
}

/// The command's total CPU time: RE reports user and system time, whose sum is the total.
pub(super) fn cpu_total_us(worker: &buck2_data::ReWorkerDetails) -> Option<u64> {
    match (worker.cpu_time_user_us, worker.cpu_time_system_us) {
        (None, None) => None,
        (user, system) => Some(user.unwrap_or(0).saturating_add(system.unwrap_or(0))),
    }
}

/// What the RE worker reported, for a command RE ran for this build.
pub(super) fn re_worker(
    details: &buck2_data::CommandExecutionDetails,
) -> Option<&buck2_data::ReWorkerDetails> {
    match details.command_kind.as_ref()?.command.as_ref()? {
        buck2_data::command_execution_kind::Command::RemoteCommand(remote) => {
            remote.re_worker.as_deref()
        }
        _ => None,
    }
}

/// Identity of an `ActionKey` for graph joining.  The encoded key is
/// ~190 B and the graph references it millions of times; a 128-bit
/// digest joins identically at 16 B with no heap allocation, and the
/// collision probability over a few million keys is ~1e-25.
pub fn key_hash(k: &buck2_data::ActionKey) -> u128 {
    use prost::Message as _;
    use sha1::Digest as _;
    let d = sha1::Sha1::digest(k.encode_to_vec());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    u128::from_be_bytes(b)
}

enum StageStartKind {
    // An executor start is only read for its fixed stage name and optional
    // Local.Execute argv. Keeping the full protobuf until End needlessly
    // enlarges every live-map bucket and retains digests/platform metadata.
    Executor {
        name: &'static str,
        local_command: Option<Box<LocalCommandRec>>,
    },
    ReUpload,
    ReSubmit,
    MatchDepFiles {
        name: &'static str,
    },
}

impl StageStartKind {
    #[cfg(test)]
    fn executor(st: buck2_data::ExecutorStageStart) -> Self {
        Self::executor_observed(st, 0, 0, None)
            .expect("no writer cannot fail")
            .0
    }

    fn executor_observed(
        st: buck2_data::ExecutorStageStart,
        sid: u64,
        ts: i64,
        writer: Option<&mut crate::execution::DetailWriter>,
    ) -> anyhow::Result<(Self, Option<crate::execution::DetailRef>)> {
        use buck2_data::executor_stage_start::Stage;
        use buck2_data::local_stage::Stage as Local;

        let name = executor_stage_name(&st);
        let command = match st.stage {
            Some(Stage::Local(buck2_data::LocalStage {
                stage: Some(Local::Execute(execute)),
            })) => execute.command.map(|command| {
                (
                    "local",
                    normalized::nonempty(command.action_digest),
                    normalized::logged_command(command.argv, command.env, Vec::new()),
                )
            }),
            Some(Stage::Local(buck2_data::LocalStage {
                stage: Some(Local::WorkerExecute(execute)),
            })) => execute.command.map(|command| {
                (
                    "worker",
                    normalized::nonempty(command.action_digest),
                    normalized::logged_command(command.argv, command.env, command.fallback_exe),
                )
            }),
            Some(Stage::Local(buck2_data::LocalStage {
                stage: Some(Local::WorkerInit(init)),
            })) => init.command.map(|command| {
                (
                    "worker_init",
                    None,
                    normalized::logged_command(command.argv, command.env, Vec::new()),
                )
            }),
            _ => None,
        };
        let mut local_command = None;
        let mut reference = None;
        if let Some((executor, action_digest, command)) = command {
            let record = crate::execution::Record::Command(crate::execution::CommandObservation {
                span_id: sid,
                start_us: ts,
                executor: executor.to_owned(),
                action_digest,
                command,
            });
            if let Some(writer) = writer {
                reference = Some(writer.append(&record)?);
            }
            let crate::execution::Record::Command(observation) = record else {
                unreachable!()
            };
            // Keep the original numerical-stage model: only Local.Execute
            // retains an argv. Other observed commands live solely on disk.
            if executor == "local" {
                local_command = Some(Box::new(LocalCommandRec {
                    argv: observation.command.argv,
                    digest: observation.action_digest,
                }));
            }
        }
        Ok((
            Self::Executor {
                name,
                local_command,
            },
            reference,
        ))
    }
}

/// Only a recorded end closes a cancelled stage. An open start at EOF is
/// never extended to the action/command end. If the duration is missing,
/// two ordered client timestamps still supply an observed interval; a
/// malformed negative duration or reversed endpoints do not imply zero.
fn cancelled_stage(
    start_ts: i64,
    end_ts: i64,
    recorded_dur: Option<i64>,
    kind: StageStartKind,
) -> Option<StageRec> {
    let dur = recorded_dur.or_else(|| end_ts.checked_sub(start_ts))?;
    if dur < 0 || end_ts < start_ts || start_ts.checked_add(dur).is_none() {
        return None;
    }
    let (name, local_command) = match kind {
        StageStartKind::Executor {
            name,
            local_command,
        } => (name, local_command),
        StageStartKind::ReUpload => ("ReUpload", None),
        StageStartKind::ReSubmit => ("ReSubmit", None),
        StageStartKind::MatchDepFiles { name } => (name, None),
    };
    Some(StageRec {
        name,
        ts: start_ts,
        dur,
        cancelled: true,
        details: local_command.map(StageDetails::LocalCommand),
        cache_timings_invalid: false,
    })
}

/// Walk the parent chain (bounded, like the converter) to the nearest
/// enclosing ActionExecution and file the stage under it; a stage
/// with no live action ancestor is dropped.
fn attribute_stage<T>(
    stages: &mut HashMap<u64, Vec<StageRec>>,
    parent: &DenseParentIndex,
    act_start: &HashMap<u64, T>,
    sid: u64,
    rec: StageRec,
) {
    if let Some(action) = enclosing_action(parent, act_start, sid) {
        stages.entry(action).or_default().push(rec);
    }
}

fn enclosing_action<T>(
    parent: &DenseParentIndex,
    act_start: &HashMap<u64, T>,
    sid: u64,
) -> Option<u64> {
    let mut cur = sid;
    for _ in 0..PARENT_HOP_CAP {
        let p = parent.get(cur);
        if p == 0 {
            return None;
        }
        if act_start.contains_key(&p) {
            return Some(p);
        }
        cur = p;
    }
    None
}

/// `"Kind.Sub"` naming, identical to the strings the converter derived
/// from the JSONL oneof tags (`Re.Queue`, `Local.Execute`, ...).
fn executor_stage_name(st: &buck2_data::ExecutorStageStart) -> &'static str {
    use buck2_data::executor_stage_start::Stage;
    use buck2_data::local_stage::Stage as L;
    use buck2_data::re_stage::Stage as R;
    match &st.stage {
        Some(Stage::Re(re)) => match &re.stage {
            Some(R::Execute(..)) => "Re.Execute",
            Some(R::Download(..)) => "Re.Download",
            Some(R::Queue(..)) => "Re.Queue",
            Some(R::WorkerDownload(..)) => "Re.WorkerDownload",
            Some(R::WorkerUpload(..)) => "Re.WorkerUpload",
            Some(R::Unknown(..)) => "Re.Unknown",
            Some(R::MaterializeFailedInputs(..)) => "Re.MaterializeFailedInputs",
            Some(R::BeforeActionExecution(..)) => "Re.BeforeActionExecution",
            Some(R::AfterActionExecution(..)) => "Re.AfterActionExecution",
            Some(R::QueueOverQuota(..)) => "Re.QueueOverQuota",
            Some(R::QueueNoWorkerAvailable(..)) => "Re.QueueNoWorkerAvailable",
            Some(R::QueueAcquiringDependencies(..)) => "Re.QueueAcquiringDependencies",
            Some(R::QueueCancelled(..)) => "Re.QueueCancelled",
            None => "Re",
        },
        Some(Stage::Local(l)) => match &l.stage {
            Some(L::Queued(..)) => "Local.Queued",
            Some(L::Execute(..)) => "Local.Execute",
            Some(L::MaterializeInputs(..)) => "Local.MaterializeInputs",
            Some(L::PrepareOutputs(..)) => "Local.PrepareOutputDirs",
            Some(L::FinalizeOutputs(..)) => "Local.FinalizeOutputs",
            Some(L::AcquireLocalResource(..)) => "Local.AcquireLocalResource",
            Some(L::WorkerInit(..)) => "Local.WorkerInit",
            Some(L::WorkerExecute(..)) => "Local.WorkerExecute",
            Some(L::WorkerQueued(..)) => "Local.WorkerQueued",
            Some(L::WorkerWait(..)) => "Local.WorkerWait",
            None => "Local",
        },
        Some(Stage::CasArtifact(stage)) => {
            match buck2_data::cas_artifact_stage::Phase::try_from(stage.phase) {
                Ok(buck2_data::cas_artifact_stage::Phase::CheckExpiration) => {
                    "CasArtifact.CheckExpiration"
                }
                Ok(buck2_data::cas_artifact_stage::Phase::ExtendExpiration) => {
                    "CasArtifact.ExtendExpiration"
                }
                Ok(buck2_data::cas_artifact_stage::Phase::RecheckExpiration) => {
                    "CasArtifact.RecheckExpiration"
                }
                Ok(buck2_data::cas_artifact_stage::Phase::TreeMetadata) => {
                    "CasArtifact.TreeMetadata"
                }
                Ok(buck2_data::cas_artifact_stage::Phase::DeclareOutputs) => {
                    "CasArtifact.DeclareOutputs"
                }
                _ => "CasArtifact.Unknown",
            }
        }
        Some(Stage::CacheQuery(..)) => "CacheQuery",
        Some(Stage::CacheHit(..)) => "CacheHit",
        Some(Stage::Prepare(..)) => "Prepare",
        Some(Stage::InFlightWait(..)) => "InFlightWait",
        None => "?",
    }
}

/// The unresolved target patterns a `CommandEnd` carries, for the
/// command kinds that build actions. Other kinds (query, audit, clean)
/// have no patterns worth naming a capture after, so they yield none.
fn command_end_patterns(c: &buck2_data::CommandEnd) -> Vec<String> {
    use buck2_data::command_end::Data;
    let pats = match &c.data {
        Some(Data::Build(b)) => &b.unresolved_target_patterns,
        Some(Data::Test(t)) => &t.unresolved_target_patterns,
        Some(Data::Targets(t)) => &t.unresolved_target_patterns,
        Some(Data::Install(i)) => &i.unresolved_target_patterns,
        _ => return Vec::new(),
    };
    pats.iter()
        .map(|p| p.value.clone())
        .filter(|v| !v.is_empty())
        .collect()
}

/// The argv of a `Local.Execute` stage, when this stage is one.
/// Kept as the independent borrowed reference for owned-projection tests.
#[cfg(test)]
fn local_execute_argv(st: &buck2_data::ExecutorStageStart) -> Option<Vec<String>> {
    use buck2_data::executor_stage_start::Stage;
    use buck2_data::local_stage::Stage as L;
    match &st.stage {
        Some(Stage::Local(l)) => match &l.stage {
            Some(L::Execute(e)) => e.command.as_ref().map(|c| c.argv.clone()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod new_stage_tests {
    use super::*;

    #[test]
    fn cas_phases_and_finalize_keep_distinct_names_including_unknown_values() {
        use buck2_data::executor_stage_start::Stage;
        for (phase, name) in [
            (0, "CasArtifact.Unknown"),
            (1, "CasArtifact.CheckExpiration"),
            (2, "CasArtifact.ExtendExpiration"),
            (3, "CasArtifact.RecheckExpiration"),
            (4, "CasArtifact.TreeMetadata"),
            (5, "CasArtifact.DeclareOutputs"),
            (999, "CasArtifact.Unknown"),
        ] {
            let stage = buck2_data::ExecutorStageStart {
                stage: Some(Stage::CasArtifact(buck2_data::CasArtifactStage { phase })),
            };
            assert_eq!(executor_stage_name(&stage), name);
        }
        let stage = buck2_data::ExecutorStageStart {
            stage: Some(Stage::Local(buck2_data::LocalStage {
                stage: Some(buck2_data::local_stage::Stage::FinalizeOutputs(
                    Default::default(),
                )),
            })),
        };
        assert_eq!(executor_stage_name(&stage), "Local.FinalizeOutputs");
    }
}

#[cfg(test)]
mod execution_tests {
    use prost::Message as _;

    use super::*;
    use crate::execution;

    fn timestamp(us: i64) -> prost_types::Timestamp {
        prost_types::Timestamp {
            seconds: 1_700_000_000,
            nanos: (us * 1000) as i32,
        }
    }

    fn event(us: i64, sid: u64, parent: u64, data: buck2_data::buck_event::Data) -> StreamValue {
        StreamValue::Event(Box::new(buck2_data::BuckEvent {
            timestamp: Some(timestamp(us)),
            span_id: sid,
            parent_id: parent,
            data: Some(data),
            ..Default::default()
        }))
    }

    fn start(
        us: i64,
        sid: u64,
        parent: u64,
        data: buck2_data::span_start_event::Data,
    ) -> StreamValue {
        event(
            us,
            sid,
            parent,
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(data),
            }),
        )
    }

    fn end(
        us: i64,
        sid: u64,
        duration: i64,
        data: buck2_data::span_end_event::Data,
    ) -> StreamValue {
        event(
            us,
            sid,
            0,
            buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                duration: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: (duration * 1000) as i32,
                }),
                data: Some(data),
                ..Default::default()
            }),
        )
    }

    fn local() -> buck2_data::LocalCommand {
        buck2_data::LocalCommand {
            argv: vec![
                "python3.12".to_owned(),
                "rustc_action.py".to_owned(),
                "@args".to_owned(),
            ],
            env: vec![buck2_data::EnvironmentEntry {
                key: "EXPLICIT".to_owned(),
                value: "value".to_owned(),
            }],
            action_digest: "local:148".to_owned(),
            ..Default::default()
        }
    }

    fn fixture() -> Vec<StreamValue> {
        use buck2_data::command_execution::Status;
        use buck2_data::command_execution_kind::Command;
        use buck2_data::local_stage::Stage as Local;
        use buck2_data::span_end_event::Data as End;
        use buck2_data::span_start_event::Data as Start;
        let execute = || {
            Start::ExecutorStage(buck2_data::ExecutorStageStart {
                stage: Some(buck2_data::executor_stage_start::Stage::Local(
                    buck2_data::LocalStage {
                        stage: Some(Local::Execute(buck2_data::LocalExecute {
                            command: Some(local()),
                        })),
                    },
                )),
            })
        };
        vec![
            start(0, 10, 0, Start::ActionExecution(Default::default())),
            start(5, 11, 10, execute()),
            end(60, 11, 55, End::ExecutorStage(Default::default())),
            // A resource-control restart is another observation, not proof of
            // another report. Its cancellation is not command-output evidence.
            start(70, 12, 10, execute()),
            end(100, 12, 30, End::SpanCancelled(Default::default())),
            // The action can finish without this command start's own end;
            // preserve its source evidence but do not synthesize a stage end.
            start(
                101,
                13,
                10,
                Start::ExecutorStage(buck2_data::ExecutorStageStart {
                    stage: Some(buck2_data::executor_stage_start::Stage::Local(
                        buck2_data::LocalStage {
                            stage: Some(Local::WorkerInit(buck2_data::WorkerInit {
                                command: Some(buck2_data::WorkerInitCommand {
                                    argv: vec!["worker".to_owned()],
                                    env: Vec::new(),
                                }),
                            })),
                        },
                    )),
                }),
            ),
            end(
                200,
                10,
                200,
                End::ActionExecution(Box::new(buck2_data::ActionExecutionEnd {
                    commands: vec![
                        buck2_data::CommandExecution {
                            status: Some(Status::Failure(Default::default())),
                            details: Some(buck2_data::CommandExecutionDetails {
                                command_kind: Some(buck2_data::CommandExecutionKind {
                                    command: Some(Command::LocalCommand(local())),
                                }),
                                cmd_stdout: "first attempt stdout\n".to_owned(),
                                cmd_stderr: "first attempt stderr\n".to_owned(),
                                signed_exit_code: Some(1),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        buck2_data::CommandExecution {
                            status: Some(Status::Cancelled(Default::default())),
                            details: None,
                            ..Default::default()
                        },
                        buck2_data::CommandExecution {
                            status: Some(Status::Success(Default::default())),
                            details: Some(buck2_data::CommandExecutionDetails {
                                command_kind: Some(buck2_data::CommandExecutionKind {
                                    command: Some(Command::OmittedLocalCommand(
                                        buck2_data::OmittedLocalCommand {
                                            action_digest: "local:148".to_owned(),
                                        },
                                    )),
                                }),
                                cmd_stderr: "warning: unstable -Ctarget-feature=v9a\n".to_owned(),
                                signed_exit_code: Some(0),
                                metadata: Some(buck2_data::CommandExecutionMetadata {
                                    client_start_time: Some(timestamp(10)),
                                    wall_time: Some(prost_types::Duration {
                                        seconds: 0,
                                        nanos: 190_000,
                                    }),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ],
                    execution_kind: buck2_data::ActionExecutionKind::Local as i32,
                    ..Default::default()
                })),
            ),
            // A late start has no live enclosing action. It must not attach
            // retrospectively to a completed action's execution manifest.
            start(210, 14, 10, execute()),
            start(220, 20, 0, Start::ActionExecution(Default::default())),
            start(225, 21, 20, execute()),
            // Open action 20 never produces a finished row/manifest.
            StreamValue::Result(Box::default()),
        ]
    }

    fn fixture_log(dir: &std::path::Path, broken: bool) -> EventLogPathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let mut bytes = buck2_data::Invocation {
            trace_id: Some(crate::ingest::testlog::RUN_ID.to_owned()),
            start_time: Some(timestamp(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec();
        for (index, item) in fixture().into_iter().enumerate() {
            if broken && index == 7 {
                // A framed protobuf with an invalid tag; later records must
                // not be reduced or written even when decoded speculatively.
                bytes.extend([1, 0]);
            }
            let progress = match item {
                StreamValue::Event(event) => {
                    buck2_cli_proto::command_progress::Progress::Event(event)
                }
                StreamValue::Result(result) => {
                    buck2_cli_proto::command_progress::Progress::Result(result)
                }
                StreamValue::PartialResult(result) => {
                    buck2_cli_proto::command_progress::Progress::PartialResult(result)
                }
            };
            bytes.extend(
                buck2_cli_proto::CommandProgress {
                    progress: Some(progress),
                }
                .encode_length_delimited_to_vec(),
            );
        }
        let path = dir.join("input.pb");
        std::fs::write(&path, bytes).unwrap();
        crate::ingest::infer(&path).unwrap()
    }

    #[test]
    fn serial_and_worker_details_are_identical_lazy_and_report_complete() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-execution-ingest-parity-{}",
            std::process::id()
        ));
        let log = fixture_log(&dir, false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut expected = None;
        for phases in [false, true] {
            for workers in [0, 1, 4] {
                let staged = crate::ingest::Staged::beside(dir.join("execution.bin"));
                let mut writer = execution::DetailWriter::create(&staged.write_to).unwrap();
                let scan = runtime
                    .block_on(scan_with_workers_and_details(
                        &log,
                        phases,
                        workers,
                        Some(&mut writer),
                    ))
                    .unwrap();
                assert_eq!(scan.actions.len(), 1);
                assert!(scan.incomplete.is_none());
                assert_eq!(
                    scan.stages[&10].len(),
                    2,
                    "open worker start is not a completed stage"
                );
                let info = writer.finish().unwrap();
                let bytes = std::fs::read(&staged.write_to).unwrap();
                if let Some(expected) = &expected {
                    assert_eq!(&bytes, expected);
                } else {
                    expected = Some(bytes);
                }
                execution::stamp(&staged.write_to, &scan.trace_id, "test-capture", &info).unwrap();
                let reader = execution::DetailReader::open(
                    &staged.write_to,
                    &scan.trace_id,
                    "test-capture",
                    &info,
                )
                .unwrap();
                let loaded = reader
                    .action(scan.actions[0].execution.unwrap(), 10)
                    .unwrap();
                assert_eq!(
                    loaded
                        .attempts
                        .iter()
                        .map(|a| a.status.as_str())
                        .collect::<Vec<_>>(),
                    ["Failure", "Cancelled", "Success"]
                );
                assert_eq!(
                    loaded
                        .observations
                        .iter()
                        .map(|o| o.span_id)
                        .collect::<Vec<_>>(),
                    [11, 12, 13]
                );
                assert_eq!(
                    loaded.attempts[0].command.as_ref().unwrap().env,
                    [("EXPLICIT".to_owned(), "value".to_owned())]
                );
                assert_eq!(
                    loaded.attempts[1].stdout.state,
                    execution::OutputState::NotRecorded
                );
                assert_eq!(
                    loaded.attempts[2].stdout.state,
                    execution::OutputState::NotRecorded
                );
                assert!(
                    loaded.attempts[2].command.is_none(),
                    "omission remains explicit in the report"
                );
                assert_eq!(loaded.command_for(2).unwrap().argv, local().argv);
                let output = reader
                    .preview(loaded.attempts[2].stderr.content.unwrap(), 4096)
                    .unwrap();
                assert_eq!(output.text, "warning: unstable -Ctarget-feature=v9a\n");
                assert!(!output.truncated);
            }
        }
    }

    #[test]
    fn execution_store_errors_are_fatal_and_input_error_preserves_only_the_prefix() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-execution-ingest-errors-{}",
            std::process::id()
        ));
        let log = fixture_log(&dir, false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let staged = crate::ingest::Staged::beside(dir.join("failed.bin"));
        let mut writer = execution::DetailWriter::create(&staged.write_to).unwrap();
        writer.fail_writes_after(0);
        let error = runtime
            .block_on(scan_with_workers_and_details(
                &log,
                false,
                0,
                Some(&mut writer),
            ))
            .err()
            .expect("store I/O must fail the conversion");
        assert!(
            error
                .to_string()
                .contains("injected execution store write failure")
        );
        // Exercise output/attempt writes as well as the earlier start record.
        let mut projected = normalized::project_end(
            10,
            200,
            Box::new(buck2_data::ActionExecutionEnd {
                commands: vec![buck2_data::CommandExecution {
                    details: Some(buck2_data::CommandExecutionDetails {
                        cmd_stdout: "output".to_owned(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert!(write_action_details(&mut projected, Vec::new(), &mut writer).is_err());

        let log = fixture_log(&dir, true);
        let mut expected = None;
        for workers in [0, 1, 4] {
            let staged = crate::ingest::Staged::beside(dir.join("prefix.bin"));
            let mut writer = execution::DetailWriter::create(&staged.write_to).unwrap();
            let scan = runtime
                .block_on(scan_with_workers_and_details(
                    &log,
                    false,
                    workers,
                    Some(&mut writer),
                ))
                .unwrap();
            assert!(matches!(
                scan.incomplete,
                Some(Incomplete::Undecodable { records: 7, .. })
            ));
            assert_eq!(scan.actions.len(), 1);
            writer.finish().unwrap();
            let bytes = std::fs::read(&staged.write_to).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&bytes, expected);
            } else {
                expected = Some(bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_and_duration_microseconds_floor_negative_nanos() {
        for (nanos, expected) in [(-1_500, -2), (-1, -1), (0, 0), (1_500, 1)] {
            assert_eq!(
                ts_us(&prost_types::Timestamp { seconds: 0, nanos }),
                expected
            );
            assert_eq!(
                dur_us(&prost_types::Duration { seconds: 0, nanos }),
                expected
            );
        }
    }

    #[test]
    fn unindexable_package_path_is_refused_before_interning() {
        let mut start = normalized::project_start(47, 0, 0, Default::default());
        start.short_at = None;
        start.cfg = "must not be interned".to_owned();
        start.category = "neither should this".to_owned();
        let end = normalized::project_end(47, 0, Default::default());
        let mut interners = Interners::default();
        let error = commit_normalized(start, end, &mut interners)
            .expect_err("an unrepresentable package offset should be refused");
        assert!(error.to_string().contains("package path for span 47"));
        assert!(interners.cfg.names.is_empty() && interners.cfg.ids.is_empty());
        assert!(interners.category.names.is_empty() && interners.category.ids.is_empty());
    }

    #[test]
    fn watcher_counts_and_reasons_remain_bounded_and_honest() {
        let mut summary = FileWatcherSummary::default();
        for n in 0..12 {
            summary.record(&buck2_data::FileWatcherStats {
                events_total: 100,
                events_processed: 50,
                fresh_instance: n == 0,
                incomplete_events_reason: Some(format!("{n}:{}", "é".repeat(1000))),
                ..Default::default()
            });
        }
        assert_eq!(summary.updates, 12);
        assert_eq!(summary.events_total, Some(1200));
        assert_eq!(summary.events_processed, Some(600));
        assert!(summary.fresh_instance && summary.reasons_truncated);
        assert_eq!(summary.incomplete_events_reasons.len(), 8);
        assert!(
            summary
                .incomplete_events_reasons
                .iter()
                .all(|s| s.len() <= 512)
        );
        summary.record(&buck2_data::FileWatcherStats {
            events_processed: u64::MAX,
            ..Default::default()
        });
        summary.record(&buck2_data::FileWatcherStats::default());
        assert_eq!(
            summary.events_processed, None,
            "overflow never becomes a partial total"
        );
        assert_eq!(summary.events_total, Some(1200));
    }

    #[test]
    fn real_zstd_complete_and_interrupted_logs_match_serial_and_parallel_decoders() {
        let dir = crate::temp_path("ingest-compressed-parity");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = crate::ingest::testlog::demo_log(&dir, Some(1), true, false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for complete in [false, true] {
            let path = crate::ingest::testlog::zstd_copy(&raw, complete);
            let log = crate::ingest::infer(&path).unwrap();
            let mut expected = None;
            for workers in [0, 1, 4] {
                let scan = runtime
                    .block_on(scan_with_workers(&log, false, workers))
                    .unwrap();
                assert!(matches!(scan.incomplete, Some(Incomplete::NoResult)));
                assert_eq!(scan.actions.len(), 2);
                let actual = (scan.capture_state(), scan.actions);
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
            }
        }
    }

    #[test]
    fn internal_decoder_failure_is_fatal_even_after_a_readable_prefix() {
        let dir = crate::temp_path("ingest-internal-failure");
        std::fs::create_dir_all(&dir).unwrap();
        let log =
            crate::ingest::infer(&crate::ingest::testlog::demo_log(&dir, Some(1), true, true))
                .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(async {
                let (invocation, stream) = log.unpack_stream().await.unwrap();
                let input = stream
                    .take(3)
                    .chain(futures::stream::once(async {
                        Err(buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Tier0,
                            "fixture pool initialization failure"
                        ))
                    }))
                    .map(|item| item.map(normalized::Record::Raw))
                    .boxed();
                scan_input(invocation, ScanInput::Stream(input), false).await
            })
            .err()
            .expect("an internal failure must not return a partial Scan");
        assert!(
            error
                .to_string()
                .contains("fixture pool initialization failure")
        );
    }

    #[test]
    fn capture_identity_validation_survives_compaction_and_missing_timestamps() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::instant_event::Data as Instant;
        use buck2_data::span_end_event::Data as End;
        let dir = crate::temp_path("ingest-integrity-parity");
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::ingest::testlog::demo_log(&dir, Some(1), true, true);
        crate::ingest::testlog::rewrite(&path, |records| {
            for record in records.iter_mut() {
                if let Some(Progress::Event(event)) = &mut record.progress {
                    match &mut event.data {
                        Some(Data::SpanEnd(end)) => {
                            if let Some(End::ActionExecution(action)) = &mut end.data {
                                if event.span_id == 20 {
                                    action.key = Some(buck2_data::ActionKey::default());
                                    let producers = action.graph_producers.get_or_insert_default();
                                    producers.span_ids.extend([20, 123_456]);
                                    producers.tset_ids.push(1);
                                }
                            }
                        }
                        Some(Data::Instant(instant)) => {
                            if matches!(instant.data, Some(Instant::TransitiveSetProjectionDef(_)))
                            {
                                event.timestamp = None;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let mut foreign = records
                .iter()
                .find(|r| matches!(&r.progress, Some(Progress::Event(_))))
                .unwrap()
                .clone();
            if let Some(Progress::Event(event)) = &mut foreign.progress {
                event.trace_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned();
            }
            records.push(foreign);
        });
        let log = crate::ingest::infer(&path).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut expected = None;
        for workers in [0, 1, 4] {
            let scan = runtime
                .block_on(scan_with_workers(&log, false, workers))
                .unwrap();
            assert_eq!(scan.integrity.self_references, 1);
            assert_eq!(scan.integrity.foreign_event_ids, 1);
            assert_eq!(scan.integrity.dangling_producer_spans, 1);
            assert_eq!(scan.integrity.ambiguous_producer_spans, 1);
            assert!(!scan.stages.contains_key(&20));
            assert_eq!(scan.tset_def_events_seen, 1);
            let state = scan.capture_state();
            if let Some(expected) = &expected {
                assert_eq!(&state, expected);
            } else {
                expected = Some(state);
            }
        }
        crate::ingest::testlog::rewrite(&path, |records| {
            for record in records {
                if let Some(Progress::Event(event)) = &mut record.progress
                    && let Some(Data::Instant(instant)) = &mut event.data
                    && let Some(Instant::ActionGraphStart(start)) = &mut instant.data
                {
                    start.schema = 999;
                    event.timestamp = None;
                }
            }
        });
        for workers in [0, 4] {
            let error = runtime
                .block_on(scan_with_workers(&log, false, workers))
                .err()
                .unwrap();
            assert!(error.to_string().contains("schema 999"));
        }
    }

    #[test]
    fn cancelled_actions_stop_owning_later_stages_and_invalid_stage_ends_stay_countable() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::span_end_event::Data as End;
        let dir = crate::temp_path("ingest-cancel-stage-owner");
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::ingest::testlog::demo_log(&dir, Some(1), true, true);
        crate::ingest::testlog::rewrite(&path, |records| {
            for record in records {
                if let Some(Progress::Event(event)) = &mut record.progress {
                    if event.span_id == 21 {
                        event.parent_id = 10;
                    }
                    if let Some(Data::SpanEnd(end)) = &mut event.data {
                        if event.span_id == 10 {
                            end.data = Some(End::SpanCancelled(Default::default()));
                        } else if event.span_id == 22 {
                            end.duration = Some(prost_types::Duration {
                                seconds: -1,
                                nanos: 0,
                            });
                        }
                    }
                }
            }
        });
        let log = crate::ingest::infer(&path).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for workers in [0, 4] {
            let scan = runtime
                .block_on(scan_with_workers(&log, false, workers))
                .unwrap();
            assert_eq!(scan.actions_cancelled, 1);
            assert_eq!(scan.actions.len(), 1);
            assert!(!scan.stages.contains_key(&10));
            assert!(
                scan.stages[&20]
                    .iter()
                    .any(|s| s.name == "ReUpload" && s.dur < 0)
            );
        }
    }

    #[test]
    fn command_end_uses_build_result_and_does_not_claim_tests_passed() {
        let mut end = buck2_data::CommandEnd {
            is_success: true,
            data: Some(buck2_data::command_end::Data::Build(Default::default())),
            ..Default::default()
        };
        assert_eq!(command_end_outcome(&end), None);
        end.build_result = Some(buck2_data::BuildResult {
            build_completed: true,
        });
        end.is_success = false;
        assert_eq!(command_end_outcome(&end), Some(true));
        end.data = Some(buck2_data::command_end::Data::Test(Default::default()));
        assert_eq!(command_end_outcome(&end), None);
        end.build_result.as_mut().unwrap().build_completed = false;
        assert_eq!(command_end_outcome(&end), Some(false));
        assert_eq!(observed_duration(10, 30, None), Some(20));
        assert_eq!(observed_duration(30, 10, None), None);
        assert_eq!(observed_duration(10, 30, Some(-1)), None);
    }

    // Exercise serial/raw reference, serial normalized and ordered worker
    // interfaces explicitly; no test changes process-wide environment.
    async fn fixture_interface_scan(
        log: &EventLogPathBuf,
        phases: bool,
        interface: u8,
    ) -> anyhow::Result<Scan> {
        if interface == 8 {
            return scan_with_workers(log, phases, 0).await;
        }
        if matches!(interface, 1 | 2 | 5 | 6 | 7) {
            let (invocation, frames) = log
                .unpack_protobuf_frames()
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))?;
            let config = super::super::decode_pool::Config {
                expected_run: invocation.trace_id.to_string().as_bytes().try_into().ok(),
                frame_records: 3,
                decoded_records: 2,
                heap_mode: super::super::HeapMode::Verify,
                compact: matches!(interface, 6 | 7),
                phases,
                ..Default::default()
            };
            let input = if matches!(interface, 6 | 7) {
                let pool = super::super::decode_pool::Pool::new_normalized(frames, config)?;
                if interface == 6 {
                    ScanInput::Pool(pool)
                } else {
                    ScanInput::Stream(futures::stream::iter(pool).boxed())
                }
            } else {
                let pool = super::super::decode_pool::Pool::new(frames, config)?;
                if interface == 2 {
                    ScanInput::RawPool(pool)
                } else {
                    ScanInput::Stream(
                        futures::stream::iter(pool)
                            .map(move |item| {
                                item.map(|value| {
                                    if interface == 5 {
                                        normalized::project(value)
                                    } else {
                                        normalized::Record::Raw(value)
                                    }
                                })
                            })
                            .boxed(),
                    )
                }
            };
            return scan_input(invocation, input, phases).await;
        }
        let (invocation, stream) = log
            .unpack_stream()
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))?;
        let stream = stream
            .map(move |item| {
                item.map(|value| {
                    let value = if interface == 3 {
                        super::super::compact::clean(value, phases)
                    } else {
                        value
                    };
                    if interface == 4 {
                        normalized::project(value)
                    } else {
                        normalized::Record::Raw(value)
                    }
                })
            })
            .boxed();
        scan_input(invocation, ScanInput::Stream(stream), phases).await
    }

    #[test]
    fn structural_graphs_preserve_bridges_without_fabricated_action_work() {
        use crate::ingest::testlog::StructureCase;
        use crate::ingest::testlog::structural_log;
        use crate::ingest::testlog::zstd_copy;
        let dir = crate::temp_path("structural-bridge-native");
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for case in [
            StructureCase::Complete,
            StructureCase::NoTouch,
            StructureCase::InputsOmitted,
            StructureCase::MissingChunk,
            StructureCase::AliasCycle,
            StructureCase::Incomplete,
            StructureCase::ContradictoryDefinition,
        ] {
            let path = structural_log(&dir, case);
            let compressed = zstd_copy(&path, true);
            let log = crate::ingest::infer(&compressed).unwrap();
            let mut expected = None;
            for interface in [0, 3, 4, 6, 7, 8] {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, false, interface))
                    .unwrap();
                assert_eq!(
                    scan.actions.len(),
                    if case == StructureCase::NoTouch { 0 } else { 2 },
                    "{case:?}, interface {interface}"
                );
                assert_eq!(
                    scan.capture_state().is_complete(),
                    Some(matches!(
                        case,
                        StructureCase::Complete
                            | StructureCase::NoTouch
                            | StructureCase::InputsOmitted
                    ))
                );
                if let Some(expected) = &expected {
                    assert_eq!(&scan.capture_state(), expected);
                } else {
                    expected = Some(scan.capture_state());
                }
                if case == StructureCase::AliasCycle {
                    assert_eq!(scan.integrity.alias_cycles, 1);
                }
                if matches!(case, StructureCase::Complete | StructureCase::NoTouch) {
                    let trace = crate::ingest::Staged::beside(
                        dir.join(format!("trace-{case:?}-{interface}.json")),
                    );
                    let graph = crate::ingest::Staged::beside(
                        dir.join(format!("graph-{case:?}-{interface}.json")),
                    );
                    super::super::emit::emit(scan, &trace, Some(&graph), false).unwrap();
                    let trace = crate::parse::parse(trace.write_to.to_str().unwrap()).unwrap();
                    let graph = crate::graph::load_graph(graph.write_to.to_str().unwrap()).unwrap();
                    if case == StructureCase::NoTouch {
                        let coverage = graph.meta.coverage.as_ref().unwrap();
                        assert_eq!(coverage.action_rows, Some(0));
                        assert_eq!(coverage.join_rows, Some(2));
                        assert_eq!(coverage.structural_rows, Some(1));
                        assert_eq!(coverage.nodes, Some(2));
                        assert_eq!(coverage.edges, Some(1));
                        assert!(
                            trace
                                .acts
                                .iter()
                                .all(|act| act.flags & crate::types::F_NOT_RECIPE != 0)
                        );
                        continue;
                    }
                    let (bundle, stats) = crate::dag::build_full(&trace, &graph, None).unwrap();
                    assert_eq!(stats.n_events, 2);
                    assert_eq!(bundle.dag.cp, 300_000);
                    assert_eq!(bundle.dag.cp_path.len(), 2);
                    assert_eq!(bundle.dag.weight_us.iter().sum::<i64>(), 300_000);
                    assert_eq!(graph.meta.coverage.as_ref().unwrap().action_rows, Some(2));
                    assert_eq!(graph.meta.coverage.as_ref().unwrap().join_rows, Some(2));
                }
            }
        }
    }

    #[test]
    fn activity_scope_survives_all_readers_without_degrading_current_action_models() {
        use crate::quality::Claim;
        use crate::quality::Dep;
        use crate::quality::Quality;
        let dir = crate::temp_path("activity-scope-native");
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for empty in [false, true] {
            let path = crate::ingest::testlog::activity_log(&dir, empty);
            let log = crate::ingest::infer(&path).unwrap();
            for interface in 0..9 {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, false, interface))
                    .unwrap();
                let state = scan.capture_state();
                assert_eq!(state.scope(), Some(CaptureScope::CurrentActionAncestors));
                assert_eq!(state.is_complete(), Some(true), "{:?}", state.problems());
                assert_eq!(state.to_json()["structure_wall_time_us"], 123);
                let trace = crate::ingest::Staged::beside(
                    dir.join(format!("activity-{empty}-{interface}-trace.json")),
                );
                let graph = crate::ingest::Staged::beside(
                    dir.join(format!("activity-{empty}-{interface}-graph.json")),
                );
                let emitted = super::super::emit::emit(scan, &trace, Some(&graph), false).unwrap();
                assert!(
                    emitted
                        .status_lines
                        .iter()
                        .any(|line| line.contains("actions-that-ran dependency graph"))
                );
                let trace = crate::parse::parse(trace.write_to.to_str().unwrap()).unwrap();
                let graph = crate::graph::load_graph(graph.write_to.to_str().unwrap()).unwrap();
                assert_eq!(graph.meta.enc, Some(3));
                assert_eq!(graph.meta.kind, "current_action_ancestors");
                assert_eq!(
                    graph.meta.coverage.as_ref().unwrap().dep_capture.as_ref(),
                    Some(&state)
                );
                if empty {
                    assert_eq!(graph.meta.coverage.as_ref().unwrap().nodes, Some(0));
                    assert_eq!(graph.meta.coverage.as_ref().unwrap().edges, Some(0));
                    continue;
                }
                let (bundle, stats) = crate::dag::build_full(&trace, &graph, None).unwrap();
                assert_eq!(stats.n_events, 2);
                assert_eq!(bundle.dag.cp, 300_000);
                assert_eq!(bundle.dag.weight_us.iter().sum::<i64>(), 300_000);
                let quality = Quality::assess(&trace, None, Some(&graph)).stitched(&stats);
                assert!(quality.capture_complete() && quality.graph_complete());
                assert_eq!(quality.graph_label(), "graph (current-action ancestors)");
                for claim in [Claim::LongestPath, Claim::Savings] {
                    let report = quality.seal(
                        Dep::Graph,
                        claim,
                        "available",
                        None,
                        vec![],
                        vec![],
                        serde_json::json!({"cp_us": bundle.dag.cp}),
                    );
                    assert_eq!(report["status"], "available");
                    assert!(report.get("bound").is_none());
                    assert_eq!(report["graph_scope"], "current_action_ancestors");
                    assert!(
                        report["limitations"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|note| note == crate::quality::ACTIVITY_SCOPE_NOTE)
                    );
                }
                let fanout = quality.seal(
                    Dep::Graph,
                    Claim::DescendantPath,
                    "available",
                    None,
                    vec![],
                    vec![],
                    serde_json::Value::Null,
                );
                assert_eq!(
                    fanout["status"], "partial",
                    "whole-build fanout is scope-limited"
                );
                assert!(
                    fanout.get("bound").is_none(),
                    "intact scoped paths are not degraded"
                );
            }
        }
    }

    #[test]
    fn unsupported_schema_fails_closed_on_every_scan_interface() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::instant_event::Data as Instant;
        let dir = crate::temp_path("capture-schema-errors");
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for invalid_start in [false, true] {
            let path = crate::ingest::testlog::activity_log(&dir, true);
            crate::ingest::testlog::rewrite(&path, |records| {
                for record in records {
                    let Some(Progress::Event(event)) = &mut record.progress else {
                        continue;
                    };
                    let Some(Data::Instant(instant)) = &mut event.data else {
                        continue;
                    };
                    match &mut instant.data {
                        Some(Instant::ActionGraphStart(start)) if invalid_start => {
                            start.schema = 999;
                            event.timestamp = None;
                        }
                        Some(Instant::ActionGraphFooter(footer)) if !invalid_start => {
                            footer.schema = 999;
                            event.timestamp = None;
                        }
                        _ => {}
                    }
                }
            });
            let log = crate::ingest::infer(&path).unwrap();
            for interface in 0..9 {
                let error = runtime
                    .block_on(fixture_interface_scan(&log, false, interface))
                    .err()
                    .expect("an unknown schema must fail closed");
                assert!(error.to_string().contains("schema 999"), "{error:#}");
            }
        }
    }

    #[test]
    fn alias_resolution_is_iterative_and_does_not_choose_a_cycle_member() {
        let mut aliases: HashMap<_, _> = (0u128..10_000)
            .map(|key| (key, AliasDef { canonical: key + 1 }))
            .collect();
        let mut integrity = crate::types::CaptureIntegrity::default();
        flatten_aliases(&mut aliases, &mut integrity);
        assert!(aliases.values().all(|alias| alias.canonical == 10_000));
        assert!(integrity.is_empty());
        aliases.insert(10_000, AliasDef { canonical: 0 });
        flatten_aliases(&mut aliases, &mut integrity);
        assert_eq!(integrity.alias_cycles, 1);
        assert!(aliases.is_empty());
    }

    #[test]
    fn local_cache_stages_survive_serial_parallel_and_compaction_paths() {
        let dir = crate::temp_path("local-cache-stage-scan");
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::ingest::testlog::local_cache_stage_log(&dir);
        let compressed = crate::ingest::testlog::zstd_copy(&path, true);
        let log = crate::ingest::infer(&compressed).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for phases in [false, true] {
            for interface in [0, 3, 4, 6, 7, 8] {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, phases, interface))
                    .unwrap();
                assert_eq!(scan.actions.len(), 1);
                // Wrong action-end SID is preserved as an integrity diagnostic,
                // but cannot close a MatchDepFiles stage in raw or normalized paths.
                assert_eq!(scan.integrity.orphan_action_ends, 1);
                let stages = &scan.stages[&10];
                let actual: Vec<_> = stages
                    .iter()
                    .map(|s| {
                        let outcome = match s.details.as_ref() {
                            Some(StageDetails::LocalCacheOutcome(outcome)) => Some(*outcome),
                            Some(StageDetails::LocalCacheTimings(end)) => Some(end.outcome),
                            _ => None,
                        };
                        (s.name, s.dur, s.cancelled, outcome)
                    })
                    .collect();
                assert_eq!(
                    actual,
                    vec![
                        ("LocalCache.CheckIdenticalAction", 30, false, Some(2)),
                        ("LocalCache.CheckFilteredInputs", 30, false, Some(1)),
                        ("RemoteCache.CheckFilteredInputs", 90, false, None),
                        ("LocalCache.CheckIdenticalAction", 10, true, None),
                        ("LocalCache.CheckIdenticalAction", 10, false, Some(3)),
                        ("LocalCache.CheckIdenticalAction", 0, false, Some(42)),
                    ],
                    "interface {interface}, phases {phases}"
                );
                assert!(matches!(
                    stages[0].details,
                    Some(StageDetails::LocalCacheOutcome(2))
                ));
                assert!(matches!(
                    stages[4].details,
                    Some(StageDetails::LocalCacheTimings(_))
                ));
            }
        }
    }

    #[test]
    fn serial_and_worker_scans_preserve_full_pairs_and_json_fallback() {
        use prost::Message as _;

        let dir = std::env::temp_dir().join(format!(
            "retrace-production-decode-parity-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let proto_path = crate::ingest::testlog::demo_log(&dir, Some(1), true, true);
        let wire = std::fs::read(&proto_path).unwrap();
        let mut input = wire.as_slice();
        let invocation = buck2_data::Invocation::decode_length_delimited(&mut input).unwrap();
        let mut json = serde_json::to_vec(&invocation).unwrap();
        json.push(b'\n');
        while !input.is_empty() {
            let record =
                buck2_cli_proto::CommandProgress::decode_length_delimited(&mut input).unwrap();
            let record = match record.progress.unwrap() {
                buck2_cli_proto::command_progress::Progress::Event(value) => {
                    StreamValue::Event(value)
                }
                buck2_cli_proto::command_progress::Progress::Result(value) => {
                    StreamValue::Result(value)
                }
                buck2_cli_proto::command_progress::Progress::PartialResult(value) => {
                    StreamValue::PartialResult(value)
                }
            };
            serde_json::to_writer(&mut json, &record).unwrap();
            json.push(b'\n');
        }
        let json_path = dir.join("same-log.json-lines");
        std::fs::write(&json_path, json).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for phases in [false, true] {
            let mut expected = None;
            for path in [&proto_path, &json_path] {
                let log = crate::ingest::infer(path).unwrap();
                for workers in [0, 1, 4] {
                    let scan = runtime
                        .block_on(scan_with_workers(&log, phases, workers))
                        .unwrap();
                    assert!(scan.command_result_seen && scan.incomplete.is_none());
                    let trace = crate::ingest::Staged::beside(dir.join("trace.json"));
                    let graph = crate::ingest::Staged::beside(dir.join("graph.json"));
                    let summary =
                        super::super::emit::emit(scan, &trace, Some(&graph), phases).unwrap();
                    assert!(matches!(
                        summary.graph,
                        super::super::emit::GraphOutcome::Written
                    ));
                    let actual = (
                        std::fs::read(&trace.write_to).unwrap(),
                        std::fs::read(&graph.write_to).unwrap(),
                        summary.capture,
                    );
                    if let Some(expected) = &expected {
                        assert_eq!(
                            &actual, expected,
                            "{path:?}, workers={workers}, phases={phases}"
                        );
                    } else {
                        expected = Some(actual);
                    }
                }
            }
        }
    }

    #[test]
    fn batch_consumer_emits_identical_demo_artifacts_for_both_phase_modes() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-batch-consumer-artifacts-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log =
            crate::ingest::infer(&crate::ingest::testlog::demo_log(&dir, Some(1), true, true))
                .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for phases in [false, true] {
            let mut expected = None;
            for interface in 0..9 {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, phases, interface))
                    .unwrap();
                assert!(scan.command_result_seen && scan.incomplete.is_none());
                let trace = crate::ingest::Staged::beside(
                    dir.join(format!("trace-{phases}-{interface}.json")),
                );
                let graph = crate::ingest::Staged::beside(
                    dir.join(format!("graph-{phases}-{interface}.json")),
                );
                super::super::emit::emit(scan, &trace, Some(&graph), phases).unwrap();
                let actual = (
                    std::fs::read(&trace.write_to).unwrap(),
                    std::fs::read(&graph.write_to).unwrap(),
                );
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected, "phases={phases}, interface={interface}");
                } else {
                    expected = Some(actual);
                }
            }
        }
    }

    #[test]
    fn normalization_preserves_cache_retry_wait_and_missing_metadata_artifacts() {
        use crate::ingest::testlog;
        let dir = std::env::temp_dir().join(format!(
            "retrace-normalized-artifacts-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let compare = |path: std::path::PathBuf| {
            let log = crate::ingest::infer(&path).unwrap();
            let mut expected = None;
            for interface in [0, 4, 6, 7, 8] {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, false, interface))
                    .unwrap();
                let trace =
                    crate::ingest::Staged::beside(dir.join(format!("norm-{interface}-trace.json")));
                let graph =
                    crate::ingest::Staged::beside(dir.join(format!("norm-{interface}-graph.json")));
                let state = scan.capture_state().to_json();
                let host = scan.run_host.clone();
                let summary = super::super::emit::emit(scan, &trace, Some(&graph), false).unwrap();
                let graph_bytes = match summary.graph {
                    super::super::emit::GraphOutcome::Written => {
                        Some(std::fs::read(&graph.write_to).unwrap())
                    }
                    super::super::emit::GraphOutcome::NotWritten => None,
                };
                let actual = (
                    std::fs::read(&trace.write_to).unwrap(),
                    graph_bytes,
                    state,
                    host,
                );
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected, "interface={interface}, log={path:?}");
                } else {
                    expected = Some(actual);
                }
            }
        };
        compare(testlog::cache_hit_log(&dir));
        for (kind, hit) in [
            (2, false),
            (2, true),
            (3, false),
            (7, false),
            (9, false),
            (10, false),
        ] {
            for prior in [false, true] {
                compare(testlog::cached_execution_log(&dir, kind, hit, prior));
            }
        }
        for case in [
            testlog::WaitStateCase::LocalWinner {
                download_end: false,
            },
            testlog::WaitStateCase::LocalWinner { download_end: true },
            testlog::WaitStateCase::RemoteRetry,
        ] {
            compare(testlog::wait_state_log(&dir, case));
        }
    }

    #[test]
    fn normalization_preserves_orphans_overwrites_and_end_order_after_result() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::span_end_event::Data as End;
        use buck2_data::span_start_event::Data as Start;
        use prost::Message as _;

        fn start(cfg: &str) -> Data {
            Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(Start::ActionExecution(buck2_data::ActionExecutionStart {
                    key: Some(buck2_data::ActionKey {
                        owner: Some(buck2_data::action_key::Owner::TargetLabel(
                            buck2_data::ConfiguredTargetLabel {
                                label: Some(buck2_data::TargetLabel {
                                    package: "pkg".to_owned(),
                                    name: "t".to_owned(),
                                }),
                                configuration: Some(buck2_data::Configuration {
                                    full_name: cfg.to_owned(),
                                }),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    }),
                    name: Some(buck2_data::ActionName {
                        category: cfg.to_owned(),
                        identifier: String::new(),
                    }),
                    ..Default::default()
                })),
            })
        }
        fn end(host: Option<&str>, deps: Vec<u64>) -> Data {
            Data::SpanEnd(buck2_data::SpanEndEvent {
                duration: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 2_000,
                }),
                data: Some(End::ActionExecution(Box::new(
                    buck2_data::ActionExecutionEnd {
                        execution_kind: buck2_data::ActionExecutionKind::Local as i32,
                        hostname: host.map(str::to_owned),
                        graph_producers: Some(buck2_data::ActionGraphProducers {
                            span_ids: deps,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ))),
                ..Default::default()
            })
        }
        let event = |sid, parent_id, data, timestamp: bool| {
            Progress::Event(Box::new(buck2_data::BuckEvent {
                timestamp: timestamp.then_some(prost_types::Timestamp {
                    seconds: 1,
                    nanos: 0,
                }),
                span_id: sid,
                parent_id,
                data: Some(data),
                ..Default::default()
            }))
        };
        let values = vec![
            event(90, 0, start("ignored-no-timestamp"), false),
            event(90, 0, end(Some("first-orphan-host"), vec![999]), true),
            event(10, 0, start("overwritten"), true),
            event(20, 0, start("first-end"), true),
            event(10, 0, start("second-end"), true),
            Progress::Result(Box::default()),
            event(20, 0, end(Some("later-host"), Vec::new()), true),
            event(
                21,
                10,
                Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(Start::ExecutorStage(Default::default())),
                }),
                true,
            ),
            event(10, 0, end(None, Vec::new()), true),
            event(
                21,
                10,
                Data::SpanEnd(buck2_data::SpanEndEvent {
                    duration: Some(prost_types::Duration {
                        seconds: 0,
                        nanos: 1_000,
                    }),
                    data: Some(End::ExecutorStage(Default::default())),
                    ..Default::default()
                }),
                true,
            ),
            event(10, 0, end(None, Vec::new()), true),
            event(30, u64::MAX, start("open-at-eof"), true),
        ];
        let mut bytes = buck2_data::Invocation {
            trace_id: Some(crate::ingest::testlog::RUN_ID.to_owned()),
            ..Default::default()
        }
        .encode_length_delimited_to_vec();
        for progress in values {
            buck2_cli_proto::CommandProgress {
                progress: Some(progress),
            }
            .encode_length_delimited(&mut bytes)
            .unwrap();
        }
        let dir = std::env::temp_dir().join(format!(
            "retrace-normalized-lifecycle-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.pb");
        std::fs::write(&path, bytes).unwrap();
        let log = crate::ingest::infer(&path).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut expected = None;
        for interface in 0..9 {
            let scan = runtime
                .block_on(fixture_interface_scan(&log, false, interface))
                .unwrap();
            assert_eq!(
                scan.actions.iter().map(|a| a.sid).collect::<Vec<_>>(),
                vec![20, 10]
            );
            assert_eq!(scan.cfg_names, vec!["first-end", "second-end"]);
            assert_eq!(scan.category_names, scan.cfg_names);
            assert_eq!(scan.action_ends_seen, 4);
            assert_eq!(scan.run_host.as_deref(), Some("first-orphan-host"));
            assert!(scan.capture_started && scan.command_result_seen);
            assert!(scan.incomplete.is_none() && scan.stages.is_empty());
            let actual = (scan.capture_state().to_json(), scan.actions);
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                expected = Some(actual);
            }
        }
    }

    #[test]
    fn command_profile_survives_native_interfaces_emission_and_first_trace_parse() {
        let dir =
            std::env::temp_dir().join(format!("retrace-command-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = crate::ingest::infer(&crate::ingest::testlog::command_profile_log(&dir)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut expected_profile = None;
        for phases in [false, true] {
            let mut expected_bytes = None;
            for interface in 0..9 {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, phases, interface))
                    .unwrap();
                assert_eq!(scan.actions.len(), 1);
                assert_eq!(scan.stages[&10][0].name, "Local.Execute");
                assert_eq!(scan.target_patterns, ["fb4a"]);
                assert_eq!(
                    scan.command_profile.result_outcome.as_deref(),
                    Some("success")
                );
                assert_eq!(
                    scan.command_profile.command_kind.as_deref(),
                    Some("install")
                );
                assert!(scan.footer.is_none());
                let profile = scan.command_profile.clone();
                let analysis = profile
                    .phases
                    .iter()
                    .find(|p| p.kind == "Analysis")
                    .unwrap();
                assert_eq!(analysis.total_duration_us, 1000);
                assert_eq!(analysis.active_us(), Some(700));
                assert_eq!(analysis.union_intervals, [[200, 900]]);
                assert_eq!(profile.snapshots[0].rss_bytes, Some(1000));
                assert_eq!(profile.install_duration_us, Some(3));
                let cp = profile.critical_path.as_ref().unwrap();
                assert_eq!(cp.entries.len(), 5);
                assert_eq!(cp.command_start_offset_us, profile.command_start_offset_us);
                assert!(cp.approximate_alignment);
                if let Some(expected) = &expected_profile {
                    assert_eq!(&profile, expected);
                } else {
                    expected_profile = Some(profile.clone());
                }
                let trace = crate::ingest::Staged::beside(
                    dir.join(format!("{phases}-{interface}-trace.json")),
                );
                let graph = crate::ingest::Staged::beside(
                    dir.join(format!("{phases}-{interface}-graph.json")),
                );
                let summary = super::super::emit::emit(scan, &trace, Some(&graph), phases).unwrap();
                let graph_bytes = match summary.graph {
                    super::super::emit::GraphOutcome::Written => {
                        Some(std::fs::read(&graph.write_to).unwrap())
                    }
                    super::super::emit::GraphOutcome::NotWritten => None,
                };
                let bytes = (std::fs::read(&trace.write_to).unwrap(), graph_bytes);
                if let Some(expected) = &expected_bytes {
                    assert_eq!(&bytes, expected, "interface={interface}, phases={phases}");
                } else {
                    expected_bytes = Some(bytes);
                }
                let parsed = crate::parse::parse(trace.write_to.to_str().unwrap()).unwrap();
                assert_eq!(parsed.runs.len(), 1);
                assert_eq!(parsed.runs[0].outcome, crate::types::Outcome::Success);
                // Emission links the reported action step (span 10) to the trace's only action.
                let mut linked = profile.clone();
                for entry in &mut linked.critical_path.as_mut().unwrap().entries {
                    if entry.span_ids == [10] {
                        entry.acts = vec![0];
                    }
                }
                assert_eq!(parsed.runs[0].command_profile.as_ref(), Some(&linked));
            }
        }
    }

    #[test]
    fn batch_consumer_keeps_continues_local_and_first_error_exits_the_whole_scan() {
        use buck2_cli_proto::command_progress::Progress;
        use prost::Message as _;

        fn frame(progress: Progress) -> buck2_error::Result<prost::bytes::BytesMut> {
            Ok(prost::bytes::BytesMut::from(
                buck2_cli_proto::CommandProgress {
                    progress: Some(progress),
                }
                .encode_length_delimited_to_vec()
                .as_slice(),
            ))
        }
        fn instant(data: buck2_data::instant_event::Data, timestamp: bool) -> Progress {
            Progress::Event(Box::new(buck2_data::BuckEvent {
                timestamp: timestamp.then_some(prost_types::Timestamp {
                    seconds: 123,
                    nanos: 0,
                }),
                data: Some(buck2_data::buck_event::Data::Instant(
                    buck2_data::InstantEvent { data: Some(data) },
                )),
                ..Default::default()
            }))
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for phases in [false, true] {
            for batches in [false, true] {
                for normalized in [false, true] {
                    let frames = vec![
                        frame(Progress::PartialResult(Box::default())),
                        // An unpositioned, irrelevant event contributes one
                        // successful record without aborting this batch.
                        frame(instant(
                            buck2_data::instant_event::Data::ReSession(Default::default()),
                            false,
                        )),
                        frame(instant(
                            buck2_data::instant_event::Data::ReSession(
                                buck2_data::RemoteExecutionSessionCreated {
                                    session_id: "before-error".to_owned(),
                                    ..Default::default()
                                },
                            ),
                            true,
                        )),
                        Err(buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "ordered fixture error"
                        )),
                        // An inner-loop-only break would resume here and turn the
                        // already established decode error into a schema refusal.
                        frame(instant(
                            buck2_data::instant_event::Data::ActionGraphStart(
                                buck2_data::ActionGraphStart { schema: 999 },
                            ),
                            true,
                        )),
                        frame(Progress::Result(Box::default())),
                    ];
                    let frames = futures::stream::iter(frames).boxed();
                    let input = if normalized {
                        let pool = super::super::decode_pool::Pool::new_normalized(
                            frames,
                            Default::default(),
                        )
                        .unwrap();
                        if batches {
                            ScanInput::Pool(pool)
                        } else {
                            ScanInput::Stream(futures::stream::iter(pool).boxed())
                        }
                    } else {
                        let pool = super::super::decode_pool::Pool::new(frames, Default::default())
                            .unwrap();
                        if batches {
                            ScanInput::RawPool(pool)
                        } else {
                            ScanInput::Stream(
                                futures::stream::iter(pool)
                                    .map(|item| item.map(normalized::Record::Raw))
                                    .boxed(),
                            )
                        }
                    };
                    let invocation = buck2_event_log::utils::Invocation {
                        command_line_args: Vec::new(),
                        expanded_command_line_args: Vec::new(),
                        working_dir: String::new(),
                        trace_id: crate::ingest::testlog::RUN_ID.parse().unwrap(),
                        start_time: None,
                    };
                    let scan = runtime
                        .block_on(scan_input(invocation, input, phases))
                        .unwrap();
                    assert_eq!(scan.re_session.as_deref(), Some("before-error"));
                    assert!(!scan.command_result_seen && !scan.capture_started);
                    assert!(scan.footer.is_none());
                    match scan.incomplete {
                        Some(Incomplete::Undecodable { records, error }) => {
                            assert_eq!(records, 3);
                            assert!(error.contains("ordered fixture error"));
                        }
                        _ => panic!("the first decode error must survive whole-scan exit"),
                    }
                }
            }
        }
    }

    #[test]
    fn batch_consumer_preserves_start_and_footer_schema_refusals() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-batch-consumer-schema-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for with_start_stamp in [false, true] {
            let log = crate::ingest::infer(&crate::ingest::testlog::demo_log_opts(
                &dir,
                Some(999),
                true,
                true,
                with_start_stamp,
            ))
            .unwrap();
            for phases in [false, true] {
                for interface in 0..9 {
                    let error = runtime
                        .block_on(fixture_interface_scan(&log, phases, interface))
                        .err()
                        .expect("schema mismatch must fail every interface");
                    assert_eq!(error.to_string(), schema_mismatch(999).to_string());
                }
            }
        }
    }

    fn local_stage(command: Option<buck2_data::LocalCommand>) -> buck2_data::ExecutorStageStart {
        buck2_data::ExecutorStageStart {
            stage: Some(buck2_data::executor_stage_start::Stage::Local(
                buck2_data::LocalStage {
                    stage: Some(buck2_data::local_stage::Stage::Execute(
                        buck2_data::LocalExecute { command },
                    )),
                },
            )),
        }
    }

    #[test]
    fn compact_executor_starts_match_all_reference_stage_names_and_argv() {
        use buck2_data::executor_stage_start::Stage as Executor;
        use buck2_data::local_stage::Stage as Local;
        use buck2_data::re_stage::Stage as Remote;

        fn check(st: buck2_data::ExecutorStageStart) {
            let expected_name = executor_stage_name(&st);
            let expected_argv = local_execute_argv(&st);
            let StageStartKind::Executor {
                name,
                local_command,
            } = StageStartKind::executor(st)
            else {
                unreachable!()
            };
            assert_eq!(name, expected_name);
            assert_eq!(local_command.map(|c| c.argv), expected_argv);
        }
        check(buck2_data::ExecutorStageStart::default());
        for stage in [
            Executor::CacheQuery(Default::default()),
            Executor::CacheHit(Default::default()),
            Executor::Prepare(Default::default()),
        ] {
            check(buck2_data::ExecutorStageStart { stage: Some(stage) });
        }
        for stage in [
            None,
            Some(Remote::Execute(Default::default())),
            Some(Remote::Download(Default::default())),
            Some(Remote::Queue(Default::default())),
            Some(Remote::WorkerDownload(Default::default())),
            Some(Remote::WorkerUpload(Default::default())),
            Some(Remote::Unknown(Default::default())),
            Some(Remote::MaterializeFailedInputs(Default::default())),
            Some(Remote::BeforeActionExecution(Default::default())),
            Some(Remote::AfterActionExecution(Default::default())),
            Some(Remote::QueueOverQuota(Default::default())),
            Some(Remote::QueueNoWorkerAvailable(Default::default())),
            Some(Remote::QueueAcquiringDependencies(Default::default())),
            Some(Remote::QueueCancelled(Default::default())),
        ] {
            check(buck2_data::ExecutorStageStart {
                stage: Some(Executor::Re(buck2_data::ReStage { stage })),
            });
        }
        for stage in [
            None,
            Some(Local::Queued(Default::default())),
            Some(Local::Execute(Default::default())),
            Some(Local::MaterializeInputs(Default::default())),
            Some(Local::PrepareOutputs(Default::default())),
            Some(Local::AcquireLocalResource(Default::default())),
            Some(Local::WorkerInit(Default::default())),
            Some(Local::WorkerExecute(buck2_data::WorkerExecute {
                command: Some(buck2_data::WorkerCommand {
                    argv: vec!["not-the-Local.Execute-fallback".to_owned()],
                    ..Default::default()
                }),
            })),
            Some(Local::WorkerQueued(Default::default())),
            Some(Local::WorkerWait(Default::default())),
        ] {
            check(buck2_data::ExecutorStageStart {
                stage: Some(Executor::Local(buck2_data::LocalStage { stage })),
            });
        }
        check(local_stage(Some(buck2_data::LocalCommand {
            argv: vec!["compiler".to_owned(), "a b.rs".to_owned()],
            ..Default::default()
        })));
        assert!(
            std::mem::size_of::<StageStartKind>()
                < std::mem::size_of::<buck2_data::ExecutorStageStart>(),
            "the live-map value must actually be smaller"
        );
    }

    #[test]
    fn stage_payload_layout_and_cache_projection_preserve_allocations_and_old_shapes() {
        #[allow(dead_code)]
        struct PreviousStageRec {
            name: &'static str,
            ts: i64,
            dur: i64,
            cancelled: bool,
            re_upload: Option<Box<buck2_data::ReUploadEnd>>,
            re_submit: Option<Box<buck2_data::ReSubmitEnd>>,
            local_command: Option<Box<LocalCommandRec>>,
        }
        eprintln!(
            "StageRec bytes: previous={}, current={}",
            std::mem::size_of::<PreviousStageRec>(),
            std::mem::size_of::<StageRec>()
        );
        assert!(std::mem::size_of::<StageRec>() < std::mem::size_of::<PreviousStageRec>());
        let (details, invalid) = executor_details("CacheQuery", Default::default(), None);
        assert!(
            details.is_none() && !invalid,
            "old empty ends need no allocation"
        );

        let (details, invalid) = executor_details(
            "CacheQuery",
            buck2_data::ExecutorStageEnd {
                cache_query: Some(buck2_data::CacheQueryTimings {
                    slot_wait_us: 0,
                    remote_lookup_us: 45,
                }),
                ..Default::default()
            },
            None,
        );
        let Some(StageDetails::CacheQuery(query)) = details else {
            panic!("query lost")
        };
        assert_eq!((query.slot_wait_us, query.remote_lookup_us), (0, 45));
        assert!(!invalid);

        for name in ["CacheQuery", "CacheHit", "Prepare"] {
            let (details, invalid) = executor_details(
                name,
                buck2_data::ExecutorStageEnd {
                    cache_query: Some(Default::default()),
                    cache_hit: Some(Default::default()),
                    ..Default::default()
                },
                None,
            );
            assert!(
                details.is_none() && invalid,
                "contradictory timing must not be selected"
            );
        }
        let (details, invalid) = executor_details(
            "Local.Execute",
            buck2_data::ExecutorStageEnd {
                cache_hit: Some(Default::default()),
                ..Default::default()
            },
            Some(Box::new(LocalCommandRec {
                argv: vec!["compiler".to_owned()],
                digest: None,
            })),
        );
        assert!(invalid);
        assert!(matches!(details, Some(StageDetails::LocalCommand(_))));
    }

    #[test]
    fn compact_executor_starts_move_argv_and_preserve_missing_empty_and_cancelled() {
        let mut argument = String::with_capacity(127);
        argument.push_str("compiler");
        let argument_pointer = argument.as_ptr();
        let argument_capacity = argument.capacity();
        let mut argv = Vec::with_capacity(31);
        argv.push(argument);
        let vector_pointer = argv.as_ptr();
        let vector_capacity = argv.capacity();
        let kind = StageStartKind::executor(local_stage(Some(buck2_data::LocalCommand {
            argv,
            action_digest: "unused stage digest".to_owned(),
            ..Default::default()
        })));
        let StageStartKind::Executor {
            name,
            local_command,
        } = &kind
        else {
            unreachable!()
        };
        assert_eq!(*name, "Local.Execute");
        let moved = &local_command.as_ref().unwrap().argv;
        assert_eq!(moved.as_ptr(), vector_pointer);
        assert_eq!(moved.capacity(), vector_capacity);
        assert_eq!(moved[0].as_ptr(), argument_pointer);
        assert_eq!(moved[0].capacity(), argument_capacity);
        let record = cancelled_stage(100, 150, None, kind).unwrap();
        assert!(record.cancelled);
        assert_eq!(record.dur, 50);
        assert_eq!(
            record.local_command().unwrap().argv.as_ptr(),
            vector_pointer
        );

        for command in [None, Some(buck2_data::LocalCommand::default())] {
            let expected = command.as_ref().map(|c| c.argv.clone());
            let kind = StageStartKind::executor(local_stage(command));
            let record = cancelled_stage(100, 100, Some(0), kind).unwrap();
            assert_eq!(record.local_command().map(|c| c.argv.clone()), expected);
            assert_eq!(record.dur, 0);
        }
        assert!(
            cancelled_stage(100, 99, None, StageStartKind::executor(local_stage(None))).is_none()
        );
        assert!(
            cancelled_stage(
                100,
                101,
                Some(-1),
                StageStartKind::executor(local_stage(None))
            )
            .is_none()
        );
    }

    #[test]
    fn compact_stage_lifecycle_preserves_duplicates_mismatched_ends_and_late_owners() {
        use buck2_data::buck_event::Data as Event;
        use buck2_data::span_end_event::Data as End;
        use buck2_data::span_start_event::Data as Start;
        use prost::Message as _;

        fn timestamp(us: i64) -> prost_types::Timestamp {
            prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: (us * 1_000) as i32,
            }
        }
        fn duration(us: i64) -> prost_types::Duration {
            prost_types::Duration {
                seconds: 0,
                nanos: (us * 1_000) as i32,
            }
        }
        fn start(data: Start) -> Event {
            Event::SpanStart(buck2_data::SpanStartEvent { data: Some(data) })
        }
        fn end(data: End, us: Option<i64>) -> Event {
            Event::SpanEnd(buck2_data::SpanEndEvent {
                data: Some(data),
                duration: us.map(duration),
                ..Default::default()
            })
        }
        fn local(argv: Option<&[&str]>) -> Event {
            start(Start::ExecutorStage(local_stage(argv.map(|args| {
                buck2_data::LocalCommand {
                    argv: args.iter().map(|s| (*s).to_owned()).collect(),
                    ..Default::default()
                }
            }))))
        }
        fn remote() -> Event {
            start(Start::ExecutorStage(buck2_data::ExecutorStageStart {
                stage: Some(buck2_data::executor_stage_start::Stage::Re(
                    buck2_data::ReStage {
                        stage: Some(buck2_data::re_stage::Stage::WorkerDownload(
                            Default::default(),
                        )),
                    },
                )),
            }))
        }

        let mut bytes = buck2_data::Invocation {
            trace_id: Some(crate::ingest::testlog::RUN_ID.to_owned()),
            start_time: Some(timestamp(0)),
            ..Default::default()
        }
        .encode_length_delimited_to_vec();
        let events = vec![
            (0, 100, 0, start(Start::ActionExecution(Default::default()))),
            (
                1,
                10,
                100,
                start(Start::Load(buck2_data::LoadBuildFileStart {
                    module_id: "phase".to_owned(),
                    ..Default::default()
                })),
            ),
            (2, 20, 10, local(Some(&["old"]))),
            (3, 20, 10, local(Some(&["new"]))),
            (4, 21, 10, local(Some(&["mismatched"]))),
            (5, 21, 10, end(End::ReUpload(Default::default()), Some(1))),
            (
                6,
                21,
                10,
                end(End::ExecutorStage(Default::default()), Some(2)),
            ),
            (7, 22, 10, remote()),
            (8, 23, 10, remote()),
            (9, 24, 10, local(None)),
            (
                10,
                23,
                10,
                end(End::SpanCancelled(Default::default()), Some(-1)),
            ),
            (
                10,
                24,
                10,
                end(End::SpanCancelled(Default::default()), Some(0)),
            ),
            (11, 25, 10, local(Some(&[]))),
            (
                12,
                25,
                10,
                end(End::ExecutorStage(Default::default()), Some(1)),
            ),
            (13, 26, 10, start(Start::ReUpload(Default::default()))),
            (
                14,
                26,
                10,
                end(End::ExecutorStage(Default::default()), Some(1)),
            ),
            (15, 26, 10, end(End::ReUpload(Default::default()), Some(2))),
            (16, 27, 10, local(Some(&["open-at-eof"]))),
            (
                20,
                22,
                10,
                end(End::SpanCancelled(Default::default()), None),
            ),
            (21, 10, 100, end(End::Load(Default::default()), Some(20))),
            (30, 28, 10, local(Some(&["after-owner-end"]))),
            (
                40,
                20,
                10,
                end(End::ExecutorStage(Default::default()), Some(37)),
            ),
            (
                50,
                100,
                0,
                end(End::ActionExecution(Default::default()), Some(50)),
            ),
            (
                60,
                28,
                10,
                end(End::ExecutorStage(Default::default()), Some(30)),
            ),
        ];
        for (us, sid, parent, data) in events {
            buck2_cli_proto::CommandProgress {
                progress: Some(buck2_cli_proto::command_progress::Progress::Event(
                    Box::new(buck2_data::BuckEvent {
                        timestamp: Some(timestamp(us)),
                        trace_id: crate::ingest::testlog::RUN_ID.to_owned(),
                        span_id: sid,
                        parent_id: parent,
                        data: Some(data),
                    }),
                )),
            }
            .encode_length_delimited(&mut bytes)
            .unwrap();
        }
        buck2_cli_proto::CommandProgress {
            progress: Some(buck2_cli_proto::command_progress::Progress::Result(
                Box::default(),
            )),
        }
        .encode_length_delimited(&mut bytes)
        .unwrap();
        let dir = std::env::temp_dir().join(format!(
            "retrace-compact-stage-lifecycle-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.pb");
        std::fs::write(&path, bytes).unwrap();
        let log = crate::ingest::infer(&path).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for phases in [false, true] {
            for interface in [0, 4, 6, 7] {
                let scan = runtime
                    .block_on(fixture_interface_scan(&log, phases, interface))
                    .unwrap();
                assert_eq!(scan.actions.len(), 1);
                assert_eq!(scan.stages.len(), 1);
                assert_eq!(scan.phases.len(), usize::from(phases));
                assert!(scan.command_result_seen);
                let stages = &scan.stages[&100];
                let actual: Vec<_> = stages
                    .iter()
                    .map(|s| {
                        (
                            s.name,
                            s.ts - ts_us(&timestamp(0)),
                            s.dur,
                            s.cancelled,
                            s.local_command().map(|c| c.argv.clone()),
                        )
                    })
                    .collect();
                assert_eq!(
                    actual,
                    vec![
                        ("Local.Execute", 9, 0, true, None),
                        ("Local.Execute", 11, 1, false, Some(vec![])),
                        ("Re.WorkerDownload", 7, 13, true, None),
                        ("Local.Execute", 3, 37, false, Some(vec!["new".to_owned()])),
                    ]
                );
                assert!(stages.iter().all(|s| !matches!(
                    s.details,
                    Some(StageDetails::Upload(_) | StageDetails::Submit(_))
                )));
            }
        }
    }

    #[test]
    fn dense_parent_index_keeps_sparse_links_during_growth_and_root_overwrites() {
        let mut links = DenseParentIndex::new(32, 2);
        links.insert(0, 0);
        links.insert(8, 77);
        assert_eq!(links.dense.len(), 1);
        assert_eq!(links.sparse.get(&8), Some(&77));
        for sid in 0..8 {
            links.insert(sid, sid + 100);
        }
        // A nearby out-of-order insert covers 8 without actually overwriting
        // it: the new zero-filled dense slot must still consult the old map.
        links.insert(10, 110);
        assert_eq!(links.dense.len(), 11);
        assert_eq!(links.dense[8], 0);
        assert_eq!(links.get(8), 77);
        links.insert(8, 0);
        assert_eq!(links.get(8), 0);
        assert!(!links.sparse.contains_key(&8));
        links.insert(8, u64::MAX);
        assert_eq!(links.get(8), u64::MAX);
        links.insert(8, 0);
        assert_eq!(links.get(8), 0);
        assert_eq!(links.get(0), 100, "span ID zero is still an ordinary key");
        links.insert(0, 0);
        assert_eq!(links.get(0), 0);

        links.insert(u64::MAX, u64::MAX);
        assert_eq!(links.get(u64::MAX), u64::MAX);
        links.insert(u64::MAX, 0);
        assert_eq!(links.get(u64::MAX), 0);
        assert_eq!(links.get(u64::MAX - 1), 0);
        assert!(links.dense.capacity() <= 32);
    }

    #[test]
    fn dense_parent_index_never_grows_for_large_or_over_budget_ids() {
        let mut links = DenseParentIndex::new(DENSE_PARENT_MAX_ENTRIES, DENSE_PARENT_MAX_GAP);
        links.insert(0, 0);
        for sid in [
            u64::MAX,
            u64::MAX - 1,
            DENSE_PARENT_MAX_ENTRIES as u64,
            (DENSE_PARENT_MAX_ENTRIES - 1) as u64,
            (DENSE_PARENT_MAX_GAP + 2) as u64,
        ] {
            links.insert(sid, 123);
            assert_eq!(links.get(sid), 123);
            assert_eq!(links.dense.len(), 1);
        }
        links.insert(DENSE_PARENT_MAX_GAP as u64, 456);
        assert_eq!(links.dense.len(), DENSE_PARENT_MAX_GAP + 1);
        assert_eq!(links.get(DENSE_PARENT_MAX_GAP as u64), 456);
        assert!(links.dense.capacity() <= DENSE_PARENT_MAX_ENTRIES);

        // Exercise the actual ceiling with a small test budget, not a 256MiB
        // unit-test allocation. ID max_entries itself must remain sparse.
        let mut links = DenseParentIndex::new(8, 8);
        links.insert(0, 0);
        links.insert(7, u64::MAX);
        assert_eq!(links.dense.len(), 8);
        assert!(links.dense.capacity() <= 8);
        links.insert(8, 7);
        assert_eq!(links.get(8), 7);
        assert_eq!(links.dense.len(), 8);
        assert_eq!(links.sparse.len(), 1);
    }

    #[test]
    fn dense_parent_index_rebases_on_warm_daemon_ids() {
        let mut links = DenseParentIndex::new(128, 4);
        for sid in 20_000_000..20_000_100 {
            links.insert(sid, sid - 1);
            assert_eq!(links.get(sid), sid - 1);
        }
        assert_eq!(links.dense.len(), 100);
        assert!(links.sparse.is_empty());
        links.insert(1, 2);
        assert_eq!(links.get(1), 2);
        assert_eq!(links.dense.len(), 100);
    }

    #[test]
    fn dense_parent_index_matches_hash_for_mixed_out_of_order_updates() {
        let mut dense = DenseParentIndex::new(128, 4);
        let mut hash = HashMap::new();
        let mut seed = 0x938f_5072_13aa_29c1_u64;
        for step in 0..2_048_u64 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let sid = match step % 5 {
                0 => step / 5,
                1 => u64::MAX - (seed % 8),
                _ => seed % 256,
            };
            let parent = match step % 7 {
                0 => 0,
                1 => u64::MAX,
                2 => sid,
                _ => (seed >> 32) % 256,
            };
            hash.insert(sid, parent);
            dense.insert(sid, parent);
            for query in (0..512).chain((u64::MAX - 8)..=u64::MAX) {
                assert_eq!(
                    dense.get(query),
                    hash.get(&query).copied().unwrap_or(0),
                    "step {step}, ID {query}"
                );
            }
        }
        assert!(dense.dense.capacity() <= 128);
    }

    #[test]
    fn parent_index_preserves_stage_ownership_cycles_and_exact_hop_limit() {
        fn owner(parent: &DenseParentIndex, sid: u64, live: &[u64]) -> Option<u64> {
            let actions = live
                .iter()
                .map(|&id| (id, (0, buck2_data::ActionExecutionStart::default())))
                .collect();
            let mut stages = HashMap::new();
            attribute_stage(
                &mut stages,
                parent,
                &actions,
                sid,
                StageRec {
                    name: "Re.WorkerDownload",
                    ts: 100,
                    dur: 200,
                    cancelled: true,
                    details: None,
                    cache_timings_invalid: false,
                },
            );
            assert!(stages.len() <= 1);
            if let Some(records) = stages.values().next() {
                assert_eq!(records.len(), 1);
                assert!(records[0].cancelled);
                assert_eq!((records[0].ts, records[0].dur), (100, 200));
            }
            stages.into_keys().next()
        }
        // Independent reference retaining the original Option-valued hash
        // lookup and walk, rather than sharing the dense lookup implementation.
        fn hash_owner(parent: &HashMap<u64, u64>, sid: u64, live: &[u64]) -> Option<u64> {
            let mut cur = sid;
            for _ in 0..PARENT_HOP_CAP {
                let &p = parent.get(&cur)?;
                if p == 0 {
                    return None;
                }
                if live.contains(&p) {
                    return Some(p);
                }
                cur = p;
            }
            None
        }
        fn check(
            dense: &DenseParentIndex,
            hash: &HashMap<u64, u64>,
            sid: u64,
            live: &[u64],
            expected: Option<u64>,
        ) {
            assert_eq!(hash_owner(hash, sid, live), expected);
            assert_eq!(owner(dense, sid, live), expected);
        }
        let mut dense = DenseParentIndex::new(128, 4);
        let mut hash = HashMap::new();
        // Deliberately insert descendants before their parents, then grow
        // across one sparse link and explicitly migrate another.
        for (sid, parent) in [
            (10, 11),
            (11, 12),
            (12, 100),
            (0, 100),
            (1, u64::MAX),
            (u64::MAX, u64::MAX - 1),
            (70, 71),
            (71, 70),
            (80, 80),
            (90, 0),
            (3, 0),
            (7, 0),
            (11, 12),
        ] {
            dense.insert(sid, parent);
            hash.insert(sid, parent);
        }
        check(&dense, &hash, 10, &[100], Some(100));
        check(&dense, &hash, 10, &[11, 100], Some(11));
        check(&dense, &hash, 0, &[100], Some(100));
        check(&dense, &hash, 1, &[u64::MAX], Some(u64::MAX));
        check(&dense, &hash, u64::MAX, &[u64::MAX - 1], Some(u64::MAX - 1));
        check(&dense, &hash, 90, &[0], None); // root stops before live-action lookup
        check(&dense, &hash, 91, &[100], None);
        check(&dense, &hash, 70, &[100], None);
        check(&dense, &hash, 80, &[100], None);
        dense.insert(11, 0);
        hash.insert(11, 0);
        check(&dense, &hash, 10, &[100], None);

        for hop in 0..=PARENT_HOP_CAP as u64 {
            dense.insert(1_000 + hop, 1_001 + hop);
            hash.insert(1_000 + hop, 1_001 + hop);
        }
        check(
            &dense,
            &hash,
            1_000,
            &[1_000 + PARENT_HOP_CAP as u64],
            Some(1_000 + PARENT_HOP_CAP as u64),
        );
        check(&dense, &hash, 1_000, &[1_001 + PARENT_HOP_CAP as u64], None);
    }

    #[test]
    fn cancelled_stage_requires_a_real_end_and_never_fabricates_completion_payloads() {
        for kind in [StageStartKind::ReUpload, StageStartKind::ReSubmit] {
            let s = cancelled_stage(100, 200, Some(100), kind).unwrap();
            assert!(s.cancelled);
            assert_eq!((s.ts, s.dur), (100, 100));
            assert!(!matches!(
                s.details,
                Some(StageDetails::Upload(_) | StageDetails::Submit(_))
            ));
        }
        assert_eq!(
            cancelled_stage(100, 200, None, StageStartKind::ReSubmit)
                .unwrap()
                .dur,
            100,
            "ordered recorded endpoints supply elapsed time when duration is absent"
        );
        assert_eq!(
            cancelled_stage(100, 100, Some(0), StageStartKind::ReSubmit)
                .unwrap()
                .dur,
            0,
            "an explicit measured zero is not an invented completion"
        );
        assert!(cancelled_stage(200, 100, None, StageStartKind::ReSubmit).is_none());
        assert!(cancelled_stage(100, 200, Some(-1), StageStartKind::ReSubmit).is_none());
        assert!(cancelled_stage(100, 200, Some(i64::MAX), StageStartKind::ReSubmit).is_none());
        assert!(cancelled_stage(i64::MIN, i64::MAX, None, StageStartKind::ReSubmit).is_none());
    }

    #[test]
    fn remote_reuse_requires_affirmative_provenance() {
        use buck2_data::RemoteCommandOrigin;
        for (cache_hit, cache_hit_type, reused) in [
            (true, RemoteCommandOrigin::ActionCache, true),
            (false, RemoteCommandOrigin::RemoteDepFileCache, true),
            (false, RemoteCommandOrigin::ReServedFromCache, true),
            (false, RemoteCommandOrigin::ActionCache, false),
            (false, RemoteCommandOrigin::Executed, false),
            // a deduplicated execution ran, shared with another build
            (false, RemoteCommandOrigin::ReDeduplicated, false),
        ] {
            let end = buck2_data::ActionExecutionEnd {
                execution_kind: buck2_data::ActionExecutionKind::Remote as i32,
                commands: vec![buck2_data::CommandExecution {
                    details: Some(buck2_data::CommandExecutionDetails {
                        command_kind: Some(buck2_data::CommandExecutionKind {
                            command: Some(
                                buck2_data::command_execution_kind::Command::RemoteCommand(
                                    buck2_data::RemoteCommand {
                                        cache_hit,
                                        cache_hit_type: cache_hit_type as i32,
                                        ..Default::default()
                                    },
                                ),
                            ),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let action = project_action(
                1,
                0,
                100,
                &buck2_data::ActionExecutionStart::default(),
                &end,
                &mut Interners::default(),
            );
            assert_eq!(action.last.reused, reused);
        }
    }

    fn label(package: &str, name: &str) -> buck2_data::TargetLabel {
        buck2_data::TargetLabel {
            package: package.to_owned(),
            name: name.to_owned(),
        }
    }

    fn configured(package: &str, name: &str) -> buck2_data::ConfiguredTargetLabel {
        buck2_data::ConfiguredTargetLabel {
            label: Some(label(package, name)),
            configuration: None,
            execution_configuration: None,
        }
    }

    fn anon(package: &str, name: &str) -> buck2_data::AnonTarget {
        buck2_data::AnonTarget {
            name: Some(label(package, name)),
            execution_configuration: None,
            hash: "deadbeef".to_owned(),
        }
    }

    fn pat(v: &str) -> buck2_data::TargetPattern {
        buck2_data::TargetPattern {
            value: v.to_owned(),
        }
    }

    #[test]
    fn analysis_names_every_target_kind() {
        use buck2_data::analysis_start::Target;
        use buck2_data::dynamic_lambda_owner::Owner;

        assert_eq!(
            analysis_detail(Some(Target::StandardTarget(configured("fbcode//a", "b")))),
            "fbcode//a:b"
        );
        assert_eq!(
            analysis_detail(Some(Target::AnonTarget(anon("fbcode//a", "b")))),
            "fbcode//a:b (anon)"
        );
        assert_eq!(
            analysis_detail(Some(Target::DynamicLambda(
                buck2_data::DynamicLambdaOwner {
                    owner: Some(Owner::TargetLabel(configured("fbcode//a", "b"))),
                }
            ))),
            "fbcode//a:b (dynamic)"
        );
        assert_eq!(
            analysis_detail(Some(Target::DynamicLambda(
                buck2_data::DynamicLambdaOwner {
                    owner: Some(Owner::AnonTarget(anon("fbcode//a", "b"))),
                }
            ))),
            "fbcode//a:b (dynamic)"
        );
        assert_eq!(
            analysis_detail(Some(Target::DynamicLambda(
                buck2_data::DynamicLambdaOwner {
                    owner: Some(Owner::BxlKey(buck2_data::BxlFunctionKey {
                        label: Some(buck2_data::BxlFunctionLabel {
                            bxl_path: "fbcode//a/b.bxl".to_owned(),
                            name: "f".to_owned(),
                        }),
                    })),
                }
            ))),
            "fbcode//a/b.bxl:f (dynamic)"
        );
    }

    #[test]
    fn analysis_of_an_unset_target_is_unnamed() {
        assert_eq!(analysis_detail(None), "");
    }

    #[test]
    fn command_end_yields_the_patterns_the_user_typed() {
        use buck2_data::command_end::Data;
        let build = buck2_data::CommandEnd {
            is_success: true,
            data: Some(Data::Build(buck2_data::BuildCommandEnd {
                unresolved_target_patterns: vec![pat("igios-no-extensions"), pat("")],
            })),
            ..Default::default()
        };
        // the empty pattern is dropped, not carried as a blank name
        assert_eq!(
            command_end_patterns(&build),
            vec!["igios-no-extensions".to_owned()]
        );

        let test = buck2_data::CommandEnd {
            is_success: true,
            data: Some(Data::Test(buck2_data::TestCommandEnd {
                unresolved_target_patterns: vec![pat("fbcode//a:b")],
            })),
            ..Default::default()
        };
        assert_eq!(command_end_patterns(&test), vec!["fbcode//a:b".to_owned()]);

        // a command kind with no patterns worth naming a capture after
        let clean = buck2_data::CommandEnd {
            is_success: true,
            data: Some(Data::Clean(Box::default())),
            ..Default::default()
        };
        assert!(command_end_patterns(&clean).is_empty());

        // and an old log whose CommandEnd carries no data at all
        assert!(command_end_patterns(&buck2_data::CommandEnd::default()).is_empty());
    }
}
