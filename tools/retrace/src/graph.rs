/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Dependency-graph loading: the newline-delimited JSON the buck2
//! importer writes with `--graph` — one header line, then one row per
//! action (`{"t": name, "d": [dep names]}`) plus one zero-weight join
//! row per referenced transitive-set projection or explicitly recorded
//! available/reused artifact structure (`"j":1`). Structural rows are not
//! executions and carry their producer-reported `structure_reuse_kind`.
//!
//! Storage model: target names map to dense IDs (`targets` + `index`).
//! Scan-order rows are temporary construction data for `pred_off`/`preds`;
//! repeated target rows concatenate in file order. Production drops the raw
//! rows before cycle analysis and does not build a transpose. Tests retain
//! both representations for independent structural checks.
//!
//! Row parsing avoids `serde_json::Value` entirely: rows are scanned by
//! hand straight into interned ids (fb4a's graph is 2.5 GB / 46.6M
//! edges). The single header is rare, so it falls back to `serde_json`.
//!
//! Row grammar: a row is a JSON object. A line that is not JSON is a
//! dropped line, and dropped lines are told apart by position: those
//! followed by an accepted line are `dropped_interior`, the rest are
//! the torn tail of a killed build. A line that is JSON but whose `t`
//! is not a string, or whose `d`/`o` is not an array of strings, is a
//! `bad_row`; a rejected line leaves no name behind. The row keys are
//! read per occurrence in key order, and only until a `graph` or
//! `graph_end` key has been seen.
//!
//! Refusals (docs/contract.md): an unsupported encoding/scope (`enc:2` is
//! full, `enc:3` is current-action ancestry), a second header (appended captures),
//! no header at all, undefined references in a file with no torn tail
//! — the importer emits definitions for everything it references, so a
//! dangling reference with no torn tail is corruption, while a torn
//! tail degrades to counted dangling refs instead — and a dependency
//! cycle: deleting rows from a real build graph cannot create one, so a
//! cycle means the file is corrupt or two different actions share one
//! name. `GraphLoadOpts::repair_cycles` loads a cyclic graph anyway;
//! the walks in `dag.rs` then drop and count each edge that closes a
//! cycle, and every consumer must report the result as partial with no
//! bound.

use std::borrow::Cow;
use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Context as _;
use fxhash::FxHashMap;

use crate::types::CaptureScope;
use crate::types::CaptureState;
use crate::types::RunInfo;
use crate::types::Trace;

mod parallel;

/// Header `coverage` block: the importer's own accounting, kept so
/// analyses can reconcile what the loader saw against what the writer
/// declared (denominators come from the input's declarations).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    pub nodes: Option<i64>,
    pub edges: Option<i64>,
    pub dangling_deps: Option<i64>,
    pub dangling_tsets: Option<i64>,
    /// Executed action rows and all zero-action-work join rows, separately.
    pub action_rows: Option<i64>,
    pub join_rows: Option<i64>,
    /// Subset of join_rows representing explicit artifact structure.
    pub structural_rows: Option<i64>,
    pub canonical_aliases: Option<i64>,
    /// the dependency capture's state as the writer saw it; the same
    /// object the trace's `run_end` carries
    pub dep_capture: Option<CaptureState>,
}

/// The one graph header.
#[derive(Debug, Clone, Default)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct GraphMeta {
    pub dir: String,
    pub goals: Vec<String>,
    pub run: Option<String>,
    pub enc: Option<i64>,
    pub kind: String,
    pub note: Option<String>,
    pub coverage: Option<Coverage>,
    /// stamp of the capture this graph was written from; the trace it
    /// sits beside must carry the same one
    pub capture: Option<String>,
}

/// Whether a trace run and a graph belong together: a graph that names
/// a different build, a different capture of the same build (a torn
/// copy against the whole log, or an earlier ingest), or only one of
/// the two carrying a capture stamp, is refused as a pair. Two
/// unstamped files, written before the stamp existed, still pair.
pub fn check_pair(run: &RunInfo, g: &GraphMeta) -> anyhow::Result<()> {
    if let Some(gr) = g.run.as_deref().filter(|r| !r.is_empty()) {
        if !run.run.is_empty() && gr != run.run {
            anyhow::bail!(
                "the graph is for build {gr}, the trace run is {}; pass --graph= for that run \
                 or --run={gr}",
                run.run
            );
        }
    }
    match (run.capture.as_deref(), g.capture.as_deref()) {
        (Some(a), Some(b)) if a != b => anyhow::bail!(
            "the graph beside this trace was written from a different capture of the same \
             build (trace {a}, graph {b}); re-run `retrace ingest` to rewrite the pair"
        ),
        (Some(_), None) | (None, Some(_)) => anyhow::bail!(
            "the trace and the graph beside it were written by different ingests (only one \
             carries a capture stamp); re-run `retrace ingest` to rewrite the pair"
        ),
        _ => Ok(()),
    }
}

/// The admission gate for every graph consumer, before stitching. The
/// single action-only run of an old trace is just as authoritative as a
/// run header. Callers decide whether a rejected *discovered* sidecar
/// should be ignored with a warning or an explicit input should fail.
pub fn check_trace_pair(tr: &Trace, run: Option<&str>, g: &GraphMeta) -> anyhow::Result<()> {
    let run_id = crate::parse::select_trace_run(tr, run)?;
    if let Some(info) = run_id
        .as_deref()
        .and_then(|id| tr.runs.iter().find(|r| r.run == id))
    {
        return check_pair(info, g);
    }
    check_pair(
        &RunInfo {
            run: run_id.unwrap_or_default(),
            ..Default::default()
        },
        g,
    )
}

/// Rows in file order: row `i` declares target `t[i]` with deps
/// `deps[off[i]..off[i + 1]]`.
#[derive(Debug, Default)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Rows {
    pub t: Vec<u32>,
    pub off: Vec<u32>,
    pub deps: Vec<u32>,
    /// row carried `"j":1` — a zero-weight tset join node, not an
    /// action-producing target
    pub j: Vec<bool>,
}

impl Rows {
    pub fn len(&self) -> usize {
        self.t.len()
    }
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }
    pub fn deps_of(&self, i: usize) -> &[u32] {
        &self.deps[self.off[i] as usize..self.off[i + 1] as usize]
    }
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Graph {
    pub path: String,
    /// interned target names (row targets AND dep references)
    pub targets: Vec<Arc<str>>,
    /// name -> target id
    pub index: FxHashMap<Arc<str>, u32>,
    /// target i appeared as some row's `"t"`; a dep reference alone
    /// does not set this
    pub is_key: Vec<bool>,
    /// target i's defining row carried `"j":1`: a zero-weight join node.
    /// Joins are transparent to reachability and contribute no work;
    /// analyses must never count them as actions nor collapse them.
    pub is_join: Vec<bool>,
    /// CSR: `preds[pred_off[i]..pred_off[i + 1]]` = dep target ids of target
    /// i, in declared file order (repeated `t` rows concatenate)
    pub pred_off: Vec<u32>,
    pub preds: Vec<u32>,
    /// Test oracle only; the stitched DAG owns the production successor index.
    #[cfg(test)]
    pub succ_off: Vec<u32>,
    #[cfg(test)]
    pub succs: Vec<u32>,
    pub n_edges: usize,
    pub n_rows: usize,
    #[cfg(test)]
    pub rows: Rows,
    pub meta: GraphMeta,
    /// lines that are not JSON, over the whole file
    pub dropped_lines: u64,
    /// of `dropped_lines`, those followed by an accepted line; the rest
    /// are the torn tail, and only a torn tail relaxes the
    /// undefined-reference refusal
    pub dropped_interior: u64,
    /// row lines that are JSON but of the wrong shape: no `"t"` string,
    /// or a `"d"`/`"o"` that is not an array of strings
    pub bad_rows: u64,
    /// distinct `tset:` names referenced but never defined by a row
    pub undefined_joins: u64,
    /// distinct non-tset names referenced but never a row target
    pub undefined_deps: u64,
    /// a few undefined names, for messages
    pub undefined_samples: Vec<String>,
    /// targets on a dependency cycle (a strongly connected component of
    /// more than one target, or a self-dependency); non-zero only under
    /// `GraphLoadOpts::repair_cycles`, the loader refuses otherwise
    pub targets_in_cycles: u64,
    /// a few `consumer -> dependency` edges inside cycles, for messages
    pub cycle_samples: Vec<String>,
    pub load_ms: u128,
}

