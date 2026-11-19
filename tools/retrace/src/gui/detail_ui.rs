/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The action-detail popup: identity and timing grids, the stage list
//! and stage bar, plus the analysis-prompt
//! window, the right-click ancestry menu, and
//! the critical-path walker. Everything derived from the record is
//! computed once at open time (`Ready::select`); the paint loop renders
//! pure data.

use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;
use std::hash::Hasher;
use std::time::Instant;

use super::CpOverride;
use super::GState;
use super::Ready;
use super::canvas::stage_color;
use super::canvas::stage_tail;
use super::canvas::stages_from_record;
use super::canvas::unmeasured_color;
use super::execution_ui::ExecutionUi;
use super::load_full;
use super::stagebar;
use super::widgets;
use super::widgets::ActTableOpts;
use super::widgets::act_table;
use super::widgets::fail_color;
use super::widgets::popup;
use super::widgets::popup_raise;
use crate::activity;
use crate::config;
use crate::dag;
use crate::detail;
use crate::humanize;
use crate::types::Trace;

fn flash_color() -> egui::Color32 {
    config::ui_color(config::UI_FLASH)
}

/// A span whose duration came from a what-if rather than a measurement.
const SIM_COLOR: egui::Color32 = egui::Color32::from_rgb(0x9c, 0xb8, 0xdc);

/// Execution summed over many actions, where no single class colour applies.
const EXEC_TOTAL_COLOR: egui::Color32 = egui::Color32::from_gray(0x8c);

/// Text colour for a label painted on `bg`: near-black or near-white, whichever
/// contrasts more with it. Stage and class colours can be configured, so no
/// fixed text colour is readable on all of them.
fn ink_on(bg: egui::Color32) -> egui::Color32 {
    const DARK: egui::Color32 = egui::Color32::from_gray(0x10);
    const LIGHT: egui::Color32 = egui::Color32::from_gray(0xf0);
    if contrast(DARK, bg) >= contrast(LIGHT, bg) {
        DARK
    } else {
        LIGHT
    }
}

/// WCAG contrast ratio between two colours, from 1 (identical) to 21.
fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    fn luminance(c: egui::Color32) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// One open action-detail popup: the raw record plus everything derived
/// from it once, at open time.
pub struct Detail {
    /// index into `Trace::acts`
    pub idx: usize,
    pub rec: serde_json::Value,
    pub rows: Vec<detail::Row>,
    /// Recorded client-clock activity, parsed once; not positioned counters.
    pub activity: activity::Activity,
    pub activity_rows: Vec<ActivityRow>,
    activity_order: Vec<ActivityEntry>,
    pub diagnostics: String,
    pub rank: usize,
    /// graph-era context rows (CP membership/slack, dep counts, peers)
    pub extra: Vec<(String, String)>,
    /// (deps, dependents) at depth 1, join nodes traversed; `None` when
    /// the capture carries no dependency graph
    pub direct: Option<(usize, usize)>,
    /// the direct dependencies' outputs grouped by target, once asked for
    pub inputs: Option<InputsByTarget>,
    /// build-median >10 MiB fetch rate for the nested rate-context line
    pub fetch_med_bps: Option<f64>,
    /// admission-depth estimate for the nested queue-depth line
    pub iu_inflight: Option<i64>,
    pub prompt: Option<Prompt>,
    timing: DetailTimingUi,
    execution: ExecutionUi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StageFocus {
    Stage(usize),
    Remainder,
}

#[derive(Default)]
struct DetailTimingUi {
    initialized: bool,
    selected_stage: Option<StageFocus>,
}

impl DetailTimingUi {
    /// Only force disclosure state on a new detail or a bar activation. A
    /// retained selection must not prevent the arrow from closing the body.
    fn stages_open_request(&self, activated: bool) -> Option<bool> {
        if activated {
            Some(true)
        } else if !self.initialized {
            Some(false)
        } else {
            None
        }
    }
}

#[derive(Default)]
struct StageBarActivation {
    activated: bool,
    stage: Option<StageFocus>,
}

/// One row in the start-ordered table. Gaps are complements of the recorded
/// union only when the activity model says that complement is available.
struct ActivityEntry {
    span: Option<usize>,
    start_us: i64,
    end_us: i64,
}

fn activity_order(a: &activity::Activity) -> Vec<ActivityEntry> {
    let mut rows: Vec<_> = a
        .spans
        .iter()
        .enumerate()
        .map(|(i, s)| ActivityEntry {
            span: Some(i),
            start_us: s.start_us,
            end_us: s.end_us,
        })
        .chain(a.gaps.iter().map(|g| ActivityEntry {
            span: None,
            start_us: g.start_us,
            end_us: g.end_us,
        }))
        .collect();
    rows.sort_unstable_by_key(|r| (r.start_us, r.end_us, r.span));
    rows
}

/// Overlapping observations occupy separate rows, never fabricated attempts.
pub struct ActivityRow {
    pub lane: activity::Lane,
    pub spans: Vec<usize>,
}

fn activity_rows(a: &activity::Activity) -> Vec<ActivityRow> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut rows: Vec<ActivityRow> = Vec::new();
    for lane in [
        activity::Lane::Client,
        activity::Lane::Remote,
        activity::Lane::Local,
    ] {
        let mut spans: Vec<_> = a
            .spans
            .iter()
            .enumerate()
            .filter(|(_, s)| s.lane == lane)
            .map(|(i, _)| i)
            .collect();
        spans.sort_unstable_by_key(|&i| (a.spans[i].start_us, a.spans[i].end_us, i));
        let mut ends: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
        for i in spans {
            let s = &a.spans[i];
            let row = if ends
                .peek()
                .is_some_and(|Reverse((end, _))| *end <= s.start_us)
            {
                let Reverse((_, row)) = ends.pop().unwrap();
                row
            } else {
                rows.push(ActivityRow {
                    lane,
                    spans: Vec::new(),
                });
                rows.len() - 1
            };
            rows[row].spans.push(i);
            ends.push(Reverse((s.end_us, row)));
        }
    }
    rows
}

/// The Copy-analysis-prompt window. Nothing is ever sent: preview and
/// copy only.
pub struct Prompt {
    pub question: String,
    pub body: String,
    pub pending: Option<String>,
}

fn prompt_for(tr: &Trace, d: &Detail, summary: &str, n_events: usize, q: Option<&str>) -> String {
    detail::analysis_prompt(
        &d.rec,
        q.unwrap_or(detail::PROMPT_PRESETS[0]),
        &detail::PromptCtx {
            t0: tr.run_origin_us(
                d.rec
                    .pointer("/args/run")
                    .and_then(serde_json::Value::as_str),
            ),
            rank: d.rank,
            n_events,
            summary,
            platform: detail::sys_platform(),
        },
    )
}

/// Out-params from the popup bodies: they run inside closures that hold
/// `app` immutably, so every mutation is deferred through this struct
/// and applied by the caller afterwards.
#[derive(Default)]
pub struct DetOut {
    pub copied: Option<&'static str>,
    pub close: bool,
    pub anc_req: Option<(usize, bool)>,
    /// (act, up): list the depth-1 dependency frontier
    pub direct_req: Option<(usize, bool)>,
    /// act: group its direct dependencies' outputs by target
    pub inputs_req: Option<usize>,
    pub prompt_open_req: bool,
    pub nav: Option<usize>,
    pub back: bool,
    pub glossary_req: bool,
}

enum RowFilter<'a> {
    In(&'a [&'a str]),
    NotIn(&'a [&'a str]),
}

impl RowFilter<'_> {
    fn keep(&self, key: &str) -> bool {
        match self {
            RowFilter::In(ks) => ks.contains(&key),
            RowFilter::NotIn(ks) => !ks.contains(&key),
        }
    }
}

fn det_row_cells(ui: &mut egui::Ui, row: &detail::Row) {
    ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
        // .extend() = never wrap; without it the label column breaks a
        // two-word key like "Wall time" across two lines
        ui.add(egui::Label::new(egui::RichText::new(row.key).strong().monospace()).extend());
    });
    let mut text = egui::RichText::new(&row.value).monospace().size(11.0);
    if row.failed {
        text = text.color(fail_color());
    }
    ui.add(egui::Label::new(text).wrap());
}

/// The command line in a bounded scroller. A local `cxx_compile` argv
/// wraps to tens of monospace lines, and an uncapped cell pushes every
/// row under it — "Why run" first — off the popup.
fn det_command_cells(ui: &mut egui::Ui, d: &Detail, row: &detail::Row) {
    ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
        ui.add(egui::Label::new(egui::RichText::new(row.key).strong().monospace()).extend());
    });
    let mut text = egui::RichText::new(&row.value).monospace().size(11.0);
    if row.failed {
        text = text.color(fail_color());
    }
    egui::ScrollArea::vertical()
        .id_salt(("cmd", d.idx))
        .max_height(54.0)
        .show(ui, |ui| {
            ui.add(egui::Label::new(text).wrap());
        });
}

fn det_rows_grid(ui: &mut egui::Ui, tr: &Trace, d: &Detail, salt: &str, filter: &RowFilter) {
    egui::Grid::new(("dg", salt))
        .num_columns(2)
        .striped(true)
        .min_col_width(104.0)
        .show(ui, |ui| {
            for row in d.rows.iter().filter(|r| filter.keep(r.key)) {
                match row.key {
                    "Stages" => det_stages_cells(ui, tr, d, row),
                    "Action" => det_command_cells(ui, d, row),
                    _ => det_row_cells(ui, row),
                }
                ui.end_row();
            }
        });
}

/// The Stages row rendered as its own legend: a bar-color chip fronts
/// each line (hover doesn't survive the screenshots these popups end up
/// in), and slow machinery waits paint red + bold. Falls back to the
/// plain text cell when the record has no structured lines.
fn det_stages_cells(ui: &mut egui::Ui, tr: &Trace, d: &Detail, row: &detail::Row) {
    let dur = tr.acts[d.idx].dur;
    let Some(lines) = detail::stage_lines(
        d.rec.get("args").unwrap_or(&serde_json::Value::Null),
        dur,
        d.fetch_med_bps,
        d.iu_inflight,
    ) else {
        det_row_cells(ui, row);
        return;
    };
    ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
        ui.add(egui::Label::new(egui::RichText::new(row.key).strong().monospace()).extend());
    });
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for l in &lines {
            let selected = l.chip
                && l.stage
                    .is_some_and(|si| d.timing.selected_stage == Some(StageFocus::Stage(si)));
            egui::Frame::NONE
                .fill(if selected {
                    ui.visuals().selection.bg_fill
                } else {
                    egui::Color32::TRANSPARENT
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if l.chip {
                            let chip = match l.stage {
                                Some(si) => stage_color(si)
                                    .unwrap_or(config::class_color(tr.acts[d.idx].color as usize)),
                                None => egui::Color32::from_gray(0x9a),
                            };
                            ui.label(egui::RichText::new("\u{25a0}").color(chip).size(11.0));
                        }
                        let mut text = egui::RichText::new(&l.text).monospace().size(11.0);
                        if l.slow {
                            text = text.color(fail_color()).strong();
                        } else if l.bold {
                            text = text.strong();
                        }
                        let r = ui.add(egui::Label::new(text).wrap());
                        if let Some(h) = &l.hover {
                            r.on_hover_text(h);
                        }
                    });
                });
        }
    });
}

/// True when anything survives `filter`, so a caller can skip the
/// "Graph & context" heading rather than print it over an empty grid.
fn det_has_extras(d: &Detail, filter: &RowFilter) -> bool {
    d.extra.iter().any(|(k, _)| filter.keep(k.as_str()))
}

fn det_extras_grid(ui: &mut egui::Ui, d: &Detail, salt: &str, filter: &RowFilter) {
    egui::Grid::new(("dgx", salt))
        .num_columns(2)
        .striped(true)
        .min_col_width(104.0)
        .show(ui, |ui| {
            for (k, v) in d.extra.iter().filter(|(k, _)| filter.keep(k.as_str())) {
                ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(k.as_str()).strong()).extend());
                });
                let response = ui.add(
                    egui::Label::new(egui::RichText::new(v.as_str()).monospace().size(11.0)).wrap(),
                );
                if detail::CONTEXT_STRIP_KEYS.contains(&k.as_str()) {
                    response.on_hover_text("Critical-path membership is not the saving from removing this action; another branch may become critical.");
                }
                ui.end_row();
            }
        });
}

