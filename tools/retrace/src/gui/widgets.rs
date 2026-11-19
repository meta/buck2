/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Popup machinery and the shared table widgets.

use std::cmp::Ordering;
use std::collections::HashMap;

use egui_extras::Column;
use egui_extras::TableBuilder;

use crate::analyses::Columns;
use crate::analyses::fmt_bytes_f;
use crate::config;
use crate::detail::DataMoved;
use crate::humanize;
use crate::types::F_FAILED;
use crate::types::Trace;

/// A failed action, everywhere one is named or drawn.
pub fn fail_color() -> egui::Color32 {
    config::ui_color(config::UI_FAILURE)
}

/// Diagonal hatching over a filled rect. Callers supply an explicit legend:
/// the counter view marks missing measurements; the observed-activity view
/// marks a canceled state with recorded boundaries (not unmeasured time).
pub fn hatch(p: &egui::Painter, r: egui::Rect) {
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(0x70));
    let mut x = r.left() - r.height();
    while x < r.right() {
        let a = egui::pos2(x.max(r.left()), r.bottom() - (r.left() - x).max(0.0));
        let b = egui::pos2(
            (x + r.height()).min(r.right()),
            r.top() + (x + r.height() - r.right()).max(0.0),
        );
        if a.x < b.x {
            p.line_segment([a, b], stroke);
        }
        x += 4.0;
    }
}

/// One analysis/detail popup. Native mode (the default) renders it as a
/// real OS window — draggable anywhere, own title bar — via an immediate
/// viewport; View ▸ "Popups inside main window" keeps the embedded
/// egui::Window behavior (which is also what the snapshot tests render).
/// `open` goes false when the user closes it either way.
pub fn popup(
    ctx: &egui::Context,
    native: bool,
    key: &str,
    title: &str,
    open: &mut bool,
    size: [f32; 2],
    add: impl FnOnce(&mut egui::Ui),
) {
    if !*open {
        return;
    }
    // A native popup opens on the desktop Space, which a full-screen main
    // window cannot see (macOS Spaces isolate full-screen apps) — fall
    // back to embedded popups there so they are never silently invisible.
    let (root_rect, root_fullscreen) = ctx.input(|i| {
        let v = i.viewport();
        (v.inner_rect, v.fullscreen.unwrap_or(false))
    });
    let native = native && !root_fullscreen;
    let gen_id = egui::Id::new(("popup_gen", key));
    let generation: u64 = ctx.data(|d| d.get_temp(gen_id).unwrap_or(0));
    let placed_id = egui::Id::new(("popup_placed", key, generation));
    if native {
        let mut add = Some(add);
        let vid = egui::ViewportId::from_hash_of(("popup", key, generation));
        let mut builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_window_level(egui::WindowLevel::AlwaysOnTop);
        // Place the window once. `ViewportBuilder::patch` re-sends the
        // position whenever it differs from the last one, and the cascade
        // origin follows the main window — so a builder position supplied
        // every frame drags every popup back into a pile the moment the
        // main window moves or its top-left edge is dragged.
        if !ctx.data(|d| d.get_temp::<bool>(placed_id).unwrap_or(false)) {
            builder = builder.with_position(cascade_pos(ctx, key, root_rect));
            ctx.data_mut(|d| d.insert_temp(placed_id, true));
        }
        // palette behavior: popups float above the main window while the
        // app is active; the shell demotes them when the app deactivates.
        ctx.data_mut(|dd| {
            dd.get_temp_mut_or_default::<Vec<egui::ViewportId>>(egui::Id::new("open_popups"))
                .push(vid);
        });
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().focused.unwrap_or(false)) {
                ui.ctx().data_mut(|dd| {
                    dd.insert_temp(egui::Id::new("popup_focused"), true);
                });
            }
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(add) = add.take() {
                            add(ui);
                        }
                    });
            });
            let ctx = ui.ctx();
            if ctx.input(|i| i.viewport().close_requested()) {
                *open = false;
                // The user destroyed this OS window (red X): the next
                // open gets a FRESH viewport id so a new window is
                // genuinely created (resurrecting a destroyed viewport
                // under the same id is unreliable).
                ctx.data_mut(|d| d.insert_temp(gen_id, generation + 1));
            }
            if !ctx.egui_wants_keyboard_input()
                && ctx.input(|i| i.modifiers.shift && i.key_pressed(egui::Key::Escape))
            {
                ctx.data_mut(|d| d.insert_temp(close_all_req_id(), true));
            }
        });
    } else {
        // No native viewport exists for this key now, so the next native
        // open is creating a window rather than updating one and has to
        // place it again. Both the fullscreen fallback above and the View
        // menu land here without touching the generation.
        ctx.data_mut(|d| d.remove::<bool>(placed_id));
        let window = egui::Window::new(title)
            .id(egui::Id::new(("popup", key)))
            .open(open)
            .default_width(size[0])
            .vscroll(true);
        // Keep the detail lanes, overview footer, and CP controls visible
        // on first open in embedded/fullscreen mode as in native mode.
        let window = if matches!(
            key,
            "detail" | "Build overview" | "Critical path (dependency graph)"
        ) {
            window.default_height(size[1])
        } else {
            window
        };
        window.show(ctx, |ui| add(ui));
    }
}

