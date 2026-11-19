/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The command's wall-clock context, deliberately separate from action-DAG
//! replay. Every rectangle has recorded positions; path weights never become
//! synthetic timestamps. Phase rows overlap, and are never stacked to 100%.

use std::collections::BTreeMap;

use super::Ready;
use super::widgets;
use crate::command_profile::CommandProfile;
use crate::command_profile::CriticalPathEntry;
use crate::command_profile::ObservedInterval;
use crate::config;
use crate::humanize;
use crate::types::F_NOT_RECIPE;
use crate::types::RunInfo;
use crate::types::Trace;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    Command,
    Phase(usize),
    Lifecycle(usize),
    Entry(usize),
}

#[derive(Default)]
pub struct State {
    pub selection: Selection,
    pub path_table: bool,
    pub filter: String,
    /// Absolute clock bounds, computed once rather than scanning millions of
    /// actions on every UI frame. Includes only the explicitly selected run.
    pub actions: Option<(i64, i64)>,
    pub unattributed_actions: usize,
}

impl State {
    pub fn new(tr: &Trace, run: Option<&str>) -> Self {
        if run.is_none() && (tr.runs.len() > 1 || tr.run_ids.len() > 1) {
            return Self::default();
        }
        let run = run.or_else(|| profile_run(&tr.runs, None).map(|r| r.run.as_str()));
        let mut actions: Option<(i64, i64)> = None;
        let mut unattributed_actions = 0;
        for a in &tr.acts {
            if a.flags & F_NOT_RECIPE == 0
                && a.run == crate::types::NO_STR
                && !tr.action_in_run(a, run)
            {
                unattributed_actions += 1;
            }
            if a.flags & F_NOT_RECIPE != 0 || !tr.action_in_run(a, run) {
                continue;
            }
            let start = tr.t0_abs.saturating_add(a.ts);
            let end = start.saturating_add(a.dur);
            actions = Some(match actions {
                Some((lo, hi)) => (lo.min(start), hi.max(end)),
                None => (start, end),
            });
        }
        Self {
            actions,
            unattributed_actions,
            ..Self::default()
        }
    }
}

pub(super) fn profile_run<'a>(runs: &'a [RunInfo], run: Option<&str>) -> Option<&'a RunInfo> {
    match run {
        Some(id) => runs.iter().find(|r| r.run == id),
        None if runs.len() == 1 => runs.first(),
        None => None,
    }
}

fn duration(us: i64) -> String {
    humanize::dur(us as f64)
}

fn label(kind: &str) -> String {
    match kind {
        "Load" => "Module loading".to_owned(),
        "LoadPackage" => "Package loading".to_owned(),
        "Listing" => "Package listing".to_owned(),
        "Analysis" | "analysis" => "Analysis".to_owned(),
        "AnonAnalysis" => "Anonymous analysis".to_owned(),
        "DynamicAnalysis" | "DynamicLambda" => "Dynamic analysis".to_owned(),
        "ActionExecution" => "Action".to_owned(),
        "FinalMaterialization" => "Final materialization".to_owned(),
        "Waiting" => "Waiting / gap".to_owned(),
        "GenericEntry" => "Buck operation".to_owned(),
        "CommandCritical" => "Command critical section".to_owned(),
        "FileWatcher" => "File watcher".to_owned(),
        "DiceStateUpdate" => "DICE state update".to_owned(),
        "DiceCriticalSection" => "DICE critical section".to_owned(),
        "DiceBlockConcurrentCommand" => "Concurrent-command wait".to_owned(),
        "DiceSynchronizeSection" => "DICE synchronization".to_owned(),
        "DiceCleanup" => "DICE cleanup".to_owned(),
        "ExclusiveCommandWait" => "Exclusive-command wait".to_owned(),
        "ConnectToInstaller" => "Connect to installer".to_owned(),
        "InstallEventInfo" => "Installer file-ready RPCs".to_owned(),
        "CreateOutputSymlinks" => "Output symlinks".to_owned(),
        "AnalysisStage" => "Analysis stages".to_owned(),
        "AnalysisResolveQueries" => "Analysis queries".to_owned(),
        "ComputeCriticalPath" => "Compute critical path".to_owned(),
        _ => kind.replace('_', " "),
    }
}

fn color(kind: &str) -> egui::Color32 {
    let k = kind.to_ascii_lowercase();
    if k.contains("analysis") {
        config::class_color(1)
    } else if k.contains("load") || k.contains("listing") {
        config::class_color(3)
    } else if k.contains("material") || k.contains("install") {
        config::class_color(2)
    } else if k.contains("wait") || k.contains("unknown") {
        config::ui_color(config::UI_UNMEASURED)
    } else {
        config::class_color(0)
    }
}

/// Clock envelope, not the sum of phase work or critical-path weights.
pub fn extent(profile: &CommandProfile, actions: Option<(i64, i64)>) -> i64 {
    let mut end = profile
        .command_end_offset_us
        .unwrap_or(0)
        .max(profile.command_start_offset_us.unwrap_or(0));
    for span in &profile.lifecycle {
        end = end.max(span.start_offset_us.saturating_add(span.duration_us));
    }
    for phase in &profile.phases {
        for &[_start, stop] in &phase.union_intervals {
            end = end.max(stop);
        }
        for span in &phase.top {
            end = end.max(span.end_offset_us().unwrap_or(span.start_offset_us));
        }
    }
    for sample in &profile.snapshots {
        end = end.max(sample.offset_us);
    }
    if let Some((_, stop)) = actions {
        end = end.max(stop.saturating_sub(profile.origin_us));
    }
    if let Some(cp) = &profile.critical_path {
        for e in &cp.entries {
            if let Some((start, duration)) = entry_span(profile, e) {
                end = end.max(start.saturating_add(duration));
            }
        }
    }
    end.max(1)
}

