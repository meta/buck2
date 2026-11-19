/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Trace + graph emission from a [`Scan`] — the output half of the
//! converter port.  Every record shape, gating rule and refusal here
//! mirrors `trace-buck.py` (spec archived at
//! `trace-runs/ingest-2026-09-05/recon/spec-converter.md`) so the
//! emitted pair round-trips through `parse.rs`/`graph.rs` exactly the
//! way converter output always has.  Where the two differ: the trace
//! array is closed (valid JSON; the readers accept both), graph rows
//! use compact separators, and per-record key order is alphabetical —
//! all shapes the readers are order- and whitespace-insensitive to.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::BufRead as _;
use std::io::Seek as _;
use std::io::Write as _;
use std::path::Path;

use buck2_data::local_command::EnvironmentRule;
use serde_json::Value;
use serde_json::json;
use sha1::Digest as _;

use crate::ingest::Staged;
use crate::ingest::scan::Scan;
use crate::ingest::scan::StageDetails;
use crate::ingest::scan::StageRec;
use crate::ingest::scan::TsetDef;
use crate::types::CaptureState;
use crate::types::Finality;
use crate::types::Outcome;

/// Whether the graph half of the pair was written.
pub enum GraphOutcome {
    Written,
    /// the log carries nothing a graph can be built from
    NotWritten,
}

/// What one emission produced, for the CLI to print: the same status
/// lines the converter printed, plus any warnings (stderr material).
pub struct EmitSummary {
    /// Exact generation stamped by this emission, even without a graph.
    pub capture: String,
    pub status_lines: Vec<String>,
    pub warnings: Vec<String>,
    pub n_events: usize,
    pub n_cache_hits: usize,
    pub lanes: usize,
    pub graph: GraphOutcome,
    pub outcome: Outcome,
    pub dep_capture: CaptureState,
}

/// Bump the domain when the fingerprint's canonicalization changes.
/// The fixed-width zero stamp is hashed in place of the final stamp,
/// avoiding a circular hash. Every other emitted byte participates,
/// including graph edges, capture counts/finality and schema fields.
const CAPTURE_DOMAIN: &[u8] = b"retrace-artifact-pair-v1\0";
const CAPTURE_PLACEHOLDER: &str = "retrace-v1-0000000000000000000000000000000000000000";

struct ArtifactDigest {
    bytes: u64,
    digest: [u8; 20],
}

/// Hash while the buffered writer flushes; no second multi-GB read or
/// duplicate JSON serialization is needed to identify a generation.
struct HashedFile {
    file: std::fs::File,
    hash: sha1::Sha1,
    bytes: u64,
}

impl std::io::Write for HashedFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

const ARTIFACT_BUFFER_BYTES: usize = 1024 * 1024;

fn artifact_writer(path: &Path) -> anyhow::Result<std::io::BufWriter<HashedFile>> {
    artifact_writer_with_capacity(path, ARTIFACT_BUFFER_BYTES)
}

fn artifact_writer_with_capacity(
    path: &Path,
    capacity: usize,
) -> anyhow::Result<std::io::BufWriter<HashedFile>> {
    let file = super::private_file(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let writer = HashedFile {
        file,
        hash: sha1::Sha1::new(),
        bytes: 0,
    };
    Ok(std::io::BufWriter::with_capacity(capacity, writer))
}

fn finish_artifact(mut w: std::io::BufWriter<HashedFile>) -> anyhow::Result<ArtifactDigest> {
    w.flush()?;
    let w = w.into_inner().map_err(|e| e.into_error())?;
    Ok(ArtifactDigest {
        bytes: w.bytes,
        digest: w.hash.finalize().into(),
    })
}

fn capture_fingerprint(trace: &ArtifactDigest, graph: Option<&ArtifactDigest>) -> String {
    let mut h = sha1::Sha1::new();
    h.update(CAPTURE_DOMAIN);
    h.update(b"trace\0");
    h.update(trace.bytes.to_le_bytes());
    h.update(trace.digest);
    h.update(b"graph\0");
    h.update([u8::from(graph.is_some())]);
    if let Some(g) = graph {
        h.update(g.bytes.to_le_bytes());
        h.update(g.digest);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("retrace-v1-{hex}")
}

/// Only staged files are patched. A failed patch aborts emission, so
/// the staging guards remove both outputs and neither is published.
fn stamp_artifact(path: &Path, is_trace: bool, capture: &str) -> anyhow::Result<()> {
    anyhow::ensure!(capture.len() == CAPTURE_PLACEHOLDER.len());
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut r = std::io::BufReader::new(&mut f);
    let mut header = Vec::new();
    let mut offset = 0;
    if is_trace {
        offset = r.read_until(b'\n', &mut header)?;
        header.clear();
    }
    r.read_until(b'\n', &mut header)?;
    let needle = format!("\"capture\":\"{CAPTURE_PLACEHOLDER}\"");
    let at = header
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}: capture stamp missing from staged header",
                path.display()
            )
        })?;
    drop(r);
    let at = offset + at + "\"capture\":\"".len();
    f.seek(std::io::SeekFrom::Start(at as u64))?;
    f.write_all(capture.as_bytes())?;
    f.flush()?;
    Ok(())
}

const PID: i64 = 1;

fn exec_kind_name(kind: i32) -> String {
    match kind {
        1 => "local",
        2 => "remote",
        3 => "action_cache",
        4 => "simple",
        5 => "deferred_dep",
        6 => "deferred",
        7 => "local_dep_file",
        8 => "local_worker",
        9 => "remote_dep_file_cache",
        10 => "local_action_cache",
        11 => "remote_worker",
        12 => "remote_served_from_cache",
        13 => "remote_deduplicated",
        n => return format!("kind_{n}"),
    }
    .to_owned()
}

const CACHE_KINDS: [i32; 5] = [3, 7, 9, 10, 12];

fn action_kind_name(kind: i32) -> std::borrow::Cow<'static, str> {
    use std::borrow::Cow;
    Cow::Borrowed(match kind {
        0 => "",
        1 => "Copy",
        2 => "DownloadFile",
        3 => "Run",
        4 => "SymlinkedDir",
        5 => "Write",
        6 => "WriteMacrosToFile",
        7 => "CasArtifact",
        n => return Cow::Owned(format!("kind_{n}")),
    })
}

/// Sum of normally completed stages with a given name; `None` when no
/// stage matched (a matched sum of 0 is meaningful and distinct). Cancelled
/// intervals are retained separately, not passed off as completion totals.
fn stage_us(stages: &[StageRec], names: &[&str]) -> Option<i64> {
    let mut sum: i64 = 0;
    let mut seen = false;
    for s in stages {
        if names.contains(&s.name) && !s.cancelled && s.dur >= 0 {
            sum = sum.saturating_add(s.dur);
            seen = true;
        }
    }
    seen.then_some(sum)
}

/// Recorded client-clock spans, not worker timing or an additive stage
/// budget. Keep overlaps, zero durations and offsets outside the enclosing
/// action intact; consumers can clip for display without changing evidence.
/// The compact tuple is (name, offset_us, duration_us, cancelled). An empty
/// versioned array distinguishes a new capture with no closed observations
/// from an older conversion that did not export activity at all.
/// `rejected`, when present, counts invalid/unrepresentable StageRec entries,
/// not open spans: only recorded normal or cancellation ends reach this list.
fn action_activity(stages: &[StageRec], start: i64) -> Value {
    let mut spans = Vec::with_capacity(stages.len());
    let mut rejected = 0usize;
    for s in stages {
        let interval = (|| {
            if s.dur < 0 {
                return None;
            }
            let offset = s.ts.checked_sub(start)?;
            // Both the recorded and relative end must remain representable.
            // Do not saturate an invalid record into plausible activity.
            s.ts.checked_add(s.dur)?;
            offset.checked_add(s.dur)?;
            Some((s.name, offset, s.dur, s.cancelled))
        })();
        if let Some(span) = interval {
            spans.push(span);
        } else {
            rejected += 1;
        }
    }
    spans.sort_unstable_by_key(|&(name, offset, dur, cancelled)| (offset, name, dur, cancelled));
    let mut activity = serde_json::Map::new();
    activity.insert("version".to_owned(), json!(1));
    activity.insert("spans".to_owned(), json!(spans));
    if rejected != 0 {
        activity.insert("rejected".to_owned(), json!(rejected));
    }
    Value::Object(activity)
}

fn insert_nonzero(map: &mut serde_json::Map<String, Value>, key: &str, v: i64) {
    if v != 0 {
        map.insert(key.to_owned(), json!(v));
    }
}

/// A malformed or overflowing contribution invalidates that field's total,
/// not unrelated measurements. `None` is sticky: later valid values cannot
/// turn a partial total back into an apparently complete measurement.
fn add_counter(sums: &mut HashMap<&'static str, Option<i64>>, key: &'static str, value: u64) {
    let total = sums.entry(key).or_insert(Some(0));
    *total = total.and_then(|sum| sum.checked_add(i64::try_from(value).ok()?));
}

fn emit_counters(
    out: &mut serde_json::Map<String, Value>,
    sums: HashMap<&'static str, Option<i64>>,
    keep_zero: bool,
    invalid: &mut Vec<&'static str>,
) {
    for (key, total) in sums {
        match total {
            Some(total) if keep_zero || total != 0 => {
                out.insert(key.to_owned(), json!(total));
            }
            None => invalid.push(key),
            _ => {}
        }
    }
}

/// One trace event to be emitted: an action or a phase row, with its
/// lane assigned by the packer.  The event OBJECT is not built until
/// the moment it is written — 767k live `serde_json::Map`s cost 6 GiB
/// on a real capture, and reordering a parallel vector of them is how
/// the graph once mislabeled every row (coord L-122 §A/§B.4).  Lanes
/// live in this side table indexed by emission entry, so nothing ever
/// reorders per-action state.
struct EvSlot {
    ts: i64,
    dur: i64,
    recipe: bool,
    /// index into `scan.actions` (recipe) or the kept-phase list
    idx: usize,
    lane: usize,
}

/// Greedy min-lane packing over ts-sorted entries (stable order for
/// ties); fills `lane` in place and returns the lane count plus that same
/// order for emission. The slots themselves never move.
fn pack_lanes(slots: &mut [EvSlot]) -> (usize, Vec<usize>) {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut order: Vec<usize> = (0..slots.len()).collect();
    order.sort_by_key(|&i| slots[i].ts);
    let mut running: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
    let mut free: BinaryHeap<Reverse<usize>> = BinaryHeap::new();
    let mut used = 0;
    for &i in &order {
        while let Some(&Reverse((release, lane))) = running.peek() {
            if release <= slots[i].ts {
                running.pop();
                free.push(Reverse(lane));
            } else {
                break;
            }
        }
        let lane = match free.pop() {
            Some(Reverse(l)) => l,
            None => {
                used += 1;
                used - 1
            }
        };
        running.push(Reverse((
            slots[i].ts.saturating_add(slots[i].dur.max(1)),
            lane,
        )));
        slots[i].lane = lane;
    }
    (used, order)
}