impl Graph {
    pub fn id(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }

    pub fn n_targets(&self) -> usize {
        self.targets.len()
    }

    /// Declared deps of target `t`.
    pub fn merged_deps(&self, t: u32) -> &[u32] {
        match self.pred_off.get(t as usize + 1) {
            Some(&e) => &self.preds[self.pred_off[t as usize] as usize..e as usize],
            None => &[],
        }
    }

    #[cfg(test)]
    pub fn merged_succs(&self, t: u32) -> &[u32] {
        match self.succ_off.get(t as usize + 1) {
            Some(&e) => &self.succs[self.succ_off[t as usize] as usize..e as usize],
            None => &[],
        }
    }

    /// `t` has a defining row.
    pub fn merged_has(&self, t: u32) -> bool {
        self.is_key.get(t as usize).copied().unwrap_or(false)
    }
}

/// The importer's sibling rule: the basename must contain the literal
/// "trace"; EVERY occurrence is replaced by "graph". Returns the path
/// only when the sibling actually exists. The converter names both
/// halves `.json`; a second probe swaps the extension to `.jsonl`, the
/// suffix the checked-in fixtures use for the line-delimited sidecar.
pub fn sibling_graph_path(trace_path: &str) -> Option<String> {
    let p = std::path::Path::new(trace_path);
    let name = p.file_name()?.to_str()?;
    if !name.contains("trace") {
        return None;
    }
    let cand = p.with_file_name(name.replace("trace", "graph"));
    if cand.exists() {
        return Some(cand.to_string_lossy().into_owned());
    }
    let jsonl = cand.with_extension("jsonl");
    if jsonl.exists() {
        return Some(jsonl.to_string_lossy().into_owned());
    }
    None
}

// ---------------------------------------------------------------- scanning

#[inline]
fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// `b[i]` must be `"`. Returns `(content_start, content_end, next,
/// escaped)`. A raw control character inside the string is a rejection;
/// escapes are validated when the string is decoded.
#[inline]
fn json_str(b: &[u8], i: usize) -> Option<(usize, usize, usize, bool)> {
    let mut j = i + 1;
    let mut esc = false;
    while j < b.len() {
        match b[j] {
            b'\\' => {
                esc = true;
                j += 2;
            }
            b'"' => return Some((i + 1, j, j + 1, esc)),
            c if c < 0x20 => return None,
            _ => j += 1,
        }
    }
    None
}

fn hex4(raw: &[u8], at: usize) -> Option<u32> {
    let hex = raw.get(at..at + 4)?;
    if !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
}

fn unescape(raw: &[u8]) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'\\' {
            let s = i;
            while i < raw.len() && raw[i] != b'\\' {
                i += 1;
            }
            out.push_str(std::str::from_utf8(&raw[s..i]).ok()?);
            continue;
        }
        i += 1;
        let c = *raw.get(i)?;
        i += 1;
        match c {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let cp = hex4(raw, i)?;
                i += 4;
                // surrogates only ever come as an escaped lead+trail
                // pair; a lone one is rejected as serde_json rejects it
                let cp = if (0xd800..0xdc00).contains(&cp) {
                    if raw.get(i) != Some(&b'\\') || raw.get(i + 1) != Some(&b'u') {
                        return None;
                    }
                    let lo = hex4(raw, i + 2)?;
                    if !(0xdc00..0xe000).contains(&lo) {
                        return None;
                    }
                    i += 6;
                    0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00)
                } else if (0xdc00..0xe000).contains(&cp) {
                    return None;
                } else {
                    cp
                };
                out.push(char::from_u32(cp)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[inline]
fn decode(raw: &[u8], esc: bool) -> Option<Cow<'_, str>> {
    if esc {
        unescape(raw).map(Cow::Owned)
    } else {
        std::str::from_utf8(raw).ok().map(Cow::Borrowed)
    }
}

/// Validates one JSON literal (`true`, `false`, `null` or a number)
/// starting at `i`; returns the index just past it.
fn scan_literal(b: &[u8], i: usize) -> Option<usize> {
    for lit in [&b"true"[..], &b"false"[..], &b"null"[..]] {
        if b.get(i..i + lit.len()) == Some(lit) {
            return Some(i + lit.len());
        }
    }
    let digits = |mut j: usize| {
        while matches!(b.get(j), Some(b'0'..=b'9')) {
            j += 1;
        }
        j
    };
    let mut j = i;
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    match *b.get(j)? {
        b'0' => j += 1,
        b'1'..=b'9' => j = digits(j),
        _ => return None,
    }
    if b.get(j) == Some(&b'.') {
        j += 1;
        if !matches!(b.get(j), Some(b'0'..=b'9')) {
            return None;
        }
        j = digits(j);
    }
    if matches!(b.get(j), Some(b'e' | b'E')) {
        j += 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if !matches!(b.get(j), Some(b'0'..=b'9')) {
            return None;
        }
        j = digits(j);
    }
    Some(j)
}

/// Skips one JSON value of any shape starting at `i` by handing it to
/// serde_json, so a value the loader does not read gets the standard
/// parser's validation (strings included). Returns the index just past
/// it.
fn skip_any(b: &[u8], i: usize) -> Option<usize> {
    let mut it = serde_json::Deserializer::from_slice(&b[i..]).into_iter::<serde_json::Value>();
    it.next()?.ok()?;
    Some(i + it.byte_offset())
}

enum Rec {
    Header,
    Unknown,
    Row,
}

/// Why a line was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowErr {
    /// not JSON (or trailing garbage after the object)
    Malformed,
    /// a read key had the wrong shape: `t` not a string, `d`/`o` not an
    /// array of strings. Meaningful only once the caller has confirmed
    /// the whole line is JSON
    Shape,
}

struct Interner {
    targets: Vec<Arc<str>>,
    index: FxHashMap<Arc<str>, u32>,
}

impl Interner {
    #[inline]
    fn get_or(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.targets.len() as u32;
        let s: Arc<str> = Arc::from(s);
        self.targets.push(s.clone());
        self.index.insert(s, i);
        i
    }

    fn mark(&self) -> usize {
        self.targets.len()
    }

    /// Forgets every name interned since `mark`, so a rejected row
    /// leaves nothing `Graph::id` could resolve.
    fn rollback(&mut self, mark: usize) {
        for name in self.targets.drain(mark..) {
            self.index.remove(&name);
        }
    }
}