/// This one action's span as a 100%-stacked stage bar (same palette as
/// the timeline and the CP walker); silent on stage-less records.
fn det_stage_bar(ui: &mut egui::Ui, tr: &Trace, d: &Detail) -> StageBarActivation {
    let Some(st) = stages_from_record(&d.rec) else {
        ui.weak("Stage counters not recorded");
        return StageBarActivation::default();
    };
    let dur = tr.acts[d.idx].dur.max(1);
    let sum: i128 = st.iter().map(|&v| v.max(0) as i128).sum();
    let denom = sum.max(dur as i128);
    let marks = detail::slow_stage_marks(dur, &st);
    let (brect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            "Stage totals — show stages",
        )
    });
    let p = ui.painter_at(brect);
    let mut segs: Vec<(Option<usize>, i64, egui::Color32, &str, bool)> = Vec::new();
    for (si, &v) in st.iter().enumerate() {
        segs.push((
            Some(si),
            v,
            stage_color(si).unwrap_or(config::class_color(tr.acts[d.idx].color as usize)),
            detail::STAGE_CHIP[si],
            marks[si] == detail::StageMark::Red,
        ));
    }
    segs.push((
        None,
        (dur as i128 - sum).max(0) as i64,
        stage_tail(),
        "counter remainder",
        false,
    ));
    // instrumented captures: the permit-wait share of the inputs-upload
    // segment shades darker, so the bar itself says how much of the
    // pink is pure queueing
    let sem_wait = detail::re_i64(&d.rec["args"], "iu_sem_wait_us").unwrap_or(0);
    let mut x = brect.left();
    let mut hover: Option<String> = None;
    let mut hot = None;
    segs.retain(|(_, v, _, _, _)| *v > 0);
    let widths = stage_widths(&segs.iter().map(|s| s.1).collect::<Vec<_>>(), brect.width());
    for ((si, v, c, lbl, is_slow), w) in segs.into_iter().zip(widths) {
        // slivers still get ~3px so they exist to the eye and the
        // pointer; the true number lives in the tooltip
        let r = egui::Rect::from_min_max(
            egui::pos2(x, brect.top()),
            egui::pos2(x + w, brect.bottom()),
        );
        p.rect_filled(r, 0.0, c);
        if let Some(pos) = resp.hover_pos() {
            if r.contains(pos) {
                hot = Some(si.map(StageFocus::Stage).unwrap_or(StageFocus::Remainder));
                let mut t = format!(
                    "{lbl} — {} ({}% of shown allocation; composition, not elapsed position)",
                    humanize::dur(v as f64),
                    100i128 * i128::from(v) / denom
                );
                if let Some(si) = si {
                    t.push_str("\n\n");
                    t.push_str(&detail::stage_hover_for_args(si, &d.rec["args"]));
                } else {
                    t.push_str("\n\n");
                    t.push_str(detail::UNACCOUNTED_HELP);
                }
                hover = Some(t);
            }
        }
        if si == Some(detail::STAGE_IU_IDX) && sem_wait > 0 {
            let frac = (sem_wait as f32 / v as f32).clamp(0.0, 1.0);
            let sub = egui::Rect::from_min_max(r.min, egui::pos2(r.left() + w * frac, r.bottom()));
            p.rect_filled(sub, 0.0, darken(c));
            if let Some(pos) = resp.hover_pos() {
                if sub.contains(pos) {
                    hover = Some(format!(
                        "waiting for an upload slot — {} ({} of the upload stage)",
                        humanize::dur(sem_wait as f64),
                        super::stagebar::share(sem_wait.min(v), v)
                    ));
                }
            }
        }
        p.rect_stroke(
            r,
            0.0,
            if is_slow {
                // the red list line's bar segment agrees with the list
                egui::Stroke::new(2.0_f32, fail_color())
            } else {
                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x28))
            },
            egui::StrokeKind::Inside,
        );
        let share = 100 * v as i128 / denom;
        if share >= 12 {
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                format!("{lbl} {share}%"),
                egui::FontId::proportional(10.0),
                ink_on(c),
            );
        }
        x += w;
    }
    if resp.has_focus() {
        p.rect_stroke(
            brect,
            0.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    let activation = StageBarActivation {
        activated: resp.clicked(),
        stage: hot,
    };
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if let Some(txt) = hover {
        resp.on_hover_text(format!(
            "{txt}\n\nClick to show stages. Segment order is not event order."
        ));
    }
    activation
}

/// Give tiny counters a pointer target without growing past the bar. This is
/// a composition graphic, not a time axis; tooltips retain the exact values.
fn stage_widths(values: &[i64], width: f32) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut result = vec![0.0; values.len()];
    let mut remaining = width.max(0.0);
    let mut total: f64 = values.iter().map(|&v| v as f64).sum();
    let floor = 3.0_f32.min(remaining / values.len() as f32);
    let mut order: Vec<_> = (0..values.len()).collect();
    order.sort_unstable_by_key(|&i| values[i]);
    for i in order {
        let w = if total > 0.0 {
            (remaining as f64 * values[i] as f64 / total) as f32
        } else {
            0.0
        };
        result[i] = w.max(floor).min(remaining);
        remaining -= result[i];
        total -= values[i] as f64;
    }
    result
}

/// The permit-wait sub-shade: the segment's own color, dimmed enough to
/// read as "the dead part of this stage" in color and grayscale alike.
fn darken(c: egui::Color32) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c.r() as f32 * 0.70) as u8,
        (c.g() as f32 * 0.70) as u8,
        (c.b() as f32 * 0.70) as u8,
    )
}

/// What a popup body reads, as against the surface it paints on and the
/// `Detail`/`DetOut` it works over. `back` is the name of the action the
/// selection would return to, if there is one.
struct DetCtx<'a> {
    tr: &'a Trace,
    back: Option<&'a str>,
}

/// Every action button in ONE wrapped row.
fn det_buttons(ui: &mut egui::Ui, cx: &DetCtx<'_>, d: &Detail, out: &mut DetOut) {
    let back = cx.back;
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if let Some(prev) = back
            && ui
                .button("\u{25c4} Back")
                .on_hover_text(format!("back to {prev}"))
                .clicked()
        {
            out.back = true;
        }
        if ui.button("Copy analysis prompt…").clicked() {
            out.prompt_open_req = true;
        }
        if let Some((deps, dependents)) = d.direct {
            for (n, up, label, hover) in [
                (deps, true, "Deps", "what this action waited on directly"),
                (
                    dependents,
                    false,
                    "Dependents",
                    "what waited on this action directly",
                ),
            ] {
                if n > 0
                    && ui
                        .button(format!("{label} ({n}) \u{25b8}"))
                        .on_hover_text(hover)
                        .clicked()
                {
                    out.direct_req = Some((d.idx, up));
                }
            }
            if deps > 0
                && d.inputs.is_none()
                && ui
                    .button("Inputs by target")
                    .on_hover_text(
                        "group what the actions this one waited on directly produced by \
                         the target that produced it",
                    )
                    .clicked()
            {
                out.inputs_req = Some(d.idx);
            }
        }
        if ui.button("Ancestors \u{25b8}").clicked() {
            out.anc_req = Some((d.idx, true));
        }
        if ui.button("Descendants \u{25b8}").clicked() {
            out.anc_req = Some((d.idx, false));
        }
        if ui.button("Copy record JSON").clicked() {
            let j = serde_json::to_string_pretty(&d.rec).unwrap_or_default();
            ui.ctx().copy_text(j);
            out.copied = Some("record JSON copied to the clipboard");
        }
        if let Some(command) = d
            .rec
            .get("args")
            .and_then(|a| a.get("digest"))
            .and_then(|v| v.as_str())
            .and_then(crate::execution_fetch::full_input_download_command)
            && ui
                .button("Copy full-input download")
                .on_hover_text("Copies a command that downloads the full input tree for the action row's digest, not the selected command report. Nothing is downloaded by copying.")
                .clicked()
        {
            ui.ctx().copy_text(command);
            out.copied = Some(
                "Full-input download command copied \u{2014} running it downloads the \
                 whole input tree for the action row's digest",
            );
        }
    });
}

fn det_raw_record(ui: &mut egui::Ui, d: &Detail, salt: &str) {
    let n_extra = d
        .rec
        .get("args")
        .and_then(|a| a.as_object())
        .map(|m| {
            m.keys()
                .filter(|k| !detail::covered_arg_keys().contains(&k.as_str()))
                .count()
        })
        .unwrap_or(0);
    let hdr = format!("raw record  ({n_extra} more field(s))");
    egui::CollapsingHeader::new(hdr)
        .open(None)
        .id_salt(("raw", salt))
        .show(ui, |ui| {
            let json = serde_json::to_string_pretty(&d.rec).unwrap_or_default();
            ui.add(
                egui::TextEdit::multiline(&mut json.as_str())
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(12),
            );
        });
}

/// Where the stage rows and the bar send a reader who wants more than
/// a name: the popup itself stays numbers-only, and the definitions
/// live one hover or one click away.
fn det_stage_pointer(ui: &mut egui::Ui, d: &Detail, out: &mut DetOut) {
    if stages_from_record(&d.rec).is_none() {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            egui::RichText::new(
                "hover any stage row or bar segment for its definition and \
                 measurement source",
            )
            .size(10.0)
            .weak(),
        );
        // "?" and not an info glyph: egui ships no font covering
        // U+24D8, so the pretty version renders as tofu
        if ui
            .small_button("?")
            .on_hover_text("open the Stage glossary (also under Help)")
            .clicked()
        {
            out.glossary_req = true;
        }
    });
}

const DET_IDENTITY: [&str; 2] = ["Target", "Category"];
const DET_TIMING: [&str; 2] = ["Started", "Wall time"];
const DET_ACCOUNTING: [&str; 2] = ["Stages", detail::RE_OBSERVATIONS_KEY];

fn activity_color(name: &str) -> egui::Color32 {
    let key = match name {
        "CacheQuery" | "RemoteCache.CheckIdenticalAction" | "RemoteCache.CheckFilteredInputs" => {
            "cache_query_us"
        }
        "LocalCache.CheckIdenticalAction" | "LocalCache.CheckFilteredInputs" => {
            "local_cache_check_us"
        }
        "CacheHit" => "cache_hit_us",
        "Prepare" => "prepare_us",
        "Local.FinalizeOutputs" => "local_finalize_outputs_us",
        "CasArtifact.CheckExpiration"
        | "CasArtifact.ExtendExpiration"
        | "CasArtifact.RecheckExpiration"
        | "CasArtifact.TreeMetadata"
        | "CasArtifact.DeclareOutputs" => "cas_artifact_us",
        "ReUpload" => "input_upload_us",
        "ReSubmit" => "exec_slot_wait_us",
        "Re.WorkerDownload" | "Local.MaterializeInputs" => "materialize_us",
        "Re.WorkerUpload" => "upload_us",
        "Re.Download" => "download_us",
        "Re.Execute" | "Local.Execute" | "Local.WorkerExecute" => "execute_us",
        n if n.starts_with("Re.Queue") || n.ends_with("Queued") => "queue_us",
        _ => "daemon_us",
    };
    detail::STAGES
        .iter()
        .position(|s| s.key == key)
        .and_then(stage_color)
        .unwrap_or_else(stage_tail)
}

/// A real elapsed-time axis. Unlike the counter-composition bar, each mark
/// uses its recorded boundaries and overlapping lanes are not added together.
fn det_activity(ui: &mut egui::Ui, d: &Detail, out: &mut DetOut) {
    let a = &d.activity;
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Recorded client-clock activity").strong());
        if a.evidence != activity::Evidence::Current {
            ui.weak(a.evidence.label());
        }
        if ui.button("Copy RE diagnostics")
            .on_hover_text("Copies the action identity, recorded intervals, and a server-log investigation checklist. Does not query services or download inputs.")
            .clicked() {
            ui.ctx().copy_text(d.diagnostics.clone());
            out.copied = Some("RE diagnostics copied to the clipboard");
        }
    });
    ui.add(egui::Label::new(&a.diagnosis).wrap());
    if let (Some(covered), Some(uncovered)) = (a.covered_us, a.uncovered_us) {
        ui.label(format!(
            "{} with recorded states · {} without a recorded state",
            humanize::dur(covered as f64),
            humanize::dur(uncovered as f64)
        ));
    }
    if !d.activity_rows.is_empty()
        && let Some(dur) = a.duration_us.filter(|&v| v > 0)
    {
        ui.weak("Time since this action started. Rows can overlap; hatching means canceled, not idle. Hover for exact boundaries.");
        let label_w = 130.0_f32.min(ui.available_width() * 0.35);
        let row_h = 22.0;
        egui::ScrollArea::vertical().id_salt(("activity-axis", d.idx))
            .max_height(190.0).show(ui, |ui| {
                let height = (d.activity_rows.len() + 1) as f32 * row_h;
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height), egui::Sense::hover());
                let p = ui.painter_at(rect);
                let left = rect.left() + label_w;
                let width = (rect.right() - left).max(1.0);
                let x = |us: i64| left + (us as f64 / dur as f64).clamp(0.0, 1.0) as f32 * width;
                let foreground = ui.visuals().text_color();
                for (frac, align) in [(0.0, egui::Align2::LEFT_CENTER), (0.5, egui::Align2::CENTER_CENTER), (1.0, egui::Align2::RIGHT_CENTER)] {
                    p.text(egui::pos2(left + frac * width, rect.top() + row_h / 2.0), align,
                        humanize::dur(dur as f64 * frac as f64), egui::FontId::monospace(10.0), foreground);
                }
                let mut hover = None;
                for (r, row) in d.activity_rows.iter().enumerate() {
                    let top = rect.top() + (r + 1) as f32 * row_h;
                    let bottom = top + row_h - 3.0;
                    if bottom < ui.clip_rect().top() || top > ui.clip_rect().bottom() {
                        continue;
                    }
                    let label_rect = egui::Rect::from_min_max(egui::pos2(rect.left(), top), egui::pos2(left - 4.0, bottom));
                    p.with_clip_rect(label_rect).text(egui::pos2(rect.left(), (top + bottom) / 2.0), egui::Align2::LEFT_CENTER,
                        match row.lane {
                            activity::Lane::Client => "Client operations",
                            activity::Lane::Remote => "Remote states",
                            activity::Lane::Local => "Local operations",
                        }, egui::FontId::proportional(11.0), foreground);
                    let track = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(rect.right(), bottom));
                    p.rect_stroke(track, 0.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
                    for &i in &row.spans {
                        let s = &a.spans[i];
                        let r = egui::Rect::from_min_max(egui::pos2(x(s.start_us), top), egui::pos2(x(s.end_us), bottom));
                        let color = activity_color(&s.name);
                        p.rect_filled(r, 0.0, color);
                        if s.start_us == s.end_us {
                            p.vline(r.left(), r.y_range(), egui::Stroke::new(1.0, color));
                        }
                        if s.cancelled && r.width() >= 2.0 { super::widgets::hatch(&p, r); }
                        if r.width() > 100.0 {
                            let label = format!("{}{}", s.label, if s.cancelled { " · canceled" } else { "" });
                            let galley = p.layout_no_wrap(label, egui::FontId::proportional(10.5), ink_on(color));
                            if galley.size().x + 8.0 <= r.width() {
                                p.galley(r.center() - galley.size() / 2.0, galley, ink_on(color));
                            }
                        }
                        if resp.hover_pos().is_some_and(|pos| r.expand(1.0).contains(pos)) {
                            hover = Some(format!("{} ({})\n+{} µs .. +{} µs · {}\n{}\nClient-clock observation; a state ending does not establish worker success or its underlying cause.",
                                s.label, s.name, s.start_us, s.end_us, humanize::dur((s.end_us - s.start_us) as f64),
                                if s.cancelled { "Recorded cancellation" } else { "Recorded state end" }));
                        }
                    }
                }
                if let Some(text) = hover { resp.on_hover_text(text); }
            });
    }
    det_activity_table(ui, &d.activity, &d.activity_order, d.idx);
    egui::CollapsingHeader::new("Evidence and limits")
        .id_salt(("activity-evidence", d.idx))
        .show(ui, |ui| {
            for highlight in &a.highlights {
                ui.add(egui::Label::new(highlight).wrap());
            }
            for explanation in &a.explanations {
                ui.add(egui::Label::new(explanation).wrap());
            }
        });
}

