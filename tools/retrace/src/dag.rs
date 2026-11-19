/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Stitched DAG: trace actions x declared graph -> schedulable DAG.
//!
//! The flat form of the proto's stitcher. The GNU-Make original grouped
//! events into processes by recipe serial and stitched `$(MAKE)`
//! recursion; buck2 is one daemon and the importer's synthetic serials
//! made it one flat process — so the process machinery is gone and this
//! is the degenerate path, kept semantically identical to it:
//!
//! - every recipe action becomes a weighted node, in trace order
//!   (ts ascending, file order breaking ties); a target built by
//!   several same-named actions chains them sequentially
//! - matching is byte-equality between the event name and the graph
//!   row's `"t"` — the join everything rests on
//! - an explicitly defined graph join with no trace action (a tset projection
//!   or recorded available/reused artifact structure) becomes a zero-weight
//!   pass-through: chains flow through it, it contributes nothing; a
//!   single-dep pass-through ALIASES to its dep (no node), which is
//!   how duplicate ids can appear in a consumer's pred list
//!   Unknown producer references are not manufactured as zero-time actions.
//! - the loader refuses a cyclic graph; under
//!   `GraphLoadOpts::repair_cycles` both walks drop each dependency that
//!   points back at a target still being walked and count it
//!   (`n_cycle_edges`), and every consumer must report such a result as
//!   partial with no bound
//! - node weights are nonnegative action durations, each used once. Every
//!   path or duration total is at most `Trace::busy`, bounded by the parser.
//!
//! Node ids are created dependencies-first, so they are a topological
//! order (`preds[i]` only holds ids `< i`).

use fxhash::FxHashMap;

use crate::graph::Graph;
use crate::types::F_CACHE_HIT;
use crate::types::F_NOT_RECIPE;
use crate::types::NO_STR;
use crate::types::Trace;

/// One recipe action, projected for analyses (`ts` is absolute µs —
/// the viewer's rebase is undone).
#[derive(Clone, Debug)]
pub struct Ev {
    pub name: String,
    pub ts: i64,
    pub has_cache: bool,
}

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct Dag {
    /// index into `Trace::acts` of the node's recipe action, `u32::MAX`
    /// for join/exit nodes.
    pub act_of_node: Vec<u32>,
    pub weight_us: Vec<i64>,
    pub pred_off: Vec<u32>,
    pub preds: Vec<u32>,
    pub succ_off: Vec<u32>,
    pub succs: Vec<u32>,
    pub t1: i64,
    pub cp: i64,
    /// longest path ending at each node, inclusive of its weight
    pub up: Vec<i64>,
    /// longest path starting at each node, inclusive of its weight
    pub down: Vec<i64>,
    /// nodes on a longest path, build order (deepest dependency first);
    /// join/exit nodes are omitted so every entry indexes a real action
    /// via `act_of_node`
    pub cp_path: Vec<u32>,
}

impl Dag {
    /// How much this node's duration can grow before the critical path
    /// (and so the build) gets longer. 0 = on the critical path.
    pub fn slack(&self, node: usize) -> i64 {
        let through = self.up[node] + self.down[node] - self.weight_us[node];
        (self.cp - through).max(0)
    }
}

pub struct DagBundle {
    pub dag: Dag,
    /// `node_ev[i]` = index into `events` of node i's originating trace
    /// event, `None` for join/exit nodes.
    pub node_ev: Vec<Option<usize>>,
    /// One `Ev` per recipe action, in trace order — the list `node_ev`
    /// indexes. Includes every run's events even when nodes were built
    /// for one selected run.
    pub events: Vec<Ev>,
}

/// Join-coverage counts for the honesty envelope: how many trace
/// actions found a graph node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DagStats {
    /// recipe events nodes were built from (after run selection)
    pub n_events: usize,
    /// of those, how many have a DEFINING graph row for their name
    pub n_matched: usize,
    /// of those, how many have a name the graph knows only as another
    /// row's dependency: the defining row is torn or was never written.
    /// They still get a node (the edge into them is real), but they are
    /// not covered.
    pub n_named_only: usize,
    /// recipe events run selection DROPPED because they carry no run
    /// id while several runs exist; zero when the selected run is the
    /// only one (then run-less events belong to it).  Surfaced so the
    /// stitch line cannot print a clean matched N/N over a silent drop.
    pub n_unattributed: usize,
    /// declared dependencies the walk dropped because they pointed back
    /// at a target still being walked; non-zero only on a graph loaded
    /// with `repair_cycles`. Which edges of a cycle are dropped follows
    /// the walk order, so the count is deterministic but not a property
    /// of the graph alone.
    pub n_cycle_edges: u64,
    /// a few of those, as `consumer -> dependency`
    pub cycle_samples: Vec<String>,
}

/// One step of the target-chain critical path (the CP walker's list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpEntry {
    /// index into the bundle's `events`
    pub ev: usize,
    pub us: i64,
}

/// The target-chain critical path: per-target chain costs walked over
/// declared deps. On a flat graph its `cp_us` equals `Dag::cp`; the
/// entry list is what the CP walker and bottleneck reports show.
#[derive(Debug)]
pub struct CpChain {
    pub cp_us: i64,
    pub entries: Vec<CpEntry>,
    /// as `DagStats::n_cycle_edges`; both walks visit the same roots
    /// over the same dep lists, so the two counts agree
    pub n_cycle_edges: u64,
    pub cycle_samples: Vec<String>,
}

/// One representative path for every surface. Ties keep the first declared
/// predecessor, then the lowest node ID, as in the stitched DAG.
pub fn chain_from_bundle(bundle: &DagBundle, stats: &DagStats) -> CpChain {
    CpChain {
        cp_us: bundle.dag.cp,
        entries: bundle
            .dag
            .cp_path
            .iter()
            .filter_map(|&node| {
                bundle.node_ev[node as usize].map(|ev| CpEntry {
                    ev,
                    us: bundle.dag.weight_us[node as usize],
                })
            })
            .collect(),
        n_cycle_edges: stats.n_cycle_edges,
        cycle_samples: stats.cycle_samples.clone(),
    }
}