/// Hand-rolled one-pass object scan. Classifies the record and, for rows,
/// interns `t`/`d`/`o` directly. The grammar is JSON's: what serde_json
/// rejects, this rejects (the differential test below is the arbiter),
/// and values under keys the loader does not read go through serde_json.
fn scan_object(
    b: &[u8],
    it: &mut Interner,
    dbuf: &mut Vec<u32>,
    obuf: &mut Vec<u32>,
    t_id: &mut Option<u32>,
    j_flag: &mut bool,
    need_deps: bool,
) -> Result<Rec, RowErr> {
    use RowErr::Malformed;
    let at = |i: usize| b.get(i).copied().ok_or(Malformed);
    let mut i = skip_ws(b, 0);
    if at(i)? != b'{' {
        return Err(Malformed);
    }
    i += 1;
    let mut saw_graph = false;
    let mut saw_other = false;
    let mut saw_row_key = false;
    // Header/footer keys win over row fields regardless of JSON key order.
    let mut shape = false;
    // separator state: after `{` a key or `}`; after a value `,` or
    // `}`; after `,` a key only
    let mut need_sep = false;
    let mut after_comma = false;
    loop {
        i = skip_ws(b, i);
        match at(i)? {
            b'}' if !after_comma => {
                i += 1;
                break;
            }
            b',' if need_sep => {
                i += 1;
                need_sep = false;
                after_comma = true;
                continue;
            }
            b'"' if !need_sep => {}
            _ => return Err(Malformed),
        }
        let (ks, ke, next, kesc) = json_str(b, i).ok_or(Malformed)?;
        i = skip_ws(b, next);
        if at(i)? != b':' {
            return Err(Malformed);
        }
        i = skip_ws(b, i + 1);
        let key = decode(&b[ks..ke], kesc).ok_or(Malformed)?;
        match key.as_ref() {
            "graph" => {
                saw_graph = true;
                i = skip_any(b, i).ok_or(Malformed)?;
            }
            "graph_end" => {
                // make-era completeness footer; buck imports never
                // write one, but tolerate the record
                saw_other = true;
                i = skip_any(b, i).ok_or(Malformed)?;
            }
            "t" if !saw_graph && !saw_other => {
                if at(i)? != b'"' {
                    shape = true;
                    i = skip_any(b, i).ok_or(Malformed)?;
                } else {
                    let (s, e, next, esc) = json_str(b, i).ok_or(Malformed)?;
                    let name = decode(&b[s..e], esc).ok_or(Malformed)?;
                    *t_id = Some(if need_deps {
                        it.get_or(name.as_ref())
                    } else {
                        0
                    });
                    saw_row_key = true;
                    i = next;
                }
            }
            "j" if !saw_graph && !saw_other => {
                // the writer emits `"j":1`; any other JSON value is a
                // valid row that is not a join
                match at(i)? {
                    b'"' | b'[' | b'{' => {
                        *j_flag = false;
                        i = skip_any(b, i).ok_or(Malformed)?;
                    }
                    _ => {
                        let end = scan_literal(b, i).ok_or(Malformed)?;
                        *j_flag = matches!(&b[i..end], b"1" | b"true");
                        i = end;
                    }
                }
            }
            "d" | "o" if !saw_graph && !saw_other => {
                let into = if key.as_ref() == "d" {
                    &mut *dbuf
                } else {
                    &mut *obuf
                };
                into.clear(); // duplicate key: last wins, like json.loads
                saw_row_key = true;
                if at(i)? != b'[' {
                    shape = true;
                    i = skip_any(b, i).ok_or(Malformed)?;
                    need_sep = true;
                    after_comma = false;
                    continue;
                }
                let arr_start = i;
                i += 1;
                // the same separator state as the object
                let mut need_sep = false;
                let mut after_comma = false;
                loop {
                    i = skip_ws(b, i);
                    match at(i)? {
                        b']' if !after_comma => {
                            i += 1;
                            break;
                        }
                        b',' if need_sep => {
                            i += 1;
                            need_sep = false;
                            after_comma = true;
                        }
                        b'"' if !need_sep => {
                            let (s, e, next, esc) = json_str(b, i).ok_or(Malformed)?;
                            let name = decode(&b[s..e], esc).ok_or(Malformed)?;
                            if need_deps {
                                into.push(it.get_or(name.as_ref()));
                            }
                            i = next;
                            need_sep = true;
                            after_comma = false;
                        }
                        // an element that is not a string
                        _ if !need_sep => {
                            shape = true;
                            into.clear();
                            i = skip_any(b, arr_start).ok_or(Malformed)?;
                            break;
                        }
                        _ => return Err(Malformed),
                    }
                }
            }
            _ => i = skip_any(b, i).ok_or(Malformed)?,
        }
        need_sep = true;
        after_comma = false;
    }
    if skip_ws(b, i) != b.len() {
        return Err(Malformed); // trailing garbage: json.loads would reject too
    }
    Ok(if saw_graph {
        Rec::Header
    } else if saw_other {
        Rec::Unknown
    } else if shape {
        return Err(RowErr::Shape);
    } else if saw_row_key {
        Rec::Row
    } else {
        Rec::Unknown
    })
}

/// Targets on a dependency cycle, found once at load by an iterative
/// Tarjan pass over the CSR: a target in a strongly connected component
/// of more than one target, or one that depends on itself. Samples are
/// edges with both ends in the same component. One O(V+E) sweep, the
/// same shape as the transpose.
fn find_cycles<F: Fn() -> anyhow::Result<()>>(
    pred_off: &[u32],
    preds: &[u32],
    names: &[Arc<str>],
    check: &F,
) -> anyhow::Result<(u64, Vec<String>)> {
    let nt = pred_off.len().saturating_sub(1);
    const NONE: u32 = u32::MAX;
    let mut index = vec![NONE; nt];
    let mut low = vec![0u32; nt];
    let mut on_stack = vec![false; nt];
    let mut comp = vec![NONE; nt];
    let mut comp_size: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    // (target, next edge position)
    let mut work: Vec<(u32, u32)> = Vec::new();
    let mut next_index = 0u32;
    let mut visits = 0usize;
    for root in 0..nt {
        if root & 4095 == 0 {
            check()?;
        }
        if index[root] != NONE {
            continue;
        }
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root as u32);
        on_stack[root] = true;
        work.push((root as u32, pred_off[root]));
        while let Some(&(v, pos)) = work.last() {
            visits = visits.wrapping_add(1);
            if visits & 4095 == 0 {
                check()?;
            }
            let vi = v as usize;
            if pos < pred_off[vi + 1] {
                work.last_mut().expect("just peeked").1 = pos + 1;
                let w = preds[pos as usize] as usize;
                if index[w] == NONE {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w as u32);
                    on_stack[w] = true;
                    work.push((w as u32, pred_off[w]));
                } else if on_stack[w] {
                    low[vi] = low[vi].min(index[w]);
                }
                continue;
            }
            if low[vi] == index[vi] {
                let id = comp_size.len() as u32;
                let mut size = 0u32;
                loop {
                    let w = stack.pop().expect("v is on the stack") as usize;
                    on_stack[w] = false;
                    comp[w] = id;
                    size += 1;
                    if w == vi {
                        break;
                    }
                }
                comp_size.push(size);
            }
            work.pop();
            if let Some(&(u, _)) = work.last() {
                let ui = u as usize;
                low[ui] = low[ui].min(low[vi]);
            }
        }
    }
    let mut n = 0u64;
    let mut samples = Vec::new();
    for t in 0..nt {
        if t & 4095 == 0 {
            check()?;
        }
        let c = comp[t];
        let deps = &preds[pred_off[t] as usize..pred_off[t + 1] as usize];
        let in_cycle = comp_size[c as usize] > 1 || deps.contains(&(t as u32));
        if !in_cycle {
            continue;
        }
        n += 1;
        if samples.len() < 5
            && let Some(&d) = deps.iter().find(|&&d| comp[d as usize] == c)
        {
            samples.push(format!("{} -> {}", names[t], names[d as usize]));
        }
    }
    Ok((n, samples))
}

// ------------------------------------------------------------------ loader

struct GraphRows {
    it: Interner,
    rows: Rows,
    meta: Option<GraphMeta>,
    dropped_interior: u64,
    pending_drops: u64,
    bad_rows: u64,
}

impl GraphRows {
    fn new() -> Self {
        let mut rows = Rows::default();
        rows.off.push(0);
        Self {
            it: Interner {
                targets: Vec::new(),
                index: FxHashMap::default(),
            },
            rows,
            meta: None,
            dropped_interior: 0,
            pending_drops: 0,
            bad_rows: 0,
        }
    }
}