fn merge_intervals(sorted: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &(s, e) in sorted {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn clipped_stage(s: &StageRec, start: i64, dur: i64) -> Option<(i64, i64)> {
    if s.dur < 0 || dur < 0 {
        return None;
    }
    // Match action_activity's representability checks before clipping. An
    // invalid stage or action end must not become plausible covered time.
    let stage_end = s.ts.checked_add(s.dur)?;
    let offset = s.ts.checked_sub(start)?;
    offset.checked_add(s.dur)?;
    let action_end = start.checked_add(dur)?;
    let from = s.ts.max(start);
    let to = stage_end.min(action_end);
    (to >= from).then_some((from, to))
}

/// An observed client-state interval union, including recorded cancellation
/// ends. This is not worker CPU, physical transfer time, or a success claim.
fn stage_union_us(
    stages: &[StageRec],
    start: i64,
    dur: i64,
    include: impl Fn(&StageRec) -> bool,
) -> Option<i64> {
    let mut matching = stages
        .iter()
        .filter(|s| include(s))
        .filter_map(|s| clipped_stage(s, start, dur));
    let first = matching.next()?;
    let Some(second) = matching.next() else {
        // Prepare and cache handling usually have just one interval. Neither
        // that common case nor absence should allocate a temporary collection.
        return Some(first.1.saturating_sub(first.0));
    };
    let mut intervals = vec![first, second];
    intervals.extend(matching);
    intervals.sort_unstable();
    let mut end = intervals[0].0;
    let mut total = 0i64;
    for (start, stop) in intervals {
        total = total.saturating_add(stop.saturating_sub(start.max(end)).max(0));
        end = end.max(stop);
    }
    Some(total)
}

/// Per-action subtotals from recorded cache-phase payloads, not a disjoint
/// decomposition of the parent span. Missing payloads/phases stay visible through
/// coverage; overflow poisons only its field. Capacities must agree, never sum.
fn cache_diagnostics<const N: usize>(
    stages: &[StageRec],
    name: &str,
    keys: [&str; N],
    fields: impl Fn(&StageDetails) -> Option<[Option<u64>; N]>,
) -> Option<Value> {
    cache_diagnostics_for(stages.iter().filter(|s| s.name == name), keys, fields)
}

fn cache_diagnostics_for<'a, const N: usize>(
    stages: impl Iterator<Item = &'a StageRec>,
    keys: [&str; N],
    fields: impl Fn(&StageDetails) -> Option<[Option<u64>; N]>,
) -> Option<Value> {
    let mut records = 0usize;
    let mut measured = 0usize;
    let mut sums = [Some(0i64); N];
    let mut counts = [0usize; N];
    for stage in stages {
        records += 1;
        if stage.cancelled || stage.cache_timings_invalid || stage.dur < 0 {
            continue;
        }
        let Some(values) = stage.details.as_ref().and_then(&fields) else {
            continue;
        };
        measured += 1;
        for (i, value) in values.into_iter().enumerate() {
            let Some(value) = value else { continue };
            counts[i] += 1;
            sums[i] = sums[i].and_then(|sum| sum.checked_add(i64::try_from(value).ok()?));
        }
    }
    if measured == 0 {
        return None;
    }
    let mut result = serde_json::Map::new();
    result.insert("records".to_owned(), json!(records));
    result.insert("measured".to_owned(), json!(measured));
    let mut partial = serde_json::Map::new();
    let mut invalid = Vec::new();
    for (i, key) in keys.into_iter().enumerate() {
        if counts[i] == 0 {
            continue;
        }
        match sums[i] {
            Some(sum) => {
                result.insert(key.to_owned(), json!(sum));
                if counts[i] < measured {
                    partial.insert(key.to_owned(), json!(counts[i]));
                }
            }
            None => invalid.push(key),
        }
    }
    if !partial.is_empty() {
        result.insert("field_records".to_owned(), Value::Object(partial));
    }
    if !invalid.is_empty() {
        result.insert("invalid_fields".to_owned(), json!(invalid));
    }
    Some(Value::Object(result))
}

/// These are local validation observations, not RE lookups. A LIVE result
/// identifies the entry's source; it does not imply the whole check was free.
fn local_cache_diagnostics(stages: &[StageRec]) -> Option<Value> {
    let local = || {
        stages
            .iter()
            .filter(|s| super::scan::is_local_cache_stage(s.name))
    };
    let mut result = cache_diagnostics_for(
        local(),
        [
            "persisted_probe_us",
            "persisted_fetch_us",
            "persisted_fetches",
        ],
        |details| match details {
            StageDetails::LocalCacheOutcome(_) => Some([None; 3]),
            StageDetails::LocalCacheTimings(end) => Some([
                end.persisted_probe_us,
                end.persisted_fetch_us,
                end.persisted_fetches,
            ]),
            _ => None,
        },
    )?;
    let mut outcomes = std::collections::BTreeMap::<i32, usize>::new();
    for stage in local().filter(|s| !s.cancelled && s.dur >= 0 && !s.cache_timings_invalid) {
        let outcome = match stage.details.as_ref() {
            Some(StageDetails::LocalCacheOutcome(outcome)) => *outcome,
            Some(StageDetails::LocalCacheTimings(end)) => end.outcome,
            _ => continue,
        };
        *outcomes.entry(outcome).or_default() += 1;
    }
    result["outcomes"] = json!(outcomes.into_iter().collect::<Vec<_>>());
    Some(result)
}

/// Current local execution activity is independent of the final result's
/// provenance: a cache winner can follow a cancelled local attempt. Count
/// only the union of captured local execution spans inside this action,
/// not the cached worker timing or RE progress/wait spans. This is an
/// observed client interval, not CPU service time or an attempt census.
fn local_execution_us(stages: &[StageRec], start: i64, dur: i64) -> Option<i64> {
    stage_union_us(stages, start, dur, |s| {
        matches!(s.name, "Local.Execute" | "Local.WorkerExecute")
    })
}

#[cfg(test)]
mod execution_tests {
    use super::*;
    use crate::ingest::testlog::WaitStateCase;

    fn stage(name: &'static str, ts: i64, dur: i64) -> StageRec {
        StageRec {
            name,
            ts,
            dur,
            cancelled: false,
            details: None,
            cache_timings_invalid: false,
        }
    }

    #[test]
    fn cache_stage_totals_recover_old_logs_and_round_trip_new_diagnostics() {
        for timings in [false, true] {
            let dir = std::env::temp_dir().join(format!(
                "retrace-cache-stages-{}-{timings}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = crate::ingest::testlog::cache_stage_log(&dir, timings);
            let compressed = crate::ingest::testlog::zstd_copy(&path, true);
            let log = crate::ingest::infer(&compressed).unwrap();
            let scan = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(crate::ingest::scan::scan(&log))
                .unwrap();
            let args = fixture_args(&scan);
            assert_eq!(args["stages"]["prepare_us"], 5);
            assert_eq!(args["stages"]["cache_query_us"], 300);
            assert_eq!(args["stages"]["cache_hit_us"], 190);
            assert_eq!(args["activity"]["spans"].as_array().unwrap().len(), 3);
            if timings {
                assert_eq!(
                    args["re"]["cache_query"],
                    json!({
                        "records": 1, "measured": 1, "permit_wait_us": 0, "sdk_us": 250
                    })
                );
                assert_eq!(
                    args["re"]["cache_hit"],
                    json!({
                        "records": 1, "measured": 1, "tree_fetch_us": 5, "local_bookkeeping_us": 10,
                        "declare_us": 150, "tree_count": 2
                    })
                );
            } else {
                assert!(args.get("re").and_then(|r| r.get("cache_query")).is_none());
                assert!(args.get("re").and_then(|r| r.get("cache_hit")).is_none());
            }
        }
    }

    #[test]
    fn local_cache_union_outcomes_and_persisted_details_remain_distinct_from_remote() {
        let dir = crate::temp_path("local-cache-stage-emit");
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::ingest::testlog::local_cache_stage_log(&dir);
        let log = crate::ingest::infer(&path).unwrap();
        let scan = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(crate::ingest::scan::scan(&log))
            .unwrap();
        let args = fixture_args(&scan);
        assert_eq!(args["stages"]["local_cache_check_us"], 70);
        assert!(args["stages"].get("cache_query_us").is_none());
        assert_eq!(
            args["re"]["local_cache"],
            json!({
                "records": 5, "measured": 4,
                "persisted_probe_us": 0, "persisted_fetch_us": 5, "persisted_fetches": 2,
                "field_records": {"persisted_probe_us": 1, "persisted_fetch_us": 1, "persisted_fetches": 1},
                "outcomes": [[1,1], [2,1], [3,1], [42,1]],
            })
        );
        let activity = crate::activity::Activity::from_record(&json!({
            "ts": scan.actions[0].ts, "dur": 100, "args": args,
        }));
        assert_eq!(activity.covered_us, Some(100));
        assert_eq!(activity.spans.len(), 6);
        assert_eq!(local_cache_diagnostics(&[]), None);
        let mut invalid = stage("LocalCache.CheckIdenticalAction", 0, 100);
        invalid.details = Some(StageDetails::LocalCacheTimings(Box::new(
            buck2_data::MatchDepFilesEnd {
                outcome: 0,
                persisted_probe_us: Some(u64::MAX),
                persisted_fetch_us: Some(0),
                persisted_fetches: None,
            },
        )));
        let detail = local_cache_diagnostics(&[invalid]).unwrap();
        assert_eq!(detail["invalid_fields"], json!(["persisted_probe_us"]));
        assert_eq!(detail["persisted_fetch_us"], 0);
        assert_eq!(detail["outcomes"], json!([[0, 1]]));
    }

    #[test]
    fn a_fully_covered_local_cache_check_keeps_its_only_diagnostics() {
        let (mut scan, _) = wait_fixture("local-only-full-coverage", WaitStateCase::RemoteRetry);
        scan.actions[0].ts = 100;
        scan.actions[0].execution_kind = 10;
        scan.actions[0].last = Default::default();
        scan.actions[0].cmds.clear();
        for duration in [0, 100] {
            scan.actions[0].dur = duration;
            for persisted in [false, true] {
                let mut check = stage("LocalCache.CheckIdenticalAction", 100, duration);
                check.details = Some(if persisted {
                    StageDetails::LocalCacheTimings(Box::new(buck2_data::MatchDepFilesEnd {
                        outcome: 3,
                        persisted_probe_us: Some(0),
                        persisted_fetch_us: Some(0),
                        persisted_fetches: Some(0),
                    }))
                } else {
                    StageDetails::LocalCacheOutcome(2)
                });
                scan.stages.insert(scan.actions[0].sid, vec![check]);
                let args = fixture_args(&scan);
                let expected = if persisted {
                    json!({
                        "records": 1, "measured": 1, "outcomes": [[3, 1]],
                        "persisted_probe_us": 0, "persisted_fetch_us": 0,
                        "persisted_fetches": 0,
                    })
                } else {
                    json!({"records": 1, "measured": 1, "outcomes": [[2, 1]]})
                };
                assert_eq!(args["re"], json!({"local_cache": expected}));
                assert_eq!(args["stages"]["local_cache_check_us"], duration);
            }
        }
        scan.stages.get_mut(&scan.actions[0].sid).unwrap().clear();
        let args = fixture_args(&scan);
        assert!(args.get("re").is_none());
        assert!(args["stages"].get("local_cache_check_us").is_none());
    }

    #[test]
    fn cas_artifact_phases_and_finalization_preserve_union_and_missingness() {
        let (mut scan, _) = wait_fixture("cas-finalize", WaitStateCase::RemoteRetry);
        scan.actions[0].ts = 100;
        scan.actions[0].dur = 100;
        let mut cancelled = stage("CasArtifact.ExtendExpiration", 120, 30);
        cancelled.cancelled = true;
        scan.stages.insert(
            10,
            vec![
                stage("CasArtifact.CheckExpiration", 80, 50),
                cancelled,
                stage("CasArtifact.RecheckExpiration", 150, 10),
                stage("CasArtifact.TreeMetadata", 160, 20),
                stage("CasArtifact.DeclareOutputs", 180, 40),
                stage("Local.FinalizeOutputs", 100, 0),
            ],
        );
        let args = fixture_args(&scan);
        assert_eq!(args["stages"]["cas_artifact_us"], 100);
        assert_eq!(args["stages"]["local_finalize_outputs_us"], 0);
        assert!(
            args["activity"]["spans"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s[0] == "CasArtifact.ExtendExpiration" && s[3] == true)
        );
        scan.stages.insert(
            10,
            vec![
                stage("CasArtifact.Unknown", 100, 100),
                stage("Local.FinalizeOutputs", 100, -1),
            ],
        );
        let args = fixture_args(&scan);
        assert!(args["stages"].get("cas_artifact_us").is_none());
        assert!(args["stages"].get("local_finalize_outputs_us").is_none());
        assert_eq!(args["activity"]["spans"][0][0], "CasArtifact.Unknown");
    }

    #[test]
    fn local_hash_source_preserves_raw_sum_without_double_counting_parent() {
        let (mut scan, _) = wait_fixture("finalize-hash", WaitStateCase::RemoteRetry);
        let a = &mut scan.actions[0];
        a.ts = 100;
        a.dur = 100;
        a.execution_kind = 1;
        a.last.remote = false;
        a.last.reused = false;
        a.last.hashing_us = Some(180);
        a.last.hashing_local_outputs = true;
        scan.stages
            .insert(10, vec![stage("Local.FinalizeOutputs", 100, 100)]);
        let args = fixture_args(&scan);
        assert_eq!(args["stages"]["hashing_us"], 180);
        assert_eq!(args["stages"]["hashing_source"], "local_outputs");
        assert_eq!(args["stages"]["local_finalize_outputs_us"], 100);
        assert_eq!(
            crate::detail::stages_of(&args).unwrap()[crate::detail::STAGE_HASH_IDX],
            0
        );
        scan.actions[0].last.hashing_local_outputs = false;
        let args = fixture_args(&scan);
        assert!(args["stages"].get("hashing_source").is_none());
        assert_eq!(
            crate::detail::stages_of(&args).unwrap()[crate::detail::STAGE_HASH_IDX],
            180
        );
        scan.actions[0].last.hashing_local_outputs = true;
        scan.actions[0].last.reused = true;
        let args = fixture_args(&scan);
        assert!(args["stages"].get("hashing_source").is_none());
        assert!(args["stages"].get("hashing_us").is_none());
        assert_eq!(args["historical_execution"]["hashing_us"], 180);
    }

    #[test]
    fn cache_and_prepare_totals_clip_union_and_keep_zero_and_cancellation() {
        let (mut scan, _) = wait_fixture("cache-union", WaitStateCase::RemoteRetry);
        scan.actions[0].ts = 100;
        scan.actions[0].dur = 100;
        let mut cancelled = stage("CacheHit", 140, 100);
        cancelled.cancelled = true;
        scan.stages.insert(
            10,
            vec![
                stage("CacheHit", 50, 100),
                stage("CacheHit", 110, 20),
                cancelled,
                stage("Prepare", 100, 0),
            ],
        );
        let args = fixture_args(&scan);
        assert_eq!(args["stages"]["cache_hit_us"], 100);
        assert_eq!(args["stages"]["prepare_us"], 0);
        scan.stages.insert(
            10,
            vec![stage("CacheHit", 201, 10), stage("Prepare", 100, -1)],
        );
        let args = fixture_args(&scan);
        assert!(args["stages"].get("cache_hit_us").is_none());
        assert!(args["stages"].get("prepare_us").is_none());
        assert_eq!(stage_union_us(&[], 100, 100, |_| true), None);
    }

    #[test]
    fn clipped_stage_rejects_overflow_and_preserves_boundary_zero() {
        for name in ["CacheHit", "Prepare"] {
            let absolute_overflow = stage(name, i64::MAX - 20, 100);
            assert_eq!(clipped_stage(&absolute_overflow, i64::MAX - 10, 10), None);
            assert_eq!(
                stage_union_us(&[absolute_overflow], i64::MAX - 10, 10, |_| true),
                None
            );
            let valid = stage(name, i64::MAX - 4, 1);
            assert_eq!(
                clipped_stage(&valid, i64::MAX - 10, 20),
                None,
                "an overflowing action end is not a valid clipping window"
            );
            let relative_overflow = stage(name, 0, i64::MAX);
            assert_eq!(clipped_stage(&relative_overflow, -10, 100), None);
            assert_eq!(action_activity(&[relative_overflow], -10)["rejected"], 1);
            let zero = stage(name, i64::MAX, 0);
            assert_eq!(
                clipped_stage(&zero, i64::MAX - 10, 10),
                Some((i64::MAX, i64::MAX))
            );
            assert_eq!(
                stage_union_us(&[zero], i64::MAX - 10, 10, |_| true),
                Some(0),
                "a valid measured zero at the boundary is not missing"
            );
            assert_eq!(
                stage_union_us(&[stage(name, i64::MAX - 20, 20)], i64::MAX - 10, 10, |_| {
                    true
                }),
                Some(10),
                "valid intervals still clip normally near the timestamp limit"
            );
        }
    }

    #[test]
    fn cache_diagnostics_preserve_partial_fields_and_reject_overflow() {
        let query = |wait: u64, lookup: u64| {
            let mut s = stage("CacheQuery", 0, 100);
            s.details = Some(StageDetails::CacheQuery(Box::new(
                buck2_data::CacheQueryTimings {
                    slot_wait_us: wait,
                    remote_lookup_us: lookup,
                },
            )));
            s
        };
        let summarize = |stages: &[StageRec]| {
            cache_diagnostics(
                stages,
                "CacheQuery",
                ["permit_wait_us", "sdk_us"],
                |d| match d {
                    StageDetails::CacheQuery(q) => {
                        Some([Some(q.slot_wait_us), Some(q.remote_lookup_us)])
                    }
                    _ => None,
                },
            )
            .unwrap()
        };
        let mut cancelled = stage("CacheQuery", 0, 10);
        cancelled.cancelled = true;
        let stages = [query(0, 15), query(5, 0), cancelled];
        assert_eq!(
            summarize(&stages),
            json!({"records": 3, "measured": 2, "permit_wait_us": 5, "sdk_us": 15})
        );
        let invalid = summarize(&[query(i64::MAX as u64, 0), query(1, 2)]);
        assert_eq!(invalid["sdk_us"], 2);
        assert!(invalid.get("permit_wait_us").is_none());
        assert_eq!(invalid["invalid_fields"], json!(["permit_wait_us"]));
        let huge = summarize(&[query(u64::MAX, 0)]);
        assert_eq!(huge["invalid_fields"], json!(["permit_wait_us"]));

        let hits: Vec<_> = [Some(0), Some(2), None]
            .into_iter()
            .map(|fetch| {
                let mut s = stage("CacheHit", 0, 10);
                s.details = Some(StageDetails::CacheHit(Box::new(
                    buck2_data::CacheHitTimings {
                        tree_fetch_us: fetch,
                        ..Default::default()
                    },
                )));
                s
            })
            .collect();
        let hit = cache_diagnostics(&hits, "CacheHit", ["tree_fetch_us"], |d| match d {
            StageDetails::CacheHit(h) => Some([h.tree_fetch_us]),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            hit,
            json!({
                "records": 3, "measured": 3, "tree_fetch_us": 2,
                "field_records": {"tree_fetch_us": 2}
            })
        );
    }

    #[test]
    fn omitted_local_argv_requires_unambiguous_same_digest_stage_evidence() {
        use crate::ingest::scan::LastCmd;
        use crate::ingest::scan::LocalCommandRec;
        let local = |digest: &str, argv: &[&str]| {
            let mut s = stage("Local.Execute", 5, 50);
            s.details = Some(StageDetails::LocalCommand(Box::new(LocalCommandRec {
                digest: Some(digest.to_owned()),
                argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            })));
            s
        };
        let last = LastCmd {
            local_digest: Some("selected:148".to_owned()),
            ..Default::default()
        };
        let mut starts = vec![
            local("other:148", &["wrong"]),
            local("selected:148", &["python3.12", "@args"]),
            local("selected:148", &["python3.12", "@args"]),
        ];
        assert_eq!(selected_argv(&last, &starts), ["python3.12", "@args"]);
        assert!(
            selected_argv(&LastCmd::default(), &starts).is_empty(),
            "unknown report cannot borrow an arbitrary earlier local command"
        );
        let remote = LastCmd {
            argv: Some(vec!["<remote>".to_owned(), "remote:148".to_owned()]),
            remote: true,
            ..Default::default()
        };
        assert_eq!(selected_argv(&remote, &starts), ["<remote>", "remote:148"]);
        starts.push(local("selected:148", &["different"]));
        assert!(selected_argv(&last, &starts).is_empty());
        assert!(selected_argv(&last, &[local("other:148", &["wrong"])]).is_empty());
    }

    #[test]
    fn upload_outcomes_are_sparse_raw_enums_and_independent_of_cache_hits() {
        let (mut scan, _) = wait_fixture(
            "upload-outcome",
            WaitStateCase::LocalWinner {
                download_end: false,
            },
        );
        // A client upload disposition must not create a cache-hit claim.
        assert!(fixture_args(&scan).get("cache").is_none());
        for (cache, dep_file) in [(0, 0), (6, 1), (1, 16), (i32::MAX, -1)] {
            scan.actions[0].cache_upload_result = cache;
            scan.actions[0].dep_file_cache_upload_result = dep_file;
            let args = fixture_args(&scan);
            for (key, expected) in [
                ("cache_upload_result", cache),
                ("dep_file_cache_upload_result", dep_file),
            ] {
                if expected == 0 {
                    assert!(args.get(key).is_none(), "{key}");
                } else {
                    assert_eq!(args[key], json!(expected), "{key}");
                }
            }
            assert!(args.get("cache").is_none());
        }
    }

    #[test]
    fn action_execution_evidence_emits_only_a_small_store_reference() {
        let (mut scan, _) = wait_fixture(
            "execution-reference",
            WaitStateCase::LocalWinner {
                download_end: false,
            },
        );
        let reference = crate::execution::DetailRef {
            offset: 512,
            len: 123,
        };
        scan.actions[0].execution = Some(reference);
        let args = fixture_args(&scan);
        assert_eq!(
            args["execution"],
            json!({"schema": 1, "reference": reference})
        );
        assert!(
            args.get("stdout").is_none()
                && args.get("stderr").is_none()
                && args.get("env").is_none()
        );
        scan.actions[0].execution = None;
        assert!(fixture_args(&scan).get("execution").is_none());
    }

    #[test]
    fn activity_preserves_zero_duplicate_overlapping_and_outside_spans_in_stable_order() {
        let mut cancelled = stage("Re.WorkerDownload", 100, 100);
        cancelled.cancelled = true;
        let mut cancelled_tie = stage("Local.Execute", 110, 5);
        cancelled_tie.cancelled = true;
        let mut stages = vec![
            stage("Local.Execute", 110, 20),
            cancelled,
            stage("Prepare", 100, 0),
            stage("Future.Stage", 90, 0),
            cancelled_tie,
            stage("Local.Execute", 110, 5),
            stage("Prepare", 100, 0),
        ];
        let expected = json!({"version": 1, "spans": [
            ["Future.Stage", -10, 0, false],
            ["Prepare", 0, 0, false],
            ["Prepare", 0, 0, false],
            ["Re.WorkerDownload", 0, 100, true],
            ["Local.Execute", 10, 5, false],
            ["Local.Execute", 10, 5, true],
            ["Local.Execute", 10, 20, false],
        ]});
        assert_eq!(action_activity(&stages, 100), expected);
        stages.reverse();
        assert_eq!(action_activity(&stages, 100), expected);
        assert_eq!(
            action_activity(&[], 100),
            json!({"version": 1, "spans": []})
        );
        let many: Vec<_> = (0..1_025).map(|i| stage("Re.Queue", i, 0)).collect();
        assert_eq!(
            action_activity(&many, 0)["spans"].as_array().unwrap().len(),
            many.len(),
            "recorded spans must not be silently capped"
        );
    }

    #[test]
    fn activity_counts_invalid_intervals_instead_of_saturating_them() {
        let stages = [
            stage("Prepare", 0, -1),
            stage("Re.Queue", i64::MAX, 0), // relative start overflow
            stage("Re.Queue", i64::MAX - 20, 15), // relative end overflow
            stage("Re.Queue", i64::MAX - 20, 21), // absolute end overflow
            stage("Prepare", 0, 0),
        ];
        assert_eq!(
            action_activity(&stages, -10),
            json!({"version": 1, "spans": [["Prepare", 10, 0, false]], "rejected": 4})
        );
        assert_eq!(
            action_activity(&[stage("Prepare", i64::MIN, 0)], 1),
            json!({"version": 1, "spans": [], "rejected": 1})
        );
        assert_eq!(
            action_activity(&[stage("Prepare", i64::MAX - 1, 1)], i64::MAX - 2),
            json!({"version": 1, "spans": [["Prepare", 1, 1, false]]})
        );
    }

    #[test]
    fn current_local_execution_is_clipped_and_unioned() {
        let stages = [
            stage("Local.Execute", 80, 50),
            stage("Local.WorkerExecute", 120, 40),
            stage("Local.Execute", 150, 100),
            stage("Local.Execute", 300, 50),
            stage("Local.Execute", 100, -1),
        ];
        assert_eq!(local_execution_us(&stages, 100, 100), Some(100));
        assert_eq!(
            local_execution_us(
                &[stage("Local.Execute", i64::MAX - 20, 100)],
                i64::MAX - 10,
                10
            ),
            None
        );
    }

    #[test]
    fn remote_progress_and_absent_local_execution_are_not_current_local_service() {
        assert_eq!(local_execution_us(&[], 0, 100), None);
        assert_eq!(
            local_execution_us(&[stage("Re.Execute", 0, 100)], 0, 100),
            None
        );
        assert_eq!(
            local_execution_us(&[stage("Local.Execute", 10, 0)], 0, 100),
            Some(0),
            "a captured zero-length span is different from no span"
        );
    }

    fn wait_fixture(name: &str, case: WaitStateCase) -> (Scan, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("retrace-wait-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::ingest::testlog::wait_state_log(&dir, case);
        let log = crate::ingest::infer(&path).unwrap();
        let scan = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(crate::ingest::scan::scan(&log))
            .unwrap();
        (scan, dir)
    }

    fn fixture_args(scan: &Scan) -> Value {
        let a = &scan.actions[0];
        let ctx = EventCtx {
            root: "/repo",
            run: &scan.trace_id,
            cfgs: &scan.cfg_names,
            categories: &scan.category_names,
        };
        action_event(a, scan.stages.get(&a.sid).unwrap(), &ctx, "fixture", 1, 0)
            .remove("args")
            .unwrap()
    }

    #[test]
    fn recorded_preparation_attempts_and_local_environment_are_emitted() {
        let (mut scan, _) = wait_fixture(
            "recorded-extras",
            WaitStateCase::LocalWinner { download_end: true },
        );
        let a = &mut scan.actions[0];
        a.prepare_us = Some(400);
        a.finish_us = Some(600);
        a.last.attempts = Some(3);
        a.last.final_queue_us = Some(2_000);
        a.last.cwd = Some("fbcode".to_owned());
        a.last.env_rule = buck2_data::local_command::EnvironmentRule::TestAllowlist as i32;
        let args = fixture_args(&scan);
        assert_eq!(
            (
                &args["re"]["prepare_command_us"],
                &args["re"]["finish_action_us"]
            ),
            (&json!(400), &json!(600))
        );
        assert_eq!(
            (
                &args["re"]["execution_attempts"],
                &args["re"]["final_queue_us"]
            ),
            (&json!(3), &json!(2_000))
        );
        assert_eq!(
            (&args["cwd"], &args["env_rule"]),
            (&json!("fbcode"), &json!("test_allowlist"))
        );
    }

    #[test]
    fn cancelled_worker_download_is_observed_coverage_not_transfer_or_daemon_cpu() {
        let (scan, _) = wait_fixture(
            "cancelled",
            WaitStateCase::LocalWinner { download_end: true },
        );
        assert!(scan.incomplete.is_none());
        assert_eq!(
            scan.actions[0].cmds.len(),
            1,
            "the remote loser is not a recorded command attempt"
        );
        let download = scan.stages[&10]
            .iter()
            .find(|s| s.name == "Re.WorkerDownload")
            .unwrap();
        assert!(download.cancelled);
        assert_eq!(download.dur, 579_439_598);
        assert!(!matches!(
            download.details,
            Some(StageDetails::Upload(_) | StageDetails::Submit(_))
        ));
        let args = fixture_args(&scan);
        assert_eq!(args["span_id"], 10);
        assert_eq!(
            args["activity"],
            json!({"version": 1, "spans": [
                ["Prepare", 1_109, 15, false],
                ["CacheQuery", 1_131, 96_313, false],
                ["ReUpload", 97_473, 518_509, false],
                ["ReSubmit", 616_017, 2_414_923, false],
                ["Re.Queue", 3_030_953, 118_342, false],
                ["Re.WorkerDownload", 3_149_358, 579_439_598, true],
                ["Local.MaterializeInputs", 551_891_486, 11_200_744, false],
                    ["Local.PrepareOutputDirs", 563_092_354, 9_495_270, false],
                ["Local.Execute", 572_587_641, 2_809_091, false],
            ]})
        );
        assert_eq!(args["re"]["worker_download_wait_us"], 579_439_598i64);
        assert_eq!(
            args["re"]["worker_download_cancelled_wait_us"],
            579_439_598i64
        );
        assert_eq!(
            args["re"]["cancelled_stages"],
            json!([{
                "name": "Re.WorkerDownload", "ts": download.ts, "dur": 579_439_598i64,
            }])
        );
        assert_eq!(args["re"]["input_fetch_us"], 11_200_743);
        assert_eq!(args["stages"]["materialize_us"], 11_200_743);
        assert_eq!(args["stages"]["execute_us"], 2_809_086);
        // The canceled remote state overlaps all local work. Count its
        // interval once for coverage, not as another transfer budget.
        assert_eq!(args["stages"]["daemon_us"], 2_070);
        assert_eq!(args["re"]["gap_pre_us"], 1_109);
        assert_eq!(args["re"]["gap_inter_us"], 147);
        assert_eq!(args["re"]["gap_post_us"], 814);
    }

    #[test]
    fn an_open_worker_state_is_not_synthetically_closed_at_action_end_or_eof() {
        let (scan, _) = wait_fixture(
            "open",
            WaitStateCase::LocalWinner {
                download_end: false,
            },
        );
        assert!(scan.incomplete.is_some());
        assert!(
            scan.stages[&10]
                .iter()
                .all(|s| s.name != "Re.WorkerDownload")
        );
        let args = fixture_args(&scan);
        assert_eq!(args["activity"]["version"], 1);
        assert!(
            args["activity"]["spans"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s[0] != "Re.WorkerDownload"),
            "open spans do not gain an invented cancellation or completion"
        );
        assert!(args["re"].get("worker_download_wait_us").is_none());
        assert!(
            args["re"]
                .get("worker_download_cancelled_wait_us")
                .is_none()
        );
        assert!(args["re"].get("cancelled_stages").is_none());
        assert!(args["stages"]["daemon_us"].as_i64().unwrap() > 500_000_000);
        assert_eq!(args["stages"]["materialize_us"], 11_200_743);
    }

    #[test]
    fn client_queue_excludes_prior_failed_worker_and_preserves_zero_or_absence() {
        let (mut scan, _) = wait_fixture("retry", WaitStateCase::RemoteRetry);
        let args = fixture_args(&scan);
        assert_eq!(args["span_id"], 10);
        assert_eq!(
            args["activity"],
            json!({"version": 1, "spans": [
                ["Re.Queue", 36_140_238, 94_947, false],
                ["Re.WorkerDownload", 36_236_107, 1_577_896_801i64, false],
                ["Re.Queue", 1_614_132_913i64, 11_788, false],
                ["Re.WorkerDownload", 1_614_144_765i64, 850_932, false],
                ["Re.WorkerUpload", 1_621_027_080i64, 1_208_061, false],
            ]})
        );
        assert_eq!(args["re"]["queue_us"], 106_730);
        assert_eq!(args["stages"]["queue_us"], 106_730);
        assert_eq!(
            args["re"]["reported_first_enqueue_to_worker_us"],
            1_577_954_967i64
        );
        assert_eq!(args["re"]["worker_download_wait_us"], 1_578_747_733i64);
        assert!(
            args["re"]
                .get("worker_download_cancelled_wait_us")
                .is_none()
        );
        assert_eq!(args["stages"]["materialize_us"], 850_809);
        assert_eq!(args["stages"]["execute_us"], 6_029_947);

        scan.actions[0].last.remote_queue_us = Some(0);
        let args = fixture_args(&scan);
        assert_eq!(args["re"]["queue_us"], 0);
        assert_eq!(args["stages"]["queue_us"], 0, "measured zero is not absent");

        scan.actions[0].last.remote_queue_us = None;
        assert_eq!(
            fixture_args(&scan)["re"]["queue_us"],
            106_735,
            "the observed queue union is a fallback, not the historical scheduling interval"
        );
        scan.stages
            .get_mut(&10)
            .unwrap()
            .retain(|s| s.name != "Re.Queue");
        let args = fixture_args(&scan);
        assert!(args["re"].get("queue_us").is_none());
        assert!(args["stages"].get("queue_us").is_none());
        assert_eq!(
            args["re"]["reported_first_enqueue_to_worker_us"],
            1_577_954_967i64
        );
    }

    #[test]
    fn reused_worker_timings_stay_historical_while_current_state_observations_survive() {
        let (mut scan, _) = wait_fixture("reused", WaitStateCase::RemoteRetry);
        let last = &mut scan.actions[0].last;
        last.reused = true;
        last.upload_us = Some(1_144_839);
        last.worker_wall_us = Some(8_089_110);
        last.hashing_us = Some(123);
        last.worker_bytes_in = Some(4096);
        last.worker_bytes_out = Some(512);
        let args = fixture_args(&scan);
        assert_eq!(args["execution_metadata"], "reused");
        for (key, expected) in [
            ("exec_us", 6_029_947),
            ("input_fetch_us", 850_809),
            ("server_upload_us", 1_144_839),
            ("worker_wall_us", 8_089_110),
            ("hashing_us", 123),
            ("worker_bytes_in", 4096),
            ("worker_bytes_out", 512),
        ] {
            assert_eq!(args["historical_execution"][key], expected);
            assert!(args["re"].get(key).is_none());
        }
        assert!(args["stages"].get("materialize_us").is_none());
        assert!(args["stages"].get("hashing_us").is_none());
        assert_eq!(
            args["re"]["queue_us"], 106_730,
            "client state is current even for a reused result"
        );
        assert_eq!(args["re"]["worker_download_wait_us"], 1_578_747_733i64);
        assert_eq!(
            args["activity"],
            action_activity(&scan.stages[&10], scan.actions[0].ts),
            "reused worker metadata does not hide current client activity"
        );
        scan.stages.get_mut(&10).unwrap().push(stage(
            "Local.MaterializeInputs",
            scan.actions[0].ts,
            100,
        ));
        assert_eq!(fixture_args(&scan)["stages"]["materialize_us"], 100);
        assert_eq!(
            fixture_args(&scan)["stages"]["materialize_source"],
            "client_span"
        );
    }

    #[test]
    fn worker_upload_is_not_reported_as_client_result_processing() {
        let (mut scan, _) = wait_fixture("worker-upload-provenance", WaitStateCase::RemoteRetry);
        let observed = stage_union_us(
            &scan.stages[&10],
            scan.actions[0].ts,
            scan.actions[0].dur,
            |stage| stage.name == "Re.WorkerUpload",
        )
        .unwrap();
        assert!(observed > 0);
        for measured in [Some(0), Some(700_000), None] {
            scan.actions[0].last.upload_us = measured;
            let args = fixture_args(&scan);
            assert_eq!(args["re"]["worker_upload_wait_us"], observed);
            assert_eq!(args["stages"]["upload_us"], measured.unwrap_or(observed));
            assert_eq!(
                args["stages"]["upload_source"],
                if measured.is_some() {
                    "worker"
                } else {
                    "client_state"
                }
            );
            assert!(args["stages"].get("result_wait_us").is_none());
            assert!(args["re"].get("result_wait_us").is_none());
            assert_eq!(
                args["activity"],
                action_activity(&scan.stages[&10], scan.actions[0].ts)
            );
        }
        scan.actions[0].last.reused = true;
        scan.actions[0].last.upload_us = Some(123);
        let args = fixture_args(&scan);
        assert_eq!(args["historical_execution"]["server_upload_us"], 123);
        assert_eq!(args["stages"]["upload_us"], observed);
        assert_eq!(args["stages"]["upload_source"], "client_state");

        scan.stages
            .get_mut(&10)
            .unwrap()
            .retain(|stage| stage.name != "Re.WorkerUpload");
        let args = fixture_args(&scan);
        assert!(args["stages"].get("upload_us").is_none());
        assert!(args["stages"].get("result_wait_us").is_none());
    }

    #[test]
    fn malformed_upload_and_submit_totals_are_omitted_without_erasing_valid_fields() {
        let (mut scan, _) = wait_fixture("malformed-counters", WaitStateCase::RemoteRetry);
        let mut stages = Vec::new();
        for value in [i64::MAX as u64, 1, 7] {
            let mut upload = stage("ReUpload", scan.actions[0].ts, 1);
            upload.details = Some(StageDetails::Upload(Box::new(buck2_data::ReUploadEnd {
                stats: Some(buck2_data::ReUploadStats {
                    slot_wait_us: value,
                    materializer_query_us: 2,
                    materialize_us: 3,
                    cas_write_us: 4,
                    digests_queried: 0,
                    ..Default::default()
                }),
                bytes_uploaded: Some(u64::MAX),
                ..Default::default()
            })));
            stages.push(upload);
            let mut submit = stage("ReSubmit", scan.actions[0].ts, 1);
            submit.details = Some(StageDetails::Submit(Box::new(buck2_data::ReSubmitEnd {
                slot_wait_us: value,
                request_prep_us: 3,
                ..Default::default()
            })));
            stages.push(submit);
        }
        scan.stages.insert(10, stages);
        let args = fixture_args(&scan);
        let re = &args["re"];
        assert_eq!(re["iu_cas_call_us"], 12);
        assert_eq!(
            (&re["iu_materialize_us"], &re["iu_materializer_query_us"]),
            (&json!(15), &json!(6))
        );
        assert_eq!(re["iu_digests_queried"], 0);
        assert_eq!(re["rs_request_prep_us"], 9);
        assert!(re.get("iu_sem_wait_us").is_none());
        assert!(re.get("iu_bytes_uploaded").is_none());
        assert!(re.get("rs_exec_sem_wait_us").is_none());
        assert_eq!(
            re["invalid_counter_fields"],
            json!(["iu_bytes_uploaded", "iu_sem_wait_us", "rs_exec_sem_wait_us"])
        );
    }

    #[test]
    fn cancelled_upload_and_submit_do_not_fabricate_normal_completion_totals() {
        for name in ["ReUpload", "ReSubmit"] {
            let mut cancelled = stage(name, 50, 200);
            cancelled.cancelled = true;
            let stages = [cancelled];
            assert_eq!(stage_us(&stages, &[name]), None);
            assert_eq!(stage_union_us(&stages, 100, 100, |_| true), Some(100));
        }
        assert_eq!(
            stage_union_us(
                &[stage("Re.Queue", i64::MAX - 20, 100)],
                i64::MAX - 10,
                10,
                |_| true
            ),
            None,
            "an overflowing stage end is rejected, not clipped into plausible coverage"
        );
        assert_eq!(
            stage_union_us(&[stage("Re.Queue", 100, -1)], 0, 100, |_| true),
            None
        );
    }

    #[test]
    fn emit_summary_binds_the_exact_no_graph_capture() {
        let (scan, dir) = wait_fixture("summary-capture", WaitStateCase::RemoteRetry);
        let trace = Staged::beside(dir.join("trace.json"));
        let summary = emit(scan, &trace, None, false).unwrap();
        let rows: Value = serde_json::from_slice(&std::fs::read(&trace.write_to).unwrap()).unwrap();
        assert_eq!(rows[0]["args"]["capture"], summary.capture);
        assert!(matches!(summary.graph, GraphOutcome::NotWritten));
    }

    #[test]
    fn activity_and_action_identity_are_present_without_stages_or_commands() {
        let (mut scan, _) = wait_fixture("empty-activity", WaitStateCase::RemoteRetry);
        scan.stages.get_mut(&10).unwrap().clear();
        scan.actions[0].cmds.clear();
        scan.actions[0].sid = u64::MAX;
        let ctx = EventCtx {
            root: "/repo",
            run: &scan.trace_id,
            cfgs: &scan.cfg_names,
            categories: &scan.category_names,
        };
        let event = action_event(&scan.actions[0], &[], &ctx, "fixture", 7, 0);
        let args = &event["args"];
        assert_eq!(args["activity"], json!({"version": 1, "spans": []}));
        assert_eq!(args["span_id"].as_u64(), Some(u64::MAX));
        assert_eq!(
            args["serial"], "7",
            "display serial is not the span identity"
        );
    }

    #[test]
    fn final_command_timing_keeps_its_recorded_status_even_when_not_a_success() {
        let (mut scan, _) = wait_fixture("final-status", WaitStateCase::RemoteRetry);
        scan.actions[0].cmds.insert(
            0,
            crate::ingest::scan::CmdRec {
                status: "WorkerFailure",
                wall_time_us: Some(90),
                start_us: Some(10),
                client_start_us: Some(20),
                elastic: false,
            },
        );
        for status in [
            "Success",
            "Failure",
            "Timeout",
            "Error",
            "Cancelled",
            "Unknown",
        ] {
            let final_command = scan.actions[0].cmds.last_mut().unwrap();
            final_command.status = status;
            final_command.client_start_us = Some(120);
            final_command.wall_time_us = Some(30);
            let args = fixture_args(&scan);
            assert_eq!(
                args["winner"],
                json!({"status": status, "client_start": 120, "dur_us": 30})
            );
            assert_eq!(args["attempts"][0]["status"], "WorkerFailure");
        }
        let final_command = scan.actions[0].cmds.last_mut().unwrap();
        final_command.client_start_us = None;
        final_command.wall_time_us = None;
        assert_eq!(
            fixture_args(&scan)["winner"],
            json!({"status": "Unknown"}),
            "a recorded outcome must not require fabricated command boundaries"
        );
    }
}

/// `LocalCommand.environment_rule` as `args.env_rule`, or `None` when no rule applied.
fn env_rule_name(rule: i32) -> Option<&'static str> {
    match EnvironmentRule::try_from(rule).ok()? {
        EnvironmentRule::Unspecified => None,
        EnvironmentRule::InheritExceptExclusions => Some("inherit_except_exclusions"),
        EnvironmentRule::TestAllowlist => Some("test_allowlist"),
        EnvironmentRule::Cleared => Some("cleared"),
    }
}

/// The final command's measured exit/signal, independent of action failure
/// (for example, a successful command can fail to produce required outputs).
fn exit_sig(a: &crate::ingest::scan::ActionRec) -> (Option<i64>, Option<i64>) {
    match a.last.signed_exit_code {
        None => (None, None),
        // negate AFTER widening: i32::MIN has no i32 negation
        Some(sec) if sec < 0 => (Some(0), Some(-i64::from(sec))),
        Some(sec) => (Some(i64::from(sec)), Some(0)),
    }
}

/// The run-level values every action record repeats, resolved once.
struct EventCtx<'a> {
    root: &'a str,
    run: &'a str,
    cfgs: &'a [String],
    categories: &'a [String],
}

/// Recover an omitted local command only from same-kind, same-digest starts
/// that agree on argv. Resource-control restarts can produce several starts
/// for one report; temporal adjacency and start/report indices do not pair
/// them. Unknown/remote reports must not borrow an earlier local command.
fn selected_argv<'a>(
    last: &'a crate::ingest::scan::LastCmd,
    stages: &'a [StageRec],
) -> &'a [String] {
    if let Some(argv) = &last.argv {
        return argv;
    }
    let Some(digest) = last.local_digest.as_deref().filter(|d| !d.is_empty()) else {
        return &[];
    };
    let mut selected: Option<&[String]> = None;
    for stage in stages {
        let Some(command) = stage.local_command() else {
            continue;
        };
        if stage.name != "Local.Execute" || command.digest.as_deref() != Some(digest) {
            continue;
        }
        let argv = command.argv.as_slice();
        if argv.is_empty() {
            continue;
        }
        if selected.is_some_and(|previous| previous != argv) {
            return &[];
        }
        selected = Some(argv);
    }
    selected.unwrap_or(&[])
}

/// One finished action -> one `cat:"recipe"` X record, fully stamped
/// (name, serial, lane).
fn action_event(
    a: &crate::ingest::scan::ActionRec,
    stages: &[StageRec],
    ctx: &EventCtx<'_>,
    name: &str,
    serial: usize,
    lane: usize,
) -> serde_json::Map<String, Value> {
    let last = &a.last;
    let (exit, sig) = exit_sig(a);

    let argv = selected_argv(last, stages);

    let ek = a.execution_kind;
    let reused_execution = CACHE_KINDS.contains(&ek) || last.reused;
    let cfg = &ctx.cfgs[a.cfg as usize];
    let mut args = serde_json::Map::new();
    args.insert("target".to_owned(), json!(a.full_target));
    args.insert("src".to_owned(), json!(ctx.categories[a.category as usize]));
    args.insert("argv".to_owned(), json!(argv));
    if let Some(cwd) = &last.cwd {
        args.insert("cwd".to_owned(), json!(cwd));
    }
    if let Some(rule) = env_rule_name(last.env_rule) {
        args.insert("env_rule".to_owned(), json!(rule));
    }
    args.insert("dir".to_owned(), json!(ctx.root));
    args.insert("exit".to_owned(), json!(exit));
    args.insert("sig".to_owned(), json!(sig));
    args.insert("makelevel".to_owned(), json!(0));
    args.insert("recurse".to_owned(), json!(false));
    args.insert("host".to_owned(), json!("buck2"));
    args.insert("v".to_owned(), json!(1));
    args.insert("run".to_owned(), json!(ctx.run));
    args.insert("serial".to_owned(), json!(serial.to_string()));
    // The source ActionExecution identity, independent of display ordering.
    args.insert("span_id".to_owned(), json!(a.sid));
    if a.failed {
        args.insert("failed".to_owned(), json!(true));
        args.insert("error_kind".to_owned(), json!(a.error_kind));
    }
    if let Some(reference) = a.execution {
        args.insert(
            "execution".to_owned(),
            json!({"schema": 1, "reference": reference}),
        );
    }
    args.insert("activity".to_owned(), action_activity(stages, a.ts));
    args.insert("slot".to_owned(), json!(lane));
    args.insert("kind".to_owned(), json!(exec_kind_name(ek)));
    args.insert("rule".to_owned(), json!(action_kind_name(a.kind)));
    // Keep the producer enum, not a guess from allows_cache_upload. Zero has
    // no reason/presence information in older logs. Nonzero unknown values
    // survive so newer producers do not silently become "not uploaded".
    for (key, outcome) in [
        ("cache_upload_result", a.cache_upload_result),
        (
            "dep_file_cache_upload_result",
            a.dep_file_cache_upload_result,
        ),
    ] {
        if outcome != 0 {
            args.insert(key.to_owned(), json!(outcome));
        }
    }
    if !cfg.is_empty() {
        args.insert("cfg".to_owned(), json!(cfg));
    }
    if a.invalidated {
        args.insert(
            "why".to_owned(),
            json!("inputs changed (buck2 invalidation tracking)"),
        );
    }
    if let Some(w) = a.wall_time_us {
        args.insert("wall_time_us".to_owned(), json!(w));
    }
    if reused_execution {
        args.insert("execution_metadata".to_owned(), json!("reused"));
        let historical: serde_json::Map<String, Value> = [
            ("exec_us", last.exec_us),
            ("input_fetch_us", last.input_fetch_us),
            ("server_upload_us", last.upload_us),
            ("worker_wall_us", last.worker_wall_us),
            ("hashing_us", last.hashing_us),
            ("worker_bytes_in", last.worker_bytes_in.map(|b| b as i64)),
            ("worker_bytes_out", last.worker_bytes_out.map(|b| b as i64)),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.map(|v| (key.to_owned(), json!(v))))
        .collect();
        if !historical.is_empty() {
            args.insert("historical_execution".to_owned(), Value::Object(historical));
        }
    } else if let Some(x) = last.exec_us {
        args.insert("exec_us".to_owned(), json!(x));
    }
    if let Some(peak) = last.mem_peak {
        if peak != 0 {
            args.insert("maxrss".to_owned(), json!(peak));
        }
    }
    for (key, v) in [
        ("cpu_us", last.cpu_us),
        ("cpu_user_us", last.cpu_user_us),
        ("cpu_sys_us", last.cpu_sys_us),
    ] {
        if let Some(v) = v.filter(|_| !reused_execution) {
            if v != 0 {
                args.insert(key.to_owned(), json!(v));
            }
        }
    }
    if last.elastic && !reused_execution {
        args.insert("elastic".to_owned(), json!(true));
    }
    if let Some(d) = &a.digest {
        args.insert("digest".to_owned(), json!(d));
    }

    if a.cmds.len() > 1 {
        args.insert("tries".to_owned(), json!(a.cmds.len()));
        let mut attempts = Vec::new();
        let mut statuses = Vec::new();
        for c in &a.cmds[..a.cmds.len() - 1] {
            statuses.push(c.status.to_owned());
            let mut at = serde_json::Map::new();
            at.insert("status".to_owned(), json!(c.status));
            if let Some(w) = c.wall_time_us {
                at.insert("dur_us".to_owned(), json!(w));
            }
            if let Some(st) = c.start_us {
                at.insert("start".to_owned(), json!(st));
            }
            if let Some(cs) = c.client_start_us {
                at.insert("client_start".to_owned(), json!(cs));
            }
            if c.elastic {
                at.insert("elastic".to_owned(), json!(true));
            }
            attempts.push(Value::Object(at));
        }
        args.insert("attempts".to_owned(), Value::Array(attempts));
        args.insert("attempt_status".to_owned(), json!(statuses));
    }
    // Keep the final command's boundaries and recorded outcome together.
    // The legacy `winner` key does not guarantee success. Action-level
    // `wall_time_us` is not this command's interval.
    if let Some(winner) = a.cmds.last() {
        let mut timing = serde_json::Map::new();
        timing.insert("status".to_owned(), json!(winner.status));
        if let Some(cs) = winner.client_start_us {
            timing.insert("client_start".to_owned(), json!(cs));
        }
        if let Some(dur) = winner.wall_time_us {
            timing.insert("dur_us".to_owned(), json!(dur));
        }
        args.insert("winner".to_owned(), Value::Object(timing));
    }

    let cache_query = stage_us(stages, &["CacheQuery"]);
    if CACHE_KINDS.contains(&ek) {
        let mut cache = serde_json::Map::new();
        cache.insert("hit".to_owned(), json!(true));
        cache.insert("kind".to_owned(), json!(exec_kind_name(ek)));
        cache.insert("bytes".to_owned(), json!(a.output_size));
        if let Some(q) = cache_query {
            cache.insert("lookup_us".to_owned(), json!(q));
        }
        args.insert("cache".to_owned(), Value::Object(cache));
    }

    // --- args.re ------------------------------------------------
    let mut re = serde_json::Map::new();
    if let Some(local_cache) = local_cache_diagnostics(stages) {
        re.insert("local_cache".to_owned(), local_cache);
    }
    if let Some(query) = cache_diagnostics(
        stages,
        "CacheQuery",
        ["permit_wait_us", "sdk_us"],
        |details| match details {
            StageDetails::CacheQuery(q) => Some([Some(q.slot_wait_us), Some(q.remote_lookup_us)]),
            _ => None,
        },
    ) {
        re.insert("cache_query".to_owned(), query);
    }
    if let Some(hit) = cache_diagnostics(
        stages,
        "CacheHit",
        [
            "tree_admission_us",
            "tree_fetch_us",
            "local_bookkeeping_us",
            "declare_us",
            "tree_count",
        ],
        |details| match details {
            StageDetails::CacheHit(h) => Some([
                h.tree_admission_us,
                h.tree_fetch_us,
                h.local_bookkeeping_us,
                h.declare_us,
                Some(h.tree_count),
            ]),
            _ => None,
        },
    ) {
        re.insert("cache_hit".to_owned(), hit);
    }
    let invalid_cache_timings = stages.iter().filter(|s| s.cache_timings_invalid).count();
    if invalid_cache_timings != 0 {
        re.insert(
            "invalid_cache_timing_records".to_owned(),
            json!(invalid_cache_timings),
        );
    }
    // RemoteCommand.queue_time measures this client's QUEUED state.
    // Metadata.queue_duration spans FIRST enqueue -> final worker start:
    // earlier worker failures/retries/backoff can dominate that interval.
    // Preserve it as a receipt, never as a measured queue budget.
    let queue = last
        .remote_queue_us
        .filter(|q| *q >= 0)
        .or_else(|| stage_union_us(stages, a.ts, a.dur, |s| s.name == "Re.Queue"));
    if let Some(q) = queue {
        re.insert("queue_us".to_owned(), json!(q));
    }
    if let Some(q) = last.queue_us.filter(|_| last.remote) {
        re.insert("reported_first_enqueue_to_worker_us".to_owned(), json!(q));
    }
    if let Some(n) = last.attempts {
        re.insert("execution_attempts".to_owned(), json!(n));
    }
    if let Some(us) = last.final_queue_us {
        re.insert("final_queue_us".to_owned(), json!(us));
    }
    for (key, cancelled_only) in [
        ("worker_download_wait_us", false),
        ("worker_download_cancelled_wait_us", true),
    ] {
        if let Some(us) = stage_union_us(stages, a.ts, a.dur, |s| {
            s.name == "Re.WorkerDownload" && (!cancelled_only || s.cancelled)
        }) {
            re.insert(key.to_owned(), json!(us));
        }
    }
    let cancelled: Vec<_> = stages
        .iter()
        .filter(|s| s.cancelled)
        .filter_map(|s| {
            clipped_stage(s, a.ts, a.dur)
                .map(|(ts, end)| json!({"name": s.name, "ts": ts, "dur": end.saturating_sub(ts)}))
        })
        .collect();
    if !cancelled.is_empty() {
        re.insert("cancelled_stages".to_owned(), json!(cancelled));
    }
    // WorkerDownload is a client-observed remote progress state, which
    // may outlive a failed fetch and overlap local fallback. It is not a
    // missing worker transfer measurement and must not be added to the
    // successful worker's input-fetch timing or promoted from a cache hit.
    let worker_fetch = last.input_fetch_us.filter(|f| !reused_execution && *f >= 0);
    let fetch = worker_fetch.or_else(|| {
        stage_union_us(stages, a.ts, a.dur, |s| {
            s.name == "Local.MaterializeInputs" && !s.cancelled
        })
    });
    if let Some(f) = fetch {
        re.insert("input_fetch_us".to_owned(), json!(f));
    }
    if let Some(x) = last.exec_us.filter(|_| !reused_execution) {
        re.insert("exec_us".to_owned(), json!(x));
    }
    if let Some(u) = last.upload_us.filter(|_| !reused_execution) {
        re.insert("server_upload_us".to_owned(), json!(u));
    }
    if let Some(w) = last.worker_wall_us.filter(|_| !reused_execution) {
        re.insert("worker_wall_us".to_owned(), json!(w));
    }
    for (key, bytes) in [
        ("worker_bytes_in", last.worker_bytes_in),
        ("worker_bytes_out", last.worker_bytes_out),
    ] {
        if let Some(b) = bytes.filter(|_| !reused_execution) {
            re.insert(key.to_owned(), json!(b));
        }
    }
    let input_upload = stage_us(stages, &["ReUpload"]);
    if let Some(u) = input_upload {
        re.insert("input_upload_us".to_owned(), json!(u));
    }
    // ReUpload sub-stages: timing fields and classification counts sum
    // across firings zero-suppressed; `iu_digests_queried` and the
    // recorded batch size keep zeros (a suppressed zero batch would fall
    // back to the declared-size proxy and price a transfer that never
    // happened).
    let mut invalid_counter_fields = Vec::new();
    {
        let mut sums = HashMap::new();
        let mut keeps = HashMap::new();
        for s in stages {
            let Some(StageDetails::Upload(ru)) = &s.details else {
                continue;
            };
            if let Some(p) = &ru.stats {
                // `iu_materialize_us` is gathering the files to send: asking the materializer
                // where each one is, then materializing those not on disk yet. The first part is
                // also shown on its own.
                for (k, v) in [
                    ("iu_sem_wait_us", p.slot_wait_us),
                    ("iu_find_missing_us", p.find_missing_us),
                    (
                        "iu_materialize_us",
                        p.materializer_query_us.saturating_add(p.materialize_us),
                    ),
                    ("iu_materializer_query_us", p.materializer_query_us),
                    ("iu_cas_call_us", p.cas_write_us),
                    ("iu_digests_absent", p.digests_missing),
                ] {
                    add_counter(&mut sums, k, v);
                }
                add_counter(&mut keeps, "iu_digests_queried", p.digests_queried);
            }
            if let Some(v) = ru.bytes_uploaded {
                add_counter(&mut keeps, "iu_bytes_uploaded", v);
            }
        }
        emit_counters(&mut re, sums, false, &mut invalid_counter_fields);
        emit_counters(&mut re, keeps, true, &mut invalid_counter_fields);
    }
    if let Some(s) = stage_us(stages, &["ReSubmit"]) {
        re.insert("submit_us".to_owned(), json!(s));
        let mut sums = HashMap::new();
        let mut permits_min: Option<u64> = None;
        for st in stages {
            let Some(StageDetails::Submit(rs)) = &st.details else {
                continue;
            };
            for (k, v) in [
                ("rs_exec_sem_wait_us", rs.slot_wait_us),
                ("rs_request_prep_us", rs.request_prep_us),
                ("rs_rpc_open_us", rs.rpc_open_us),
            ] {
                add_counter(&mut sums, k, v);
            }
            let p = rs.free_slots_hint;
            permits_min = Some(permits_min.map_or(p, |m| m.min(p)));
        }
        emit_counters(&mut re, sums, false, &mut invalid_counter_fields);
        if let Some(p) = permits_min {
            re.insert("rs_exec_sem_permits_available".to_owned(), json!(p));
        }
    }
    if !invalid_counter_fields.is_empty() {
        invalid_counter_fields.sort_unstable();
        re.insert(
            "invalid_counter_fields".to_owned(),
            json!(invalid_counter_fields),
        );
    }
    // UPLOADING_OUTPUT is a client-observed worker state, not a daemon
    // receipt-processing queue. Prefer the current worker's measured output
    // duration in stage totals; retain state observation separately, including
    // its original positioned interval in activity.
    let worker_upload_wait = stage_union_us(stages, a.ts, a.dur, |s| s.name == "Re.WorkerUpload");
    if let Some(w) = worker_upload_wait {
        re.insert("worker_upload_wait_us".to_owned(), json!(w));
    }
    let worker_upload = last.upload_us.filter(|u| !reused_execution && *u >= 0);
    let output_upload = worker_upload.or(worker_upload_wait);

    // Positioned gap map over the stage intervals, clipped to the
    // action span; interval union, not wall subtraction.
    let mut intervals: Vec<(i64, i64)> = stages
        .iter()
        .filter_map(|s| clipped_stage(s, a.ts, a.dur))
        .filter(|(s, e)| e > s)
        .collect();
    intervals.sort_unstable();
    let merged = merge_intervals(&intervals);
    if !merged.is_empty() {
        let gap_pre = merged[0].0.saturating_sub(a.ts).max(0);
        let mut gap_inter: i64 = 0;
        for w in merged.windows(2) {
            gap_inter = gap_inter.saturating_add(w[1].0.saturating_sub(w[0].1).max(0));
        }
        let gap_post =
            a.ts.saturating_add(a.dur)
                .saturating_sub(merged.last().unwrap().1)
                .max(0);
        if gap_pre != 0 || gap_inter != 0 || gap_post != 0 {
            re.insert("gap_pre_us".to_owned(), json!(gap_pre));
            re.insert("gap_inter_us".to_owned(), json!(gap_inter));
            re.insert("gap_post_us".to_owned(), json!(gap_post));
            for (key, us) in [
                ("prepare_command_us", a.prepare_us),
                ("finish_action_us", a.finish_us),
            ] {
                if let Some(us) = us {
                    re.insert(key.to_owned(), json!(us));
                }
            }
        }
    }

    if matches!(ek, 2 | 11 | 12 | 13) || last.remote {
        re.insert("kind".to_owned(), json!(exec_kind_name(ek)));
        args.insert("re".to_owned(), Value::Object(re));
    } else if !re.is_empty()
        && (queue.unwrap_or(0) != 0
            || input_upload.unwrap_or(0) != 0
            || output_upload.is_some()
            || re.contains_key("worker_download_wait_us")
            || re.contains_key("cancelled_stages")
            || re.contains_key("gap_pre_us")
            || re.contains_key("cache_query")
            || re.contains_key("cache_hit")
            || re.contains_key("local_cache")
            || re.contains_key("invalid_cache_timing_records"))
    {
        args.insert("re".to_owned(), Value::Object(re));
    }

    // --- args.stages ---------------------------------------------
    let mut st = serde_json::Map::new();
    // These are positioned client observations, including recorded cancellation,
    // not completed-work counters. Zero is evidence; absence is not zero.
    for (name, key) in [
        ("CacheHit", "cache_hit_us"),
        ("Prepare", "prepare_us"),
        ("Local.FinalizeOutputs", "local_finalize_outputs_us"),
    ] {
        if let Some(elapsed) = stage_union_us(stages, a.ts, a.dur, |s| s.name == name) {
            st.insert(key.to_owned(), json!(elapsed));
        }
    }
    if let Some(elapsed) = stage_union_us(stages, a.ts, a.dur, |s| {
        matches!(
            s.name,
            "CasArtifact.CheckExpiration"
                | "CasArtifact.ExtendExpiration"
                | "CasArtifact.RecheckExpiration"
                | "CasArtifact.TreeMetadata"
                | "CasArtifact.DeclareOutputs"
        )
    }) {
        st.insert("cas_artifact_us".to_owned(), json!(elapsed));
    }
    if let Some(elapsed) = stage_union_us(stages, a.ts, a.dur, |s| {
        super::scan::is_local_cache_stage(s.name)
    }) {
        st.insert("local_cache_check_us".to_owned(), json!(elapsed));
    }
    if let Some(q) = cache_query {
        insert_nonzero(&mut st, "cache_query_us", q);
    }
    if let Some(q) = queue {
        st.insert("queue_us".to_owned(), json!(q));
    }
    if let Some(f) = fetch {
        st.insert("materialize_us".to_owned(), json!(f));
        if worker_fetch.is_none() {
            st.insert("materialize_source".to_owned(), json!("client_span"));
        }
    }
    if let Some(h) = last.hashing_us.filter(|_| !reused_execution) {
        insert_nonzero(&mut st, "hashing_us", h);
        if h >= 0 && last.hashing_local_outputs {
            st.insert("hashing_source".to_owned(), json!("local_outputs"));
        }
    }
    if reused_execution {
        if let Some(x) = local_execution_us(stages, a.ts, a.dur) {
            st.insert("execute_us".to_owned(), json!(x));
            st.insert("execute_source".to_owned(), json!("client_span"));
        }
    } else if let Some(x) = last.exec_us {
        insert_nonzero(&mut st, "execute_us", x);
    }
    insert_nonzero(&mut st, "input_upload_us", input_upload.unwrap_or(0));
    if let Some(upload) = output_upload {
        st.insert("upload_us".to_owned(), json!(upload));
        st.insert(
            "upload_source".to_owned(),
            json!(if worker_upload.is_some() {
                "worker"
            } else {
                "client_state"
            }),
        );
    }
    insert_nonzero(
        &mut st,
        "download_us",
        stage_us(stages, &["Re.Download"]).unwrap_or(0),
    );
    if a.dur > 0 {
        let covered = merged
            .iter()
            .fold(0i64, |sum, (s, e)| sum.saturating_add(e.saturating_sub(*s)));
        let daemon = a.dur.saturating_sub(covered);
        if daemon > 1000 {
            st.insert("daemon_us".to_owned(), json!(daemon));
        }
    }
    if !st.is_empty() {
        args.insert("stages".to_owned(), Value::Object(st));
    }
    if let Some(b) = a.input_files_bytes {
        if b != 0 {
            args.insert("bytes_in".to_owned(), json!(b));
        }
    }
    if a.output_size != 0 {
        args.insert("bytes_out".to_owned(), json!(a.output_size));
    }

    let mut obj = serde_json::Map::new();
    obj.insert("ph".to_owned(), json!("X"));
    obj.insert("cat".to_owned(), json!("recipe"));
    obj.insert("name".to_owned(), json!(name));
    obj.insert("ts".to_owned(), json!(a.ts));
    obj.insert("dur".to_owned(), json!(a.dur));
    obj.insert("pid".to_owned(), json!(PID));
    obj.insert("tid".to_owned(), json!(lane));
    obj.insert("args".to_owned(), Value::Object(args));
    obj
}

/// The action's row name: the converter suffixes ` ~N` to the Nth
/// action sharing a short name, leaving the first unsuffixed.
fn display_name(short: &str, ordinal: u32) -> String {
    if ordinal > 1 {
        format!("{short} ~{ordinal}")
    } else {
        short.to_owned()
    }
}

/// The same immutable projection is used by serial and parallel workers.
/// Lane assignment, duplicate ordinals and display serials are never recomputed
/// from completion order.
/// Points each entry of Buck's reported critical path at the trace's action records for its
/// spans, by their position among the records the trace writes in `order` (every slot is an
/// action record). Lets a viewer open those actions when there is no dependency graph.
fn link_reported_path(
    profile: &mut crate::command_profile::CommandProfile,
    actions: &[super::scan::ActionRec],
    slots: &[EvSlot],
    order: &[usize],
) {
    let Some(path) = profile.critical_path.as_mut() else {
        return;
    };
    let position: HashMap<u64, u32> = order
        .iter()
        .enumerate()
        .filter(|(_, k)| slots[**k].recipe)
        .map(|(pos, k)| (actions[slots[*k].idx].sid, pos as u32))
        .collect();
    for entry in &mut path.entries {
        entry.acts = entry
            .span_ids
            .iter()
            .filter_map(|sid| position.get(sid).copied())
            .collect();
    }
}

fn slot_event(e: &EvSlot, scan: &Scan, ctx: &EventCtx<'_>, ordinals: &[u32]) -> Value {
    if e.recipe {
        let a = &scan.actions[e.idx];
        Value::Object(action_event(
            a,
            scan.stages.get(&a.sid).map(Vec::as_slice).unwrap_or(&[]),
            ctx,
            &display_name(a.short(), ordinals[e.idx]),
            e.idx + 1,
            e.lane,
        ))
    } else {
        let p = &scan.phases[e.idx];
        let mut obj = json!({
            "ph": "X", "cat": "phase", "name": p.kind,
            "ts": p.ts, "dur": p.dur, "pid": PID, "tid": e.lane,
            "args": {
                "detail": p.detail, "src": p.kind, "argv": [], "dir": ctx.root,
                "exit": 0, "sig": 0, "makelevel": 0, "recurse": false,
                "host": "buck2", "v": 1, "run": ctx.run, "slot": e.lane,
            },
        });
        if !p.rule.is_empty() {
            obj["args"]["rule"] = json!(p.rule);
        }
        obj
    }
}

#[derive(Clone, Copy)]
struct ParallelEmit {
    workers: usize,
    records_per_job: usize,
    chunk_bytes: usize,
}

/// Zero workers selects the serial path. Production counts come from the
/// shared hardware policy; tests supply counts explicitly without environment
/// mutation. One worker remains useful on a two-core machine with a writer.
fn parallel_emit_options(workers: usize) -> Option<ParallelEmit> {
    (workers != 0).then_some(ParallelEmit {
        workers,
        records_per_job: 256,
        chunk_bytes: 256 * 1024,
    })
}

#[derive(Debug, Default)]
struct ParallelEmitStats {
    #[cfg(test)]
    workers: usize,
    records: usize,
    chunks: usize,
    bytes: u64,
    max_chunk_bytes: usize,
    max_chunk_records: usize,
    oversized_records: usize,
}

enum EmitChunk {
    Data { bytes: Vec<u8>, records: usize },
    Done,
    Failed(anyhow::Error),
}

fn serialize_job(
    range: std::ops::Range<usize>,
    options: ParallelEmit,
    separator: &[u8],
    render: &impl Fn(usize, &mut Vec<u8>) -> anyhow::Result<()>,
    tx: &std::sync::mpsc::SyncSender<EmitChunk>,
) -> anyhow::Result<()> {
    let send = |bytes: Vec<u8>, records: usize| {
        tx.send(EmitChunk::Data { bytes, records })
            .map_err(|_| anyhow::anyhow!("parallel trace writer stopped"))
    };
    let mut chunk = Vec::with_capacity(options.chunk_bytes);
    let mut records = 0;
    let mut row = Vec::new();
    for index in range {
        row.clear();
        render(index, &mut row)?;
        row.extend_from_slice(separator);
        if records != 0 && row.len() > options.chunk_bytes - chunk.len() {
            send(std::mem::take(&mut chunk), records)?;
            chunk = Vec::with_capacity(options.chunk_bytes);
            records = 0;
        }
        if row.len() > options.chunk_bytes {
            // An individual record is indivisible. Emit it alone; do not keep
            // its unusually large scratch allocation for subsequent records.
            send(std::mem::take(&mut row), 1)?;
        } else {
            chunk.extend_from_slice(&row);
            records += 1;
            if row.capacity() > options.chunk_bytes {
                row = Vec::new();
            }
        }
    }
    if records != 0 {
        send(chunk, records)?;
    }
    Ok(())
}

/// Fixed slots, not an unbounded completion-order map. Each worker has one
/// queued output and one active chunk/record; the writer holds one more.
/// Normal chunks are byte-capped and jobs are record-capped. A chunk may
/// exceed the byte cap only when it consists of one oversized record. The
/// current record's JSON object/scratch is additional per-worker memory;
/// these bounds are not a claim about allocator overhead or process RSS.
fn write_parallel_records(
    w: &mut impl std::io::Write,
    count: usize,
    options: ParallelEmit,
    render: &(impl Fn(usize, &mut Vec<u8>) -> anyhow::Result<()> + Sync),
) -> anyhow::Result<ParallelEmitStats> {
    // Every action/phase is followed by run_end, even without snapshots.
    write_parallel_records_delimited(w, count, options, b",\n", render)
}

fn write_parallel_records_delimited(
    w: &mut impl std::io::Write,
    count: usize,
    options: ParallelEmit,
    separator: &[u8],
    render: &(impl Fn(usize, &mut Vec<u8>) -> anyhow::Result<()> + Sync),
) -> anyhow::Result<ParallelEmitStats> {
    anyhow::ensure!(
        (1..=64).contains(&options.workers)
            && options.records_per_job != 0
            && options.chunk_bytes != 0,
        "invalid parallel trace emission limits"
    );
    let jobs = count.div_ceil(options.records_per_job);
    let n_workers = options.workers.min(jobs);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(n_workers);
        for worker in 0..n_workers {
            let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<std::ops::Range<usize>>(1);
            let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
            let handle = std::thread::Builder::new()
                .name(format!("retrace-emit-{worker}"))
                .spawn_scoped(scope, move || {
                    while let Ok(range) = job_rx.recv() {
                        if let Err(error) =
                            serialize_job(range, options, separator, render, &result_tx)
                        {
                            drop(result_tx.send(EmitChunk::Failed(error)));
                            break;
                        }
                        if result_tx.send(EmitChunk::Done).is_err() {
                            break;
                        }
                    }
                })?;
            workers.push((job_tx, result_rx, handle));
        }
        let mut result = (|| {
            let mut stats = ParallelEmitStats {
                #[cfg(test)]
                workers: n_workers,
                ..Default::default()
            };
            let job_range = |job: usize| {
                let start = job * options.records_per_job;
                start..start.saturating_add(options.records_per_job).min(count)
            };
            for (job, (tx, _, _)) in workers.iter().enumerate() {
                tx.send(job_range(job))?;
            }
            for job in 0..jobs {
                let (tx, rx, _) = &workers[job % n_workers];
                loop {
                    let message = rx.recv();
                    match message.map_err(|_| {
                        anyhow::anyhow!("parallel trace worker stopped before completing its job")
                    })? {
                        EmitChunk::Data { bytes, records } => {
                            anyhow::ensure!(
                                records != 0
                                    && records <= options.records_per_job
                                    && (bytes.len() <= options.chunk_bytes || records == 1),
                                "parallel trace chunk exceeds its bounds"
                            );
                            w.write_all(&bytes)?;
                            stats.records += records;
                            stats.chunks += 1;
                            stats.bytes = stats.bytes.saturating_add(bytes.len() as u64);
                            stats.max_chunk_bytes = stats.max_chunk_bytes.max(bytes.len());
                            stats.max_chunk_records = stats.max_chunk_records.max(records);
                            stats.oversized_records +=
                                usize::from(bytes.len() > options.chunk_bytes);
                        }
                        EmitChunk::Done => break,
                        EmitChunk::Failed(error) => return Err(error),
                    }
                }
                let next = job.saturating_add(n_workers);
                if next < jobs {
                    tx.send(job_range(next))?;
                }
            }
            anyhow::ensure!(
                stats.records == count,
                "parallel trace record count mismatch"
            );
            Ok(stats)
        })();
        // A writer failure must disconnect blocked senders BEFORE joining.
        // With unwinding, worker panics become a normal conversion refusal;
        // an aborting panic profile terminates the whole process instead.
        for (tx, rx, handle) in workers {
            drop(tx);
            drop(rx);
            if handle.join().is_err() && result.is_ok() {
                result = Err(anyhow::anyhow!("parallel trace worker panicked"));
            }
        }
        result
    })
}

