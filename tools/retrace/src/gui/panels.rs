/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The Analysis-menu windows: table renders over the diff-5 analyses.
//! All state mutation flows back through `Ready` after the popups
//! close (sort clicks and row navigation are collected per frame).

use super::GState;
use super::Ready;
use super::widgets::ActTableOpts;
use super::widgets::TRow;
use super::widgets::Tbl;
use super::widgets::act_table;
use super::widgets::draw_tbl;
use super::widgets::fail_color;
use super::widgets::popup;
use crate::analyses;
use crate::humanize;

/// The parts of the window, and what to call them. Aaron asked for these
/// names so we could talk about the same thing; they are the vocabulary the
/// rest of the docs use too.
const PARTS: [(&str, &str); 10] = [
    (
        "timeline",
        "the big panel: one row per lane, one bar per action",
    ),
    (
        "lane",
        "a row. Lanes are packed, not threads \u{2014} a build has as \
         many as it had actions in flight at its peak",
    ),
    (
        "lane gutter",
        "the numbered column down the left. The range on screen \
         is spelled out on the in-flight line",
    ),
    (
        "bar",
        "one action. Colored coverage comes only from positioned observations; \
         neutral portions have no positioned evidence. Stage totals are in details",
    ),
    ("time ruler", "the tick row along the top of the timeline"),
    (
        "in-flight strip",
        "the blue histogram under the ruler: how many actions \
         were running, across the window on screen",
    ),
    (
        "overview",
        "the strip along the bottom: the whole build on the \
         timeline's own two axes \u{2014} time across, lane \
         down, so a column's height is how busy the build \
         was. The lane axis is squashed: a build spends most \
         of its life in its lowest few thousand lanes and \
         they would be a hairline otherwise",
    ),
    (
        "viewport brush",
        "the pink box on the overview: the time and the \
         lanes the timeline is showing. Zoom either axis \
         and it resizes. Click or drag to jump",
    ),
    (
        "legend",
        "the colour chips in the menu bar: what class each bar \
         colour means",
    ),
    (
        "scrollbars",
        "right edge scrolls lanes, bottom scrolls time",
    ),
];

