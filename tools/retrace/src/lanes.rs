/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Row/lane assignment.
//!
//! `tid` in the trace is never read; rows come from re-packing
//! timestamps. buck2 runs one daemon with unbounded parallelism, so the
//! layout is always a single labelled band of packed concurrency lanes —
//! the row count IS the observed peak concurrency, which is information,
//! not decoration. Cache/phase bookkeeping rows (when a capture carries
//! them) pack separately into a trailing band so they never inflate the
//! daemon band's concurrency reading.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::types::Act;
use crate::types::Band;
use crate::types::F_NOT_RECIPE;

/// Greedy free-lane packing: each act takes the lowest lane free at its
/// start. ORDER must be ts-ascending with file order breaking ties.
/// Returns the number of lanes used and writes `base + lane` into each act.
fn pack(acts: &mut [Act], order: &[usize], base: u32) -> u32 {
    // (Reverse(end_ts), Reverse(lane)) => pop lowest end, then lowest lane.
    let mut running: BinaryHeap<(Reverse<i64>, Reverse<u32>)> = BinaryHeap::new();
    let mut free: BinaryHeap<Reverse<u32>> = BinaryHeap::new();
    let mut used = 0u32;
    for &i in order {
        let ts = acts[i].ts;
        while let Some(&(Reverse(end), Reverse(lane))) = running.peek() {
            if end <= ts {
                running.pop();
                free.push(Reverse(lane));
            } else {
                break;
            }
        }
        let lane = match free.pop() {
            Some(Reverse(l)) => l,
            None => {
                let l = used;
                used += 1;
                l
            }
        };
        acts[i].lane = base + lane;
        // a zero-length action still holds its lane for 1us
        running.push((Reverse(ts + acts[i].dur.max(1)), Reverse(lane)));
    }
    used
}

/// ts-ascending, file order breaking ties (matches a stable sort over
/// the source file).
fn ts_order(acts: &[Act], keep: impl Fn(&Act) -> bool) -> Vec<usize> {
    let mut v: Vec<usize> = (0..acts.len()).filter(|&i| keep(&acts[i])).collect();
    v.sort_unstable_by_key(|&i| (acts[i].ts, acts[i].seq));
    v
}

pub struct Layout {
    pub n_lanes: u32,
    pub bands: Vec<Band>,
}

/// The daemon band, then a trailing band for cache/phase rows when the
/// capture has any. Every row holds content, so `n_lanes` is the number
/// of rows the build actually used and a row index is its lane number.
pub fn assign(acts: &mut [Act]) -> Layout {
    let recipes: Vec<usize> = ts_order(acts, |a| a.flags & F_NOT_RECIPE == 0);
    let others: Vec<usize> = ts_order(acts, |a| a.flags & F_NOT_RECIPE != 0);

    let n = pack(acts, &recipes, 0);
    let mut bands = vec![Band {
        base: 0,
        nrows: n,
        gutter: "buck2".into(),
    }];
    let mut n_lanes = n;

    if !others.is_empty() {
        let base = n_lanes;
        let n = pack(acts, &others, base);
        bands.push(Band {
            base,
            nrows: n,
            gutter: "cache".into(),
        });
        n_lanes += n;
    }

    Layout { n_lanes, bands }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(ts: i64, dur: i64, seq: u32, flags: u8) -> Act {
        Act {
            ts,
            dur,
            lane: 0,
            color: 0,
            flags,
            name: 0,
            off: 0,
            len: 0,
            seq,
            dir: crate::types::NO_STR,
            why: crate::types::NO_STR,
            run: crate::types::NO_STR,
            cache_bytes: None,
        }
    }

    #[test]
    fn packs_lowest_free_lane() {
        // acts 0/1 overlap, act 2 starts after act 0 ends -> reuses its lane
        let mut acts = vec![act(0, 10, 0, 0), act(5, 10, 1, 0), act(10, 5, 2, 0)];
        let l = assign(&mut acts);
        assert_eq!(acts[0].lane, 0);
        assert_eq!(acts[1].lane, 1);
        assert_eq!(acts[2].lane, 0); // freed by act 0 (end 10 <= ts 10)
        assert_eq!(l.n_lanes, 2);
        assert_eq!(l.bands.len(), 1);
        assert_eq!(l.bands[0].nrows, 2);
    }

    #[test]
    fn zero_duration_act_holds_its_lane() {
        let mut acts = vec![act(0, 0, 0, 0), act(0, 5, 1, 0)];
        assign(&mut acts);
        assert_ne!(acts[0].lane, acts[1].lane);
    }

    #[test]
    fn ties_break_on_file_order() {
        let mut acts = vec![act(0, 5, 0, 0), act(0, 5, 1, 0), act(0, 5, 2, 0)];
        assign(&mut acts);
        assert_eq!((acts[0].lane, acts[1].lane, acts[2].lane), (0, 1, 2));
    }

    #[test]
    fn non_recipe_rows_get_a_trailing_band() {
        let mut acts = vec![act(0, 10, 0, 0), act(0, 10, 1, F_NOT_RECIPE)];
        let l = assign(&mut acts);
        assert_eq!(acts[0].lane, 0);
        assert_eq!(acts[1].lane, 1); // base = 1 recipe lane
        assert_eq!(l.bands.len(), 2);
        assert_eq!(l.bands[1].base, 1);
        assert_eq!(l.n_lanes, 2);
    }

    /// The row count is read as a concurrency reading and exported as
    /// `"lanes"`, so a row that carries no act makes both wrong. Bands
    /// are laid out end to end with nothing between them.
    #[test]
    fn every_row_belongs_to_an_act() {
        let mut acts = vec![
            act(0, 10, 0, 0),
            act(5, 10, 1, 0),
            act(0, 10, 2, F_NOT_RECIPE),
        ];
        let l = assign(&mut acts);
        let mut occupied: Vec<u32> = acts.iter().map(|a| a.lane).collect();
        occupied.sort_unstable();
        occupied.dedup();
        assert_eq!(occupied, (0..l.n_lanes).collect::<Vec<_>>());
        assert_eq!(
            l.bands.iter().map(|b| b.nrows).sum::<u32>(),
            l.n_lanes,
            "bands cover every row and no row twice"
        );
    }
}
