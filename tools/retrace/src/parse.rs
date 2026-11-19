/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Trace ingest: the writer's Chrome-trace-style action log (a
//! line-framed JSON array — one record per line, tolerant of trailing
//! commas and of a missing close bracket) into [`Trace`].
//!
//! Larger files are decoded within a hardware-aware thread budget: split
//! into line-aligned chunks, one scoped thread per chunk, then an
//! order-preserving merge. Single-chunk inputs parse inline. Per-chunk
//! results carry chunk-local string-table ids and byte offsets that the
//! merge rebases, so the output is bit-identical to a serial parse
//! (there is a regression test pinning exactly that).
//!
//! Input is capped before any byte is read: `stat()` first, refuse over
//! the (configurable) cap. A refusal after minutes of streaming taught
//! that lesson; the cap exists to catch a mistaken input, not to define
//! the supported scale.

use std::borrow::Cow;
use std::io::Read as _;
use std::time::Instant;

use anyhow::Context as _;

use crate::types::CaptureState;
use crate::types::Outcome;
use crate::types::*;

/// Default input cap. Above the validated multi-GB captures while still
/// catching an obviously wrong input; `--max-input-bytes` moves or removes it.
pub const DEFAULT_MAX_INPUT_BYTES: u64 = 32 << 30;
/// Guard malformed measurements once, before arithmetic and display indexing.
/// One year for a single action is well beyond a usable build capture.
pub const MAX_DURATION_US: i64 = 366 * 24 * 60 * 60 * 1_000_000;
const MAX_TIMESTAMP_US: i64 = 10_000 * 366 * 24 * 60 * 60 * 1_000_000;

#[derive(Clone)]
pub struct LoadOpts {
    /// refuse inputs larger than this many bytes; 0 disables the cap
    pub max_input_bytes: u64,
    /// Decode-thread upper bound, also capped by available parallelism;
    /// 0 selects the hardware-aware default (up to 16).
    pub threads: usize,
    /// smallest input slice worth its own decode thread; 0 keeps the
    /// 4 MiB default. Tests lower it so a KB-scale fixture still
    /// exercises the multi-chunk decode-and-merge path.
    pub min_chunk_bytes: usize,
}

impl Default for LoadOpts {
    fn default() -> Self {
        LoadOpts {
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            threads: 0,
            min_chunk_bytes: 0,
        }
    }
}

/// Producers attach `args.cache` only to an action a cache served, so its
/// presence is the hit signal and `F_CACHE_HIT` is set from that. The `hit`
/// field they write alongside is a constant `true`, never `false`, and is
/// deliberately not read; a miss carries no `cache` object at all.
#[derive(serde::Deserialize)]
struct CacheArgs {
    bytes: Option<f64>,
}

#[derive(serde::Deserialize)]
struct XArgs<'a> {
    #[serde(borrow)]
    kind: Option<Cow<'a, str>>,
    exit: Option<i64>,
    sig: Option<i64>,
    // `failed` is a boolean on actions but a count on run-end records.
    failed: Option<serde_json::Value>,
    cache: Option<CacheArgs>,
    #[serde(borrow)]
    dir: Option<Cow<'a, str>>,
    #[serde(borrow)]
    why: Option<Cow<'a, str>>,
    #[serde(borrow)]
    run: Option<Cow<'a, str>>,
}

#[derive(serde::Deserialize)]
struct Row<'a> {
    ph: Option<&'a str>,
    cat: Option<&'a str>,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    ts: Option<i64>,
    dur: Option<i64>,
    #[serde(borrow)]
    args: Option<XArgs<'a>>,
}

/// Small append-only string interner for the per-act side tables. The
/// map is the only store while interning (one allocation per distinct
/// string); the dense table is materialized once at the end.
#[derive(Default)]
struct Intern {
    map: fxhash::FxHashMap<String, u32>,
}

impl Intern {
    fn get_or(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.map.len() as u32;
        self.map.insert(s.to_owned(), i);
        i
    }
    fn opt(&mut self, s: Option<&str>) -> u32 {
        match s {
            Some(v) => self.get_or(v),
            None => NO_STR,
        }
    }
    fn into_vals(self) -> Vec<String> {
        let mut vals = vec![String::new(); self.map.len()];
        for (s, i) in self.map {
            vals[i as usize] = s;
        }
        vals
    }
}

pub fn classify(name: &str, kind: Option<&str>) -> u8 {
    if matches!(
        kind,
        Some("action_cache") | Some("local_action_cache") | Some("remote_served_from_cache")
    ) {
        return 6;
    }
    let base = name.rsplit('/').next().unwrap_or(name);
    let last = base.rsplit(' ').next().unwrap_or(base);
    if last.ends_with(".o") || last.ends_with(".obj") || name.contains("compile") {
        0
    } else if last.ends_with(".a")
        || last.ends_with(".so")
        || last.ends_with(".dylib")
        || name.contains("library")
    {
        1
    } else if name.contains("link") || name.contains("binary") {
        2
    } else if name.contains("genrule")
        || name.contains("write")
        || name.contains("generated")
        || name.contains("symlink")
        || name.contains("copy")
    {
        3
    } else if last.ends_with(".sh") || last.ends_with(".py") || name.contains("script") {
        4
    } else {
        5
    }
}

/// Buck2 subcommands that name targets. Walking back from the end of
/// `argv` stops here, so the subcommand itself is never mistaken for a
/// target.
const BUCK_SUBCOMMANDS: &[&str] = &[
    "build", "test", "run", "install", "targets", "ctargets", "query", "uquery", "cquery",
    "aquery", "audit", "docs", "bxl", "explain", "profile", "complete",
];

/// Best-effort recovery of the target patterns from a recorded `argv`,
/// for captures written before the ingester started recording them
/// outright. Takes the trailing run of tokens that look like patterns:
/// a flag, an `@argfile`, a `k=v` flag value or a bare number ends the
/// run, as does the subcommand.
///
/// This is a label, never an input to a number. A capture whose argv
/// defeats the heuristic loses its short name and shows its path, which
/// is what every capture did before.
fn targets_from_argv(argv: &[&str]) -> Vec<String> {
    let plausible = |t: &str| {
        !t.is_empty()
            && !t.starts_with('-')
            && !t.starts_with('@')
            && !t.contains('=')
            && t.parse::<f64>().is_err()
            && !BUCK_SUBCOMMANDS.contains(&t)
    };
    let mut out: Vec<String> = Vec::new();
    // argv[0] is the binary; a one-element argv names nothing
    for t in argv.iter().skip(1).rev() {
        if !plausible(t) {
            break;
        }
        out.push((*t).to_owned());
    }
    out.reverse();
    out
}

fn merge_run_info(r: &mut RunInfo, v: &serde_json::Value, ts: i64, end: bool) {
    let a = v.get("args").unwrap_or(&serde_json::Value::Null);
    let g = |k: &str| a.get(k).and_then(|x| x.as_i64());
    let s = |k: &str| a.get(k).and_then(|x| x.as_str()).map(|x| x.to_owned());
    if r.run.is_empty() {
        r.run = s("run").unwrap_or_default();
    }
    if !end {
        if r.started {
            r.duplicate_records += 1;
            return;
        }
        r.started = true;
        r.ts = ts;
        r.re_session = s("re_session");
        r.jobs = g("jobs");
        r.ncpu = g("ncpu");
        r.host = s("host");
        r.hostname = s("hostname");
        r.user = s("user");
        r.os = s("os");
        r.arch = s("arch");
        r.mem = g("mem");
        r.load = s("load");
        r.version = s("version");
        r.capture = s("capture");
        r.execution_store = a
            .get("execution_store")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        r.tool = s("tool");
        r.cwd = s("cwd");
        r.git = s("git");
        r.argv = a.get("argv").map(|x| match x {
            serde_json::Value::Array(items) => items
                .iter()
                .map(|i| i.as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_string(),
        });
        r.targets = a
            .get("targets")
            .and_then(|x| x.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str().map(|s| s.to_owned()))
                    .collect::<Vec<_>>()
            })
            .filter(|t| !t.is_empty())
            .or_else(|| {
                let argv: Vec<&str> = match a.get("argv") {
                    Some(serde_json::Value::Array(items)) => {
                        items.iter().filter_map(|i| i.as_str()).collect()
                    }
                    _ => Vec::new(),
                };
                let t = targets_from_argv(&argv);
                (!t.is_empty()).then_some(t)
            });
        r.file_changes = a
            .get("file_changes")
            .and_then(|x| x.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str().map(|s| s.to_owned()))
                    .collect()
            });
        r.file_changes_total = g("file_changes_total");
    } else {
        // First footer wins, mirroring the header branch: an appended
        // or replayed log must not let a later duplicate overwrite the
        // run's recorded outcome.
        if r.ended {
            r.duplicate_records += 1;
            return;
        }
        r.ended = true;
        r.end_ts = Some(ts);
        r.result = g("result").or_else(|| g("exit"));
        r.outcome = Outcome::from_run_end_args(a);
        r.dep_capture = a.get("dep_capture").and_then(CaptureState::from_json);
        r.remade = g("remade");
        r.uptodate = g("uptodate");
        r.failed = g("failed");
        if let Some(c) = a.get("cache") {
            r.cache_hits = c.get("hits").and_then(|x| x.as_i64());
            r.cache_misses = c.get("misses").and_then(|x| x.as_i64());
            r.cache_stores = c.get("stores").and_then(|x| x.as_i64());
        }
    }
}

/// One `run_start`/`run_end` record, in file order within its chunk.
struct RunEvt {
    end: bool,
    ts: i64,
    val: serde_json::Value,
}

