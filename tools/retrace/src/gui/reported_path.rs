/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The critical path buck2 computed during a build, for captures recorded without
//! `--log-action-graph`. There is no dependency graph to replay, but the path buck2 reported is
//! in the log, and its action steps open the same details as the timeline.

use super::Ready;
use super::widgets;
use crate::command_profile::BuckCriticalPath;
use crate::command_profile::CriticalPathEntry;
use crate::humanize;

pub const TITLE: &str = "Critical path (reported by buck2)";

#[derive(Default)]
pub struct State {
    /// Index into `rows` of the selected step.
    step: Option<usize>,
    show_all: bool,
    /// Built on first show: the entries, each with the action it names in this trace.
    rows: Option<Vec<Row>>,
}

struct Row {
    entry: CriticalPathEntry,
    act: Option<usize>,
}

fn reported(app: &Ready) -> Option<&BuckCriticalPath> {
    super::command_ui::profile_run(&app.tr.runs, app.run_id.as_deref())?
        .command_profile
        .as_ref()?
        .critical_path
        .as_ref()
}

/// Whether the capture holds a path buck2 reported.
pub fn available(app: &Ready) -> bool {
    reported(app).is_some_and(|path| !path.entries.is_empty())
}

/// The trace action an entry names: its recorded position, confirmed against the span id in the
/// action's own record so a stale or foreign link opens nothing rather than the wrong action.
fn linked_act(app: &Ready, by_seq: &[usize], entry: &CriticalPathEntry) -> Option<usize> {
    entry.acts.iter().find_map(|&seq| {
        let idx = *by_seq.get(seq as usize)?;
        let act = app.tr.acts.get(idx)?;
        let record = super::load_full(&app.tr, act.off, act.len)?;
        let span = record["args"]["span_id"].as_u64()?;
        entry.span_ids.contains(&span).then_some(idx)
    })
}

fn build_rows(app: &Ready) -> Vec<Row> {
    let Some(path) = reported(app) else {
        return Vec::new();
    };
    let mut by_seq = vec![usize::MAX; app.tr.acts.len()];
    for (idx, act) in app.tr.acts.iter().enumerate() {
        if let Some(slot) = by_seq.get_mut(act.seq as usize) {
            *slot = idx;
        }
    }
    path.entries
        .iter()
        .map(|entry| Row {
            entry: entry.clone(),
            act: linked_act(app, &by_seq, entry),
        })
        .collect()
}

/// Waiting gaps and reused keys carry no path weight; they are hidden unless asked for.
fn weightless(entry: &CriticalPathEntry) -> bool {
    entry.kind == "Waiting" || entry.reused == Some(true)
}

fn kind_name(kind: &str) -> &str {
    match kind {
        "ActionExecution" => "action",
        "TestListing" => "test listing",
        "TestExecution" => "test run",
        "GenericEntry" => "command",
        "ComputeCriticalPath" => "path computation",
        "EnsureTransitiveSetProjection" => "set",
        "Waiting" => "waiting",
        "Analysis" => "analysis",
        "Load" => "load",
        "Materialization" => "materialization",
        other => other,
    }
}

fn dur(us: i64) -> String {
    humanize::dur(us as f64)
}