/// Fixed-height, truncated cells make visible-row rendering safe. Full names
/// and exact offsets remain available on hover; sorted data is built once.
fn det_activity_table(
    ui: &mut egui::Ui,
    activity: &activity::Activity,
    rows: &[ActivityEntry],
    salt: usize,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let height = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.text_style_height(&egui::TextStyle::Monospace))
        .max(ui.spacing().interact_size.y);
    egui::ScrollArea::vertical()
        .id_salt(("activity-table", salt))
        .max_height(220.0)
        .show_rows(ui, height, rows.len() + 1, |ui, range| {
            let rendered = range.len();
            let gap = ui.spacing().item_spacing.x;
            let width = (ui.available_width() - 4.0 * gap).max(0.0);
            let widths = [width * 0.38, width * 0.14, width * 0.14, width * 0.14, width * 0.20];
            for index in range {
                ui.push_id(index, |ui| ui.horizontal(|ui| {
                    let cell = |ui: &mut egui::Ui, col: usize, text: egui::RichText| {
                        ui.add_sized([widths[col], height], egui::Label::new(text).truncate())
                    };
                    if index == 0 {
                        for (col, label) in ["Observation", "Start", "End", "Elapsed", "End event"].iter().enumerate() {
                            cell(ui, col, egui::RichText::new(*label).strong());
                        }
                        return;
                    }
                    let row = &rows[index - 1];
                    let (label, end) = if let Some(i) = row.span {
                        let s = &activity.spans[i];
                        (s.label.as_str(), if s.cancelled { "Canceled" } else { "Ended" })
                    } else {
                        ("No recorded state", "Cause not recorded")
                    };
                    let details = if let Some(i) = row.span {
                        format!("{} ({})\n+{} µs .. +{} µs\n{end}; a state ending does not establish worker success.", label, activity.spans[i].name, row.start_us, row.end_us)
                    } else {
                        format!("No retained activity interval covers +{} µs .. +{} µs. Its cause is not recorded; this is not a measured idle or daemon-CPU period.", row.start_us, row.end_us)
                    };
                    cell(ui, 0, egui::RichText::new(label)).on_hover_text(details);
                    for (col, value) in [
                        format!("+{}", activity_time(row.start_us)),
                        format!("+{}", activity_time(row.end_us)),
                        activity_time(row.end_us - row.start_us),
                    ].iter().enumerate() {
                        cell(ui, col + 1, egui::RichText::new(value).monospace()).on_hover_text(value);
                    }
                    cell(ui, 4, egui::RichText::new(end)).on_hover_text(end);
                }));
            }
            rendered
        }).inner
}

fn activity_time(us: i64) -> String {
    if us < 1_000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.3}ms", us as f64 / 1_000.0)
    } else {
        format!("{:.3}s", us as f64 / 1_000_000.0)
    }
}

/// A compact identity header and always-visible counter composition. Recorded
/// chronology is an independent disclosure, never fabricated from totals.
fn det_layout(ui: &mut egui::Ui, cx: &DetCtx<'_>, d: &mut Detail, out: &mut DetOut) {
    let tr = cx.tr;
    fn sec(ui: &mut egui::Ui, t: &str) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(t).strong().size(12.5));
    }
    let header: Vec<_> = DET_IDENTITY
        .iter()
        .chain(DET_TIMING.iter())
        .copied()
        .collect();
    det_rows_grid(ui, tr, d, "header", &RowFilter::In(&header));
    ui.separator();
    egui::CollapsingHeader::new("Chronological activity")
        .id_salt(("activity", d.idx))
        .open((!d.timing.initialized).then_some(false))
        .show(ui, |ui| det_activity(ui, d, out));
    ui.horizontal_wrapped(|ui| {
        ui.strong("Stage totals");
        ui.weak("Counter allocation, not event order").on_hover_text(
            "Counters can overlap and come from different measurement sources. A counter residual is not an observed wait or daemon CPU work. Open Chronological activity for recorded intervals.");
    });
    let activation = det_stage_bar(ui, tr, d);
    if activation.activated {
        d.timing.selected_stage = activation.stage;
    }
    egui::CollapsingHeader::new("Stages")
        .id_salt(("stage-accounting", d.idx))
        .open(d.timing.stages_open_request(activation.activated))
        .show(ui, |ui| {
            ui.label("Stage totals are not event order. Counters can overlap; the remainder is not a measured wait.");
            det_rows_grid(ui, tr, d, "accounting", &RowFilter::In(&DET_ACCOUNTING));
            if d.timing.selected_stage == Some(StageFocus::Remainder) {
                egui::Frame::NONE.fill(ui.visuals().selection.bg_fill).show(ui, |ui| {
                    ui.label("Counter remainder: wall time not covered by these totals; not an observed wait.").on_hover_text(detail::UNACCOUNTED_HELP);
                });
            }
            det_stage_pointer(ui, d, out);
        });
    d.timing.initialized = true;
    sec(ui, "Execution");
    if let Some(copied) = d.execution.show(ui) {
        out.copied = Some(copied);
    }
    let known: Vec<&str> = DET_IDENTITY
        .iter()
        .chain(DET_TIMING.iter())
        .chain(DET_ACCOUNTING.iter())
        .chain(["Action"].iter())
        .copied()
        .collect();
    det_rows_grid(ui, tr, d, "b3", &RowFilter::NotIn(&known));
    let xf = RowFilter::NotIn(&[]);
    if det_has_extras(d, &xf) {
        sec(ui, "Graph & context");
        det_extras_grid(ui, d, "b", &xf);
    }
    if let Some(inputs) = &d.inputs {
        sec(ui, "Inputs by producing target");
        det_inputs(ui, d, inputs, out);
    }
    det_buttons(ui, cx, d, out);
    det_raw_record(ui, d, "b");
}

const INPUTS_BY_TARGET_HELP: &str = "From the recorded action graph: every action this one \
    waited on directly, with everything it produced, grouped by target. An action counts in full \
    even when this one read only some of its outputs, so the total can exceed the recorded input \
    size. Source files, which no action produces, are not listed.";

/// Rows of the inputs-by-target table; the copied TSV has every row.
const INPUT_TARGET_ROWS: usize = 40;

fn det_inputs(ui: &mut egui::Ui, d: &Detail, inputs: &InputsByTarget, out: &mut DetOut) {
    let bytes_in = d
        .rec
        .get("args")
        .and_then(|a| a.get("bytes_in"))
        .and_then(serde_json::Value::as_f64)
        .filter(|&b| b >= 0.0);
    ui.label(format!(
        "{} actions produced {} from {} targets{}",
        humanize::count(inputs.producers as u64),
        crate::analyses::fmt_bytes(inputs.total),
        humanize::count(inputs.groups.len() as u64),
        bytes_in.map_or_else(String::new, |b| format!(
            "; this action's recorded input size is {}",
            crate::analyses::fmt_bytes(b as i64)
        )),
    ));
    ui.label(egui::RichText::new(INPUTS_BY_TARGET_HELP).size(10.5).weak());
    if inputs.no_size > 0 {
        ui.weak(format!(
            "{} of these actions have no recorded output size and count as zero",
            humanize::count(inputs.no_size as u64)
        ));
    }
    egui::Grid::new(("inputs-by-target", d.idx))
        .striped(true)
        .show(ui, |ui| {
            for h in ["Produced", "Share", "Actions", "Target"] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.end_row();
            for (target, n, bytes) in inputs.groups.iter().take(INPUT_TARGET_ROWS) {
                ui.label(crate::analyses::fmt_bytes(*bytes));
                ui.label(format!(
                    "{:.1}%",
                    100.0 * *bytes as f64 / inputs.total.max(1) as f64
                ));
                ui.label(humanize::count(*n as u64));
                ui.add(egui::Label::new(egui::RichText::new(target).monospace()).truncate());
                ui.end_row();
            }
        });
    ui.horizontal_wrapped(|ui| {
        if inputs.groups.len() > INPUT_TARGET_ROWS {
            ui.weak(format!(
                "and {} more targets",
                humanize::count((inputs.groups.len() - INPUT_TARGET_ROWS) as u64)
            ));
        }
        if ui.button("Copy as TSV").clicked() {
            ui.ctx().copy_text(inputs.tsv());
            out.copied = Some("inputs by target copied as TSV");
        }
    });
}

/// What an action took from other actions, grouped by the target that produced it.
pub struct InputsByTarget {
    /// (target, producing actions, their summed recorded outputs), largest first
    pub groups: Vec<(String, usize, i64)>,
    pub producers: usize,
    /// producing actions whose record carries no output size
    pub no_size: usize,
    pub total: i64,
}

impl InputsByTarget {
    fn tsv(&self) -> String {
        let mut out = String::from("produced_bytes\tactions\ttarget\n");
        for (target, n, bytes) in &self.groups {
            out.push_str(&format!("{bytes}\t{n}\t{target}\n"));
        }
        out
    }
}