/// What the graph pass needs from one action once the trace is on
/// disk: the three dependency channels and nothing else.
struct DepRec {
    span_ids: Vec<u64>,
    keys: Vec<u128>,
    tsets: Vec<u64>,
}

/// Run-level facts stamped into a graph header, plus the join tables
/// node ids resolve through — everything the graph writers need from a
/// scan whose action records have already been released.
struct GraphCtx<'a> {
    root: &'a str,
    trace_id: &'a str,
    capture: &'a str,
    goals: &'a [String],
    /// why an undefined definition is expected rather than corruption:
    /// the log is known to be short of the whole capture
    gap_reason: Option<&'a str>,
    dep_capture: &'a Value,
    by_sid: &'a HashMap<u64, usize>,
    key_to_idx: &'a HashMap<u128, u32>,
    names: &'a [&'a str],
}

/// Emit the trace (and optionally the graph) for one scanned log.
///
/// Takes the scan by value: the graph pass is the run's peak, and it
/// only reaches it because everything the trace needed has been freed
/// first.
#[cfg(test)]
pub(crate) fn emit(
    scan: Scan,
    trace: &Staged,
    graph: Option<&Staged>,
    want_phases: bool,
) -> anyhow::Result<EmitSummary> {
    emit_with_execution(scan, trace, graph, want_phases, None)
}