/// Delay owned profile decoding until the ordered merge. A malformed or
/// duplicate metadata row must not discard action rows or choose a run.
struct ProfileRow {
    off: usize,
    len: usize,
}

#[derive(Default)]
struct ChunkOut {
    /// `name`/`dir`/`why`/`run` hold CHUNK-LOCAL table ids; `off` is
    /// already absolute (the chunk knows its base offset).
    acts: Vec<Act>,
    names: Vec<String>,
    dirs: Vec<String>,
    whys: Vec<String>,
    run_ids: Vec<String>,
    run_evts: Vec<RunEvt>,
    profile_rows: Vec<ProfileRow>,
    profile_limit_exceeded: bool,
    dropped: usize,
    rejected_fields: usize,
    min_ts: i64,
    max_end: i64,
}

fn parse_chunk(buf: &[u8], base_off: u64) -> ChunkOut {
    let mut out = ChunkOut {
        min_ts: i64::MAX,
        ..Default::default()
    };
    let mut dirs = Intern::default();
    let mut whys = Intern::default();
    let mut run_ids = Intern::default();
    let mut pos = 0usize;
    while pos < buf.len() {
        let nl = buf[pos..].iter().position(|&b| b == b'\n');
        let end = match nl {
            Some(i) => pos + i + 1,
            None => buf.len(),
        };
        let raw = &buf[pos..end];
        let n = raw.len();
        if let Ok(line) = std::str::from_utf8(raw) {
            let s = line
                .trim()
                .trim_start_matches('\u{feff}')
                .trim_end_matches(',');
            if !s.starts_with('{') {
                // Array framing is expected; anything else on a line
                // of its own is an undecodable record and must be
                // counted like one, or the dropped tally lies.
                if !matches!(s, "" | "[" | "]" | "[]") {
                    out.dropped += 1;
                }
            } else if s.len() > 2 {
                let repaired;
                let parsed = match serde_json::from_str::<Row>(s) {
                    Ok(row) => Ok(row),
                    Err(error) => {
                        repaired = repair_optional_args(s);
                        if let Some(repaired) = &repaired {
                            out.rejected_fields += repaired.rejected_fields();
                            serde_json::from_str::<Row>(&repaired.text)
                        } else {
                            Err(error)
                        }
                    }
                };
                match parsed {
                    Ok(r) if r.ph == Some("X") => {
                        if let (Some(ts), Some(name), Some(dur)) = (r.ts, r.name.as_deref(), r.dur)
                        {
                            if !(0..=MAX_DURATION_US).contains(&dur)
                                || !(0..=MAX_TIMESTAMP_US - MAX_DURATION_US).contains(&ts)
                                || n > u32::MAX as usize
                                || out.acts.len() >= u32::MAX as usize
                            {
                                out.dropped += 1;
                                pos = end;
                                continue;
                            }
                            if r.args
                                .as_ref()
                                .and_then(|a| a.failed.as_ref())
                                .is_some_and(|value| !value.is_boolean())
                            {
                                out.rejected_fields += 1;
                            }
                            let (kind, failed, hit) = match &r.args {
                                Some(a) => (
                                    a.kind.as_deref(),
                                    a.failed.as_ref().and_then(|value| value.as_bool())
                                        == Some(true)
                                        || a.exit.unwrap_or(0) != 0
                                        || a.sig.unwrap_or(0) != 0,
                                    a.cache.is_some(),
                                ),
                                None => (None, false, false),
                            };
                            let not_recipe = matches!(r.cat, Some("cache") | Some("phase"));
                            let (dir, why, run, cbytes) = match &r.args {
                                Some(a) => (
                                    dirs.opt(a.dir.as_deref()),
                                    whys.opt(a.why.as_deref()),
                                    run_ids.opt(a.run.as_deref()),
                                    a.cache.as_ref().and_then(|c| c.bytes),
                                ),
                                None => (NO_STR, NO_STR, NO_STR, None),
                            };
                            let color = classify(name, kind);
                            // cache/phase bookkeeping rows must not
                            // inflate span
                            if !not_recipe {
                                out.min_ts = out.min_ts.min(ts);
                                out.max_end = out.max_end.max(ts + dur);
                            }
                            out.names.push(name.to_owned());
                            out.acts.push(Act {
                                ts,
                                dur,
                                lane: 0,
                                color,
                                flags: (if failed { F_FAILED } else { 0 })
                                    | (if hit { F_CACHE_HIT } else { 0 })
                                    | (if not_recipe { F_NOT_RECIPE } else { 0 }),
                                name: (out.names.len() - 1) as u32,
                                off: base_off + pos as u64,
                                len: n as u32,
                                seq: 0, // assigned at merge: global file order
                                dir,
                                why,
                                run,
                                cache_bytes: cbytes,
                            });
                        } else {
                            out.dropped += 1;
                        }
                    }
                    Ok(r) if r.ph == Some("M") && r.cat == Some("run") => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                            let is_end = r.name.as_deref() == Some("run_end");
                            let is_start = r.name.as_deref() == Some("run_start");
                            if is_end || is_start {
                                out.run_evts.push(RunEvt {
                                    end: is_end,
                                    ts: r.ts.unwrap_or(0),
                                    val: v,
                                });
                            }
                        }
                    }
                    Ok(r) if r.ph == Some("M") && r.cat == Some("command-profile") => {
                        if out.profile_rows.len() < crate::command_profile::MAX_PROFILE_RECORDS {
                            out.profile_rows.push(ProfileRow {
                                off: base_off as usize
                                    + pos
                                    + (s.as_ptr() as usize - line.as_ptr() as usize),
                                len: s.len(),
                            });
                        } else {
                            out.profile_limit_exceeded = true;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => out.dropped += 1,
                }
            } else if s != "{}" {
                out.dropped += 1;
            }
        } else {
            out.dropped += 1;
        }
        pos = end;
    }
    out.dirs = dirs.into_vals();
    out.whys = whys.into_vals();
    out.run_ids = run_ids.into_vals();
    out
}

/// Repaired text and disjoint rejection counts. RE markers remain in the text,
/// so a decoder which counts those markers adds only `unmarked_rejections`.
pub(crate) struct RepairedArgs {
    pub text: String,
    pub unmarked_rejections: usize,
    marked_rejections: usize,
}

impl RepairedArgs {
    fn rejected_fields(&self) -> usize {
        self.unmarked_rejections + self.marked_rejections
    }
}

/// Recover malformed optional fields only on the slow error path. Ordinary
/// records still use the borrowing typed decoder without a Value allocation.
pub(crate) fn repair_optional_args(text: &str) -> Option<RepairedArgs> {
    use serde_json::Value;
    fn fields(value: &mut Value, strings: &[&str], numbers: &[&str], bools: &[&str]) -> usize {
        let Some(object) = value.as_object_mut() else {
            return 0;
        };
        let mut removed = 0;
        object.retain(|key, value| {
            let valid = if bools.contains(&key.as_str()) {
                value.is_boolean()
            } else if value.is_null() {
                true
            } else if strings.contains(&key.as_str()) {
                value.is_string()
            } else if numbers.contains(&key.as_str()) {
                value.as_i64().is_some()
            } else {
                true
            };
            removed += usize::from(!valid);
            valid
        });
        removed
    }
    let mut row: Value = serde_json::from_str(text).ok()?;
    let action_row = row.get("ph").and_then(Value::as_str) == Some("X");
    let mut numeric_normalized = false;
    for key in ["ts", "dur"] {
        if let Some(value) = row.get_mut(key)
            && value.as_i64().is_none()
            && let Some(number) = value.as_f64()
            && number.is_finite()
            && number.fract() == 0.0
            && (0.0..=9_007_199_254_740_991.0).contains(&number)
        {
            *value = Value::from(number as i64);
            numeric_normalized = true;
        }
    }
    let Some(args) = row.get_mut("args") else {
        return numeric_normalized.then(|| RepairedArgs {
            text: row.to_string(),
            unmarked_rejections: 0,
            marked_rejections: 0,
        });
    };
    let mut removed = 0;
    let mut marked_rejections = 0;
    if !args.is_object() && !args.is_null() {
        *args = Value::Null;
        removed += 1;
    } else {
        removed += fields(
            args,
            &[
                "kind",
                "src",
                "dir",
                "why",
                "run",
                "target",
                "execution_metadata",
            ],
            &[
                "exit",
                "sig",
                "maxrss",
                "bytes_in",
                "bytes_out",
                "exec_us",
                "wall_time_us",
                "tries",
                "cpu_us",
            ],
            if action_row {
                &["failed", "elastic"]
            } else {
                &["elastic"]
            },
        );
        for key in ["cache", "stages", "re", "winner"] {
            let Some(value) = args.get_mut(key) else {
                continue;
            };
            if !value.is_object() && !value.is_null() {
                *value = Value::Null;
                removed += 1;
                continue;
            }
            if let Some(object) = value.as_object_mut() {
                let mut invalid_re = Vec::new();
                object.retain(|field, value| {
                    let valid = if key == "re" && field == "invalid_counter_fields" {
                        value
                            .as_array()
                            .is_some_and(|items| items.iter().all(Value::is_string))
                    } else {
                        value.is_null()
                            || match field.as_str() {
                                "execute_source" | "hashing_source" => value.is_string(),
                                "bytes" => value.as_i64().is_some(),
                                "hit" => value.is_boolean(),
                                "client_start" => crate::types::micros_value(value).is_some(),
                                _ if field.ends_with("_us") || key == "re" => {
                                    value.as_i64().is_some()
                                }
                                _ => true,
                            }
                    };
                    removed += usize::from(!valid);
                    if !valid && key == "re" && field != "invalid_counter_fields" {
                        invalid_re.push(Value::String(field.clone()));
                    }
                    valid
                });
                if !invalid_re.is_empty() {
                    marked_rejections += invalid_re.len();
                    object
                        .entry("invalid_counter_fields")
                        .or_insert_with(|| Value::Array(Vec::new()))
                        .as_array_mut()
                        .expect("validated diagnostic array")
                        .extend(invalid_re);
                }
            }
        }
        for key in ["attempts", "attempt_status"] {
            let Some(value) = args.get_mut(key) else {
                continue;
            };
            if !value.is_array() {
                *value = Value::Array(Vec::new());
                removed += 1;
            } else if let Some(items) = value.as_array_mut() {
                items.retain_mut(|item| {
                    let valid = if key == "attempt_status" {
                        item.is_string()
                    } else {
                        item.is_object()
                    };
                    if valid && key == "attempts" {
                        removed += fields(item, &["status"], &["dur_us"], &["elastic"]);
                        if let Some(object) = item.as_object_mut() {
                            for key in ["start", "client_start"] {
                                if object.get(key).is_some_and(|v| {
                                    !v.is_null() && crate::types::micros_value(v).is_none()
                                }) {
                                    object.remove(key);
                                    removed += 1;
                                }
                            }
                        }
                    }
                    removed += usize::from(!valid);
                    valid
                });
            }
        }
    }
    (removed != 0 || numeric_normalized).then(|| RepairedArgs {
        text: row.to_string(),
        unmarked_rejections: removed - marked_rejections,
        marked_rejections,
    })
}

