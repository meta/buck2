/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The timeline: culled bar painting with in-bar activity coverage, the
//! time ruler, the concurrency strip, the lane gutter, the minimap with
//! its viewport brush, scrollbars, hover/click, and keyboard
//! navigation. Bars carry the parse-time class colors; wide-enough bars
//! re-read their record lazily (budgeted per frame) to draw the
//! positioned activity coverage. Stage counters remain a composition in
//! the detail view, never invented intervals on this time axis.

use std::collections::HashMap;
use std::collections::HashSet;

use super::Ready;
use super::load_full;
use super::widgets;
use super::widgets::fail_color;
use crate::config;
use crate::detail;
use crate::humanize;
use crate::parse::cull;
use crate::types::F_CACHE_HIT;
use crate::types::F_FAILED;
use crate::types::F_NOT_RECIPE;
use crate::types::Trace;
use crate::types::fmt_us;

fn flash_color() -> egui::Color32 {
    config::ui_color(config::UI_FLASH)
}

fn cache_outline() -> egui::Color32 {
    config::ui_color(config::UI_CACHE_OUTLINE)
}

fn cp_outline() -> egui::Color32 {
    config::ui_color(config::UI_CRITICAL_PATH)
}

/// ancestry highlight (right-click ▸ show ancestors/descendants)
pub fn anc_color() -> egui::Color32 {
    config::ui_color(config::UI_ANCESTRY)
}

/// The unaccounted remainder of a segmented bar.
pub fn stage_tail() -> egui::Color32 {
    config::ui_color(config::UI_STAGE_TAIL)
}

/// Wall belonging to actions that carry no stage counters. Darker than
/// [`stage_tail`] and always hatched, because it is the one span on a
/// bar that reports an absence rather than a measurement.
pub fn unmeasured_color() -> egui::Color32 {
    config::ui_color(config::UI_UNMEASURED)
}

/// Minimum bar width that gets activity coverage.
const SEG_MIN_PX: f32 = 8.0;

/// Make room without evicting this frame's visible working set. The interval
/// budget is a soft cap: visible bars may exceed it, but must not reread each
/// other's records on every frame. Off-screen entries are evicted first.
fn make_activity_room(
    cache: &mut HashMap<usize, Vec<(i64, i64, bool)>>,
    total: &mut usize,
    state: &mut [u8],
    incoming: usize,
    cap: usize,
    on_screen: &HashSet<usize>,
) {
    while total.saturating_add(incoming) > cap {
        let Some(key) = cache.keys().copied().find(|key| !on_screen.contains(key)) else {
            break;
        };
        *total -= cache
            .remove(&key)
            .expect("should remove a key just read from the map")
            .len();
        state[key] = 0;
    }
}

fn interval_pixels(start: f64, end: f64, left: f64, upp: f64, width: f32) -> Option<(f32, f32)> {
    let x0 = ((start - left) / upp).clamp(-4.0, f64::from(width) + 4.0);
    let x1 = ((end - left) / upp).clamp(-4.0, f64::from(width) + 4.0);
    (x1 > x0).then_some((x0 as f32, x1 as f32))
}

/// Width of the lane-number gutter to the left of the plot.
pub const GUTTER_W: f32 = 54.0;

/// Fill for the column left of the plot. The canvas and the minimap strip
/// both paint it, so that column reads as one gutter running the height of
/// the window rather than two panels that happen to agree.
pub fn gutter_fill() -> egui::Color32 {
    config::ui_color(config::UI_GUTTER)
}

/// Width of a scrollbar; the plot stops one of these short of the right
/// edge, and the horizontal bar is one of these tall.
pub const SB_W: f32 = 12.0;
/// The plot width in the default 1500-point window: the window less the
/// gutter and the vertical scrollbar. Only a seed — every painted frame
/// overwrites `canvas_x0`/`canvas_w` with the real geometry.
pub const DEFAULT_CANVAS_W: f32 = 1500.0 - GUTTER_W - SB_W;
pub const DEFAULT_CANVAS_H: f32 = 800.0;

/// Lane row height limits for ctrl-scroll / `[` / `]`. The floor is the
/// smallest row that still paints a distinguishable bar; the ceiling
/// keeps a 57k-lane trace from needing a scroll per row. The config
/// file's `row_height` is held to the same pair.
pub const ROW_H_MIN: f32 = config::ROW_HEIGHT_MIN;
pub const ROW_H_MAX: f32 = config::ROW_HEIGHT_MAX;

/// Configured color for a valid `detail::STAGES` index. Execution returns
/// `None` because it uses the action's own class color.
pub fn stage_color(i: usize) -> Option<egui::Color32> {
    config::stage_color(i)
}

/// Counter allocation in `detail::STAGE_ORDER`, not chronological intervals.
/// `None` means no usable stage counters; explicitly recorded zero is retained.
pub fn stages_from_record(v: &serde_json::Value) -> Option<[i64; detail::STAGE_N]> {
    let st = detail::stages_of(v.get("args")?)?;
    Some(st)
}

/// Concurrency profile: how many actions are in flight over time, at a
/// fixed bucket resolution across the whole trace (viewport-sampled when
/// drawn). Difference-array build: O(acts), not O(acts x buckets).
const CONC_BUCKETS: usize = 8192;

pub fn build_concurrency(tr: &Trace) -> (Vec<u32>, u32) {
    let span = tr.span.max(1) as f64;
    let mut diff = vec![0i32; CONC_BUCKETS + 1];
    for a in &tr.acts {
        if a.flags & F_NOT_RECIPE != 0 {
            continue; // bookkeeping rows are not real work
        }
        let b0 = ((a.ts as f64 / span) * CONC_BUCKETS as f64) as usize;
        let b1 = ((a.ts.saturating_add(a.dur) as f64 / span) * CONC_BUCKETS as f64).ceil() as usize;
        // clamp b0 into the last bucket BEFORE deriving b1: a bar
        // sitting exactly at the span end must not push b1 to
        // CONC_BUCKETS + 1, one past the difference array
        let b0 = b0.min(CONC_BUCKETS - 1);
        let b1 = b1.clamp(b0 + 1, CONC_BUCKETS);
        diff[b0] += 1;
        diff[b1] -= 1;
    }
    let mut out = Vec::with_capacity(CONC_BUCKETS);
    let mut run = 0i32;
    let mut max = 0u32;
    for d in diff.iter().take(CONC_BUCKETS) {
        run += *d;
        let v = run.max(0) as u32;
        max = max.max(v);
        out.push(v);
    }
    (out, max.max(1))
}

/// Where a lane sits on the overview's vertical axis, as a fraction of
/// the strip's height.
///
/// Deliberately not linear. A build's lane count is its peak
/// concurrency, and peaks are brief: one large capture opens 57,162 lanes
/// for about a second and spends the other 210 seconds under 8,000. Down
/// a linear axis that is a hairline along the top with one spike in it —
/// true, and no use for finding anything. The square root gives the range
/// a build actually lives in about a third of the height and still puts
/// the peak on the bottom edge.
///
/// Both the picture and the brush over it go through here, so they cannot
/// disagree about where a lane is.
fn lane_frac(lane: usize, n_lanes: usize) -> f64 {
    (lane as f64 / n_lanes.max(1) as f64).clamp(0.0, 1.0).sqrt()
}

/// A picture of the whole build on the timeline's own two axes: time
/// across, lane down (see `lane_frac`), one cell per `w` x `h` slice,
/// coloured by whichever work class holds the most of that cell.
///
/// `0xff` marks a cell with nothing in it. Because lanes are packed, a
/// column is filled from the top down to whatever was in flight then —
/// the concurrency curve arrives for free as the silhouette.
///
/// Costs one pass over the actions rather than one per cell: each action
/// lands in exactly one lane, so it is two writes into a difference array
/// that is then run along the time axis, the same trick
/// [`build_concurrency`] uses.
pub fn lane_time_map(tr: &Trace, w: usize, h: usize) -> Vec<u8> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let nc = config::CLASS_N;
    let span = tr.span.max(1) as f64;
    let n_lanes = tr.lanes.len().max(1);
    let mut diff = vec![0i32; (w + 1) * h * nc];
    for a in &tr.acts {
        // bookkeeping rows are excluded from the span, so a trailing one
        // can sit at or past it — and the overview shows real work
        if a.flags & F_NOT_RECIPE != 0 {
            continue;
        }
        let c0 = (((a.ts as f64 / span) * w as f64) as usize).min(w - 1);
        let c1 = (((a.ts.saturating_add(a.dur) as f64 / span) * w as f64).ceil() as usize)
            .clamp(c0 + 1, w);
        let row = ((lane_frac(a.lane as usize, n_lanes) * h as f64) as usize).min(h - 1);
        let k = (a.color as usize).min(nc - 1);
        diff[(c0 * h + row) * nc + k] += 1;
        diff[(c1 * h + row) * nc + k] -= 1;
    }
    let mut out = vec![0xffu8; w * h];
    let mut run = vec![0i32; h * nc];
    for c in 0..w {
        for row in 0..h {
            let mut best = (0i32, 0usize);
            for k in 0..nc {
                let v = &mut run[row * nc + k];
                *v += diff[(c * h + row) * nc + k];
                if *v > best.0 {
                    best = (*v, k);
                }
            }
            if best.0 > 0 {
                out[c * h + row] = best.1 as u8;
            }
        }
    }
    out
}