/// Groups the recorded outputs of `act`'s direct dependencies by the target
/// that produced them, reading each dependency's record from the trace.
pub fn inputs_by_target(tr: &Trace, b: &dag::DagBundle, act: usize) -> InputsByTarget {
    let producers = direct_neighbors(b, act, true);
    let mut by: HashMap<String, (usize, i64)> = HashMap::new();
    let mut no_size = 0;
    for &p in &producers {
        let a = &tr.acts[p];
        let rec = load_full(tr, a.off, a.len);
        let args = rec.as_ref().and_then(|r| r.get("args"));
        let label = args
            .and_then(|a| a.get("target"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&tr.names[a.name as usize]);
        // "cell//pkg:name (category identifier)": the category names the action, not the target
        let target = label.split_once(" (").map_or(label, |(t, _)| t);
        let bytes = args
            .and_then(|a| a.get("bytes_out"))
            .and_then(serde_json::Value::as_f64)
            .filter(|&b| b >= 0.0);
        if bytes.is_none() {
            no_size += 1;
        }
        let group = by.entry(target.to_owned()).or_default();
        group.0 += 1;
        group.1 += bytes.unwrap_or(0.0) as i64;
    }
    let mut groups: Vec<_> = by.into_iter().map(|(t, (n, b))| (t, n, b)).collect();
    groups.sort_by(|x, y| y.2.cmp(&x.2).then_with(|| x.0.cmp(&y.0)));
    InputsByTarget {
        total: groups.iter().map(|g| g.2).sum(),
        groups,
        producers: producers.len(),
        no_size,
    }
}

/// Reachable ACTIONS from `act` through the stitched DAG, upward
/// (ancestors: everything this action waited on) or downward
/// (descendants: everything that waits on it). Join nodes are traversed
/// transparently and never reported.
pub fn ancestry(b: &dag::DagBundle, n_acts: usize, act: usize, up: bool) -> Vec<usize> {
    let d = &b.dag;
    let mut node_of = vec![u32::MAX; n_acts];
    for (n, &a) in d.act_of_node.iter().enumerate() {
        if a != u32::MAX {
            node_of[a as usize] = n as u32;
        }
    }
    let start = node_of[act];
    if start == u32::MAX {
        return Vec::new();
    }
    let (off, arr) = if up {
        (&d.pred_off, &d.preds)
    } else {
        (&d.succ_off, &d.succs)
    };
    let mut seen = vec![false; d.act_of_node.len()];
    seen[start as usize] = true;
    let mut stack = vec![start];
    let mut out = Vec::new();
    while let Some(x) = stack.pop() {
        for &y in &arr[off[x as usize] as usize..off[x as usize + 1] as usize] {
            if !seen[y as usize] {
                seen[y as usize] = true;
                let a = d.act_of_node[y as usize];
                if a != u32::MAX {
                    out.push(a as usize);
                }
                stack.push(y);
            }
        }
    }
    out
}

/// The depth-1 dependency frontier of `act`: the first ACTION reached
/// along every edge, upward (what it waited on) or downward (what waited
/// on it). Join nodes carry no action, so traversal continues through
/// them and stops at the action behind them.
pub fn direct_neighbors(b: &dag::DagBundle, act: usize, up: bool) -> Vec<usize> {
    let Some(start) = b.dag.act_of_node.iter().position(|&a| a == act as u32) else {
        return Vec::new();
    };
    frontier_from(&b.dag, start as u32, up)
}

/// [`direct_neighbors`] for a caller that already resolved the node.
fn frontier_from(d: &dag::Dag, start: u32, up: bool) -> Vec<usize> {
    let (off, arr) = if up {
        (&d.pred_off, &d.preds)
    } else {
        (&d.succ_off, &d.succs)
    };
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack = vec![start];
    let mut out = Vec::new();
    while let Some(x) = stack.pop() {
        for &y in &arr[off[x as usize] as usize..off[x as usize + 1] as usize] {
            let a = d.act_of_node[y as usize];
            if a != u32::MAX {
                out.push(a as usize);
            } else if seen.insert(y) {
                stack.push(y);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Compute + install the depth-1 frontier list for `aidx`, ordered so the
/// action that finished last comes first — on a step that waited, that is
/// the one that actually gated it.
pub fn apply_direct(app: &mut Ready, ctx: &egui::Context, aidx: usize, up: bool) {
    let dagb = match &app.gstate {
        GState::Ready { dag: Some(d), .. } => d.clone(),
        _ => return,
    };
    let Some(act) = app.tr.acts.get(aidx) else {
        return;
    };
    let name = app.tr.names[act.name as usize].clone();
    let short: String = name.chars().take(48).collect();
    let mut acts = direct_neighbors(&dagb, aidx, up);
    let total = acts.len();
    acts.sort_unstable_by_key(|&i| {
        std::cmp::Reverse(app.tr.acts[i].ts.saturating_add(app.tr.acts[i].dur))
    });
    let kind = if up {
        "direct deps"
    } else {
        "direct dependents"
    };
    let scope = ancestry_count_scope(&app.gstate);
    app.anc_set = Some(acts.iter().copied().collect());
    acts.truncate(2000);
    app.anc_title = format!(
        "{kind} of {short} \u{2014} {} {scope}, showing {}, latest finisher first",
        humanize::count(total as u64),
        humanize::count(acts.len() as u64)
    );
    app.anc_list = acts;
    app.sorts.insert("ancestry", (0, false));
    app.anc_window = true;
    popup_raise(ctx, "ancestry");
    app.toast = Some((
        format!(
            "{} {kind} highlighted{} (Esc clears)",
            humanize::count(total as u64),
            ancestry_scope_suffix(&app.gstate)
        ),
        Instant::now(),
    ));
}

/// Compute + install an ancestry highlight/list for `aidx` (shared by
/// the right-click menu and the detail popup's buttons).
pub fn apply_ancestry(app: &mut Ready, ctx: &egui::Context, aidx: usize, up: bool, list: bool) {
    let dagb = match &app.gstate {
        GState::Ready { dag: Some(d), .. } => d.clone(),
        _ => return,
    };
    let Some(act) = app.tr.acts.get(aidx) else {
        return;
    };
    let name = app.tr.names[act.name as usize].clone();
    let short: String = name.chars().take(48).collect();
    let mut acts = ancestry(&dagb, app.tr.acts.len(), aidx, up);
    let total = acts.len();
    acts.sort_unstable_by_key(|&i| std::cmp::Reverse(app.tr.acts[i].dur));
    let kind = if up { "ancestors" } else { "descendants" };
    let scope = ancestry_count_scope(&app.gstate);
    app.anc_set = Some(acts.iter().copied().collect());
    acts.truncate(2000);
    app.anc_title = format!(
        "{kind} of {short} \u{2014} {} {scope}, top {} by duration",
        humanize::count(total as u64),
        humanize::count(acts.len() as u64)
    );
    app.anc_list = acts;
    app.sorts.insert("ancestry", (2, true));
    if list {
        app.anc_window = true;
        popup_raise(ctx, "ancestry");
    }
    app.toast = Some((
        format!(
            "{} {kind} highlighted{} (Esc clears)",
            humanize::count(total as u64),
            ancestry_scope_suffix(&app.gstate)
        ),
        Instant::now(),
    ));
}

fn activity_scope_note(gstate: &GState) -> Option<&'static str> {
    match gstate {
        GState::Ready { quality, .. }
            if quality.graph_scope()
                == Some(crate::types::CaptureScope::CurrentActionAncestors) =>
        {
            Some(crate::quality::ACTIVITY_SCOPE_NOTE)
        }
        _ => None,
    }
}

fn ancestry_count_scope(gstate: &GState) -> &'static str {
    if activity_scope_note(gstate).is_some() {
        "recorded actions in current-action ancestry"
    } else {
        "total"
    }
}

fn ancestry_scope_suffix(gstate: &GState) -> &'static str {
    if activity_scope_note(gstate).is_some() {
        " (recorded actions in current-action ancestry)"
    } else {
        ""
    }
}

/// The right-click context menu, the ancestry list window, the detail
/// popup and the analysis-prompt window. Returns a navigation request.
pub fn show_detail_ui(app: &mut Ready, ctx: &egui::Context) -> Option<usize> {
    let mut nav: Option<usize> = None;
    let mut new_sort: Option<(&'static str, (u8, bool))> = None;

    // ---- right-click context menu (ancestry) ----
    if let Some((aidx, menu)) = app.ctx_menu.filter(|&(aidx, _)| aidx < app.tr.acts.len()) {
        let have_dag = matches!(&app.gstate, GState::Ready { dag: Some(_), .. });
        let name = app.tr.names[app.tr.acts[aidx].name as usize].clone();
        let short: String = name.chars().take(48).collect();
        let mut close = false;
        let mut req: Option<(bool, bool)> = None;
        let (_, dismissed) = widgets::ctx_menu(ctx, "actmenu", menu, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(240.0);
                ui.label(egui::RichText::new(&short).monospace().size(10.5));
                ui.separator();
                for (up, list, label) in [
                    (true, false, "Highlight ancestors"),
                    (false, false, "Highlight descendants"),
                    (true, true, "List ancestors\u{2026}"),
                    (false, true, "List descendants\u{2026}"),
                ] {
                    if ui.add_enabled(have_dag, egui::Button::new(label)).clicked() {
                        req = Some((up, list));
                        close = true;
                    }
                }
                if !have_dag {
                    ui.label(
                        egui::RichText::new("needs the dependency graph")
                            .size(10.0)
                            .weak(),
                    );
                }
                ui.separator();
                if ui.button("Clear highlight").clicked() {
                    app.anc_set = None;
                    app.anc_window = false;
                    close = true;
                }
            });
        });
        if let Some((up, list)) = req {
            apply_ancestry(app, ctx, aidx, up, list);
        }
        if close || dismissed {
            app.ctx_menu = None;
        }
    }

    // ---- ancestry list window ----
    if app.anc_window && !app.anc_list.is_empty() {
        let mut open = app.anc_window;
        let title = app.anc_title.clone();
        popup(
            ctx,
            app.native_popups,
            "ancestry",
            &title,
            &mut open,
            [1200.0, 640.0],
            |ui| {
                if let Some(note) = activity_scope_note(&app.gstate) {
                    ui.label(note);
                }
                let key = "ancestry";
                let cur = app.sorts.get(key).copied().unwrap_or((2, true));
                new_sort = act_table(
                    ui,
                    &app.tr,
                    &app.anc_list,
                    key,
                    cur,
                    &mut nav,
                    ActTableOpts::default(),
                )
                .map(|v| (key, v));
            },
        );
        app.anc_window = open;
    }

    // ---- detail popup ----
    let mut copied: Option<&'static str> = None;
    let mut close_detail = false;
    let mut out = DetOut::default();
    let mut det = app.detail.take();
    if let Some(d) = &mut det {
        if app.native_popups {
            let last = egui::Id::new("detail_last_idx");
            if ctx.data(|dd| dd.get_temp::<usize>(last)) != Some(d.idx) {
                ctx.data_mut(|dd| dd.insert_temp(last, d.idx));
                popup_raise(ctx, "detail");
            }
        }
        let title = d
            .rec
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&app.tr.names[app.tr.acts[d.idx].name as usize])
            .to_owned();
        let tr = app.tr.clone();
        let n_events = app.n_events;
        {
            let mut open = true;
            let back = app.hist.last().map(|&i| {
                let n = &app.tr.names[app.tr.acts[i].name as usize];
                n.chars().take(48).collect::<String>()
            });
            popup(
                ctx,
                app.native_popups,
                "detail",
                &title,
                &mut open,
                [640.0, 660.0],
                |ui| {
                    let cx = DetCtx {
                        tr: &tr,
                        back: back.as_deref(),
                    };
                    det_layout(ui, &cx, d, &mut out);
                },
            );
            if !open {
                close_detail = true;
            }
        }
        if out.prompt_open_req {
            d.prompt = Some(Prompt {
                question: detail::PROMPT_PRESETS[0].to_owned(),
                body: detail::prompt_intro(),
                pending: None,
            });
        }

        // ---- analysis-prompt window ----
        let mut prompt_state = d.prompt.take();
        let mut prompt_open = prompt_state.is_some();
        if let Some(pr) = &mut prompt_state {
            let name = d.rec.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let summary = app.summary.clone();
            popup(
                ctx,
                app.native_popups,
                "prompt",
                &format!("Analysis prompt — {name}"),
                &mut prompt_open,
                [760.0, 460.0],
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Question:");
                        egui::ComboBox::from_id_salt("promptq")
                            .width(420.0)
                            .selected_text(pr.question.clone())
                            .show_ui(ui, |ui| {
                                for p in detail::PROMPT_PRESETS {
                                    ui.selectable_value(&mut pr.question, p.to_owned(), p);
                                }
                            });
                        if ui.button("Preview").clicked() {
                            let q = if pr.question.trim().is_empty() {
                                detail::PROMPT_PRESETS[0].to_owned()
                            } else {
                                pr.question.trim().to_owned()
                            };
                            let p = prompt_for(&tr, d, &summary, n_events, Some(&q));
                            pr.body = format!(
                                "PREVIEW — nothing has been sent.  Press Copy prompt to put \
                                 exactly the text below on the clipboard (review it for \
                                 paths/secrets first):\n\n{p}"
                            );
                            pr.pending = Some(p);
                        }
                        if ui.button("Copy prompt").clicked() {
                            let p = pr.pending.clone().unwrap_or_else(|| {
                                prompt_for(&tr, d, &summary, n_events, Some(&pr.question))
                            });
                            ui.ctx().copy_text(p);
                            copied = Some("analysis prompt copied to the clipboard");
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut pr.question).desired_width(700.0));
                    ui.separator();
                    ui.add(
                        egui::TextEdit::multiline(&mut pr.body.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(22),
                    );
                },
            );
        }
        if prompt_open {
            d.prompt = prompt_state;
        }
        if out.close {
            close_detail = true;
        }
    }
    if close_detail {
        det = None;
        app.clear_selection();
    }
    app.detail = det;
    if out.copied.is_some() {
        copied = out.copied;
    }
    if let Some(n) = out.nav {
        nav = Some(n);
    }
    if let Some((aidx, up)) = out.anc_req {
        apply_ancestry(app, ctx, aidx, up, true);
    }
    if let Some((aidx, up)) = out.direct_req {
        apply_direct(app, ctx, aidx, up);
    }
    if let Some(aidx) = out.inputs_req
        && let GState::Ready {
            dag: Some(dagb), ..
        } = &app.gstate
        && let Some(d) = app.detail.as_mut()
        && d.idx == aidx
    {
        d.inputs = Some(inputs_by_target(&app.tr, dagb, aidx));
    }
    // the pop is what makes Back a step backwards rather than a second
    // entry in the history
    if out.back
        && let Some(prev) = app.hist.pop()
    {
        app.nav_back = true;
        nav = Some(prev);
    }
    if out.glossary_req {
        app.panels.glossary = true;
    }

    if let Some(msg) = copied {
        app.toast = Some((msg.to_owned(), Instant::now()));
    }
    if let Some((k, v)) = new_sort {
        app.sorts.insert(k, v);
    }
    nav
}

/// How many override rows the scroll box shows before it scrolls. The
/// popup must not grow with the scenario: eleven overrides is a normal
/// afternoon and a box that tall pushes the step list off screen.
const OVERRIDE_ROWS_VISIBLE: f32 = 6.0;

/// The savings cell for one override row: `(text, hover)`.
///
/// A pending replay reads `…`, never `0`: zero is a finding here —
/// reverting that override alone does not raise the scenario's CP. This
/// does not establish path membership, so zero must not be labeled off-path.
pub fn margin_label(m: Option<i64>) -> (String, &'static str) {
    match m {
        None => ("\u{2026}".to_owned(), "replaying this row's what-if"),
        Some(0) => (
            "0 s  (no additional CP saving in this scenario)".to_owned(),
            "reverting only this override, while keeping the others, does not raise the \
             simulated critical path; this does not establish whether the action is on that path",
        ),
        Some(v) => (
            format!(
                "{}{}",
                if v > 0 { "\u{2212}" } else { "+" },
                humanize::dur(v.abs() as f64)
            ),
            "what this row still buys, given every other row: the simulated CP without it, \
             minus the simulated CP",
        ),
    }
}

/// The scenario list: one override per row, `×` to drop it, and what
/// that row is still buying given every other row. Returns the index to
/// remove and the action to open, if the user clicked one.
///
/// `margins` is parallel to `rows`; `None` means the replay has not
/// landed.
fn override_rows(
    ui: &mut egui::Ui,
    rows: &[(String, String, Option<usize>)],
    margins: &[Option<i64>],
) -> (Option<usize>, Option<usize>) {
    let mut drop = None;
    let mut open = None;
    let cell_h = ui.spacing().interact_size.y;
    let pitch = cell_h + ui.spacing().item_spacing.y;
    // egui's default bar is floating and fully transparent until the
    // pointer enters the area. Here the bar is the only thing saying the
    // scenario continues past the last visible row, so it has to be a
    // solid one that reserves its own width.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    egui::ScrollArea::vertical()
        .max_height(pitch * OVERRIDE_ROWS_VISIBLE)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("cp_override_rows")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for (k, (name, mode, act)) in rows.iter().enumerate() {
                        if ui
                            .small_button("\u{00d7}")
                            .on_hover_text("drop this step from the scenario")
                            .clicked()
                        {
                            drop = Some(k);
                        }
                        let hit = ui.add(
                            egui::Label::new(egui::RichText::new(name).size(11.5))
                                .sense(egui::Sense::click()),
                        );
                        // the whole-graph row names no action, so it must
                        // not offer to open one
                        let hit = match act {
                            Some(_) => hit
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("open this action"),
                            None => hit,
                        };
                        let mut clicked = hit.clicked();
                        clicked |= ui
                            .add(
                                egui::Label::new(egui::RichText::new(mode).size(11.5).weak())
                                    .sense(egui::Sense::click()),
                            )
                            .clicked();
                        if clicked && let Some(i) = act {
                            open = Some(*i);
                        }
                        let m = margins.get(k).copied().flatten();
                        let (text, hover) = margin_label(m);
                        let mut t = egui::RichText::new(text).size(11.5);
                        if !matches!(m, Some(v) if v != 0) {
                            t = t.weak();
                        }
                        let w = ui.available_width().max(190.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(w, cell_h),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                // a right-to-left ui reports only the label
                                // it wrapped, and the Grid takes that as the
                                // column's width — leaving the striping to
                                // stop short of the numbers it is there to
                                // line up
                                ui.set_min_width(w);
                                ui.label(t).on_hover_text(hover);
                            },
                        );
                        ui.end_row();
                    }
                });
        });
    (drop, open)
}