#[derive(Default)]
struct Profiles {
    // Keep invalid rows with a known run, too: a valid row followed by a
    // malformed duplicate must not silently keep the first profile.
    rows: Vec<(String, Option<crate::command_profile::CommandProfile>)>,
    warnings: Vec<String>,
    /// The whole set was refused: no rows, and later batches add none.
    unavailable: bool,
}

impl Profiles {
    fn refused(warning: String) -> Self {
        Self {
            warnings: vec![warning],
            unavailable: true,
            ..Default::default()
        }
    }
}

fn profile_diagnostic(error: &anyhow::Error) -> String {
    // Deserializer errors can echo a hostile string value. Bound the retained
    // diagnostic while formatting, not by first allocating another full copy.
    struct Diagnostic {
        text: String,
        truncated: bool,
    }
    impl std::fmt::Write for Diagnostic {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            let remaining = 1024usize.saturating_sub(self.text.len());
            if text.len() <= remaining {
                self.text.push_str(text);
                return Ok(());
            }
            let mut end = remaining;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.push_str(&text[..end]);
            self.truncated = true;
            Err(std::fmt::Error)
        }
    }
    use std::fmt::Write as _;
    let mut diagnostic = Diagnostic {
        text: String::new(),
        truncated: false,
    };
    let _ = write!(&mut diagnostic, "{error:#}");
    if diagnostic.truncated {
        diagnostic.text.push_str("... [diagnostic truncated]");
    }
    diagnostic.text
}

fn read_profiles(data: &[u8], base: usize, chunks: &[ChunkOut]) -> Profiles {
    use crate::command_profile::MAX_PROFILE_BYTES;
    use crate::command_profile::MAX_PROFILE_RECORDS;

    // Bound metadata across an appended trace, not independently per parser
    // thread. Refuse the profiles together if omitted rows could be duplicate
    // identities; ordinary actions and run headers remain usable.
    const MAX_TOTAL_BYTES: usize = 32 << 20;
    let count: usize = chunks.iter().map(|c| c.profile_rows.len()).sum();
    let bytes = chunks
        .iter()
        .flat_map(|c| &c.profile_rows)
        .try_fold(0usize, |sum, row| sum.checked_add(row.len));
    if count > MAX_PROFILE_RECORDS
        || chunks.iter().any(|c| c.profile_limit_exceeded)
        || bytes.is_none_or(|n| n > MAX_TOTAL_BYTES)
        || chunks
            .iter()
            .flat_map(|c| &c.profile_rows)
            .any(|row| row.len > MAX_PROFILE_BYTES + (64 << 10))
    {
        return Profiles::refused("command-profile metadata exceeds reader limits; profiles are unavailable, action rows are retained".to_owned());
    }
    let mut profiles = Profiles::default();
    for row in chunks.iter().flat_map(|c| &c.profile_rows) {
        let value = row
            .off
            .checked_sub(base)
            .and_then(|start| {
                start
                    .checked_add(row.len)
                    .and_then(|end| data.get(start..end))
            })
            .context("profile row is outside the parsed input")
            .and_then(|raw| {
                serde_json::from_slice::<serde_json::Value>(raw).context("decode profile metadata")
            });
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                return Profiles::refused(format!(
                    "command-profile metadata cannot be decoded unambiguously; profiles are unavailable: {}",
                    profile_diagnostic(&error)
                ));
            }
        };
        let args = &value["args"];
        let run = args
            .get("run")
            .and_then(serde_json::Value::as_str)
            .filter(|run| !run.is_empty() && run.len() <= 16_384);
        let decoded = crate::command_profile::decode_args(args)
            .and_then(|p| p.context("command-profile metadata has no profile payload"));
        match decoded {
            Ok(profile) => profiles.rows.push((
                profile.run.clone().expect("decode_args binds run"),
                Some(profile),
            )),
            Err(error) => {
                if let Some(run) = run {
                    profiles.rows.push((run.to_owned(), None));
                }
                profiles.warnings.push(format!(
                    "command-profile metadata ignored: {}",
                    profile_diagnostic(&error)
                ));
            }
        }
    }
    profiles
}

fn attach_profiles(profiles: Profiles, runs: &mut [RunInfo]) -> Vec<String> {
    let mut warnings = profiles.warnings;
    let mut seen = fxhash::FxHashSet::default();
    for (run, profile) in profiles.rows {
        let duplicate = !seen.insert(run.clone());
        if let Some(slot) = runs.iter_mut().find(|r| r.run == run) {
            if duplicate {
                slot.command_profile = None;
                warnings.push(format!(
                    "multiple command profiles for run {run}; profile is unavailable"
                ));
            } else {
                slot.command_profile = profile;
            }
        } else {
            warnings.push(format!(
                "command profile for run {run} has no matching run metadata; ignored"
            ));
        }
    }
    warnings
}

/// Line-aligned chunk boundaries: every chunk starts at 0 or just past a
/// newline, so no line is ever split across threads.
fn chunk_bounds(data: &[u8], want: usize) -> Vec<(usize, usize)> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut cuts = vec![0usize];
    for k in 1..want {
        let target = data.len() * k / want;
        let prev = *cuts.last().unwrap();
        if target <= prev {
            continue;
        }
        match data[target..].iter().position(|&b| b == b'\n') {
            Some(i) if target + i + 1 < data.len() => cuts.push(target + i + 1),
            _ => break,
        }
    }
    cuts.push(data.len());
    cuts.windows(2).map(|w| (w[0], w[1])).collect()
}

pub fn parse(path: &str) -> anyhow::Result<Trace> {
    parse_opts(path, &LoadOpts::default())
}

pub fn parse_opts(path: &str, opts: &LoadOpts) -> anyhow::Result<Trace> {
    parse_opts_with_threads(path, opts, crate::parallelism::trace_threads(opts.threads))
}

/// The shared viewer loader supplies a budget that already accounts for its
/// overlapping graph work. Do not independently enlarge that allocation here.
pub(crate) fn parse_opts_with_threads(
    path: &str,
    opts: &LoadOpts,
    threads: usize,
) -> anyhow::Result<Trace> {
    parse_opts_with_threads_cancellable(
        path,
        opts,
        threads,
        &std::sync::atomic::AtomicBool::new(false),
    )
}