fn parse_graph_header(path: &str, b: &[u8], meta: &mut Option<GraphMeta>) -> anyhow::Result<bool> {
    if meta.is_some() {
        anyhow::bail!(
            "{path}: a second graph header appeared — appended graph captures are \
             not supported; convert each build to its own graph file"
        );
    }
    let v: serde_json::Value = match serde_json::from_slice(b) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let Some(g) = v.get("graph") else {
        return Ok(true);
    };
    let note = g.get("note").and_then(|x| x.as_str()).map(|x| x.to_owned());
    let kind_s = match g.get("kind").and_then(|x| x.as_str()) {
        Some(k) if !k.is_empty() => k.to_owned(),
        _ => {
            if note.as_deref().unwrap_or("").starts_with("cp-chain-only") {
                "cp_chain".to_owned()
            } else {
                "full".to_owned()
            }
        }
    };
    let enc = g.get("enc").and_then(|x| x.as_i64());
    if kind_s == "cp_chain" {
        anyhow::bail!(
            "{path}: this is a critical-path-chain export, not the real dependency \
             graph — capture with --log-action-graph and re-convert"
        );
    }
    let activity_scope = kind_s == "current_action_ancestors";
    let required_encoding = if activity_scope { 3 } else { 2 };
    if enc != Some(required_encoding) {
        anyhow::bail!(
            "{path}: header carries enc {} where {required_encoding} is required — a flattened or \
             pre-RFC export; analyzing it as the real graph would produce confident \
             nonsense, re-convert with a current trace-buck.py",
            enc.map(|e| e.to_string())
                .unwrap_or_else(|| "<absent>".into())
        );
    }
    let cov = g.get("coverage").map(|c| Coverage {
        nodes: c.get("nodes").and_then(|x| x.as_i64()),
        edges: c.get("edges").and_then(|x| x.as_i64()),
        dangling_deps: c.get("dangling_deps").and_then(|x| x.as_i64()),
        dangling_tsets: c.get("dangling_tsets").and_then(|x| x.as_i64()),
        action_rows: c.get("action_rows").and_then(|x| x.as_i64()),
        join_rows: c.get("join_rows").and_then(|x| x.as_i64()),
        structural_rows: c.get("structural_rows").and_then(|x| x.as_i64()),
        canonical_aliases: c.get("canonical_aliases").and_then(|x| x.as_i64()),
        dep_capture: c.get("dep_capture").and_then(CaptureState::from_json),
    });
    let scope = cov
        .as_ref()
        .and_then(|c| c.dep_capture.as_ref())
        .and_then(CaptureState::scope);
    if scope == Some(CaptureScope::Unknown)
        || activity_scope != (scope == Some(CaptureScope::CurrentActionAncestors))
    {
        anyhow::bail!(
            "{path}: graph encoding/kind contradicts or omits its capture scope; \
             re-run `retrace ingest` with a reader supporting this capture"
        );
    }
    *meta = Some(GraphMeta {
        dir: g
            .get("dir")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned(),
        goals: g
            .get("goals")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| match x {
                        serde_json::Value::String(s) => s.clone(),
                        o => o.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        run: g.get("run").and_then(|x| x.as_str()).map(|x| x.to_owned()),
        enc,
        kind: kind_s,
        note,
        coverage: cov,
        capture: g
            .get("capture")
            .and_then(|x| x.as_str())
            .map(|x| x.to_owned()),
    });
    Ok(true)
}

fn scan_rows_serial<R: BufRead, F: Fn() -> anyhow::Result<()>>(
    rd: &mut R,
    path: &str,
    need_deps: bool,
    check: &F,
) -> anyhow::Result<GraphRows> {
    let mut it = Interner {
        targets: Vec::new(),
        index: FxHashMap::default(),
    };
    let mut rows = Rows::default();
    rows.off.push(0);
    let mut meta: Option<GraphMeta> = None;
    // a dropped line is interior once a later line is accepted;
    // whatever is still pending at the end is the torn tail
    let mut dropped_interior: u64 = 0;
    let mut pending_drops: u64 = 0;
    let mut bad_rows: u64 = 0;

    let mut line: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut dbuf: Vec<u32> = Vec::new();
    let mut obuf: Vec<u32> = Vec::new();

    loop {
        check()?;
        line.clear();
        match rd.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(e).with_context(|| path.to_owned()),
        }
        let mut a = 0usize;
        let mut z = line.len();
        while a < z && matches!(line[a], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            a += 1;
        }
        while z > a && matches!(line[z - 1], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            z -= 1;
        }
        if a == z {
            continue;
        }
        let b = &line[a..z];

        dbuf.clear();
        obuf.clear();
        let mut t_id: Option<u32> = None;
        let mut j_flag = false;
        let mark = it.mark();
        let kind = match scan_object(
            b,
            &mut it,
            &mut dbuf,
            &mut obuf,
            &mut t_id,
            &mut j_flag,
            need_deps,
        ) {
            Ok(k) => k,
            Err(e) => {
                it.rollback(mark);
                // JSON of the wrong shape is a bad row; anything else is
                // a dropped line whose position is settled by what follows
                if e == RowErr::Shape && serde_json::from_slice::<serde_json::Value>(b).is_ok() {
                    bad_rows += 1;
                    dropped_interior += pending_drops;
                    pending_drops = 0;
                } else {
                    pending_drops += 1;
                }
                continue;
            }
        };
        dropped_interior += pending_drops;
        pending_drops = 0;

        match kind {
            Rec::Header => {
                if !parse_graph_header(path, b, &mut meta)? {
                    pending_drops += 1;
                }
            }
            Rec::Unknown => {}
            Rec::Row => {
                if meta.is_none() {
                    // a row with no header above it means the head of the
                    // file is gone; the no-header refusal below says so
                    pending_drops += 1;
                    continue;
                }
                if !need_deps {
                    continue;
                }
                let Some(t) = t_id else {
                    bad_rows += 1;
                    continue;
                };
                rows.t.push(t);
                rows.j.push(j_flag);
                rows.deps.extend_from_slice(&dbuf);
                rows.deps.extend_from_slice(&obuf);
                let Ok(off) = u32::try_from(rows.deps.len()) else {
                    anyhow::bail!(
                        "{path}: more than {} dependency edges — the CSR uses u32 offsets; \
                         this is far past any real build and almost certainly a corrupt file",
                        u32::MAX
                    );
                };
                rows.off.push(off);
            }
        }
    }

    Ok(GraphRows {
        it,
        rows,
        meta,
        dropped_interior,
        pending_drops,
        bad_rows,
    })
}

pub fn load_graph(path: &str) -> anyhow::Result<Graph> {
    load_graph_opts(path, &GraphLoadOpts::default())
}

#[derive(Debug, Clone, Copy)]
pub struct GraphLoadOpts {
    /// Refuse inputs larger than this many bytes; 0 disables the cap.
    pub max_input_bytes: u64,
    /// `false` skips materialising rows/deps entirely; row counts and
    /// the header/format refusals still apply, but the refusals that
    /// need the dep lists (undefined references, cycles) are skipped
    pub need_deps: bool,
    /// suppress the stderr dropped-line notice
    pub quiet: bool,
    /// load a cyclic graph instead of refusing it; the walks then drop
    /// and count the edges that close a cycle, and every result built
    /// on the graph must be reported as partial with no bound
    pub repair_cycles: bool,
}

impl Default for GraphLoadOpts {
    fn default() -> Self {
        GraphLoadOpts {
            max_input_bytes: crate::parse::DEFAULT_MAX_INPUT_BYTES,
            need_deps: true,
            quiet: false,
            repair_cycles: false,
        }
    }
}

pub fn load_graph_opts(path: &str, opts: &GraphLoadOpts) -> anyhow::Result<Graph> {
    // The ordinary loader specializes the check to a statically empty body.
    load_graph_checked(path, *opts, || Ok(()))
}

/// A refused trace can cancel its unused speculative graph. Unit fixtures
/// without a shared viewer allocation use the standalone hardware budget.
#[cfg(test)]
pub(crate) fn load_graph_cancellable(path: &str, cancel: &AtomicBool) -> anyhow::Result<Graph> {
    load_graph_cancellable_with_workers_and_limit(
        path,
        cancel,
        crate::parallelism::graph_workers(),
        crate::parse::DEFAULT_MAX_INPUT_BYTES,
    )
}

/// Preserve the viewer request's input cap through both speculative and serial
/// reads. Dependency loading and cycle refusal remain mandatory for the GUI.
pub(crate) fn load_graph_cancellable_with_workers_and_limit(
    path: &str,
    cancel: &AtomicBool,
    workers: usize,
    max_input_bytes: u64,
) -> anyhow::Result<Graph> {
    // Hold speculative notices until the selected trace admits this load.
    let opts = GraphLoadOpts {
        max_input_bytes,
        quiet: true,
        ..Default::default()
    };
    load_graph_with_workers(
        path,
        opts,
        || {
            anyhow::ensure!(!cancel.load(Ordering::Relaxed), "graph preload cancelled");
            Ok(())
        },
        workers,
    )
}

fn load_graph_checked<F: Fn() -> anyhow::Result<()>>(
    path: &str,
    opts: GraphLoadOpts,
    check: F,
) -> anyhow::Result<Graph> {
    load_graph_with_workers(path, opts, check, crate::parallelism::graph_workers())
}