/// Where a native popup parks a ⇧Esc: that window owns OS keyboard focus
/// while it is in front, so the root viewport never sees the press.
fn close_all_req_id() -> egui::Id {
    egui::Id::new("close_all_popups")
}

/// Takes the pending "close every popup" request, if one was raised.
pub fn take_close_all(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(close_all_req_id()))
        .unwrap_or(false)
}

/// Where popup `key`'s window first appears: a cascade near the main
/// window's origin, in the order the popups were first opened. Slots wrap
/// well before the offset could push a window off a laptop screen.
fn cascade_pos(ctx: &egui::Context, key: &str, root_rect: Option<egui::Rect>) -> egui::Pos2 {
    const SLOTS: u32 = 10;
    const STEP: f32 = 26.0;
    let slot_id = egui::Id::new(("popup_slot", key));
    let slot = ctx.data_mut(|d| match d.get_temp::<u32>(slot_id) {
        Some(s) => s,
        None => {
            let next_id = egui::Id::new("popup_slot_next");
            let s: u32 = d.get_temp(next_id).unwrap_or(0);
            d.insert_temp(next_id, (s + 1) % SLOTS);
            d.insert_temp(slot_id, s);
            s
        }
    }) as f32;
    root_rect
        .map(|r| r.left_top() + egui::vec2(80.0 + slot * STEP, 80.0 + slot * STEP))
        .unwrap_or(egui::pos2(120.0, 120.0))
}

/// Raise (or queue focus for) the native window of popup `key`: opening
/// something already open must bring the buried window back above the
/// main one.
pub fn popup_raise(ctx: &egui::Context, key: &str) {
    let gen_id = egui::Id::new(("popup_gen", key));
    let generation: u64 = ctx.data(|d| d.get_temp(gen_id).unwrap_or(0));
    ctx.send_viewport_cmd_to(
        egui::ViewportId::from_hash_of(("popup", key, generation)),
        egui::ViewportCommand::Focus,
    );
}

/// Where a right-click context menu is pinned, and the pass it was
/// opened on.
///
/// The pass number is what keeps the menu on screen. egui gives an
/// `Area` id it has not seen before one sizing pass, drawn invisible
/// with `interactable` false; `Areas::layer_id_at` skips a layer that is
/// not interactable, so on that pass the click that opened the menu
/// reads as a click on whatever is behind it and
/// `Response::clicked_elsewhere` is true no matter where the pointer is.
/// Dismissing on that answer closes the menu on the frame it opened.
///
/// The stamp is taken in the constructor because there is no other way
/// to make one of these: a caller cannot open a menu and forget it.
#[derive(Clone, Copy, Debug)]
pub struct MenuAt {
    pub at: egui::Pos2,
    pass: u64,
}