pub(crate) fn parse_opts_with_threads_cancellable(
    path: &str,
    opts: &LoadOpts,
    threads: usize,
    cancelled: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<Trace> {
    let check = || {
        anyhow::ensure!(
            !cancelled.load(std::sync::atomic::Ordering::Relaxed),
            "trace parsing cancelled"
        );
        Ok(())
    };
    check()?;
    let file = std::fs::File::open(path).with_context(|| format!("open {path}"))?;
    let meta = file.metadata().with_context(|| format!("stat {path}"))?;
    if opts.max_input_bytes > 0 && meta.len() > opts.max_input_bytes {
        anyhow::bail!(
            "{path} is {} bytes, over the {} byte input cap; pass \
             --max-input-bytes=N (0 = uncapped) if this is really the file you mean",
            meta.len(),
            opts.max_input_bytes
        );
    }
    let t0 = Instant::now();
    let source = TraceSource::new(file).with_context(|| format!("identify {path}"))?;
    use std::io::BufRead;
    // Bound the input buffer independently of the capture's size. Parsed
    // actions survive each batch; borrowed source bytes do not. A single
    // oversized record may extend a batch, and is still checked by its reader.
    let batch_bytes = threads.clamp(1, 16) * (32 << 20);
    let mut input = std::io::BufReader::with_capacity(1 << 20, source.reader());
    let mut chunks = Vec::new();
    let mut profiles = Profiles::default();
    let mut offset = 0usize;
    let mut profile_bytes = 0usize;
    let mut profile_count = 0usize;
    loop {
        check()?;
        // Grown as it is read: reserving the whole batch up front zero-fills up to 512 MiB
        // on a many-core machine, even for a two-line trace.
        let mut data = Vec::new();
        input
            .by_ref()
            .take(batch_bytes as u64)
            .read_to_end(&mut data)
            .with_context(|| format!("read {path}"))?;
        if data.is_empty() {
            break;
        }
        if data.last() != Some(&b'\n') {
            input
                .read_until(b'\n', &mut data)
                .with_context(|| format!("read {path}"))?;
        }
        if offset == 0 {
            // The trace dialect is a line-framed JSON array; a graph sidecar is a
            // single giant object. A mixed-up argument used to parse to an empty
            // trace and panic downstream on the degenerate span, so refuse it
            // here from the first non-whitespace byte.
            let without_bom = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&data);
            if without_bom
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|&b| b == b'{')
            {
                let head = String::from_utf8_lossy(&data[..data.len().min(512)]);
                let kind = if head.contains("\"graph\"") {
                    "a dependency-graph sidecar (trace-buck dialect, first key `graph`)"
                } else if head.contains("\"header\"") {
                    "a dependency-graph dump (raw buck2 dialect, first key `header`)"
                } else {
                    "a single JSON object, not the line-framed array traces use"
                };
                anyhow::bail!(
                    "{path} looks like {kind}; pass the trace file as the \
             positional argument and attach the graph with --graph={path}"
                );
            }
            let first_line = without_bom
                .split(|b| *b == b'\n')
                .next()
                .unwrap_or_default();
            if std::str::from_utf8(first_line)
                .is_ok_and(|s| s.trim().starts_with('[') && !matches!(s.trim(), "[" | "[]"))
            {
                anyhow::bail!(
                    "{path}: expected a line-framed trace array (one event per line), not a single-line JSON array; export with retrace ingest"
                );
            }
        }

        // no point spinning threads for a few MB
        let min_chunk = if opts.min_chunk_bytes > 0 {
            opts.min_chunk_bytes
        } else {
            4 << 20
        };
        let want = threads.min(1 + data.len() / min_chunk).max(1);
        let bounds = chunk_bounds(&data, want);
        let data_ref: &[u8] = &data;
        let batch: Vec<ChunkOut> = if bounds.len() <= 1 {
            // One core or a small input: parse on the caller rather than adding
            // a worker beside a coordinator that only waits for it.
            bounds
                .into_iter()
                .map(|(a, b)| parse_chunk(&data_ref[a..b], (offset + a) as u64))
                .collect()
        } else {
            std::thread::scope(|s| {
                let handles: Vec<_> = bounds
                    .iter()
                    .map(|&(a, b)| {
                        s.spawn(move || parse_chunk(&data_ref[a..b], (offset + a) as u64))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("decode thread panicked"))
                    .collect()
            })
        };
        profile_count += batch.iter().map(|c| c.profile_rows.len()).sum::<usize>();
        profile_bytes = profile_bytes.saturating_add(
            batch
                .iter()
                .flat_map(|c| &c.profile_rows)
                .map(|r| r.len)
                .sum::<usize>(),
        );
        if !profiles.unavailable {
            let mut batch_profiles = if profile_count <= crate::command_profile::MAX_PROFILE_RECORDS
                && profile_bytes <= 32 << 20
            {
                read_profiles(&data, offset, &batch)
            } else {
                Profiles::refused("command-profile metadata exceeds reader limits; profiles are unavailable, action rows are retained".to_owned())
            };
            if batch_profiles.unavailable {
                profiles = batch_profiles;
            } else {
                profiles.rows.append(&mut batch_profiles.rows);
                profiles.warnings.append(&mut batch_profiles.warnings);
            }
        }
        offset += data.len();
        chunks.extend(batch);
        check()?;
    }
    let parse_ms = t0.elapsed().as_millis();
    // Every chunk owns its names/metadata, and all borrowing workers have
    // joined. Lazy detail reads use TraceSource, not this temporary Vec.
    // Full parse wall time includes this release; the internal decode and
    // index stopwatches intentionally do not include its intervening cost.

    // ---- order-preserving merge ----
    let t1 = Instant::now();
    let action_count: usize = chunks.iter().map(|c| c.acts.len()).sum();
    anyhow::ensure!(
        action_count < u32::MAX as usize,
        "too many action rows for compact indices"
    );
    let mut acts: Vec<Act> = Vec::with_capacity(action_count);
    let mut names: Vec<String> = Vec::with_capacity(acts.capacity());
    let mut dirs = Intern::default();
    let mut whys = Intern::default();
    let mut run_ids = Intern::default();
    let mut runs: Vec<RunInfo> = Vec::new();
    let mut dropped = 0usize;
    let mut rejected_fields = 0usize;
    let mut min_ts = i64::MAX;
    let mut max_end = 0i64;
    for c in chunks {
        check()?;
        let remap = |vals: &[String], it: &mut Intern| -> Vec<u32> {
            vals.iter().map(|v| it.get_or(v)).collect()
        };
        let dmap = remap(&c.dirs, &mut dirs);
        let wmap = remap(&c.whys, &mut whys);
        let rmap = remap(&c.run_ids, &mut run_ids);
        let base = acts.len() as u32;
        for mut a in c.acts {
            a.seq = base + (a.name); // chunk-local name idx == chunk-local act idx
            a.name = a.seq;
            if a.dir != NO_STR {
                a.dir = dmap[a.dir as usize];
            }
            if a.why != NO_STR {
                a.why = wmap[a.why as usize];
            }
            if a.run != NO_STR {
                a.run = rmap[a.run as usize];
            }
            acts.push(a);
        }
        names.extend(c.names);
        for e in c.run_evts {
            let run_id = e
                .val
                .get("args")
                .and_then(|a| a.get("run"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            // start and end merge symmetrically by run id, so a
            // duplicated or out-of-order header cannot mint a second
            // RunInfo for the same run
            let slot = match runs.iter_mut().position(|q| q.run == run_id) {
                Some(i) => &mut runs[i],
                None => {
                    runs.push(RunInfo::default());
                    runs.last_mut().expect("just pushed")
                }
            };
            merge_run_info(slot, &e.val, e.ts, e.end);
        }
        dropped += c.dropped;
        rejected_fields += c.rejected_fields;
        min_ts = min_ts.min(c.min_ts);
        max_end = max_end.max(c.max_end);
    }

    if min_ts == i64::MAX {
        min_ts = 0;
    }
    let mut busy = 0i64;
    let mut n_failed = 0;
    let mut n_hits = 0;
    let mut class_count = [0usize; CLASS_NAMES.len()];
    let mut class_time = [0i64; CLASS_NAMES.len()];
    for a in &mut acts {
        a.ts -= min_ts;
        busy = busy
            .checked_add(a.dur)
            .filter(|n| *n <= i64::MAX / 1000)
            .context("aggregate action durations exceed the supported arithmetic range")?;
        // failed/hit counts cover recipe rows only: a cache or phase
        // bookkeeping row also carries exit codes and cache objects,
        // and counting those double-counted every served action
        if a.flags & F_NOT_RECIPE == 0 {
            n_failed += usize::from(a.flags & F_FAILED != 0);
            n_hits += usize::from(a.flags & F_CACHE_HIT != 0);
        }
        class_count[a.color as usize] += 1;
        class_time[a.color as usize] += a.dur;
    }
    let layout = crate::lanes::assign(&mut acts);
    check()?;
    acts.sort_unstable_by_key(|a| (a.lane, a.ts));
    let n_lanes = layout.n_lanes as usize;
    let mut lanes: Vec<Lane> = Vec::with_capacity(n_lanes);
    let mut i = 0u32;
    for lane in 0..n_lanes as u32 {
        let start = i;
        let mut max_dur = 0i64;
        while (i as usize) < acts.len() && acts[i as usize].lane == lane {
            max_dur = max_dur.max(acts[i as usize].dur);
            i += 1;
        }
        lanes.push(Lane {
            start,
            end: i,
            max_dur,
        });
    }
    let index_ms = t1.elapsed().as_millis();

    let mut health = Health {
        rejected_fields,
        ..Health::default()
    };
    if rejected_fields != 0 {
        health.warnings.push(format!(
            "{rejected_fields} malformed optional field(s) ignored; action rows retained"
        ));
    }
    health.warnings.extend(attach_profiles(profiles, &mut runs));
    if dropped > 0 {
        health
            .warnings
            .push(format!("{dropped} undecodable line(s) skipped"));
    }
    let unended = runs.iter().filter(|r| !r.ended).count();
    if unended > 0 {
        // per run, not all-or-nothing: an appended multi-run trace with
        // one torn run must still warn
        health.warnings.push(if runs.len() == 1 {
            "no run_end footer: build was killed or capture is incomplete".to_owned()
        } else {
            format!(
                "{unended} of {} runs have no run_end footer: build was \
                 killed or capture is incomplete",
                runs.len()
            )
        });
    }
    health.dropped = dropped;
    health.unended = unended;
    // a run that ended without a result, and a capture that says it is
    // short, each warn once; a run with no run_end already did above
    for r in &runs {
        let who = |msg: String| {
            if runs.len() == 1 {
                msg
            } else {
                format!("run {}: {msg}", r.run)
            }
        };
        if r.duplicate_records > 0 {
            health.warnings.push(who(format!(
                "{} duplicate run header/footer record(s); the first was kept",
                r.duplicate_records
            )));
        }
        if r.started && r.end_ts.is_some_and(|end| end < r.ts) {
            health.warnings.push(who(
                "run_end precedes run_start; command wall duration is unavailable".to_owned(),
            ));
        }
        if r.ended && r.outcome == Outcome::Unknown {
            health.warnings.push(who(
                "run_end records no reliable command result; the outcome is unknown, not success"
                    .to_owned(),
            ));
        }
        if let Some(state) = &r.dep_capture {
            for p in state.problems() {
                health.warnings.push(who(p));
            }
        }
    }

    source
        .validate()
        .with_context(|| format!("{path} changed while parsing"))?;
    Ok(Trace {
        path: path.to_owned(),
        source,
        acts,
        names,
        lanes,
        span: max_end - min_ts,
        t0_abs: min_ts,
        busy,
        n_failed,
        n_hits,
        class_count,
        class_time,
        runs,
        health,
        parse_ms,
        index_ms,
        dirs: dirs.into_vals(),
        whys: whys.into_vals(),
        run_ids: run_ids.into_vals(),
        bands: layout.bands,
    })
}

/// Resolve a `--run` selector against the trace's runs: an exact run-id
/// match wins, else a 1-based ordinal in file order. No selector is fine
/// only when the trace holds a single run — guessing among appended runs
/// would silently analyze the wrong build (docs/contract.md).
pub fn select_run(runs: &[RunInfo], sel: Option<&str>) -> anyhow::Result<usize> {
    let listing = || {
        runs.iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "  {}: {}",
                    i + 1,
                    if r.run.is_empty() { "?" } else { &r.run }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    if runs.is_empty() {
        anyhow::bail!("trace carries no run_start/run_end records");
    }
    match sel {
        None if runs.len() == 1 => Ok(0),
        None => anyhow::bail!(
            "trace holds {} runs; pick one with --run=<id|ordinal>:\n{}",
            runs.len(),
            listing()
        ),
        Some(s) => {
            if let Some(i) = runs.iter().position(|r| r.run == s) {
                return Ok(i);
            }
            if let Ok(n) = s.parse::<usize>() {
                if n >= 1 && n <= runs.len() {
                    return Ok(n - 1);
                }
            }
            anyhow::bail!(
                "--run={s} matches no run id and is not an ordinal 1..={}:\n{}",
                runs.len(),
                listing()
            )
        }
    }
}

/// Resolve metadata-bearing and legacy action-only traces identically.
/// A completely unnamed old trace is valid; several named runs need an
/// explicit selection even when `run_start` records were never written.
pub fn select_trace_run(tr: &Trace, sel: Option<&str>) -> anyhow::Result<Option<String>> {
    // Appended legacy rows can name a run whose metadata was not retained.
    // Keep metadata order, then append distinct action-only IDs; metadata for
    // one run must not silently make another recorded run unselectable.
    let mut seen = std::collections::HashSet::new();
    let runs: Vec<RunInfo> = tr
        .runs
        .iter()
        .map(|r| r.run.as_str())
        .chain(tr.run_ids.iter().map(String::as_str))
        .filter(|r| !r.is_empty() && seen.insert(*r))
        .map(|r| RunInfo {
            run: r.to_owned(),
            ..Default::default()
        })
        .collect();
    if runs.is_empty() && sel.is_none() {
        return Ok(None);
    }
    select_run(&runs, sel).map(|i| Some(runs[i].run.clone()))
}

/// Culled paint pass; PAINT gets (act_idx, x0, x1, row).
pub fn cull<F: FnMut(usize, f32, f32, usize)>(
    tr: &Trace,
    t_left: f64,
    upp: f64,
    lane_top: usize,
    lane_rows: usize,
    width_px: f32,
    mut paint: F,
) -> (usize, usize) {
    if !upp.is_finite()
        || upp <= 0.0
        || !t_left.is_finite()
        || !width_px.is_finite()
        || width_px <= 0.0
    {
        return (0, 0);
    }
    let mut painted = 0;
    let mut merged = 0;
    let t_right = t_left + upp * width_px as f64;
    for row in 0..lane_rows {
        let li = lane_top + row;
        if li >= tr.lanes.len() {
            break;
        }
        let l = &tr.lanes[li];
        let slice = &tr.acts[l.start as usize..l.end as usize];
        // lanes::pack never overlaps actions within a lane, and parsing
        // rejects negative durations: end times are monotone in this slice.
        let lo = slice.partition_point(|a| ((a.ts + a.dur) as f64) < t_left);
        let mut last_x2 = f32::MIN;
        for (k, a) in slice[lo..].iter().enumerate() {
            let ts = a.ts as f64;
            if ts > t_right {
                break;
            }
            if (ts + a.dur as f64) < t_left {
                continue;
            }
            let x0 = ((ts - t_left) / upp).clamp(-4.0, width_px as f64 + 4.0) as f32;
            let x1 =
                (((ts + a.dur as f64) - t_left) / upp).clamp(-4.0, width_px as f64 + 4.0) as f32;
            if x1 - x0 < 0.8 && x0 < last_x2 + 0.4 {
                merged += 1;
                continue;
            }
            last_x2 = x1.max(x0 + 0.8);
            painted += 1;
            paint(
                l.start as usize + lo + k,
                x0.max(-4.0),
                x1.min(width_px + 4.0),
                row,
            );
        }
    }
    (painted, merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata as fixture;

    #[test]
    fn missing_invalid_and_zero_durations_are_distinct() {
        let path = crate::temp_path("numeric-rows-trace.json");
        std::fs::write(&path, "\u{feff}[\n  {\"ph\":\"X\",\"name\":\"zero\",\"ts\":1.0,\"dur\":0.0},\n{\"ph\":\"X\",\"name\":\"missing\",\"ts\":2},\n{\"ph\":\"X\",\"name\":\"negative\",\"ts\":2,\"dur\":-3},\n{\"ph\":\"X\",\"dur\":4},\n{\"ph\":\"X\",\"name\":\"huge\",\"ts\":2,\"dur\":9223372036854775807}\n]\n").unwrap();
        let trace = parse(path.to_str().unwrap()).unwrap();
        assert_eq!(trace.acts.len(), 1);
        assert_eq!(trace.acts[0].dur, 0);
        assert_eq!(trace.health.dropped, 4);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn trace_parse_cancellation_precedes_io() {
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let error =
            parse_opts_with_threads_cancellable("not-present", &LoadOpts::default(), 1, &cancelled)
                .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn run_selection_unifies_metadata_and_action_only_run_ids() {
        let path = crate::temp_path("mixed-run-metadata.json");
        std::fs::write(&path, concat!(
            "[\n",
            r#"{"ph":"M","cat":"run","name":"run_start","ts":100,"args":{"run":"A"}},"#, "\n",
            r#"{"ph":"X","name":"b","ts":110,"dur":20,"args":{"run":"B"}},"#, "\n",
            r#"{"ph":"X","name":"a","ts":120,"dur":10,"args":{"run":"A"}},"#, "\n",
            r#"{"ph":"M","cat":"run","name":"run_end","ts":150,"args":{"run":"A","outcome":"success"}}"#, "\n",
            "]\n",
        )).unwrap();
        let tr = parse(path.to_str().unwrap()).unwrap();
        assert!(select_trace_run(&tr, None).is_err());
        for (selector, expected) in [("A", "A"), ("B", "B"), ("1", "A"), ("2", "B")] {
            assert_eq!(
                select_trace_run(&tr, Some(selector)).unwrap().as_deref(),
                Some(expected)
            );
        }
        assert_eq!(tr.selected_summary(Some("B")).actions, 1);
        assert!(select_trace_run(&tr, Some("3")).is_err());
        std::fs::remove_file(path).unwrap();
    }

    fn profile_row(run: &str) -> serde_json::Value {
        serde_json::json!({"ph":"M","cat":"command-profile","name":"command_profile","ts":100,"args":{
            "run":run,"profile":{"schema_version":1,"origin_us":100,
            "command_start_offset_us":0,"command_end_offset_us":100,
            "lifecycle":[{"kind":"command","start_offset_us":0,"duration_us":100}]}}})
    }

    fn parse_profile_rows(rows: &[serde_json::Value], threads: usize) -> Trace {
        let path = crate::temp_path("command-profile-initial-parse.json");
        let text = format!(
            "[\n{}\n]\n",
            rows.iter()
                .map(|r| serde_json::to_string(r).unwrap())
                .collect::<Vec<_>>()
                .join(",\n")
        );
        std::fs::write(&path, text).unwrap();
        let trace = parse_opts_with_threads(
            path.to_str().unwrap(),
            &LoadOpts {
                min_chunk_bytes: 1,
                ..Default::default()
            },
            threads,
        )
        .unwrap();
        // These tests inspect eagerly parsed fields only, not lazy readers.
        std::fs::remove_file(path).unwrap();
        trace
    }

    fn profile_trace_rows() -> Vec<serde_json::Value> {
        use serde_json::json;
        vec![
            json!({"ph":"M","cat":"run","name":"run_start","ts":100,"args":{"run":"a"}}),
            json!({"ph":"X","name":"action","ts":110,"dur":10,"args":{"run":"a"}}),
            json!({"ph":"M","cat":"run","name":"run_end","ts":200,"args":{"run":"a","exit":0}}),
        ]
    }

    #[test]
    fn command_profiles_are_ready_after_parse_and_preserve_run_header_order() {
        use serde_json::json;
        let mut rows = profile_trace_rows();
        rows.insert(0, profile_row("b"));
        rows.insert(
            2,
            json!({"ph":"M","cat":"run","name":"run_start","ts":100,"args":{"run":"b"}}),
        );
        rows.push(profile_row("a"));
        rows.push(
            json!({"ph":"M","cat":"run","name":"run_end","ts":200,"args":{"run":"b","exit":0}}),
        );
        let one = parse_profile_rows(&rows, 1);
        let many = parse_profile_rows(&rows, 4);
        assert_eq!(
            one.runs.iter().map(|r| r.run.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(one.acts.len(), 1);
        assert_eq!(one.span, 10);
        assert!(one.health.warnings.is_empty(), "{:?}", one.health.warnings);
        assert_eq!(many.health.warnings, one.health.warnings);
        for (a, b) in one.runs.iter().zip(&many.runs) {
            assert_eq!(a.command_profile, b.command_profile);
            assert_eq!(
                a.command_profile.as_ref().unwrap().run.as_deref(),
                Some(a.run.as_str())
            );
        }
        assert_eq!(select_run(&many.runs, Some("2")).unwrap(), 1);
    }

    #[test]
    fn malformed_cross_run_or_duplicate_profiles_never_discard_actions() {
        use serde_json::json;
        let mut invalids = vec![profile_row("a"); 4];
        invalids[0]["args"]["profile"]["schema_version"] = json!(99);
        invalids[1]["args"]["profile"]["origin_us"] = json!(-1);
        invalids[2]["args"]["profile"]["run"] = json!("b");
        invalids[3]["args"]
            .as_object_mut()
            .unwrap()
            .remove("profile");
        for invalid in invalids {
            for with_prior in [false, true] {
                let mut rows = profile_trace_rows();
                if with_prior {
                    rows.push(profile_row("a"));
                }
                rows.push(invalid.clone());
                let trace = parse_profile_rows(&rows, 4);
                assert_eq!(trace.acts.len(), 1);
                assert_eq!(trace.span, 10);
                assert_eq!(trace.health.dropped, 0);
                assert!(trace.runs[0].command_profile.is_none());
                assert!(
                    trace
                        .health
                        .warnings
                        .iter()
                        .any(|w| w.contains("command-profile"))
                );
            }
        }
        let mut rows = profile_trace_rows();
        rows.extend([profile_row("a"), profile_row("a"), profile_row("a")]);
        let trace = parse_profile_rows(&rows, 4);
        assert!(trace.runs[0].command_profile.is_none());
        assert!(
            trace
                .health
                .warnings
                .iter()
                .any(|w| w.contains("multiple command profiles"))
        );
    }

    #[test]
    fn orphan_profiles_do_not_mint_runs_or_change_action_only_selection() {
        use serde_json::json;
        let trace = parse_profile_rows(
            &[
                profile_row("unrelated"),
                json!({"ph":"X","name":"action","ts":110,"dur":10,"args":{"run":"a"}}),
            ],
            4,
        );
        assert!(trace.runs.is_empty());
        assert_eq!(
            select_trace_run(&trace, None).unwrap().as_deref(),
            Some("a")
        );
        assert!(
            trace
                .health
                .warnings
                .iter()
                .any(|w| w.contains("no matching run metadata"))
        );
        let legacy = parse_profile_rows(&profile_trace_rows(), 1);
        assert!(legacy.runs[0].command_profile.is_none());
        assert!(legacy.health.warnings.is_empty());
    }

    #[test]
    fn profile_row_caps_are_global_and_refuse_ambiguous_omitted_duplicates() {
        let mut rows = profile_trace_rows();
        rows.extend((0..=crate::command_profile::MAX_PROFILE_RECORDS).map(|_| profile_row("a")));
        for threads in [1, 4] {
            let trace = parse_profile_rows(&rows, threads);
            assert!(trace.runs[0].command_profile.is_none());
            assert_eq!(trace.acts.len(), 1);
            assert_eq!(trace.health.dropped, 0);
            assert_eq!(trace.health.warnings.len(), 1);
            assert!(trace.health.warnings[0].contains("exceeds reader limits"));
        }
    }

    #[test]
    fn invalid_profile_error_text_is_bounded_without_losing_actions() {
        let mut bad = profile_row("a");
        bad["args"]["profile"]["schema_version"] = serde_json::json!("é".repeat(100_000));
        let mut rows = profile_trace_rows();
        rows.push(bad);
        let trace = parse_profile_rows(&rows, 4);
        assert_eq!(trace.acts.len(), 1);
        assert!(trace.runs[0].command_profile.is_none());
        assert_eq!(trace.health.warnings.len(), 1);
        assert!(trace.health.warnings[0].len() < 1200);
        assert!(trace.health.warnings[0].contains("diagnostic truncated"));
    }

    #[test]
    fn action_only_runs_use_the_same_selection_contract_as_run_headers() {
        let path = crate::temp_path("quality-action-only-selection.json");
        std::fs::write(
            &path,
            concat!(
                "[\n",
                "{\"ph\":\"X\",\"name\":\"a\",\"ts\":0,\"dur\":1,\"args\":{\"run\":\"r1\"}},\n",
                "{\"ph\":\"X\",\"name\":\"b\",\"ts\":1,\"dur\":1,\"args\":{\"run\":\"r2\"}}\n]\n"
            ),
        )
        .unwrap();
        let tr = parse(path.to_str().unwrap()).unwrap();
        assert!(select_trace_run(&tr, None).is_err());
        assert_eq!(
            select_trace_run(&tr, Some("2")).unwrap().as_deref(),
            Some("r2")
        );
        assert_eq!(
            select_trace_run(&tr, Some("r1")).unwrap().as_deref(),
            Some("r1")
        );
        assert!(select_trace_run(&tr, Some("missing")).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn lazy_reads_never_mix_a_replaced_path_or_an_in_place_rewrite() {
        use std::io::Read;
        use std::io::Seek;
        use std::io::SeekFrom;
        let path = crate::temp_path("quality-source-generation.json");
        let original = "[\n{\"ph\":\"X\",\"name\":\"old\",\"ts\":0,\"dur\":1}\n]\n";
        std::fs::write(&path, original).unwrap();
        let tr = parse(path.to_str().unwrap()).unwrap();
        let mut first = tr.source_reader().unwrap();
        let mut second = tr.source_reader().unwrap();
        first.seek(SeekFrom::Start(2)).unwrap();
        let mut buf = [0u8; 2];
        second.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"[\n", "lazy readers have independent seek positions");

        let replacement = crate::temp_path("quality-source-replacement.json");
        std::fs::write(&replacement, original.replace("old", "new")).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        // Unix unlink metadata can conservatively invalidate the old
        // generation. Either refusal or the retained original is safe;
        // reading bytes from the new pathname is never safe.
        if let Ok(mut r) = tr.source_reader() {
            let mut got = String::new();
            r.read_to_string(&mut got).unwrap();
            tr.validate_source().unwrap();
            assert_eq!(got, original);
        }

        let fresh = parse(path.to_str().unwrap()).unwrap();
        std::fs::write(&path, original).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1)),
        )
        .unwrap();
        assert!(
            fresh.source_reader().is_err(),
            "same-length in-place edits invalidate offsets"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn argv_recovery_names_the_build_the_user_typed() {
        // the real 2026-09-05 mobile capture: the target trails a
        // boolean flag, so "skip the token after a flag" would drop it
        assert_eq!(
            targets_from_argv(&[
                "/path/to/buck2",
                "--isolation-dir",
                "mob-igios-noext",
                "build",
                "--client-metadata",
                "reason=mobile laptop build trace capture",
                "--log-action-deps",
                "igios-no-extensions",
            ]),
            vec!["igios-no-extensions".to_owned()]
        );
        // several targets, and a target directly after the subcommand
        assert_eq!(
            targets_from_argv(&["buck2", "build", "//foo:bar", "//baz:qux"]),
            vec!["//foo:bar".to_owned(), "//baz:qux".to_owned()]
        );
        // the subcommand is never itself a target
        assert_eq!(targets_from_argv(&["buck2", "build"]), Vec::<String>::new());
        // a k=v flag value, a numeric flag value and an @argfile all end
        // the run rather than being mistaken for targets
        assert_eq!(
            targets_from_argv(&["buck2", "build", "//a:b", "-c", "foo=bar"]),
            Vec::<String>::new()
        );
        assert_eq!(
            targets_from_argv(&["buck2", "build", "//a:b", "-j", "8"]),
            Vec::<String>::new()
        );
        assert_eq!(
            targets_from_argv(&["buck2", "build", "@mode/opt", "//a:b"]),
            vec!["//a:b".to_owned()]
        );
        // nothing to recover from
        assert_eq!(targets_from_argv(&[]), Vec::<String>::new());
        assert_eq!(targets_from_argv(&["buck2"]), Vec::<String>::new());
    }

    #[test]
    fn smoke_trace_parses_with_buck_semantics() {
        let p = fixture("smoke-trace.json");
        let tr = parse(&p).unwrap();
        assert_eq!(tr.acts.len(), 4);
        assert_eq!(tr.names.len(), 4);
        // phase row (ts before the first recipe) must not move the origin
        assert_eq!(tr.t0_abs, 1_000_100);
        assert_eq!(tr.span, 900);
        assert_eq!(tr.n_hits, 1);
        assert_eq!(tr.n_failed, 1);
        assert_eq!(tr.busy, 1_800); // all rows, phases included
        // cache-hit action classed "cache" via kind, genrule via name
        assert_eq!(tr.class_count[6], 1);
        assert_eq!(tr.class_count[3], 1);
        assert_eq!(tr.class_count[5], 2);
        // 3 overlapping recipes in the daemon band + 1 phase row behind
        assert_eq!(tr.bands.len(), 2);
        assert_eq!(tr.bands[0].nrows, 3);
        assert_eq!(tr.lanes.len(), 4);
        assert!(tr.health.warnings.is_empty(), "{:?}", tr.health.warnings);
        // run merge: start fields + end fields land on one RunInfo
        assert_eq!(tr.runs.len(), 1);
        let r = &tr.runs[0];
        assert_eq!(r.run, "aaaa-bbbb");
        assert_eq!(r.ncpu, Some(64));
        assert_eq!(r.result, Some(1));
        assert_eq!(r.cache_hits, Some(1));
        assert_eq!(
            r.file_changes.as_deref(),
            Some(&["fbcode/x/y.rs".to_owned()][..])
        );
        assert_eq!(r.file_changes_total, Some(1));
        // side tables interned
        assert_eq!(tr.dirs, vec!["/repo".to_owned()]);
        assert_eq!(tr.whys.len(), 1);
        assert_eq!(tr.run_ids, vec!["aaaa-bbbb".to_owned()]);
    }

    fn trace_state(tr: &Trace) -> serde_json::Value {
        let Trace {
            path: _,
            source: _,
            acts,
            names,
            lanes,
            span,
            t0_abs,
            busy,
            n_failed,
            n_hits,
            class_count,
            class_time,
            runs,
            health,
            parse_ms: _,
            index_ms: _,
            dirs,
            whys,
            run_ids,
            bands,
        } = tr;
        serde_json::json!({
            "acts": format!("{acts:?}"), "names": names, "lanes": format!("{lanes:?}"),
            "span": span, "t0_abs": t0_abs, "busy": busy, "n_failed": n_failed,
            "n_hits": n_hits, "class_count": class_count, "class_time": class_time,
            "runs": format!("{runs:?}"), "health": format!("{health:?}"),
            "dirs": dirs, "whys": whys, "run_ids": run_ids, "bands": format!("{bands:?}"),
        })
    }

    #[test]
    fn early_byte_release_preserves_parallel_trace_state_and_lazy_reads() {
        fn lazy(tr: &Trace, act: &Act) -> serde_json::Value {
            use std::io::Read as _;
            use std::io::Seek as _;
            let mut reader = tr.source_reader().unwrap();
            reader.seek(std::io::SeekFrom::Start(act.off)).unwrap();
            let mut bytes = vec![0; act.len as usize];
            reader.read_exact(&mut bytes).unwrap();
            tr.validate_source().unwrap();
            serde_json::from_str(
                String::from_utf8_lossy(&bytes)
                    .trim_end()
                    .trim_end_matches(','),
            )
            .unwrap()
        }
        for name in ["smoke-trace.json", "review-trace.json", "torn-trace.txt"] {
            let path = fixture(name);
            let opts = LoadOpts {
                min_chunk_bytes: 64,
                ..Default::default()
            };
            let serial = parse_opts_with_threads(&path, &opts, 1).unwrap();
            for threads in [1, 4] {
                let parsed = parse_opts_with_threads(&path, &opts, threads).unwrap();
                assert_eq!(trace_state(&serial), trace_state(&parsed));
                for act in &serial.acts {
                    assert_eq!(lazy(&serial, act), lazy(&parsed, act));
                }
                parsed.validate_source().unwrap();
            }
        }
    }

    #[test]
    fn early_byte_release_preserves_input_cap_and_wrong_dialect_refusals() {
        let path = crate::temp_path("early-drop-refusal.json");
        std::fs::write(&path, "{\"graph\":{\"enc\":2}}\n").unwrap();
        for max_input_bytes in [0, 1] {
            let opts = LoadOpts {
                max_input_bytes,
                ..Default::default()
            };
            let p = path.to_str().unwrap();
            let serial = parse_opts_with_threads(p, &opts, 1).err().unwrap();
            let parallel = parse_opts_with_threads(p, &opts, 4).err().unwrap();
            assert_eq!(format!("{serial:#}"), format!("{parallel:#}"));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn parallel_decode_is_bit_identical_to_serial() {
        let p = fixture("smoke-trace.json");
        let one = parse_opts_with_threads(
            &p,
            &LoadOpts {
                threads: 1,
                ..Default::default()
            },
            1,
        )
        .unwrap();
        // the fixture is KB-scale, so the default 4 MiB chunk floor
        // would collapse this back to one chunk; lowering the floor is
        // what makes the multi-chunk decode-and-merge path real here
        let four = parse_opts_with_threads(
            &p,
            &LoadOpts {
                threads: 4,
                min_chunk_bytes: 64,
                ..Default::default()
            },
            4,
        )
        .unwrap();
        assert_eq!(one.acts.len(), four.acts.len());
        for (a, b) in one.acts.iter().zip(four.acts.iter()) {
            assert_eq!(
                (a.ts, a.dur, a.lane, a.color, a.flags, a.seq, a.off, a.len),
                (b.ts, b.dur, b.lane, b.color, b.flags, b.seq, b.off, b.len)
            );
            assert_eq!(one.names[a.name as usize], four.names[b.name as usize]);
        }
        assert_eq!(one.names, four.names);
        assert_eq!(one.dirs, four.dirs);
        assert_eq!(one.span, four.span);
        assert_eq!(one.busy, four.busy);
    }

    #[test]
    fn torn_capture_counts_drops_and_warns() {
        let p = fixture("torn-trace.txt");
        let tr = parse(&p).unwrap();
        assert_eq!(tr.acts.len(), 1);
        assert_eq!(tr.health.warnings.len(), 2, "{:?}", tr.health.warnings);
        assert!(tr.health.warnings[0].contains("1 undecodable"));
        assert!(tr.health.warnings[1].contains("no run_end"));
        // the typed counts mirror the strings
        assert_eq!((tr.health.dropped, tr.health.unended), (1, 1));
        // a run with no run_end is torn, not "outcome unknown" as well
        assert_eq!(tr.runs[0].outcome, Outcome::Unknown);
        assert!(tr.runs[0].dep_capture.is_none());
    }

    #[test]
    fn run_end_with_null_exit_is_ended_but_the_outcome_is_unknown() {
        let write = |name: &str, body: &str| {
            let mut p = std::env::temp_dir();
            p.push(format!("retrace-parse-{}-{name}.json", std::process::id()));
            std::fs::write(&p, body).unwrap();
            p.to_string_lossy().into_owned()
        };
        let p = write(
            "nullexit",
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":0,"pid":1,"args":{"v":1,"run":"a"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"x","ts":0,"dur":5,"args":{"run":"a"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_end","ts":10,"pid":1,"args":{"v":1,"run":"a","exit":null,"outcome":"unknown","dep_capture":{"state":"started","footer_present":false,"finality":"unknown","declared":{},"observed":{"actions_recorded":1,"tset_def_events":0},"command_result_seen":false,"command_end_seen":false}}}"#,
                "\n]\n"
            ),
        );
        let tr = parse(&p).unwrap();
        let r = &tr.runs[0];
        assert!(r.ended);
        assert_eq!(r.result, None);
        assert_eq!(r.outcome, Outcome::Unknown);
        let Some(CaptureState::Started(c)) = &r.dep_capture else {
            panic!("{:?}", r.dep_capture);
        };
        assert!(!c.footer_present);
        assert_eq!(
            tr.health.warnings.len(),
            2,
            "one for the unknown outcome, one per capture problem: {:?}",
            tr.health.warnings
        );
        assert!(tr.health.warnings[0].contains("outcome is unknown"));
        assert!(tr.health.warnings[1].contains("no dependency-capture footer"));
        assert_eq!((tr.health.dropped, tr.health.unended), (0, 0));
        std::fs::remove_file(&p).ok();

        // a trace from before the record carried an outcome
        let p = write(
            "legacy",
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":0,"pid":1,"args":{"v":1,"run":"a"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"x","ts":0,"dur":5,"args":{"run":"a"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_end","ts":10,"pid":1,"args":{"v":1,"run":"a","exit":0}}"#,
                "\n]\n"
            ),
        );
        let tr = parse(&p).unwrap();
        assert_eq!(tr.runs[0].outcome, Outcome::Success);
        assert_eq!(tr.runs[0].result, Some(0));
        assert!(tr.runs[0].dep_capture.is_none());
        assert!(tr.health.warnings.is_empty(), "{:?}", tr.health.warnings);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn duplicate_run_start_merges_and_partial_footers_warn_per_run() {
        // run "a" appears twice (duplicated header) and never ends; run
        // "b" is complete. One RunInfo each, and the footer warning
        // names the one torn run out of two.
        let p = tmp_file(
            "dup-runs-trace.json",
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":1,"args":{"run":"a","ncpu":4}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":2,"args":{"run":"a"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":3,"args":{"run":"b"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_end","ts":9,"args":{"run":"b","exit":0}},"#,
                "\n",
            ),
        );
        let tr = parse(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(tr.runs.len(), 2, "{:?}", tr.runs);
        assert_eq!(tr.runs[0].run, "a");
        assert_eq!(tr.runs[0].ncpu, Some(4), "first header's fields survive");
        assert!(!tr.runs[0].ended);
        assert!(tr.runs[1].ended);
        assert_eq!(tr.health.warnings.len(), 2, "{:?}", tr.health.warnings);
        assert_eq!(tr.runs[0].duplicate_records, 1);
        assert!(
            tr.health
                .warnings
                .iter()
                .any(|w| w.contains("the first was kept"))
        );
        assert!(
            tr.health.warnings[0].contains("1 of 2 runs have no run_end"),
            "{}",
            tr.health.warnings[0]
        );
    }

    #[test]
    fn re_session_is_optional_and_bound_to_the_first_run_header() {
        let p = tmp_file(
            "re-session-trace.json",
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":1,"args":{"run":"a","re_session":"reSessionID-exact"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":2,"args":{"run":"a","re_session":"wrong"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":3,"args":{"run":"b"}}"#,
                "\n]\n",
            ),
        );
        let tr = parse(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(tr.runs[0].re_session.as_deref(), Some("reSessionID-exact"));
        assert_eq!(tr.runs[1].re_session, None);
    }

    #[test]
    fn hostname_is_the_machine_and_host_stays_the_tool_tag() {
        // `host` has always been the trace format's build-tool tag (the
        // literal "buck2"); the machine the build ran on is `hostname`,
        // and it is absent from captures taken before it was recorded.
        let p = tmp_file(
            "hostname-trace.json",
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":1,"args":{"run":"a","host":"buck2","hostname":"devvm31653"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":2,"args":{"run":"b","host":"buck2"}},"#,
                "\n",
            ),
        );
        let tr = parse(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(tr.runs[0].host.as_deref(), Some("buck2"));
        assert_eq!(tr.runs[0].hostname.as_deref(), Some("devvm31653"));
        assert_eq!(tr.runs[1].hostname, None, "older captures name no machine");
    }

    #[test]
    fn hostile_rows_are_counted_dropped_and_duplicate_footers_keep_the_first() {
        let p = tmp_file(
            "hostile-trace.json",
            concat!(
                "[\n",
                r#"{"ph":"M","cat":"run","name":"run_start","ts":1,"args":{"run":"a"}},"#,
                "\n",
                // ts+dur overflows i64: corruption, not a measurement
                r#"{"ph":"X","name":"bad","ts":9223372036854775806,"dur":9,"pid":1,"tid":0},"#,
                "\n",
                // negative ts: same
                r#"{"ph":"X","name":"neg","ts":-5,"dur":9,"pid":1,"tid":0},"#,
                "\n",
                // valid-UTF-8 garbage: neither a record nor framing
                "ph\":\"X\"}\n",
                r#"{"ph":"X","name":"ok (run)","ts":100,"dur":10,"pid":1,"tid":0,"args":{"run":"a"}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_end","ts":200,"args":{"run":"a","exit":0}},"#,
                "\n",
                r#"{"ph":"M","cat":"run","name":"run_end","ts":300,"args":{"run":"a","exit":7}},"#,
                "\n",
                "]\n",
            ),
        );
        let tr = parse(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(tr.acts.len(), 1, "only the sane row survives");
        assert!(
            tr.health
                .warnings
                .iter()
                .any(|w| w.contains("3 undecodable")),
            "all three hostile lines counted: {:?}",
            tr.health.warnings
        );
        assert_eq!(tr.runs.len(), 1);
        assert_eq!(tr.runs[0].result, Some(0), "first footer wins");
    }

    fn tmp_file(tag: &str, contents: &str) -> String {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "retrace-parse-test-{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            tag
        ));
        std::fs::write(&path, contents).unwrap();
        path.to_str().unwrap().to_owned()
    }

    #[test]
    fn empty_array_is_framing_but_nonempty_single_line_arrays_are_refused() {
        for text in ["[]\n", "[\n]\n"] {
            let path = tmp_file("empty-array-trace.json", text);
            let tr = parse(&path).expect("should accept an empty trace array");
            assert!(tr.acts.is_empty());
            assert_eq!(tr.health.dropped, 0);
            std::fs::remove_file(path).expect("should remove empty trace fixture");
        }
        let path = tmp_file(
            "single-line-array-trace.json",
            "[{\"ph\":\"X\",\"name\":\"a\",\"ts\":0,\"dur\":1}]\n",
        );
        let error = parse(&path).expect_err("should require one event per line");
        assert!(error.to_string().contains("not a single-line JSON array"));
        std::fs::remove_file(path).expect("should remove single-line trace fixture");
    }

    #[test]
    fn cull_keeps_long_actions_that_begin_before_the_view() {
        let path = tmp_file(
            "cull-overlap-trace.json",
            "[\n{\"ph\":\"X\",\"name\":\"A\",\"ts\":0,\"dur\":1000},\n\
             {\"ph\":\"X\",\"name\":\"B\",\"ts\":10,\"dur\":1}\n]\n",
        );
        let tr = parse(&path).expect("should parse overlapping action fixture");
        assert_eq!(tr.lanes.len(), 2);
        let mut seen = Vec::new();
        cull(&tr, 500.0, 1.0, 0, tr.lanes.len(), 1000.0, |i, _, _, _| {
            seen.push(i)
        });
        assert_eq!(seen, [tr.lanes[0].start as usize]);
        assert_eq!(tr.names[tr.acts[seen[0]].name as usize], "A");
        std::fs::remove_file(path).expect("should remove culling fixture");
    }

    #[test]
    fn graph_sidecar_as_trace_is_refused_with_directions() {
        // trace-buck graph dialect
        let p = tmp_file("g1.json", "{\"graph\": {\"nodes\": []}}\n");
        let err = format!("{:#}", parse(&p).unwrap_err());
        assert!(err.contains("graph"), "{err}");
        assert!(err.contains("--graph="), "{err}");
        std::fs::remove_file(&p).unwrap();
        // raw buck2 graph dialect
        let p = tmp_file("g2.json", "{\"header\": {\"schema\": 1}}\n");
        let err = format!("{:#}", parse(&p).unwrap_err());
        assert!(err.contains("header"), "{err}");
        assert!(err.contains("--graph="), "{err}");
        std::fs::remove_file(&p).unwrap();
        // any other lone JSON object is still not a trace
        let p = tmp_file("g3.json", "  {\"something\": 1}\n");
        let err = format!("{:#}", parse(&p).unwrap_err());
        assert!(err.contains("single JSON object"), "{err}");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn input_cap_refuses_upfront_and_names_the_flag() {
        let p = fixture("smoke-trace.json");
        let err = format!(
            "{:#}",
            parse_opts(
                &p,
                &LoadOpts {
                    max_input_bytes: 10,
                    ..Default::default()
                },
            )
            .unwrap_err()
        );
        assert!(err.contains("--max-input-bytes"), "{err}");
    }

    #[test]
    fn run_selection_id_then_ordinal_then_refusal() {
        let mk = |id: &str| RunInfo {
            run: id.to_owned(),
            ..Default::default()
        };
        let runs = vec![mk("111e8400-uuid-a"), mk("222e8400-uuid-b")];
        // single run: no selector needed
        assert_eq!(select_run(&runs[..1], None).unwrap(), 0);
        // several runs: refuse without a selector, listing candidates
        let err = format!("{:#}", select_run(&runs, None).unwrap_err());
        assert!(
            err.contains("2 runs") && err.contains("222e8400-uuid-b"),
            "{err}"
        );
        // exact id match
        assert_eq!(select_run(&runs, Some("222e8400-uuid-b")).unwrap(), 1);
        // 1-based ordinal
        assert_eq!(select_run(&runs, Some("2")).unwrap(), 1);
        // id match beats ordinal when a run is literally named "2"
        let tricky = vec![mk("a"), mk("2"), mk("b")];
        assert_eq!(select_run(&tricky, Some("2")).unwrap(), 1);
        // nonsense selector refused with the candidate list
        let err = format!("{:#}", select_run(&runs, Some("nope")).unwrap_err());
        assert!(err.contains("1..=2"), "{err}");
    }

    #[test]
    fn classify_buck_names() {
        assert_eq!(classify("z (cxx_link)", Some("action_cache")), 6);
        assert_eq!(
            classify("z (cxx_link)", Some("remote_served_from_cache")),
            6,
            "RE serving its own cache is a cache hit"
        );
        assert_eq!(classify("z (cxx_link)", Some("local")), 2);
        assert_eq!(classify("w (genrule)", Some("local")), 3);
        assert_eq!(classify("y.o (cxx_compile)", Some("remote")), 0);
        // suffix rules look at the LAST space-separated token
        assert_eq!(classify("libx.a", None), 1);
        assert_eq!(classify("out/run.sh", None), 4);
        assert_eq!(classify("y (rustc emit)", Some("local")), 5);
    }

    #[test]
    fn field_repair_keeps_run_counts_and_marks_invalid_re_counters() {
        let text = r#"{"ph":"M","name":"run_end","args":{"failed":3,"maxrss":"bad","re":{"invalid_counter_fields":null,"iu_sem_wait_us":"bad","iu_cas_call_us":12}}}"#;
        let fixed = repair_optional_args(text).unwrap();
        assert_eq!(fixed.rejected_fields(), 3);
        assert_eq!(fixed.unmarked_rejections, 2);
        let value: serde_json::Value = serde_json::from_str(&fixed.text).unwrap();
        assert_eq!(value["args"]["failed"], 3);
        assert_eq!(value["args"]["re"]["iu_cas_call_us"], 12);
        assert_eq!(
            value["args"]["re"]["invalid_counter_fields"],
            serde_json::json!(["iu_sem_wait_us"])
        );
        assert!(value["args"]["re"].get("iu_sem_wait_us").is_none());
    }

    #[test]
    fn chunk_bounds_are_line_aligned_and_cover() {
        let data = b"aaa\nbb\nccccc\nd\n";
        for want in 1..6 {
            let b = chunk_bounds(data, want);
            assert_eq!(b[0].0, 0);
            assert_eq!(b.last().unwrap().1, data.len());
            for w in b.windows(2) {
                assert_eq!(w[0].1, w[1].0);
            }
            for &(s, _) in &b[1..] {
                assert_eq!(data[s - 1], b'\n', "chunk must start after a newline");
            }
        }
    }
}