fn has_positions(profile: &CommandProfile, actions: Option<(i64, i64)>) -> bool {
    profile.command_start_offset_us.is_some()
        || profile.command_end_offset_us.is_some()
        || !profile.lifecycle.is_empty()
        || !profile.snapshots.is_empty()
        || actions.is_some()
        || profile.phases.iter().any(|p| !p.union_intervals.is_empty())
        || profile
            .critical_path
            .as_ref()
            .is_some_and(|cp| cp.entries.iter().any(|e| entry_span(profile, e).is_some()))
}

fn path_cost(profile: &CommandProfile) -> Option<i64> {
    let cp = profile.critical_path.as_ref()?;
    if cp.entries.is_empty() || cp.dropped_entries != 0 {
        return None;
    }
    cp.entries
        .iter()
        .try_fold(0i64, |sum, e| sum.checked_add(e.duration_us?))
}

fn entry_span(profile: &CommandProfile, e: &CriticalPathEntry) -> Option<(i64, i64)> {
    let base = profile.critical_path.as_ref()?.command_start_offset_us?;
    let (offset, dur) = producer_span(e)?;
    let start = base.checked_add(offset)?;
    start.checked_add(dur)?;
    (start >= 0).then_some((start, dur))
}

fn producer_span(e: &CriticalPathEntry) -> Option<(i64, i64)> {
    let start = e.start_offset_us?;
    let dur = if e.reused == Some(true) {
        0
    } else {
        e.total_us?
    };
    if start < 0 || dur < 0 {
        return None;
    }
    start.checked_add(dur)?;
    Some((start, dur))
}

fn clock_x(rect: egui::Rect, us: i64, extent: i64) -> f32 {
    rect.left() + rect.width() * (us as f64 / extent.max(1) as f64).clamp(0.0, 1.0) as f32
}

/// Exact geometry; only the hit target is widened for tiny spans. Filling
/// a minimum-width bar would exaggerate short phases on a long build.
fn span_rect(rect: egui::Rect, start: i64, end: i64, extent: i64) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(clock_x(rect, start, extent), rect.top()),
        egui::pos2(clock_x(rect, end.max(start), extent), rect.bottom()),
    )
}

fn row(
    ui: &mut egui::Ui,
    name: &str,
    selected: bool,
    paint: impl FnOnce(&mut egui::Ui, egui::Rect),
) -> bool {
    let height = ui.spacing().interact_size.y;
    let mut clicked = false;
    ui.horizontal(|ui| {
        clicked = ui
            .add_sized([170.0, height], egui::Button::selectable(selected, name))
            .clicked();
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().faint_bg_color);
        paint(ui, rect.shrink2(egui::vec2(0.0, 3.0)));
    });
    clicked
}