/// Traversal state of a target during a walk.
const ACTIVE: u8 = 1;
const DONE: u8 = 2;

/// Records one dependency the walk cannot follow because `d` is still
/// being walked: an edge that closes a cycle.
fn note_cycle_edge(n: &mut u64, samples: &mut Vec<String>, t: &str, d: &str) {
    *n += 1;
    if samples.len() < 5 {
        samples.push(format!("{t} -> {d}"));
    }
}

// ------------------------------------------------------------------ prep

struct Prep<'a> {
    /// index into `Trace::acts`, all recipe events in (ts, seq) order
    ord: Vec<u32>,
    events: Vec<Ev>,
    /// positions in `ord` that nodes are built from (run-selected)
    sel: Vec<u32>,
    /// event position -> target key (graph id, or a locally allocated
    /// id past `g.n_targets()` for names the graph never mentions)
    ekey: Vec<u32>,
    /// target key -> name (graph targets first, then event-only names)
    keyname: Vec<&'a str>,
    dur: Vec<i64>,
    n_matched: usize,
    n_named_only: usize,
    n_unattributed: usize,
}

/// `run_sel`: the run id nodes should be scoped to (from
/// `parse::select_run`). `None` is only valid for a single-run trace —
/// several runs with no selection returns `None` rather than guessing
/// (docs/contract.md).
fn prepare<'a>(tr: &'a Trace, g: &'a Graph, run_sel: Option<&str>) -> Option<Prep<'a>> {
    crate::graph::check_trace_pair(tr, run_sel, &g.meta).ok()?;
    let selected_run = crate::parse::select_trace_run(tr, run_sel).ok()?;
    let run_sel = selected_run.as_deref();
    let mut ord: Vec<u32> = (0..tr.acts.len() as u32)
        .filter(|&i| tr.acts[i as usize].flags & F_NOT_RECIPE == 0)
        .collect();
    ord.sort_unstable_by_key(|&i| (tr.acts[i as usize].ts, tr.acts[i as usize].seq));
    if ord.is_empty() {
        return None;
    }
    let events = build_events(tr, &ord);
    let dur: Vec<i64> = ord.iter().map(|&a| tr.acts[a as usize].dur).collect();

    // A recipe event with no run id can only be attributed when the
    // selected run is the sole run present; otherwise it is dropped
    // AND counted — a shrinking denominator must never look clean.
    let mut n_unattributed = 0usize;
    let sel: Vec<u32> = (0..ord.len() as u32)
        .filter(|&i| {
            let action = &tr.acts[ord[i as usize] as usize];
            let selected = tr.action_in_run(action, run_sel);
            if !selected && action.run == NO_STR {
                n_unattributed += 1;
            }
            selected
        })
        .collect();
    if sel.is_empty() {
        return None;
    }

    let mut ekey: Vec<u32> = vec![u32::MAX; ord.len()];
    let mut keyname: Vec<&str> = g.targets.iter().map(|s| &**s).collect();
    let mut n_matched = 0usize;
    let mut n_named_only = 0usize;
    {
        let mut next = u32::try_from(g.n_targets()).ok()?;
        let mut extra: FxHashMap<&str, u32> = FxHashMap::default();
        for &i in &sel {
            let name = &tr.names[tr.acts[ord[i as usize] as usize].name as usize];
            ekey[i as usize] = match g.id(name) {
                // the interner holds dependency-only names too; only a
                // defining row covers the action
                Some(id) if g.merged_has(id) => {
                    n_matched += 1;
                    id
                }
                Some(id) => {
                    n_named_only += 1;
                    id
                }
                None => match extra.get(name.as_str()) {
                    Some(&id) => id,
                    None => {
                        // Reserve u32::MAX for the missing-key sentinel above.
                        if next == u32::MAX {
                            return None;
                        }
                        extra.insert(name.as_str(), next);
                        keyname.push(name.as_str());
                        next += 1;
                        next - 1
                    }
                },
            };
        }
    }
    Some(Prep {
        ord,
        events,
        sel,
        ekey,
        keyname,
        dur,
        n_matched,
        n_named_only,
        n_unattributed,
    })
}

fn build_events(tr: &Trace, ord: &[u32]) -> Vec<Ev> {
    ord.iter()
        .map(|&a| {
            let e = &tr.acts[a as usize];
            Ev {
                name: tr.names[e.name as usize].clone(),
                ts: e.ts + tr.t0_abs, // undo the viewer's rebase: absolute µs
                has_cache: e.flags & F_CACHE_HIT != 0,
            }
        })
        .collect()
}

// ------------------------------------------------------------------ build

#[derive(Default)]
struct Out {
    w: Vec<i64>,
    pred_off: Vec<u32>,
    preds: Vec<u32>,
    node_ev: Vec<Option<usize>>,
}

impl Out {
    #[inline]
    fn node(&mut self, weight: i64, ds: &[u32], e: Option<usize>) -> u32 {
        self.w.push(weight);
        self.preds.extend_from_slice(ds);
        self.pred_off.push(self.preds.len() as u32);
        self.node_ev.push(e);
        (self.w.len() - 1) as u32
    }
}

/// Static dispatch keeps the test-only sparse reference out of the production
/// edge walk. Production traversal uses dense tables over actual interned keys.
trait WalkTables {
    fn new(keys: usize) -> Self;
    fn state(&self, key: u32) -> u8;
    fn set_state(&mut self, key: u32, state: u8);
    fn completion(&self, key: u32) -> Option<u32>;
    fn set_completion(&mut self, key: u32, node: u32);
}

#[cfg(test)]
struct SparseWalk {
    comp: FxHashMap<u32, u32>,
    state: FxHashMap<u32, u8>,
}

#[cfg(test)]
impl WalkTables for SparseWalk {
    fn new(_keys: usize) -> Self {
        Self {
            comp: FxHashMap::default(),
            state: FxHashMap::default(),
        }
    }
    #[inline]
    fn state(&self, key: u32) -> u8 {
        self.state.get(&key).copied().unwrap_or(0)
    }
    #[inline]
    fn set_state(&mut self, key: u32, state: u8) {
        self.state.insert(key, state);
    }
    #[inline]
    fn completion(&self, key: u32) -> Option<u32> {
        self.comp.get(&key).copied()
    }
    #[inline]
    fn set_completion(&mut self, key: u32, node: u32) {
        self.comp.insert(key, node);
    }
}