/// Records a table's sort click. Every open panel draws in one frame and
/// each returns `None` unless one of its own headers was hit, so this has
/// to fold: assigning would let a panel drawn later erase the click.
fn keep_sort(
    slot: &mut Option<(&'static str, (u8, bool))>,
    key: &'static str,
    clicked: Option<(u8, bool)>,
) {
    if let Some(v) = clicked {
        *slot = Some((key, v));
    }
}

fn upload_duration(us: i64, measured: usize) -> String {
    if measured == 0 {
        "unavailable".to_owned()
    } else {
        analyses::fmt_dur(us)
    }
}

fn upload_coverage(measured: usize, population: usize) -> String {
    format!(
        "{} / {}{}",
        humanize::count(measured as u64),
        humanize::count(population as u64),
        if measured < population {
            " (partial)"
        } else {
            ""
        }
    )
}

/// Rows of the upload breakdown's list of uploads that waited beside a local attempt.
const UPLOAD_RACE_ROWS: usize = 20;

fn upload_digest_count(count: i64, measured: usize, population: usize) -> String {
    if measured == 0 {
        "unavailable".to_owned()
    } else {
        format!(
            "{} ({} actions measured)",
            humanize::count(count as u64),
            upload_coverage(measured, population)
        )
    }
}

fn upload_phase_sum(subs: &[crate::upload::SubStat]) -> i128 {
    subs.iter()
        .filter(|s| s.n_measured > 0)
        .map(|s| i128::from(s.sum_us.max(0)))
        .sum()
}

pub fn show_panels(app: &mut Ready, ctx: &egui::Context) {
    let mut nav: Option<usize> = None;
    let mut new_sort: Option<(&'static str, (u8, bool))> = None;

    // a dead worker must leave an error and a way forward in the panel,
    // never a spinner that can never resolve
    let an_err = app.an_error.clone();
    let retry = std::cell::Cell::new(false);
    let spin = |ui: &mut egui::Ui| match &an_err {
        Some(e) => {
            ui.colored_label(fail_color(), format!("analysis failed: {e}"));
            if ui.button("Retry").clicked() {
                retry.set(true);
            }
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("analyzing\u{2026}");
            });
        }
    };

    // ---- Build overview ----
    let mut open = app.panels.overview;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Build overview",
            "Build overview",
            &mut open,
            [1000.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => {
                    let val_w = (ui.available_width() - 240.0).max(420.0);
                    egui::Grid::new("ovpy")
                        .num_columns(2)
                        .striped(true)
                        .min_col_width(120.0)
                        .show(ui, |ui| {
                            for r in &an.ov_rows {
                                ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(r.key).strong())
                                            .extend(),
                                    );
                                });
                                let failed = r.key == "Result" && r.value.contains("FAILED");
                                // an unrecorded outcome is not a success either
                                let unknown = r.key == "Result" && r.value.starts_with("unknown");
                                let mut text = egui::RichText::new(&r.value).monospace().size(11.0);
                                if failed {
                                    text = text.color(fail_color());
                                } else if unknown {
                                    text = text
                                        .color(crate::config::ui_color(crate::config::UI_CAUTION));
                                }
                                ui.scope(|ui| {
                                    ui.set_min_width(val_w);
                                    ui.set_max_width(val_w);
                                    ui.add(egui::Label::new(text).wrap());
                                });
                                ui.end_row();
                            }
                        });
                    if let Some((q, act, name)) = &an.queue {
                        ui.separator();
                        ui.label(
                        egui::RichText::new(format!(
                            "RE queue counter recorded for {} of {} actions; mean {}, p90 {}, max {} ({name}); includes measured zeros",
                            humanize::count(q.covered as u64),
                            humanize::count(q.total as u64),
                            humanize::dur(q.mean_us as f64),
                            humanize::dur(q.p90_us as f64),
                            humanize::dur(q.max_us as f64),
                        ))
                        .size(11.0),
                    );
                        if let Some(a) = act {
                            if ui.small_button("worst wait \u{25b8}").clicked() {
                                nav = Some(*a);
                            }
                        }
                    }
                }
            },
        );
        app.panels.overview = open;
    }

    // ---- breakdown by type ----
    let mut open = app.panels.breakdown;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Build breakdown by type",
            "Build breakdown by type",
            &mut open,
            [760.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => {
                    // the rows sum to exactly the denominator `share_pct`
                    // divides by, so naming it here cannot drift from the
                    // column
                    let work: i64 = an.breakdown.iter().map(|r| r.total_us).sum();
                    ui.label(
                        egui::RichText::new(format!(
                            "Shares are of {} of action work summed across parallel actions, \
                             not of the build's wall \u{2014} a type owning 40% of the work does \
                             not save 40% of the build.",
                            humanize::dur(work as f64)
                        ))
                        .size(10.5)
                        .weak(),
                    );
                    let t = Tbl {
                        cols: analyses::BREAKDOWN_COLUMNS,
                        rows: an
                            .breakdown
                            .iter()
                            .map(|r| TRow {
                                cells: r.cells(),
                                keys: vec![
                                    f64::NAN,
                                    r.count as f64,
                                    r.total_us as f64,
                                    r.share_pct as f64,
                                    r.avg_us as f64,
                                ],
                                act: None,
                                hover: None,
                            })
                            .collect(),
                    };
                    let key = "breakdown";
                    let cur = app.sorts.get(key).copied().unwrap_or((2, true));
                    keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                }
            },
        );
        app.panels.breakdown = open;
    }

    // ---- busiest categories ----
    let mut open = app.panels.rule_sites;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Busiest categories",
            "Busiest categories",
            &mut open,
            [1000.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => {
                    ui.label(
                        egui::RichText::new(
                            "Total work attributed to each action category.  Click a header \
                             to sort.",
                        )
                        .size(11.0),
                    );
                    ui.separator();
                    let t = Tbl {
                        cols: analyses::RULE_SITES_COLUMNS,
                        rows: an
                            .rule_sites
                            .iter()
                            .map(|r| TRow {
                                cells: r.cells(),
                                keys: vec![
                                    f64::NAN,
                                    r.count as f64,
                                    r.total_us as f64,
                                    r.share_pct as f64,
                                ],
                                act: None,
                                hover: None,
                            })
                            .collect(),
                    };
                    let key = "rulesites";
                    let cur = app.sorts.get(key).copied().unwrap_or((2, true));
                    keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                }
            },
        );
        app.panels.rule_sites = open;
    }

    // ---- exec-peer percentiles ----
    let mut open = app.panels.peers;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Exec-peer percentiles",
            "Exec-peer percentiles",
            &mut open,
            [900.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => {
                    ui.label(
                        egui::RichText::new(
                            "Duration distribution per category (nearest-rank percentiles). \
                             The detail popup shows where one action sits among these peers.",
                        )
                        .size(11.0),
                    );
                    ui.separator();
                    const COLS: analyses::Columns = &[
                        ("category", 240, 'w'),
                        ("actions", 80, 'e'),
                        ("p50", 90, 'e'),
                        ("p90", 90, 'e'),
                        ("max", 90, 'e'),
                        ("total", 100, 'e'),
                    ];
                    let t = Tbl {
                        cols: COLS,
                        rows: an
                            .peers
                            .iter()
                            .map(|r| TRow {
                                cells: vec![
                                    r.cat.clone(),
                                    r.n.to_string(),
                                    humanize::dur(r.p50_us as f64),
                                    humanize::dur(r.p90_us as f64),
                                    humanize::dur(r.max_us as f64),
                                    humanize::dur(r.total_us as f64),
                                ],
                                keys: vec![
                                    f64::NAN,
                                    r.n as f64,
                                    r.p50_us as f64,
                                    r.p90_us as f64,
                                    r.max_us as f64,
                                    r.total_us as f64,
                                ],
                                act: None,
                                hover: None,
                            })
                            .collect(),
                    };
                    let key = "peers";
                    let cur = app.sorts.get(key).copied().unwrap_or((5, true));
                    keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                }
            },
        );
        app.panels.peers = open;
    }

    // ---- top memory ----
    let mut open = app.panels.top_mem;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Top memory (peak RSS)",
            "Top memory (peak RSS)",
            &mut open,
            [1200.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) if an.top_mem.is_empty() => {
                    ui.label(analyses::TOP_MEMORY_REFUSAL);
                }
                Some(an) => {
                    let t = Tbl {
                        cols: analyses::TOP_MEMORY_COLUMNS,
                        rows: an
                            .top_mem
                            .iter()
                            .map(|(r, act)| TRow {
                                cells: r.cells(),
                                keys: vec![
                                    r.maxrss as f64,
                                    f64::NAN,
                                    f64::NAN,
                                    r.dur_us as f64,
                                    r.exec_us.map_or(f64::NAN, |us| us as f64),
                                    f64::NAN,
                                ],
                                act: *act,
                                hover: None,
                            })
                            .collect(),
                    };
                    let key = "topmem";
                    let cur = app.sorts.get(key).copied().unwrap_or((0, true));
                    keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                }
            },
        );
        app.panels.top_mem = open;
    }

    // ---- CP-replay what-if ----
    let mut open = app.panels.whatif;
    if open {
        popup(
            ctx,
            app.native_popups,
            "What-if: CP-replay scenarios",
            "What-if: CP-replay scenarios",
            &mut open,
            [1000.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) if !an.reports.whatif => spin(ui),
                Some(an) => match &an.whatif {
                    None => {
                        ui.label(
                            "Needs the dependency graph — scenarios replay the longest path \
                             over declared deps.  Reopen once the graph chip turns green.",
                        );
                    }
                    Some(rep) => {
                        ui.label(
                            egui::RichText::new(format!(
                                "Baseline: critical path {} over {} action nodes ({} with \
                                 stage data).  Each row edits per-node durations from the \
                                 recorded stages and replays the longest path — the change \
                                 to the dependency floor, not a span prediction.  These are \
                                 independent leave-one-out replays against the same baseline \
                                 \u{2014} they do not add.",
                                analyses::fmt_dur(rep.base_cp_us),
                                humanize::count(rep.n_action_nodes as u64),
                                humanize::count(rep.n_with_stages as u64),
                            ))
                            .size(11.0),
                        );
                        savings_conditional_note(ui, &app.gstate);
                        ui.separator();
                        const WCOLS: analyses::Columns = &[
                            ("scenario", 340, 'w'),
                            ("est. CP", 110, 'e'),
                            ("saved", 110, 'e'),
                            ("vs CP", 80, 'e'),
                            ("modified", 90, 'e'),
                        ];
                        let t = Tbl {
                            cols: WCOLS,
                            rows: rep
                                .rows
                                .iter()
                                .map(|r| TRow {
                                    cells: vec![
                                        r.label.clone(),
                                        if r.is_priced() {
                                            analyses::fmt_dur(r.cp_us)
                                        } else {
                                            "not measured".into()
                                        },
                                        // a regression is not a saving: the
                                        // vs-CP column carries the sign
                                        if r.is_priced() && r.saved_us > 0 {
                                            analyses::fmt_dur(r.saved_us)
                                        } else {
                                            "—".to_owned()
                                        },
                                        if !r.is_priced() {
                                            "—".into()
                                        } else {
                                            format!(
                                                "{:+.1}%",
                                                if rep.base_cp_us > 0 {
                                                    -100.0 * r.saved_us as f64
                                                        / rep.base_cp_us as f64
                                                } else {
                                                    0.0
                                                }
                                            )
                                        },
                                        analyses::fmt_n(r.n_modified as f64),
                                    ],
                                    keys: vec![
                                        f64::NAN,
                                        r.cp_us as f64,
                                        r.saved_us as f64,
                                        r.saved_us as f64,
                                        r.n_modified as f64,
                                    ],
                                    act: None,
                                    hover: Some(format!(
                                        "Basis: {:?}; observation coverage {:?}. {}",
                                        r.basis, r.coverage, r.note
                                    )),
                                })
                                .collect(),
                        };
                        let key = "whatif";
                        let cur = app.sorts.get(key).copied().unwrap_or((2, true));
                        keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                    }
                },
            },
        );
        app.panels.whatif = open;
    }

    // ---- ranked findings ----
    let mut open = app.panels.findings;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Ranked findings",
            "Ranked findings",
            &mut open,
            [1150.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => {
                    let Some((rep, acts)) = &an.findings else {
                        spin(ui);
                        return;
                    };
                    ui.weak(format!("Stage-counter coverage: {} of {} actions. Opportunities price only the available evidence.", rep.with_stages, rep.nacts));
                    ui.weak("Findings use recorded durations; what-if overrides are not applied.");
                    if !app.cp_overrides.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "▲ {} what-if override(s) are active; these findings ignore them and may list actions your scenario already removed.",
                                app.cp_overrides.len()
                            ))
                            .size(11.0)
                            .color(crate::config::ui_color(crate::config::UI_CAUTION)),
                        );
                    }
                    ui.label(
                        egui::RichText::new(match rep.base_cp_us {
                            Some(cp) => format!(
                                "Ranked by evidence: cp-replay first, then bounds, then observations without an estimate. cp-replay compares against the {} dependency model; bound = conditional upper \
                                 bound, no replay. Hover a row for evidence; click to \
                                 navigate.",
                                analyses::fmt_dur(cp)
                            ),
                            None => "No dependency graph: every opportunity is a bound or \
                                     absent.  Reopen once the graph chip turns green for \
                                     CP-replayed numbers."
                                .to_owned(),
                        })
                        .size(11.0),
                    );
                    savings_conditional_note(ui, &app.gstate);
                    ui.separator();
                    const FCOLS: analyses::Columns = &[
                        ("class", 150, 'w'),
                        ("finding", 430, 'w'),
                        ("cost", 100, 'e'),
                        ("opportunity", 110, 'e'),
                        ("estimate", 90, 'w'),
                    ];
                    let t = Tbl {
                        cols: FCOLS,
                        rows: rep
                            .rows
                            .iter()
                            .zip(acts)
                            .enumerate()
                            .map(|(rank, (r, act))| TRow {
                                cells: vec![
                                    r.class.to_owned(),
                                    humanize::mid_ellipsis(&r.title, 80),
                                    analyses::fmt_dur(r.cost_us),
                                    match r.opportunity_us {
                                        Some(o) => analyses::fmt_dur(o),
                                        None => "—".to_owned(),
                                    },
                                    r.estimate.to_owned(),
                                ],
                                keys: vec![
                                    f64::NAN,
                                    f64::NAN,
                                    r.cost_us as f64,
                                    r.opportunity_us.unwrap_or(-1) as f64,
                                    -(rank as f64),
                                ],
                                act: *act,
                                hover: Some(r.detail.clone()),
                            })
                            .collect(),
                    };
                    let key = "findings";
                    let cur = app.sorts.get(key).copied().unwrap_or((4, true));
                    keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                }
            },
        );
        app.panels.findings = open;
    }

    // ---- blast radius ----
    let mut open = app.panels.blast;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Blast radius (recorded descendant cost)",
            "Blast radius (recorded descendant cost)",
            &mut open,
            [1150.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) if !an.reports.blast => spin(ui),
                Some(an) => match &an.blast {
                    None => {
                        ui.label(
                            "Needs the dependency graph — this analysis walks declared deps.  \
                             Reopen once the graph chip turns green.",
                        );
                    }
                    Some(rows) => {
                        if let GState::Ready { quality, .. } = &app.gstate
                            && quality.graph_scope()
                                == Some(crate::types::CaptureScope::CurrentActionAncestors)
                        {
                            ui.label(crate::quality::ACTIVITY_SCOPE_NOTE);
                        }
                        ui.label(
                            egui::RichText::new(
                                "Recorded descendant path cost uses this capture's durations, including cache hits. It is not a warm-build forecast: invalidation, cache reuse and scheduling may change. Downstream action work is summed across parallel actions, not build wall time. Hover for cache-hit coverage; click to navigate.",
                            )
                            .size(11.0),
                        );
                        ui.separator();
                        const BCOLS: analyses::Columns = &[
                            ("target", 420, 'w'),
                            ("own actions", 90, 'e'),
                            ("descendant path", 110, 'e'),
                            ("descendant actions", 100, 'e'),
                            ("recorded work (\u{3a3})", 130, 'e'),
                        ];
                        let t = Tbl {
                            cols: BCOLS,
                            rows: rows
                                .iter()
                                .map(|r| TRow {
                                    cells: vec![
                                        humanize::mid_ellipsis(&r.label, 70),
                                        analyses::fmt_n(r.n_actions as f64),
                                        analyses::fmt_dur(r.warm_wall_us),
                                        analyses::fmt_n(r.downstream_actions as f64),
                                        analyses::fmt_dur(r.downstream_work_us),
                                    ],
                                    keys: vec![
                                        f64::NAN,
                                        r.n_actions as f64,
                                        r.warm_wall_us as f64,
                                        r.downstream_actions as f64,
                                        r.downstream_work_us as f64,
                                    ],
                                    act: Some(r.act as usize),
                                    hover: Some(format!("{} of {} downstream actions were cache hits; their recorded duration is not their cache-miss cost.", r.downstream_cache_hits, r.downstream_actions)),
                                })
                                .collect(),
                        };
                        let key = "blast";
                        let cur = app.sorts.get(key).copied().unwrap_or((2, true));
                        keep_sort(&mut new_sort, key, draw_tbl(ui, key, &t, cur, &mut nav));
                    }
                },
            },
        );
        app.panels.blast = open;
    }
    // ---- upload breakdown (inputs lane) ----
    let mut open = app.panels.upload;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Upload breakdown (inputs lane)",
            "Upload breakdown (inputs lane)",
            &mut open,
            [900.0, 600.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => match &an.upload {
                    None => {
                        ui.label(
                            "This capture has no upload sub-stage data — it predates the \
                             instrumented producer (2026-08-29). The inputs-upload stage is \
                             still shown as one block in the timeline and stage bars.",
                        );
                    }
                    Some((r, tops, race)) => {
                        ui.label(
                            egui::RichText::new(
                                "Where the \"inputs upload\" stage actually goes. The permit \
                                 wait is time queued for one of the client's upload slots \
                                 (buck2's upload semaphore) — dead time, not transfer. Only \
                                 the CAS write moves bytes.",
                            )
                            .size(11.0),
                        );
                        ui.separator();
                        // stacked share bar over the sub-stage totals
                        let total = upload_phase_sum(&r.subs);
                        if total > 0 {
                            let (brect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width().max(300.0), 26.0),
                                egui::Sense::hover(),
                            );
                            let p = ui.painter_at(brect);
                            let mut x = brect.left();
                            for i in 0..crate::upload::SUB_N {
                                if r.subs[i].n_measured == 0 {
                                    continue;
                                }
                                let w = brect.width()
                                    * (r.subs[i].sum_us.max(0) as f64 / total as f64) as f32;
                                let seg = egui::Rect::from_min_size(
                                    egui::pos2(x, brect.top()),
                                    egui::vec2(w, brect.height()),
                                );
                                let [cr, cg, cb] = crate::upload::SUB_COLOR[i];
                                p.rect_filled(seg, 0.0, egui::Color32::from_rgb(cr, cg, cb));
                                x += w;
                            }
                            ui.horizontal_wrapped(|ui| {
                                for i in 0..crate::upload::SUB_N {
                                    let [cr, cg, cb] = crate::upload::SUB_COLOR[i];
                                    ui.colored_label(
                                        egui::Color32::from_rgb(cr, cg, cb),
                                        "\u{25a0}",
                                    );
                                    let s = &r.subs[i];
                                    ui.label(if s.n_measured == 0 {
                                        format!("{} unavailable", crate::upload::SUB_CHIP[i])
                                    } else {
                                        format!(
                                            "{} {} ({:.0}% of recorded phases)",
                                            crate::upload::SUB_CHIP[i],
                                            humanize::dur(s.sum_us as f64),
                                            100.0 * s.sum_us as f64 / total as f64
                                        )
                                    });
                                }
                            });
                            ui.weak("Shares cover recorded phase counters only; partial coverage does not explain the whole upload interval.");
                            ui.separator();
                        }
                        egui::Grid::new("updk").striped(true).show(ui, |ui| {
                            for h in [
                                "sub-stage",
                                "measured / actions; zero",
                                "p50",
                                "p90",
                                "p99",
                                "recorded total",
                            ] {
                                ui.label(egui::RichText::new(h).strong());
                            }
                            ui.end_row();
                            for i in 0..crate::upload::SUB_N {
                                let s = &r.subs[i];
                                ui.label(crate::upload::SUB_LABEL[i]);
                                ui.label(format!(
                                    "{}; {} zero",
                                    upload_coverage(s.n_measured, r.n_upload_actions),
                                    humanize::count(s.n_zero as u64)
                                ));
                                ui.label(upload_duration(s.p50_us, s.n_measured));
                                ui.label(upload_duration(s.p90_us, s.n_measured));
                                ui.label(upload_duration(s.p99_us, s.n_measured));
                                ui.label(upload_duration(s.sum_us, s.n_measured));
                                ui.end_row();
                            }
                        });
                        ui.separator();
                        ui.label(format!(
                            "coverage: {} of {} uploading actions carry the breakdown \
                             (stage total {})",
                            humanize::count(r.n_with_breakdown as u64),
                            humanize::count(r.n_upload_actions as u64),
                            humanize::dur(r.stage_sum_us as f64),
                        ));
                        if r.n_breakdown_without_stage > 0 {
                            ui.label(format!("{} actions have phase measurements without an upload-stage total; excluded from stage coverage.", r.n_breakdown_without_stage));
                        }
                        ui.label(format!(
                            "input digests: {} not in CAS (new content)",
                            upload_digest_count(r.absent, r.absent_measured, r.n_upload_actions),
                        ));
                        if !tops.is_empty() {
                            ui.separator();
                            ui.label(egui::RichText::new("worst permit waits:").strong());
                            egui::Grid::new("uptw").striped(true).show(ui, |ui| {
                                for ((name, act), &(_, wait)) in tops.iter().zip(r.top_wait.iter())
                                {
                                    ui.label(analyses::fmt_dur(wait));
                                    ui.add(egui::Label::new(name).truncate());
                                    match act {
                                        Some(a) => {
                                            if ui.small_button("open \u{25b8}").clicked() {
                                                nav = Some(*a);
                                            }
                                        }
                                        None => {
                                            ui.label("");
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                        }
                        if !race.is_empty() {
                            ui.separator();
                            let gathering: i64 = r.local_race.iter().map(|&(_, g)| g).sum();
                            ui.label(
                                egui::RichText::new(format!(
                                    "uploads that waited while a local attempt prepared the same \
                                     inputs: {} actions, {} gathering local files",
                                    humanize::count(race.len() as u64),
                                    humanize::dur(gathering as f64),
                                ))
                                .strong(),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "buck2 ran these actions locally and remotely at once. While \
                                     the local attempt was getting its inputs onto this machine, \
                                     the remote attempt's upload waited to read the files it had \
                                     to send. The local attempts were cancelled when the remote \
                                     results arrived. Ranked findings estimates the effect on the \
                                     critical path.",
                                )
                                .size(10.5),
                            );
                            egui::Grid::new("uplr").striped(true).show(ui, |ui| {
                                for ((name, act), &(_, gather)) in
                                    race.iter().zip(r.local_race.iter()).take(UPLOAD_RACE_ROWS)
                                {
                                    ui.label(analyses::fmt_dur(gather));
                                    ui.add(egui::Label::new(name).truncate());
                                    match act {
                                        Some(a) => {
                                            if ui.small_button("open \u{25b8}").clicked() {
                                                nav = Some(*a);
                                            }
                                        }
                                        None => {
                                            ui.label("");
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                            if race.len() > UPLOAD_RACE_ROWS {
                                ui.weak(format!(
                                    "and {} more; Analysis \u{2192} Cancelled attempts lists them all",
                                    humanize::count((race.len() - UPLOAD_RACE_ROWS) as u64)
                                ));
                            }
                        }
                        if r.subs[crate::analyses::UploadSub::PHASE_PERMIT_WAIT].n_measured > 0 {
                            ui.separator();
                            ui.label(
                                egui::RichText::new(
                                    "The what-if panel's \"input-upload permit wait eliminated\" \
                                     row replays the CP with this wait removed.",
                                )
                                .size(10.5),
                            );
                        }
                    }
                },
            },
        );
        app.panels.upload = open;
    }
    // ---- stall audit ----
    let mut open = app.panels.stall;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Unaccounted time (stall audit)",
            "Unaccounted time (stall audit)",
            &mut open,
            [1100.0, 640.0],
            |ui| match &app.an {
                None => spin(ui),
                Some(an) => {
                    let rep = &an.stall;
                    ui.label(format!(
                        "instrumentation coverage: {} of {} actions carry stage counters; \
                         {}% of total action time is stage-accounted ({} of {})",
                        humanize::count(rep.with_stages as u64),
                        humanize::count(rep.nacts as u64),
                        (100 * rep.staged / rep.total_dur.max(1)),
                        humanize::dur(rep.staged as f64),
                        humanize::dur(rep.total_dur as f64),
                    ));
                    ui.label(
                        egui::RichText::new(
                            "Cohorts group actions of near-identical span with <5% \
                             stage-counter coverage. The cause is not recorded. \
                             Unattributed time is summed across overlapping actions, \
                             not measured CPU work or recoverable build wall time.",
                        )
                        .size(10.5)
                        .weak(),
                    );
                    ui.separator();
                    egui::Grid::new("stallg")
                        .striped(true)
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            for h in [
                                "Actions",
                                "Typical span",
                                "Window",
                                "Staged",
                                "Unattributed (\u{3a3} actions)",
                                "",
                            ] {
                                ui.label(egui::RichText::new(h).strong());
                            }
                            ui.end_row();
                            for (c, sample) in rep.cohorts.iter().zip(&an.stall_samples) {
                                ui.label(humanize::count(c.n as u64));
                                ui.label(humanize::dur(c.dur_typ as f64));
                                ui.label(format!(
                                    "+{} .. +{}  ({})",
                                    humanize::dur(c.t0 as f64),
                                    humanize::dur(c.t1 as f64),
                                    humanize::dur((c.t1 - c.t0) as f64)
                                ));
                                ui.label(format!("{:.1}%", c.staged_frac * 100.0));
                                ui.label(humanize::dur(c.lost_us as f64));
                                match sample {
                                    Some(a) => {
                                        if ui.small_button("sample \u{25b8}").clicked() {
                                            nav = Some(*a);
                                        }
                                    }
                                    None => {
                                        ui.label("");
                                    }
                                }
                                ui.end_row();
                            }
                            if rep.cohorts.is_empty() {
                                ui.label("no synchronized-stall cohorts found");
                                ui.end_row();
                            }
                        });
                }
            },
        );
        app.panels.stall = open;
    }

    // ---- longest / failed / critical path (action lists) ----
    let mut open = app.panels.longest;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Longest actions",
            "Longest actions",
            &mut open,
            [1200.0, 640.0],
            |ui| {
                let key = "longest";
                let cur = app.sorts.get(key).copied().unwrap_or((2, true));
                keep_sort(
                    &mut new_sort,
                    key,
                    act_table(
                        ui,
                        &app.tr,
                        &app.longest,
                        key,
                        cur,
                        &mut nav,
                        ActTableOpts {
                            origin_us: Some(app.tr.run_origin_us(app.run_id.as_deref())),
                            data: Some(&app.longest_data),
                            ..Default::default()
                        },
                    ),
                );
            },
        );
        app.panels.longest = open;
    }

    let mut open = app.panels.failed;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Failed actions",
            "Failed actions",
            &mut open,
            [1200.0, 640.0],
            |ui| {
                let key = "failed";
                let cur = app.sorts.get(key).copied().unwrap_or((1, false));
                keep_sort(
                    &mut new_sort,
                    key,
                    act_table(
                        ui,
                        &app.tr,
                        &app.failed_list,
                        key,
                        cur,
                        &mut nav,
                        ActTableOpts {
                            origin_us: Some(app.tr.run_origin_us(app.run_id.as_deref())),
                            ..Default::default()
                        },
                    ),
                );
            },
        );
        app.panels.failed = open;
    }

    // ---- retried actions ----
    let mut open = app.panels.retried;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Retried actions",
            "Retried actions",
            &mut open,
            [1100.0, 640.0],
            |ui| match app.an.as_ref().and_then(|a| a.retries.as_ref()) {
                None => {
                    ui.label("No action in this run ran more than once.");
                }
                Some((c, acts)) => {
                    // critical-path members among the re-ran actions, and
                    // what their discarded attempts can have cost the build
                    let on_cp = |act: &Option<usize>| {
                        matches!(
                            (app.cp_set.as_ref(), act),
                            (Some(set), Some(i)) if set.contains(i)
                        )
                    };
                    let (mut cp_n, mut cp_delay, mut cp_dup) = (0usize, 0i64, 0i64);
                    for (r, act) in c.rows.iter().zip(acts) {
                        if on_cp(act) {
                            cp_n += 1;
                            cp_delay += r.delay_us;
                            cp_dup += r.lost_us - r.delay_us;
                        }
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} actions re-ran \u{b7} {} of command time on {} discarded \
                             attempts \u{b7} classified pre-winner elapsed budget {} \
                             (not a build-delay bound)",
                            humanize::count(c.actions as u64),
                            humanize::dur(c.lost_us as f64),
                            humanize::count(c.extra_attempts as u64),
                            humanize::dur(c.delay_us as f64),
                        ))
                        .size(13.0)
                        .strong(),
                    );
                    let reasons = c
                        .by_status
                        .iter()
                        .map(|(k, _, ex)| format!("{ex} {}", k.to_lowercase()))
                        .collect::<Vec<_>>()
                        .join(" \u{b7} ");
                    let mut how: Vec<String> = Vec::new();
                    if c.concurrent > 0 {
                        how.push(format!(
                            "{} overlapped the winner; causal build delay is not established",
                            humanize::count(c.concurrent as u64)
                        ));
                    }
                    if c.rerun > 0 {
                        how.push(format!(
                            "{} ended before the winner started; their summed \
                             elapsed budget is {}",
                            humanize::count(c.rerun as u64),
                            humanize::dur(c.delay_us as f64)
                        ));
                    }
                    if c.unknown > 0 {
                        how.push(format!(
                            "{} cannot be placed against the attempt that won (no \
                             coherent comparable interval recorded)",
                            humanize::count(c.unknown as u64)
                        ));
                    }
                    let count_only = c
                        .extra_attempts
                        .saturating_sub(c.concurrent + c.rerun + c.unknown);
                    if count_only > 0 {
                        how.push(format!(
                            "{} carry only a retry count with no attempt records",
                            humanize::count(count_only as u64)
                        ));
                    }
                    ui.label(format!("reasons: {reasons}   ({})", how.join(" \u{b7} ")));
                    let cp_line = match app.cp_set.as_ref() {
                        None => "critical path: not computed yet \u{2014} this line fills \
                                 in once the graph has loaded"
                            .to_owned(),
                        Some(_) if cp_n == 0 => format!(
                            "captured critical path: none of these {} actions is on it; \
                             indirect effects are not measured",
                            humanize::count(c.actions as u64)
                        ),
                        Some(_) => format!(
                            "captured critical path: {} of these actions are on it (in bold) \u{2014} \
                             {} of classified pre-winner elapsed budget; \
                             {} of other discarded-attempt elapsed time \
                             (overlapping or unclassified; not measured critical-path impact)",
                            humanize::count(cp_n as u64),
                            humanize::dur(cp_delay as f64),
                            humanize::dur(cp_dup as f64)
                        ),
                    };
                    ui.label(egui::RichText::new(cp_line).size(12.0));
                    ui.label(
                        egui::RichText::new(
                            "buck2 reports an attempt as cancelled when its hybrid \
                             executor drops an attempt; a local-vs-remote race is one possible reason. \
                             These are command elapsed times, not worker CPU. Timing can \
                             establish overlap or ordering, but neither proves the effect \
                             of removing an attempt on contention or whole-build delay.",
                        )
                        .size(9.5)
                        .weak(),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .show_rows(
                            ui,
                            ui.spacing().interact_size.y,
                            c.rows.len() + 1,
                            |ui, range| {
                                egui::Grid::new("retried")
                                    .num_columns(4)
                                    .striped(true)
                                    .start_row(range.start)
                                    .min_col_width(70.0)
                                    .min_row_height(ui.spacing().interact_size.y)
                                    .show(ui, |ui| {
                                        if range.start == 0 {
                                            for h in ["Attempts", "Discarded", "Reasons", "Action"]
                                            {
                                                ui.label(egui::RichText::new(h).strong());
                                            }
                                            ui.end_row();
                                        }
                                        for i in range.start.max(1)..range.end {
                                            let (r, act) = (&c.rows[i - 1], &acts[i - 1]);
                                            // critical-path members read in bold
                                            let bold = on_cp(act);
                                            let cell = |t: egui::RichText| {
                                                if bold { t.strong() } else { t }
                                            };
                                            let mut clicked = ui
                                                .add(
                                                    egui::Label::new(cell(
                                                        egui::RichText::new(format!("{}", r.tries))
                                                            .monospace()
                                                            .size(11.0),
                                                    ))
                                                    .sense(egui::Sense::click()),
                                                )
                                                .clicked();
                                            clicked |= ui
                                                .add(
                                                    egui::Label::new(cell(
                                                        egui::RichText::new(humanize::dur(
                                                            r.lost_us as f64,
                                                        ))
                                                        .monospace()
                                                        .size(11.0),
                                                    ))
                                                    .sense(egui::Sense::click()),
                                                )
                                                .clicked();
                                            let why = r
                                                .attempts
                                                .iter()
                                                .zip(&r.fates)
                                                .map(|(at, (ft, basis))| {
                                                    let fate = attempt_fate_label(*ft, *basis);
                                                    let w = format!("{} ({fate})", at.status);
                                                    let el = if at.elastic {
                                                        " on elastic capacity"
                                                    } else {
                                                        ""
                                                    };
                                                    if at.dur_us > 0 {
                                                        format!(
                                                            "{w} after {}{el}",
                                                            humanize::dur(at.dur_us as f64)
                                                        )
                                                    } else {
                                                        format!("{w}{el}")
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .join("; ");
                                            clicked |= ui
                                                .add_sized(
                                                    [360.0, ui.spacing().interact_size.y],
                                                    egui::Label::new(cell(
                                                        egui::RichText::new(&why).size(11.0),
                                                    ))
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                                )
                                                .on_hover_text(&why)
                                                .clicked();
                                            clicked |= ui
                                                .add(
                                                    egui::Label::new(cell(
                                                        egui::RichText::new(act.map_or(
                                                            "(unresolved action)",
                                                            |i| {
                                                                app.tr.names
                                                                    [app.tr.acts[i].name as usize]
                                                                    .as_str()
                                                            },
                                                        ))
                                                        .monospace()
                                                        .size(11.0),
                                                    ))
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                                )
                                                .clicked();
                                            if clicked && let Some(i) = act {
                                                nav = Some(*i);
                                            }
                                            ui.end_row();
                                        }
                                    });
                            },
                        );
                }
            },
        );
        app.panels.retried = open;
    }

    // ---- cancelled attempts (lost local-vs-remote races) ----
    let mut open = app.panels.cancelled;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Cancelled attempts",
            "Cancelled attempts",
            &mut open,
            [1100.0, 640.0],
            |ui| match app.an.as_ref().and_then(|a| a.cancelled.as_ref()) {
                None => {
                    ui.label("No action in this run had an attempt cancelled.");
                }
                Some((c, acts)) => {
                    let on_cp = |act: &Option<usize>| {
                        matches!(
                            (app.cp_set.as_ref(), act),
                            (Some(set), Some(i)) if set.contains(i)
                        )
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{} actions ran locally and remotely at once and had the losing \
                             attempt cancelled \u{b7} {} local, {} remote \u{b7} {} in the stage \
                             each was cancelled in",
                            humanize::count(c.actions as u64),
                            humanize::count(c.local as u64),
                            humanize::count((c.actions - c.local) as u64),
                            humanize::dur(c.cancelled_us as f64),
                        ))
                        .size(13.0)
                        .strong(),
                    );
                    ui.label(format!(
                        "cancelled while: {}",
                        c.by_phase
                            .iter()
                            .map(|(phase, n)| format!(
                                "{} {}",
                                crate::detail::attempt_phase(phase),
                                humanize::count(*n as u64)
                            ))
                            .collect::<Vec<_>>()
                            .join(" \u{b7} ")
                    ));
                    if c.slow_gathers > 0 {
                        ui.label(format!(
                            "{} of these actions' uploads spent at least {} gathering local files \
                             while the local attempt was preparing the same inputs ({} in total); \
                             Upload breakdown lists them and Ranked findings estimates the effect \
                             on the critical path",
                            humanize::count(c.slow_gathers as u64),
                            humanize::dur(analyses::SLOW_GATHER_US as f64),
                            humanize::dur(c.slow_gather_us as f64),
                        ));
                    }
                    let cp_n = acts.iter().filter(|act| on_cp(act)).count();
                    let cp_line = match app.cp_set.as_ref() {
                        None => "critical path: not computed yet \u{2014} this line fills \
                                 in once the graph has loaded"
                            .to_owned(),
                        Some(_) => format!(
                            "captured critical path: {} of these actions are on it (in bold)",
                            humanize::count(cp_n as u64)
                        ),
                    };
                    ui.label(egui::RichText::new(cp_line).size(12.0));
                    ui.label(
                        egui::RichText::new(
                            "The cancelled time ran alongside the attempt that won and is not \
                             part of the action's duration. Inputs a cancelled local attempt \
                             was fetching keep arriving in the background after the cancel, \
                             and use this machine's network and disk.",
                        )
                        .size(9.5)
                        .weak(),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .show_rows(
                            ui,
                            ui.spacing().interact_size.y,
                            c.rows.len() + 1,
                            |ui, range| {
                                egui::Grid::new("cancelled")
                                    .num_columns(6)
                                    .striped(true)
                                    .start_row(range.start)
                                    .min_col_width(70.0)
                                    .min_row_height(ui.spacing().interact_size.y)
                                    .show(ui, |ui| {
                                        if range.start == 0 {
                                            for h in [
                                                "Cancelled",
                                                "Attempt",
                                                "Cancelled while",
                                                "Upload gathering",
                                                "Inputs",
                                                "Action",
                                            ] {
                                                ui.label(egui::RichText::new(h).strong());
                                            }
                                            ui.end_row();
                                        }
                                        for i in range.start.max(1)..range.end {
                                            let (r, act) = (&c.rows[i - 1], &acts[i - 1]);
                                            let bold = on_cp(act);
                                            let mut clicked = false;
                                            let mut cell = |ui: &mut egui::Ui, t: String| {
                                                let t =
                                                    egui::RichText::new(t).monospace().size(11.0);
                                                clicked |= ui
                                                    .add(
                                                        egui::Label::new(if bold {
                                                            t.strong()
                                                        } else {
                                                            t
                                                        })
                                                        .truncate()
                                                        .sense(egui::Sense::click()),
                                                    )
                                                    .clicked();
                                            };
                                            cell(ui, humanize::dur(r.cancelled_us as f64));
                                            cell(
                                                ui,
                                                if r.local { "local" } else { "remote" }.to_owned(),
                                            );
                                            cell(
                                                ui,
                                                crate::detail::attempt_phase(&r.phase).to_owned(),
                                            );
                                            cell(
                                                ui,
                                                r.gather_us.map_or_else(String::new, |g| {
                                                    humanize::dur(g as f64)
                                                }),
                                            );
                                            cell(
                                                ui,
                                                if r.bytes_in > 0 {
                                                    analyses::fmt_bytes(r.bytes_in)
                                                } else {
                                                    String::new()
                                                },
                                            );
                                            cell(
                                                ui,
                                                act.map_or("(unresolved action)", |i| {
                                                    app.tr.names[app.tr.acts[i].name as usize]
                                                        .as_str()
                                                })
                                                .to_owned(),
                                            );
                                            if clicked && let Some(i) = act {
                                                nav = Some(*i);
                                            }
                                            ui.end_row();
                                        }
                                    });
                            },
                        );
                }
            },
        );
        app.panels.cancelled = open;
    }

    // ---- capture health drill-in (the chips' detail view) ----
    let mut open = app.panels.health;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Capture health",
            "Capture health",
            &mut open,
            [760.0, 480.0],
            |ui| {
                if app.tr.health.warnings.is_empty() {
                    ui.label("no capture warnings");
                }
                for w in &app.tr.health.warnings {
                    ui.label(egui::RichText::new(format!("▲ {w}")).size(11.5));
                }
                ui.separator();
                match &app.an {
                    None => spin(ui),
                    Some(an) => {
                        egui::Grid::new("hg")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                let mut row = |k: &str, v: String| {
                                    ui.label(egui::RichText::new(k).strong());
                                    ui.label(v);
                                    ui.end_row();
                                };
                                row(
                                    "second-pass args",
                                    format!(
                                        "{} of {} actions re-decoded",
                                        humanize::count(
                                            app.tr.acts.len().saturating_sub(an.extras_unmatched)
                                                as u64
                                        ),
                                        humanize::count(app.tr.acts.len() as u64)
                                    ),
                                );
                                row(
                                    "stage counters",
                                    format!(
                                        "{} of {} actions ({}% of work attributed)",
                                        humanize::count(an.stall.with_stages as u64),
                                        humanize::count(an.stall.nacts as u64),
                                        100 * an.stall.staged / an.stall.total_dur.max(1)
                                    ),
                                );
                                row("parse", format!("{} ms", app.tr.parse_ms));
                                row("index", format!("{} ms", app.tr.index_ms));
                            });
                    }
                }
                match &app.gstate {
                    GState::Failed(e) => {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!("graph load failed: {e}"))
                                .color(fail_color()),
                        );
                    }
                    GState::Ready { g, quality, .. } => {
                        ui.separator();
                        ui.label(format!(
                            "graph: {} nodes ({} joins), {} edges{}",
                            humanize::count(g.n_rows as u64),
                            humanize::count(g.n_joins as u64),
                            humanize::count(g.n_edges as u64),
                            if quality.graph_complete() {
                                String::new()
                            } else {
                                " — PARTIAL".to_owned()
                            }
                        ));
                        for l in quality.graph_limitations() {
                            ui.label(egui::RichText::new(format!("▲ {l}")).size(11.5));
                        }
                        // what the writer declared beside what was loaded
                        if let Some(gq) = &quality.graph {
                            let n = |v: Option<i64>| match v {
                                Some(n) => humanize::count(n.max(0) as u64),
                                None => "not declared".to_owned(),
                            };
                            egui::Grid::new("gq")
                                .num_columns(3)
                                .striped(true)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("").strong());
                                    ui.label(egui::RichText::new("declared").strong());
                                    ui.label(egui::RichText::new("loaded").strong());
                                    ui.end_row();
                                    ui.label("node rows");
                                    ui.label(n(gq.declared_nodes));
                                    ui.label(humanize::count(gq.loaded_rows as u64));
                                    ui.end_row();
                                    ui.label("edges");
                                    ui.label(n(gq.declared_edges));
                                    ui.label(humanize::count(gq.loaded_edges as u64));
                                    ui.end_row();
                                    ui.label("dangling refs");
                                    ui.label(n(Some(gq.declared_dangling)));
                                    ui.label(humanize::count(gq.undefined));
                                    ui.end_row();
                                    ui.label("dropped lines");
                                    ui.label("—");
                                    ui.label(format!(
                                        "{} ({} inside the file)",
                                        gq.dropped_lines, gq.dropped_interior
                                    ));
                                    ui.end_row();
                                    ui.label("bad rows");
                                    ui.label("—");
                                    ui.label(gq.bad_rows.to_string());
                                    ui.end_row();
                                    if let Some(st) = &quality.stitch {
                                        ui.label("actions with rows");
                                        ui.label(humanize::count(st.n_events as u64));
                                        ui.label(format!(
                                            "{}{}",
                                            humanize::count(st.n_defined as u64),
                                            if st.n_named_only > 0 {
                                                format!(" ({} named only)", st.n_named_only)
                                            } else {
                                                String::new()
                                            }
                                        ));
                                        ui.end_row();
                                    }
                                });
                        }
                    }
                    _ => {}
                }
            },
        );
        app.panels.health = open;
    }

    // ---- keyboard help ----
    let mut open = app.panels.help;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Keyboard shortcuts",
            "Help \u{2014} parts of the window, and keys",
            &mut open,
            [720.0, 720.0],
            |ui| {
                ui.label(
                    egui::RichText::new("Parts of the window")
                        .strong()
                        .size(12.0),
                );
                ui.add_space(2.0);
                egui::Grid::new("parts")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (k, d) in PARTS {
                            ui.label(egui::RichText::new(k).monospace().strong());
                            ui.add(egui::Label::new(d).wrap());
                            ui.end_row();
                        }
                    });
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Keys and gestures").strong().size(12.0));
                ui.add_space(2.0);
                egui::Grid::new("keys")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (k, d) in [
                            ("drag", "pan"),
                            ("scroll / two-finger swipe", "pan vertically / horizontally"),
                            ("\u{21e7}scroll", "pan horizontally (either scroll axis)"),
                            (
                                "Ctrl+Shift+scroll, Cmd+scroll, pinch, or +/-",
                                "zoom time, at the cursor",
                            ),
                            (
                                "Ctrl+scroll or ] / [",
                                "taller / shorter lane rows, at the cursor",
                            ),
                            ("f", "fit the whole trace across the width"),
                            (
                                "\u{21e7}f",
                                "fit its height too \u{2014} every lane on screen",
                            ),
                            (
                                "right-click overview \u{25b8} Reset view",
                                "fit time, restore lane height and scroll to the top",
                            ),
                            (
                                "Cmd/Ctrl+F",
                                "find an action by name, or paste a digest prefix",
                            ),
                            ("n / N", "next / previous Find match"),
                            ("left / right", "pan by a third of a screen"),
                            ("up / down", "scroll lanes"),
                            ("c", "toggle critical-path outline"),
                            ("Backspace", "back to the previously selected action"),
                            ("Esc", "clear selection / close menu"),
                            ("\u{21e7}Esc", "close every popup"),
                            ("? or /", "this window"),
                        ] {
                            ui.label(egui::RichText::new(k).monospace().strong());
                            ui.label(d);
                            ui.end_row();
                        }
                    });
            },
        );
        app.panels.help = open;
    }

    // ---- stage glossary ----
    let mut open = app.panels.glossary;
    if open {
        popup(
            ctx,
            app.native_popups,
            "Stage glossary",
            "Stage glossary",
            &mut open,
            [760.0, 640.0],
            |ui| {
                ui.label(
                    egui::RichText::new(
                        "What each stage of an action's bar means, and who \
                         measured it. The same names appear in legends, \
                         popups, tables, and what-if scenarios; hovering any \
                         chip or popup row shows the same text. One exception \
                         the table cannot show: on a local action the worker \
                         fetch is this machine's materializer, and the popup \
                         says so.",
                    )
                    .weak(),
                );
                ui.separator();
                egui::Grid::new("glossary_stages")
                    .num_columns(2)
                    .striped(true)
                    .min_col_width(150.0)
                    .show(ui, |ui| {
                        for si in 0..crate::detail::STAGE_N {
                            let c = crate::config::stage_color(si)
                                .unwrap_or(egui::Color32::from_gray(0x60));
                            ui.label(
                                egui::RichText::new(format!(
                                    "\u{25a0} {}",
                                    crate::detail::STAGE_LABEL[si]
                                ))
                                .strong()
                                .color(c),
                            );
                            // same order as the row hovers everywhere
                            // else: who measured it, what it is, then
                            // the caveat
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(crate::detail::STAGE_PROV[si].text())
                                            .size(11.0)
                                            .weak(),
                                    )
                                    .wrap(),
                                );
                                ui.add(egui::Label::new(crate::detail::STAGE_HELP[si]).wrap());
                                if let Some(caveat) = crate::detail::STAGE_CAVEAT[si] {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(caveat).size(11.0).weak(),
                                        )
                                        .wrap(),
                                    );
                                }
                            });
                            ui.end_row();
                        }
                    });
                ui.separator();
                egui::Grid::new("glossary_terms")
                    .num_columns(2)
                    .striped(true)
                    .min_col_width(150.0)
                    .show(ui, |ui| {
                        for (term, help) in crate::detail::GLOSSARY_TERMS {
                            ui.label(egui::RichText::new(term).strong());
                            ui.add(egui::Label::new(help).wrap());
                            ui.end_row();
                        }
                    });
            },
        );
        app.panels.glossary = open;
    }

    if retry.get() {
        app.an_error = None;
        app.ensure_analysis();
    }

    if let Some((k, v)) = new_sort {
        app.sorts.insert(k, v);
    }
    if let Some(idx) = nav {
        app.nav_req = Some(idx);
    }
}