fn chart(ui: &mut egui::Ui, profile: &CommandProfile, state: &mut State) {
    let end = extent(profile, state.actions);
    if has_positions(profile, state.actions) {
        ui.horizontal(|ui| {
            ui.add_space(170.0 + ui.spacing().item_spacing.x);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(
                    ui.available_width(),
                    ui.text_style_height(&egui::TextStyle::Small),
                ),
                egui::Sense::hover(),
            );
            for tick in 0..=4 {
                let us = (i128::from(end) * i128::from(tick) / 4) as i64;
                ui.painter().text(
                    egui::pos2(clock_x(rect, us, end), rect.center().y),
                    if tick == 0 {
                        egui::Align2::LEFT_CENTER
                    } else if tick == 4 {
                        egui::Align2::RIGHT_CENTER
                    } else {
                        egui::Align2::CENTER_CENTER
                    },
                    duration(us),
                    egui::TextStyle::Small.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
            }
        });
        if row(
            ui,
            "Recorded window",
            state.selection == Selection::Command,
            |ui, rect| {
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                    egui::StrokeKind::Inside,
                );
                if let Some(start) = profile.command_start_offset_us {
                    let x = clock_x(rect, start, end);
                    ui.painter().line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(2.0, config::ui_color(config::UI_FOCUS)),
                    );
                }
                if let Some(stop) = profile.command_end_offset_us {
                    let x = clock_x(rect, stop, end);
                    ui.painter().line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(2.0, config::ui_color(config::UI_FOCUS)),
                    );
                }
            },
        ) {
            state.selection = Selection::Command;
        }

        if let Some(cp) = &profile.critical_path
            && cp.command_start_offset_us.is_some()
        {
            let mut picked = None;
            row(
                ui,
                if cp.approximate_alignment {
                    "Buck path (approx. aligned)"
                } else {
                    "Buck reported path"
                },
                matches!(state.selection, Selection::Entry(_)),
                |ui, rect| {
                    for (i, entry) in cp.entries.iter().enumerate() {
                        let Some((start, dur)) = entry_span(profile, entry) else {
                            continue;
                        };
                        let r = span_rect(rect, start, start.saturating_add(dur), end);
                        if dur == 0 {
                            ui.painter().line_segment(
                                [r.left_top(), r.left_bottom()],
                                egui::Stroke::new(1.0, color(&entry.kind)),
                            );
                        } else {
                            ui.painter().rect_filled(r, 0.0, color(&entry.kind));
                        }
                        if state.selection == Selection::Entry(i) {
                            ui.painter().rect_stroke(
                                r.expand(1.0),
                                0.0,
                                egui::Stroke::new(2.0, config::ui_color(config::UI_FOCUS)),
                                egui::StrokeKind::Outside,
                            );
                        }
                        let hit = r.expand2(egui::vec2((3.0 - r.width()).max(0.0) / 2.0, 0.0));
                        let response =
                            ui.interact(hit, ui.id().with(("cp_entry", i)), egui::Sense::click());
                        if response
                            .clone()
                            .on_hover_ui(|ui| {
                                ui.label(format!(
                                    "{}\n{}\nEvent clock +{}; {}\nPath cost: {}",
                                    label(&entry.kind),
                                    entry.label,
                                    duration(start),
                                    if entry.reused == Some(true) {
                                        "reused-result marker, not a current execution interval"
                                            .to_owned()
                                    } else {
                                        format!("recorded span {}", duration(dur))
                                    },
                                    entry
                                        .duration_us
                                        .map(duration)
                                        .unwrap_or_else(|| "not recorded".into())
                                ));
                            })
                            .clicked()
                        {
                            picked = Some(i);
                        }
                    }
                },
            );
            if let Some(i) = picked {
                state.selection = Selection::Entry(i);
            }
        }

        if let Some((start, stop)) = state.actions {
            row(ui, "Action window", false, |ui, rect| {
                let r = span_rect(
                    rect,
                    start.saturating_sub(profile.origin_us),
                    stop.saturating_sub(profile.origin_us),
                    end,
                );
                ui.painter()
                    .rect_filled(r, 2.0, config::class_color(0).gamma_multiply(0.5));
                ui.interact(r, ui.id().with("action_window"), egui::Sense::hover())
                .on_hover_text("First action start to last action end. This is an envelope, not continuous execution, CPU use, or the critical path.");
            });
        }
        // Outer command wrappers are already represented by the recorded-window
        // row. Retain their raw evidence without duplicating almost-full-width
        // bars; concrete startup waits and installer activity remain visible.
        let mut lifecycle = BTreeMap::<&str, Vec<(usize, &ObservedInterval)>>::new();
        for (i, span) in profile.lifecycle.iter().enumerate() {
            if !matches!(
                span.kind.as_str(),
                "Command" | "CommandCritical" | "DiceCriticalSection"
            ) {
                lifecycle.entry(&span.kind).or_default().push((i, span));
            }
        }
        enum Lane<'a> {
            Lifecycle(&'a str, Vec<(usize, &'a ObservedInterval)>),
            Phase(usize, &'a crate::command_profile::PhaseSummary),
        }
        let mut lanes: Vec<_> = lifecycle
            .into_iter()
            .map(|(kind, spans)| Lane::Lifecycle(kind, spans))
            .chain(
                profile
                    .phases
                    .iter()
                    .enumerate()
                    .map(|(i, p)| Lane::Phase(i, p)),
            )
            .collect();
        lanes.sort_by_key(|lane| match lane {
            Lane::Lifecycle(_, spans) => spans
                .iter()
                .map(|(_, span)| span.start_offset_us)
                .min()
                .unwrap_or(i64::MAX),
            Lane::Phase(_, phase) => phase
                .union_intervals
                .first()
                .map(|r| r[0])
                .unwrap_or(i64::MAX),
        });
        for lane in lanes {
            match lane {
                Lane::Lifecycle(kind, spans) => {
                    let mut picked = None;
                    let selected = spans
                        .iter()
                        .any(|(i, _)| state.selection == Selection::Lifecycle(*i));
                    if row(ui, &label(kind), selected, |ui, rect| {
                        for &(i, span) in &spans {
                            let r = span_rect(
                                rect,
                                span.start_offset_us,
                                span.end_offset_us().unwrap_or(span.start_offset_us),
                                end,
                            );
                            ui.painter().rect_filled(r, 1.0, color(kind));
                            if span.cancelled {
                                ui.painter().line_segment(
                                    [r.left_top(), r.right_top()],
                                    egui::Stroke::new(1.0, config::ui_color(config::UI_UNMEASURED)),
                                );
                            }
                            // Thousands of overlapping canceled intervals must not
                            // produce thousands of full-width hatch patterns per frame.
                            // Every interval retains its canceled tooltip and detail.
                            if span.cancelled && state.selection == Selection::Lifecycle(i) {
                                widgets::hatch(ui.painter(), r);
                            }
                            let hit = r.expand2(egui::vec2((3.0 - r.width()).max(0.0) / 2.0, 0.0));
                            if ui
                                .interact(hit, ui.id().with(("lifecycle", i)), egui::Sense::click())
                                .on_hover_ui(|ui| {
                                    ui.label(format!(
                                        "{} · +{} · {}{}",
                                        label(kind),
                                        duration(span.start_offset_us),
                                        duration(span.duration_us),
                                        if span.cancelled { " · canceled" } else { "" }
                                    ));
                                })
                                .clicked()
                            {
                                picked = Some(i);
                            }
                        }
                    }) {
                        picked = Some(spans[0].0);
                    }
                    if let Some(i) = picked {
                        state.selection = Selection::Lifecycle(i);
                    }
                }
                Lane::Phase(i, phase) => {
                    let mut bar_clicked = false;
                    let clicked = row(
                        ui,
                        &if phase.active_us().is_some() {
                            label(&phase.kind)
                        } else {
                            format!("{} (partial)", label(&phase.kind))
                        },
                        state.selection == Selection::Phase(i),
                        |ui, rect| {
                            let mut previous: Option<egui::Rect> = None;
                            for &[start, stop] in &phase.union_intervals {
                                let r = span_rect(rect, start, stop, end);
                                if r.width() <= 0.0 {
                                    continue;
                                }
                                match &mut previous {
                                    Some(p) if r.left().floor() <= p.right().floor() => {
                                        p.max.x = p.max.x.max(r.max.x)
                                    }
                                    Some(p) => {
                                        ui.painter().rect_filled(*p, 1.0, color(&phase.kind));
                                        *p = r;
                                    }
                                    None => previous = Some(r),
                                }
                            }
                            if let Some(p) = previous {
                                ui.painter().rect_filled(p, 1.0, color(&phase.kind));
                            }
                            bar_clicked = ui.interact(rect, ui.id().with(("phase_lane", i)), egui::Sense::click())
                .on_hover_text(format!("{} recorded starts; {} completed; {} still open. Filled positions are the union of observed intervals; simultaneous work is counted once here. Click for details.", humanize::count(phase.count), humanize::count(phase.completed), humanize::count(phase.open_at_eof)))
                .clicked();
                        },
                    );
                    if clicked || bar_clicked {
                        state.selection = Selection::Phase(i);
                    }
                }
            }
        }
        ui.weak("One wall-clock axis. Rows overlap; do not add them. Blank time is not evidence of idle CPU.");
        if profile.lifecycle.iter().any(|s| s.start_offset_us < 0)
            || profile
                .phases
                .iter()
                .any(|p| p.union_intervals.first().is_some_and(|r| r[0] < 0))
        {
            ui.label("Some observations predate the recorded origin. They are cropped at zero in this chart; their original offsets remain in the evidence tables.");
        }
    } else {
        ui.weak("No positioned command observations were recorded. Duration counters are available below; they do not establish a timeline.");
    }
    if let Some(cp) = &profile.critical_path
        && cp.command_start_offset_us.is_none()
    {
        ui.separator();
        ui.strong("Reported critical path · Buck command clock");
        ui.weak("This log does not synchronize the path's clock origin with the event timeline above. Positions below use Buck's own command-relative offsets.");
        let path_end = cp
            .entries
            .iter()
            .filter_map(|e| producer_span(e).and_then(|(s, d)| s.checked_add(d)))
            .max();
        let Some(path_end) = path_end else {
            ui.weak("No positioned path entries were recorded; browse the path table for model weights.");
            return;
        };
        let path_end = path_end.max(1);
        let mut picked = None;
        row(
            ui,
            &format!("0 → {}", duration(path_end)),
            false,
            |ui, rect| {
                for (i, e) in cp.entries.iter().enumerate() {
                    let Some((start, dur)) = producer_span(e) else {
                        continue;
                    };
                    let r = span_rect(rect, start, start.saturating_add(dur), path_end);
                    if dur > 0 {
                        ui.painter().rect_filled(r, 0.0, color(&e.kind));
                    } else {
                        ui.painter().line_segment(
                            [r.left_top(), r.left_bottom()],
                            egui::Stroke::new(1.0, color(&e.kind)),
                        );
                    }
                    if state.selection == Selection::Entry(i) {
                        ui.painter().rect_stroke(
                            r.expand(1.0),
                            0.0,
                            egui::Stroke::new(2.0, config::ui_color(config::UI_FOCUS)),
                            egui::StrokeKind::Outside,
                        );
                    }
                    let hit = r.expand2(egui::vec2((3.0 - r.width()).max(0.0) / 2.0, 0.0));
                    if ui
                        .interact(hit, ui.id().with(("producer_cp", i)), egui::Sense::click())
                        .on_hover_ui(|ui| {
                            ui.label(format!(
                                "{} · {}\nBuck clock +{}; path cost {}",
                                label(&e.kind),
                                e.label,
                                duration(start),
                                e.duration_us
                                    .map(duration)
                                    .unwrap_or_else(|| "not recorded".into())
                            ));
                        })
                        .clicked()
                    {
                        picked = Some(i);
                    }
                }
            },
        );
        if let Some(i) = picked {
            state.selection = Selection::Entry(i);
        }
    }
}