/// The overview ribbon: [`lane_time_map`] rendered into a texture.
///
/// The first version put *normalised class mix* down the strip instead of
/// lane, which made a full-height picture out of a build that is mostly
/// idle — and then the colours under the brush disagreed with the colours
/// on screen, because the strip was averaging all 59,683 actions in flight
/// while the timeline showed the thirty lanes that fit. Plotting the same
/// lane axis the timeline plots is what makes the two agree, and it is
/// what lets the brush have a height.
pub fn build_minimap(app: &mut Ready, ctx: &egui::Context) {
    const W: usize = 1024;
    const H: usize = 128;
    let map = lane_time_map(&app.tr, W, H);
    let bg = egui::Color32::from_rgb(0xea, 0xe8, 0xe5);
    let mut px = vec![bg; W * H];
    for c in 0..W {
        for row in 0..H {
            let k = map[c * H + row];
            if k != 0xff {
                px[row * W + c] = config::class_color(k as usize);
            }
        }
    }
    let img = egui::ColorImage::new([W, H], px);
    app.minimap = Some(ctx.load_texture("minimap", img, egui::TextureOptions::LINEAR));
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct WheelZoom {
    time: f64,
    lanes: f64,
    handled: bool,
}

/// Raw modifiers distinguish the two axes before egui folds both into
/// `zoom_delta()`. On Windows/Linux, Ctrl also sets `command`.
fn wheel_zoom(events: &[egui::Event]) -> WheelZoom {
    let mut zoom = WheelZoom::default();
    for e in events {
        if let egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
            ..
        } = e
        {
            if !delta.is_finite() || !(modifiers.ctrl || modifiers.mac_cmd) {
                continue;
            }
            let points = (f64::from(delta.x) + f64::from(delta.y))
                * match unit {
                    egui::MouseWheelUnit::Line => 8.0,
                    egui::MouseWheelUnit::Page => 200.0,
                    egui::MouseWheelUnit::Point => 1.0,
                };
            zoom.handled = true;
            if modifiers.mac_cmd || (modifiers.ctrl && modifiers.shift) {
                zoom.time += points;
            } else {
                zoom.lanes += points;
            }
        }
    }
    zoom
}

/// `zoom_delta()` also contains smoothed wheel input. Only these genuine
/// pinch sources can supplement the raw wheel routing without replaying it.
fn pinch_zoom(input: &egui::InputState) -> f32 {
    input.multi_touch().map_or_else(
        || {
            input.events.iter().fold(1.0, |zoom, event| match event {
                egui::Event::Zoom(factor) => zoom * factor,
                _ => zoom,
            })
        },
        |touch| touch.zoom_delta,
    )
}

fn apply_zoom_input(app: &mut Ready, wheel: WheelZoom, pinch: f32, anchor: egui::Vec2) {
    if wheel.handled {
        if wheel.lanes != 0.0 {
            app.zoom_rows_at(
                (wheel.lanes / 180.0).clamp(-4.0, 4.0).exp() as f32,
                anchor.y,
            );
        }
        if wheel.time != 0.0 {
            app.zoom_at((wheel.time / 180.0).clamp(-4.0, 4.0).exp(), anchor.x);
        }
    } else if pinch.is_finite() && pinch > 0.0 && (pinch - 1.0).abs() > 1e-4 {
        app.zoom_at(f64::from(pinch), anchor.x);
    }
}

fn viewport_size(app: &Ready) -> egui::Vec2 {
    egui::vec2(
        if app.canvas_w.is_finite() && app.canvas_w > 1.0 {
            app.canvas_w
        } else {
            DEFAULT_CANVAS_W
        },
        if app.canvas_h.is_finite() && app.canvas_h > 1.0 {
            app.canvas_h
        } else {
            DEFAULT_CANVAS_H
        },
    )
}

fn reset_view(app: &mut Ready) {
    let size = viewport_size(app);
    app.fit(size.x);
    app.row_h = config::get()
        .appearance
        .row_height
        .unwrap_or(11.0)
        .clamp(ROW_H_MIN, ROW_H_MAX);
    app.scroll = 0.0;
    app.clamp_view(size.x, size.y);
}

fn overview_controls(app: &mut Ready, ui: &mut egui::Ui) {
    let size = viewport_size(app);
    let mut changed = false;
    for (label, factor) in [("Time zoom in", 1.6), ("Time zoom out", 1.0 / 1.6)] {
        if ui
            .button(label)
            .on_hover_text("Ctrl+Shift+wheel or Cmd+wheel; + / - keys. Pinch also zooms time.")
            .clicked()
        {
            app.zoom_at(factor, size.x / 2.0);
            changed = true;
        }
    }
    for (label, factor) in [("Lane zoom in", 1.3), ("Lane zoom out", 1.0 / 1.3)] {
        if ui
            .button(label)
            .on_hover_text("Ctrl+wheel or [ / ] keys change lane height.")
            .clicked()
        {
            app.zoom_rows_at(factor, size.y / 2.0);
            changed = true;
        }
    }
    ui.separator();
    if ui
        .button("Reset view")
        .on_hover_text(
            "Fit the full time range, restore the configured lane height, and scroll to the first lane. Keep filters and selection.",
        )
        .clicked()
    {
        reset_view(app);
        changed = true;
    }
    if changed {
        app.clamp_view(size.x, size.y);
        ui.close();
    }
}

/// The x range the plot paints time across, projected onto `strip` — the
/// one mapping the minimap and the horizontal scrollbar must share with
/// the canvas. Until the first canvas frame, derive the plot from this strip's
/// width and the current gutter setting.
fn plot_axis(app: &Ready, strip: egui::Rect) -> egui::Rect {
    let (x0, w) = if app.canvas_x0.is_finite() && app.canvas_w.is_finite() && app.canvas_w > 1.0 {
        (app.canvas_x0, app.canvas_w)
    } else {
        let gutter = if app.show_gutter { GUTTER_W } else { 0.0 };
        (
            strip.left() + gutter,
            (strip.width() - gutter - SB_W).max(1.0),
        )
    };
    egui::Rect::from_min_max(
        egui::pos2(x0, strip.top()),
        egui::pos2(x0 + w.max(1.0), strip.bottom()),
    )
}

/// A resized window can leave no room beyond the fixed header and gutters.
fn canvas_size(full: egui::Rect, gutter: f32, head_h: f32) -> egui::Vec2 {
    egui::vec2(
        (full.width() - SB_W - gutter).max(0.0),
        (full.height() - SB_W - head_h).max(0.0),
    )
}

fn action_label_font_size(row_h: f32) -> Option<f32> {
    let size = (row_h - 3.0).min(10.0);
    // Seven points is already the gutter's smallest text size. At denser zoom
    // the bars still carry timing, but their labels do not fit readably.
    (size >= 7.0).then_some(size)
}

/// The time window the plot is showing: the instant under its left
/// edge, and the one just past its right. Anything drawing a "you are
/// here" — the overview brush, the horizontal scrollbar thumb — has to
/// bracket exactly this. Measuring the window against the *window's*
/// width rather than the plot's is the same mistake [`plot_axis`]
/// exists to prevent, one axis over: it widens the brush by a gutter
/// and a scrollbar and the bracket stops agreeing with the ruler.
fn view_window(app: &Ready) -> (f64, f64) {
    let w = if app.canvas_w > 1.0 {
        app.canvas_w
    } else {
        DEFAULT_CANVAS_W
    };
    (app.t_left, app.t_left + app.upp * f64::from(w))
}

/// The lanes the plot is showing, as a fraction of every lane in the
/// build. Vertical zoom moves the second number and not the first, which
/// is exactly what the brush's height has to track.
fn lane_window(app: &Ready) -> (f64, f64) {
    let n = app.tr.lanes.len();
    let row_h = f64::from(app.row_h.max(0.01));
    let h = if app.canvas_h > 1.0 {
        app.canvas_h
    } else {
        DEFAULT_CANVAS_H
    };
    let l0 = f64::from(app.scroll) / row_h;
    let l1 = l0 + f64::from(h) / row_h;
    (
        lane_frac(l0.max(0.0) as usize, n),
        lane_frac(l1.max(0.0) as usize, n),
    )
}

/// A scrollbar thumb's start and length, as fractions of its track.
/// `total` is a measured extent, and a trace with no lane or no span
/// makes it zero — guarding the division here is what keeps both bars
/// from painting a NaN rect, which egui accepts and draws as garbage
/// rather than rejecting.
fn thumb(pos: f64, view: f64, total: f64, min_len: f64, min_pos: f64) -> (f32, f32) {
    let total = if total > 0.0 { total } else { 1.0 };
    (
        (pos / total).clamp(min_pos, 1.0) as f32,
        (view / total).clamp(min_len, 1.0) as f32,
    )
}