struct DenseWalk {
    comp: Vec<u32>,
    state: Vec<u8>,
}

impl WalkTables for DenseWalk {
    fn new(keys: usize) -> Self {
        // Five requested bytes per actual interned key, including unvisited
        // targets. Never size from an externally supplied largest numeric ID.
        // This may cost MORE than sparse maps for a tiny incremental build.
        Self {
            comp: vec![u32::MAX; keys],
            state: vec![0; keys],
        }
    }
    #[inline]
    fn state(&self, key: u32) -> u8 {
        self.state.get(key as usize).copied().unwrap_or(0)
    }
    #[inline]
    fn set_state(&mut self, key: u32, state: u8) {
        self.state[key as usize] = state;
    }
    #[inline]
    fn completion(&self, key: u32) -> Option<u32> {
        self.comp
            .get(key as usize)
            .copied()
            .filter(|&node| node != u32::MAX)
    }
    #[inline]
    fn set_completion(&mut self, key: u32, node: u32) {
        self.comp[key as usize] = node;
    }
}

/// Stitch the selected trace actions through the recorded graph.
///
/// Returns `None` when pairing or run selection fails, no recipe actions
/// belong to the selected run, or trace plus graph names exceed the u32 ID
/// space. An unlabelled single-run trace is valid. To report selection or
/// pairing errors, call `parse::select_trace_run` and `graph::check_trace_pair`
/// first, as the CLI and viewer do.
pub fn build_full(tr: &Trace, g: &Graph, run_sel: Option<&str>) -> Option<(DagBundle, DagStats)> {
    build_full_with::<DenseWalk>(tr, g, run_sel)
}

/// Cancellable [`build_full`], with the same admission rules. An inadmissible
/// pair returns `Ok(None)`; cancellation returns an error.
pub fn build_full_cancellable(
    tr: &Trace,
    g: &Graph,
    run_sel: Option<&str>,
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<Option<(DagBundle, DagStats)>> {
    let cancelled = || cancel.load(std::sync::atomic::Ordering::Relaxed);
    anyhow::ensure!(!cancelled(), "graph analysis cancelled");
    let result = build_full_with_cancel::<DenseWalk>(tr, g, run_sel, &cancelled);
    anyhow::ensure!(!cancelled(), "graph analysis cancelled");
    Ok(result)
}

fn build_full_with<W: WalkTables>(
    tr: &Trace,
    g: &Graph,
    run_sel: Option<&str>,
) -> Option<(DagBundle, DagStats)> {
    build_full_with_cancel::<W>(tr, g, run_sel, &|| false)
}

fn build_full_with_cancel<W: WalkTables>(
    tr: &Trace,
    g: &Graph,
    run_sel: Option<&str>,
    cancelled: &dyn Fn() -> bool,
) -> Option<(DagBundle, DagStats)> {
    if cancelled() {
        return None;
    }
    let p = prepare(tr, g, run_sel)?;
    if cancelled() {
        return None;
    }
    let mut out = Out::default();
    out.pred_off.push(0);

    // built: target key -> its events (positions), in trace order.
    let mut built: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    let mut border: Vec<u32> = Vec::with_capacity(p.sel.len());
    for &e in &p.sel {
        if e.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let k = p.ekey[e as usize];
        match built.entry(k) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(vec![e]);
                border.push(k);
            }
            std::collections::hash_map::Entry::Occupied(mut o) => o.get_mut().push(e),
        }
    }

    let has = |t: u32| -> bool { g.merged_has(t) };
    let deps = |t: u32| -> &[u32] { g.merged_deps(t) };
    let name = |k: u32| -> &str { p.keyname.get(k as usize).copied().unwrap_or("") };

    let mut walk = W::new(p.keyname.len());
    let mut n_cycle_edges = 0u64;
    let mut cycle_samples: Vec<String> = Vec::new();
    let mut visited = 0usize;
    // Iterative DFS with resumable dep iterators.
    for &rootk in &border {
        if walk.state(rootk) != 0 {
            continue;
        }
        walk.set_state(rootk, ACTIVE);
        let mut stack: Vec<(u32, u32)> = vec![(rootk, 0)];
        while let Some(&(t, cur0)) = stack.last() {
            visited += 1;
            if visited.is_multiple_of(4096) && cancelled() {
                return None;
            }
            let dl = deps(t);
            let mut cur = cur0 as usize;
            let mut descend = None;
            while cur < dl.len() {
                let d = dl[cur];
                cur += 1;
                if walk.state(d) == 0 && (has(d) || built.contains_key(&d)) {
                    descend = Some(d);
                    break;
                }
            }
            if let Some(last) = stack.last_mut() {
                last.1 = cur as u32;
            }
            if let Some(d) = descend {
                walk.set_state(d, ACTIVE);
                stack.push((d, 0));
                continue;
            }
            // post-order: declared deps in list order (duplicates kept).
            // A dep still ACTIVE closes a cycle and is dropped and
            // counted; a DONE dep with no completion node is a
            // zero-weight dead end; a dep never entered was neither
            // defined nor traced (an undefined reference the loader
            // already counted).
            let mut dn: Vec<u32> = Vec::with_capacity(dl.len());
            for d in dl {
                match walk.state(*d) {
                    ACTIVE => {
                        note_cycle_edge(&mut n_cycle_edges, &mut cycle_samples, name(t), name(*d));
                    }
                    DONE => {
                        if let Some(id) = walk.completion(*d) {
                            dn.push(id);
                        }
                    }
                    _ => {}
                }
            }
            match built.get(&t) {
                None => {
                    // declared but ran in no traced action (tset join, or
                    // satisfied by an earlier command): free pass-through
                    if !dn.is_empty() {
                        let id = if dn.len() == 1 {
                            dn[0]
                        } else {
                            out.node(0, &dn, None)
                        };
                        walk.set_completion(t, id);
                    }
                }
                Some(es) => {
                    let mut previous = None;
                    for &e in es {
                        // Serial repeated actions need one predecessor, not a
                        // fresh heap allocation for every event.
                        let predecessor = [previous.unwrap_or(0)];
                        let dependencies = if previous.is_some() {
                            &predecessor[..]
                        } else {
                            &dn[..]
                        };
                        previous =
                            Some(out.node(p.dur[e as usize], dependencies, Some(e as usize)));
                    }
                    if let Some(id) = previous {
                        walk.set_completion(t, id);
                    }
                }
            }
            walk.set_state(t, DONE);
            stack.pop();
        }
    }
    // Exit node joins every BUILT target's completion.
    let ex: Vec<u32> = border.iter().filter_map(|&t| walk.completion(t)).collect();
    out.node(0, &ex, None);

    if cancelled() {
        return None;
    }
    let dag = finish(&mut out, &p.ord);
    if cancelled() {
        return None;
    }
    let stats = DagStats {
        n_events: p.sel.len(),
        n_matched: p.n_matched,
        n_named_only: p.n_named_only,
        n_unattributed: p.n_unattributed,
        n_cycle_edges,
        cycle_samples,
    };
    Some((
        DagBundle {
            dag,
            node_ev: out.node_ev,
            events: p.events,
        },
        stats,
    ))
}