fn field(ui: &mut egui::Ui, name: &str, value: impl Into<egui::WidgetText>) {
    ui.label(name);
    ui.label(value);
    ui.end_row();
}

fn observed_table(ui: &mut egui::Ui, spans: &[ObservedInterval]) {
    let height = ui.spacing().interact_size.y;
    ui.horizontal(|ui| {
        for (name, width) in [("Start", 100.0), ("Elapsed", 100.0), ("Activity", 180.0)] {
            ui.add_sized(
                [width, height],
                egui::Label::new(egui::RichText::new(name).strong()),
            );
        }
        ui.strong("Recorded detail");
    });
    egui::ScrollArea::vertical()
        .id_salt("observed_span_scroll")
        .max_height(240.0)
        .show_rows(ui, height, spans.len(), |ui, range| {
            for i in range {
                let span = &spans[i];
                ui.horizontal(|ui| {
                    for (text, width) in [
                        (format!("+{}", duration(span.start_offset_us)), 100.0),
                        (duration(span.duration_us), 100.0),
                        (
                            if span.cancelled {
                                format!("{} · canceled", label(&span.kind))
                            } else {
                                label(&span.kind)
                            },
                            180.0,
                        ),
                    ] {
                        ui.add_sized([width, height], egui::Label::new(text).truncate());
                    }
                    let text = span.label.as_deref().unwrap_or("—");
                    ui.add(egui::Label::new(text).truncate())
                        .on_hover_text(text);
                });
            }
        });
}