/// The overview strip with the viewport brush.
pub fn show_minimap(app: &mut Ready, root: &mut egui::Ui) {
    let mini_h = 64.0;
    egui::Panel::bottom("mini")
        .exact_size(mini_h)
        .show(root, |ui| {
            let Some(tex) = app.minimap.as_ref().map(egui::TextureHandle::id) else {
                return;
            };
            let panel = ui.available_rect_before_wrap();
            // The overview is a second view of the same time axis, so it
            // has to be drawn on that axis and not on the panel's: t=0
            // belongs under the plot's 0s tick, one lane gutter in, and
            // t=span under the plot's right edge, one scrollbar short of
            // the window. Painting it panel-wide shears both the image
            // and the brush left by the gutter, which is why the brush's
            // left edge sat against the window frame while the bars it
            // was tracking started 54 points further in.
            let rect = plot_axis(app, panel);
            let painter = ui.painter_at(panel);
            // The image only covers the time axis, which is narrower than
            // the panel by a lane gutter on the left and a scrollbar on
            // the right. Nothing else paints those two strips, so fill
            // them here; the gutter colour continues the canvas's own.
            painter.rect_filled(panel, 0.0, gutter_fill());
            painter.text(
                egui::pos2(panel.left() + 4.0, panel.center().y),
                egui::Align2::LEFT_CENTER,
                "overview\nright-click",
                egui::FontId::proportional(9.0),
                egui::Color32::from_gray(0x60),
            );
            painter.image(
                tex,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let span = app.tr.span.max(1) as f64;
            let (t0, t1) = view_window(app);
            let x0 = rect.left() + (t0 / span).clamp(0.0, 1.0) as f32 * rect.width();
            let x1 =
                (rect.left() + (t1 / span).clamp(0.0, 1.0) as f32 * rect.width()).max(x0 + 2.0);
            let (f0, f1) = lane_window(app);
            let y0 = rect.top() + f0.clamp(0.0, 1.0) as f32 * rect.height();
            let y1 = (rect.top() + f1.clamp(0.0, 1.0) as f32 * rect.height()).max(y0 + 2.0);
            // A four-second window of a 233-second build is 1.7% of the
            // strip and thirty lanes of 57,000 are two points of its
            // height: an outline that small is easy to lose. Veiling the
            // time outside the window makes it the thing that is lit
            // rather than the thing that is outlined. The veil is on time
            // only — the lanes off screen are still worth reading, and
            // that is the whole point of showing them.
            let veil = egui::Color32::from_rgba_unmultiplied(0xf6, 0xf5, 0xf3, 0x90);
            for side in [
                egui::Rect::from_min_max(rect.left_top(), egui::pos2(x0, rect.bottom())),
                egui::Rect::from_min_max(egui::pos2(x1, rect.top()), rect.right_bottom()),
            ] {
                if side.width() > 0.0 {
                    painter.rect_filled(side, 0.0, veil);
                }
            }
            painter.rect_stroke(
                egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)),
                0.0,
                egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0xe8, 0x3e, 0x8e)),
                egui::StrokeKind::Outside,
            );
            // The two numbers the strip cannot show by shape: how much
            // wall the whole width stands for, and how many lanes the
            // whole height does.
            painter.text(
                egui::pos2(rect.right() - 3.0, panel.bottom() - 1.0),
                egui::Align2::RIGHT_BOTTOM,
                format!(
                    "{} \u{d7} {} lanes",
                    fmt_us(span),
                    humanize::count(app.tr.lanes.len() as u64)
                ),
                egui::FontId::proportional(9.0),
                egui::Color32::from_gray(0x50),
            );
            // Clicking anywhere on the strip centres the view on that
            // point of the build — in both axes, since the strip has both.
            let resp = ui.allocate_rect(panel, egui::Sense::click_and_drag());
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    resp.enabled(),
                    "Timeline overview",
                )
            });
            if resp.clicked() || resp.dragged_by(egui::PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    let f = f64::from(((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
                    app.t_left = f * span - (t1 - t0) / 2.0;
                    let n = app.tr.lanes.len().max(1) as f32;
                    // invert `lane_frac`
                    let g = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                    let lane = g * g * n;
                    let rows = app.canvas_h.max(1.0) / app.row_h.max(0.01);
                    app.scroll = (lane - rows / 2.0).max(0.0) * app.row_h;
                }
            }
            resp.context_menu(|ui| overview_controls(app, ui));
            resp.on_hover_text(
                "Drag to center time and lanes. Right-click for zoom controls and Reset view.\nOver the timeline: Ctrl+wheel changes lane height; Ctrl+Shift+wheel or Cmd+wheel zooms time.",
            );
        });
}