pub(crate) fn emit_with_execution(
    scan: Scan,
    trace: &Staged,
    graph: Option<&Staged>,
    want_phases: bool,
    execution: Option<&crate::execution::StoreInfo>,
) -> anyhow::Result<EmitSummary> {
    emit_with_options_and_execution(
        scan,
        trace,
        graph,
        want_phases,
        parallel_emit_options(crate::parallelism::trace_emit_workers()),
        parallel_emit_options(crate::parallelism::graph_emit_workers()),
        execution,
    )
}

#[cfg(test)]
fn emit_with_options(
    scan: Scan,
    trace: &Staged,
    graph: Option<&Staged>,
    want_phases: bool,
    parallel: Option<ParallelEmit>,
    graph_parallel: Option<ParallelEmit>,
) -> anyhow::Result<EmitSummary> {
    emit_with_options_and_execution(
        scan,
        trace,
        graph,
        want_phases,
        parallel,
        graph_parallel,
        None,
    )
}

fn emit_with_options_and_execution(
    mut scan: Scan,
    trace: &Staged,
    graph: Option<&Staged>,
    want_phases: bool,
    parallel: Option<ParallelEmit>,
    graph_parallel: Option<ParallelEmit>,
    execution: Option<&crate::execution::StoreInfo>,
) -> anyhow::Result<EmitSummary> {
    let mut warnings = Vec::new();
    let root = scan
        .project_root
        .clone()
        .unwrap_or_else(|| scan.working_dir.clone());
    let run = scan.trace_id.clone();
    let capture = CAPTURE_PLACEHOLDER;
    let dep_capture = scan.capture_state();
    let dep_capture_json = dep_capture.to_json();
    let outcome = match scan.command_profile.result_outcome.as_deref() {
        Some("success") => Outcome::Success,
        Some("failed" | "tests-failed") => Outcome::Failed,
        _ => match scan.build_errors {
            Some(n) => {
                if n > 0 {
                    Outcome::Failed
                } else {
                    Outcome::Success
                }
            }
            None => match scan.command_end_outcome {
                Some(true) => Outcome::Success,
                Some(false) => Outcome::Failed,
                _ => Outcome::Unknown,
            },
        },
    };
    let empty: Vec<StageRec> = Vec::new();

    // Pass 1, scan order: the dedup ordinal behind each display name,
    // the graph join tables, and the emission slots.  No event object
    // and no finished name exists yet — the trace builds each name as
    // it writes it, and the 3.9M-entry table the graph needs is not
    // materialised until the records themselves have been freed.
    let mut name_count: HashMap<&str, u32> = HashMap::new();
    let mut by_sid: HashMap<u64, usize> = HashMap::new();
    let mut key_to_idx: HashMap<u128, u32> = HashMap::new();
    let mut dup_ordinal: Vec<u32> = Vec::with_capacity(scan.actions.len());
    for (i, a) in scan.actions.iter().enumerate() {
        let n = name_count.entry(a.short()).or_insert(0);
        *n += 1;
        if !scan.ambiguous_spans.contains(&a.sid) {
            by_sid.entry(a.sid).or_insert(i);
            if let Some(key) = a.key_hash {
                key_to_idx.entry(key).or_insert(i as u32);
            }
        }
        dup_ordinal.push(*n);
    }

    let kept_phases: Vec<usize> = if want_phases {
        (0..scan.phases.len())
            .filter(|&i| scan.phases[i].dur >= 1_000)
            .collect()
    } else {
        Vec::new()
    };

    let mut slots: Vec<EvSlot> = Vec::with_capacity(scan.actions.len() + kept_phases.len());
    for (i, a) in scan.actions.iter().enumerate() {
        slots.push(EvSlot {
            ts: a.ts,
            dur: a.dur,
            recipe: true,
            idx: i,
            lane: 0,
        });
    }
    for &i in &kept_phases {
        slots.push(EvSlot {
            ts: scan.phases[i].ts,
            dur: scan.phases[i].dur,
            recipe: false,
            idx: i,
            lane: 0,
        });
    }
    if slots.is_empty() && !scan.command_profile.has_evidence() {
        anyhow::bail!(
            "{}: no completed actions or command-profile evidence were recorded",
            trace.publish_at.display()
        );
    }
    // Packing and file emission use the same stable timestamp order.
    let (lanes, order) = pack_lanes(&mut slots);
    let n_events = slots.len();
    link_reported_path(&mut scan.command_profile, &scan.actions, &slots, &order);

    // Aggregates for run_end, computed from the projection so the
    // record body and the tallies cannot disagree.
    let n_recipes = scan.actions.len();
    let n_hits = scan
        .actions
        .iter()
        .filter(|a| CACHE_KINDS.contains(&a.execution_kind))
        .count();
    let n_failed = scan
        .actions
        .iter()
        .filter(|a| a.failed || !matches!(exit_sig(a), (None | Some(0), None | Some(0))))
        .count();
    let misses = scan
        .actions
        .iter()
        .filter(|a| matches!(a.execution_kind, 1 | 2 | 8 | 11 | 13))
        .count();
    let restored_bytes: i64 = scan
        .actions
        .iter()
        .filter(|a| CACHE_KINDS.contains(&a.execution_kind))
        .fold(0i64, |sum, a| {
            sum.saturating_add(i64::try_from(a.output_size).unwrap_or(i64::MAX))
        });
    let key_us: i64 = scan
        .actions
        .iter()
        .filter(|a| CACHE_KINDS.contains(&a.execution_kind))
        .filter_map(|a| stage_us(scan.stages.get(&a.sid).unwrap_or(&empty), &["CacheQuery"]))
        .fold(0i64, i64::saturating_add);

    // run_start / run_end metadata and the snapshot counters.
    let mut sargs = serde_json::Map::new();
    sargs.insert("v".to_owned(), json!(1));
    sargs.insert("run".to_owned(), json!(run));
    sargs.insert("capture".to_owned(), json!(capture));
    if let Some(execution) = execution {
        sargs.insert(
            "execution_store".to_owned(),
            serde_json::to_value(execution)?,
        );
    }
    sargs.insert("tool".to_owned(), json!("buck2"));
    sargs.insert("cwd".to_owned(), json!(scan.working_dir));
    sargs.insert("argv".to_owned(), json!(scan.argv));
    if !scan.target_patterns.is_empty() {
        sargs.insert("targets".to_owned(), json!(scan.target_patterns));
    }
    sargs.insert("version".to_owned(), json!("buck2"));
    // `host` is the trace format's build-tool tag, not a machine; the
    // machine goes in `hostname`, and only when the log actually named it.
    sargs.insert("host".to_owned(), json!("buck2"));
    if let Some(h) = &scan.run_host {
        sargs.insert("hostname".to_owned(), json!(h));
    }
    if let Some(ncpu) = scan.ncpu {
        sargs.insert("ncpu".to_owned(), json!(ncpu));
    }
    if let Some(mem) = scan.mem_bytes {
        sargs.insert("mem".to_owned(), json!(mem));
    }
    if let Some(s) = &scan.re_session {
        sargs.insert("re_session".to_owned(), json!(s));
    }
    if scan.file_watcher.updates != 0 {
        sargs.insert("file_watcher".to_owned(), json!(scan.file_watcher));
    }
    if !scan.file_changes.is_empty() {
        let mut seen = HashSet::new();
        let uniq: Vec<&String> = scan
            .file_changes
            .iter()
            .filter(|p| seen.insert(p.as_str()))
            .collect();
        sargs.insert(
            "file_changes".to_owned(),
            json!(uniq.iter().take(50).collect::<Vec<_>>()),
        );
        // This is a count of retained distinct paths, not the watcher's
        // producer-reported event count. The latter lives in file_watcher.
        sargs.insert("file_changes_total".to_owned(), json!(uniq.len()));
    }
    let run_start = json!({
        "ph": "M", "cat": "run", "name": "run_start",
        "ts": scan.start_time_us, "pid": PID, "args": Value::Object(sargs),
    });
    // Separate from run_start so the bounded generation-header probe remains
    // small. These observations never become action-graph nodes or weights.
    scan.command_profile.schema_version = 1;
    scan.command_profile.run = Some(run.clone());
    let command_profile = json!({
        "ph": "M", "cat": "command-profile", "name": "command_profile",
        "ts": scan.command_profile.origin_us, "pid": PID,
        "args": {"run": &run, "profile": &scan.command_profile},
    });

    // `exit` is null when the outcome is unknown: a reader that
    // defaulted it to 0 would call a killed build a success
    let exit: Option<i64> = match outcome {
        _ if !scan.command_result_seen => None,
        Outcome::Success => Some(0),
        Outcome::Failed => Some(1),
        Outcome::Unknown => None,
    };
    let span_min = slots.iter().map(|e| e.ts).min().unwrap_or(0);
    let span_max = slots
        .iter()
        .map(|e| e.ts.saturating_add(e.dur))
        .max()
        .unwrap_or(0);
    let t_end = scan.cmd_end.map(|(ts, _)| ts).unwrap_or(if span_max != 0 {
        span_max
    } else {
        scan.start_time_us
    });
    let run_end = json!({
        "ph": "M", "cat": "run", "name": "run_end", "ts": t_end, "pid": PID,
        "args": {
            "v": 1, "run": run, "exit": exit, "outcome": outcome.as_str(),
            "dep_capture": dep_capture_json, "makelevel": 0,
            "failed": n_failed,
            "cache": {
                "hits": n_hits, "misses": misses,
                "uncacheable": n_recipes.saturating_sub(n_hits).saturating_sub(misses),
                "restored_bytes": restored_bytes, "key_us": key_us,
            },
        },
    });

    let counters: Vec<Value> = scan
        .snapshots
        .iter()
        .map(|s| {
            json!({
                "ph": "C", "cat": "snapshot", "name": "buck2", "ts": s.ts,
                "pid": PID, "tid": 0,
                "args": {"heap": s.heap, "rss_max": s.rss_max, "dl_bytes": s.dl_bytes,
                         "ul_bytes": s.ul_bytes, "cpu_us": s.cpu_us},
            })
        })
        .collect();

    // Pass 2: write the trace, building each record at the moment it
    // is written and dropping it after.  Line-framed like the
    // converter's, but a closed JSON array (the readers accept both).
    let trace_digest = {
        fn put<S: serde::Serialize>(
            w: &mut impl std::io::Write,
            v: &S,
            i: &mut usize,
            total: usize,
        ) -> anyhow::Result<()> {
            serde_json::to_writer(&mut *w, v)?;
            *i += 1;
            writeln!(w, "{}", if *i == total { "" } else { "," })?;
            Ok(())
        }
        let ectx = EventCtx {
            root: &root,
            run: &run,
            cfgs: &scan.cfg_names,
            categories: &scan.category_names,
        };
        let mut w = artifact_writer(&trace.write_to)?;
        let total = 2 + slots.len() + counters.len() + 1;
        let mut i = 0usize;
        writeln!(w, "[")?;
        put(&mut w, &run_start, &mut i, total)?;
        put(&mut w, &command_profile, &mut i, total)?;
        if let Some(options) = parallel {
            let stats =
                write_parallel_records(&mut w, order.len(), options, &|position, bytes| {
                    let obj = slot_event(&slots[order[position]], &scan, &ectx, &dup_ordinal);
                    serde_json::to_writer(bytes, &obj)?;
                    Ok(())
                })?;
            i += stats.records;
        } else {
            for &k in &order {
                let obj = slot_event(&slots[k], &scan, &ectx, &dup_ordinal);
                put(&mut w, &obj, &mut i, total)?;
            }
        }
        for c in &counters {
            put(&mut w, c, &mut i, total)?;
        }
        put(&mut w, &run_end, &mut i, total)?;
        writeln!(w, "]")?;
        finish_artifact(w)?
    };

    let mut status_lines = vec![format!(
        "{}: {} actions ({} cache hits), span {:.2}s, {} lanes, {} snapshots, run {}",
        trace.publish_at.display(),
        n_recipes,
        n_hits,
        (span_max.saturating_sub(span_min)) as f64 / 1e6,
        lanes,
        scan.snapshots.len(),
        run,
    )];

    let mut graph_outcome = GraphOutcome::NotWritten;
    let mut graph_digest = None;
    let activity_scope = scan.capture_scope == crate::types::CaptureScope::CurrentActionAncestors;
    let has_structural_graph = activity_scope
        || (scan.capture_schema == Some(super::scan::SUPPORTED_CAPTURE_SCHEMA)
            && (!scan.structures.is_empty()
                || !scan.tset_defs.is_empty()
                || !scan.aliases.is_empty()));
    if n_recipes == 0 {
        status_lines.push(if activity_scope {
            "No completed actions were recorded; the current-action-ancestors graph can be empty. This is not a full build graph; action timing analyses are unavailable."
        } else if has_structural_graph {
            "No completed actions were recorded; artifact structure is retained without execution durations. Action timing analyses are unavailable."
        } else {
            "Command profile retained; action graph and action analyses are unavailable because no completed actions were recorded."
        }.to_owned());
    }
    if let Some(gp) =
        graph.filter(|_| (n_recipes != 0 || has_structural_graph) && scan.capture_started)
    {
        // Nothing below reads an action's names, timings or stages —
        // only its three dependency channels — so the trace-only two
        // thirds of the scan is released here rather than at the end
        // of `emit`.  On a 3.9M-action capture that is ~5 GiB that
        // would otherwise sit under the graph pass, which is the peak.
        drop(name_count);
        drop(slots);
        drop(order);
        drop(counters);
        scan.stages = HashMap::new();
        scan.phases = Vec::new();
        scan.snapshots = Vec::new();
        // The graph's node names are materialised here, in the same
        // walk that releases the records: an undeduplicated name is
        // moved straight out of `full_target` rather than copied.
        let mut scan_names: Vec<String> = Vec::with_capacity(scan.actions.len());
        let deps_of: Vec<DepRec> = std::mem::take(&mut scan.actions)
            .into_iter()
            .enumerate()
            .map(|(i, mut a)| {
                scan_names.push(if dup_ordinal[i] > 1 {
                    display_name(a.short(), dup_ordinal[i])
                } else {
                    let mut s = std::mem::take(&mut a.full_target);
                    s.drain(..a.short_at as usize);
                    s
                });
                DepRec {
                    span_ids: a.dep_span_ids,
                    keys: a.dep_action_keys,
                    tsets: a.dep_tset_ids,
                }
            })
            .collect();
        drop(dup_ordinal);
        let tset_defs = std::mem::take(&mut scan.tset_defs);

        let names: Vec<&str> = scan_names.iter().map(String::as_str).collect();
        let ctx = GraphCtx {
            root: &root,
            trace_id: &scan.trace_id,
            capture,
            goals: &scan.build_targets,
            gap_reason: if scan.incomplete.is_some() {
                Some("attributable to the truncated stream")
            } else {
                match &dep_capture {
                    CaptureState::Started(s) if !s.footer_present => {
                        Some("attributable to the missing capture footer")
                    }
                    CaptureState::Started(s)
                        if s.structure
                            .as_ref()
                            .is_some_and(|st| st.complete == Some(false)) =>
                    {
                        Some("attributable to the capture's unfinished definition walk")
                    }
                    CaptureState::Started(s) if s.finality == Finality::Nonfinal => {
                        Some("attributable to the nonfinal capture footer")
                    }
                    CaptureState::Started(s) if s.finality == Finality::Unknown => {
                        Some("the capture footer does not establish finality")
                    }
                    _ => None,
                }
            },
            dep_capture: &dep_capture_json,
            by_sid: &by_sid,
            key_to_idx: &key_to_idx,
            names: &names,
        };
        let (nodes, edges, digest) = write_full_graph(
            &ctx,
            deps_of,
            tset_defs,
            std::mem::take(&mut scan.structures),
            std::mem::take(&mut scan.aliases),
            &gp.write_to,
            &mut warnings,
            graph_parallel,
        )?;
        graph_digest = Some(digest);
        graph_outcome = GraphOutcome::Written;
        status_lines.push(format!(
            "{}: {} dependency graph, {nodes} nodes, {edges} edges",
            gp.publish_at.display(),
            if activity_scope {
                // Complete for the actions that ran: only reused or source-only parts with
                // nothing run below them are left out.
                "actions-that-ran"
            } else {
                "full"
            }
        ));
    }

    let capture = capture_fingerprint(&trace_digest, graph_digest.as_ref());
    stamp_artifact(&trace.write_to, true, &capture)?;
    if let Some(gp) = graph.filter(|_| graph_digest.is_some()) {
        stamp_artifact(&gp.write_to, false, &capture)?;
    }

    Ok(EmitSummary {
        capture,
        status_lines,
        warnings,
        n_events,
        n_cache_hits: n_hits,
        lanes,
        graph: graph_outcome,
        outcome,
        dep_capture,
    })
}