impl MenuAt {
    pub fn open(ctx: &egui::Context, at: egui::Pos2) -> Self {
        Self {
            at,
            pass: ctx.cumulative_pass_nr(),
        }
    }
}

/// A context menu pinned where the pointer opened it, above everything
/// else. Returns what `add` returned, and whether the user clicked
/// outside the menu — which callers combine with their own reasons to
/// close before clearing the menu's state.
pub fn ctx_menu<R>(
    ctx: &egui::Context,
    id: &str,
    menu: MenuAt,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool) {
    let area = egui::Area::new(egui::Id::new(id))
        .fixed_pos(menu.at)
        .order(egui::Order::Foreground)
        .show(ctx, add);
    let dismissed = ctx.cumulative_pass_nr() > menu.pass && area.response.clicked_elsewhere();
    (area.inner, dismissed)
}

/// A generic sortable table: display strings plus the raw values to
/// sort on, so "10 minutes" orders after "625 s" the way a reader
/// expects.
pub struct Tbl {
    pub cols: Columns,
    pub rows: Vec<TRow>,
}

pub struct TRow {
    pub cells: Vec<String>,
    pub keys: Vec<f64>,
    pub act: Option<usize>,
    pub hover: Option<String>,
}

/// Row indices in display order. A column that has no numeric key is
/// `NaN` in every row and sorts by its displayed text; a key that is
/// `NaN` in only some rows has no place in that column's numeric order,
/// so it sorts to the end. Keeping those two cases apart is what makes
/// the comparison transitive: mixing them would let `a < b`, `b < c` and
/// `c < a` all hold at once, which `sort_by` treats as a caller bug.
fn row_order(rows: &[TRow], (col, desc): (u8, bool)) -> Vec<usize> {
    let cell = |r: &TRow| r.cells.get(col as usize).cloned().unwrap_or_default();
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        let (ra, rb) = (&rows[a], &rows[b]);
        let ka = ra.keys.get(col as usize).copied().unwrap_or(f64::NAN);
        let kb = rb.keys.get(col as usize).copied().unwrap_or(f64::NAN);
        let o = match (ka.is_nan(), kb.is_nan()) {
            (false, false) => ka.total_cmp(&kb),
            (true, true) => cell(ra).cmp(&cell(rb)),
            // Not reversed: unrankable rows stay at the end in both directions.
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
        };
        if desc { o.reverse() } else { o }
    });
    order
}

pub fn draw_tbl(
    ui: &mut egui::Ui,
    id: &str,
    t: &Tbl,
    sort: (u8, bool),
    nav: &mut Option<usize>,
) -> Option<(u8, bool)> {
    let (col, desc) = sort;
    let order = row_order(&t.rows, sort);
    let mut new_sort = None;
    egui::Grid::new(id)
        .num_columns(t.cols.len())
        .striped(true)
        .show(ui, |ui| {
            for (i, (name, w, _)) in t.cols.iter().enumerate() {
                let arrow = if i as u8 == col {
                    if desc { " \u{25be}" } else { " \u{25b4}" }
                } else {
                    ""
                };
                let mut label = name.to_string();
                if let Some(c) = label.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                ui.scope(|ui| {
                    ui.set_min_width(*w as f32 * 0.85);
                    if ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(format!("{label}{arrow}")).strong(),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text("click to sort")
                        .clicked()
                    {
                        new_sort = Some(if i as u8 == col {
                            (col, !desc)
                        } else {
                            (i as u8, true)
                        });
                    }
                });
            }
            ui.end_row();
            for pos in order {
                let r = &t.rows[pos];
                let mut clicked = false;
                for (i, cell) in r.cells.iter().enumerate() {
                    let align = t.cols.get(i).map(|c| c.2).unwrap_or('w');
                    let text = egui::RichText::new(cell).monospace().size(11.0);
                    let resp = if align == 'w' {
                        ui.add(egui::Label::new(text).wrap().sense(egui::Sense::click()))
                    } else {
                        ui.add(egui::Label::new(text).extend().sense(egui::Sense::click()))
                    };
                    let resp = match (&r.hover, i) {
                        (Some(h), 0) => resp.on_hover_text(h),
                        _ => resp,
                    };
                    clicked |= resp.clicked();
                }
                if clicked {
                    if let Some(a) = r.act {
                        *nav = Some(a);
                    }
                }
                ui.end_row();
            }
        });
    new_sort
}