fn inspector(ui: &mut egui::Ui, profile: &CommandProfile, state: &State) {
    match state.selection {
        Selection::Lifecycle(i) => {
            let Some(span) = profile.lifecycle.get(i) else {
                return;
            };
            ui.heading(label(&span.kind));
            egui::Grid::new("lifecycle_facts")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    field(
                        ui,
                        "Start (from log origin)",
                        format!("+{}", duration(span.start_offset_us)),
                    );
                    field(ui, "Observed elapsed", duration(span.duration_us));
                    field(
                        ui,
                        "Completion",
                        if span.cancelled {
                            "Canceled"
                        } else {
                            "Span ended; success is not established by timing alone"
                        },
                    );
                    if let Some(text) = &span.label {
                        field(ui, "Recorded detail", text);
                    }
                });
            if span.kind == "InstallEventInfo" {
                ui.weak("This is an installer file-ready RPC, not total device installation time. Artifact production and installer activity may overlap.");
            }
        }
        Selection::Phase(i) => {
            let Some(phase) = profile.phases.get(i) else {
                return;
            };
            ui.heading(label(&phase.kind));
            egui::Grid::new("phase_facts")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    field(ui, "Recorded spans", humanize::count(phase.count));
                    field(
                        ui,
                        "Completed / canceled / open",
                        format!(
                            "{} / {} / {}",
                            humanize::count(phase.completed),
                            humanize::count(phase.cancelled),
                            humanize::count(phase.open_at_eof)
                        ),
                    );
                    field(
                        ui,
                        "Time with observed activity",
                        match phase.active_us() {
                            Some(us) => duration(us),
                            None => phase
                                .retained_active_us()
                                .map(|us| {
                                    format!("at least {} (partial interval coverage)", duration(us))
                                })
                                .unwrap_or_else(|| {
                                    "unavailable (invalid retained intervals)".into()
                                }),
                        },
                    );
                    field(
                        ui,
                        "Total span time",
                        if phase.total_duration_us == i64::MAX {
                            "unavailable (duration sum overflow)".into()
                        } else {
                            format!(
                                "{} (spans overlap and include waiting: neither work nor wall time)",
                                duration(phase.total_duration_us)
                            )
                        },
                    );
                    field(
                        ui,
                        "Rejected / clipped intervals / unmatched ends",
                        format!(
                            "{} / {} / {}",
                            phase.rejected_intervals, phase.clipped_intervals, phase.unmatched_ends
                        ),
                    );
                });
            ui.weak("Activity coverage is not causal critical-path membership. Shortening an analysis helps the build only if it advances work that blocks completion.");
            ui.separator();
            ui.strong("Longest recorded examples");
            observed_table(ui, &phase.top);
            if phase.dropped_examples != 0 {
                ui.weak(format!(
                    "Showing a bounded set; {} additional examples are not retained.",
                    humanize::count(phase.dropped_examples)
                ));
            }
        }
        Selection::Entry(i) => {
            let Some(entry) = profile
                .critical_path
                .as_ref()
                .and_then(|cp| cp.entries.get(i))
            else {
                return;
            };
            ui.heading(label(&entry.kind));
            ui.add(egui::Label::new(&entry.label).wrap());
            egui::Grid::new("path_entry_facts")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    field(
                        ui,
                        "Position",
                        entry
                            .start_offset_us
                            .map(|s| format!("+{} on Buck's command clock", duration(s)))
                            .unwrap_or_else(|| "not recorded; not positioned on the chart".into()),
                    );
                    for (name, value) in [
                        ("Path cost", entry.duration_us),
                        ("Total span", entry.total_us),
                        ("User-controlled portion", entry.user_us),
                        ("Queue portion", entry.queue_us),
                        ("Non-critical portion", entry.noncritical_us),
                        (
                            "Buck-reported improvement bound",
                            entry.potential_improvement_us,
                        ),
                    ] {
                        if let Some(value) = value {
                            field(ui, name, duration(value));
                        }
                    }
                    if entry.reused == Some(true) {
                        field(
                            ui,
                            "DICE result",
                            "Reused after dependency validation; not re-executed",
                        );
                    }
                });
            ui.weak("These are Buck's reported path weights and bounds, not Retrace action-replay predictions. Span duration and path cost can differ.");
            if entry.kind.to_ascii_lowercase().contains("wait") {
                ui.weak("A gap or waiting entry does not identify a resource bottleneck unless its recorded category does. It is not measured daemon CPU.");
            }
        }
        Selection::Command => {
            ui.heading("Where the command spent time");
            ui.label("Select a phase or a path segment above. The rows preserve overlap, including analysis during action execution and installation while artifacts become ready.");
            egui::Grid::new("command_facts")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    if let Some(start) = profile.command_start_offset_us {
                        field(
                            ui,
                            "Command start marker",
                            format!("+{} from the log origin", duration(start)),
                        );
                    }
                    if let Some(end) = profile.command_end_offset_us {
                        field(
                            ui,
                            "Command end marker",
                            format!("+{} from the log origin", duration(end)),
                        );
                    }
                    if let Some(elapsed) = profile
                        .command_start_offset_us
                        .zip(profile.command_end_offset_us)
                        .and_then(|(a, b)| b.checked_sub(a))
                    {
                        field(ui, "Between command markers", duration(elapsed));
                    }
                    if let Some((start, stop)) = state.actions {
                        field(
                            ui,
                            "First action (from log origin)",
                            duration(start.saturating_sub(profile.origin_us).max(0)),
                        );
                        field(
                            ui,
                            "First-to-last action window",
                            duration(stop.saturating_sub(start)),
                        );
                    }
                    if let Some(us) = profile.install_duration_us {
                        field(
                            ui,
                            "Installer-reported duration",
                            format!("{} (duration only; not placed on this clock)", duration(us)),
                        );
                    }
                });
            if !profile.lifecycle.is_empty() {
                egui::CollapsingHeader::new("Recorded lifecycle events")
                    .show(ui, |ui| observed_table(ui, &profile.lifecycle));
            }
        }
    }
}

fn path_table(ui: &mut egui::Ui, profile: &CommandProfile, state: &mut State) {
    let Some(cp) = &profile.critical_path else {
        ui.label("Buck did not report a critical path in this capture. Observed phases remain available; no dependency path is inferred from their timestamps.");
        return;
    };
    let visible = |filter: &str| -> Vec<_> {
        let query = filter.to_lowercase();
        cp.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                query.is_empty()
                    || super::contains_folded(&e.kind, &query)
                    || super::contains_folded(&e.label, &query)
            })
            .collect()
    };
    let mut rows = visible(&state.filter);
    ui.horizontal(|ui| {
        let shown: Vec<usize> = rows.iter().map(|(i, _)| *i).collect();
        let selected = match state.selection {
            Selection::Entry(i) => Some(i),
            _ => None,
        };
        if let Some(i) = super::step_buttons(ui, &shown, selected) {
            state.selection = Selection::Entry(i);
        }
        ui.label(format!(
            "{} reported entries",
            humanize::count(cp.entries.len() as u64)
        ));
    });
    if ui
        .add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text("Filter path by kind, package or target"),
        )
        .changed()
    {
        rows = visible(&state.filter);
    }
    ui.weak("Producer path order · start is on the command clock · cost is the weight Buck used, not a new wall-clock interval");
    let row_h = ui.spacing().interact_size.y;
    egui::ScrollArea::vertical()
        .id_salt("command_path_rows")
        .max_height(220.0)
        .show_rows(ui, row_h, rows.len(), |ui, range| {
            for j in range {
                let (i, entry) = rows[j];
                let start = entry
                    .start_offset_us
                    .map(|s| format!("+{}", duration(s)))
                    .unwrap_or_else(|| "unpositioned".into());
                let cost = entry
                    .duration_us
                    .map(duration)
                    .unwrap_or_else(|| "unknown".into());
                let text = format!(
                    "{}   {}   {}   {}",
                    start,
                    cost,
                    label(&entry.kind),
                    entry.label
                );
                if ui
                    .add(egui::Button::selectable(
                        state.selection == Selection::Entry(i),
                        humanize::mid_ellipsis(&text, 125),
                    ))
                    .on_hover_text(text)
                    .clicked()
                {
                    state.selection = Selection::Entry(i);
                }
            }
        });
}

