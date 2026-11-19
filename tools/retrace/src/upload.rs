/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The input-upload sub-stage breakdown: where the "inputs upload"
//! stage actually goes, per action and in aggregate. Instrumented
//! captures record, inside every upload span, the wait for the
//! client-side upload permit (buck2's `cas_semaphore`), the check of
//! what storage already has, local materialization, and the RE client
//! upload call itself — plus how many input digests storage lacked.
//! These counters describe
//! parallel action time, not a partition of command wall time.

use crate::analyses::Facts;
use crate::analyses::SLOW_GATHER_US;
use crate::analyses::UploadSub;

/// Aggregate stats for one sub-stage across the selected actions.
#[derive(Clone, Debug, Default)]
pub struct SubStat {
    /// actions where this sub-stage was nonzero
    pub n: usize,
    /// Covered actions, including measured-zero phases.
    pub n_measured: usize,
    pub n_zero: usize,
    pub sum_us: i64,
    pub p50_us: i64,
    pub p90_us: i64,
    pub p99_us: i64,
    pub max_us: i64,
    /// event index of the max, for click-through
    pub max_ev: Option<u32>,
}

pub const SUB_N: usize = 4;

/// Display names, parallel to the stats array; same no-jargon rule as
/// the stage labels.
pub const SUB_LABEL: [&str; SUB_N] = [
    "permit wait (client upload slot)",
    "presence check (is each input already in CAS?)",
    "materialize local files",
    "CAS write (the upload call itself)",
];

pub const SUB_CHIP: [&str; SUB_N] = ["permit wait", "presence check", "materialize", "CAS write"];

/// Bar tints, parallel to the stats array. Phase duration alone does not
/// establish idle CPU, network throughput, or the cause of a delay.
pub const SUB_COLOR: [[u8; 3]; SUB_N] = [
    [0xe0, 0x5c, 0x3c], // permit wait — dead time
    [0xf1, 0xce, 0x63], // presence check
    [0x6f, 0xb8, 0xe8], // materialize
    [0xa5, 0xd8, 0xa5], // CAS write — real work
];

pub struct UploadReport {
    /// actions carrying an `input_upload_us` stage
    pub n_upload_actions: usize,
    /// of those, actions carrying the sub-stage breakdown
    pub n_with_breakdown: usize,
    /// Breakdown-only rows are not part of the positive-upload population.
    pub n_breakdown_without_stage: usize,
    /// Σ input_upload_us over the covered actions
    pub stage_sum_us: i64,
    pub subs: [SubStat; SUB_N],
    /// digest classification totals across the covered actions
    pub absent: i64,
    pub absent_measured: usize,
    /// worst permit waits, descending: (event, sem_wait_us)
    pub top_wait: Vec<(u32, i64)>,
    /// Actions with a permit wait, before `top_wait` keeps the worst ten.
    pub n_waits: usize,
    /// Uploads that spent at least [`SLOW_GATHER_US`] gathering local files while the action's
    /// local attempt (a hybrid race the remote side won) was preparing the same inputs:
    /// (event, gathering time), descending.
    pub local_race: Vec<(u32, i64)>,
    /// Modeled input-upload windows for occupancy lookups, sorted
    /// independently; a window opens at action start + cache lookup
    /// (assuming upload follows lookup). These are not positioned
    /// observations and count permit holders as well as waiters.
    pub iu_starts: Vec<i64>,
    pub iu_ends: Vec<i64>,
}

impl UploadReport {
    /// How many actions were inside their own inputs-upload window at
    /// instant `t` (a window starting exactly at `t` counts).
    pub fn depth_at(&self, t: i64) -> i64 {
        let s = self.iu_starts.partition_point(|&x| x <= t) as i64;
        let e = self.iu_ends.partition_point(|&x| x <= t) as i64;
        (s - e).max(0)
    }
}