/// One sortable action list, shared by every panel that shows a list of
/// actions (critical path, longest, failed). Cells render
/// human-readably; sorting always uses the underlying raw value.
/// Optional [`act_table`] extras: per-panel columns and interactions.
#[derive(Default)]
pub struct ActTableOpts<'a> {
    /// Absolute origin for recorded Start values; defaults to the trace origin.
    pub origin_us: Option<i64>,
    /// adds a sortable "Execution" column (execute-stage time)
    pub exec_us: Option<&'a HashMap<usize, i64>>,
    /// adds a sortable "Savings" column (\u{2026} while computing)
    pub savings_us: Option<&'a HashMap<usize, i64>>,
    /// adds sortable "Inputs" and "Outputs" columns: the data each action moved
    pub data: Option<&'a HashMap<usize, DataMoved>>,
    /// replaces the Duration column's value and sort key, so a simulated
    /// path's rows cannot disagree with the totals drawn around them
    pub dur_us: Option<&'a HashMap<usize, i64>>,
    /// receives `(act, pos)` when a row is right-clicked
    pub sec: Option<&'a mut Option<(usize, egui::Pos2)>>,
    /// highlights this action regardless of the current display order
    pub selected: Option<usize>,
    /// one-shot request to reveal this action inside the enclosing scroller
    pub reveal: Option<usize>,
    /// Total table height, including fixed headings. The body owns its only
    /// scroller and renders visible rows; callers must not wrap this in one.
    pub max_height: Option<f32>,
}

struct ActionCell {
    label: egui::Label,
    hover: Option<String>,
}

impl ActionCell {
    fn show(self, ui: &mut egui::Ui, selected: Option<bool>) -> egui::Response {
        let label = selected.map(|_| self.label.text().to_owned());
        let mut response = ui.add(self.label.sense(egui::Sense::click()));
        if let (Some(label), Some(selected)) = (label, selected) {
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    response.enabled(),
                    selected,
                    label.clone(),
                )
            });
        }
        if let Some(hover) = self.hover {
            response = response.on_hover_text(hover);
        }
        response
    }
}

fn selected_action_row(items: &[usize], order: &[usize], selected: Option<usize>) -> Option<usize> {
    order
        .iter()
        .position(|&pos| Some(items[pos]) == selected)
        .map(|row| row + 1) // the column headings occupy row zero
}