fn load_graph_with_workers<F: Fn() -> anyhow::Result<()>>(
    path: &str,
    opts: GraphLoadOpts,
    check: F,
    workers: usize,
) -> anyhow::Result<Graph> {
    let GraphLoadOpts {
        need_deps,
        quiet,
        max_input_bytes,
        ..
    } = opts;
    let t_start = Instant::now();
    let f = std::fs::File::open(path).with_context(|| path.to_owned())?;
    let len = f.metadata().with_context(|| format!("stat {path}"))?.len();
    if max_input_bytes > 0 && len > max_input_bytes {
        anyhow::bail!(
            "{path} is {len} bytes, over the {max_input_bytes} byte input cap; pass \
             --max-input-bytes=N (0 = uncapped) if this is really the file you mean"
        );
    }
    let mut rd = std::io::BufReader::with_capacity(1 << 20, f);

    let GraphRows {
        mut it,
        mut rows,
        meta,
        dropped_interior,
        pending_drops,
        bad_rows,
    } = if workers == 0 {
        scan_rows_serial(&mut rd, path, need_deps, &check)?
    } else {
        parallel::scan(
            &mut rd,
            path,
            need_deps,
            parallel::Config::new(workers),
            &check,
        )?
    };

    // Both scanners must fit the shared ID space before any ID is used.
    check_target_count(path, it.targets.len())?;
    let Some(meta) = meta else {
        anyhow::bail!("{path}: no graph header found — not a graph capture, or its head was lost");
    };

    let torn_tail = pending_drops;
    let dropped = dropped_interior + torn_tail;
    if !quiet {
        dropped_line_notice(path, dropped, dropped_interior);
    }

    let mut nt = it.targets.len();
    let mut is_key = vec![false; nt];
    let mut is_join = vec![false; nt];
    for r in 0..rows.len() {
        if r & 4095 == 0 {
            check()?;
        }
        let t = rows.t[r] as usize;
        is_key[t] = true;
        // |= so a repeated target's join marking cannot depend on which
        // of its rows came last in the file
        is_join[t] |= rows.j[r];
    }

    // undefined-reference accounting (distinct names), per the corrupt-
    // vs-torn contract in the module doc
    let mut undefined_joins = 0u64;
    let mut undefined_deps = 0u64;
    let mut undefined_samples: Vec<String> = Vec::new();
    let mut referenced = vec![false; nt];
    for (i, &d) in rows.deps.iter().enumerate() {
        if i & 4095 == 0 {
            check()?;
        }
        referenced[d as usize] = true;
    }
    for t in 0..nt {
        if t & 4095 == 0 {
            check()?;
        }
        if referenced[t] && !is_key[t] {
            if it.targets[t].starts_with("tset:") {
                undefined_joins += 1;
            } else {
                undefined_deps += 1;
            }
            if undefined_samples.len() < 5 {
                undefined_samples.push(it.targets[t].to_string());
            }
        }
    }
    // an interior malformed line is not evidence of a killed build, so
    // it does not relax this refusal; only a torn tail does
    if (undefined_joins > 0 || undefined_deps > 0) && torn_tail == 0 && need_deps {
        anyhow::bail!(
            "{path}: {undefined_joins} tset join(s) and {undefined_deps} dep target(s) are \
             referenced but never defined, and the file has no torn tail: the file is corrupt \
             or was written by a mismatched importer — refusing to emit a silently-holed graph \
             (first: {})",
            undefined_samples.join(", ")
        );
    }

    // Scanner scratch may have interned names from ignored prefixes or an
    // overwritten JSON key. Only accepted target/dependency positions define
    // the graph's name universe. The ordinary path reuses the liveness marks
    // already needed above and does not rebuild either names or edges.
    if (0..nt).any(|i| !is_key[i] && !referenced[i]) {
        let mut remap = vec![u32::MAX; nt];
        let mut next = 0u32;
        for i in 0..nt {
            if is_key[i] || referenced[i] {
                remap[i] = next;
                next += 1;
            }
        }
        let mut i = 0;
        it.targets.retain(|_| {
            let keep = remap[i] != u32::MAX;
            i += 1;
            keep
        });
        it.index.retain(|_, id| {
            let next = remap[*id as usize];
            *id = next;
            next != u32::MAX
        });
        for id in rows.t.iter_mut().chain(rows.deps.iter_mut()) {
            *id = remap[*id as usize];
        }
        let mut i = 0;
        is_key.retain(|_| {
            let keep = remap[i] != u32::MAX;
            i += 1;
            keep
        });
        let mut i = 0;
        is_join.retain(|_| {
            let keep = remap[i] != u32::MAX;
            i += 1;
            keep
        });
        nt = it.targets.len();
    }
    drop(referenced);
    // dep map CSR: repeated `t` rows concatenate in file order
    let mut pred_off = vec![0u32; nt + 1];
    for r in 0..rows.len() {
        if r & 4095 == 0 {
            check()?;
        }
        let t = rows.t[r] as usize;
        pred_off[t + 1] += rows.off[r + 1] - rows.off[r];
    }
    for i in 0..nt {
        if i & 4095 == 0 {
            check()?;
        }
        pred_off[i + 1] += pred_off[i];
    }
    let mut preds = vec![0u32; *pred_off.last().unwrap_or(&0) as usize];
    {
        let mut cur: Vec<u32> = pred_off[..nt].to_vec();
        for r in 0..rows.len() {
            if r & 4095 == 0 {
                check()?;
            }
            let t = rows.t[r] as usize;
            let src = &rows.deps[rows.off[r] as usize..rows.off[r + 1] as usize];
            let at = cur[t] as usize;
            preds[at..at + src.len()].copy_from_slice(src);
            cur[t] += src.len() as u32;
        }
    }
    let n_rows = rows.len();
    #[cfg(not(test))]
    drop(rows);
    #[cfg(test)]
    let (succ_off, succs) = {
        let mut succ_off = vec![0u32; nt + 1];
        for (i, &d) in preds.iter().enumerate() {
            if i & 4095 == 0 {
                check()?;
            }
            succ_off[d as usize + 1] += 1;
        }
        for i in 0..nt {
            if i & 4095 == 0 {
                check()?;
            }
            succ_off[i + 1] += succ_off[i];
        }
        let mut succs = vec![0u32; preds.len()];
        {
            let mut cur: Vec<u32> = succ_off[..nt].to_vec();
            for t in 0..nt {
                if t & 4095 == 0 {
                    check()?;
                }
                for &d in &preds[pred_off[t] as usize..pred_off[t + 1] as usize] {
                    succs[cur[d as usize] as usize] = t as u32;
                    cur[d as usize] += 1;
                }
            }
        }
        (succ_off, succs)
    };

    let (targets_in_cycles, cycle_samples) = find_cycles(&pred_off, &preds, &it.targets, &check)?;
    if targets_in_cycles > 0 && need_deps && !opts.repair_cycles {
        anyhow::bail!(
            "{path}: {targets_in_cycles} target(s) lie on a dependency cycle; a build graph is \
             never cyclic, so the file is corrupt or two different actions share one name \
             (first: {}); rerun with --json, --stats or --summary plus --repair-cycles to \
             analyse anyway with the cycle edges dropped and every result marked partial; \
             the viewer window cannot repair cycles",
            cycle_samples.join(", ")
        );
    }

    let n_edges = preds.len();
    Ok(Graph {
        path: path.to_owned(),
        targets: it.targets,
        index: it.index,
        is_key,
        is_join,
        pred_off,
        preds,
        #[cfg(test)]
        succ_off,
        #[cfg(test)]
        succs,
        n_edges,
        n_rows,
        #[cfg(test)]
        rows,
        meta,
        dropped_lines: dropped,
        dropped_interior,
        bad_rows,
        undefined_joins,
        undefined_deps,
        undefined_samples,
        targets_in_cycles,
        cycle_samples,
        load_ms: t_start.elapsed().as_millis(),
    })
}

fn check_target_count(path: &str, n: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        n < u32::MAX as usize,
        "{path}: more than {} distinct target names — target ids are u32; \
         this is far past any real build and almost certainly a corrupt file",
        u32::MAX - 1
    );
    Ok(())
}