/// The footer's counter composition and per-action wall reconciliation.
/// Kept independent of rendering so scenario exclusions cannot silently
/// reintroduce aggregate-before-clamping accounting.
#[derive(Default)]
struct CpFooter {
    totals: [i64; detail::STAGE_N],
    accounting: detail::CpAccounting,
    simulated_us: i64,
}

/// Missing cache entries are pending reads, not evidence of missing measurements.
fn cp_stages_pending(
    list: &[usize],
    overridden: &[bool],
    cache: &HashMap<usize, Option<[i64; detail::STAGE_N]>>,
) -> usize {
    list.iter()
        .zip(overridden)
        .filter(|(act, overridden)| !**overridden && !cache.contains_key(*act))
        .count()
}

fn scenario_action_duration(app: &Ready, act: usize) -> i64 {
    app.cp_sim
        .as_ref()
        .and_then(|s| s.path.iter().position(|&i| i == act).map(|i| s.w_us[i]))
        .or_else(|| match app.cp_override_of(act) {
            Some(CpOverride::Node { w_us, .. }) => Some(*w_us),
            _ => None,
        })
        .unwrap_or(app.tr.acts[act].dur)
}

/// The strip and footer must paint/account the same scenario counters.
fn cp_stage_counters(
    mut stages: [i64; detail::STAGE_N],
    free_admission: bool,
) -> [i64; detail::STAGE_N] {
    if free_admission {
        stages[detail::STAGE_SLOT_IDX] = 0;
    }
    stages
}

impl CpFooter {
    fn add_step(
        &mut self,
        duration: i64,
        stages: Option<[i64; detail::STAGE_N]>,
        overridden: bool,
        free_admission: bool,
    ) {
        // An override sets a duration, not its stage composition.
        if overridden {
            self.simulated_us = self.simulated_us.saturating_add(duration.max(0));
            return;
        }
        let stages = stages.map(|stages| {
            // The replay removes this band's cost from each action.
            // Remove its counter before reconciling that action too.
            let stages = cp_stage_counters(stages, free_admission);
            for (sum, value) in self.totals.iter_mut().zip(&stages) {
                *sum = sum.saturating_add(*value);
            }
            stages
        });
        self.accounting
            .add_step(duration, stages.as_ref().map(|stages| stages.as_slice()));
    }