fn recorded_action_start(ts: i64, trace_origin: i64, command_origin: i64) -> i64 {
    (i128::from(ts) + i128::from(trace_origin) - i128::from(command_origin))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

pub fn act_table(
    ui: &mut egui::Ui,
    tr: &Trace,
    items: &[usize],
    id: &str,
    sort: (u8, bool),
    nav: &mut Option<usize>,
    opts: ActTableOpts<'_>,
) -> Option<(u8, bool)> {
    let ActTableOpts {
        origin_us,
        exec_us,
        savings_us,
        data,
        dur_us,
        mut sec,
        selected,
        reveal,
        max_height,
    } = opts;
    let dur_of = |i: usize| {
        dur_us
            .and_then(|m| m.get(&i).copied())
            .unwrap_or(tr.acts[i].dur)
    };
    let (col, desc) = sort;
    let mut headers: Vec<&str> = vec![
        "#",
        if dur_us.is_some() {
            "Recorded start"
        } else {
            "Start"
        },
        "Duration",
    ];
    let exec_col = exec_us.map(|_| {
        headers.push("Execution");
        headers.len() as u8 - 1
    });
    let savings_col = savings_us.map(|_| {
        headers.push("Savings");
        headers.len() as u8 - 1
    });
    let data_cols = data.map(|_| {
        headers.push("Inputs");
        headers.push("Outputs");
        (headers.len() as u8 - 2, headers.len() as u8 - 1)
    });
    headers.push("Action");
    let action_col = headers.len() as u8 - 1;
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        let (ia, ib) = (items[a], items[b]);
        let by_map = |m: &HashMap<usize, i64>| {
            let e = |i: usize| m.get(&i).copied().unwrap_or(-1);
            e(ia).cmp(&e(ib))
        };
        let o = if col == 1 {
            tr.acts[ia].ts.cmp(&tr.acts[ib].ts)
        } else if col == 2 {
            dur_of(ia).cmp(&dur_of(ib))
        } else if exec_col == Some(col)
            && let Some(m) = exec_us
        {
            by_map(m)
        } else if savings_col == Some(col)
            && let Some(m) = savings_us
        {
            by_map(m)
        } else if let Some((inputs, outputs)) = data_cols
            && (col == inputs || col == outputs)
            && let Some(m) = data
        {
            let size = |i: usize| {
                let d = m.get(&i).copied().unwrap_or_default();
                if col == inputs { d.inputs } else { d.outputs }.unwrap_or(-1.0)
            };
            size(ia).total_cmp(&size(ib))
        } else if col == action_col {
            tr.names[tr.acts[ia].name as usize].cmp(&tr.names[tr.acts[ib].name as usize])
        } else {
            a.cmp(&b) // "#" = the order the analysis produced
        };
        if desc { o.reverse() } else { o }
    });

    let mut new_sort = None;
    let mut draw_header = |ui: &mut egui::Ui, i: usize| {
        let arrow = if i as u8 == col {
            if desc { " \u{25be}" } else { " \u{25b4}" }
        } else {
            ""
        };
        if ui
            .add(
                egui::Label::new(egui::RichText::new(format!("{}{arrow}", headers[i])).strong())
                    .sense(egui::Sense::click()),
            )
            .on_hover_text("click to sort")
            .clicked()
        {
            new_sort = Some(if i as u8 == col {
                (col, !desc)
            } else {
                (i as u8, i as u8 != action_col)
            });
        }
    };
    // Both table layouts use exactly the same cell values, accessibility,
    // hover explanations and action identity. Only their geometry differs.
    let cells = |pos: usize, clipped: bool| {
        let idx = items[pos];
        let a = tr.acts[idx];
        let label =
            |text: String| egui::Label::new(egui::RichText::new(text).monospace().size(11.0));
        let start = recorded_action_start(a.ts, tr.t0_abs, origin_us.unwrap_or(tr.t0_abs));
        let dur = dur_of(idx);
        let mut cells = vec![
            ActionCell {
                label: label(format!("{}", pos + 1)),
                hover: None,
            },
            ActionCell {
                label: label(format!("+{}", humanize::dur(start as f64))),
                hover: Some(format!(
                    "Recorded +{:.3}s from the selected command's origin; not a simulated start",
                    start as f64 / 1e6
                )),
            },
            ActionCell {
                label: label(humanize::dur(dur as f64)),
                hover: Some(if dur == a.dur {
                    format!("{:.3}s exactly", a.dur as f64 / 1e6)
                } else {
                    format!(
                        "simulated {:.3}s \u{2014} measured {:.3}s",
                        dur as f64 / 1e6,
                        a.dur as f64 / 1e6
                    )
                }),
            },
        ];
        if let Some(m) = exec_us {
            cells.push(ActionCell {
                label: label(match m.get(&idx) {
                    Some(&us) => humanize::dur(us as f64),
                    None => "\u{2014}".to_owned(),
                }),
                hover: Some("Recorded execution-stage counter; the difference from Duration is not necessarily a measured wait.".into()),
            });
        }
        if let Some(m) = savings_us {
            cells.push(ActionCell {
                label: label(match m.get(&idx) {
                    Some(&us) => humanize::dur(us as f64),
                    None => "\u{2026}".to_owned(),
                }),
                hover: Some(
                    "dependency-model reduction if this step ALONE were free (cp-replay); \
                     \u{2026} = still computing. These are independent \
                     leave-one-out replays against the same baseline \u{2014} they \
                     do not add."
                        .into(),
                ),
            });
        }
        if let Some(m) = data {
            let d = m.get(&idx).copied().unwrap_or_default();
            let size = |b: Option<f64>| b.map_or_else(|| "\u{2014}".to_owned(), fmt_bytes_f);
            cells.push(ActionCell {
                label: label(size(d.inputs)),
                hover: Some(d.inputs_note()),
            });
            cells.push(ActionCell {
                label: label(size(d.outputs)),
                hover: Some(d.outputs_note()),
            });
        }
        let name = &tr.names[a.name as usize];
        let mut text = egui::RichText::new(name).monospace().size(11.0);
        if a.flags & F_FAILED != 0 {
            text = text.color(fail_color());
        }
        cells.push(ActionCell {
            label: if clipped {
                egui::Label::new(text).truncate()
            } else {
                egui::Label::new(text).wrap()
            },
            hover: clipped.then(|| name.to_owned()),
        });
        cells
    };
    let mut accept_click = |idx, response: egui::Response| {
        if response.secondary_clicked() {
            if let Some(out) = sec.as_mut()
                && let Some(pos) = response.interact_pointer_pos()
            {
                **out = Some((idx, pos));
            }
        } else if response.clicked() {
            *nav = Some(idx);
        }
    };

    if let Some(max_height) = max_height {
        let header_height = ui.text_style_height(&egui::TextStyle::Body);
        let row_height = ui.fonts_mut(|fonts| fonts.row_height(&egui::FontId::monospace(11.0)));
        let body_height = (max_height - header_height - ui.spacing().item_spacing.y).max(0.0);
        let mut table = TableBuilder::new(ui)
            .id_salt((id, "bounded"))
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::click())
            .min_scrolled_height(0.0)
            .max_scroll_height(body_height)
            .auto_shrink([false, true]);
        for _ in 0..headers.len() - 1 {
            table = table.column(Column::auto().at_least(58.0));
        }
        table = table.column(Column::remainder().clip(true));
        if let Some(row) = order.iter().position(|&pos| reveal == Some(items[pos])) {
            table = table.scroll_to_row(row, Some(egui::Align::Center));
        }
        table
            .header(header_height, |mut row| {
                for i in 0..headers.len() {
                    row.col(|ui| draw_header(ui, i));
                }
            })
            .body(|body| {
                body.rows(row_height, order.len(), |mut row| {
                    let pos = order[row.index()];
                    let idx = items[pos];
                    let is_selected = selected == Some(idx);
                    row.set_selected(is_selected);
                    for cell in cells(pos, true) {
                        row.col(|ui| {
                            if is_selected {
                                ui.visuals_mut().override_text_color =
                                    Some(ui.visuals().selection.stroke.color);
                            }
                            let response = cell.show(ui, selected.map(|_| is_selected));
                            accept_click(idx, response);
                        });
                    }
                    accept_click(idx, row.response());
                });
            });
        return new_sort;
    }

    let selected_row = selected_action_row(items, &order, selected);
    let virtualized = order.len() > 1000;
    let row_height = ui.spacing().interact_size.y;
    let mut draw = |ui: &mut egui::Ui, range: std::ops::Range<usize>| {
        egui::Grid::new(id)
            .num_columns(headers.len())
            .with_row_color(move |row, style| {
                if Some(row + range.start) == selected_row {
                    Some(style.visuals.selection.bg_fill)
                } else if row % 2 == 1 {
                    Some(style.visuals.faint_bg_color)
                } else {
                    None
                }
            })
            .min_col_width(58.0)
            .min_row_height(if virtualized { row_height } else { 0.0 })
            .show(ui, |ui| {
                if range.start == 0 {
                    for i in 0..headers.len() {
                        draw_header(ui, i);
                    }
                    ui.end_row();
                }
                for row in range.start.max(1)..range.end {
                    let pos = order[row - 1];
                    let idx = items[pos];
                    let is_selected = selected == Some(idx);
                    let previous_color = ui.visuals().override_text_color;
                    if is_selected {
                        ui.visuals_mut().override_text_color =
                            Some(ui.visuals().selection.stroke.color);
                    }
                    let mut response: Option<egui::Response> = None;
                    for cell in cells(pos, virtualized) {
                        let cell = cell.show(ui, selected.map(|_| is_selected));
                        response = Some(match response {
                            Some(previous) => previous.union(cell),
                            None => cell,
                        });
                    }
                    if let Some(response) = response {
                        if reveal == Some(idx) {
                            ui.scroll_to_rect(response.rect, Some(egui::Align::Center));
                        }
                        accept_click(idx, response);
                    }
                    if is_selected {
                        ui.visuals_mut().override_text_color = previous_color;
                    }
                    ui.end_row();
                }
            });
    };
    if virtualized {
        let mut scroll = egui::ScrollArea::vertical().id_salt((id, "virtual_rows"));
        if let Some(row) = selected_action_row(items, &order, reveal) {
            scroll = scroll
                .vertical_scroll_offset(row as f32 * (row_height + ui.spacing().item_spacing.y));
        }
        scroll.show_rows(ui, row_height, order.len() + 1, |ui, range| draw(ui, range));
    } else {
        draw(ui, 0..order.len() + 1);
    }
    new_sort
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_start_uses_the_command_origin_not_first_action() {
        assert_eq!(recorded_action_start(100, 1_000_100, 1_000_000), 200);
        assert_eq!(recorded_action_start(100, 1_000_100, 1_000_100), 100);
        assert_eq!(
            recorded_action_start(i64::MAX, i64::MAX, i64::MAX),
            i64::MAX
        );
    }

    #[test]
    fn action_selection_tracks_identity_in_sorted_display_order() {
        let items = [40, 20, 60];
        assert_eq!(selected_action_row(&items, &[0, 1, 2], Some(40)), Some(1));
        assert_eq!(selected_action_row(&items, &[2, 1, 0], Some(40)), Some(3));
        assert_eq!(selected_action_row(&items, &[2, 0, 1], Some(20)), Some(3));
        assert_eq!(selected_action_row(&items, &[0, 1, 2], Some(99)), None);
        assert_eq!(selected_action_row(&items, &[0, 1, 2], None), None);
    }

    fn row(cell: &str, key: f64) -> TRow {
        TRow {
            cells: vec![cell.to_owned()],
            keys: vec![key],
            act: None,
            hover: None,
        }
    }

    #[test]
    fn a_column_with_no_numeric_key_sorts_by_text() {
        let rows = vec![
            row("charlie", f64::NAN),
            row("alpha", f64::NAN),
            row("bravo", f64::NAN),
        ];
        assert_eq!(row_order(&rows, (0, false)), vec![1, 2, 0]);
        assert_eq!(row_order(&rows, (0, true)), vec![0, 2, 1]);
    }

    /// A ratio key computes to `NaN` on the one row with a zero
    /// denominator while its neighbours stay numeric. Comparing that row
    /// by text and the rest by value is not a total order, so the sort is
    /// entitled to panic or to return anything at all; the numeric rows
    /// have to keep their order and the unrankable one has to go to the
    /// end.
    #[test]
    fn one_nan_key_among_numbers_sorts_to_the_end() {
        let rows = vec![row("a", 3.0), row("b", f64::NAN), row("c", 1.0)];
        assert_eq!(row_order(&rows, (0, false)), vec![2, 0, 1]);
        assert_eq!(
            row_order(&rows, (0, true)),
            vec![0, 2, 1],
            "descending still puts the unknown last"
        );
    }

    #[test]
    fn a_row_too_short_for_the_column_is_unrankable() {
        let rows = vec![
            row("a", 2.0),
            TRow {
                cells: Vec::new(),
                keys: Vec::new(),
                act: None,
                hover: None,
            },
            row("c", 1.0),
        ];
        assert_eq!(row_order(&rows, (0, false)), vec![2, 0, 1]);
        assert_eq!(row_order(&rows, (0, true)), vec![0, 2, 1]);
    }
}