/// The main canvas: timeline + scrollbars + hover. `nav` carries a
/// navigation request produced by a panel this frame.
pub fn show_canvas(app: &mut Ready, root: &mut egui::Ui, nav: Option<usize>) {
    let ctx = &root.ctx().clone();
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(0xf2, 0xf1, 0xef)))
        .show(root, |ui| {
            let full = ui.available_rect_before_wrap();
            let sb = SB_W;
            let ruler_h = 22.0;
            let gutter = if app.show_gutter { GUTTER_W } else { 0.0 };
            let conc_h = if app.show_conc { 30.0 } else { 0.0 };
            let head_h = ruler_h + conc_h;
            let size = canvas_size(full, gutter, head_h);
            let (width, view_h) = (size.x, size.y);
            let plot_left = full.left() + gutter;
            app.canvas_x0 = plot_left;
            app.canvas_w = width;
            app.canvas_h = view_h;
            if width == 0.0 || view_h == 0.0 {
                return;
            }
            let rect = egui::Rect::from_min_max(
                full.min,
                egui::pos2(full.right() - sb, full.bottom() - sb),
            );
            let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let painter = ui.painter_at(full);

            if let Some(idx) = nav {
                app.navigate(idx, width, view_h);
            }

            if resp.dragged() {
                let d = resp.drag_delta();
                app.t_left -= d.x as f64 * app.upp;
                app.scroll = (app.scroll - d.y).max(0.0);
            }
            // Only the layer under the pointer gets wheel/pinch/hover:
            // egui clears `hovered()` when a window occludes this rect,
            // so scrolling a popup never pans the timeline underneath.
            let over_canvas = resp.hovered();
            let (scroll_delta, pinch, pointer_raw, wheel) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    pinch_zoom(i),
                    i.pointer.hover_pos(),
                    wheel_zoom(&i.events),
                )
            });
            let pointer = if over_canvas { pointer_raw } else { None };
            let scroll_delta = if over_canvas && !wheel.handled {
                scroll_delta
            } else {
                egui::Vec2::ZERO
            };
            let pinch = if over_canvas { pinch } else { 1.0 };
            let wheel = if over_canvas {
                wheel
            } else {
                WheelZoom::default()
            };
            if scroll_delta != egui::Vec2::ZERO {
                app.scroll = (app.scroll - scroll_delta.y).max(0.0);
                app.t_left -= scroll_delta.x as f64 * app.upp;
            }
            apply_zoom_input(
                app,
                wheel,
                pinch,
                egui::vec2(
                    pointer.map(|p| p.x - plot_left).unwrap_or(width / 2.0),
                    pointer
                        .map(|p| p.y - (rect.top() + head_h))
                        .unwrap_or(view_h / 2.0),
                ),
            );
            let mut jump = 0isize;
            let mut back = false;
            let typing = ctx.egui_wants_keyboard_input() && ctx.memory(|m| m.focused().is_some());
            ui.input(|i| {
                // Every timeline binding is a bare character, and the Find
                // overlay is an Area in this same viewport, so a keystroke
                // meant for a text field arrives here too — buck2 target
                // names are full of the hyphen that zooms out.
                if typing {
                    return;
                }
                if i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus) {
                    app.zoom_at(1.6, width / 2.0);
                }
                if i.key_pressed(egui::Key::Minus) {
                    app.zoom_at(1.0 / 1.6, width / 2.0);
                }
                // the vertical pair: `+`/`-` stretch time, `]`/`[`
                // stretch the lanes. Anchored at the middle of the lane
                // area so the rows under the eye stay put.
                if i.key_pressed(egui::Key::CloseBracket) {
                    app.zoom_rows_at(1.3, view_h / 2.0);
                }
                if i.key_pressed(egui::Key::OpenBracket) {
                    app.zoom_rows_at(1.0 / 1.3, view_h / 2.0);
                }
                if i.key_pressed(egui::Key::F) {
                    app.t_left = 0.0;
                    app.upp = app.tr.span as f64 / f64::from(width.max(100.0));
                }
                // ⇧F fits vertically too: the row height that puts every
                // lane on screen at once, floored so the bars stay
                // visible on a 57k-lane trace rather than collapsing to
                // nothing.
                if i.key_pressed(egui::Key::F) && i.modifiers.shift {
                    app.scroll = 0.0;
                    app.row_h =
                        (view_h / app.tr.lanes.len().max(1) as f32).clamp(ROW_H_MIN, ROW_H_MAX);
                }
                if i.key_pressed(egui::Key::C) && app.cp_set.is_some() {
                    app.cp_on = !app.cp_on;
                }
                // `key_pressed` ignores modifiers, so without the carve-out
                // the ⇧Esc that closes every popup would also drop the
                // selection the popups were opened from.
                if i.key_pressed(egui::Key::Escape) && !i.modifiers.shift {
                    app.clear_selection();
                    app.anc_set = None;
                    app.ctx_menu = None;
                }
                if i.key_pressed(egui::Key::Questionmark) || i.key_pressed(egui::Key::Slash) {
                    app.panels.help = true;
                }
                if i.key_pressed(egui::Key::N) {
                    jump = if i.modifiers.shift { -1 } else { 1 };
                }
                if i.key_pressed(egui::Key::Backspace) {
                    back = true;
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    app.t_left -= app.upp * width as f64 / 3.0;
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    app.t_left += app.upp * width as f64 / 3.0;
                }
                if i.key_pressed(egui::Key::ArrowUp) {
                    app.scroll = (app.scroll - app.row_h * 6.0).max(0.0);
                }
                if i.key_pressed(egui::Key::ArrowDown) {
                    app.scroll += app.row_h * 6.0;
                }
            });
            if jump != 0 {
                app.jump_match(jump, width, view_h);
            }
            if back {
                app.go_back(width, view_h);
            }
            app.clamp_view(width, view_h);

            // ruler
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(plot_left, rect.top()),
                    egui::vec2(width, ruler_h),
                ),
                0.0,
                egui::Color32::from_rgb(0xe4, 0xe2, 0xdf),
            );
            let raw = app.upp * 120.0;
            let mag = 10f64.powf(raw.log10().floor());
            let step = [1.0f64, 2.0, 5.0, 10.0]
                .iter()
                .map(|m| m * mag)
                .find(|s| *s >= raw)
                .unwrap_or(mag * 10.0);
            let first_tick = (app.t_left / step).floor() * step;
            let n_ticks = ((app.upp * f64::from(width) / step).ceil() as usize).saturating_add(2).min(4096);
            for tick in 0..n_ticks {
                let t = first_tick + tick as f64 * step;
                let x = plot_left + ((t - app.t_left) / app.upp) as f32;
                if x >= plot_left {
                    painter.vline(
                        x,
                        egui::Rangef::new(rect.top(), rect.bottom()),
                        egui::Stroke::new(0.5_f32, egui::Color32::from_gray(0xd8)),
                    );
                    painter.text(
                        egui::pos2(x + 3.0, rect.top() + 3.0),
                        egui::Align2::LEFT_TOP,
                        fmt_us(t),
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_gray(0x50),
                    );
                }
            }

            // concurrency strip: actions in flight over the visible window
            if app.show_conc && !app.conc.is_empty() {
                let top = rect.top() + ruler_h;
                let strip = egui::Rect::from_min_max(
                    egui::pos2(plot_left, top),
                    egui::pos2(rect.right(), top + conc_h),
                );
                painter.rect_filled(strip, 0.0, egui::Color32::from_rgb(0xea, 0xe8, 0xe5));
                let span = app.tr.span.max(1) as f64;
                let n = app.conc.len();
                let step_px = 2.0f32;
                let cols = (width / step_px).ceil() as usize;
                let mut peak = 0u32;
                for c in 0..cols {
                    let t = app.t_left + app.upp * (c as f32 * step_px) as f64;
                    let b = ((t / span) * n as f64) as isize;
                    if b < 0 || b >= n as isize {
                        continue;
                    }
                    let v = app.conc[b as usize];
                    peak = peak.max(v);
                    let h = (v as f32 / app.conc_max as f32) * (conc_h - 4.0);
                    let x = plot_left + c as f32 * step_px;
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x, strip.bottom() - h - 1.0),
                            egui::pos2(x + step_px - 0.4, strip.bottom() - 1.0),
                        ),
                        0.0,
                        egui::Color32::from_rgb(0x7a, 0x9c, 0xc4),
                    );
                }
                // The lane range belongs on this line because it is the
                // answer to "why doesn't the overview match what I see":
                // a 57k-lane build puts about a dozen rows on screen, so
                // the timeline is a keyhole and the overview is the wall.
                let l0 = (app.scroll / app.row_h) as usize;
                let n_lanes = app.tr.lanes.len();
                let l1 = (l0 + (view_h / app.row_h).ceil() as usize).min(n_lanes);
                painter.text(
                    egui::pos2(plot_left + 4.0, strip.top() + 1.0),
                    egui::Align2::LEFT_TOP,
                    format!(
                        "in flight — exact peak {} here, {} max bucket occupancy overall   ·   lanes {}–{} of {}",
                        humanize::count(peak as u64),
                        humanize::count(app.conc_max as u64),
                        l0,
                        l1.saturating_sub(1),
                        humanize::count(n_lanes as u64),
                    ),
                    egui::FontId::proportional(9.0),
                    egui::Color32::from_gray(0x60),
                );
            }

            // lanes
            let lane_top = (app.scroll / app.row_h) as usize;
            let rows = (view_h / app.row_h).ceil() as usize + 1;
            let plot_top = rect.top() + head_h;
            // Everything that scrolls is painted through here. The topmost
            // row only starts at `plot_top` when the scroll offset happens to
            // be a whole number of rows; the rest of the time it begins above
            // it, and unclipped it paints straight over the in-flight strip —
            // at a tall row height it hides the strip entirely. The last row
            // runs off the bottom edge onto the scrollbar for the same reason.
            let plot_painter = painter.with_clip_rect(egui::Rect::from_min_max(
                egui::pos2(rect.left(), plot_top),
                egui::pos2(rect.right(), rect.bottom()),
            ));
            let y_off = rect.top() + head_h - (app.scroll - lane_top as f32 * app.row_h);
            let matches = app.matches.as_deref();
            let selected = app.selected;
            let cp_set = if app.cp_on { app.cp_set.as_ref() } else { None };
            let anc_set = app.anc_set.as_ref();
            let row_h = app.row_h;
            let label_font_size = action_label_font_size(row_h);
            let mut labels: Vec<(egui::Pos2, String)> = Vec::new();
            // idx, x0, x1, y, base color (the exec segment's own color)
            let mut wide: Vec<(usize, f32, f32, f32, egui::Color32)> = Vec::new();
            let acts = &app.tr.acts;
            let names = &app.tr.names;
            let (painted, _merged) = cull(
                &app.tr,
                app.t_left,
                app.upp,
                lane_top,
                rows,
                width,
                |idx, x0, x1, row| {
                    let a = acts[idx];
                    let y = y_off + row as f32 * row_h;
                    let mut c = config::class_color(a.color as usize);
                    if a.flags & F_FAILED != 0 {
                        c = fail_color();
                    }
                    let dimmed = matches.map(|m| !m[idx]).unwrap_or(false);
                    if dimmed {
                        c = c.gamma_multiply(0.22);
                    }
                    let r = egui::Rect::from_min_max(
                        egui::pos2(plot_left + x0, y),
                        egui::pos2(plot_left + x1.max(x0 + 0.8), y + row_h - 1.5),
                    );
                    let seg_ok = !dimmed && a.dur as f64 / app.upp >= f64::from(SEG_MIN_PX);
                    plot_painter.rect_filled(r, 1.0, if seg_ok { stage_tail() } else { c });
                    if a.flags & F_CACHE_HIT != 0 && !dimmed && !seg_ok && x1 - x0 > 3.0 {
                        plot_painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(r.left(), r.top()),
                                egui::pos2(r.right(), r.top() + 2.0),
                            ),
                            0.0,
                            egui::Color32::from_white_alpha(160),
                        );
                    }
                    if a.flags & F_CACHE_HIT != 0 && x1 - x0 > 2.0 {
                        plot_painter.rect_stroke(
                            r,
                            1.0,
                            egui::Stroke::new(1.0_f32, cache_outline()),
                            egui::StrokeKind::Outside,
                        );
                    }
                    if let Some(ansl) = anc_set {
                        if ansl.contains(&idx) {
                            plot_painter.rect_stroke(
                                r.expand(1.0),
                                1.0,
                                egui::Stroke::new(1.5_f32, anc_color()),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                    if let Some(cps) = cp_set {
                        if cps.contains(&idx) {
                            plot_painter.rect_stroke(
                                r.expand(1.0),
                                1.0,
                                egui::Stroke::new(1.5_f32, cp_outline()),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                    if selected == Some(idx) {
                        // tint the WHOLE extent: pale leading stage
                        // segments otherwise read as background
                        plot_painter.rect_filled(r, 1.0, flash_color().gamma_multiply(0.15));
                        plot_painter.rect_stroke(
                            r.expand(1.5),
                            1.0,
                            egui::Stroke::new(2.0_f32, flash_color()),
                            egui::StrokeKind::Outside,
                        );
                    }
                    // Failure is action status, not positioned activity. Keep
                    // it visible even when the interior has no observations.
                    // Inside leaves cache and selection outlines independent.
                    if a.flags & F_FAILED != 0 {
                        plot_painter.rect_stroke(
                            r,
                            1.0,
                            egui::Stroke::new(1.0_f32, c),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if seg_ok {
                        wide.push((idx, plot_left + x0, plot_left + x1, y, c));
                    }
                    if x1 - x0 > 48.0 && label_font_size.is_some() {
                        labels.push((
                            egui::pos2(r.left() + 3.0, r.center().y),
                            names[a.name as usize]
                                .rsplit('/')
                                .next()
                                .unwrap_or("")
                                .to_owned(),
                        ));
                    }
                },
            );
            // Only client-clock observations have positions. Negative cache
            // entries survive eviction so uninstrumented bars are not reread.
            {
                let mut budget = 80usize;
                let since = std::time::Instant::now();
                let mut need_repaint = false;
                let on_screen: HashSet<_> = wide.iter().map(|&(idx, ..)| idx).collect();
                for &(idx, _x0, _x1, y, base) in &wide {
                    if app.activity_state[idx] == 0 && budget > 0 && since.elapsed() < std::time::Duration::from_millis(2) {
                        budget -= 1;
                        let a = app.tr.acts[idx];
                        let spans = load_full(&app.tr, a.off, a.len).map(|record| {
                            let activity = crate::activity::Activity::from_record(&record);
                            activity.covered.iter().map(|i| (i.start_us, i.end_us, false))
                                .chain(activity.spans.iter().filter(|s| s.cancelled).map(|s| (s.start_us, s.end_us, true)))
                                .collect::<Vec<_>>()
                        }).unwrap_or_default();
                        app.activity_state[idx] = if spans.is_empty() { 1 } else { 2 };
                        if !spans.is_empty() {
                            const MAX_CACHED_INTERVALS: usize = 65_536;
                            make_activity_room(
                                &mut app.activity_cache,
                                &mut app.activity_cache_intervals,
                                &mut app.activity_state,
                                spans.len(),
                                MAX_CACHED_INTERVALS,
                                &on_screen,
                            );
                            app.activity_cache_intervals += spans.len();
                            app.activity_cache.insert(idx, spans);
                        }
                    } else if app.activity_state[idx] == 0 {
                        need_repaint = true;
                        continue;
                    }
                    let Some(spans) = app.activity_cache.get(&idx) else { continue };
                    let action_start = app.tr.acts[idx].ts as f64;
                    for &(start, end, canceled) in spans {
                        let Some((x0, x1)) = interval_pixels(action_start + start as f64, action_start + end as f64, app.t_left, app.upp, width) else { continue };
                        let rect = egui::Rect::from_min_max(egui::pos2(plot_left + x0, y + 1.0), egui::pos2(plot_left + x1, y + row_h - 2.5));
                        if canceled { widgets::hatch(&plot_painter, rect); }
                        else { plot_painter.rect_filled(rect, 0.0, base); }
                    }
                }
                if need_repaint {
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                }
            }
            for (pos, text) in labels {
                plot_painter.text(
                    pos,
                    egui::Align2::LEFT_CENTER,
                    text,
                    egui::FontId::proportional(label_font_size.expect("labels require readable row height")),
                    egui::Color32::from_gray(0x28),
                );
            }
            app.painted = painted;

            // lane gutter
            if app.show_gutter {
                let g = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), plot_top),
                    egui::pos2(plot_left, rect.bottom()),
                );
                plot_painter.rect_filled(g, 0.0, gutter_fill());
                painter.vline(
                    plot_left,
                    egui::Rangef::new(rect.top(), rect.bottom()),
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0xc8)),
                );
                let label_every = if app.row_h >= 11.0 { 1 } else { 4 };
                for row in 0..rows {
                    let li = lane_top + row;
                    if li >= app.tr.lanes.len() {
                        break;
                    }
                    let y = y_off + row as f32 * app.row_h;
                    let band = app
                        .tr
                        .bands
                        .iter()
                        .find(|b| li as u32 >= b.base && (li as u32) < b.base + b.nrows);
                    let num_pos = egui::pos2(plot_left - 5.0, y + app.row_h / 2.0 - 0.5);
                    let num_font = egui::FontId::proportional((app.row_h - 3.0).clamp(7.0, 10.0));
                    match band.filter(|b| b.base > 0 && li as u32 == b.base) {
                        // a band after the first opens with a rule across
                        // the plot and its name where the row number would
                        // go; the first band's name is the corner label
                        Some(b) => {
                            plot_painter.hline(
                                egui::Rangef::new(rect.left(), rect.right()),
                                y,
                                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0xcc)),
                            );
                            plot_painter.text(
                                num_pos,
                                egui::Align2::RIGHT_CENTER,
                                &b.gutter,
                                num_font,
                                egui::Color32::from_gray(0x38),
                            );
                        }
                        None => {
                            if !li.is_multiple_of(label_every) {
                                continue;
                            }
                            plot_painter.text(
                                num_pos,
                                egui::Align2::RIGHT_CENTER,
                                format!("{li}"),
                                num_font,
                                egui::Color32::from_gray(0x78),
                            );
                        }
                    }
                }
                painter.text(
                    egui::pos2(rect.left() + 4.0, rect.top() + 4.0),
                    egui::Align2::LEFT_TOP,
                    "lane (packed)",
                    egui::FontId::proportional(9.0),
                    egui::Color32::from_gray(0x60),
                );
            }

            // The head above this line is fixed and the plot below it is
            // clipped to it, so a row scrolled to a fraction gets cut off
            // here; the rule is what makes that read as a pane edge.
            painter.hline(
                egui::Rangef::new(rect.left(), rect.right()),
                plot_top,
                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0xc4)),
            );

            // scroll bars
            let total_h = app.tr.lanes.len() as f32 * app.row_h;
            let vtrack = egui::Rect::from_min_max(
                egui::pos2(full.right() - sb, full.top() + head_h),
                egui::pos2(full.right(), full.bottom() - sb),
            );
            painter.rect_filled(vtrack, 0.0, egui::Color32::from_gray(0xe0));
            let (vpos, vfrac) = thumb(
                f64::from(app.scroll),
                f64::from(view_h),
                f64::from(total_h),
                0.02,
                0.0,
            );
            let vthumb = egui::Rect::from_min_max(
                egui::pos2(vtrack.left() + 2.0, vtrack.top() + vpos * vtrack.height()),
                egui::pos2(
                    vtrack.right() - 2.0,
                    vtrack.top() + (vpos + vfrac).min(1.0) * vtrack.height(),
                ),
            );
            let vresp = ui.allocate_rect(vtrack, egui::Sense::click_and_drag());
            if vresp.dragged() || vresp.clicked() {
                if let Some(p) = vresp.interact_pointer_pos() {
                    let f = ((p.y - vtrack.top()) / vtrack.height() - vfrac / 2.0)
                        .clamp(0.0, 1.0 - vfrac);
                    app.scroll = f * total_h;
                }
            }
            painter.rect_filled(vthumb, 3.0, egui::Color32::from_gray(0xa8));

            let total_w_us = app.tr.span as f64;
            // Same rule as the minimap: `hpos`/`hfrac` are fractions of
            // the *plot*, so the track they are painted on has to be the
            // plot's x range. Spanning the window instead put the thumb a
            // whole gutter left of the time it claimed to point at.
            let htrack = egui::Rect::from_min_max(
                egui::pos2(plot_left, full.bottom() - sb),
                egui::pos2(rect.right(), full.bottom()),
            );
            painter.rect_filled(htrack, 0.0, egui::Color32::from_gray(0xe0));
            let view_us = app.upp * width as f64;
            let (hpos, hfrac) = thumb(app.t_left, view_us, total_w_us, 0.005, -0.5);
            let hthumb = egui::Rect::from_min_max(
                egui::pos2(
                    htrack.left() + hpos.max(0.0) * htrack.width(),
                    htrack.top() + 2.0,
                ),
                egui::pos2(
                    htrack.left() + ((hpos + hfrac).clamp(0.0, 1.0)) * htrack.width(),
                    htrack.bottom() - 2.0,
                ),
            );
            let hresp = ui.allocate_rect(htrack, egui::Sense::click_and_drag());
            if hresp.dragged() || hresp.clicked() {
                if let Some(p) = hresp.interact_pointer_pos() {
                    // mirror the vertical bar: the thumb centre cannot
                    // pass the right edge
                    let f = ((p.x - htrack.left()) / htrack.width() - hfrac / 2.0)
                        .clamp(0.0, 1.0 - hfrac);
                    app.t_left = f as f64 * total_w_us;
                }
            }
            painter.rect_filled(hthumb, 3.0, egui::Color32::from_gray(0xa8));

            // hover / click
            if let Some(p) = pointer {
                if rect.contains(p) && p.x > plot_left && p.y > plot_top {
                    let li = lane_top + ((p.y - y_off) / row_h).max(0.0) as usize;
                    let t_at = app.t_left + app.upp * (p.x - plot_left) as f64;
                    if li < app.tr.lanes.len() {
                        let l = &app.tr.lanes[li];
                        let slice = &app.tr.acts[l.start as usize..l.end as usize];
                        let hi = slice.partition_point(|a| a.ts as f64 <= t_at);
                        let hit = slice[..hi].last().filter(|a| a.ts.saturating_add(a.dur) as f64 >= t_at).map(|a| (hi - 1, a));
                        if let Some((k, a)) = hit {
                            egui::Tooltip::always_open(
                                ctx.clone(),
                                ui.layer_id(),
                                egui::Id::new("tip"),
                                egui::PopupAnchor::Pointer,
                            )
                            .show(|ui: &mut egui::Ui| {
                                ui.set_min_width(460.0);
                                ui.label(
                                    egui::RichText::new(&app.tr.names[a.name as usize])
                                        .monospace()
                                        .size(11.0),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "+{}  for {}",
                                        humanize::dur(a.ts as f64),
                                        humanize::dur(a.dur as f64)
                                    ))
                                    .size(11.0),
                                );
                                ui.weak("Fill shows positioned activity coverage; blank portions have no positioned observation. Stage totals are available in details, not placed on this clock.");
                            });
                            let hit_idx = l.start as usize + k;
                            if resp.clicked() {
                                app.select(hit_idx);
                            }
                            if resp.secondary_clicked() {
                                app.ctx_menu = Some((hit_idx, widgets::MenuAt::open(ctx, p)));
                            }
                        }
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpositioned_failure_keeps_status_without_hiding_cache_or_selection() {
        for flags in [0, F_FAILED, F_FAILED | F_CACHE_HIT] {
            for selected in [false, true] {
                let mut app = one_bar_app(1_000_000);
                std::sync::Arc::get_mut(&mut app.tr).unwrap().acts[0].flags = flags;
                app.activity_state[0] = 1; // known absence of positioned intervals
                app.selected = selected.then_some(0);
                let ctx = egui::Context::default();
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    show_canvas(&mut app, ui, None);
                });
                output.textures_delta.clear();
                let rects: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Rect(r) => Some(r),
                        _ => None,
                    })
                    .collect();
                let bar = rects
                    .iter()
                    .find(|r| r.fill == stage_tail())
                    .expect("missing activity retains a neutral interior");
                assert!(bar.rect.width() >= SEG_MIN_PX);
                let failure = rects.iter().find(|r| {
                    r.rect == bar.rect
                        && r.stroke.color == fail_color()
                        && r.stroke_kind == egui::StrokeKind::Inside
                });
                assert_eq!(failure.is_some(), flags & F_FAILED != 0);
                assert_eq!(
                    rects.iter().any(|r| {
                        r.rect == bar.rect
                            && r.stroke.color == cache_outline()
                            && r.stroke_kind == egui::StrokeKind::Outside
                    }),
                    flags & F_CACHE_HIT != 0,
                );
                assert_eq!(
                    rects.iter().any(|r| {
                        r.rect == bar.rect.expand(1.5)
                            && r.stroke.color == flash_color()
                            && r.stroke_kind == egui::StrokeKind::Outside
                    }),
                    selected,
                );
            }
        }
    }

    #[test]
    fn positioned_geometry_clips_edges_without_rescaling_the_interval() {
        assert_eq!(
            super::interval_pixels(20.0, 50.0, 30.0, 1.0, 100.0),
            Some((-4.0, 20.0))
        );
        assert_eq!(
            super::interval_pixels(1.0e15 + 50.0, 1.0e15 + 75.0, 1.0e15, 1.0, 100.0),
            Some((50.0, 75.0))
        );
        assert_eq!(super::interval_pixels(200.0, 250.0, 0.0, 1.0, 100.0), None);
    }

    #[test]
    fn measured_zero_stage_array_is_not_missing() {
        assert_eq!(
            super::stages_from_record(&serde_json::json!({"args":{"stages":{"execute_us":0}}})),
            Some([0; crate::detail::STAGE_N])
        );
        assert_eq!(
            super::stages_from_record(&serde_json::json!({"args":{}})),
            None
        );
    }

    use super::*;
    use crate::parse;

    #[test]
    fn activity_cache_evicts_only_off_screen_entries() {
        let mut cache: HashMap<_, _> = (0..10).map(|i| (i, vec![(0, 1, false); 10])).collect();
        let mut total = 100;
        let mut state = vec![2; 10];
        let on_screen = (0..5).collect();
        make_activity_room(&mut cache, &mut total, &mut state, 30, 100, &on_screen);
        assert_eq!(total, 70);
        for (i, &status) in state.iter().enumerate() {
            if i < 5 {
                assert!(cache.contains_key(&i));
            }
            assert_eq!(status, if cache.contains_key(&i) { 2 } else { 0 });
        }
    }

    #[test]
    fn visible_activity_working_set_can_exceed_the_soft_cap() {
        let mut cache: HashMap<_, _> = (0..10).map(|i| (i, vec![(0, 1, false); 10])).collect();
        let mut total = 100;
        let mut state = vec![2; 10];
        let on_screen = (0..10).collect();
        make_activity_room(&mut cache, &mut total, &mut state, 30, 100, &on_screen);
        assert_eq!(cache.len(), 10);
        assert_eq!(total, 100);
        assert_eq!(state, vec![2; 10]);
    }

    #[test]
    fn zero_sized_minimap_is_empty() {
        let app = one_bar_app(100);
        assert!(lane_time_map(&app.tr, 0, 10).is_empty());
        assert!(lane_time_map(&app.tr, 10, 0).is_empty());
    }

    #[test]
    fn canvas_navigation_updates_detail_and_records_one_history_entry() {
        let path = crate::testdata("smoke-trace.json");
        let tr = parse::parse(&path).expect("should parse the smoke fixture");
        let mut app = Ready::new(tr, None);
        app.select(0);
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_canvas(&mut app, ui, Some(1));
        });
        output.drop_without_applying_deltas();
        assert_eq!(app.selected, Some(1));
        assert_eq!(
            app.detail
                .as_ref()
                .expect("should open navigated detail")
                .idx,
            1
        );
        assert_eq!(app.hist, vec![0]);
    }

    fn one_bar_app(dur: i64) -> Ready {
        let p = crate::temp_path(&format!("axis-{dur}.json"));
        std::fs::write(
            &p,
            format!(
                "[\n{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\
                 \"ts\":0,\"dur\":{dur}}},\n"
            ),
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        std::fs::remove_file(&p).ok();
        Ready::new(tr, None)
    }

    fn wheel_event(
        modifiers: egui::Modifiers,
        unit: egui::MouseWheelUnit,
        delta: egui::Vec2,
    ) -> egui::Event {
        egui::Event::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit,
            delta,
            modifiers,
        }
    }

    #[test]
    fn wheel_zoom_distinguishes_windows_control_shift_and_mac_command() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        for (modifiers, time, lanes) in [
            (ctrl, 0.0, 24.0),
            (
                egui::Modifiers {
                    shift: true,
                    ..ctrl
                },
                24.0,
                0.0,
            ),
            (
                egui::Modifiers {
                    mac_cmd: true,
                    command: true,
                    ..Default::default()
                },
                24.0,
                0.0,
            ),
            (
                egui::Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                0.0,
                24.0,
            ),
        ] {
            assert_eq!(
                wheel_zoom(&[wheel_event(
                    modifiers,
                    egui::MouseWheelUnit::Line,
                    egui::vec2(0.0, 3.0),
                )]),
                WheelZoom {
                    time,
                    lanes,
                    handled: true
                },
            );
        }
    }

    #[test]
    fn wheel_zoom_preserves_plain_scroll_and_handles_units_and_invalid_deltas() {
        for modifiers in [
            egui::Modifiers::default(),
            egui::Modifiers {
                shift: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                wheel_zoom(&[wheel_event(
                    modifiers,
                    egui::MouseWheelUnit::Line,
                    egui::vec2(0.0, 3.0),
                )]),
                WheelZoom::default(),
                "plain and Shift scrolling remain pans",
            );
        }
        let ctrl_shift = egui::Modifiers {
            ctrl: true,
            shift: true,
            command: true,
            ..Default::default()
        };
        for (unit, expected) in [
            (egui::MouseWheelUnit::Point, 2.0),
            (egui::MouseWheelUnit::Line, 16.0),
            (egui::MouseWheelUnit::Page, 400.0),
        ] {
            assert_eq!(
                wheel_zoom(&[wheel_event(ctrl_shift, unit, egui::vec2(2.0, 0.0))]).time,
                expected,
                "Shift wheel backends may put the delta on x",
            );
        }
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                wheel_zoom(&[wheel_event(
                    ctrl_shift,
                    egui::MouseWheelUnit::Point,
                    egui::vec2(bad, 1.0),
                )]),
                WheelZoom::default(),
            );
        }
        assert_eq!(wheel_zoom(&[egui::Event::Zoom(1.2)]), WheelZoom::default());
    }

    #[test]
    fn wheel_and_pinch_zoom_anchor_only_the_requested_axes() {
        let mut app = one_bar_app(10_000_000);
        app.upp = 1000.0;
        app.t_left = 2_000_000.0;
        app.row_h = 11.0;
        app.scroll = 132.0;
        let anchor = egui::vec2(200.0, 120.0);
        let time_at = app.t_left + app.upp * f64::from(anchor.x);
        let lanes = (app.row_h, app.scroll);
        apply_zoom_input(
            &mut app,
            WheelZoom {
                time: 90.0,
                handled: true,
                ..Default::default()
            },
            1.0,
            anchor,
        );
        assert!((app.upp - 1000.0 / 0.5f64.exp()).abs() < 1e-8);
        assert!((app.t_left + app.upp * f64::from(anchor.x) - time_at).abs() < 1e-8);
        assert_eq!((app.row_h, app.scroll), lanes);
        let time_view = (app.t_left, app.upp);
        let lane_at = (app.scroll + anchor.y) / app.row_h;
        apply_zoom_input(
            &mut app,
            WheelZoom {
                lanes: 90.0,
                handled: true,
                ..Default::default()
            },
            1.0,
            anchor,
        );
        assert!(((app.scroll + anchor.y) / app.row_h - lane_at).abs() < 1e-5);
        assert!(app.row_h > lanes.0);
        assert_eq!(
            (app.t_left, app.upp),
            time_view,
            "lane zoom cannot move time"
        );
        let lanes = (app.row_h, app.scroll);
        apply_zoom_input(&mut app, WheelZoom::default(), 1.2, anchor);
        assert!((app.upp - time_view.1 / f64::from(1.2f32)).abs() < 1e-8);
        assert_eq!((app.row_h, app.scroll), lanes, "pinch remains time-only");

        let valid = (app.t_left, app.upp, app.row_h, app.scroll);
        for invalid in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            apply_zoom_input(&mut app, WheelZoom::default(), invalid, anchor);
            assert_eq!((app.t_left, app.upp, app.row_h, app.scroll), valid);
        }
    }

    fn input_frame(
        input: egui::InputState,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::InputState {
        input.begin_pass(
            egui::RawInput {
                time: Some(time),
                predicted_dt: 1.0 / 60.0,
                events,
                ..Default::default()
            },
            true,
            1.0,
            egui::InputOptions::default(),
        )
    }

    #[test]
    fn slow_input_frames_never_replay_a_large_wheel_as_pinch() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        for modifiers in [
            ctrl,
            egui::Modifiers {
                shift: true,
                ..ctrl
            },
            egui::Modifiers {
                mac_cmd: true,
                command: true,
                ..Default::default()
            },
        ] {
            let mut app = one_bar_app(10_000_000);
            app.upp = 1000.0;
            app.t_left = 2_000_000.0;
            let anchor = egui::vec2(200.0, 120.0);
            let first_time = 1.0 / 60.0;
            let input = input_frame(
                egui::InputState::default(),
                first_time,
                vec![wheel_event(
                    modifiers,
                    egui::MouseWheelUnit::Line,
                    egui::vec2(0.0, 25.0),
                )],
            );
            assert!(input.zoom_delta() > 1.0);
            assert_eq!(pinch_zoom(&input), 1.0);
            apply_zoom_input(
                &mut app,
                wheel_zoom(&input.events),
                pinch_zoom(&input),
                anchor,
            );
            let after_wheel = (app.t_left, app.upp, app.row_h, app.scroll);

            // The real egui smoother still reports wheel zoom after the old
            // 200ms deadline when a frame is delayed. It is not a pinch.
            let input = input_frame(input, first_time + 0.25, vec![]);
            assert!(input.zoom_delta() > 1.05);
            assert!(input.is_scrolling());
            assert_eq!(pinch_zoom(&input), 1.0);
            apply_zoom_input(
                &mut app,
                wheel_zoom(&input.events),
                pinch_zoom(&input),
                anchor,
            );
            assert_eq!((app.t_left, app.upp, app.row_h, app.scroll), after_wheel);

            // A genuine backend pinch is independent even while the wheel
            // tail remains. Do not multiply by the mixed zoom_delta().
            let input = input_frame(
                input,
                first_time + 0.5,
                vec![egui::Event::Zoom(1.2), egui::Event::Zoom(1.25)],
            );
            let pinch = pinch_zoom(&input);
            assert_eq!(pinch, 1.5);
            assert!(input.zoom_delta() > pinch);
            apply_zoom_input(&mut app, wheel_zoom(&input.events), pinch, anchor);
            assert!((app.upp - after_wheel.1 / f64::from(pinch)).abs() < 1e-8);
            assert_eq!((app.row_h, app.scroll), (after_wheel.2, after_wheel.3));
        }
    }

    #[test]
    fn multitouch_pinch_uses_egui_priority_without_double_counting_raw_zoom() {
        let touch = |id, phase, x| egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id),
            phase,
            pos: egui::pos2(x, 50.0),
            force: None,
        };
        let input = input_frame(
            egui::InputState::default(),
            0.0,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
        );
        let input = input_frame(
            input,
            0.02,
            vec![
                touch(0, egui::TouchPhase::Start, 40.0),
                touch(1, egui::TouchPhase::Start, 60.0),
            ],
        );
        assert!(input.multi_touch().is_some());
        let input = input_frame(
            input,
            0.04,
            vec![
                touch(0, egui::TouchPhase::Move, 30.0),
                touch(1, egui::TouchPhase::Move, 70.0),
                egui::Event::Zoom(3.0),
            ],
        );
        assert_eq!(input.multi_touch().unwrap().zoom_delta, 2.0);
        assert_eq!(pinch_zoom(&input), 2.0);
    }

    #[test]
    fn reset_view_restores_both_axes_and_preserves_inspection_state() {
        let mut app = one_bar_app(10_000_000);
        app.canvas_w = 1000.0;
        app.canvas_h = 400.0;
        app.upp = 100.0;
        app.t_left = 3_000_000.0;
        app.row_h = ROW_H_MAX;
        app.scroll = 500.0;
        app.selected = Some(0);
        app.hist = vec![0];
        app.filter = "a".to_owned();
        app.matches = Some(vec![true]);
        app.cp_on = true;
        app.anc_set = Some([0].into_iter().collect());
        reset_view(&mut app);
        assert_eq!(view_window(&app), (0.0, 10_000_000.0));
        assert_eq!(app.scroll, 0.0);
        assert_eq!(
            app.row_h,
            config::get().appearance.row_height.unwrap_or(11.0)
        );
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.hist, [0]);
        assert_eq!(app.filter, "a");
        assert_eq!(app.matches, Some(vec![true]));
        assert!(app.cp_on);
        assert_eq!(app.anc_set, Some([0].into_iter().collect()));
        let reset = (app.t_left, app.upp, app.row_h, app.scroll);
        apply_zoom_input(&mut app, WheelZoom::default(), 1.2, egui::Vec2::ZERO);
        assert!((app.upp - reset.1 / f64::from(1.2f32)).abs() < 1e-8);
        assert_eq!(
            (app.t_left, app.row_h, app.scroll),
            (reset.0, reset.2, reset.3)
        );
    }

    #[test]
    fn reset_view_and_large_wheel_events_keep_degenerate_views_finite() {
        let mut app = one_bar_app(0);
        app.canvas_w = f32::NAN;
        app.canvas_h = 0.0;
        reset_view(&mut app);
        assert!(app.upp.is_finite() && app.upp > 0.0);
        assert_eq!(app.t_left, 0.0);
        assert_eq!(app.scroll, 0.0);
        let wheel = wheel_zoom(&[wheel_event(
            egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
            egui::MouseWheelUnit::Page,
            egui::vec2(f32::MAX, f32::MAX),
        )]);
        apply_zoom_input(&mut app, wheel, 1.0, egui::vec2(20.0, 20.0));
        assert!(app.upp.is_finite() && app.t_left.is_finite() && app.scroll.is_finite());
        assert_eq!(app.row_h, ROW_H_MAX);
    }

    /// The minimap and the horizontal scrollbar are painted outside the
    /// central panel, so they only line up with the bars if they are
    /// told where the plot is. Drawing them on the panel's own rect —
    /// what they used to do — sheared them a whole lane gutter left, so
    /// the viewport brush hugged the window frame while the view it
    /// tracked started 54 points further in.
    #[test]
    fn the_minimap_axis_is_the_plots_axis_not_the_windows() {
        let mut app = one_bar_app(1_000_000);
        // Derive the plot geometry the way `show_canvas` does rather
        // than restating the answer, so that moving the gutter or the
        // scrollbar in one place and not the other fails here instead
        // of shipping a brush that points at the wrong time.
        let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1500.0, 950.0));
        let plot_left = full.left() + GUTTER_W;
        let plot_right = full.right() - SB_W;
        app.canvas_x0 = plot_left;
        app.canvas_w = plot_right - plot_left;

        let strip =
            egui::Rect::from_min_max(egui::pos2(full.left(), 0.0), egui::pos2(full.right(), 46.0));
        let axis = super::plot_axis(&app, strip);
        assert_eq!(axis.left(), plot_left, "t=0 sits under the plot's 0s tick");
        assert_eq!(
            axis.right(),
            plot_right,
            "t=span sits under the plot's edge"
        );
        assert!(
            axis.left() > strip.left(),
            "the old bug: the axis started at the window edge, a whole \
             gutter left of the bars it was tracking"
        );
        assert_eq!(axis.top(), strip.top(), "the strip keeps its own height");
        assert_eq!(axis.bottom(), strip.bottom());
    }

    /// Before the first painted frame there is no measured geometry, and
    /// the fallback still must not put the axis on the window edge.
    #[test]
    fn the_minimap_axis_falls_back_to_the_default_plot_not_the_strip() {
        let mut app = one_bar_app(1_000_000);
        app.canvas_w = 0.0;
        let strip = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1500.0, 46.0));
        let axis = super::plot_axis(&app, strip);
        assert_eq!(axis.left(), GUTTER_W);
        assert_eq!(axis.width(), DEFAULT_CANVAS_W);
    }

    #[test]
    fn a_hidden_gutter_and_invalid_previous_geometry_use_the_current_strip() {
        let mut app = one_bar_app(1_000_000);
        app.show_gutter = false;
        app.canvas_w = 0.0;
        let strip = egui::Rect::from_min_size(egui::pos2(20.0, 0.0), egui::vec2(800.0, 46.0));
        for (x0, width) in [(0.0, 0.0), (f32::NAN, 100.0), (10.0, f32::INFINITY)] {
            app.canvas_x0 = x0;
            app.canvas_w = width;
            let axis = plot_axis(&app, strip);
            assert_eq!(axis.left(), strip.left());
            assert_eq!(axis.right(), strip.right() - SB_W);
        }
    }

    #[test]
    fn small_windows_do_not_create_negative_plot_extents() {
        for size in [egui::vec2(0.0, 0.0), egui::vec2(30.0, 40.0)] {
            let full = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            assert_eq!(canvas_size(full, GUTTER_W, 52.0), egui::Vec2::ZERO);
        }
        let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0));
        assert_eq!(
            canvas_size(full, GUTTER_W, 52.0),
            egui::vec2(DEFAULT_CANVAS_W, 886.0)
        );
        assert_eq!(action_label_font_size(ROW_H_MIN), None);
        assert_eq!(action_label_font_size(9.0), None);
        assert_eq!(action_label_font_size(11.0), Some(8.0));
        assert_eq!(action_label_font_size(ROW_H_MAX), Some(10.0));
    }

    /// Aaron read the brush as misplaced, and the arithmetic is the
    /// place that would go wrong: the window it brackets has to be
    /// measured with the plot's width, the same number the ruler ticks
    /// are spaced by. Take it from the panel or the OS window instead
    /// and the brush is a gutter plus a scrollbar too wide — visibly so
    /// once the view is zoomed in far enough that the whole window is a
    /// few seconds.
    #[test]
    fn the_brush_brackets_exactly_the_time_the_ruler_shows() {
        let mut app = one_bar_app(10_000_000);
        app.canvas_x0 = GUTTER_W;
        app.canvas_w = 1000.0;
        app.upp = 2_000.0; // 2ms per point: a 2s window of a 10s build
        app.t_left = 3_000_000.0;
        let (t0, t1) = super::view_window(&app);
        assert_eq!(t0, 3_000_000.0);
        assert_eq!(t1 - t0, 2_000_000.0, "1000 points at 2ms each");

        // The tick the ruler puts at the right edge of the plot is
        // t_left + upp * canvas_w, so that is where the brush must end.
        let strip = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1500.0, 46.0));
        let axis = super::plot_axis(&app, strip);
        let t_at_right = app.t_left + app.upp * f64::from(axis.width());
        assert_eq!(t1, t_at_right);

        // The window is wider than the plot by the gutter and the
        // scrollbar; measuring against it would overstate the window.
        assert!(t_left_at(&app, strip.width()) > t1);
    }

    fn t_left_at(app: &Ready, w: f32) -> f64 {
        app.t_left + app.upp * f64::from(w)
    }

    fn app_from(events: &str, tag: &str) -> Trace {
        let p = crate::temp_path(&format!("mini-{tag}.json"));
        std::fs::write(&p, format!("[\n{events}")).unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        std::fs::remove_file(&p).ok();
        tr
    }

    /// Aaron's complaint, as a test: the timeline showed thirty lanes of
    /// `generate_hmap` and the overview under it showed a blue-and-yellow
    /// mix, because the strip was averaging every action in flight while
    /// the screen showed the handful that fit. On the shared lane axis a
    /// band of lanes has to carry the class those lanes were running.
    #[test]
    fn a_band_of_lanes_keeps_its_own_colour_in_the_strip() {
        // Two classes, 200 lanes each, running at the same time. Lanes
        // are handed out lowest-free-first in start order, so the .cpp
        // actions take the low band and the .a actions the high one.
        let mut ev = String::new();
        for i in 0..200 {
            ev.push_str(&format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a{i}.cpp\",\
                 \"ts\":0,\"dur\":10000000}},\n"
            ));
        }
        for i in 0..200 {
            ev.push_str(&format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"lib{i}.a\",\
                 \"ts\":1000000,\"dur\":9000000}},\n"
            ));
        }
        let tr = app_from(&ev, "bands");
        let n = tr.lanes.len();
        let low = tr.acts.iter().find(|a| a.ts == 0).expect("a .cpp survived");
        let high = tr
            .acts
            .iter()
            .max_by_key(|a| a.lane)
            .expect("a .a survived");
        assert_ne!(low.color, high.color, "the two classes differ");
        assert!(high.lane >= 200, "the second class took the high band");
        assert_eq!(low.lane, 0, "the first class started at the bottom lane");

        let (w, h) = (64usize, 128usize);
        let map = super::lane_time_map(&tr, w, h);
        let cell = |t_us: f64, lane: u32| {
            let c = (((t_us / tr.span as f64) * w as f64) as usize).min(w - 1);
            let row = ((super::lane_frac(lane as usize, n) * h as f64) as usize).min(h - 1);
            map[c * h + row]
        };
        assert_eq!(cell(5_000_000.0, low.lane), low.color);
        assert_eq!(cell(5_000_000.0, high.lane), high.color);
        // ...and the high band is empty before its class started.
        assert_eq!(cell(500_000.0, high.lane), 0xff);
    }

    /// Lanes are packed, so a column is filled from the top down to
    /// whatever was in flight — which is what lets the strip say how busy
    /// the build was without a separate curve drawn over it. The axis is
    /// squashed, so the test pins the ordering rather than the scale: a
    /// busier moment must never draw shorter than a quieter one.
    #[test]
    fn the_silhouette_never_disagrees_with_the_concurrency_profile() {
        let mut ev = String::new();
        for i in 0..40 {
            ev.push_str(&format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a{i}.cpp\",\
                 \"ts\":{},\"dur\":{}}},\n",
                i * 1_000_000,
                (i % 7 + 1) * 3_000_000
            ));
        }
        let tr = app_from(&ev, "silhouette");
        let (conc, _) = build_concurrency(&tr);
        let h = 64usize;
        let map = super::lane_time_map(&tr, conc.len(), h);
        let filled: Vec<usize> = (0..conc.len())
            .map(|c| {
                map[c * h..(c + 1) * h]
                    .iter()
                    .filter(|&&k| k != 0xff)
                    .count()
            })
            .collect();
        for (c, &want) in conc.iter().enumerate() {
            assert_eq!(
                filled[c] == 0,
                want == 0,
                "bucket {c}: {} rows lit, {want} running",
                filled[c]
            );
            for (d, &other) in conc.iter().enumerate() {
                if want < other {
                    assert!(
                        filled[c] <= filled[d],
                        "bucket {c} ({want} running) drew {} rows, \
                         taller than bucket {d} ({other} running) at {}",
                        filled[c],
                        filled[d]
                    );
                }
            }
        }
    }

    /// The brush's height is the answer to "which of these lanes am I
    /// looking at", so it has to come off the same axis the picture is
    /// drawn on. Reading it as a plain fraction of the lane count — the
    /// obvious thing to write — puts a box a quarter of the way down when
    /// the picture has those lanes half way down.
    #[test]
    fn the_brush_height_is_read_off_the_same_lane_axis_as_the_picture() {
        let mut ev = String::new();
        for i in 0..400 {
            ev.push_str(&format!(
                "{{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a{i}.cpp\",\
                 \"ts\":0,\"dur\":10000000}},\n"
            ));
        }
        let mut app = Ready::new(app_from(&ev, "brush"), None);
        app.canvas_h = 600.0;
        app.row_h = 12.0;
        app.scroll = 0.0;
        let n = app.tr.lanes.len();
        let (f0, f1) = super::lane_window(&app);
        assert_eq!(f0, 0.0, "scrolled to the top");
        assert_eq!(f1, super::lane_frac(50, n), "600 points of 12 is 50 lanes");

        // Halving the row height doubles the lanes on screen, and the
        // brush has to grow to say so.
        app.row_h = 6.0;
        let (_, taller) = super::lane_window(&app);
        assert!(taller > f1, "{taller} should exceed {f1}");
    }

    #[test]
    fn concurrency_buckets_survive_a_bar_that_rounds_onto_the_span_end() {
        // ts/span rounds to exactly 1.0 in f64 once the span outgrows
        // 2^53 us, putting the raw start bucket one past the difference
        // array; the clamp must keep it in range instead of panicking
        let p = crate::temp_path("conc.json");
        std::fs::write(
            &p,
            concat!(
                "[\n",
                r#"{"ph":"X","cat":"recipe","name":"a","ts":0,"dur":1},"#,
                "\n",
                r#"{"ph":"X","cat":"recipe","name":"b","ts":9223372036854775000,"dur":9},"#,
                "\n",
            ),
        )
        .unwrap();
        let tr = parse::parse(p.to_str().unwrap()).unwrap();
        std::fs::remove_file(&p).ok();
        let (out, max) = build_concurrency(&tr);
        assert_eq!(out.len(), 8192);
        assert!(max >= 1, "both bars counted somewhere in range");
    }

    /// A trace whose every action is instantaneous has a span of zero, and
    /// one with no action at all has no lane, so both scrollbars divide by
    /// zero. egui accepts a NaN rect and paints it, so nothing downstream
    /// reports the mistake.
    #[test]
    fn a_thumb_stays_finite_when_there_is_nothing_to_scroll() {
        for (pos, view, min_len, min_pos) in [(0.0, 0.0, 0.005, -0.5), (0.0, 640.0, 0.02, 0.0)] {
            let (p, len) = super::thumb(pos, view, 0.0, min_len, min_pos);
            assert!(p.is_finite() && len.is_finite(), "pos {p}, len {len}");
            assert!((min_pos as f32..=1.0).contains(&p), "pos {p}");
            assert!((min_len as f32..=1.0).contains(&len), "len {len}");
        }

        let (p, len) = super::thumb(250.0, 500.0, 1000.0, 0.02, 0.0);
        assert_eq!((p, len), (0.25, 0.5), "a real total is still divided by");
    }
}