/// Keep fields in the alphabetical order previously produced by
/// serde_json::Map: the exact serialized bytes participate in the capture
/// fingerprint. Borrow dependency names instead of allocating a Value string
/// for every edge, and omit the join marker rather than emitting null.
#[derive(serde::Serialize)]
struct GraphRow<'a> {
    d: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    j: Option<u8>,
    t: &'a str,
}

fn write_graph_row(
    w: &mut impl std::io::Write,
    target: &str,
    deps: &[&str],
    join: bool,
) -> anyhow::Result<()> {
    let row = GraphRow {
        d: deps,
        j: join.then_some(1),
        t: target,
    };
    serde_json::to_writer(&mut *w, &row)?;
    w.write_all(b"\n")?;
    Ok(())
}

/// The rows and names are immutable by this point. Only serialization is
/// parallel; a single writer preserves row order, hashing and error handling.
/// An unavailable bounded shared cache falls back to this ordinary encoding;
/// never multiply the cache's storage budget by the worker count.
fn write_parallel_graph_rows<'a>(
    w: &mut impl std::io::Write,
    rows: &[(u32, Vec<u32>, bool)],
    options: ParallelEmit,
    name: &(impl Fn(u32) -> &'a str + Sync),
) -> anyhow::Result<ParallelEmitStats> {
    write_parallel_records_delimited(w, rows.len(), options, b"\n", &|i, bytes| {
        let (target, deps, join) = &rows[i];
        bytes.extend_from_slice(b"{\"d\":[");
        for (i, dep) in deps.iter().enumerate() {
            if i != 0 {
                bytes.push(b',');
            }
            serde_json::to_writer(&mut *bytes, name(*dep))?;
        }
        bytes.extend_from_slice(if *join {
            b"],\"j\":1,\"t\":"
        } else {
            b"],\"t\":"
        });
        serde_json::to_writer(&mut *bytes, name(*target))?;
        bytes.push(b'}');
        Ok(())
    })
}

/// Populate actual join targets first, then action targets, in row order
/// within each group. Short tset names must not be starved by earlier long
/// action names. This is a cache-admission hypothesis, not measured frequency.
/// The ID space can contain unnamed holes, so it must not drive name().
/// Two selection passes still encode only actual targets, not every edge.
/// Non-admitted names are skipped before serialization, rather than escaping
/// them into a sink through the ordinary writer's fallback path.
fn populate_graph_name_cache<'a>(
    cache: &mut GraphNameCache,
    rows: &[(u32, Vec<u32>, bool)],
    name: &impl Fn(u32) -> &'a str,
) -> anyhow::Result<()> {
    let mut sink = std::io::sink();
    for joins in [true, false] {
        for (target, _, join) in rows {
            if *join != joins {
                continue;
            }
            let Some(_) = usize::try_from(*target)
                .ok()
                .and_then(|id| cache.index.get(id))
            else {
                continue;
            };
            if cache.encoded_name(*target)?.is_some() {
                continue;
            }
            let remaining = cache
                .arena
                .len()
                .checked_sub(cache.used)
                .ok_or_else(|| anyhow::anyhow!("graph-name cache used bytes outside arena"))?;
            let rendered = name(*target);
            if !graph_name_fits_remaining(rendered.len(), remaining) {
                continue;
            }
            cache.write_name(&mut sink, *target, || rendered)?;
        }
    }
    Ok(())
}

fn graph_name_fits_remaining(name_len: usize, remaining: usize) -> bool {
    name_len
        .checked_mul(6)
        .and_then(|maximum| maximum.checked_add(2))
        .is_some_and(|maximum| maximum <= remaining)
}

/// All workers borrow the same completed cache. A miss retains the ordinary
/// serde encoding, without cache mutation, locking or a per-worker copy.
fn write_parallel_graph_rows_with_cache<'a>(
    w: &mut impl std::io::Write,
    rows: &[(u32, Vec<u32>, bool)],
    options: ParallelEmit,
    name: &(impl Fn(u32) -> &'a str + Sync),
    cache: &GraphNameCache,
) -> anyhow::Result<ParallelEmitStats> {
    let encode = |bytes: &mut Vec<u8>, id: u32| -> anyhow::Result<()> {
        if let Some(encoded) = cache.encoded_name(id)? {
            bytes.extend_from_slice(encoded);
        } else {
            serde_json::to_writer(bytes, name(id))?;
        }
        Ok(())
    };
    write_parallel_records_delimited(w, rows.len(), options, b"\n", &|i, bytes| {
        let (target, deps, join) = &rows[i];
        bytes.extend_from_slice(b"{\"d\":[");
        for (i, dep) in deps.iter().enumerate() {
            if i != 0 {
                bytes.push(b',');
            }
            encode(bytes, *dep)?;
        }
        bytes.extend_from_slice(if *join {
            b"],\"j\":1,\"t\":"
        } else {
            b"],\"t\":"
        });
        encode(bytes, *target)?;
        bytes.push(b'}');
        Ok(())
    })
}