    /// This is a counter-composition denominator, not elapsed wall.
    /// Every positive residual and excess was retained independently.
    fn bar_total(&self) -> i64 {
        self.accounting
            .total
            .saturating_add(self.accounting.overlap)
            .saturating_add(self.simulated_us)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct CpTableFollow {
    selected: Option<usize>,
    layout: u64,
}

impl CpTableFollow {
    fn update(&mut self, selected: usize, layout: u64) -> bool {
        let next = Self {
            selected: Some(selected),
            layout,
        };
        let changed = *self != next;
        *self = next;
        changed
    }
}

fn cp_table_layout_key(list: &[usize], durations: &[i64], simulated: bool) -> u64 {
    let mut key = std::collections::hash_map::DefaultHasher::new();
    list.hash(&mut key);
    durations.hash(&mut key);
    simulated.hash(&mut key);
    // Sorting and background column updates must not steal the user's manual
    // scroll. Follow a new selected step or a changed path, not its row order.
    key.finish()
}

/// The critical-path walker: the CP as a stage-colored strip, one
/// action per segment, a slider + prev/next to step through, the
/// sortable step list, and the where-did-the-time-go totals bar.
pub fn show_cp_walker(app: &mut Ready, ctx: &egui::Context) -> Option<usize> {
    let mut nav: Option<usize> = None;
    let mut new_sort: Option<(&'static str, (u8, bool))> = None;
    if !app.panels.cp || app.cp_list.is_empty() {
        return None;
    }
    app.ensure_cp_savings();
    let sim = app.cp_sim.clone();
    // one row per live override, in the order they were added: with a
    // stack of them the banner's own headline can only carry the total,
    // so the composition has to be readable somewhere. Names keep their
    // tail — every `cxx_compile` of one target shares a prefix and
    // differs only in the source file.
    let rows: Vec<(String, String, Option<usize>)> = app
        .cp_overrides
        .iter()
        .map(|o| match o {
            super::CpOverride::Node { act, label, .. } => (
                humanize::mid_ellipsis(&app.tr.names[app.tr.acts[*act].name as usize], 72),
                label.clone(),
                Some(*act),
            ),
            // the whole-graph override edits no single action, so it has
            // no mode of its own to print in the second column and no
            // action to open
            super::CpOverride::FreeAdmission => {
                (super::FREE_ADMISSION_LABEL.to_owned(), String::new(), None)
            }
        })
        .collect();
    let (list, step_dur) = super::cp_walk_steps(sim.as_ref(), &app.cp_list, &app.tr.acts);
    let free_admission = app.cp_overrides.contains(&super::CpOverride::FreeAdmission);
    // which steps carry a per-step override, parallel to `list`. Only
    // these are drawn and accounted as opaque slabs: a scenario that
    // rewrites the whole graph still has a known composition, so it must
    // not be inferred from "this weight is not the measured one".
    let overridden: Vec<bool> = list
        .iter()
        .map(|&i| app.cp_override_of(i).is_some())
        .collect();
    let mut open = app.panels.cp;
    let mut cp_step = app.cp_step;
    popup(
        ctx,
        app.native_popups,
        "Critical path (dependency graph)",
        "Critical Path",
        &mut open,
        [1200.0, 640.0],
        |ui| {
            if ui.input(|i| i.key_pressed(egui::Key::Escape) && !i.modifiers.shift) {
                app.cp_ctx = None;
            }
            let total = step_dur
                .iter()
                .fold(0i64, |total, &duration| total.saturating_add(duration));
            ui.weak("Action-only dependency model: startup, analysis and installation are shown in Build timeline. Replay savings are model changes, not predictions of whole-command wall time.");
            if let Some(s) = &sim {
                // `replay_path` returns only action nodes and every other
                // node is weight 0, so the simulated path's weights sum to
                // the simulated CP exactly
                if !s.has_consistent_duration() {
                    ui.colored_label(
                        config::ui_color(config::UI_CAUTION),
                        "The simulated path and its duration disagree; reset the scenario to retry.",
                    );
                    if ui.button("Reset scenario").clicked() {
                        app.cp_overrides.clear();
                        app.respawn_cp_sim();
                    }
                    return;
                }
            }
            ui.label(format!(
                "{} steps · {} dependency-model duration",
                list.len(),
                humanize::dur(total as f64),
            ))
            .on_hover_text("Longest weighted chain in the recorded dependency model. Other branches may become critical after a change; this is not a whole-command wall-time prediction.");
            if sim.is_none() && total > app.tr.selected_summary(app.run_id.as_deref()).span_us {
                ui.weak("The model chain exceeds the observed action window: recorded dependencies or modeled serialization overlap in time.");
            }
            if let Some(s) = &sim {
                let saved = s.base_cp_us - s.cp_us;
                let short: String = match s.act {
                    Some(act) => app.tr.names[app.tr.acts[act].name as usize]
                        .chars()
                        .take(48)
                        .collect(),
                    None => String::new(),
                };
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0xdc, 0xe8, 0xf8))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("SIMULATION").strong().size(11.5));
                            // a scenario nobody built out of overrides —
                            // a whole-graph replay — has no rows to name
                            // it, so its own label carries the subject
                            let subject = if rows.is_empty() {
                                format!("{short} {} \u{2014} ", s.label)
                            } else {
                                String::new()
                            };
                            ui.label(
                                egui::RichText::new(format!(
                                    "{subject}simulated CP {} \u{2192} {} ({}{})",
                                    humanize::dur(s.base_cp_us as f64),
                                    humanize::dur(s.cp_us as f64),
                                    if saved >= 0 { "\u{2212}" } else { "+" },
                                    humanize::dur(saved.abs() as f64),
                                ))
                                .size(11.5),
                            );
                            if !rows.is_empty() && ui.small_button("Clear all").clicked() {
                                app.clear_cp_overrides();
                            }
                        });
                        if !rows.is_empty() {
                            let (dropped, opened) = override_rows(ui, &rows, &app.cp_margins);
                            if let Some(k) = dropped {
                                app.remove_cp_override(k);
                            }
                            if let Some(i) = opened {
                                nav = Some(i);
                            }
                            ui.label(
                                egui::RichText::new(
                                    "Rows are leave-one-out marginal savings; they need not add \
                                     to the scenario total.",
                                )
                                .size(9.5)
                                .weak(),
                            );
                        }
                        ui.label(
                            egui::RichText::new(
                                "assumes each action starts the moment its deps finish; all \
                                 other durations as measured \u{2014} compare simulation to \
                                 simulation, never to the measured wall",
                            )
                            .size(9.5)
                            .weak(),
                        );
                    });
            } else if app.cp_sim_rx.is_some() {
                ui.label(egui::RichText::new("simulating\u{2026}").italics().weak());
            }
            if let Some(error) = &app.cp_sim_error {
                ui.label(error);
                if ui.small_button("Clear unavailable scenario").clicked() {
                    app.clear_cp_overrides();
                }
            }
            if let Some(error) = &app.cp_savings_error {
                ui.label(error);
            }
            if let Some(a) = app.an.as_ref().and_then(|an| an.admission.as_ref()) {
                ui.weak(format!("Admission timing coverage: {} of {} action nodes; {} have a positive removable wait.", a.n_recorded, a.n_actions, a.n_modified));
            }
            let free = app.cp_overrides.contains(&CpOverride::FreeAdmission);
            if ui
                .button(if free {
                    "\u{25c9} Admission waits removed"
                } else {
                    "Simulate: admission waits removed"
                })
                .on_hover_text(
                    "replay the critical path with every action's \
                     execution_concurrency_limit wait removed \u{2014} the \
                     \"raise the limit\" what-if (rung-2 captures only). \
                     Stacks with per-step overrides.",
                )
                .clicked()
            {
                app.toggle_cp_free_admission();
            }
            ui.separator();
            let n = list.len();
            if n == 0 {
                ui.label("This scenario has no action steps.");
                return;
            }
            let mut step = app
                .cp_selected
                .and_then(|act| list.iter().position(|&i| i == act))
                .unwrap_or_else(|| cp_step.min(n - 1));
            let was = step;
            let strip_h = 30.0;
            let (srect, sresp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), strip_h),
                egui::Sense::click_and_drag(),
            );
            let sresp = sresp.on_hover_text(
                "click a step to open it \u{2022} drag to pan \u{2022} wheel or pinch to zoom \
                 \u{2022} double-click to fit",
            );
            let p = ui.painter_at(srect);
            let total_dur = total.max(1) as f64;
            // Step bounds in strip units: the fitted layout at 1/1024 of
            // a point, which is what lets the shortest step keep a
            // clickable floor while the whole path is on screen and still
            // leaves the view an integer range to zoom into.
            const UNITS: f64 = 1024.0;
            let minw = 1.5f32;
            let flex = (srect.width() - minw * n as f32).max(0.0);
            let mut u = 0.0f64;
            let mut bounds: Vec<(f64, f64)> = Vec::with_capacity(n);
            for &d in &step_dur {
                let w = f64::from(minw + flex * (d as f64 / total_dur) as f32) * UNITS;
                bounds.push((u, u + w));
                u += w;
            }
            let denom = u.max(1.0) as i64;
            stagebar::gestures(ui, &sresp, srect, denom, &mut app.cp_view);
            let (v0, vw) = app.cp_view.view(denom);
            let scale = f64::from(srect.width()) / vw.max(1.0);
            let sx = |t: f64| srect.left() + ((t - v0) * scale) as f32;
            // Counters for every CP step (once). Their widths show
            // composition, not the chronological positions of stages.
            let cache_start = std::time::Instant::now();
            let mut n_read = 0;
            for &i in &list {
                if !app.stage_cache.contains_key(&i) {
                    if n_read >= 80 || cache_start.elapsed() >= std::time::Duration::from_millis(2)
                    {
                        ctx.request_repaint();
                        break;
                    }
                    let a = app.tr.acts[i];
                    let e = load_full(&app.tr, a.off, a.len)
                        .as_ref()
                        .and_then(stages_from_record);
                    app.stage_cache.insert(i, e);
                    n_read += 1;
                }
            }
            for (k, (&i, &(u0, u1))) in list.iter().zip(&bounds).enumerate() {
                if u1 < v0 || u0 > v0 + vw {
                    continue;
                }
                let left = sx(u0);
                let r = egui::Rect::from_min_max(
                    egui::pos2(left, srect.top() + 3.0),
                    egui::pos2((sx(u1) - 0.5).max(left + 0.5), srect.bottom() - 3.0),
                );
                match app.stage_cache.get(&i).copied().flatten() {
                    // the override sets this step's length, not its
                    // composition; segmenting it would scale measured stages
                    // to a duration they never had
                    _ if overridden[k] => {
                        p.rect_filled(r, 1.0, SIM_COLOR);
                    }
                    Some(st) => {
                        let st = cp_stage_counters(st, free_admission);
                        p.rect_filled(r, 1.0, stage_tail());
                        let sdenom = st.iter().sum::<i64>().max(step_dur[k]).max(1);
                        let sscale = r.width() / sdenom as f32;
                        let mut cx = r.left();
                        for (si, &us) in st.iter().enumerate() {
                            if us == 0 {
                                continue;
                            }
                            let w = (us as f32 * sscale).max(0.5);
                            p.rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(cx, r.top()),
                                    egui::pos2((cx + w).min(r.right()), r.bottom()),
                                ),
                                0.0,
                                stage_color(si)
                                    .unwrap_or(config::class_color(app.tr.acts[i].color as usize)),
                            );
                            cx += w;
                        }
                    }
                    None => {
                        p.rect_filled(r, 1.0, config::class_color(app.tr.acts[i].color as usize));
                    }
                }
                // the outline is what separates one step from the next, so
                // it has to stay legible against every stage colour behind
                // it; a step wide enough to hold it gets the heavier line
                p.rect_stroke(
                    r,
                    1.0,
                    egui::Stroke::new(
                        if r.width() >= 4.0 { 1.5_f32 } else { 1.0_f32 },
                        egui::Color32::from_gray(0x10),
                    ),
                    egui::StrokeKind::Inside,
                );
                if k == step {
                    p.rect_stroke(
                        r.expand(1.5),
                        1.0,
                        egui::Stroke::new(2.0_f32, flash_color()),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            if sresp.clicked() {
                if let Some(pp) = sresp.interact_pointer_pos() {
                    let t = v0 + f64::from(pp.x - srect.left()) / scale;
                    if let Some(k) = strip_step_at(&bounds, t) {
                        step = k;
                        nav = Some(list[k]);
                    }
                }
            }
            if sresp.secondary_clicked() {
                if let Some(pp) = sresp.interact_pointer_pos() {
                    let t = v0 + f64::from(pp.x - srect.left()) / scale;
                    if let Some(k) = strip_step_at(&bounds, t) {
                        app.cp_ctx = Some((list[k], widgets::MenuAt::open(ctx, pp)));
                    }
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.label(
                    egui::RichText::new("counter composition per step:")
                        .size(9.5)
                        .weak(),
                )
                .on_hover_text(
                    "Stage widths are counter shares, not chronological positions. Counters may \
                     overlap; a step is scaled by its counters plus any positive wall residual.",
                );
                for (si, lbl) in detail::STAGE_CHIP.iter().enumerate() {
                    let c = stage_color(si).unwrap_or(EXEC_TOTAL_COLOR);
                    ui.label(
                        egui::RichText::new(format!("\u{25a0} {lbl}"))
                            .size(9.5)
                            .color(c),
                    )
                    .on_hover_text(detail::stage_hover(si, false));
                }
                ui.label(
                    egui::RichText::new("\u{25a0} unaccounted")
                        .size(9.5)
                        .color(egui::Color32::from_gray(0xa0)),
                )
                .on_hover_text(detail::UNACCOUNTED_HELP);
                ui.label(
                    egui::RichText::new("\u{25a7} not measured")
                        .size(9.5)
                        .color(egui::Color32::from_gray(0x70)),
                )
                .on_hover_text(
                    "wall belonging to steps whose records carry no stage counters \u{2014} not \
                     measured, not idle",
                );
                if sim.is_some() {
                    ui.label(
                        egui::RichText::new("\u{25a0} simulated")
                            .size(9.5)
                            .color(SIM_COLOR),
                    );
                }
            });
            ui.horizontal(|ui| {
                let previous = ui
                    .add_enabled(step > 0, egui::Button::new("\u{25c4}"))
                    .on_hover_text("Previous step");
                previous.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        previous.enabled(),
                        "Previous step",
                    )
                });
                if previous.clicked() {
                    step -= 1;
                }
                let mut visible_step = step + 1;
                ui.add(egui::Slider::new(&mut visible_step, 1..=n).text("Step"));
                step = visible_step.saturating_sub(1).min(n - 1);
                let next = ui
                    .add_enabled(step + 1 < n, egui::Button::new("\u{25ba}"))
                    .on_hover_text("Next step");
                next.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, next.enabled(), "Next step")
                });
                if next.clicked() {
                    step += 1;
                }
                if ui.button("Fit").clicked() {
                    app.cp_view.fit();
                }
            });
            if step != was {
                let (u0, u1) = bounds[step];
                app.cp_view.reveal_span(u0 as i64, (u1 - u0) as i64, denom);
            }
            {
                let i = list[step];
                let a = app.tr.acts[i];
                ui.label(
                    egui::RichText::new(&app.tr.names[a.name as usize])
                        .monospace()
                        .size(11.5),
                );
                if ui.button("Open detail \u{25b8}").clicked() {
                    nav = Some(i);
                }
            }
            cp_step = step;
            app.cp_selected = Some(list[step]);
            ui.separator();
            // ---- scrollable middle: the step table (the walker header
            // above and the verdict footer below stay fixed) ----
            // the combined-savings caption sits between the table and the
            // footer, so the table has to give up its line too
            let footer_reserve = if sim.is_none() && app.cp_savings_combined.is_some() {
                162.0
            } else {
                146.0
            };
            let table_h = (ui.available_height() - footer_reserve).max(120.0);
            let exec_map: HashMap<usize, i64> = list
                .iter()
                .filter_map(|&i| {
                    app.stage_cache
                        .get(&i)
                        .copied()
                        .flatten()
                        .map(|st| (i, st[detail::STAGE_EXEC_IDX]))
                })
                .collect();
            let dur_map: Option<HashMap<usize, i64>> = sim
                .as_ref()
                .map(|_| list.iter().copied().zip(step_dur.iter().copied()).collect());
            let key = "cp";
            let cur = app.sorts.get(key).copied().unwrap_or((0, false));
            let layout = cp_table_layout_key(&list, &step_dur, sim.is_some());
            let follow_id = ui.id().with(("cp-table-follow", &app.tr.path, &app.run_id));
            let reveal = ui.ctx().data_mut(|data| {
                data.get_temp_mut_or_default::<CpTableFollow>(follow_id)
                    .update(list[step], layout)
            });
            let mut row_sec: Option<(usize, egui::Pos2)> = None;
            let mut table_nav = None;
            new_sort = act_table(
                ui,
                &app.tr,
                &list,
                key,
                cur,
                &mut table_nav,
                ActTableOpts {
                    origin_us: Some(app.tr.run_origin_us(app.run_id.as_deref())),
                    exec_us: Some(&exec_map),
                    // savings apply to the LIVE critical path; a
                    // simulated path's sensitivities would need a
                    // recompute per simulation
                    savings_us: if sim.is_none() {
                        Some(&app.cp_savings)
                    } else {
                        None
                    },
                    data: None,
                    dur_us: dur_map.as_ref(),
                    sec: Some(&mut row_sec),
                    selected: Some(list[step]),
                    reveal: reveal.then_some(list[step]),
                    max_height: Some(table_h),
                },
            )
            .map(|v| (key, v));
            if let Some(idx) = table_nav {
                if let Some(selected) = list.iter().position(|&act| act == idx) {
                    cp_step = selected;
                    let (u0, u1) = bounds[selected];
                    app.cp_view.reveal_span(u0 as i64, (u1 - u0) as i64, denom);
                }
                nav = Some(idx);
            }
            if let Some((idx, pos)) = row_sec {
                app.cp_ctx = Some((idx, widgets::MenuAt::open(ctx, pos)));
            }
            // The Savings column invites reading down it and adding up.
            // Say what one replay of the top steps together actually
            // returns, so the sum is never the only figure on screen.
            if let (None, Some((sum, together))) = (&sim, app.cp_savings_combined) {
                let clause = match together.cmp(&sum) {
                    std::cmp::Ordering::Less => "they overlap",
                    std::cmp::Ordering::Greater => "they compound",
                    std::cmp::Ordering::Equal => "the two are not related by addition",
                };
                let n_top = app.cp_savings.len().min(super::N_TOP_SAVINGS);
                ui.label(
                    egui::RichText::new(format!(
                        "Top {n_top} steps individually save {}; simulating all {n_top} \
                         together saves {} \u{2014} {clause}.",
                        humanize::dur(sum as f64),
                        humanize::dur(together as f64)
                    ))
                    .size(10.5)
                    .weak(),
                );
            }
            // ---- fixed footer: counters and residuals, reconciled per
            // action. Duration-only counters are not a disjoint timeline. ----
            ui.separator();
            let pending = cp_stages_pending(&list, &overridden, &app.stage_cache);
            if pending > 0 {
                ui.weak(format!(
                    "Loading stage totals\u{2026} {pending} action records remain."
                ));
            } else {
                let mut footer = CpFooter::default();
                for (k, &i) in list.iter().enumerate() {
                    // All unedited steps have now been read. A cached None
                    // means no stage counters were available in that record.
                    footer.add_step(
                        step_dur[k],
                        app.stage_cache.get(&i).copied().flatten(),
                        overridden[k],
                        free_admission,
                    );
                }
                let bar_total = footer.bar_total();
                let CpFooter {
                    totals,
                    accounting: acc,
                    simulated_us: sim_us,
                } = footer;
                let unacc = acc.unaccounted;
                let pct = 100i128 * i128::from(acc.exec)
                    / i128::from(acc.total.saturating_sub(acc.unmeasured).max(1));
                let mut verdict = format!(
                    "{}Of {} critical path, execution counters total {} ({pct}% of wall with stage \
                 data) \u{2014} other stage allocations (counters and derived values) total {}; remaining per-action counter residual {}",
                    if sim.is_some() {
                        "Simulated path \u{2014} "
                    } else {
                        ""
                    },
                    humanize::dur(acc.total as f64),
                    humanize::dur(acc.exec as f64),
                    humanize::dur(acc.waits as f64),
                    humanize::dur(acc.unaccounted as f64)
                );
                if acc.unmeasured > 0 {
                    verdict.push_str(&format!(
                        " \u{2014} {} across {} of {} steps carry no stage counters (not measured, \
                     not idle)",
                        humanize::dur(acc.unmeasured as f64),
                        acc.n_unmeasured,
                        n
                    ));
                }
                if acc.overlap > 0 {
                    verdict.push_str(&format!(
                        " ({} of per-action counter excess, not additional wall time)",
                        humanize::dur(acc.overlap as f64)
                    ));
                }
                let text = egui::RichText::new(verdict).size(14.0).strong();
                // a low percentage is only an alarm when the counters are
                // there to back it: past a fifth of the path uninstrumented
                // it says more about the capture, and under a sim it is not
                // a measurement at all
                ui.label(text).on_hover_text(
                "Residual is summed after clamping wall minus stage counters separately for each \
                 action. Excess is the opposite difference, also clamped per action. Excess may \
                 reflect overlapping or inconsistent counters; neither quantity measures idle \
                 time or an exact interval union.",
            );
                if sim_us > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "excludes the edited step's simulated {} \u{2014} an override does not \
                         say which stage shrank",
                            humanize::dur(sim_us as f64)
                        ))
                        .size(9.5)
                        .weak(),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "Bar shares of {} = {} path wall + {} counter excess; counter composition, \
                     not a timeline.",
                        humanize::dur(bar_total as f64),
                        humanize::dur(total as f64),
                        humanize::dur(acc.overlap as f64),
                    ))
                    .size(9.5)
                    .weak(),
                );
                let (brect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 30.0),
                    egui::Sense::hover(),
                );
                let bp = ui.painter_at(brect);
                let mut bx = brect.left();
                let mut cumulative = 0f64;
                let mut segs: Vec<(i64, egui::Color32, &str, bool)> = Vec::new();
                for (si, &v) in totals.iter().enumerate() {
                    segs.push((
                        v,
                        stage_color(si).unwrap_or(EXEC_TOTAL_COLOR),
                        detail::STAGE_CHIP[si],
                        false,
                    ));
                }
                segs.push((unacc, stage_tail(), "unaccounted", false));
                segs.push((acc.unmeasured, unmeasured_color(), "not measured", true));
                segs.push((sim_us, SIM_COLOR, "simulated", false));
                for (v, c, lbl, hatched) in segs {
                    if v <= 0 {
                        continue;
                    }
                    cumulative += v as f64;
                    // Cumulative fractions share the displayed denominator;
                    // excess never runs off the bar or clips a later segment.
                    let right = brect.left()
                        + (cumulative / bar_total.max(1) as f64 * f64::from(brect.width())) as f32;
                    let r = egui::Rect::from_min_max(
                        egui::pos2(bx, brect.top()),
                        egui::pos2(right, brect.bottom()),
                    );
                    bp.rect_filled(r, 0.0, c);
                    if hatched {
                        super::widgets::hatch(&bp, r);
                    }
                    bp.rect_stroke(
                        r,
                        0.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x28)),
                        egui::StrokeKind::Inside,
                    );
                    let share = 100i128 * i128::from(v) / i128::from(bar_total.max(1));
                    if share >= 6 {
                        bp.text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{lbl} {share}%"),
                            egui::FontId::proportional(11.0),
                            ink_on(c),
                        );
                    }
                    bx = right;
                }
            }
            // ---- what-if context menu (strip bars and table rows) ----
            if let Some((act, menu)) = app.cp_ctx {
                let dur = scenario_action_duration(app, act);
                let short: String = app.tr.names[app.tr.acts[act].name as usize]
                    .chars()
                    .take(48)
                    .collect();
                // the record's own cache-lookup key, read on first use:
                // the stage array cannot say whether the key was there
                if !app.lookup_cache.contains_key(&act) {
                    let a = app.tr.acts[act];
                    let lookup = load_full(&app.tr, a.off, a.len)
                        .as_ref()
                        .and_then(|v| v.get("args"))
                        .and_then(detail::recorded_lookup_us);
                    app.lookup_cache.insert(act, lookup);
                }
                let hit_w: Option<i64> = app.lookup_cache.get(&act).copied().flatten();
                let (hit_enabled, hit_hover, hit_label) = cache_hit_control(hit_w);
                let mut close = false;
                let mut launch: Option<(i64, &str)> = None;
                let mut reset = false;
                let mut cascade = false;
                let overridden = app.cp_override_of(act).is_some();
                let (_, dismissed) = widgets::ctx_menu(ui.ctx(), "cpwhatif", menu, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(230.0);
                        ui.label(egui::RichText::new(&short).monospace().size(10.5));
                        ui.separator();
                        for (label, sim_label, w) in [
                            ("Simulate at 75%", "at 75% of current scenario", (i128::from(dur) * 3 / 4) as i64),
                            ("Simulate at 50%", "at 50%", dur / 2),
                            ("Simulate at 25%", "at 25%", dur / 4),
                            ("Simulate removed", "removed", 0),
                        ] {
                            if ui.button(label).clicked() {
                                launch = Some((w, sim_label));
                                close = true;
                            }
                        }
                        let hit = ui
                            .add_enabled(hit_enabled, egui::Button::new("Simulate as cache hit"))
                            .on_hover_text(hit_hover)
                            .on_disabled_hover_text(hit_hover);
                        if hit.clicked()
                            && let Some(w) = hit_w
                        {
                            launch = Some((w, hit_label));
                            close = true;
                        }
                        if ui
                            .button("Cascade\u{2026}")
                            .on_hover_text(
                                "Open the greedy critical-path experiment for the current scenario. \
                                 It chooses the longest remaining critical-path action after each \
                                 removal; opening it does not change this action or start a replay.",
                            )
                            .clicked()
                        {
                            cascade = true;
                            close = true;
                        }
                        if overridden {
                            ui.separator();
                            if ui
                                .button("Reset this step to measured")
                                .on_hover_text(
                                    "drops this step's override and replays whatever \
                                         else is still simulated",
                                )
                                .clicked()
                            {
                                reset = true;
                                close = true;
                            }
                        }
                    });
                });
                if let Some((w, label)) = launch {
                    app.set_cp_override(act, w, label.to_owned());
                }
                if reset {
                    app.clear_cp_override(act);
                }
                if cascade {
                    app.panels.cascade = true;
                    popup_raise(ctx, super::cascade::TITLE);
                }
                if close || dismissed {
                    app.cp_ctx = None;
                }
            }
        },
    );
    app.panels.cp = open;
    app.cp_step = cp_step;
    if let Some(&act) = list.get(cp_step) {
        app.cp_selected = Some(act);
    }
    if let Some((k, v)) = new_sort {
        app.sorts.insert(k, v);
    }
    nav
}