/// Shows the panel; returns an action to navigate to when a step that names one is chosen.
pub fn show(app: &mut Ready, ctx: &egui::Context) -> Option<usize> {
    if !app.panels.reported_cp {
        return None;
    }
    if app.reported_path.rows.is_none() {
        app.reported_path.rows = Some(build_rows(app));
    }
    let backend = reported(app).and_then(|p| p.backend.clone());
    let mut open = true;
    let mut nav = None;
    let state = &mut app.reported_path;
    let names: Vec<Option<String>> = state
        .rows
        .as_ref()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    r.act
                        .map(|i| app.tr.names[app.tr.acts[i].name as usize].clone())
                })
                .collect()
        })
        .unwrap_or_default();
    widgets::popup(
        ctx,
        app.native_popups,
        TITLE,
        TITLE,
        &mut open,
        [980.0, 640.0],
        |ui| {
            let Some(rows) = state.rows.as_ref() else {
                return;
            };
            ui.heading("The critical path buck2 reported");
            ui.label(
                "This build was recorded without --log-action-graph, so Retrace has no \
                 dependency graph. buck2 computed this path during the build. Choose an action \
                 step to open it in the timeline. What-if replays, the cascade and rankings need \
                 the recorded graph.",
            );
            let weight: i64 = rows.iter().filter_map(|r| r.entry.duration_us).sum();
            let linked = rows.iter().filter(|r| r.act.is_some()).count();
            ui.weak(format!(
                "{} steps · path weight {} · {} actions open in the timeline{}",
                humanize::count(rows.len() as u64),
                dur(weight),
                humanize::count(linked as u64),
                backend
                    .as_deref()
                    .map(|b| format!(" · computed by buck2's {b} backend"))
                    .unwrap_or_default(),
            ));
            ui.checkbox(
                &mut state.show_all,
                "Show waiting gaps and reused steps (no path weight)",
            );
            let visible: Vec<usize> = (0..rows.len())
                .filter(|&i| state.show_all || !weightless(&rows[i].entry))
                .collect();
            let mut chosen = None;
            ui.horizontal(|ui| {
                chosen = super::step_buttons(ui, &visible, state.step);
            });
            ui.separator();
            ui.weak(
                "Step · start on the command clock · buck2's weight · could save · kind · what",
            );
            let max_weight = rows
                .iter()
                .filter_map(|r| r.entry.duration_us)
                .max()
                .unwrap_or(1)
                .max(1);
            let row_h = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical()
                .id_salt("reported_path_rows")
                .show_rows(ui, row_h, visible.len(), |ui, range| {
                    for &i in &visible[range] {
                        let row = &rows[i];
                        let e = &row.entry;
                        let weight = e.duration_us.unwrap_or(0);
                        let what = names[i].as_deref().unwrap_or(if e.label.is_empty() {
                            "(unnamed)"
                        } else {
                            &e.label
                        });
                        let text = format!(
                            "{:>4}  {:>9}  {:>9}  {:>9}  {:<16}  {}",
                            i + 1,
                            e.start_offset_us
                                .map(|s| format!("+{}", dur(s)))
                                .unwrap_or_else(|| "?".into()),
                            e.duration_us.map(dur).unwrap_or_else(|| "?".into()),
                            e.potential_improvement_us
                                .filter(|p| *p > 0)
                                .map(dur)
                                .unwrap_or_default(),
                            kind_name(&e.kind),
                            what,
                        );
                        ui.horizontal(|ui| {
                            let bar =
                                ui.available_width().min(80.0) * weight as f32 / max_weight as f32;
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(80.0, row_h * 0.6),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(bar.max(1.0), rect.height()),
                                ),
                                2.0,
                                ui.visuals().selection.bg_fill,
                            );
                            let label = egui::RichText::new(text).monospace();
                            if row.act.is_some() {
                                if ui
                                    .add(egui::Button::selectable(state.step == Some(i), label))
                                    .on_hover_text("Open this action in the timeline")
                                    .clicked()
                                {
                                    chosen = Some(i);
                                }
                            } else {
                                ui.add(egui::Label::new(label.weak())).on_hover_text(
                                    if e.kind == "ActionExecution" {
                                        "This action's record is not in the trace"
                                    } else {
                                        "Not an action: nothing to open"
                                    },
                                );
                            }
                        });
                    }
                });
            if let Some(i) = chosen {
                state.step = Some(i);
                nav = rows[i].act;
            }
        },
    );
    app.panels.reported_cp = open;
    nav
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, reused: Option<bool>) -> CriticalPathEntry {
        CriticalPathEntry {
            kind: kind.to_owned(),
            reused,
            ..Default::default()
        }
    }

    #[test]
    fn waiting_and_reused_steps_are_weightless() {
        assert!(weightless(&entry("Waiting", None)));
        assert!(weightless(&entry(
            "EnsureTransitiveSetProjection",
            Some(true)
        )));
        assert!(!weightless(&entry("ActionExecution", None)));
        assert!(!weightless(&entry("TestExecution", Some(false))));
    }
}