const GRAPH_NAME_CACHE_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Bound small graphs by their conservative encoded-name size plus the dense
/// index. Overflow saturates at the caller's cap, never wraps or allocates.
fn adaptive_graph_name_cache_budget(
    maximum: usize,
    id_space: usize,
    name_lengths: impl IntoIterator<Item = usize>,
) -> Option<usize> {
    let maximum = maximum.min(GRAPH_NAME_CACHE_MAX_BYTES);
    let index = id_space.checked_mul(std::mem::size_of::<EncodedGraphName>())?;
    let available = maximum.checked_sub(index)?;
    if available < 2 {
        return None;
    }
    let mut encoded = 0usize;
    for len in name_lengths {
        let bound = len
            .checked_mul(6)
            .and_then(|n| n.checked_add(2))
            .unwrap_or(available);
        encoded = encoded.saturating_add(bound).min(available);
        if encoded == available {
            break;
        }
    }
    (encoded >= 2).then_some(index + encoded)
}

/// Caching is optional: an oversized index or an allocation refusal leaves
/// graph emission on the ordinary borrowed serializer.
fn graph_name_cache(
    id_space: usize,
    name_lengths: impl IntoIterator<Item = usize>,
) -> Option<GraphNameCache> {
    let budget =
        adaptive_graph_name_cache_budget(GRAPH_NAME_CACHE_MAX_BYTES, id_space, name_lengths)?;
    GraphNameCache::new(budget, id_space).ok()
}

#[derive(Clone, Copy, Default)]
struct EncodedGraphName {
    offset: u32,
    // A JSON string is at least two bytes, so zero means not cached.
    len: u32,
}

/// A bounded serialization cache, not an additional artifact format. Only
/// strings encoded by serde_json enter the arena. The dense index and byte
/// arena never grow: the budget covers their Vec storage, not allocator
/// bookkeeping or process RSS. Encoding writes directly into the unused arena
/// slice without a temporary Vec or reallocations.
struct GraphNameCache {
    index: Vec<EncodedGraphName>,
    arena: Vec<u8>,
    used: usize,
}

impl GraphNameCache {
    fn new(budget: usize, id_space: usize) -> Result<Self, &'static str> {
        if budget > GRAPH_NAME_CACHE_MAX_BYTES {
            return Err("budget exceeds cache limit");
        }
        let entry_size = std::mem::size_of::<EncodedGraphName>();
        let index_bytes = id_space
            .checked_mul(entry_size)
            .ok_or("index size overflow")?;
        if index_bytes > budget || budget - index_bytes < 2 {
            return Err("index and encoded name do not fit");
        }
        let mut index = Vec::new();
        index
            .try_reserve_exact(id_space)
            .map_err(|_| "index allocation failed")?;
        let index_bytes = index
            .capacity()
            .checked_mul(entry_size)
            .ok_or("index capacity overflow")?;
        let remaining = budget
            .checked_sub(index_bytes)
            .ok_or("index capacity exceeds budget")?;
        if remaining < 2 {
            return Err("encoded name does not fit");
        }
        let mut arena = Vec::new();
        arena
            .try_reserve_exact(remaining)
            .map_err(|_| "arena allocation failed")?;
        if arena.capacity() > remaining {
            return Err("arena capacity exceeds budget");
        }
        index.resize(id_space, EncodedGraphName::default());
        arena.resize(remaining, 0);
        Ok(Self {
            index,
            arena,
            used: 0,
        })
    }

    #[cfg(test)]
    fn index_bytes(&self) -> usize {
        self.index.capacity() * std::mem::size_of::<EncodedGraphName>()
    }

    /// Immutable lookup after population. Missing/out-of-index IDs use the
    /// normal serializer; an inconsistent cached range remains an error.
    fn encoded_name(&self, id: u32) -> anyhow::Result<Option<&[u8]>> {
        let Some(entry) = usize::try_from(id).ok().and_then(|i| self.index.get(i)) else {
            return Ok(None);
        };
        if entry.len == 0 {
            return Ok(None);
        }
        let start = usize::try_from(entry.offset)?;
        let end = start
            .checked_add(usize::try_from(entry.len)?)
            .ok_or_else(|| anyhow::anyhow!("graph-name cache offset overflow"))?;
        let encoded = self
            .arena
            .get(..self.used)
            .and_then(|used| used.get(start..end))
            .ok_or_else(|| anyhow::anyhow!("graph-name cache offset outside arena"))?;
        Ok(Some(encoded))
    }

    fn write_name<'a>(
        &mut self,
        w: &mut impl std::io::Write,
        id: u32,
        name: impl FnOnce() -> &'a str,
    ) -> anyhow::Result<()> {
        let idx = usize::try_from(id).ok();
        let Some(entry) = idx.and_then(|i| self.index.get(i)).copied() else {
            serde_json::to_writer(w, name())?;
            return Ok(());
        };
        if entry.len != 0 {
            let start = usize::try_from(entry.offset)?;
            let end = start
                .checked_add(usize::try_from(entry.len)?)
                .ok_or_else(|| anyhow::anyhow!("graph-name cache offset overflow"))?;
            let encoded = self
                .arena
                .get(..self.used)
                .and_then(|used| used.get(start..end))
                .ok_or_else(|| anyhow::anyhow!("graph-name cache offset outside arena"))?;
            w.write_all(encoded)?;
            return Ok(());
        }
        let name = name();
        // Six bytes per input byte plus quotes bounds JSON string escaping.
        // Large names remain on the normal streaming path without allocation.
        let maximum = name.len().checked_mul(6).and_then(|n| n.checked_add(2));
        if maximum.is_none_or(|n| n > self.arena.len()) {
            serde_json::to_writer(w, name)?;
            return Ok(());
        }
        let maximum = maximum.unwrap();
        if maximum > self.arena.len() - self.used {
            serde_json::to_writer(w, name)?;
            return Ok(());
        }
        let start = self.used;
        let bound_end = start
            .checked_add(maximum)
            .ok_or_else(|| anyhow::anyhow!("graph-name cache encoding bound overflow"))?;
        let unused = self
            .arena
            .get_mut(start..bound_end)
            .ok_or_else(|| anyhow::anyhow!("graph-name cache encoding bound outside arena"))?;
        let mut cursor = std::io::Cursor::new(unused);
        if serde_json::to_writer(&mut cursor, name).is_err() {
            // An internal bounded-write refusal must not change the export.
            // No index/used offset advances, so partial scratch is invisible.
            serde_json::to_writer(w, name)?;
            return Ok(());
        }
        let len = usize::try_from(cursor.position())?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| anyhow::anyhow!("graph-name cache size overflow"))?;
        anyhow::ensure!(
            end <= self.arena.len(),
            "graph-name cache exceeds fixed arena"
        );
        let cached = EncodedGraphName {
            offset: u32::try_from(start)?,
            len: u32::try_from(len)?,
        };
        self.used = end;
        self.index[idx.unwrap()] = cached;
        w.write_all(&self.arena[start..end])?;
        Ok(())
    }
}

fn write_cached_graph_row<'a>(
    w: &mut impl std::io::Write,
    target: u32,
    deps: &[u32],
    join: bool,
    cache: &mut GraphNameCache,
    name: &impl Fn(u32) -> &'a str,
) -> anyhow::Result<()> {
    w.write_all(b"{\"d\":[")?;
    for (i, &id) in deps.iter().enumerate() {
        if i != 0 {
            w.write_all(b",")?;
        }
        cache.write_name(w, id, || name(id))?;
    }
    w.write_all(if join {
        b"],\"j\":1,\"t\":"
    } else {
        b"],\"t\":"
    })?;
    cache.write_name(w, target, || name(target))?;
    w.write_all(b"}\n")?;
    Ok(())
}

/// The recorded graph sidecar, including an explicitly scoped `enc:3` variant and
/// independent actions with no edges. This
/// refuses per the corrupt-log contract: a dangling projection on a
/// COMPLETE log is corruption; on a truncated one it is counted.
fn write_full_graph(
    ctx: &GraphCtx<'_>,
    deps_of: Vec<DepRec>,
    mut tset_defs: HashMap<u64, TsetDef>,
    mut structures: HashMap<u128, crate::ingest::scan::StructureDef>,
    aliases: HashMap<u128, crate::ingest::scan::AliasDef>,
    path: &Path,
    warnings: &mut Vec<String>,
    parallel: Option<ParallelEmit>,
) -> anyhow::Result<(usize, usize, ArtifactDigest)> {
    // A projection's identity is its command-local integer id: that is
    // what `dep_tset_ids` and `child_ids` join on, and the wire
    // `display_key` is a rendering capped at 4 KiB that distinct
    // projections under one deep dynamic-output chain share
    // (`data.proto`: "Display only, never a join key").  Rows are named
    // `tset:<id>`; the graph reader keys on the `tset:` prefix alone.
    // Split chunks of one definition were already unioned by id in the
    // scan, so nothing merges here.
    //
    // Ids are then mapped to dense `kid`s, because child references
    // repeat them millions of times on a real capture and an edge must
    // cost 4 B (coord L-122 §B.1).  A referenced-but-undefined id gets
    // a kid too, so it can be counted as dangling below.
    fn kid_of(id: u64, kid_id: &mut Vec<u64>, id_key: &mut HashMap<u64, u32>) -> u32 {
        *id_key.entry(id).or_insert_with(|| {
            let kid = u32::try_from(kid_id.len()).expect("fewer than 4G distinct tset ids");
            kid_id.push(id);
            kid
        })
    }
    // kid -> wire id; `id_key` is the inverse.
    let mut kid_id: Vec<u64> = Vec::new();
    let mut id_key: HashMap<u64, u32> = HashMap::new();

    // Both walks go through a sorted id list, never the map itself:
    // HashMap iteration order would leak into the kid assignment, the
    // join-row worklist and ultimately the bytes of graph.json, and the
    // artifact must hash identically run to run (it is how a rewrite of
    // this function is proven to have moved nothing).
    let mut def_ids: Vec<u64> = tset_defs.keys().copied().collect();
    def_ids.sort_unstable();
    for id in &def_ids {
        kid_of(*id, &mut kid_id, &mut id_key);
    }
    let defined_count = def_ids.len();
    struct JoinDef {
        own_keys: Vec<u128>,
        own_sids: Vec<u64>,
        children: Vec<u32>,
    }
    // A separate pass, not a continuation of the one above: a child
    // may carry a higher id than its parent, and a defined child must
    // keep the kid its own definition was given.
    let mut defs: HashMap<u32, JoinDef> = HashMap::new();
    for id in &def_ids {
        let def = tset_defs
            .remove(id)
            .expect("id came from this map's key set");
        let kid = id_key[id];
        let own_keys = def.own_keys;
        let children: Vec<u32> = def
            .child_ids
            .iter()
            .map(|c| kid_of(*c, &mut kid_id, &mut id_key))
            .collect();
        defs.insert(
            kid,
            JoinDef {
                own_keys,
                own_sids: def.own_span_ids,
                children,
            },
        );
    }
    drop(tset_defs);
    drop(def_ids);

    // Structure kids and undefined-tset kids overlap in [defined_count, ..).
    // Only kids below defined_count name real tset nodes: both child-resolution
    // loops below guard that boundary before creating edges. Undefined tset
    // kids are diagnostic-only, never nodes. Thus node_id_space takes the max
    // of those two ranges, not their sum. Keep legacy node/edge ordering.
    let mut structure_keys: Vec<_> = structures
        .keys()
        .copied()
        .filter(|key| !ctx.key_to_idx.contains_key(key))
        .collect();
    structure_keys.sort_unstable();
    let mut structure_inputs_omitted = Vec::with_capacity(structure_keys.len());
    let mut structure_kids = HashMap::with_capacity(structure_keys.len());
    for (index, key) in structure_keys.iter().enumerate() {
        let kid = u32::try_from(
            defined_count
                .checked_add(index)
                .ok_or_else(|| anyhow::anyhow!("structural join index overflow"))?,
        )?;
        structure_kids.insert(*key, kid);
        let def = structures.remove(key).expect("key came from this map");
        structure_inputs_omitted.push(def.inputs_omitted);
        let children = def
            .deps
            .child_ids
            .into_iter()
            .map(|id| kid_of(id, &mut kid_id, &mut id_key))
            .collect();
        defs.insert(
            kid,
            JoinDef {
                own_keys: def.deps.own_keys,
                own_sids: def.deps.own_span_ids,
                children,
            },
        );
    }
    drop(structures);

    let mut tset_name: HashMap<u32, String> = HashMap::new();
    let mut named_order: Vec<u32> = Vec::new();
    fn tname(
        kid: u32,
        kid_id: &[u64],
        defined_count: usize,
        structure_keys: &[u128],
        tset_name: &mut HashMap<u32, String>,
        named_order: &mut Vec<u32>,
    ) -> bool {
        if tset_name.contains_key(&kid) {
            return false;
        }
        let name = if (kid as usize) < defined_count {
            format!("tset:{}", kid_id[kid as usize])
        } else {
            format!(
                "structure:{:032x}",
                structure_keys[kid as usize - defined_count]
            )
        };
        tset_name.insert(kid, name);
        named_order.push(kid);
        true
    }

    // A node id is an index into `names` when it is below `n_act`, and
    // `n_act + kid` for a tset projection.  Storing ids instead of the
    // display strings is what keeps this pass off the heap: a cloned
    // name costs ~94 B and one allocation per edge, an id costs 4 B and
    // none (measured 2026-09-06: 7.06 GiB -> 0.32 GiB on a 79.5M-edge
    // capture).
    let n_act = u32::try_from(deps_of.len()).expect("fewer than 4G actions");
    anyhow::ensure!(
        u32::try_from(defined_count + structure_keys.len())
            .ok()
            .is_some_and(|joins| n_act.checked_add(joins).is_some()),
        "graph node index overflow"
    );
    let key_node = |key: &u128| {
        let canonical = aliases.get(key).map_or(key, |alias| &alias.canonical);
        ctx.key_to_idx
            .get(canonical)
            .copied()
            .or_else(|| structure_kids.get(canonical).map(|kid| n_act + kid))
    };
    let mut rows: Vec<(u32, Vec<u32>, bool)> = Vec::new();
    let mut structure_rows: Vec<(u32, Vec<u32>, bool)> = Vec::new();
    let mut edges = 0usize;
    // An alias is identity evidence, not a definition of its destination.
    // Preserve a missing canonical destination even without a current action
    // referring to that alias (for example, a no-touch structural capture).
    let mut unmapped = aliases
        .values()
        .filter(|alias| {
            !ctx.key_to_idx.contains_key(&alias.canonical)
                && !structure_kids.contains_key(&alias.canonical)
        })
        .count();
    let mut undefined: HashSet<u32> = HashSet::new();

    // Keep every explicitly emitted canonical structure, not just bridges
    // reached from this command's executed actions. A no-touch command has
    // no such roots, and its already-recorded closure must not disappear.
    for index in 0..structure_keys.len() {
        tname(
            (defined_count + index) as u32,
            &kid_id,
            defined_count,
            &structure_keys,
            &mut tset_name,
            &mut named_order,
        );
    }
    if n_act == 0 && ctx.dep_capture.get("structure").is_some() {
        for kid in 0..defined_count {
            tname(
                kid as u32,
                &kid_id,
                defined_count,
                &structure_keys,
                &mut tset_name,
                &mut named_order,
            );
        }
    }

    let mut seen_deps = HashSet::new();
    // Consumed, not borrowed: each record's three channels are freed
    // as its row is built, so the resolved ids replace the raw ones
    // instead of stacking on top of them.
    for (i, a) in deps_of.into_iter().enumerate() {
        let mut deps: Vec<u32> = Vec::new();
        for s in &a.span_ids {
            match ctx.by_sid.get(s) {
                Some(&j) => deps.push(j as u32),
                None => unmapped += 1,
            }
        }
        for k in &a.keys {
            match key_node(k) {
                Some(j) => {
                    if j >= n_act {
                        tname(
                            j - n_act,
                            &kid_id,
                            defined_count,
                            &structure_keys,
                            &mut tset_name,
                            &mut named_order,
                        );
                    }
                    deps.push(j);
                }
                None => unmapped += 1,
            }
        }
        for id in &a.tsets {
            let kid = kid_of(*id, &mut kid_id, &mut id_key);
            if (kid as usize) < defined_count {
                tname(
                    kid,
                    &kid_id,
                    defined_count,
                    &structure_keys,
                    &mut tset_name,
                    &mut named_order,
                );
                deps.push(n_act + kid);
            } else {
                undefined.insert(kid);
            }
        }
        if deps.len() > 1 {
            seen_deps.clear();
            deps.retain(|dep| seen_deps.insert(*dep));
        }
        edges += deps.len();
        deps.shrink_to_fit();
        rows.push((i as u32, deps, false));
    }

    // Join rows: LIFO worklist so transitively referenced projections get
    // rows too. Dedup resolved nodes across channels without changing order;
    // raw-id sets also keep repeated unresolved references from inflating gaps.
    let mut queue: Vec<u32> = named_order.clone();
    while let Some(kid) = queue.pop() {
        let m = defs
            .remove(&kid)
            .expect("each named definition is visited once");
        let mut deps: Vec<u32> = Vec::new();
        let mut seen_sids: HashSet<u64> = HashSet::new();
        let mut seen_keys: HashSet<u128> = HashSet::new();
        let mut seen_children: HashSet<u32> = HashSet::new();
        for s in &m.own_sids {
            if !seen_sids.insert(*s) {
                continue;
            }
            match ctx.by_sid.get(s) {
                Some(&j) => deps.push(j as u32),
                None => unmapped += 1,
            }
        }
        for kb in &m.own_keys {
            if !seen_keys.insert(*kb) {
                continue;
            }
            match key_node(kb) {
                Some(j) => {
                    if j >= n_act
                        && tname(
                            j - n_act,
                            &kid_id,
                            defined_count,
                            &structure_keys,
                            &mut tset_name,
                            &mut named_order,
                        )
                    {
                        queue.push(j - n_act);
                    }
                    deps.push(j);
                }
                None => unmapped += 1,
            }
        }
        for &c in &m.children {
            if !seen_children.insert(c) {
                continue;
            }
            if (c as usize) < defined_count {
                if !tset_name.contains_key(&c) {
                    queue.push(c);
                }
                tname(
                    c,
                    &kid_id,
                    defined_count,
                    &structure_keys,
                    &mut tset_name,
                    &mut named_order,
                );
                deps.push(n_act + c);
            } else {
                undefined.insert(c);
            }
        }
        if (kid as usize) >= defined_count {
            // Several explicit aliases can name the same canonical producer.
            deps.sort_unstable();
            deps.dedup();
        } else if deps.len() > 1 {
            seen_deps.clear();
            deps.retain(|dep| seen_deps.insert(*dep));
        }
        edges += deps.len();
        deps.shrink_to_fit();
        if (kid as usize) >= defined_count {
            structure_rows.push((n_act + kid, deps, true));
        } else {
            rows.push((n_act + kid, deps, true));
        }
    }

    if !undefined.is_empty() {
        let msg = format!(
            "{} referenced tset projection(s) have no definition in this log",
            undefined.len()
        );
        if let Some(why) = ctx.gap_reason {
            warnings.push(format!(
                "{msg} -- {why}; affected edges counted as dangling"
            ));
        } else {
            anyhow::bail!(
                "{msg} and the stream is complete: the log is corrupt or was written by a \
                 mismatched buck2 -- refusing to emit a silently-holed graph"
            );
        }
    }
    if unmapped != 0 {
        warnings.push(format!(
            "{unmapped} action dependency references have no recorded producer; the producer \
             may be from an earlier command, cancelled, or absent from this capture"
        ));
    }

    // Resolution is complete. Only rows, rendered names and coverage counts
    // survive into serialization; release the graph-construction workspaces
    // before allocating an optional bounded name cache.
    let node_id_space = usize::try_from(n_act)
        .ok()
        .and_then(|n| n.checked_add(kid_id.len().max(defined_count + structure_keys.len())));
    let alias_count = aliases.len();
    drop(aliases);
    drop(structure_kids);
    drop(structure_keys);
    drop(defs);
    drop(id_key);
    drop(kid_id);
    drop(named_order);
    drop(queue);
    drop(seen_deps);
    let nm = |id: u32| -> &str {
        if id < n_act {
            ctx.names[id as usize]
        } else {
            tset_name[&(id - n_act)].as_str()
        }
    };
    let mut name_cache = node_id_space.and_then(|id_space| {
        graph_name_cache(
            id_space,
            ctx.names
                .iter()
                .map(|name| name.len())
                .chain(tset_name.values().map(String::len)),
        )
    });
    if parallel.is_some() {
        if let Some(cache) = &mut name_cache {
            populate_graph_name_cache(cache, &rows, &nm)?;
            populate_graph_name_cache(cache, &structure_rows, &nm)?;
        }
    }

    // A collision on the staging name is a leftover from a dead
    // process; it is refused, never truncated.
    let mut w = artifact_writer(path)?;
    let mut header = json!({"graph": {
        "dir": ctx.root, "pid": PID, "makelevel": 0, "goals": ctx.goals,
        "kind": "full", "enc": 2, "run": ctx.trace_id, "capture": ctx.capture,
        "coverage": {"nodes": rows.len() + structure_rows.len(), "edges": edges,
                     "action_rows": rows.iter().filter(|r| !r.2).count(),
                     "join_rows": rows.iter().filter(|r| r.2).count() + structure_rows.len(),
                     "dangling_deps": unmapped, "dangling_tsets": undefined.len(),
                     "dep_capture": ctx.dep_capture},
    }});
    if ctx.dep_capture.get("scope").and_then(Value::as_str) == Some("current_action_ancestors") {
        // Old readers reject enc:3 instead of silently treating the subset as full.
        header["graph"]["kind"] = json!("current_action_ancestors");
        header["graph"]["enc"] = json!(3);
        header["graph"]["note"] = json!(crate::quality::ACTIVITY_SCOPE_NOTE);
    }
    if ctx.dep_capture.get("structure").is_some() {
        header["graph"]["coverage"]["structural_rows"] = json!(structure_rows.len());
        header["graph"]["coverage"]["canonical_aliases"] = json!(alias_count);
    }
    writeln!(w, "{}", serde_json::to_string(&header)?)?;
    if let Some(options) = parallel {
        if let Some(cache) = &name_cache {
            write_parallel_graph_rows_with_cache(&mut w, &rows, options, &nm, cache)?;
        } else {
            write_parallel_graph_rows(&mut w, &rows, options, &nm)?;
        }
    } else {
        let mut dn: Vec<&str> = Vec::new();
        for (t, d, j) in &rows {
            if let Some(cache) = &mut name_cache {
                write_cached_graph_row(&mut w, *t, d, *j, cache, &nm)?;
            } else {
                dn.clear();
                dn.extend(d.iter().map(|&x| nm(x)));
                write_graph_row(&mut w, nm(*t), &dn, *j)?;
            }
        }
    }
    // These rows carry per-definition provenance, without becoming trace
    // actions or measured work: whether the recorder named the reused action
    // without its inputs because nothing that ran lies below it.
    #[derive(serde::Serialize)]
    struct StructureRow<'a> {
        d: &'a [&'a str],
        inputs_omitted: bool,
        j: u8,
        t: &'a str,
    }
    let mut dependency_names = Vec::new();
    for (target, deps, _) in &structure_rows {
        dependency_names.clear();
        dependency_names.extend(deps.iter().map(|&id| nm(id)));
        let row = StructureRow {
            d: &dependency_names,
            inputs_omitted: structure_inputs_omitted[(*target - n_act) as usize - defined_count],
            j: 1,
            t: nm(*target),
        };
        serde_json::to_writer(&mut w, &row)?;
        w.write_all(b"\n")?;
    }
    let digest = finish_artifact(w)?;
    Ok((rows.len() + structure_rows.len(), edges, digest))
}