fn snapshot_context(ui: &mut egui::Ui, profile: &CommandProfile, state: &State) {
    let window = match state.selection {
        Selection::Lifecycle(i) => profile
            .lifecycle
            .get(i)
            .and_then(|span| Some((span.start_offset_us, span.end_offset_us()?))),
        Selection::Command => Some((0, extent(profile, state.actions))),
        Selection::Phase(i) => profile.phases.get(i).and_then(|phase| {
            Some((
                phase.union_intervals.first()?[0],
                phase.union_intervals.last()?[1],
            ))
        }),
        Selection::Entry(i) => profile
            .critical_path
            .as_ref()
            .and_then(|cp| cp.entries.get(i))
            .and_then(|entry| entry_span(profile, entry))
            .map(|(a, d)| (a, a.saturating_add(d))),
    };
    let Some((start, end)) = window else {
        return;
    };
    if profile.snapshots.is_empty() {
        return;
    }
    let Ok(summary) = crate::command_profile::summarize_snapshots(
        &profile.snapshots,
        start,
        end,
        profile.dropped_snapshots,
    ) else {
        return;
    };
    ui.separator();
    ui.strong(format!(
        "Daemon context · +{} to +{}{}",
        duration(start),
        duration(end),
        if matches!(state.selection, Selection::Phase(_)) {
            " (phase envelope, including gaps)"
        } else {
            ""
        }
    ));
    let mut readings = Vec::new();
    if let Some(rate) = summary.rate_per_second(&summary.cpu) {
        readings.push(format!("CPU {:.1} cores across sampled span", rate / 1e6));
    }
    let samples = profile
        .snapshots
        .iter()
        .filter(|s| s.offset_us >= start && s.offset_us <= end);
    if let Some(peak) = samples.clone().filter_map(|s| s.rss_bytes).max() {
        readings.push(format!(
            "sampled peak RSS {}",
            crate::analyses::fmt_bytes_f(peak as f64)
        ));
    }
    let selected_queue = crate::command_profile::materializer_queue_range(samples.clone());
    if let Some(peak) = samples.filter_map(|s| s.heap_bytes).max() {
        readings.push(format!(
            "sampled peak allocator active {}",
            crate::analyses::fmt_bytes_f(peak as f64)
        ));
    }
    if let Some(bytes) = summary.download.delta {
        readings.push(format!(
            "download counter +{}",
            crate::analyses::fmt_bytes_f(bytes as f64)
        ));
    }
    if readings.is_empty() {
        readings.push("insufficient samples for counter deltas".into());
    }
    ui.label(readings.join(" · "));
    if let Some(all) = crate::command_profile::materializer_queue_range(&profile.snapshots) {
        let selected = selected_queue
            .map(|range| {
                format!(
                    "{}–{} reported queued commands in this window ({} of {} samples)",
                    humanize::count(range.minimum),
                    humanize::count(range.maximum),
                    range.sample_count,
                    summary.sample_count,
                )
            })
            .unwrap_or_else(|| "no recorded queue values in this window".into());
        ui.label(format!(
            "Materializer queue: {selected} · command-wide sampled peak {}",
            humanize::count(all.maximum),
        ))
        .on_hover_text(
            "Daemon-wide pending materializer commands across both priority queues, not actions ahead of the selected action. Retained samples can miss peaks; missing or dropped samples are not zero. This gauge does not establish per-action wait or service rate. The protobuf scalar has no presence bit: zero may mean empty or a producer default.",
        );
        if all.minimum == 0 {
            ui.weak("Queue zero may mean empty or not reported by the producer; an empty queue is not verified.");
        }
    } else {
        ui.weak("Materializer queue: not recorded in this trace.");
    }
    ui.weak(format!("Shared daemon activity, not attribution to this phase. {} samples · endpoint coverage {} · largest gap {}.",
        summary.sample_count,
        summary.coverage_fraction().map(|c| format!("{:.1}%", c * 100.0)).unwrap_or_else(|| "unknown".into()),
        summary.max_gap_us.map(duration).unwrap_or_else(|| "unknown".into())))
        .on_hover_ui(|ui| {
            for warning in &summary.warnings { ui.label(warning); }
            for (name, counter) in [("CPU", &summary.cpu), ("Download", &summary.download), ("Upload", &summary.upload)] {
                if let Some(reason) = &counter.unavailable_reason { ui.label(format!("{name}: {reason}")); }
            }
        });
    if summary.dropped_samples != 0
        || summary.cpu.reset_observed
        || summary.download.reset_observed
        || summary.upload.reset_observed
    {
        ui.label("Some counter deltas are unavailable: samples were dropped or a counter reset. Hover the coverage line for details.");
    }
}