/// Build the [`Detail`] for act `idx` — called by `Ready::select`.
pub fn build_detail(app: &Ready, idx: usize) -> Option<Detail> {
    let a = app.tr.acts[idx];
    let rec = load_full(&app.tr, a.off, a.len)?;
    let fetch_med_bps = app.an.as_ref().and_then(|an| an.fetch_med_bps);
    let iu_inflight = iu_admission_depth(app, &rec, a.ts);
    let ctx = detail::PopupCtx {
        t0: app.tr.run_origin_us(app.run_id.as_deref()),
        fetch_med_bps,
        iu_inflight,
    };
    let rows = detail::popup_rows(&rec, &ctx);
    // rank = 1 + how many recipe acts are strictly longer (ties break on
    // trace order)
    let key = (a.ts, a.seq);
    let rank = 1 + app
        .tr
        .acts
        .iter()
        .filter(|o| {
            o.flags & crate::types::F_NOT_RECIPE == 0
                && app.tr.action_in_run(o, app.run_id.as_deref())
                && (o.dur > a.dur || (o.dur == a.dur && (o.ts, o.seq) < key))
        })
        .count();

    let mut extra: Vec<(String, String)> = Vec::new();
    if let Some(note) = activity_scope_note(&app.gstate) {
        extra.push(("Graph scope".to_owned(), note.to_owned()));
    }
    let mut direct = None;
    if let GState::Ready { dag: Some(b), .. } = &app.gstate {
        let d = &b.dag;
        let mut node = u32::MAX;
        for (nn, &aa) in d.act_of_node.iter().enumerate() {
            if aa == idx as u32 {
                node = nn as u32;
                break;
            }
        }
        if node != u32::MAX {
            let nn = node as usize;
            let slack = d.slack(nn);
            extra.push((
                "Critical path".into(),
                if app.cp_set.as_ref().is_some_and(|path| path.contains(&idx)) {
                    "on the displayed dependency-model critical path".into()
                } else if slack == 0 {
                    "on a tied longest chain (zero slack), not the displayed path".into()
                } else {
                    format!(
                        "off the recorded dependency-model critical path \u{2014} slack {} \
                         (additional delay before this model's longest path grows)",
                        humanize::dur(slack as f64)
                    )
                },
            ));
            let deps = frontier_from(d, node, true).len();
            let dependents = frontier_from(d, node, false).len();
            direct = Some((deps, dependents));
            extra.push((
                "Graph".into(),
                format!(
                    "{} direct dep(s)\n{} direct dependent(s)",
                    humanize::count(deps as u64),
                    humanize::count(dependents as u64)
                ),
            ));
        }
    }
    // where does this action sit among its category peers? Same
    // distribution and rank rule as the peers table (analyses::rank_among).
    if let Some(an) = &app.an {
        if let Some(cat) = rec
            .get("args")
            .and_then(|a| a.get("src"))
            .and_then(|v| v.as_str())
        {
            let cohort =
                crate::analyses::peer_category(cat, a.flags & crate::types::F_CACHE_HIT != 0);
            if let Some(durs) = an.peer_durs.get(&cohort) {
                if let Some((pct, n, med)) = crate::analyses::rank_among(durs, a.dur) {
                    extra.push((
                        "Among peers".into(),
                        format!(
                            "p{pct} of {} \u{201c}{cohort}\u{201d} actions (median {})",
                            humanize::count(n as u64),
                            humanize::dur(med as f64)
                        ),
                    ));
                }
            }
        }
    }
    let activity = activity::Activity::from_record(&rec);
    let activity_rows = activity_rows(&activity);
    let activity_order = activity_order(&activity);
    let run_id = rec
        .get("args")
        .and_then(|a| a.get("run"))
        .and_then(|r| r.as_str());
    let re_session = app
        .tr
        .runs
        .iter()
        .find(|r| Some(r.run.as_str()) == run_id)
        .and_then(|r| r.re_session.as_deref());
    let diagnostics = activity.diagnostics_text(&rec, re_session);
    let execution = ExecutionUi::new(&app.tr, &rec);
    Some(Detail {
        idx,
        rec,
        rows,
        activity,
        activity_rows,
        activity_order,
        diagnostics,
        rank,
        extra,
        direct,
        inputs: None,
        fetch_med_bps,
        iu_inflight,
        prompt: None,
        timing: DetailTimingUi::default(),
        execution,
    })
}

/// Admission depth for the nested queue-depth line: how many actions
/// were inside their own inputs-upload stage when this one entered it.
/// `None` until the Analysis worker (which owns the global window pass)
/// has finished, and on uninstrumented records — the line only renders
/// where the sub-rows do.
fn iu_admission_depth(app: &Ready, rec: &serde_json::Value, ts: i64) -> Option<i64> {
    let (rep, _, _) = app.an.as_ref()?.upload.as_ref()?;
    let a = rec.get("args")?;
    admission_depth(rep, a, ts)
}

/// Modeled upload occupancy at admission, excluding this action itself.
fn admission_depth(
    rep: &crate::upload::UploadReport,
    a: &serde_json::Value,
    ts: i64,
) -> Option<i64> {
    if !detail::iu_instrumented(a) {
        return None;
    }
    let wait = detail::re_i64(a, "iu_sem_wait_us").unwrap_or(0);
    let st = a.get("stages")?;
    let upload = st.get("input_upload_us")?.as_i64().filter(|v| *v > 0)?;
    let lookup = st
        .get("cache_query_us")
        .and_then(|v| v.as_i64())
        .filter(|v| *v >= 0)
        .unwrap_or(0);
    // These modeled bands include uploading permit holders. This is not a
    // measured semaphore queue. Stay inside this action's half-open window,
    // even if its reported wait reaches or exceeds the upload-stage duration.
    let wait = wait.min(upload - 1);
    Some(
        rep.depth_at(ts.saturating_add(lookup).saturating_add(wait))
            .saturating_sub(1)
            .max(0),
    )
}

/// Step under strip position `t`. Bounds tile [0, end), with no gaps.
/// Pointer hits egui routes beyond either end select the nearest end step.
fn strip_step_at(bounds: &[(f64, f64)], t: f64) -> Option<usize> {
    let last = bounds.len().checked_sub(1)?;
    Some(bounds.iter().position(|&(_, u1)| t < u1).unwrap_or(last))
}

const HIT_HOVER: &str = "replay with this step's duration replaced by its recorded cache-lookup \
     time (stages.cache_query_us). This simplifying lookup-only model excludes request \
     preparation and cached-result handling, even when recorded. It is not a measured hit cost.";

const HIT_DISABLED_HOVER: &str = "this step recorded no cache-lookup time, so a cache-hit replay has \
     no measured cost to stand on and would be the same as removing the step; use Simulate \
     removed for that.";