#[cfg(test)]
mod parallel_emit_tests {
    use super::*;

    #[test]
    fn unknown_action_kinds_keep_their_number() {
        assert_eq!(action_kind_name(0), "");
        assert_eq!(action_kind_name(3), "Run");
        assert_eq!(action_kind_name(9999), "kind_9999");
    }

    #[test]
    fn re_served_results_are_cache_hits_and_deduplicated_ones_are_executions() {
        use buck2_data::ActionExecutionKind;
        let served = ActionExecutionKind::RemoteServedFromCache as i32;
        let deduplicated = ActionExecutionKind::RemoteDeduplicated as i32;
        assert_eq!(exec_kind_name(served), "remote_served_from_cache");
        assert_eq!(exec_kind_name(deduplicated), "remote_deduplicated");
        assert!(CACHE_KINDS.contains(&served));
        assert!(
            !CACHE_KINDS.contains(&deduplicated),
            "a deduplicated execution ran, shared with another build"
        );
    }

    #[test]
    fn resolved_graph_edges_are_unique_in_first_seen_order() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-graph-resolved-order-{}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("should create graph test directory");
        let by_sid = HashMap::from([(10, 0), (11, 1)]);
        let key_to_idx = HashMap::from([(100, 0)]);
        let ctx = GraphCtx {
            root: "/repo",
            trace_id: "fixture",
            capture: "test-capture",
            goals: &[],
            gap_reason: None,
            dep_capture: &json!({}),
            by_sid: &by_sid,
            key_to_idx: &key_to_idx,
            names: &["producer", "other", "consumer"],
        };
        // Fixed golden for the pre-dedup ordering, independent of the trace
        // stamp (unknown exits intentionally change trace serialization).
        let expected = concat!(
            "{\"graph\":{\"capture\":\"test-capture\",\"coverage\":{\"action_rows\":3,\"dangling_deps\":0,\"dangling_tsets\":0,\"dep_capture\":{},\"edges\":5,\"join_rows\":2,\"nodes\":5},\"dir\":\"/repo\",\"enc\":2,\"goals\":[],\"kind\":\"full\",\"makelevel\":0,\"pid\":1,\"run\":\"fixture\"}}\n",
            "{\"d\":[],\"t\":\"producer\"}\n",
            "{\"d\":[],\"t\":\"other\"}\n",
            "{\"d\":[\"other\",\"producer\",\"tset:7\"],\"t\":\"consumer\"}\n",
            "{\"d\":[\"producer\",\"tset:8\"],\"j\":1,\"t\":\"tset:7\"}\n",
            "{\"d\":[],\"j\":1,\"t\":\"tset:8\"}\n",
        );
        for duplicates in [false, true] {
            for parallel in [None, Some(options(2, 1, 128))] {
                let empty = || DepRec {
                    span_ids: vec![],
                    keys: vec![],
                    tsets: vec![],
                };
                let deps = vec![
                    empty(),
                    empty(),
                    DepRec {
                        span_ids: if duplicates {
                            vec![11, 10, 11]
                        } else {
                            vec![11, 10]
                        },
                        keys: if duplicates { vec![100, 101] } else { vec![] },
                        tsets: if duplicates { vec![7, 7] } else { vec![7] },
                    },
                ];
                let defs = HashMap::from([
                    (
                        7,
                        TsetDef {
                            own_keys: if duplicates { vec![100, 101] } else { vec![] },
                            own_span_ids: if duplicates { vec![10, 10] } else { vec![10] },
                            child_ids: if duplicates { vec![8, 8] } else { vec![8] },
                            contents_omitted: false,
                        },
                    ),
                    (
                        8,
                        TsetDef {
                            own_keys: vec![],
                            own_span_ids: vec![],
                            child_ids: vec![],
                            contents_omitted: false,
                        },
                    ),
                ]);
                let aliases =
                    HashMap::from([(101, crate::ingest::scan::AliasDef { canonical: 100 })]);
                let staged = Staged::beside(dir.join("graph.json"));
                let mut warnings = Vec::new();
                let (nodes, edges, _) = write_full_graph(
                    &ctx,
                    deps,
                    defs,
                    HashMap::new(),
                    aliases,
                    &staged.write_to,
                    &mut warnings,
                    parallel,
                )
                .expect("resolved graph should emit");
                assert_eq!((nodes, edges), (5, 5));
                assert!(warnings.is_empty(), "{warnings:?}");
                assert_eq!(
                    std::fs::read(&staged.write_to).expect("should read graph"),
                    expected.as_bytes()
                );
            }
        }
    }

    #[test]
    fn cache_summary_totals_saturate_large_bytes_and_durations() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-cache-summary-overflow-{}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("should create summary test directory");
        let log =
            crate::ingest::infer(&crate::ingest::testlog::demo_log(&dir, Some(1), true, true))
                .expect("fixture path should be recognized");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("should create ingestion runtime");
        // Test both an oversized individual u64 and overflow only in the sum.
        for sizes in [[u64::MAX, 1], [i64::MAX as u64 / 2 + 1; 2]] {
            let mut scan = runtime
                .block_on(crate::ingest::scan::scan(&log))
                .expect("fixture should scan");
            for (action, size) in scan.actions.iter_mut().zip(sizes) {
                action.execution_kind = 3;
                action.output_size = size;
                scan.stages.insert(
                    action.sid,
                    vec![StageRec {
                        name: "CacheQuery",
                        ts: action.ts,
                        dur: i64::MAX / 2 + 1,
                        cancelled: false,
                        details: None,
                        cache_timings_invalid: false,
                    }],
                );
            }
            let trace = Staged::beside(dir.join("trace.json"));
            emit(scan, &trace, None, false).expect("large counters should not overflow");
            let rows: Value =
                serde_json::from_slice(&std::fs::read(&trace.write_to).expect("should read trace"))
                    .expect("trace should be JSON");
            let end = rows
                .as_array()
                .expect("trace should be an array")
                .iter()
                .find(|r| r["name"] == "run_end")
                .expect("run end should exist");
            assert_eq!(end["args"]["cache"]["restored_bytes"], i64::MAX);
            assert_eq!(end["args"]["cache"]["key_us"], i64::MAX);
        }
    }

    fn options(workers: usize, records_per_job: usize, chunk_bytes: usize) -> ParallelEmit {
        ParallelEmit {
            workers,
            records_per_job,
            chunk_bytes,
        }
    }

    fn serial_rows(values: &[Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in values {
            serde_json::to_writer(&mut bytes, value).unwrap();
            bytes.extend_from_slice(b",\n");
        }
        bytes
    }

    #[test]
    fn parallel_graph_rows_preserve_borrowed_and_cached_bytes() {
        let names = [
            "ordinary".to_owned(),
            "quote\"\\\n\u{0}\u{1f600}".to_owned(),
            "big".repeat(1000),
        ];
        let nm = |id: u32| names[id as usize].as_str();
        let rows: Vec<_> = (0..37)
            .map(|i| {
                (
                    i % 3,
                    if i % 5 == 0 { vec![] } else { vec![2, 1, 0, 1] },
                    i % 2 == 0,
                )
            })
            .collect();
        let mut expected = Vec::new();
        let mut cached = Vec::new();
        let mut cache = graph_name_cache(names.len(), names.iter().map(String::len)).unwrap();
        for (t, d, j) in &rows {
            write_graph_row(
                &mut expected,
                nm(*t),
                &d.iter().map(|id| nm(*id)).collect::<Vec<_>>(),
                *j,
            )
            .unwrap();
            write_cached_graph_row(&mut cached, *t, d, *j, &mut cache, &nm).unwrap();
        }
        assert_eq!(cached, expected);
        for workers in [1, 2, 4, 16] {
            for chunk_bytes in [1, 64, 4096, 64 * 1024] {
                let mut actual = Vec::new();
                let stats = write_parallel_graph_rows(
                    &mut actual,
                    &rows,
                    options(workers, 3, chunk_bytes),
                    &nm,
                )
                .unwrap();
                assert_eq!(actual, expected);
                assert_eq!(stats.records, rows.len());
                assert_eq!(stats.bytes, expected.len() as u64);
                assert!(stats.max_chunk_records <= 3);
            }
        }
        let mut empty = Vec::new();
        let stats = write_parallel_graph_rows(&mut empty, &[], options(4, 3, 64), &nm).unwrap();
        assert_eq!(stats.records, 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn zero_workers_select_serial_and_positive_counts_keep_bounded_jobs() {
        assert!(parallel_emit_options(0).is_none());
        for workers in [1, 2, 4, 8] {
            let options = parallel_emit_options(workers).unwrap();
            assert_eq!(options.workers, workers);
            assert_eq!(options.records_per_job, 256);
            assert_eq!(options.chunk_bytes, 256 * 1024);
        }
    }

    #[test]
    fn parallel_rows_match_serial_bytes_with_empty_small_and_oversized_chunks() {
        let values: Vec<_> = (0..73)
            .map(|i| {
                json!({
                    "serial": i, "name": "quote\"\\\n\u{00e9}\u{1f600}",
                    "payload": if i % 11 == 0 { "large".repeat(1000) } else { "small".to_owned() },
                    "zero": 0, "missing": null,
                })
            })
            .collect();
        let expected = serial_rows(&values);
        for workers in [1, 2, 4] {
            for chunk_bytes in [1, 64, 1024, 16 * 1024] {
                let mut actual = Vec::new();
                let stats = write_parallel_records(
                    &mut actual,
                    values.len(),
                    options(workers, 7, chunk_bytes),
                    &|i, bytes| {
                        serde_json::to_writer(bytes, &values[i])?;
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(actual, expected);
                assert_eq!(stats.records, values.len());
                assert_eq!(stats.bytes, expected.len() as u64);
                assert!(stats.max_chunk_records <= 7);
                assert_eq!(
                    stats.oversized_records,
                    values
                        .iter()
                        .filter(|v| serde_json::to_vec(v).unwrap().len() + 2 > chunk_bytes)
                        .count()
                );
            }
        }
        let mut empty = Vec::new();
        let stats = write_parallel_records(&mut empty, 0, options(4, 7, 64), &|_, _| {
            panic!("no records")
        })
        .unwrap();
        assert!(empty.is_empty());
        assert_eq!(stats.workers, 0);
        assert_eq!(stats.records, 0);
    }

    #[test]
    fn later_job_can_finish_first_without_changing_order() {
        let gate = (std::sync::Mutex::new(false), std::sync::Condvar::new());
        let values: Vec<_> = (0..12).map(|i| json!({"index": i})).collect();
        let mut actual = Vec::new();
        write_parallel_records(&mut actual, values.len(), options(2, 2, 32), &|i, bytes| {
            if i == 0 {
                let ready = gate.0.lock().unwrap();
                drop(gate.1.wait_while(ready, |ready| !*ready).unwrap());
            }
            serde_json::to_writer(bytes, &values[i])?;
            if i == 3 {
                *gate.0.lock().unwrap() = true;
                gate.1.notify_one();
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, serial_rows(&values));
    }

    #[test]
    fn writer_and_renderer_failures_disconnect_bounded_workers() {
        struct FailAfter {
            remaining: usize,
            bytes: Vec<u8>,
        }
        impl std::io::Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Err(std::io::Error::other("test write failure"));
                }
                let n = self.remaining.min(bytes.len());
                self.bytes.extend_from_slice(&bytes[..n]);
                self.remaining -= n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let values: Vec<_> = (0..80).map(|i| json!({"index": i})).collect();
        let expected = serial_rows(&values);
        for limit in [0, 1, 20, expected.len() - 1] {
            let mut writer = FailAfter {
                remaining: limit,
                bytes: Vec::new(),
            };
            let result = write_parallel_records(
                &mut writer,
                values.len(),
                options(4, 5, 32),
                &|i, bytes| {
                    serde_json::to_writer(bytes, &values[i])?;
                    Ok(())
                },
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("test write failure")
            );
            assert_eq!(writer.bytes, expected[..limit]);
        }
        let result = write_parallel_records(&mut Vec::new(), 40, options(4, 5, 32), &|i, bytes| {
            anyhow::ensure!(i != 7, "test render failure");
            serde_json::to_writer(bytes, &json!({"index": i}))?;
            Ok(())
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("test render failure")
        );
        // An aborting panic profile terminates the entire test process rather
        // than returning a JoinHandle error. Ordinary failure coverage above
        // applies to both profiles; this deliberate panic requires unwinding.
        #[cfg(panic = "unwind")]
        {
            let result =
                write_parallel_records(&mut Vec::new(), 40, options(4, 5, 32), &|i, bytes| {
                    assert!(i != 7, "test worker panic");
                    serde_json::to_writer(bytes, &json!({"index": i}))?;
                    Ok(())
                });
            assert!(result.is_err());
        }
    }

    #[test]
    fn serial_and_parallel_full_pairs_keep_identical_capture_identity() {
        let dir = std::env::temp_dir().join(format!(
            "retrace-production-emit-parity-{}",
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
            for (trace_workers, graph_workers) in [(0, 0), (1, 1), (4, 4), (8, 4)] {
                let scan = runtime
                    .block_on(crate::ingest::scan::scan_with_phases(&log, phases))
                    .unwrap();
                let trace = Staged::beside(dir.join("trace.json"));
                let graph = Staged::beside(dir.join("graph.json"));
                let summary = emit_with_options(
                    scan,
                    &trace,
                    Some(&graph),
                    phases,
                    parallel_emit_options(trace_workers),
                    parallel_emit_options(graph_workers),
                )
                .unwrap();
                assert!(matches!(summary.graph, GraphOutcome::Written));
                let actual = (
                    std::fs::read(&trace.write_to).unwrap(),
                    std::fs::read(&graph.write_to).unwrap(),
                    summary.capture,
                );
                if let Some(expected) = &expected {
                    assert_eq!(
                        &actual, expected,
                        "trace={trace_workers}, graph={graph_workers}, phases={phases}"
                    );
                } else {
                    expected = Some(actual);
                }
            }
        }
    }

    #[test]
    fn parallel_trace_preserves_real_activity_phases_counters_and_capture_stamp() {
        let dir =
            std::env::temp_dir().join(format!("retrace-parallel-emit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::ingest::testlog::wait_state_log(
            &dir,
            crate::ingest::testlog::WaitStateCase::LocalWinner { download_end: true },
        );
        let log = crate::ingest::infer(&path).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let render = |parallel| {
            let mut scan = runtime.block_on(crate::ingest::scan::scan(&log)).unwrap();
            scan.phases.clear();
            scan.snapshots.clear();
            for (kind, ts, dur, rule) in [
                ("Analysis", scan.actions[0].ts, 2000, "cxx_library"),
                ("Materialization", -1, 4000, ""),
                ("Load", 0, 999, ""),
            ] {
                scan.phases.push(crate::ingest::scan::PhaseRec {
                    kind,
                    ts,
                    dur,
                    detail: "quoted\"\\\n detail".to_owned(),
                    rule: rule.to_owned(),
                });
            }
            scan.snapshots.push(crate::ingest::scan::SnapRec {
                ts: 123,
                heap: 0,
                rss_max: 15,
                dl_bytes: 21,
                ul_bytes: 0,
                cpu_us: 33,
            });
            let trace = Staged::beside(dir.join("trace.json"));
            let summary = emit_with_options(scan, &trace, None, true, parallel, None).unwrap();
            (std::fs::read(&trace.write_to).unwrap(), summary.capture)
        };
        let expected = render(None);
        let rows: Value = serde_json::from_slice(&expected.0).unwrap();
        assert_eq!(rows[0]["args"]["capture"], expected.1);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .filter(|r| r["cat"] == "phase")
                .count(),
            2
        );
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .filter(|r| r["cat"] == "snapshot")
                .count(),
            1
        );
        for workers in [1, 2, 4] {
            for chunk_bytes in [1, 128, 4096] {
                assert_eq!(render(Some(options(workers, 1, chunk_bytes))), expected);
            }
        }
    }
}

#[cfg(test)]
mod shared_graph_cache_tests {
    use super::*;

    fn options(workers: usize, bytes: usize) -> ParallelEmit {
        ParallelEmit {
            workers,
            records_per_job: 1,
            chunk_bytes: bytes,
        }
    }

    #[test]
    fn shared_graph_cache_preload_skips_cached_invalid_and_nonadmitted_names() {
        assert!(graph_name_fits_remaining(0, 2));
        assert!(!graph_name_fits_remaining(0, 1));
        assert!(graph_name_fits_remaining(1, 8));
        assert!(!graph_name_fits_remaining(1, 7));
        assert!(!graph_name_fits_remaining(usize::MAX, usize::MAX));
        assert!(!graph_name_fits_remaining(usize::MAX / 6 + 1, usize::MAX));

        let mut cache =
            GraphNameCache::new(2 * std::mem::size_of::<EncodedGraphName>() + 8, 2).unwrap();
        let calls = [std::cell::Cell::new(0), std::cell::Cell::new(0)];
        let name = |id: u32| {
            let counter = calls.get(id as usize).expect("out-of-index name rendered");
            counter.set(counter.get() + 1);
            "a"
        };
        let rows = vec![
            (0, Vec::new(), false),
            (0, Vec::new(), false),
            (u32::MAX, Vec::new(), true),
            (2, Vec::new(), false),
            (1, Vec::new(), false),
        ];
        populate_graph_name_cache(&mut cache, &rows, &name).unwrap();
        assert_eq!(calls.each_ref().map(std::cell::Cell::get), [1, 1]);
        assert_eq!(cache.encoded_name(0).unwrap(), Some(b"\"a\"".as_slice()));
        assert!(cache.encoded_name(1).unwrap().is_none());
        assert_eq!(cache.used, 3);

        // Preload must still validate cached ranges, not silently skip a
        // corrupt entry merely because its length is nonzero.
        cache.index[0].offset = u32::MAX;
        assert!(populate_graph_name_cache(&mut cache, &rows, &name).is_err());
        cache.index[0] = EncodedGraphName::default();
        cache.used = cache.arena.len() + 1;
        assert!(populate_graph_name_cache(&mut cache, &rows, &name).is_err());
    }

    #[test]
    fn shared_graph_cache_preserves_bytes_holes_fallback_and_fixed_storage() {
        // The row order puts a long action first, but admission reserves the
        // short join first. The conservative encoding bound then leaves the
        // action uncached without changing its eventual serialized bytes.
        let priority_names = ["action-name-padding", "tset:7"];
        let priority_rows = vec![(0, Vec::new(), false), (1, vec![0], true)];
        let mut priority = GraphNameCache::new(
            2 * std::mem::size_of::<EncodedGraphName>() + priority_names[0].len() * 6 + 2,
            2,
        )
        .unwrap();
        populate_graph_name_cache(&mut priority, &priority_rows, &|id| {
            priority_names[id as usize]
        })
        .unwrap();
        assert!(priority.encoded_name(1).unwrap().is_some());
        assert!(priority.encoded_name(0).unwrap().is_none());

        let large = "oversized\"\\\n".repeat(4096);
        let name = |id| match id {
            2 => "q\"",
            7 => large.as_str(),
            9 => "\u{00e9}\u{1f600}",
            42 => "out-of-index fallback",
            _ => panic!("attempt to render an ID-space hole"),
        };
        let rows = vec![
            (2, vec![9, 2, 7, 42, 2], false),
            (7, vec![2, 9], true),
            (9, Vec::new(), false),
        ];
        let mut expected = Vec::new();
        for (target, deps, join) in &rows {
            let deps: Vec<_> = deps.iter().map(|&id| name(id)).collect();
            write_graph_row(&mut expected, name(*target), &deps, *join).unwrap();
        }
        let index_bytes = 10 * std::mem::size_of::<EncodedGraphName>();
        for extra in [2, 128, 4096] {
            let budget = index_bytes + extra;
            let mut cache = GraphNameCache::new(budget, 10).unwrap();
            populate_graph_name_cache(&mut cache, &rows, &name).unwrap();
            assert!(cache.encoded_name(0).unwrap().is_none());
            assert!(cache.encoded_name(42).unwrap().is_none());
            assert!(cache.encoded_name(7).unwrap().is_none());
            if extra >= 128 {
                assert_eq!(
                    cache.encoded_name(2).unwrap().unwrap(),
                    serde_json::to_vec(name(2)).unwrap()
                );
            }
            let retained = cache.index_bytes() + cache.arena.capacity();
            assert!(retained <= budget);
            let pointers = (cache.index.as_ptr(), cache.arena.as_ptr());
            let used = cache.used;
            for workers in [1, 2, 4] {
                for chunk_bytes in [1, 64, 4096] {
                    let mut actual = Vec::new();
                    let stats = write_parallel_graph_rows_with_cache(
                        &mut actual,
                        &rows,
                        options(workers, chunk_bytes),
                        &name,
                        &cache,
                    )
                    .unwrap();
                    assert_eq!(actual, expected);
                    assert_eq!(stats.records, rows.len());
                    assert_eq!(stats.bytes, expected.len() as u64);
                    assert_eq!(cache.used, used);
                    assert_eq!(cache.index_bytes() + cache.arena.capacity(), retained);
                    assert_eq!((cache.index.as_ptr(), cache.arena.as_ptr()), pointers);
                }
            }
        }
    }

    #[test]
    fn shared_graph_cache_keeps_checked_ranges_and_writer_errors() {
        struct Fail;
        impl std::io::Write for Fail {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("shared cache write failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let rows = vec![(0, vec![0], false)];
        let mut cache = GraphNameCache::new(128, 1).unwrap();
        populate_graph_name_cache(&mut cache, &rows, &|_| "name").unwrap();
        let error = write_parallel_graph_rows_with_cache(
            &mut Fail,
            &rows,
            options(2, 64),
            &|_| panic!("cached name must not need the fallback"),
            &cache,
        )
        .unwrap_err();
        assert!(error.to_string().contains("shared cache write failure"));
        cache.index[0].offset = u32::MAX;
        assert!(cache.encoded_name(0).is_err());
        assert!(
            write_parallel_graph_rows_with_cache(
                &mut Vec::new(),
                &rows,
                options(2, 64),
                &|_| "name",
                &cache,
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod artifact_buffer_tests {
    use super::*;

    fn emit_pair(capacity: Option<usize>, name_cache: bool) -> (Vec<u8>, Vec<u8>, String) {
        // Existing staging guards provide unique names and remove both test
        // files on drop; no process-global environment mutation is needed.
        let trace = Staged::beside(std::env::temp_dir().join("retrace-buffer-trace.json"));
        let graph = Staged::beside(std::env::temp_dir().join("retrace-buffer-graph.json"));
        let mut tw = match capacity {
            Some(capacity) => artifact_writer_with_capacity(&trace.write_to, capacity),
            None => artifact_writer(&trace.write_to),
        }
        .unwrap();
        writeln!(tw, "[").unwrap();
        serde_json::to_writer(
            &mut tw,
            &json!({"name": "run_start", "args": {"capture": CAPTURE_PLACEHOLDER}}),
        )
        .unwrap();
        tw.write_all(b"\n]\n").unwrap();
        let trace_digest = finish_artifact(tw).unwrap();

        let mut gw = match capacity {
            Some(capacity) => artifact_writer_with_capacity(&graph.write_to, capacity),
            None => artifact_writer(&graph.write_to),
        }
        .unwrap();
        let header = json!({"graph": {"capture": CAPTURE_PLACEHOLDER}});
        serde_json::to_writer(&mut gw, &header).unwrap();
        gw.write_all(b"\n").unwrap();
        gw.flush().unwrap();
        let mut header_bytes = serde_json::to_vec(&header).unwrap();
        header_bytes.push(b'\n');
        assert_eq!(std::fs::read(&graph.write_to).unwrap(), header_bytes);

        // This row exceeds the smaller tested buffers; the final smaller row
        // also checks finish_artifact's buffered tail.
        let long = "edge\"\\\n".repeat(4096);
        if name_cache {
            let names = [long.as_str(), "null", "", "last"];
            let nm = |id: u32| names[id as usize];
            let mut cache =
                graph_name_cache(names.len(), names.iter().map(|name| name.len())).unwrap();
            write_cached_graph_row(&mut gw, 0, &[0, 1, 2], true, &mut cache, &nm).unwrap();
            write_cached_graph_row(&mut gw, 3, &[], false, &mut cache, &nm).unwrap();
            assert_ne!(cache.index[0].len, 0);
        } else {
            write_graph_row(&mut gw, &long, &[&long, "null", ""], true).unwrap();
            write_graph_row(&mut gw, "last", &[], false).unwrap();
        }
        let graph_digest = finish_artifact(gw).unwrap();
        for (path, digest) in [
            (&trace.write_to, &trace_digest),
            (&graph.write_to, &graph_digest),
        ] {
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(digest.bytes, bytes.len() as u64);
            let expected: [u8; 20] = sha1::Sha1::digest(&bytes).into();
            assert_eq!(digest.digest, expected);
        }
        let capture = capture_fingerprint(&trace_digest, Some(&graph_digest));
        stamp_artifact(&trace.write_to, true, &capture).unwrap();
        stamp_artifact(&graph.write_to, false, &capture).unwrap();
        (
            std::fs::read(&trace.write_to).unwrap(),
            std::fs::read(&graph.write_to).unwrap(),
            capture,
        )
    }

    #[test]
    fn artifact_bytes_flush_and_capture_are_independent_of_buffer_capacity() {
        let expected = emit_pair(None, false);
        for capacity in [1, 17, 4096, 1024 * 1024] {
            assert_eq!(emit_pair(Some(capacity), false), expected);
        }
    }

    #[test]
    fn graph_name_cache_preserves_artifact_bytes_and_capture() {
        let expected = emit_pair(None, false);
        assert_eq!(emit_pair(None, true), expected);
        assert_eq!(emit_pair(Some(1024 * 1024), true), expected);
    }
}

#[cfg(test)]
mod lane_order_tests {
    use super::*;

    fn slots(specs: &[(i64, i64)]) -> Vec<EvSlot> {
        specs
            .iter()
            .enumerate()
            .map(|(i, &(ts, dur))| EvSlot {
                ts,
                dur,
                recipe: i % 3 != 0,
                idx: i * 5 + 7,
                lane: usize::MAX,
            })
            .collect()
    }

    fn assert_reference(specs: &[(i64, i64)]) {
        let mut actual = slots(specs);
        let (count, order) = pack_lanes(&mut actual);
        // An explicit original-index tie breaker independently specifies the
        // stable timestamp order formerly sorted again for file emission.
        let mut expected_order: Vec<usize> = (0..specs.len()).collect();
        expected_order.sort_unstable_by_key(|&i| (specs[i].0, i));
        assert_eq!(order, expected_order);

        // A simple lane table specifies the old heap algorithm's contract:
        // use the lowest available lane, with the same release arithmetic.
        let mut release: Vec<i64> = Vec::new();
        for &i in &expected_order {
            let (ts, dur) = specs[i];
            let lane = release
                .iter()
                .position(|&end| end <= ts)
                .unwrap_or_else(|| {
                    release.push(ts);
                    release.len() - 1
                });
            release[lane] = ts.saturating_add(dur.max(1));
            assert_eq!(actual[i].lane, lane);
        }
        assert_eq!(count, release.len());
        for (i, slot) in actual.iter().enumerate() {
            assert_eq!((slot.ts, slot.dur), specs[i]);
            assert_eq!(slot.recipe, i % 3 != 0);
            assert_eq!(slot.idx, i * 5 + 7);
        }
    }

    #[test]
    fn shared_order_keeps_equal_timestamp_action_and_phase_identity() {
        let specs = [(10, 10), (-5, 5), (10, -3), (-5, 0), (20, 1)];
        let mut actual = slots(&specs);
        let (count, order) = pack_lanes(&mut actual);
        assert_eq!(order, vec![1, 3, 0, 2, 4]);
        assert_eq!(count, 2);
        assert_eq!(
            actual.iter().map(|slot| slot.lane).collect::<Vec<_>>(),
            vec![0, 0, 1, 1, 0]
        );
        assert_reference(&specs);
    }

    #[test]
    fn shared_order_preserves_empty_zero_negative_and_saturating_durations() {
        assert_reference(&[]);
        assert_reference(&[(0, 0)]);
        assert_reference(&[(0, -1), (0, 0), (1, 0), (1, 2), (3, 1)]);
        assert_reference(&[
            (i64::MAX, 10),
            (i64::MIN, i64::MAX),
            (-1, i64::MAX),
            (i64::MIN, i64::MIN),
            (i64::MAX, 0),
            (i64::MAX - 1, 10),
        ]);
    }

    #[test]
    fn shared_order_matches_reference_across_repeated_timestamps_and_lane_reuse() {
        for seed in 0..128u64 {
            let mut state = seed + 1;
            let specs: Vec<_> = (0..32)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    ((state % 13) as i64 - 6, ((state >> 8) % 9) as i64 - 2)
                })
                .collect();
            assert_reference(&specs);
        }
    }
}

#[cfg(test)]
mod graph_row_tests {
    use super::*;

    fn assert_legacy_bytes(target: &str, deps: &[&str], join: bool) -> Vec<u8> {
        let legacy = if join {
            json!({"t": target, "d": deps, "j": 1})
        } else {
            json!({"t": target, "d": deps})
        };
        let mut expected = serde_json::to_string(&legacy).unwrap().into_bytes();
        expected.push(b'\n');

        let mut actual = Vec::new();
        write_graph_row(&mut actual, target, deps, join).unwrap();
        assert_eq!(actual, expected);
        actual
    }

    #[test]
    fn action_and_join_rows_preserve_key_and_dependency_order() {
        let deps = ["z", "a", "z", "null", "unknown", ""];
        let action = assert_legacy_bytes("action", &deps, false);
        assert_eq!(
            action,
            b"{\"d\":[\"z\",\"a\",\"z\",\"null\",\"unknown\",\"\"],\"t\":\"action\"}\n"
        );
        let join = assert_legacy_bytes("tset:42", &deps, true);
        assert_eq!(
            join,
            b"{\"d\":[\"z\",\"a\",\"z\",\"null\",\"unknown\",\"\"],\"j\":1,\"t\":\"tset:42\"}\n"
        );
    }

    #[test]
    fn empty_graph_rows_preserve_empty_arrays_and_omit_absent_join_marker() {
        assert_eq!(
            assert_legacy_bytes("", &[], false),
            b"{\"d\":[],\"t\":\"\"}\n"
        );
        assert_eq!(
            assert_legacy_bytes("tset:0", &[], true),
            b"{\"d\":[],\"j\":1,\"t\":\"tset:0\"}\n"
        );
    }

    #[test]
    fn graph_rows_preserve_json_escaping_and_unicode() {
        let controls: String = (0u8..=31).map(char::from).collect();
        let escaped = "quote\" backslash\\ slash/ <>& \u{00e9} \u{1f600} \u{2028}\u{2029}";
        for join in [false, true] {
            assert_legacy_bytes(&controls, &[escaped, &controls, ""], join);
            assert_legacy_bytes(escaped, &[&controls, escaped], join);
        }
    }

    #[test]
    fn long_graph_rows_match_across_buffer_boundaries() {
        let long_name = "long\"\\\n\u{1f600}".repeat(4096);
        let owned: Vec<String> = (0..4096).map(|i| format!("dep-{i}\\\"")).collect();
        let mut deps: Vec<&str> = owned.iter().map(String::as_str).collect();
        deps.push(&long_name);
        for join in [false, true] {
            let expected = assert_legacy_bytes(&long_name, &deps, join);
            let mut actual = Vec::new();
            {
                let mut w = std::io::BufWriter::with_capacity(17, &mut actual);
                write_graph_row(&mut w, &long_name, &deps, join).unwrap();
                w.flush().unwrap();
            }
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn first_use_admits_name_and_hits_skip_resolution() {
        let mut cache = GraphNameCache::new(1024, 1).unwrap();
        let name = "\"\\\n\u{0000}\u{1f600}";
        let expected = serde_json::to_vec(name).unwrap();
        let mut actual = Vec::new();
        cache.write_name(&mut actual, 0, || name).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(cache.used, expected.len());
        assert_eq!(cache.index[0].offset, 0);
        assert_eq!(cache.index[0].len as usize, expected.len());
        cache
            .write_name(&mut actual, 0, || panic!("a hit must not resolve the name"))
            .unwrap();
        assert_eq!(actual, expected.repeat(2));
    }

    #[test]
    fn adaptive_cache_budget_bounds_small_graphs_and_saturates_overflow() {
        let maximum = GRAPH_NAME_CACHE_MAX_BYTES;
        assert_eq!(
            adaptive_graph_name_cache_budget(maximum, 2, [3, 5]),
            Some(68)
        );
        assert_eq!(
            adaptive_graph_name_cache_budget(maximum, 2, [0, 0]),
            Some(20)
        );
        assert_eq!(
            adaptive_graph_name_cache_budget(128, 2, [usize::MAX]),
            Some(128)
        );
        assert_eq!(
            adaptive_graph_name_cache_budget(128, 2, [10, 10, 10]),
            Some(128)
        );
        assert_eq!(
            adaptive_graph_name_cache_budget(maximum, usize::MAX, [1]),
            None
        );
        assert_eq!(adaptive_graph_name_cache_budget(8, 1, [0]), None);
        assert_eq!(adaptive_graph_name_cache_budget(0, 0, [1]), None);
        assert_eq!(adaptive_graph_name_cache_budget(maximum, 0, []), None);
        assert!(GraphNameCache::new(0, 0).is_err());
        assert!(GraphNameCache::new(8, 1).is_err());
        assert!(GraphNameCache::new(4096, usize::MAX).is_err());
        assert!(GraphNameCache::new(GRAPH_NAME_CACHE_MAX_BYTES + 1, 0).is_err());
        let cache = graph_name_cache(2, [3, 5]).unwrap();
        assert!(cache.index_bytes() + cache.arena.capacity() <= 68);
    }

    #[test]
    fn production_cache_keeps_small_graphs_small_and_falls_back_for_large_ids() {
        let names = ["small", "leaf"];
        let nm = |id: u32| names[id as usize];
        let expected = assert_legacy_bytes(names[0], &[names[1]], false);
        let mut cache = graph_name_cache(names.len(), names.iter().map(|name| name.len())).unwrap();
        assert!(cache.index_bytes() + cache.arena.capacity() <= 74);
        let mut actual = Vec::new();
        write_cached_graph_row(&mut actual, 0, &[1], false, &mut cache, &nm).unwrap();
        assert_eq!(actual, expected);

        for id_space in [
            usize::MAX,
            GRAPH_NAME_CACHE_MAX_BYTES / std::mem::size_of::<EncodedGraphName>(),
        ] {
            let mut actual = Vec::new();
            let cache = graph_name_cache(id_space, names.iter().map(|name| name.len()));
            assert!(cache.is_none());
            if let Some(mut cache) = cache {
                write_cached_graph_row(&mut actual, 0, &[1], false, &mut cache, &nm).unwrap();
            } else {
                write_graph_row(&mut actual, nm(0), &[nm(1)], false).unwrap();
            }
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn cached_and_uncached_rows_match_for_escapes_empty_names_and_duplicate_ids() {
        let controls: String = (0u8..=31).map(char::from).collect();
        let names = [
            "target\"\\",
            controls.as_str(),
            "\u{00e9}\u{1f600}\u{2028}",
            "",
            "null",
            "same",
            "same",
        ];
        let nm = |id: u32| names[id as usize];
        let deps = [6, 1, 2, 5, 3, 4, 1];
        let dependency_names: Vec<_> = deps.iter().map(|&id| nm(id)).collect();
        for join in [false, true] {
            let expected = assert_legacy_bytes(nm(0), &dependency_names, join);
            for budget in [0, 1, 4096] {
                let mut cache = if budget == 0 {
                    None
                } else {
                    GraphNameCache::new(budget, names.len()).ok()
                };
                let mut actual = Vec::new();
                for _ in 0..2 {
                    if let Some(cache) = &mut cache {
                        write_cached_graph_row(&mut actual, 0, &deps, join, cache, &nm).unwrap();
                    } else {
                        write_graph_row(&mut actual, nm(0), &dependency_names, join).unwrap();
                    }
                }
                assert_eq!(actual, expected.repeat(2));
                if let Some(cache) = cache {
                    assert!(cache.index.iter().any(|entry| entry.len != 0));
                    assert!(cache.index_bytes() + cache.arena.capacity() <= budget);
                    assert!(cache.used <= cache.arena.len());
                }
            }
        }
        let mut cache = GraphNameCache::new(1024, names.len()).unwrap();
        let mut actual = Vec::new();
        write_cached_graph_row(&mut actual, 3, &[], false, &mut cache, &nm).unwrap();
        assert_eq!(actual, assert_legacy_bytes("", &[], false));
    }

    #[test]
    fn exhausted_and_oversized_name_cache_falls_back_without_growing() {
        let names: Vec<_> = (0..64).map(|i| format!("name-{i:04}")).collect();
        let nm = |id: u32| names[id as usize].as_str();
        let deps: Vec<u32> = (0..64).collect();
        let dependency_names: Vec<_> = deps.iter().map(|&id| nm(id)).collect();
        let expected = assert_legacy_bytes(nm(0), &dependency_names, true);
        let mut cache = GraphNameCache::new(1024, names.len()).unwrap();
        let capacities = (cache.index.capacity(), cache.arena.capacity());
        let mut actual = Vec::new();
        for _ in 0..2 {
            write_cached_graph_row(&mut actual, 0, &deps, true, &mut cache, &nm).unwrap();
        }
        assert_eq!(actual, expected.repeat(2));
        assert!(cache.index.iter().any(|entry| entry.len == 0));
        assert!(cache.index.iter().any(|entry| entry.len != 0));
        assert_eq!((cache.index.capacity(), cache.arena.capacity()), capacities);
        assert!(cache.index_bytes() + cache.arena.capacity() <= 1024);

        let long = "oversized\"\\\n".repeat(4096);
        let names = ["small", long.as_str()];
        let nm = |id: u32| names[id as usize];
        let mut cache = GraphNameCache::new(1024, names.len()).unwrap();
        let mut actual = Vec::new();
        write_cached_graph_row(&mut actual, 1, &[0, 1, 0], false, &mut cache, &nm).unwrap();
        assert_eq!(
            actual,
            assert_legacy_bytes(&long, &["small", &long, "small"], false)
        );
        assert_eq!(cache.index[1].len, 0);
        assert_ne!(cache.index[0].len, 0);
        assert!(cache.index_bytes() + cache.arena.capacity() <= 1024);
    }

    #[test]
    fn name_cache_checks_ids_and_cached_offsets() {
        let mut cache = GraphNameCache::new(1024, 1).unwrap();
        let mut actual = Vec::new();
        cache
            .write_name(&mut actual, u32::MAX, || "outside\"")
            .unwrap();
        assert_eq!(actual, serde_json::to_vec("outside\"").unwrap());
        assert_eq!(cache.used, 0);
        cache.index[0] = EncodedGraphName {
            offset: u32::MAX,
            len: u32::MAX,
        };
        assert!(cache.write_name(&mut Vec::new(), 0, || "unused").is_err());
    }

    #[test]
    fn cached_graph_rows_propagate_short_write_failures() {
        struct FailAfter {
            remaining: usize,
            bytes: Vec<u8>,
        }
        impl std::io::Write for FailAfter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Err(std::io::Error::other("test write limit"));
                }
                let n = self.remaining.min(bytes.len());
                self.bytes.extend_from_slice(&bytes[..n]);
                self.remaining -= n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let long = "uncached".repeat(1024);
        for second in ["cached\"\\\n", long.as_str()] {
            let names = ["target", second];
            let nm = |id: u32| names[id as usize];
            let expected = assert_legacy_bytes(nm(0), &[second, second], true);
            let mut cache = GraphNameCache::new(1024, names.len()).unwrap();
            write_cached_graph_row(&mut Vec::new(), 0, &[1, 1], true, &mut cache, &nm).unwrap();
            for limit in [0, 1, 9, expected.len() / 2, expected.len() - 1] {
                let mut writer = FailAfter {
                    remaining: limit,
                    bytes: Vec::new(),
                };
                assert!(
                    write_cached_graph_row(&mut writer, 0, &[1, 1], true, &mut cache, &nm).is_err()
                );
                assert_eq!(writer.bytes, expected[..limit]);
            }
        }
    }
}