pub(crate) fn dropped_line_notice(path: &str, dropped: u64, dropped_interior: u64) {
    if dropped > 0 {
        let torn_tail = dropped - dropped_interior;
        eprintln!(
            "{path}: skipped {dropped} line(s) that are not JSON ({dropped_interior} inside the \
             file, {torn_tail} in a torn tail)"
        );
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata as fixture;

    fn tmp(name: &str, body: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("retrace-graph-{}-{}", std::process::id(), name));
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn names(g: &Graph, ids: &[u32]) -> Vec<String> {
        ids.iter()
            .map(|&i| g.targets[i as usize].to_string())
            .collect()
    }

    #[test]
    fn input_cap_refuses_upfront_and_names_the_flag() {
        let path = fixture("smoke-graph.jsonl");
        let bytes = std::fs::metadata(&path)
            .expect("should stat smoke graph")
            .len();
        for workers in [0, 2] {
            let error = load_graph_with_workers(
                &path,
                GraphLoadOpts {
                    max_input_bytes: 10,
                    ..QUIET
                },
                || Ok(()),
                workers,
            )
            .expect_err("should refuse graph larger than its cap");
            assert!(format!("{error:#}").contains("--max-input-bytes"));
            for max_input_bytes in [0, bytes] {
                let graph = load_graph_with_workers(
                    &path,
                    GraphLoadOpts {
                        max_input_bytes,
                        ..QUIET
                    },
                    || Ok(()),
                    workers,
                )
                .expect("should load an uncapped or exactly fitting graph");
                assert_eq!(graph.n_rows, 4);
            }
        }
    }

    #[test]
    fn target_count_reserves_the_missing_id_sentinel() {
        check_target_count("test", u32::MAX as usize - 1)
            .expect("should accept the last supported target count");
        let error = check_target_count("test", u32::MAX as usize)
            .expect_err("should reserve the missing-target sentinel");
        assert!(error.to_string().contains("target ids are u32"));
    }

    #[test]
    fn graph_names_share_allocations_with_the_lookup_index() {
        let path = fixture("smoke-graph.jsonl");
        for workers in [0, 2] {
            let graph = load_graph_with_workers(&path, QUIET, || Ok(()), workers)
                .expect("should load shared graph names");
            for name in &graph.targets {
                let (key, &id) = graph
                    .index
                    .get_key_value(name.as_ref())
                    .expect("should find every interned name in the index");
                assert!(Arc::ptr_eq(name, key));
                assert!(Arc::ptr_eq(name, &graph.targets[id as usize]));
            }
        }
    }

    #[test]
    fn cancellable_graph_loader_preserves_rows_indexes_and_refusals() {
        let path = crate::temp_path("cancellable-graph.json");
        let p = path.to_str().unwrap();
        let cases = [
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"b\"]}\n{\"t\":\"b\",\"d\":[]}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"ghost\"]}\n{\"t\":",
            "{\"graph\":{\"enc\":2}}\n{broken}\n{\"t\":\"a\",\"d\":[]}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"a\"]}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"ghost\"]}\n",
            "{\"graph\":{\"enc\":2}}\n{\"graph\":{\"enc\":2}}\n",
        ];
        for body in cases {
            std::fs::write(&path, body).unwrap();
            let ordinary = load_graph_opts(p, &QUIET);
            let cancellable = load_graph_cancellable(p, &AtomicBool::new(false));
            match (ordinary, cancellable) {
                (Ok(mut a), Ok(mut b)) => {
                    a.load_ms = 0;
                    b.load_ms = 0;
                    assert_eq!(format!("{a:?}"), format!("{b:?}"));
                }
                (Err(a), Err(b)) => assert_eq!(format!("{a:#}"), format!("{b:#}")),
                pair => panic!("loader parity failed: {pair:?}"),
            }
        }
        let error = load_graph_cancellable(p, &AtomicBool::new(true)).unwrap_err();
        assert!(error.to_string().contains("graph preload cancelled"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cancellation_is_checked_inside_a_large_cycle_walk() {
        let n = 8192u32;
        let off: Vec<_> = (0..=n).collect();
        let preds: Vec<_> = (0..n).map(|i| (i + 1) % n).collect();
        let names: Vec<Arc<str>> = vec![Arc::from(""); n as usize];
        let checks = std::cell::Cell::new(0usize);
        let result = find_cycles(&off, &preds, &names, &|| {
            checks.set(checks.get() + 1);
            anyhow::ensure!(checks.get() < 2, "test cancellation");
            Ok(())
        });
        assert_eq!(result.unwrap_err().to_string(), "test cancellation");
        assert_eq!(checks.get(), 2);
    }

    #[test]
    fn smoke_graph_loads_with_join_rows() {
        let p = fixture("smoke-graph.jsonl");
        let g = load_graph(&p).unwrap();
        assert_eq!(g.meta.kind, "full");
        assert_eq!(g.meta.enc, Some(2));
        assert_eq!(g.meta.run.as_deref(), Some("aaaa-bbbb"));
        assert_eq!(g.rows.len(), 4);
        assert_eq!(g.n_edges, 4);
        assert_eq!(g.dropped_lines, 0);
        assert_eq!(g.undefined_joins + g.undefined_deps, 0);
        let cov = g.meta.coverage.clone().unwrap();
        assert_eq!(cov.nodes, Some(4));
        assert_eq!(cov.edges, Some(4));
        assert_eq!(cov.dangling_tsets, Some(0));

        let w = g.id("w (genrule)").unwrap();
        let y = g.id("y (rustc emit)").unwrap();
        let ts = g.id("tset:0a1b2c3d4e5f").unwrap();
        assert_eq!(
            names(&g, g.merged_deps(w)),
            ["y (rustc emit)", "tset:0a1b2c3d4e5f"]
        );
        assert!(g.is_join[ts as usize]);
        assert!(!g.is_join[w as usize]);
        assert!(g.merged_has(ts), "join rows are defined targets");
        // transpose: who consumes y? (id order: tset row precedes w)
        assert_eq!(
            names(&g, g.merged_succs(y)),
            ["tset:0a1b2c3d4e5f", "w (genrule)"]
        );
    }

    #[test]
    fn header_row_counts_and_capture_state_are_read_and_absent_on_older_files() {
        let p = fixture("smoke-graph.jsonl");
        let cov = load_graph(&p).unwrap().meta.coverage.unwrap();
        assert_eq!((cov.action_rows, cov.join_rows), (None, None));
        assert_eq!(cov.dep_capture, None);
        assert_eq!(cov.nodes, Some(4));
        let p = tmp(
            "covstate.json",
            "{\"graph\":{\"enc\":2,\"coverage\":{\"nodes\":1,\"edges\":0,\"action_rows\":1,\
             \"join_rows\":0,\"dep_capture\":{\"state\":\"not-enabled\"}}}}\n\
             {\"t\":\"a\",\"d\":[]}\n",
        );
        let cov = load_graph(&p).unwrap().meta.coverage.unwrap();
        assert_eq!((cov.action_rows, cov.join_rows), (Some(1), Some(0)));
        assert_eq!(cov.dep_capture, Some(CaptureState::NotEnabled));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn scoped_graph_headers_fail_closed_without_matching_encoding_and_metadata() {
        for (enc, kind, scope, accepted) in [
            (
                3,
                "current_action_ancestors",
                "current_action_ancestors",
                true,
            ),
            (2, "full", "full", true),
            (
                2,
                "current_action_ancestors",
                "current_action_ancestors",
                false,
            ),
            (3, "full", "current_action_ancestors", false),
            (3, "current_action_ancestors", "full", false),
            (3, "current_action_ancestors", "future_scope", false),
            (2, "full", "current_action_ancestors", false),
        ] {
            let header = serde_json::json!({"graph": {"enc": enc, "kind": kind,
                "coverage": {"dep_capture": {"state": "started", "scope": scope}}}});
            let mut meta = None;
            let result =
                parse_graph_header("fixture", &serde_json::to_vec(&header).unwrap(), &mut meta);
            assert_eq!(result.is_ok(), accepted, "{header}: {result:?}");
        }
        let mut meta = None;
        assert!(
            parse_graph_header(
                "fixture",
                br#"{"graph":{"enc":3,"kind":"current_action_ancestors"}}"#,
                &mut meta
            )
            .is_err()
        );
    }

    #[test]
    fn header_capture_stamp_is_read_and_absent_on_older_files() {
        let p = fixture("smoke-graph.jsonl");
        assert_eq!(load_graph(&p).unwrap().meta.capture, None);
        let p = tmp(
            "stamped.json",
            "{\"graph\":{\"enc\":2,\"capture\":\"abc\"}}\n{\"t\":\"a\",\"d\":[]}\n",
        );
        assert_eq!(load_graph(&p).unwrap().meta.capture.as_deref(), Some("abc"));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn check_pair_accepts_matching_and_unstamped_pairs_and_refuses_the_rest() {
        let run = |id: &str, capture: Option<&str>| RunInfo {
            run: id.to_owned(),
            capture: capture.map(|c| c.to_owned()),
            ..Default::default()
        };
        let graph = |id: Option<&str>, capture: Option<&str>| GraphMeta {
            run: id.map(|r| r.to_owned()),
            capture: capture.map(|c| c.to_owned()),
            ..Default::default()
        };
        let msg = |r: anyhow::Result<()>| format!("{:#}", r.unwrap_err());
        check_pair(&run("r", Some("abc")), &graph(Some("r"), Some("abc"))).unwrap();
        check_pair(&run("r", None), &graph(Some("r"), None)).unwrap();
        check_pair(&run("r", Some("abc")), &graph(Some(""), Some("abc"))).unwrap();
        assert!(
            msg(check_pair(
                &run("r", Some("abc")),
                &graph(Some("r"), Some("def"))
            ))
            .contains("different capture")
        );
        assert!(
            msg(check_pair(&run("r", Some("abc")), &graph(Some("r"), None)))
                .contains("different ingests")
        );
        assert!(
            msg(check_pair(&run("r", None), &graph(Some("r"), Some("abc"))))
                .contains("different ingests")
        );
        // the review's run-mismatch counterexample
        let m = msg(check_pair(
            &run("trace-run", None),
            &graph(Some("another-run"), None),
        ));
        assert!(m.contains("another-run") && m.contains("trace-run"), "{m}");
    }

    #[test]
    fn missing_enc_is_refused_as_flattened() {
        let p = tmp(
            "noenc.json",
            "{\"graph\":{\"dir\":\"/r\",\"kind\":\"full\"}}\n",
        );
        let err = format!("{:#}", load_graph(&p).unwrap_err());
        assert!(
            err.contains("enc <absent>") && err.contains("flattened"),
            "{err}"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cp_chain_export_is_refused_with_capture_hint() {
        let p = tmp(
            "cpchain.json",
            "{\"graph\":{\"dir\":\"/r\",\"note\":\"cp-chain-only: BuildGraphInfo\"}}\n",
        );
        let err = format!("{:#}", load_graph(&p).unwrap_err());
        assert!(err.contains("--log-action-graph"), "{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn second_header_is_refused() {
        let p = tmp(
            "twohdr.json",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[]}\n{\"graph\":{\"enc\":2}}\n",
        );
        let err = format!("{:#}", load_graph(&p).unwrap_err());
        assert!(err.contains("second graph header"), "{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn no_header_is_refused() {
        let p = tmp("nohdr.json", "{\"t\":\"a\",\"d\":[]}\n");
        let err = format!("{:#}", load_graph(&p).unwrap_err());
        assert!(err.contains("no graph header"), "{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn undefined_refs_with_clean_file_are_corruption() {
        let p = tmp(
            "corrupt.json",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"tset:dead\",\"ghost\"]}\n",
        );
        let err = format!("{:#}", load_graph(&p).unwrap_err());
        assert!(
            err.contains("1 tset join(s)") && err.contains("1 dep target(s)"),
            "{err}"
        );
        assert!(
            err.contains("tset:dead") && err.contains("no torn tail"),
            "{err}"
        );
        std::fs::remove_file(&p).ok();
    }

    const QUIET: GraphLoadOpts = GraphLoadOpts {
        max_input_bytes: crate::parse::DEFAULT_MAX_INPUT_BYTES,
        need_deps: true,
        quiet: true,
        repair_cycles: false,
    };

    /// `(t, deps, join)` as read from one accepted row.
    type RowRead = (Option<String>, Vec<String>, bool);

    /// What the loader accepts and reads from one row, by the standard
    /// parser: `None` = rejected; `Some(None)` = a header or footer
    /// record; `Some(Some(read))` = a row or unknown record.
    fn oracle(b: &[u8]) -> Option<Option<RowRead>> {
        let v: serde_json::Value = serde_json::from_slice(b).ok()?;
        let o = v.as_object()?;
        if o.contains_key("graph") || o.contains_key("graph_end") {
            return Some(None);
        }
        let t = match o.get("t") {
            None => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => return None,
        };
        let mut deps = Vec::new();
        for k in ["d", "o"] {
            match o.get(k) {
                None => {}
                Some(serde_json::Value::Array(a)) => {
                    for x in a {
                        deps.push(x.as_str()?.to_owned());
                    }
                }
                Some(_) => return None,
            }
        }
        let j = matches!(o.get("j"), Some(v) if *v == serde_json::json!(1) || *v == serde_json::json!(true));
        Some(Some((t, deps, j)))
    }

    /// The scanner agrees with serde_json on every row of a small corpus
    /// and on every single-byte mutation of it, and a rejected row leaves
    /// the interner as it found it.
    #[test]
    fn scanner_agrees_with_serde_json_on_mutated_rows() {
        let bases: &[&[u8]] = &[
            br#"{"t":"w (genrule)","d":["y (rustc emit)","tset:0a1b2c3d4e5f"]}"#,
            br#"{"t":"tset:0a1b2c3d4e5f","d":["y (rustc emit)"],"j":1}"#,
            br#"{"t":"a\"b","d":["c\\d","\u00e9"]}"#,
            br#"{"t":"x","d":[],"j":true}"#,
            br#"{"t":"x","d":["p"],"o":["q","r"]}"#,
            br#"{"t":"x","d":[],"x":{"y":[1,2.5e3,{"z":null}],"s":"\ud83d\ude00"}}"#,
            br#"{}"#,
            br#"{ "t" : "sp" , "d" : [ "a" , "b" ] }"#,
            br#"{"graph":{"enc":2,"run":"r"}}"#,
        ];
        let targeted: &[&[u8]] = &[
            br#"{"t":"a" "d":["b",]}"#,
            br#"{"t":"a","d":[,"b"]}"#,
            br#"{"t":"a","d":["a",,"b"]}"#,
            br#"{"t":"a","d":["b",]}"#,
            br#"{"t":"a",}"#,
            br#"{,}"#,
            br#"{"t":"a","j":01}"#,
            br#"{"t":"a","j":-}"#,
            br#"{"t":"a","j":1.}"#,
            br#"{"t":"a","j":1e5}"#,
            br#"{"t":"a","j":-0}"#,
            br#"{"t":"a","j":truex}"#,
            br#"{"t":"a","j":"1"}"#,
            br#"{"t":"a","j":null}"#,
            br#"{"t":"a","j":[1]}"#,
            br#"{"t":"a","j":{}}"#,
            br#"{"t":"\ud800"}"#,
            br#"{"t":"\udc00x"}"#,
            br#"{"t":"\ud800\u0041"}"#,
            br#"{"t":"\q"}"#,
            br#"{"t":"a","x":"\ud800"}"#,
            b"{\"t\":\"a\tb\"}",
            br#"{"t":1}"#,
            br#"{"t":"a","d":[1]}"#,
            br#"{"t":"a","d":{}}"#,
            br#"{"t":"a","d":"b"}"#,
            br#"{"t":"a","d":[]} x"#,
            br#"["t"]"#,
            br#"{"t":1,"graph":{}}"#,
            br#"{"d":[1],"graph":{}}"#,
            br#"{"d":"x","graph":{}}"#,
            br#"{"o":[1],"graph":{}}"#,
            br#"{"o":"x","graph":{}}"#,
            br#"{"t":"a","graph_end":1}"#,
            br#"{"t":1,"graph_end":1}"#,
        ];
        let inserts: &[u8] = b",\":[]{} \\x0\x01";
        let mut cases: Vec<Vec<u8>> = Vec::new();
        for &b in bases.iter().chain(targeted) {
            cases.push(b.to_vec());
            for cut in 0..b.len() {
                cases.push(b[..cut].to_vec());
                let mut del = b.to_vec();
                del.remove(cut);
                cases.push(del);
                let mut dup = b.to_vec();
                dup.insert(cut, b[cut]);
                cases.push(dup);
                for &c in inserts {
                    let mut ins = b.to_vec();
                    ins.insert(cut, c);
                    cases.push(ins);
                }
            }
        }
        assert!(cases.len() > 5000, "{}", cases.len());

        let mut it = Interner {
            targets: Vec::new(),
            index: FxHashMap::default(),
        };
        it.get_or("seed");
        let mut n_accepted = 0;
        for b in &cases {
            let mark = it.mark();
            let (mut dbuf, mut obuf) = (Vec::new(), Vec::new());
            let mut t_id = None;
            let mut j_flag = false;
            let got = scan_object(
                b,
                &mut it,
                &mut dbuf,
                &mut obuf,
                &mut t_id,
                &mut j_flag,
                true,
            );
            let want = oracle(b);
            let shown = String::from_utf8_lossy(b);
            match (got, want) {
                (Err(_), None) => {
                    it.rollback(mark);
                    assert_eq!(it.mark(), mark);
                    assert_eq!(it.index.len(), mark, "{shown:?}");
                }
                (Ok(Rec::Header | Rec::Unknown), Some(None)) => {}
                (Ok(Rec::Row | Rec::Unknown), Some(Some((t, deps, j)))) => {
                    n_accepted += 1;
                    assert_eq!(
                        t_id.map(|i| it.targets[i as usize].to_string()),
                        t,
                        "{shown:?}"
                    );
                    let got_deps: Vec<String> = dbuf
                        .iter()
                        .chain(&obuf)
                        .map(|&i| it.targets[i as usize].to_string())
                        .collect();
                    assert_eq!(got_deps, deps, "{shown:?}");
                    assert_eq!(j_flag, j, "{shown:?}");
                }
                (got, want) => panic!(
                    "{shown:?}: scanner {} vs serde {}",
                    match got {
                        Ok(Rec::Header) => "header",
                        Ok(Rec::Unknown) => "unknown",
                        Ok(Rec::Row) => "row",
                        Err(RowErr::Malformed) => "malformed",
                        Err(RowErr::Shape) => "shape",
                    },
                    match want {
                        None => "reject".to_owned(),
                        Some(w) => format!("{w:?}"),
                    }
                ),
            }
        }
        assert!(n_accepted > 100, "{n_accepted}");
    }

    #[test]
    fn malformed_interior_row_is_counted_and_does_not_unlock_torn_tolerance() {
        let body =
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\" \"d\":[\"b\",]}\n{\"t\":\"b\",\"d\":[]}\n";
        let p = tmp("interior.json", body);
        let g = load_graph_opts(&p, &QUIET).unwrap();
        assert_eq!(g.dropped_lines, 1);
        assert_eq!(g.dropped_interior, 1);
        assert_eq!(g.bad_rows, 0);
        assert_eq!(g.rows.len(), 1);
        assert_eq!(g.n_edges, 0);
        assert!(g.id("a").is_none(), "a rejected row interns nothing");
        std::fs::remove_file(&p).ok();

        let p = tmp(
            "interior2.json",
            &format!("{body}{{\"t\":\"c\",\"d\":[\"tset:dead\"]}}\n"),
        );
        let err = format!("{:#}", load_graph_opts(&p, &QUIET).unwrap_err());
        assert!(
            err.contains("never defined") && err.contains("no torn tail"),
            "{err}"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn wrong_shape_but_valid_json_is_a_bad_row_not_a_torn_line() {
        let p = tmp(
            "shape.json",
            "{\"graph\":{\"enc\":2}}\n{\"t\":1,\"d\":[]}\n{\"t\":\"a\",\"d\":[1]}\n\
             {\"t\":\"b\",\"d\":[]}\n",
        );
        let g = load_graph_opts(&p, &QUIET).unwrap();
        assert_eq!(g.bad_rows, 2);
        assert_eq!(g.dropped_lines, 0);
        assert_eq!(g.rows.len(), 1);
        assert!(g.id("a").is_none());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cyclic_graph_is_refused_by_default_and_counted_under_repair() {
        let repair = GraphLoadOpts {
            repair_cycles: true,
            ..QUIET
        };
        let two = ["{\"t\":\"a\",\"d\":[\"b\"]}", "{\"t\":\"b\",\"d\":[\"a\"]}"];
        let self_loop = ["{\"t\":\"a\",\"d\":[\"a\"]}"];
        let join = [
            "{\"t\":\"a\",\"d\":[\"tset:x\"]}",
            "{\"t\":\"tset:x\",\"d\":[\"b\"],\"j\":1}",
            "{\"t\":\"b\",\"d\":[\"a\"]}",
        ];
        // a consumer of the cycle is not on it
        let with_consumer = [
            "{\"t\":\"a\",\"d\":[\"b\"]}",
            "{\"t\":\"b\",\"d\":[\"a\"]}",
            "{\"t\":\"c\",\"d\":[\"a\"]}",
        ];
        for (name, rows, n) in [
            ("two", &two[..], 2),
            ("self", &self_loop[..], 1),
            ("join", &join[..], 3),
            ("consumer", &with_consumer[..], 2),
        ] {
            for reversed in [false, true] {
                let mut rows: Vec<&str> = rows.to_vec();
                if reversed {
                    rows.reverse();
                }
                let body = format!("{{\"graph\":{{\"enc\":2}}}}\n{}\n", rows.join("\n"));
                let p = tmp(&format!("cycle-{name}-{reversed}.json"), &body);
                let err = format!("{:#}", load_graph_opts(&p, &QUIET).unwrap_err());
                assert!(
                    err.contains("dependency cycle")
                        && err.contains("--repair-cycles")
                        && err.contains("--json"),
                    "{name}: {err}"
                );
                assert!(err.contains(" -> "), "{name}: {err}");
                assert!(
                    !err.contains("c -> a"),
                    "{name}: a consumer is not a cycle edge"
                );
                let g = load_graph_opts(&p, &repair).unwrap();
                assert_eq!(g.targets_in_cycles, n, "{name} reversed={reversed}");
                assert!(!g.cycle_samples.is_empty());
                assert!(g.cycle_samples.iter().all(|s| !s.starts_with("c ->")));
                std::fs::remove_file(&p).ok();
            }
        }
        // a clean graph is unchanged by the flag
        let p = fixture("smoke-graph.jsonl");
        let g = load_graph_opts(&p, &repair).unwrap();
        assert_eq!(g.targets_in_cycles, 0);
        assert!(g.cycle_samples.is_empty());
    }

    #[test]
    fn undefined_refs_with_torn_tail_degrade_to_counts() {
        let p = tmp(
            "torn.json",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"tset:dead\"]}\n{\"t\":\"tset:de",
        );
        let g = load_graph_opts(&p, &QUIET).unwrap();
        assert_eq!(g.dropped_lines, 1);
        assert_eq!(g.dropped_interior, 0, "the drop is the torn tail");
        assert_eq!(g.undefined_joins, 1);
        assert_eq!(g.undefined_samples, ["tset:dead".to_owned()]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn repeated_target_rows_concatenate() {
        let p = tmp(
            "dup.json",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"x\"]}\n{\"t\":\"a\",\"d\":[\"y\"]}\n\
             {\"t\":\"x\",\"d\":[]}\n{\"t\":\"y\",\"d\":[]}\n",
        );
        let g = load_graph(&p).unwrap();
        let a = g.id("a").unwrap();
        assert_eq!(names(&g, g.merged_deps(a)), ["x", "y"]);
        assert_eq!(g.n_edges, 2);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn escapes_and_unicode_targets() {
        let p = tmp(
            "esc.json",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\\\"b\",\"d\":[\"c\\\\d\",\"\\u00e9\"]}\n\
             {\"t\":\"c\\\\d\",\"d\":[]}\n{\"t\":\"\\u00e9\",\"d\":[]}\n",
        );
        let g = load_graph(&p).unwrap();
        let t = g.id("a\"b").unwrap();
        assert_eq!(names(&g, g.merged_deps(t)), ["c\\d", "\u{e9}"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn row_without_t_is_counted_not_fatal() {
        let p = tmp(
            "not.json",
            "{\"graph\":{\"enc\":2}}\n{\"d\":[\"x\"]}\n{\"t\":\"a\"}\n{\"t\":\"x\",\"d\":[]}\n",
        );
        let g = load_graph(&p).unwrap();
        assert_eq!(g.bad_rows, 1);
        assert_eq!(g.rows.len(), 2);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn need_deps_false_still_enforces_refusals() {
        let p = fixture("smoke-graph.jsonl");
        let shallow = GraphLoadOpts {
            need_deps: false,
            ..QUIET
        };
        let g = load_graph_opts(&p, &shallow).unwrap();
        assert!(g.rows.is_empty());
        assert_eq!(g.meta.enc, Some(2));
        let p = tmp("noenc2.json", "{\"graph\":{\"kind\":\"full\"}}\n");
        assert!(load_graph_opts(&p, &shallow).is_err());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn sibling_rule_replaces_every_trace_occurrence() {
        // the rule operates on names; existence is checked on disk, so
        // test against the real fixture pair. Compared as paths, not
        // strings: `with_file_name` re-emits the final separator as the
        // platform one, and RETRACE_TESTDATA arrives with `/` even on
        // Windows.
        use std::path::PathBuf;
        let t = fixture("smoke-trace.json");
        // the fixture sidecar is `.jsonl`, so this also proves the
        // extension-swap probe
        assert_eq!(
            sibling_graph_path(&t).map(PathBuf::from),
            Some(PathBuf::from(fixture("smoke-graph.jsonl")))
        );
        assert_eq!(sibling_graph_path("/tmp/nothing-here.json"), None);
    }
}