/// `None` when no selected action carries an input-upload stage or breakdown.
pub fn upload_report(f: &Facts) -> Option<UploadReport> {
    let mut vals: [Vec<i64>; SUB_N] = Default::default();
    let mut r = UploadReport {
        n_upload_actions: 0,
        n_with_breakdown: 0,
        n_breakdown_without_stage: 0,
        stage_sum_us: 0,
        subs: Default::default(),
        absent: 0,
        absent_measured: 0,
        top_wait: Vec::new(),
        n_waits: 0,
        local_race: Vec::new(),
        iu_starts: Vec::new(),
        iu_ends: Vec::new(),
    };
    for &e in &f.sel {
        let a = f.args(e);
        let Some(st) = a.stages.filter(|st| st.input_upload_us > 0) else {
            r.n_breakdown_without_stage += usize::from(a.iu.is_some());
            continue;
        };
        r.n_upload_actions += 1;
        r.stage_sum_us += st.input_upload_us;
        let start = f.ts(e).saturating_add(st.cache_query_us);
        r.iu_starts.push(start);
        r.iu_ends.push(start.saturating_add(st.input_upload_us));
        let Some(sub) = &a.iu else { continue };
        r.n_with_breakdown += 1;
        if sub.valid(UploadSub::ABSENT) {
            r.absent += sub.absent;
            r.absent_measured += 1;
        }
        for (i, (vs, st)) in vals.iter_mut().zip(r.subs.iter_mut()).enumerate() {
            let Some(v) = sub.phase(i) else { continue };
            st.n_measured += 1;
            st.n_zero += usize::from(v == 0);
            if v > 0 {
                vs.push(v);
                st.sum_us += v;
                if v > st.max_us {
                    st.max_us = v;
                    st.max_ev = Some(e);
                }
            }
        }
        if sub.valid(UploadSub::SEM_WAIT) && sub.sem_wait_us > 0 {
            r.top_wait.push((e, sub.sem_wait_us));
        }
        if sub.valid(UploadSub::MATERIALIZE)
            && sub.materialize_us >= SLOW_GATHER_US
            && a.cancelled
                .is_some_and(|c| c.preparing_local_inputs(&f.x.srcs))
        {
            r.local_race.push((e, sub.materialize_us));
        }
    }
    if r.n_upload_actions == 0 && r.n_breakdown_without_stage == 0 {
        return None;
    }
    for (i, v) in vals.iter_mut().enumerate() {
        v.sort_unstable();
        r.subs[i].n = v.len();
        let q = |percent: usize| {
            if v.is_empty() {
                0
            } else {
                v[crate::analyses::percentile_index(v.len(), percent)]
            }
        };
        r.subs[i].p50_us = q(50);
        r.subs[i].p90_us = q(90);
        r.subs[i].p99_us = q(99);
    }
    r.top_wait.sort_by_key(|&(_, w)| std::cmp::Reverse(w));
    r.n_waits = r.top_wait.len();
    r.top_wait.truncate(10);
    r.local_race
        .sort_by_key(|&(e, gather)| (std::cmp::Reverse(gather), e));
    r.iu_starts.sort_unstable();
    r.iu_ends.sort_unstable();
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_that_waited_beside_a_local_attempt_are_listed_and_found() {
        let path = crate::temp_path("upload-local-race-trace.json");
        let row = |name: &str, gather: i64, cancelled: &str| {
            format!(
                r#"{{"ph":"X","cat":"recipe","name":"{name}","ts":100,"dur":90000000,"args":{{"run":"r","stages":{{"input_upload_us":71000000}},"re":{{"iu_materialize_us":{gather},"iu_digests_queried":1,"cancelled_stages":[{cancelled}]}}}}}}"#
            )
        };
        let local = r#"{"name":"Local.MaterializeInputs","ts":101,"dur":83000000}"#;
        std::fs::write(
            &path,
            format!(
                "[\n{},\n{},\n{}\n]\n",
                row("raced", 69_000_000, local),
                row("quick", 10_000, local),
                row("alone", 69_000_000, ""),
            ),
        )
        .unwrap();
        let tr = crate::parse::parse(path.to_str().unwrap()).unwrap();
        let x = crate::analyses::load_extras(&tr);
        let facts = Facts::for_run(&tr, &x, None).unwrap();
        let report = upload_report(&facts).unwrap();
        assert_eq!(report.local_race.len(), 1);
        assert_eq!(facts.name(report.local_race[0].0), "raced");
        let findings = crate::findings::findings(&facts, None);
        let row = findings
            .rows
            .iter()
            .find(|r| r.class == "local_race_upload_wait")
            .expect("a finding for the raced upload");
        assert_eq!((row.cost_us, row.estimate), (69_000_000, "bound"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn parsed_upload_coverage_zeros_and_maxima_use_one_population() {
        let path = crate::temp_path("upload-coverage-trace.json");
        std::fs::write(&path, concat!(
            "[\n",
            r#"{"ph":"X","cat":"recipe","name":"other","ts":0,"dur":100,"args":{"run":"old"}},"#,"\n",
            r#"{"ph":"X","cat":"recipe","name":"covered","ts":100,"dur":100,"args":{"run":"new","stages":{"input_upload_us":90},"re":{"iu_sem_wait_us":60,"iu_digests_queried":1}}},"#,"\n",
            r#"{"ph":"X","cat":"recipe","name":"missing","ts":200,"dur":100,"args":{"run":"new","stages":{"input_upload_us":90}}},"#,"\n",
            r#"{"ph":"X","cat":"recipe","name":"no-stage","ts":300,"dur":100,"args":{"run":"new","re":{"iu_sem_wait_us":99,"iu_digests_queried":1}}},"#,"\n",
        )).unwrap();
        let tr = crate::parse::parse(path.to_str().unwrap()).unwrap();
        let x = crate::analyses::load_extras(&tr);
        let facts = Facts::for_run(&tr, &x, Some("new")).unwrap();
        let report = upload_report(&facts).unwrap();
        assert_eq!((report.n_upload_actions, report.n_with_breakdown), (2, 1));
        assert_eq!(report.n_breakdown_without_stage, 1);
        assert_eq!(report.stage_sum_us, 180);
        assert_eq!(report.subs[0].sum_us, 60);
        assert_eq!(facts.name(report.subs[0].max_ev.unwrap()), "covered");
        assert_eq!((report.subs[0].n_measured, report.subs[0].n_zero), (1, 0));
        assert_eq!(
            (
                report.subs[3].n,
                report.subs[3].n_measured,
                report.subs[3].n_zero
            ),
            (0, 1, 1)
        );
        assert_eq!(report.subs[3].max_ev, None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sub_arrays_agree() {
        assert_eq!(SUB_LABEL.len(), SUB_N);
        assert_eq!(SUB_CHIP.len(), SUB_N);
        assert_eq!(SUB_COLOR.len(), SUB_N);
    }

    #[test]
    fn depth_at_counts_open_windows() {
        let r = UploadReport {
            n_upload_actions: 3,
            n_with_breakdown: 0,
            n_breakdown_without_stage: 0,
            stage_sum_us: 0,
            subs: Default::default(),
            absent: 0,
            absent_measured: 0,
            top_wait: Vec::new(),
            n_waits: 0,
            local_race: Vec::new(),
            iu_starts: vec![0, 2, 5],
            iu_ends: vec![6, 8, 10],
        };
        assert_eq!(r.depth_at(0), 1, "the window opening at t counts");
        assert_eq!(r.depth_at(5), 3, "all three windows open at t=5");
        assert_eq!(r.depth_at(7), 2, "the (0,6) window closed at 6");
        assert_eq!(r.depth_at(11), 0, "all closed");
    }

    #[test]
    fn pick_order_matches_labels() {
        let sub = UploadSub {
            sem_wait_us: 1,
            find_missing_us: 2,
            materialize_us: 3,
            cas_call_us: 4,
            ..Default::default()
        };
        assert_eq!(sub.phase(UploadSub::PHASE_PERMIT_WAIT), Some(1));
        assert_eq!(sub.phase(UploadSub::PHASE_PRESENCE_CHECK), Some(2));
        assert_eq!(sub.phase(UploadSub::PHASE_MATERIALIZE), Some(3));
        assert_eq!(sub.phase(UploadSub::PHASE_CAS_CALL), Some(4));
    }
}