/// The cache-hit what-if control: offered only when the step recorded a
/// lookup time (a measured zero counts; an absent key does not), with
/// the hover saying exactly what the replay models.
pub(crate) fn cache_hit_control(lookup: Option<i64>) -> (bool, &'static str, &'static str) {
    match lookup {
        Some(_) => (true, HIT_HOVER, "as cache hit (lookup only)"),
        None => (false, HIT_DISABLED_HOVER, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_by_target_groups_direct_dependencies_by_the_target_that_produced_them() {
        let path = crate::temp_path("inputs-by-target.json");
        let row = |name: &str, target: &str, out: &str| {
            format!(
                r#"{{"ph":"X","cat":"recipe","name":"{name}","ts":0,"dur":10,"args":{{"target":"{target}"{out}}}}}"#
            )
        };
        std::fs::write(
            &path,
            format!(
                "[\n{},\n{},\n{},\n{}\n]\n",
                row(
                    "lib (cxx_compile a.cpp)",
                    "fbsource//lib:lib (cxx_compile a.cpp)",
                    r#","bytes_out":300"#
                ),
                row(
                    "lib (cxx_compile b.cpp)",
                    "fbsource//lib:lib (cxx_compile b.cpp)",
                    r#","bytes_out":200"#
                ),
                row(
                    "sdk (http_archive)",
                    "fbsource//toolchain:sdk (http_archive)",
                    ""
                ),
                row(
                    "app (cxx_link app)",
                    "fbsource//app:app (cxx_link app)",
                    r#","bytes_out":9"#
                ),
            ),
        )
        .unwrap();
        let tr = crate::parse::parse(path.to_str().unwrap()).unwrap();
        let act = |name: &str| {
            tr.acts
                .iter()
                .position(|a| tr.names[a.name as usize] == name)
                .unwrap() as u32
        };
        let link = act("app (cxx_link app)");
        let producers = [
            act("lib (cxx_compile a.cpp)"),
            act("lib (cxx_compile b.cpp)"),
            act("sdk (http_archive)"),
        ];
        let b = dag::DagBundle {
            dag: dag::Dag {
                act_of_node: vec![producers[0], producers[1], producers[2], link],
                weight_us: vec![10; 4],
                pred_off: vec![0, 0, 0, 0, 3],
                preds: vec![0, 1, 2],
                succ_off: vec![0, 1, 2, 3, 3],
                succs: vec![3, 3, 3],
                t1: 20,
                cp: 20,
                up: vec![10, 10, 10, 20],
                down: vec![20, 20, 20, 10],
                cp_path: vec![0, 3],
            },
            node_ev: vec![None; 4],
            events: Vec::new(),
        };
        let inputs = inputs_by_target(&tr, &b, link as usize);
        std::fs::remove_file(&path).ok();
        assert_eq!(inputs.producers, 3);
        assert_eq!(
            inputs.groups,
            vec![
                ("fbsource//lib:lib".to_owned(), 2, 500),
                ("fbsource//toolchain:sdk".to_owned(), 1, 0),
            ],
            "both compiles of lib are one target; the category is not part of the target"
        );
        assert_eq!(
            (inputs.total, inputs.no_size),
            (500, 1),
            "the archive recorded no output size"
        );
        assert!(
            inputs
                .tsv()
                .starts_with("produced_bytes\tactions\ttarget\n500\t2\tfbsource//lib:lib\n")
        );
    }

    #[test]
    fn bar_labels_stay_readable_on_every_colour_they_are_painted_on() {
        let mut backgrounds = vec![
            EXEC_TOTAL_COLOR,
            SIM_COLOR,
            stage_tail(),
            unmeasured_color(),
            egui::Color32::BLACK,
            egui::Color32::WHITE,
        ];
        backgrounds.extend((0..detail::STAGE_N).filter_map(stage_color));
        backgrounds.extend((0..config::CLASS_N).map(config::class_color));
        for bg in backgrounds {
            assert!(contrast(ink_on(bg), bg) >= 4.0, "label on {bg:?}");
        }
        assert_eq!(
            ink_on(egui::Color32::from_gray(0x30)),
            egui::Color32::from_gray(0xf0),
            "a dark segment gets light text"
        );
    }

    #[test]
    fn footer_does_not_treat_pending_stage_reads_as_unmeasured_actions() {
        let mut cache = HashMap::new();
        assert_eq!(cp_stages_pending(&[0, 1], &[false, false], &cache), 2);
        cache.insert(0, None);
        assert_eq!(cp_stages_pending(&[0, 1], &[false, false], &cache), 1);
        assert_eq!(cp_stages_pending(&[0, 1], &[false, true], &cache), 0);
        cache.insert(1, Some([0; detail::STAGE_N]));
        assert_eq!(cp_stages_pending(&[0, 1], &[false, false], &cache), 0);
    }

    #[test]
    fn off_path_action_keeps_its_current_override_for_percentage_edits() {
        let trace = crate::parse::parse(&crate::testdata("smoke-trace.json")).unwrap();
        let mut app = Ready::new(trace, None);
        app.set_cp_override(0, 20, "edited".into());
        app.cp_sim = Some(super::super::CpSimView {
            act: Some(0),
            label: "edited".into(),
            base_cp_us: 100,
            cp_us: 50,
            path: vec![1],
            w_us: vec![50],
        });
        assert_eq!(scenario_action_duration(&app, 0), 20);
        assert_eq!(scenario_action_duration(&app, 1), 50);
        assert_eq!(scenario_action_duration(&app, 2), app.tr.acts[2].dur);
    }

    #[test]
    fn strip_hits_include_both_ends_and_preserve_interior_boundaries() {
        assert_eq!(strip_step_at(&[], 0.0), None);
        let bounds = [(0.0, 10.0), (10.0, 25.0)];
        for t in [-2.0, 0.0, 9.9] {
            assert_eq!(strip_step_at(&bounds, t), Some(0));
        }
        for t in [10.0, 25.0, 40.0] {
            assert_eq!(strip_step_at(&bounds, t), Some(1));
        }
    }

    fn upload_windows(ends: Vec<i64>) -> crate::upload::UploadReport {
        crate::upload::UploadReport {
            n_upload_actions: ends.len(),
            n_with_breakdown: 0,
            n_breakdown_without_stage: 0,
            stage_sum_us: 0,
            subs: Default::default(),
            absent: 0,
            absent_measured: 0,
            top_wait: Vec::new(),
            n_waits: 0,
            local_race: Vec::new(),
            iu_starts: vec![0; ends.len()],
            iu_ends: ends,
        }
    }

    #[test]
    fn upload_admission_stays_inside_its_own_half_open_window() {
        let report = upload_windows(vec![100, 1000]);
        for wait in [50, 100, 150] {
            let args = serde_json::json!({
                "stages": {"input_upload_us": 100, "cache_query_us": 0},
                "re": {"iu_sem_wait_us": wait}
            });
            assert_eq!(admission_depth(&report, &args, 0), Some(1));
            assert_eq!(
                admission_depth(&upload_windows(vec![100]), &args, 0),
                Some(0)
            );
        }
    }

    #[test]
    fn upload_admission_ignores_an_invalid_wait() {
        let report = upload_windows(vec![100, 1000]);
        let mut args = serde_json::json!({
            "stages": {"input_upload_us": 100, "cache_query_us": -10},
            "re": {"iu_cas_call_us": 12, "iu_sem_wait_us": 10000,
                   "invalid_counter_fields": ["iu_sem_wait_us"]}
        });
        assert_eq!(admission_depth(&report, &args, 0), Some(1));
        args["re"] = serde_json::json!({"iu_bytes_uploaded": 42});
        assert_eq!(admission_depth(&report, &args, 0), None);
    }

    #[test]
    fn critical_path_table_follow_is_one_shot_for_selection_or_layout_changes() {
        let mut follow = CpTableFollow::default();
        assert!(follow.update(10, 100));
        assert!(!follow.update(10, 100));
        assert!(follow.update(20, 100));
        assert!(!follow.update(20, 100));
        assert!(follow.update(20, 101));
        assert!(!follow.update(20, 101));
    }

    #[test]
    fn critical_path_table_layout_tracks_path_and_replay_not_display_order() {
        let key = cp_table_layout_key(&[10, 20], &[100, 200], false);
        assert_ne!(key, cp_table_layout_key(&[10, 30], &[100, 200], false));
        assert_ne!(key, cp_table_layout_key(&[10, 20], &[100, 50], false));
        assert_ne!(key, cp_table_layout_key(&[10, 20], &[100, 200], true));
        // Display order and column values are deliberately not inputs:
        // sorting or another Savings result must not request another reveal.
        let mut follow = CpTableFollow::default();
        assert!(follow.update(10, key));
        for _ in 0..5 {
            assert!(!follow.update(10, cp_table_layout_key(&[10, 20], &[100, 200], false)));
        }
        assert!(follow.update(20, key));
    }

    #[test]
    fn disclosure_requests_are_one_shot_and_selection_does_not_lock_open() {
        let mut state = DetailTimingUi::default();
        assert_eq!(state.stages_open_request(false), Some(false));
        assert_eq!(state.stages_open_request(true), Some(true));
        state.initialized = true;
        state.selected_stage = Some(StageFocus::Stage(detail::STAGE_FETCH_IDX));
        assert_eq!(state.stages_open_request(true), Some(true));
        assert_eq!(state.stages_open_request(false), None);
    }

    #[test]
    fn counter_slivers_fit_without_overflowing_the_allocation_bar() {
        for (values, width) in [
            (vec![1, 999_999, 1], 640.0),
            (vec![1; detail::STAGE_N], 10.0),
            (vec![i64::MAX; detail::STAGE_N], 640.0),
            (vec![100], 0.0),
        ] {
            let widths = stage_widths(&values, width);
            assert_eq!(widths.len(), values.len());
            assert!(widths.iter().all(|&v| v >= 0.0 && v <= width));
            assert!((widths.iter().sum::<f32>() - width).abs() < 0.001);
        }
        assert!(stage_widths(&[], 640.0).is_empty());
    }

    #[test]
    fn chronology_merges_gaps_in_start_order_without_requiring_absolute_time() {
        let a = activity::Activity::from_record(&serde_json::json!({
            "dur":100,"args":{"activity":{"version":1,"spans":[
                ["Local.Execute",60,10,false], ["Re.WorkerDownload",10,30,true],
                ["Local.MaterializeInputs",30,10,false]
            ]}}
        }));
        let order = activity_order(&a);
        let bounds: Vec<_> = order
            .iter()
            .map(|r| (r.start_us, r.end_us, r.span.is_some()))
            .collect();
        assert_eq!(
            bounds,
            vec![
                (0, 10, false),
                (10, 40, true),
                (30, 40, true),
                (40, 60, false),
                (60, 70, true),
                (70, 100, false)
            ]
        );
        assert_eq!(a.covered_us, Some(40));
    }

    #[test]
    fn chronology_does_not_invent_missing_or_invalid_observations() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"activity":{"version":99,"spans":[]}}),
            serde_json::json!({"activity":{"version":1,"spans":[["Re.Execute",1,-10,false]]}}),
        ] {
            let a = activity::Activity::from_record(&serde_json::json!({"dur":100,"args":args}));
            assert!(activity_order(&a).is_empty());
        }
        let empty = activity::Activity::from_record(
            &serde_json::json!({"dur":100,"args":{"activity":{"version":1,"spans":[]}}}),
        );
        let order = activity_order(&empty);
        assert_eq!(order.len(), 1);
        assert_eq!(
            (order[0].start_us, order[0].end_us, order[0].span),
            (0, 100, None)
        );
    }

    #[test]
    fn chronology_table_only_lays_out_the_visible_rows() {
        let spans: Vec<_> = (0..16_384)
            .map(|i| serde_json::json!(["Re.Execute", i * 2, 1, false]))
            .collect();
        let activity = activity::Activity::from_record(&serde_json::json!({
            "dur":32_769,"args":{"activity":{"version":1,"spans":spans}}
        }));
        let rows = activity_order(&activity);
        assert!(rows.len() >= 32_768);
        let ctx = egui::Context::default();
        let mut shown = 0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            shown = det_activity_table(ui, &activity, &rows, 0);
        });
        output.textures_delta.clear();
        assert!(
            (1..50).contains(&shown),
            "only viewport rows are widgets: {shown}"
        );
    }

    #[test]
    fn observed_activity_rows_preserve_overlap_without_inventing_attempts() {
        let a = activity::Activity::from_record(&serde_json::json!({
            "ts":1000,"dur":100,"args":{"activity":{"version":1,"spans":[
                ["Re.WorkerDownload",0,95,true],
                ["Re.Queue",10,5,false],
                ["Local.MaterializeInputs",80,10,false],
                ["Local.Execute",90,5,false],
                ["Local.Execute",93,2,false]
            ]}}
        }));
        let rows = activity_rows(&a);
        assert_eq!(rows.len(), 4);
        let mut shown: Vec<_> = rows.iter().flat_map(|r| r.spans.iter().copied()).collect();
        shown.sort_unstable();
        assert_eq!(shown, (0..a.spans.len()).collect::<Vec<_>>());
        for row in rows {
            for pair in row.spans.windows(2) {
                assert!(a.spans[pair[0]].end_us <= a.spans[pair[1]].start_us);
            }
        }
        assert_eq!(a.covered_us, Some(95));
    }

    #[test]
    fn cp_footer_preserves_residual_and_uses_counter_composition_shares() {
        let mut first = [0; detail::STAGE_N];
        first[detail::STAGE_EXEC_IDX] = 5_000_000;
        first[detail::STAGE_FETCH_IDX] = 10_000_000;
        let mut second = [0; detail::STAGE_N];
        second[detail::STAGE_EXEC_IDX] = 1_000_000;
        let mut footer = CpFooter::default();
        footer.add_step(10_000_000, Some(first), false, false);
        footer.add_step(10_000_000, Some(second), false, false);
        assert_eq!(footer.accounting.unaccounted, 9_000_000);
        assert_eq!(footer.accounting.overlap, 5_000_000);
        assert_eq!(footer.bar_total(), 25_000_000);
        assert_eq!(
            footer.totals.iter().sum::<i64>()
                + footer.accounting.unaccounted
                + footer.accounting.unmeasured
                + footer.simulated_us,
            footer.bar_total(),
            "all segments fit the displayed denominator, not the 20-second wall"
        );
        assert_eq!(
            100 * footer.accounting.unaccounted / footer.bar_total(),
            36,
            "bar percentages are shares of counter composition, not wall percentages"
        );
    }

    #[test]
    fn cp_footer_distinguishes_zero_counters_from_missing_measurements() {
        let mut footer = CpFooter::default();
        footer.add_step(0, Some([0; detail::STAGE_N]), false, false);
        assert_eq!(footer.bar_total(), 0);
        assert_eq!(footer.accounting.n_unmeasured, 0);
        footer.add_step(10, None, false, false);
        footer.add_step(5, Some([0; detail::STAGE_N]), false, false);
        assert_eq!(footer.bar_total(), 15);
        assert_eq!(footer.accounting.unmeasured, 10);
        assert_eq!(footer.accounting.n_unmeasured, 1);
        assert_eq!(footer.accounting.unaccounted, 5);
        assert_eq!(footer.accounting.overlap, 0);
    }

    #[test]
    fn cp_footer_keeps_overridden_steps_out_of_measured_composition() {
        let mut stages = [0; detail::STAGE_N];
        stages[detail::STAGE_EXEC_IDX] = 7;
        let mut footer = CpFooter::default();
        footer.add_step(10, Some(stages), false, false);
        footer.add_step(4, Some(stages), true, false);
        footer.add_step(2, None, true, false);
        footer.add_step(5, None, false, false);
        assert_eq!(footer.accounting.total, 15);
        assert_eq!(footer.accounting.exec, 7);
        assert_eq!(footer.accounting.unaccounted, 3);
        assert_eq!(footer.accounting.unmeasured, 5);
        assert_eq!(footer.accounting.n_unmeasured, 1);
        assert_eq!(footer.accounting.overlap, 0);
        assert_eq!(footer.simulated_us, 6);
        assert_eq!(footer.bar_total(), 21);
        assert_eq!(footer.totals[detail::STAGE_EXEC_IDX], 7);
    }

    #[test]
    fn cp_footer_removes_free_admission_before_each_steps_reconciliation() {
        let mut first = [0; detail::STAGE_N];
        first[detail::STAGE_EXEC_IDX] = 8;
        first[detail::STAGE_SLOT_IDX] = 4;
        let mut second = [0; detail::STAGE_N];
        second[detail::STAGE_EXEC_IDX] = 1;
        second[detail::STAGE_SLOT_IDX] = 2;
        let mut footer = CpFooter::default();
        // Each action originally lasted 10; the replay removed its own
        // permit wait, not a single aggregate deduction from the path.
        footer.add_step(6, Some(first), false, true);
        footer.add_step(8, Some(second), false, true);
        assert_eq!(footer.accounting.total, 14);
        assert_eq!(footer.accounting.exec, 9);
        assert_eq!(footer.accounting.unaccounted, 7);
        assert_eq!(footer.accounting.overlap, 2);
        assert_eq!(footer.totals[detail::STAGE_SLOT_IDX], 0);
        assert_eq!(footer.simulated_us, 0);
        assert_eq!(footer.bar_total(), 16);
    }

    #[test]
    fn cache_hit_control_refuses_without_a_recorded_lookup() {
        let (enabled, hover, _) = cache_hit_control(None);
        assert!(!enabled);
        assert!(hover.contains("recorded no cache-lookup time"));
        // a measured zero is a measurement, not an absence
        let (enabled, _, label) = cache_hit_control(Some(0));
        assert!(enabled);
        assert_eq!(label, "as cache hit (lookup only)");
        let (enabled, hover, _) = cache_hit_control(Some(300_000));
        assert!(enabled);
        assert!(hover.contains("cached-result handling") && hover.contains("even when recorded"));
    }

    #[test]
    fn a_pending_margin_never_reads_as_zero() {
        let (pending, _) = margin_label(None);
        let (zero, hover) = margin_label(Some(0));
        assert_eq!(pending, "\u{2026}");
        assert_ne!(pending, zero);
        assert!(
            zero.contains("no additional CP saving in this scenario"),
            "zero is a finding, not a blank: {zero}"
        );
        assert!(hover.contains("reverting only this override"));
        assert!(hover.contains("does not establish whether the action is on that path"));
        assert!(!zero.contains("off the simulated path"));
    }

    #[test]
    fn duration_observations_remain_in_accounting_not_execution() {
        assert!(DET_ACCOUNTING.contains(&detail::RE_OBSERVATIONS_KEY));
        assert!(RowFilter::In(&DET_ACCOUNTING).keep(detail::RE_OBSERVATIONS_KEY));
        assert!(!RowFilter::NotIn(&DET_ACCOUNTING).keep(detail::RE_OBSERVATIONS_KEY));
    }

    #[test]
    fn a_margin_reads_as_a_signed_saving() {
        assert_eq!(margin_label(Some(66_000_000)).0, "\u{2212}66 s");
        // an override that made the scenario worse still has to be
        // legible as such
        assert_eq!(margin_label(Some(-1_500_000)).0, "+1.5 s");
    }
}