/// Public for the rendering/interaction harness. Does not perform I/O or
/// scan action rows; opening and interacting are independent of graph size.
pub fn contents(
    ui: &mut egui::Ui,
    profile: &CommandProfile,
    state: &mut State,
    action_cp: Option<i64>,
) -> bool {
    let mut open_action_model = false;
    ui.horizontal_wrapped(|ui| {
        if let Some(kind) = &profile.command_kind {
            ui.strong(kind);
        }
        if let Some(outcome) = &profile.result_outcome {
            ui.label(format!("· {outcome}"));
        }
        if profile.command_cancelled {
            ui.strong("· canceled");
        }
    });
    if let Some(error) = &profile.result_error {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.strong("Recorded command diagnostic");
                if ui.button("Copy diagnostic").clicked() {
                    ui.ctx().copy_text(error.clone());
                }
            });
            ui.label(error);
        });
    }
    ui.horizontal_wrapped(|ui| {
        let elapsed = extent(profile, state.actions);
        ui.heading(if !has_positions(profile, state.actions) {
            "Recorded window unavailable".into()
        } else if elapsed < 1_000_000 {
            format!("Recorded window {}", duration(elapsed))
        } else {
            format!("Recorded window {} ({}s)", humanize::clock(elapsed as f64), humanize::num1(elapsed as f64 / 1e6))
        });
        if let Some(cost) = path_cost(profile) {
            ui.separator();
            ui.label(format!("Buck path cost {}", duration(cost))).on_hover_text("Sum of the reported CriticalPathEntry2 duration weights. This is not necessarily client wall time or the action-only graph model.");
        }
        if let Some(cost) = action_cp {
            ui.separator();
            open_action_model = ui.button(format!("Action model {} →", duration(cost))).on_hover_text("Open the action-dependency model for what-if simulations. Startup, analysis and installation are outside that model.").clicked();
        }
    });
    if profile.command_end_offset_us.is_none() {
        ui.label("Command end was not recorded. The chart ends at the latest retained observation, not a claimed command completion.");
    }
    if profile
        .critical_path
        .as_ref()
        .is_some_and(|cp| cp.approximate_alignment)
    {
        ui.weak("Buck's path is approximately aligned to the daemon Command start, not an exact cross-clock join.");
    }
    chart(ui, profile, state);
    ui.separator();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.path_table, false, "Selected activity");
        ui.add_enabled_ui(profile.critical_path.is_some(), |ui| {
            ui.selectable_value(&mut state.path_table, true, "Browse reported path");
        })
        .response
        .on_disabled_hover_text("Buck did not record a critical-path model for this command.");
        if ui.button("Copy command evidence").clicked() {
            if let Ok(json) = serde_json::to_string_pretty(profile) {
                ui.ctx().copy_text(json);
            }
        }
    });
    if state.path_table {
        path_table(ui, profile, state);
    }
    ui.separator();
    inspector(ui, profile, state);
    snapshot_context(ui, profile, state);
    if profile.has_incomplete_evidence() {
        ui.separator();
        egui::CollapsingHeader::new("Evidence limits")
            .default_open(false)
            .show(ui, |ui| {
                for warning in &profile.warnings {
                    ui.label(warning);
                }
                if profile.lifecycle_open_at_eof != 0 {
                    ui.label(format!(
                        "{} lifecycle spans were still open at end of log.",
                        profile.lifecycle_open_at_eof
                    ));
                }
                if profile.dropped_snapshots != 0 {
                    ui.label(format!(
                        "{} snapshots exceed retention limits.",
                        profile.dropped_snapshots
                    ));
                }
                if let Some(cp) = &profile.critical_path
                    && cp.dropped_entries != 0
                {
                    ui.label(format!(
                        "{} Buck path entries were not retained; path totals are partial.",
                        cp.dropped_entries
                    ));
                }
                if profile.dropped_lifecycle != 0 {
                    ui.label(format!(
                        "{} lifecycle intervals exceed the retention limit.",
                        profile.dropped_lifecycle
                    ));
                }
                if profile.rejected_records != 0 {
                    ui.label(format!(
                        "{} records could not be retained or were rejected.",
                        profile.rejected_records
                    ));
                }
                let incomplete_phases = profile
                    .phases
                    .iter()
                    .filter(|phase| phase.has_incomplete_evidence())
                    .count();
                if incomplete_phases != 0 {
                    ui.label(format!(
                        "{incomplete_phases} phase summaries have incomplete observations; inspect their coverage and counts in the phase table."
                    ));
                }
            });
    }
    if !profile.notes.is_empty() {
        egui::CollapsingHeader::new("Interpretation notes").show(ui, |ui| {
            for note in &profile.notes {
                ui.weak(note);
            }
        });
    }
    open_action_model
}

pub fn show_inline(app: &mut Ready, root: &mut egui::Ui) {
    let run = profile_run(&app.tr.runs, app.run_id.as_deref());
    egui::CentralPanel::default().show(root, |ui| {
        egui::ScrollArea::vertical().id_salt("command-profile-main").show(ui, |ui| {
            ui.heading("Command profile");
            if let Some(run) = run {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Command ID: {}", run.run));
                    if ui.button("Copy command ID").clicked() {
                        ui.ctx().copy_text(run.run.clone());
                    }
                    if command_id(&run.run)
                        && let Some(link) = config::build_link(&run.run)
                    {
                        ui.hyperlink_to("Open build page", link);
                    }
                });
                if let Some(argv) = &run.argv {
                    ui.label(argv);
                }
                if run.command_profile.as_ref().is_none_or(|p| p.result_outcome.is_none()) {
                    ui.label(format!("Outcome: {}", run.outcome.as_str()));
                }
            }
            if app.command_view.unattributed_actions > 0 {
                ui.label(format!("No completed actions are attributed to this command; {} action rows lack a run identity in this multi-run trace. Action analysis is unavailable for this selection.", app.command_view.unattributed_actions));
            } else {
                ui.label("No completed actions were recorded for this command. Action graph, action critical path and simulation are not applicable to this profile.");
            }
            ui.weak("This does not by itself indicate a truncated capture. Query-result output is not reconstructed from timing records.");
            ui.separator();
            match run.and_then(|r| r.command_profile.as_ref()) {
                Some(profile) => { contents(ui, profile, &mut app.command_view, None); }
                None => {
                    ui.label("No validated command profile is available. Reopen the original event log to recover recorded lifecycle and analysis evidence.");
                }
            }
        });
    });
}