/// The target-chain critical path: chain cost per built target, longest
/// tail over declared deps (equal costs break on the target NAME, first
/// maximum wins — pinned to the Python original's tuple comparison).
/// An independent cost oracle, not the viewer's path: tied paths can select
/// different steps from `chain_from_bundle` while agreeing on total cost.
/// Returns `None` under the same pairing, run-selection, recipe-presence
/// and u32 name-space restrictions as [`build_full`].
pub fn cp_chain(tr: &Trace, g: &Graph, run_sel: Option<&str>) -> Option<(CpChain, Vec<Ev>)> {
    let p = prepare(tr, g, run_sel)?;

    let mut built: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    let mut border: Vec<u32> = Vec::with_capacity(p.sel.len());
    for &e in &p.sel {
        let k = p.ekey[e as usize];
        match built.entry(k) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(vec![e]);
                border.push(k);
            }
            std::collections::hash_map::Entry::Occupied(mut o) => o.get_mut().push(e),
        }
    }
    let mut cost: FxHashMap<u32, i64> = FxHashMap::default();
    for &t in &border {
        cost.insert(t, built[&t].iter().map(|&e| p.dur[e as usize]).sum());
    }

    let has = |t: u32| -> bool { g.merged_has(t) };
    let deps = |t: u32| -> &[u32] { g.merged_deps(t) };
    let name = |k: u32| -> &str { p.keyname.get(k as usize).copied().unwrap_or("") };

    let mut tail: FxHashMap<u32, (i64, Option<u32>)> = FxHashMap::default();
    let mut state: FxHashMap<u32, u8> = FxHashMap::default();
    let mut n_cycle_edges = 0u64;
    let mut cycle_samples: Vec<String> = Vec::new();
    for &rootk in &border {
        if state.contains_key(&rootk) {
            continue;
        }
        state.insert(rootk, ACTIVE);
        let mut stack: Vec<(u32, u32)> = vec![(rootk, 0)];
        while let Some(&(node, cur0)) = stack.last() {
            let dl = deps(node);
            let mut cur = cur0 as usize;
            let mut descend = None;
            while cur < dl.len() {
                let d = dl[cur];
                cur += 1;
                if !state.contains_key(&d) && (has(d) || built.contains_key(&d)) {
                    descend = Some(d);
                    break;
                }
            }
            if let Some(last) = stack.last_mut() {
                last.1 = cur as u32;
            }
            if let Some(d) = descend {
                state.insert(d, ACTIVE);
                stack.push((d, 0));
                continue;
            }
            let mut best: Option<(i64, u32)> = None;
            for &d in dl {
                if state.get(&d) == Some(&ACTIVE) {
                    note_cycle_edge(&mut n_cycle_edges, &mut cycle_samples, name(node), name(d));
                    continue;
                }
                let Some(&(tu, _)) = tail.get(&d) else {
                    continue;
                };
                match best {
                    None => best = Some((tu, d)),
                    Some((bu, bd)) => {
                        if (tu, name(d)) > (bu, name(bd)) {
                            best = Some((tu, d));
                        }
                    }
                }
            }
            let (bu, bd) = match best {
                Some((u, d)) => (u, Some(d)),
                None => (0, None),
            };
            tail.insert(node, (cost.get(&node).copied().unwrap_or(0) + bu, bd));
            state.insert(node, DONE);
            stack.pop();
        }
    }

    // first maximum in insertion order
    let mut top = *border.first()?;
    let mut topv = tail.get(&top).map(|x| x.0).unwrap_or(0);
    for &t in &border[1..] {
        let v = tail.get(&t).map(|x| x.0).unwrap_or(0);
        if v > topv {
            topv = v;
            top = t;
        }
    }
    let mut chain: Vec<u32> = Vec::new();
    let mut n = Some(top);
    while let Some(t) = n {
        chain.push(t);
        n = tail.get(&t).and_then(|x| x.1);
    }
    chain.reverse(); // build order: deepest dependency first

    let mut entries: Vec<CpEntry> = Vec::new();
    for t in &chain {
        let Some(es) = built.get(t) else { continue };
        for &e in es {
            entries.push(CpEntry {
                ev: e as usize,
                us: p.dur[e as usize],
            });
        }
    }
    Some((
        CpChain {
            cp_us: topv,
            entries,
            n_cycle_edges,
            cycle_samples,
        },
        p.events,
    ))
}