fn attempt_fate_label(fate: analyses::AttemptFate, basis: analyses::FateBasis) -> &'static str {
    match (fate, basis) {
        (analyses::AttemptFate::Concurrent, analyses::FateBasis::DurationOverflow) => {
            "overlapped winner (inferred from durations)"
        }
        (analyses::AttemptFate::Concurrent, _) => "overlapped winner",
        (analyses::AttemptFate::Rerun, _) => "ended before winner started",
        (analyses::AttemptFate::Unknown, _) => "ordering unknown",
    }
}

/// The same sentence the JSON envelopes carry when a saving is replayed
/// on a graph with a known gap; the panel may summarise the envelope,
/// not drop it.
fn savings_conditional_note(ui: &mut egui::Ui, gstate: &GState) {
    if let GState::Ready { quality, .. } = gstate
        && quality.graph_scope() == Some(crate::types::CaptureScope::CurrentActionAncestors)
    {
        ui.label(crate::quality::ACTIVITY_SCOPE_NOTE);
    }
    if let GState::Ready { quality, .. } = gstate
        && !quality.graph_complete()
    {
        ui.label(
            egui::RichText::new(format!("▲ {}", crate::quality::SAVINGS_CONDITIONAL))
                .size(11.0)
                .color(crate::config::ui_color(crate::config::UI_CAUTION)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::PARTS;
    use super::keep_sort;

    #[test]
    fn inferred_attempt_overlap_is_labeled_as_inferred() {
        use crate::analyses::AttemptFate;
        use crate::analyses::FateBasis;
        assert_eq!(
            super::attempt_fate_label(AttemptFate::Concurrent, FateBasis::ClientInterval),
            "overlapped winner"
        );
        assert_eq!(
            super::attempt_fate_label(AttemptFate::Concurrent, FateBasis::DurationOverflow),
            "overlapped winner (inferred from durations)"
        );
        assert_eq!(
            super::attempt_fate_label(AttemptFate::Unknown, FateBasis::None),
            "ordering unknown"
        );
    }

    #[test]
    fn upload_formatting_preserves_unknown_zero_and_partial_measurements() {
        assert_eq!(super::upload_duration(0, 0), "unavailable");
        assert_eq!(super::upload_duration(0, 1), "0us");
        assert_eq!(super::upload_coverage(1, 3), "1 / 3 (partial)");
        assert_eq!(super::upload_coverage(3, 3), "3 / 3");
        assert_eq!(super::upload_digest_count(0, 0, 3), "unavailable");
        assert_eq!(
            super::upload_digest_count(0, 1, 3),
            "0 (1 / 3 (partial) actions measured)"
        );
        let measured = crate::upload::SubStat {
            n_measured: 1,
            sum_us: i64::MAX,
            ..Default::default()
        };
        let unknown = crate::upload::SubStat {
            sum_us: i64::MAX,
            ..Default::default()
        };
        assert_eq!(
            super::upload_phase_sum(&[measured.clone(), measured, unknown]),
            2 * i128::from(i64::MAX)
        );
    }

    #[test]
    fn a_later_panel_cannot_erase_a_sort_click() {
        let mut slot = None;
        keep_sort(&mut slot, "breakdown", Some((3, true)));
        keep_sort(&mut slot, "longest", None);
        assert_eq!(
            slot,
            Some(("breakdown", (3, true))),
            "the breakdown click must survive a later panel nobody clicked"
        );
        keep_sort(&mut slot, "longest", Some((1, false)));
        assert_eq!(
            slot,
            Some(("longest", (1, false))),
            "a real click in a later panel still wins"
        );
    }

    /// rustfmt reflows a long string literal by breaking it across lines,
    /// and a break written without a trailing backslash keeps every space
    /// of the new line's indentation inside the string. It compiles, it
    /// lints clean, and it renders as a sentence with a hole punched
    /// through it.
    #[test]
    fn the_glossary_has_no_indentation_stuck_inside_it() {
        for (k, d) in PARTS {
            assert!(!d.contains("  "), "{k}: {d:?}");
            assert!(!d.trim().is_empty(), "{k} has no description");
        }
    }
}