// Keep a source-supplied run label out of URL syntax. Older trace fixtures
// permit arbitrary run labels; only canonical UUIDs link to Buck invocations.
fn command_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn show(app: &mut Ready, ctx: &egui::Context) {
    let mut open = app.panels.command;
    if !open {
        return;
    }
    let run = profile_run(&app.tr.runs, app.run_id.as_deref());
    let profile = run.and_then(|r| r.command_profile.as_ref());
    let cp = match &app.gstate {
        super::GState::Ready { dag: Some(b), .. } => Some(b.dag.cp),
        _ => None,
    };
    let mut open_action_model = false;
    widgets::popup(
        ctx,
        app.native_popups,
        "Build timeline",
        "Build timeline — whole command",
        &mut open,
        [1100.0, 760.0],
        |ui| match profile {
            Some(profile) => open_action_model = contents(ui, profile, &mut app.command_view, cp),
            None => {
                if app.run_id.is_none() && app.tr.runs.len() > 1 {
                    ui.heading("Choose one build to inspect its command timeline");
                    ui.label("This trace contains multiple runs. Reopen it with --run=ID; timings from different commands are not combined into a single command profile.");
                    return;
                }
                ui.heading("Whole-command evidence is not available in this trace");
                ui.label("Reopen the original Buck event log with this version of Retrace to retain startup, analysis, lifecycle events and Buck's reported path. An older converted JSON trace cannot recover events it omitted.");
                if let Some((start, stop)) = app.command_view.actions {
                    ui.label(format!(
                        "Recorded action window: {}. This is not the whole command.",
                        duration(stop.saturating_sub(start))
                    ));
                }
            }
        },
    );
    app.panels.command = open;
    if open_action_model {
        app.panels.cp = true;
        widgets::popup_raise(ctx, "Critical path (dependency graph)");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn absent_positions_do_not_claim_a_one_microsecond_window() {
        let profile = CommandProfile {
            client_wall_us: Some(5_000_000),
            ..Default::default()
        };
        assert!(!super::has_positions(&profile, None));
        assert_eq!(
            super::producer_span(&CriticalPathEntry {
                start_offset_us: Some(12),
                ..Default::default()
            }),
            None
        );
        assert_eq!(
            super::producer_span(&CriticalPathEntry {
                start_offset_us: Some(12),
                reused: Some(true),
                ..Default::default()
            }),
            Some((12, 0))
        );
    }

    use super::*;

    #[test]
    fn geometry_uses_clock_not_summed_path_cost() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(200.0, 20.0));
        let a = span_rect(rect, 20, 80, 100);
        let b = span_rect(rect, 40, 90, 100);
        assert!(a.intersects(b), "concurrent work must remain overlapping");
        assert_eq!(a.left(), 50.0);
        assert_eq!(b.right(), 190.0);
    }

    #[test]
    fn tiny_spans_do_not_get_artificial_duration() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 20.0));
        assert!(span_rect(rect, 0, 1, 1_000_000).width() < 0.01);
    }

    #[test]
    fn reused_entry_is_a_marker_not_historical_execution() {
        let entry = CriticalPathEntry {
            start_offset_us: Some(500),
            total_us: Some(999_999),
            duration_us: Some(0),
            reused: Some(true),
            ..Default::default()
        };
        assert_eq!(producer_span(&entry), Some((500, 0)));
        let profile = CommandProfile {
            critical_path: Some(crate::command_profile::BuckCriticalPath {
                command_start_offset_us: Some(100),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(entry_span(&profile, &entry), Some((600, 0)));
        let ordinary = CriticalPathEntry {
            reused: None,
            ..entry
        };
        assert_eq!(entry_span(&profile, &ordinary), Some((600, 999_999)));
    }

    #[test]
    fn maximum_clock_ticks_remain_representable() {
        for tick in 0..=4 {
            let us = (i128::from(i64::MAX) * i128::from(tick) / 4) as i64;
            assert!((0..=i64::MAX).contains(&us));
        }
    }

    #[test]
    fn incomplete_window_includes_markers_samples_and_retained_examples() {
        use crate::command_profile::PhaseSummary;
        use crate::command_profile::SnapshotSample;
        let mut profile = CommandProfile {
            command_start_offset_us: Some(100),
            ..Default::default()
        };
        assert_eq!(extent(&profile, None), 100);
        profile.snapshots.push(SnapshotSample {
            offset_us: 500,
            ..Default::default()
        });
        assert_eq!(extent(&profile, None), 500);
        profile.phases.push(PhaseSummary {
            top: vec![ObservedInterval {
                start_offset_us: 550,
                duration_us: 50,
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(extent(&profile, None), 600);
        assert_eq!(profile.command_end_offset_us, None);
    }

    #[test]
    fn command_profile_requires_an_unambiguous_run() {
        let runs = [
            RunInfo {
                run: "a".into(),
                ..Default::default()
            },
            RunInfo {
                run: "b".into(),
                ..Default::default()
            },
        ];
        assert!(profile_run(&runs, None).is_none());
        assert!(profile_run(&runs, Some("missing")).is_none());
        assert_eq!(profile_run(&runs, Some("b")).unwrap().run, "b");
        assert_eq!(profile_run(&runs[..1], None).unwrap().run, "a");
    }

    #[test]
    fn runless_rows_are_attributed_only_when_the_command_is_unambiguous() {
        let path = crate::temp_path("command-ui-runless.json");
        std::fs::write(
            &path,
            "[\n{\"ph\":\"X\",\"name\":\"action\",\"ts\":100,\"dur\":20}\n]\n",
        )
        .unwrap();
        let mut tr = crate::parse::parse(path.to_str().unwrap()).unwrap();
        tr.runs.push(RunInfo {
            run: "a".into(),
            ..Default::default()
        });
        assert_eq!(State::new(&tr, Some("a")).actions, Some((100, 120)));
        tr.runs.push(RunInfo {
            run: "b".into(),
            ..Default::default()
        });
        let state = State::new(&tr, Some("a"));
        assert_eq!(state.actions, None);
        assert_eq!(state.unattributed_actions, 1);
        std::fs::remove_file(path).unwrap();
    }
}