fn finish(out: &mut Out, ord: &[u32]) -> Dag {
    let n = out.w.len();
    let mut longest = vec![0i64; n];
    let mut bestp = vec![u32::MAX; n];
    // self-referential pass (reads longest[j] for j < i while writing
    // longest[i]); an iterator form would fight the borrow checker
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let ps = &out.preds[out.pred_off[i] as usize..out.pred_off[i + 1] as usize];
        let mut m = 0i64;
        let mut bp = u32::MAX;
        if let Some(&f) = ps.first() {
            m = longest[f as usize];
            bp = f;
            for &j in &ps[1..] {
                if longest[j as usize] > m {
                    m = longest[j as usize];
                    bp = j;
                }
            }
        }
        longest[i] = out.w[i] + m;
        bestp[i] = bp;
    }
    let mut cp = 0i64;
    let mut top = usize::MAX;
    for (i, &l) in longest.iter().enumerate() {
        if l > cp {
            cp = l;
            top = i;
        }
    }
    let mut cp_path: Vec<u32> = Vec::new();
    if top != usize::MAX {
        let mut cur = top as u32;
        loop {
            cp_path.push(cur);
            let nx = bestp[cur as usize];
            if nx == u32::MAX {
                break;
            }
            cur = nx;
        }
        cp_path.reverse();
    }
    // longest path STARTING at each node (inclusive): reverse pass over
    // the same topological order (preds only reference lower ids, so by
    // the time i is visited descending, down[i] is final)
    let mut down: Vec<i64> = out.w.clone();
    for i in (0..n).rev() {
        for &j in &out.preds[out.pred_off[i] as usize..out.pred_off[i + 1] as usize] {
            let cand = down[i] + out.w[j as usize];
            if cand > down[j as usize] {
                down[j as usize] = cand;
            }
        }
    }

    let act_of_node: Vec<u32> = out
        .node_ev
        .iter()
        .map(|e| match e {
            Some(i) => ord[*i],
            None => u32::MAX,
        })
        .collect();
    cp_path.retain(|&i| act_of_node[i as usize] != u32::MAX);

    // transpose pred -> succ
    let mut succ_off = vec![0u32; n + 1];
    for &d in &out.preds {
        if (d as usize) < n {
            succ_off[d as usize + 1] += 1;
        }
    }
    for i in 0..n {
        succ_off[i + 1] += succ_off[i];
    }
    let mut succs = vec![0u32; out.preds.len()];
    {
        let mut cur: Vec<u32> = succ_off[..n].to_vec();
        for i in 0..n {
            for &d in &out.preds[out.pred_off[i] as usize..out.pred_off[i + 1] as usize] {
                // defensive: node() only ever emits ids < i, but never panic
                if let Some(slot) = cur.get_mut(d as usize) {
                    succs[*slot as usize] = i as u32;
                    *slot += 1;
                }
            }
        }
    }

    let t1 = out.w.iter().sum();
    // `down` remains an independent allocation. Only the three arrays whose
    // originals otherwise die with Out are eligible to move. Moving preserves
    // their spare capacities, so lower transient copying need not lower the
    // final retained capacity. `node_ev` remains with Out for the bundle.
    let weight_us = std::mem::take(&mut out.w);
    let pred_off = std::mem::take(&mut out.pred_off);
    let preds = std::mem::take(&mut out.preds);
    Dag {
        act_of_node,
        weight_us,
        pred_off,
        preds,
        succ_off,
        succs,
        t1,
        cp,
        up: longest,
        down,
        cp_path,
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::load_graph;
    use crate::parse;
    use crate::testdata as fixture;

    fn pred_lists(d: &Dag) -> Vec<Vec<u32>> {
        (0..d.weight_us.len())
            .map(|i| d.preds[d.pred_off[i] as usize..d.pred_off[i + 1] as usize].to_vec())
            .collect()
    }

    fn smoke() -> (Trace, Graph) {
        let tr = parse::parse(&fixture("smoke-trace.json")).expect("should parse smoke trace");
        let g = load_graph(&fixture("smoke-graph.jsonl")).expect("should parse smoke graph");
        (tr, g)
    }

    fn assert_dense_parity(tr: &Trace, g: &Graph, run: Option<&str>) {
        let baseline = build_full_with::<SparseWalk>(tr, g, run);
        match (baseline, build_full(tr, g, run)) {
            (Some((a, sa)), Some((b, sb))) => {
                assert_eq!(a.dag, b.dag);
                assert_eq!(a.node_ev, b.node_ev);
                assert_eq!(format!("{:?}", a.events), format!("{:?}", b.events));
                assert_eq!(sa, sb);
            }
            (None, None) => {}
            _ => panic!("dense DAG changed admission"),
        }
    }

    #[test]
    fn dense_dag_preserves_aliases_dead_ends_cycles_and_ties() {
        let (tr, g) = smoke();
        assert_dense_parity(&tr, &g, None);
        let cases: &[(&str, &[&str])] = &[
            (
                "switch-alias",
                &[
                    r#"{"t":"a","d":["tset:x","b","tset:x"]}"#,
                    r#"{"t":"tset:x","d":["b"],"j":1}"#,
                    r#"{"t":"b","d":[]}"#,
                ],
            ),
            (
                "switch-cycle",
                &[r#"{"t":"a","d":["b"]}"#, r#"{"t":"b","d":["a"]}"#],
            ),
            (
                "switch-self",
                &[r#"{"t":"a","d":["a"]}"#, r#"{"t":"b","d":[]}"#],
            ),
            (
                "switch-dead",
                &[r#"{"t":"a","d":["dead"]}"#, r#"{"t":"dead","d":[]}"#],
            ),
            ("switch-event-only", &[]),
        ];
        let tied = [
            r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":5}"#,
            r#"{"ph":"X","cat":"recipe","name":"b","ts":0,"dur":5}"#,
            r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":0}"#,
        ];
        for &(tag, rows) in cases {
            let (mut tr, gp) = pair(tag, &tied, rows);
            let g = crate::graph::load_graph_opts(&gp, &REPAIR).unwrap();
            assert!(build_full_with::<SparseWalk>(&tr, &g, None).is_some());
            assert_dense_parity(&tr, &g, None);
            // JSON parsing clamps zero durations to one. Set the core input
            // directly to exercise real zero-duration actionable nodes too.
            for act in &mut tr.acts {
                act.dur = 0;
            }
            assert_dense_parity(&tr, &g, None);
            let (zero, _) = build_full(&tr, &g, None).unwrap();
            assert_eq!(zero.dag.cp, 0);
            assert!(zero.dag.cp_path.is_empty());
            std::fs::remove_file(gp).unwrap();
        }
        // Named-only dependencies survive a torn tail but do not gain a row.
        let (tr, gp) = pair(
            "switch-torn",
            &[A10, B5],
            &[r#"{"t":"a","d":["b"]}"#, "{torn"],
        );
        let g = crate::graph::load_graph_opts(&gp, &REPAIR_OFF_QUIET).unwrap();
        assert_dense_parity(&tr, &g, None);
        std::fs::remove_file(gp).unwrap();
    }

    #[test]
    fn dense_tables_preserve_sparse_unvisited_graphs_and_run_selection() {
        let mut rows = vec![r#"{"t":"a","d":[]}"#.to_owned()];
        rows.extend((0..4096).map(|i| format!("{{\"t\":\"unvisited-{i}\",\"d\":[]}}")));
        let refs: Vec<_> = rows.iter().map(String::as_str).collect();
        let (tr, gp) = pair("switch-unvisited", &[A10], &refs);
        let mut g = load_graph(&gp).unwrap();
        assert_eq!(g.n_targets(), 4097);
        assert_dense_parity(&tr, &g, None);
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!((b.dag.cp, b.dag.weight_us.len(), st.n_events), (10, 2, 1));
        // The same strict admission gate must still reject a stamped mismatch.
        g.meta.capture = Some("unmatched-capture".to_owned());
        assert!(build_full_with::<SparseWalk>(&tr, &g, None).is_none());
        assert_dense_parity(&tr, &g, None);
        std::fs::remove_file(gp).unwrap();

        let (tr, gp) = pair(
            "switch-runs",
            &[
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"run":"r1"}}"#,
                r#"{"ph":"X","cat":"recipe","name":"b","ts":1,"dur":20,"args":{"run":"r2"}}"#,
                r#"{"ph":"X","cat":"recipe","name":"c","ts":2,"dur":0}"#,
            ],
            &[r#"{"t":"a","d":[]}"#, r#"{"t":"b","d":[]}"#],
        );
        let g = load_graph(&gp).unwrap();
        for run in [None, Some("r1"), Some("r2"), Some("missing")] {
            assert_dense_parity(&tr, &g, run);
        }
        std::fs::remove_file(gp).unwrap();
    }

    #[test]
    fn finish_moves_arrays_and_preserves_node_events_and_independent_down() {
        fn input() -> Out {
            let mut out = Out::default();
            out.w.reserve(31);
            out.pred_off.reserve(29);
            out.preds.reserve(23);
            out.pred_off.push(0);
            out.node(5, &[], Some(0));
            out.node(7, &[0], Some(1));
            out
        }
        let mut moving = input();
        let pointers = (
            moving.w.as_ptr(),
            moving.pred_off.as_ptr(),
            moving.preds.as_ptr(),
        );
        let capacities = (
            moving.w.capacity(),
            moving.pred_off.capacity(),
            moving.preds.capacity(),
        );
        let actual = finish(&mut moving, &[10, 20]);
        assert_eq!(actual.weight_us, [5, 7]);
        assert_eq!(actual.pred_off, [0, 0, 1]);
        assert_eq!(actual.preds, [0]);
        assert_eq!(actual.act_of_node, [10, 20]);
        assert_eq!(actual.up, [5, 12]);
        assert_eq!(actual.down, [12, 7]);
        assert_eq!(actual.cp_path, [0, 1]);
        assert_eq!((actual.t1, actual.cp), (12, 12));
        assert_ne!(actual.down.as_ptr(), actual.weight_us.as_ptr());
        assert_eq!(
            pointers,
            (
                actual.weight_us.as_ptr(),
                actual.pred_off.as_ptr(),
                actual.preds.as_ptr()
            )
        );
        assert_eq!(
            capacities,
            (
                actual.weight_us.capacity(),
                actual.pred_off.capacity(),
                actual.preds.capacity()
            )
        );
        assert_eq!(
            (
                moving.w.capacity(),
                moving.pred_off.capacity(),
                moving.preds.capacity()
            ),
            (0, 0, 0)
        );
        assert_eq!(moving.node_ev, [Some(0), Some(1)]);
    }

    #[test]
    fn smoke_dag_with_join_passthrough() {
        let (tr, g) = smoke();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        let d = &b.dag;
        // y=500, z=800 (through the aliased tset join), w=300, exit
        assert_eq!(d.weight_us, [500, 800, 300, 0]);
        // the single-dep tset row ALIASED to y's node: no join node, and
        // w's pred list keeps the duplicate id (y direct + y via tset)
        assert_eq!(
            pred_lists(d),
            vec![vec![], vec![0], vec![0, 0], vec![0, 1, 2]]
        );
        assert_eq!(d.t1, 1600);
        assert_eq!(d.cp, 1300); // y -> tset -> z
        assert_eq!(d.cp_path, [0, 1]);
        assert_eq!(b.node_ev, vec![Some(0), Some(1), Some(2), None]);
        // slack: w can grow 500us before y->w rivals y->z
        assert_eq!(d.slack(2), 500);
        assert_eq!(d.slack(0), 0);
        assert_eq!(d.slack(1), 0);
        assert_eq!(
            st,
            DagStats {
                n_events: 3,
                n_matched: 3,
                ..Default::default()
            }
        );
        // events are absolute-time
        assert_eq!(b.events[0].ts, 1_000_100);
        // topological ids
        for i in 0..d.weight_us.len() {
            for &p in &d.preds[d.pred_off[i] as usize..d.pred_off[i + 1] as usize] {
                assert!((p as usize) < i);
            }
        }
    }

    #[test]
    fn smoke_cp_chain_matches_dag_cp() {
        let (tr, g) = smoke();
        let (c, evs) = cp_chain(&tr, &g, None).unwrap();
        assert_eq!(c.cp_us, 1300);
        let names: Vec<&str> = c.entries.iter().map(|e| evs[e.ev].name.as_str()).collect();
        assert_eq!(names, ["y (rustc emit)", "z (cxx_link)"]);
        assert_eq!(
            c.entries.iter().map(|e| e.us).collect::<Vec<_>>(),
            [500, 800]
        );
    }

    #[test]
    fn event_missing_from_graph_still_gets_a_node() {
        let (tr, _) = smoke();
        // a graph that only knows y: z and w become event-only keys
        let mut p = std::env::temp_dir();
        p.push(format!("retrace-dag-{}-partial.json", std::process::id()));
        std::fs::write(
            &p,
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"y (rustc emit)\",\"d\":[]}\n",
        )
        .unwrap();
        let g = load_graph(p.to_str().unwrap()).unwrap();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!(b.dag.weight_us.len(), 4); // y, z, w, exit
        assert_eq!(
            st,
            DagStats {
                n_events: 3,
                n_matched: 1,
                ..Default::default()
            }
        );
        assert_eq!(b.dag.t1, 1600);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn duplicate_event_names_chain_sequentially() {
        let mut p = std::env::temp_dir();
        p.push(format!("retrace-dag-{}-dup-trace.json", std::process::id()));
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"run":"r"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":10,"dur":20,"args":{"run":"r"}},"#,
                "\n",
            ),
        )
        .unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!("retrace-dag-{}-dup-graph.json", std::process::id()));
        std::fs::write(&gp, "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[]}\n").unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        let (b, _) = build_full(&tr, &g, None).unwrap();
        // two action nodes chained, then exit; cp = t1 = 30
        assert_eq!(b.dag.weight_us, [10, 20, 0]);
        assert_eq!(pred_lists(&b.dag), vec![vec![], vec![0], vec![1]]);
        assert_eq!(b.dag.cp, 30);
        assert_eq!(b.dag.t1, 30);
        std::fs::remove_file(&p).ok();
        std::fs::remove_file(&gp).ok();
    }

    /// A trace/graph pair written to temp files; `graph_rows` go under
    /// an `enc:2` header.
    fn pair(tag: &str, trace_rows: &[&str], graph_rows: &[&str]) -> (Trace, String) {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-dag-{}-{tag}-trace.json",
            std::process::id()
        ));
        std::fs::write(&p, format!("[\n{}\n]\n", trace_rows.join(",\n"))).unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!(
            "retrace-dag-{}-{tag}-graph.json",
            std::process::id()
        ));
        std::fs::write(
            &gp,
            format!("{{\"graph\":{{\"enc\":2}}}}\n{}\n", graph_rows.join("\n")),
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        std::fs::remove_file(&p).ok();
        (tr, gp.to_string_lossy().into_owned())
    }

    const REPAIR: crate::graph::GraphLoadOpts = crate::graph::GraphLoadOpts {
        max_input_bytes: crate::parse::DEFAULT_MAX_INPUT_BYTES,
        need_deps: true,
        quiet: true,
        repair_cycles: true,
    };

    const A10: &str = r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{}}"#;
    const B5: &str = r#"{"ph":"X","cat":"recipe","name":"b","ts":10,"dur":5,"args":{}}"#;

    #[test]
    fn repair_mode_counts_back_edges_in_both_walks() {
        // two-cycle: the walk starts at a (the first traced root), so
        // b's dependency on a is the edge that closes the cycle
        let (tr, gp) = pair(
            "two",
            &[A10, B5],
            &[r#"{"t":"a","d":["b"]}"#, r#"{"t":"b","d":["a"]}"#],
        );
        assert!(load_graph(&gp).is_err(), "refused without repair");
        let g = crate::graph::load_graph_opts(&gp, &REPAIR).unwrap();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!(st.n_cycle_edges, 1);
        assert_eq!(st.cycle_samples, ["b -> a"]);
        assert_eq!(b.dag.t1, 15);
        assert_eq!(b.dag.cp, 15, "a after b once b -> a is dropped");
        let (c, _) = cp_chain(&tr, &g, None).unwrap();
        assert_eq!(c.n_cycle_edges, st.n_cycle_edges);
        assert_eq!(c.cycle_samples, st.cycle_samples);
        assert_eq!(c.cp_us, 15);
        for i in 0..b.dag.weight_us.len() {
            for &p in &b.dag.preds[b.dag.pred_off[i] as usize..b.dag.pred_off[i + 1] as usize] {
                assert!((p as usize) < i);
            }
        }
        std::fs::remove_file(&gp).ok();

        // self-cycle
        let (tr, gp) = pair("self", &[A10], &[r#"{"t":"a","d":["a"]}"#]);
        let g = crate::graph::load_graph_opts(&gp, &REPAIR).unwrap();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!(st.n_cycle_edges, 1);
        assert_eq!(st.cycle_samples, ["a -> a"]);
        assert_eq!((b.dag.t1, b.dag.cp), (10, 10));
        let (c, _) = cp_chain(&tr, &g, None).unwrap();
        assert_eq!(c.cycle_samples, ["a -> a"]);
        std::fs::remove_file(&gp).ok();

        // a cycle through a join row: the action edge is the one
        // dropped, the join stays a pass-through
        let acyclic = [
            r#"{"t":"a","d":["tset:x"]}"#,
            r#"{"t":"tset:x","d":["b"],"j":1}"#,
            r#"{"t":"b","d":[]}"#,
        ];
        let (tr, gp) = pair("join-ok", &[A10, B5], &acyclic);
        let g = load_graph(&gp).unwrap();
        let (ok, _) = build_full(&tr, &g, None).unwrap();
        std::fs::remove_file(&gp).ok();
        let (tr, gp) = pair(
            "join",
            &[A10, B5],
            &[acyclic[0], acyclic[1], r#"{"t":"b","d":["a"]}"#],
        );
        let g = crate::graph::load_graph_opts(&gp, &REPAIR).unwrap();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!(st.n_cycle_edges, 1);
        assert_eq!(st.cycle_samples, ["b -> a"]);
        assert_eq!(b.dag.weight_us.len(), ok.dag.weight_us.len());
        assert_eq!(b.dag.cp, 15);
        std::fs::remove_file(&gp).ok();
    }

    /// A name the graph holds only as another row's dependency is not
    /// a defined row: the action still gets a node through the edge,
    /// but it is not covered.
    #[test]
    fn a_dependency_only_name_is_not_a_defined_row() {
        let (tr, gp) = pair("namedonly", &[A10, B5], &[]);
        // the torn third line is what lets the loader keep b's dangling
        // reference instead of refusing the file
        std::fs::write(
            &gp,
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"b\"]}\n{\"t\":\"tset:de",
        )
        .unwrap();
        let g = crate::graph::load_graph_opts(&gp, &REPAIR_OFF_QUIET).unwrap();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!((st.n_events, st.n_matched, st.n_named_only), (2, 1, 1));
        assert_eq!(b.dag.weight_us.len(), 3, "b, a, exit");
        assert_eq!(b.dag.cp, 15, "the edge a -> b still stitches");
        std::fs::remove_file(&gp).ok();
    }

    const REPAIR_OFF_QUIET: crate::graph::GraphLoadOpts = crate::graph::GraphLoadOpts {
        max_input_bytes: crate::parse::DEFAULT_MAX_INPUT_BYTES,
        need_deps: true,
        quiet: true,
        repair_cycles: false,
    };

    #[test]
    fn zero_weight_dead_end_is_not_a_cycle() {
        // b is defined, never traced, and has nothing retained below it
        let (tr, gp) = pair(
            "deadend",
            &[A10],
            &[r#"{"t":"a","d":["b"]}"#, r#"{"t":"b","d":[]}"#],
        );
        let g = load_graph(&gp).unwrap();
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!(st.n_cycle_edges, 0);
        assert!(st.cycle_samples.is_empty());
        assert_eq!((b.dag.t1, b.dag.cp), (10, 10));
        let (c, _) = cp_chain(&tr, &g, None).unwrap();
        assert_eq!(c.n_cycle_edges, 0);
        assert_eq!(c.cp_us, 10);
        std::fs::remove_file(&gp).ok();
    }

    /// The refusal is a property of the graph; the dropped edge of a
    /// simple cycle follows the walk order and is reported as such.
    #[test]
    fn cycle_count_survives_row_and_event_reordering() {
        let b_first = r#"{"ph":"X","cat":"recipe","name":"b","ts":0,"dur":5,"args":{}}"#;
        let a_second = r#"{"ph":"X","cat":"recipe","name":"a","ts":10,"dur":10,"args":{}}"#;
        let (tr, gp) = pair(
            "reorder",
            &[b_first, a_second],
            &[r#"{"t":"b","d":["a"]}"#, r#"{"t":"a","d":["b"]}"#],
        );
        let err = format!("{:#}", load_graph(&gp).unwrap_err());
        assert!(err.contains("dependency cycle"), "{err}");
        let g = crate::graph::load_graph_opts(&gp, &REPAIR).unwrap();
        assert_eq!(g.targets_in_cycles, 2);
        let (b, st) = build_full(&tr, &g, None).unwrap();
        assert_eq!(st.n_cycle_edges, 1);
        assert_eq!(
            st.cycle_samples,
            ["a -> b"],
            "b roots first, so a -> b closes it"
        );
        assert_eq!(b.dag.cp, 15);
        let (c, _) = cp_chain(&tr, &g, None).unwrap();
        assert_eq!(c.cycle_samples, ["a -> b"]);
        std::fs::remove_file(&gp).ok();
    }

    #[test]
    fn several_runs_require_selection() {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-dag-{}-runs-trace.json",
            std::process::id()
        ));
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"run":"r1"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":100,"dur":20,"args":{"run":"r2"}},"#,
                "\n"
            ),
        )
        .unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!(
            "retrace-dag-{}-runs-graph.json",
            std::process::id()
        ));
        std::fs::write(&gp, "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[]}\n").unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        assert!(
            build_full(&tr, &g, None).is_none(),
            "no guessing among runs"
        );
        let (b1, s1) = build_full(&tr, &g, Some("r1")).unwrap();
        assert_eq!((b1.dag.t1, s1.n_events), (10, 1));
        let (b2, _) = build_full(&tr, &g, Some("r2")).unwrap();
        assert_eq!(b2.dag.t1, 20);
        // events list still carries both runs (node_ev scopes the nodes)
        assert_eq!(b2.events.len(), 2);
        std::fs::remove_file(&p).ok();
        std::fs::remove_file(&gp).ok();
    }

    #[test]
    fn run_less_events_are_counted_when_dropped_and_kept_for_a_sole_run() {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "retrace-dag-{}-unattr-trace.json",
            std::process::id()
        ));
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"run":"r1"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"b","ts":50,"dur":20,"args":{"run":"r2"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"c","ts":100,"dur":5,"args":{}},"#,
                "\n"
            ),
        )
        .unwrap();
        let mut gp = std::env::temp_dir();
        gp.push(format!(
            "retrace-dag-{}-unattr-graph.json",
            std::process::id()
        ));
        std::fs::write(
            &gp,
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[]}\n{\"t\":\"b\",\"d\":[]}\n{\"t\":\"c\",\"d\":[]}\n",
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        let g = load_graph(gp.to_str().unwrap()).unwrap();
        // two runs exist: the run-less event cannot be attributed, and
        // the drop is counted rather than shrinking n_events silently
        let (_, st) = build_full(&tr, &g, Some("r1")).unwrap();
        assert_eq!((st.n_events, st.n_unattributed), (1, 1));
        std::fs::remove_file(&p).ok();
        std::fs::remove_file(&gp).ok();

        // sole run: a run-less event belongs to it, nothing is dropped
        let mut p2 = std::env::temp_dir();
        p2.push(format!(
            "retrace-dag-{}-solerun-trace.json",
            std::process::id()
        ));
        std::fs::write(
            &p2,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":10,"args":{"run":"r1"}},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"c","ts":100,"dur":5,"args":{}},"#,
                "\n"
            ),
        )
        .unwrap();
        let tr2 = parse::parse(p2.to_str().unwrap()).unwrap();
        let (_, st2) = build_full(&tr2, &g, Some("r1")).unwrap();
        assert_eq!((st2.n_events, st2.n_unattributed), (2, 0));
        std::fs::remove_file(&p2).ok();
    }
}
