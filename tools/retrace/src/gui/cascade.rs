/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A bounded, streamed experiment; no DAG traversal runs on the UI thread.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use super::CpOverride;
use super::GState;
use super::Ready;
use super::widgets;
use crate::dag;
use crate::sim;
use crate::types::Trace;

pub const TITLE: &str = "Greedy CP cascade";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CascadeColumn {
    #[default]
    Step,
    Action,
    Entered,
    Exited,
    Duration,
    CpAfter,
    Marginal,
    Cumulative,
    Total,
}

const COLUMNS: [(CascadeColumn, &str); 9] = [
    (CascadeColumn::Step, "Step"),
    (CascadeColumn::Action, "Chosen action"),
    (CascadeColumn::Entered, "Entered CP"),
    (CascadeColumn::Exited, "Left CP"),
    (CascadeColumn::Duration, "Removed duration"),
    (CascadeColumn::CpAfter, "CP after"),
    (CascadeColumn::Marginal, "Marginal saving"),
    (CascadeColumn::Cumulative, "Saving from start"),
    (CascadeColumn::Total, "Total modeled CP savings"),
];

const PATH_CHANGES: &str = "Distinct actions entering or leaving the selected representative \
    critical path after this removal, compared with the preceding step. This is not the union \
    of all equally critical paths and does not mean work was created or deleted. A zero-duration \
    action can remain on the path because its dependencies still exist.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CascadeSort {
    column: CascadeColumn,
    descending: bool,
}

impl CascadeSort {
    fn select(&mut self, column: CascadeColumn) {
        self.descending = if self.column == column {
            !self.descending
        } else {
            !matches!(column, CascadeColumn::Step | CascadeColumn::Action)
        };
        self.column = column;
    }

    /// Sorting is a view over the report; prefix application always reads the
    /// untouched experiment-order steps. Equal keys retain original step order.
    fn row_order<'a>(
        self,
        steps: &[sim::CascadeStep],
        name_of: impl Fn(u32) -> &'a str,
    ) -> Vec<usize> {
        let mut order: Vec<usize> = (0..steps.len()).collect();
        order.sort_by(|&a, &b| {
            let (left, right) = (&steps[a], &steps[b]);
            let comparison = match self.column {
                CascadeColumn::Step => left.index.cmp(&right.index),
                CascadeColumn::Action => name_of(left.act).cmp(name_of(right.act)),
                CascadeColumn::Entered => left.entered_actions.cmp(&right.entered_actions),
                CascadeColumn::Exited => left.exited_actions.cmp(&right.exited_actions),
                CascadeColumn::Duration => left.duration_us.cmp(&right.duration_us),
                CascadeColumn::CpAfter => left.cp_after_us.cmp(&right.cp_after_us),
                CascadeColumn::Marginal => left.marginal_saved_us.cmp(&right.marginal_saved_us),
                CascadeColumn::Cumulative => {
                    left.cumulative_saved_us.cmp(&right.cumulative_saved_us)
                }
                CascadeColumn::Total => left.total_saved_us.cmp(&right.total_saved_us),
            };
            let comparison = if self.descending {
                comparison.reverse()
            } else {
                comparison
            };
            comparison.then_with(|| left.index.cmp(&right.index))
        });
        order
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Running,
    Canceling,
    Complete,
    Canceled,
    Invalidated,
    Failed(String),
}

enum Message {
    Baseline { measured: i64, start: i64 },
    Step(sim::CascadeStep),
    Finished(Result<Option<&'static str>, String>),
}

struct Worker {
    rx: mpsc::Receiver<Message>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Never join on the UI thread. The worker releases its Arc<DagBundle>
        // and scratch at its next cooperative check, even when Ready is dropped.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct CascadeState {
    pub limit: usize,
    pub measured_cp_us: Option<i64>,
    pub start_cp_us: Option<i64>,
    pub steps: Vec<sim::CascadeStep>,
    pub prefix: usize,
    pub status: Status,
    pub source_overrides: Vec<CpOverride>,
    stop_reason: Option<&'static str>,
    sort: CascadeSort,
    worker: Option<Worker>,
    valid: bool,
}

impl Default for CascadeState {
    fn default() -> Self {
        Self {
            limit: sim::MAX_CASCADE_STEPS,
            measured_cp_us: None,
            start_cp_us: None,
            steps: Vec::new(),
            prefix: 0,
            status: Status::Idle,
            source_overrides: Vec::new(),
            stop_reason: None,
            sort: CascadeSort::default(),
            worker: None,
            valid: true,
        }
    }
}

impl CascadeState {
    pub fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    pub fn cancel(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel.store(true, Ordering::Relaxed);
            if self.valid {
                self.status = Status::Canceling;
            }
        }
    }

    /// Keep the old receiver only until its worker finishes. Its messages are
    /// ignored, and another worker cannot start in parallel with its scratch.
    pub fn invalidate(&mut self) {
        self.cancel();
        if self.start_cp_us.is_some() || self.is_running() {
            self.status = Status::Invalidated;
        }
        self.valid = false;
        self.measured_cp_us = None;
        self.start_cp_us = None;
        self.steps.clear();
        self.prefix = 0;
        self.stop_reason = None;
    }

    pub fn poll(&mut self) {
        loop {
            let Some(worker) = &self.worker else {
                return;
            };
            let message = worker.rx.try_recv();
            match message {
                Ok(Message::Baseline { measured, start }) if self.valid => {
                    self.measured_cp_us = Some(measured);
                    self.start_cp_us = Some(start);
                }
                Ok(Message::Step(step)) if self.valid => {
                    // The core is capped too; never retain an unbounded stream.
                    if self.steps.len() < sim::MAX_CASCADE_STEPS {
                        let follow = self.prefix == self.steps.len();
                        self.steps.push(step);
                        if follow {
                            self.prefix = self.steps.len();
                        }
                    }
                }
                Ok(Message::Finished(outcome)) => {
                    self.worker = None;
                    if self.valid {
                        self.status = match outcome {
                            Ok(Some(reason)) => {
                                self.stop_reason = Some(reason);
                                Status::Complete
                            }
                            Ok(None) => Status::Canceled,
                            Err(error) => Status::Failed(error),
                        };
                    }
                    return;
                }
                Ok(_) => {} // stale scenario's compact results
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker = None;
                    if self.valid {
                        self.status = Status::Failed(
                            "Cascade worker stopped unexpectedly; see stderr".to_owned(),
                        );
                    }
                    return;
                }
            }
        }
    }
}

/// Same composition as the manual scenario: admission-free base first,
/// absolute per-action edits last. No replay and no per-override marginals.
fn weights(
    b: &dag::DagBundle,
    admission: Option<&super::AdmissionWeights>,
    overrides: &[CpOverride],
    canceled: &impl Fn() -> bool,
) -> anyhow::Result<Option<Vec<i64>>> {
    if canceled() {
        return Ok(None);
    }
    let base = if overrides.contains(&CpOverride::FreeAdmission) {
        let Some(admission) = admission.filter(|a| a.n_recorded > 0) else {
            anyhow::bail!(
                "Execution-admission timing was not recorded; this scenario is unavailable."
            );
        };
        admission.weights.clone()
    } else {
        b.dag.weight_us.clone()
    };
    let edits: Vec<(usize, i64)> = overrides
        .iter()
        .filter_map(|o| match o {
            CpOverride::Node { act, w_us, .. } => Some((*act, *w_us)),
            CpOverride::FreeAdmission => None,
        })
        .collect();
    if edits.iter().any(|&(_, w)| w < 0) {
        anyhow::bail!("Scenario durations must be non-negative.");
    }
    Ok(sim::stack_overrides_cancellable(b, base, &edits, "cascade start", canceled).map(|s| s.w))
}

fn run(
    b: &dag::DagBundle,
    admission: Option<&super::AdmissionWeights>,
    overrides: &[CpOverride],
    limit: usize,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<Message>,
) -> anyhow::Result<Option<&'static str>> {
    let canceled = || cancel.load(Ordering::Relaxed);
    let Some(w) = weights(b, admission, overrides, &canceled)? else {
        return Ok(None);
    };
    let Some(mut cascade) = sim::GreedyCascade::new_with_cancel(&b.dag, w, limit, canceled) else {
        return Ok(None);
    };
    if tx
        .send(Message::Baseline {
            measured: cascade.base_cp_us(),
            start: cascade.start_cp_us(),
        })
        .is_err()
    {
        return Ok(None);
    }
    let mut completed = 0;
    while let Some(step) = cascade.next_with_cancel(canceled) {
        if canceled() || tx.send(Message::Step(step)).is_err() {
            return Ok(None);
        }
        completed += 1;
    }
    Ok(
        (!canceled()).then_some(if cascade.cp_us() == 0 || completed < limit {
            "no positive-duration action remains on the representative critical path"
        } else {
            "reached the requested removal limit"
        }),
    )
}

/// Construct all prefix edits in one batch. Repeated actions replace rather
/// than duplicate manual rows, exactly as set_cp_override does.
fn prefix_overrides(
    source: &[CpOverride],
    steps: &[sim::CascadeStep],
    prefix: usize,
) -> Vec<CpOverride> {
    let mut overrides = source.to_vec();
    for step in steps.iter().take(prefix.min(sim::MAX_CASCADE_STEPS)) {
        let act = step.act as usize;
        let new = CpOverride::Node {
            act,
            w_us: 0,
            label: "removed by cascade".to_owned(),
        };
        match overrides
            .iter_mut()
            .find(|o| matches!(o, CpOverride::Node { act: a, .. } if *a == act))
        {
            Some(old) => *old = new,
            None => overrides.push(new),
        }
    }
    overrides
}

impl Ready {
    pub fn start_cp_cascade(&mut self) {
        if self.cascade.is_running() {
            return;
        }
        let GState::Ready { dag: Some(b), .. } = &self.gstate else {
            return;
        };
        let b = b.clone();
        let source = self.cp_overrides.clone();
        let admission = self.an.as_ref().and_then(|an| an.admission.clone());
        if source.contains(&CpOverride::FreeAdmission) && admission.is_none() {
            self.ensure_analysis();
            self.cascade.status = Status::Failed(
                "Loading admission measurements; run the experiment when analysis is ready.".into(),
            );
            return;
        }
        let limit = self.cascade.limit.clamp(1, sim::MAX_CASCADE_STEPS);
        let cancel = Arc::new(AtomicBool::new(false));
        let canceled = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.cascade = CascadeState {
            limit,
            source_overrides: source.clone(),
            status: Status::Running,
            sort: self.cascade.sort,
            worker: Some(Worker { rx, cancel }),
            ..CascadeState::default()
        };
        let started = std::thread::Builder::new()
            .name("retrace-cascade".into())
            .spawn(move || {
                let result = run(&b, admission.as_deref(), &source, limit, &canceled, &tx);
                // Release graph/trace references before permitting another experiment.
                drop(b);
                drop(tx.send(Message::Finished(
                    result.map_err(|error| format!("{error:#}")),
                )));
            });
        if let Err(e) = started {
            self.cascade.worker = None;
            self.cascade.status = Status::Failed(format!("cannot start cascade worker: {e}"));
        }
    }

    pub fn apply_cp_cascade_prefix(&mut self, prefix: usize) -> bool {
        if self.cascade.is_running()
            || !self.cascade.valid
            || self.cp_overrides != self.cascade.source_overrides
            || prefix == 0
            || prefix > self.cascade.steps.len()
        {
            return false;
        }
        self.cp_overrides = prefix_overrides(&self.cp_overrides, &self.cascade.steps, prefix);
        // One respawn, not N O(V+E) worker launches. This also invalidates the
        // experiment, so another click cannot apply a prefix from a stale base.
        self.respawn_cp_sim();
        self.panels.cp = true;
        true
    }
}

pub fn show(app: &mut Ready, ctx: &egui::Context) {
    let mut open = app.panels.cascade;
    widgets::popup(
        ctx,
        app.native_popups,
        TITLE,
        TITLE,
        &mut open,
        [1000.0, 560.0],
        |ui| {
            contents(app, ui);
        },
    );
    app.panels.cascade = open;
    if !open {
        app.cascade.cancel();
    }
}

fn action_name(tr: &Trace, act: u32) -> &str {
    tr.acts
        .get(act as usize)
        .and_then(|act| tr.names.get(act.name as usize))
        .map(String::as_str)
        .unwrap_or("Action unavailable")
}

fn step_table(
    ui: &mut egui::Ui,
    tr: &Trace,
    steps: &[sim::CascadeStep],
    sort: &mut CascadeSort,
    prefix: &mut usize,
    navigate: &mut Option<usize>,
) -> egui::scroll_area::ScrollAreaOutput<()> {
    egui::ScrollArea::horizontal()
        .id_salt("cascade_steps_scroll")
        .auto_shrink([false, true])
        .max_width(ui.available_width())
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            egui::Grid::new("cascade_steps")
                .num_columns(COLUMNS.len())
                .striped(true)
                .show(ui, |ui| {
                    for (column, header) in COLUMNS {
                        let arrow = if column == sort.column {
                            if sort.descending { " \u{25be}" } else { " \u{25b4}" }
                        } else {
                            ""
                        };
                        let help = match column {
                            CascadeColumn::Step => "Original experiment order. Select a step to apply all removals through it, regardless of display sorting.",
                            CascadeColumn::Entered | CascadeColumn::Exited => PATH_CHANGES,
                            CascadeColumn::Duration => "Duration removed in this scenario, after any starting overrides; not necessarily the original recorded duration.",
                            CascadeColumn::Marginal => "Additional dependency-model saving from this step, conditional on every preceding removal. This is not an independent saving or a wall-time prediction.",
                            _ => "Sorting changes only the display, not the experiment or the removals applied.",
                        };
                        if ui
                            .add(
                                egui::Label::new(egui::RichText::new(format!("{header}{arrow}")).strong())
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text(format!("Click to sort; click again to reverse.\n{help}"))
                            .clicked()
                        {
                            sort.select(column);
                        }
                    }
                    ui.end_row();
                    for row in sort.row_order(steps, |act| action_name(tr, act)) {
                        let step = &steps[row];
                        if ui
                            .selectable_label(*prefix == step.index, step.index.to_string())
                            .on_hover_text(format!("Select removals 1 through {} in experiment order", step.index))
                            .clicked()
                        {
                            *prefix = step.index;
                        }
                        if tr.acts.get(step.act as usize).is_some() {
                            let name = action_name(tr, step.act);
                            let short: String = name.chars().take(42).collect();
                            let short = if short.len() < name.len() {
                                format!("{short}…")
                            } else {
                                short
                            };
                            if ui.link(short).on_hover_text(name).clicked() {
                                *navigate = Some(step.act as usize);
                            }
                        } else {
                            ui.label("Action unavailable");
                        }
                        let path_help = format!(
                            "{PATH_CHANGES}\nPath {} after this removal; CP before: {:.3}s.",
                            if step.path_changed { "changed" } else { "unchanged" },
                            step.cp_before_us as f64 / 1e6,
                        );
                        ui.monospace(format!("+{}", step.entered_actions)).on_hover_text(&path_help);
                        ui.monospace(format!("-{}", step.exited_actions)).on_hover_text(&path_help);
                        for duration in [
                            step.duration_us,
                            step.cp_after_us,
                            step.marginal_saved_us,
                            step.cumulative_saved_us,
                            step.total_saved_us,
                        ] {
                            ui.monospace(format!("{:.3}s", duration as f64 / 1e6));
                        }
                        ui.end_row();
                    }
                });
        })
}

/// Separate from popup plumbing so snapshot tests can draw deterministic rows.
pub fn contents(app: &mut Ready, ui: &mut egui::Ui) {
    let have_dag = matches!(&app.gstate, GState::Ready { dag: Some(_), .. });
    let running = app.cascade.is_running();
    let mut start = false;
    let mut cancel = false;
    let mut apply = false;
    let mut navigate = None;
    ui.label("Recompute the critical path after every removal, then choose its longest remaining action.");
    ui.weak("Remove means zero duration for that action, not a cache hit with lookup cost. This changes no build or source files.");
    ui.weak("Keeps other recorded durations and dependencies fixed; assumes unlimited resources. Not a build-duration forecast or a globally optimal selection.");
    if let GState::Ready { quality, .. } = &app.gstate {
        if !quality.graph_complete() {
            ui.label("Graph completeness is unverified or incomplete; this experiment uses only recorded dependencies.");
        }
    }
    ui.horizontal(|ui| {
        ui.label("Maximum removals");
        ui.add_enabled(
            !running,
            egui::DragValue::new(&mut app.cascade.limit).range(1..=sim::MAX_CASCADE_STEPS),
        );
        start = ui
            .add_enabled(have_dag && !running, egui::Button::new("Run experiment"))
            .clicked();
        cancel = ui
            .add_enabled(running, egui::Button::new("Cancel"))
            .clicked();
        if running {
            ui.spinner();
        }
    });
    ui.label(format!(
        "Starting from {} current override(s)",
        app.cp_overrides.len()
    ));
    let status = match &app.cascade.status {
        Status::Idle => "Ready; the current manual scenario remains unchanged".to_owned(),
        Status::Running => format!(
            "Replaying: {} completed removal(s)",
            app.cascade.steps.len()
        ),
        Status::Canceling => "Canceling; waiting for the worker to release its scratch".to_owned(),
        Status::Complete => format!(
            "Finished: {} removal(s); {}",
            app.cascade.steps.len(),
            app.cascade.stop_reason.unwrap_or("experiment complete")
        ),
        Status::Canceled => "Canceled; completed rows remain available".to_owned(),
        Status::Invalidated if running => {
            "Scenario changed; previous results discarded. Waiting for the worker to finish"
                .to_owned()
        }
        Status::Invalidated => {
            "Scenario changed; previous results discarded. Run again for the current overrides"
                .to_owned()
        }
        Status::Failed(error) => format!("Experiment failed: {error}"),
    };
    ui.label(status);
    if let (Some(measured), Some(initial)) = (app.cascade.measured_cp_us, app.cascade.start_cp_us) {
        ui.label(format!(
            "Baseline CP (recorded durations): {:.3}s | Starting scenario CP (step 0): {:.3}s",
            measured as f64 / 1e6,
            initial as f64 / 1e6
        ));
    }
    ui.separator();
    step_table(
        ui,
        &app.tr,
        &app.cascade.steps,
        &mut app.cascade.sort,
        &mut app.cascade.prefix,
        &mut navigate,
    );
    ui.weak("A zero marginal saving can expose a tied critical path; subsequent removals may still help. Savings are recomputed, never added as independent estimates.");
    let can_apply = !running
        && app.cascade.valid
        && app.cascade.prefix > 0
        && app.cascade.prefix <= app.cascade.steps.len()
        && app.cp_overrides == app.cascade.source_overrides;
    if ui
        .add_enabled(
            can_apply,
            egui::Button::new(format!(
                "Apply first {} removal(s) in experiment order",
                app.cascade.prefix
            )),
        )
        .clicked()
    {
        apply = true;
    }
    if let Some(act) = navigate {
        app.nav_req = Some(act);
    }
    if cancel {
        app.cascade.cancel();
    }
    if start {
        app.start_cp_cascade();
    }
    if apply && app.apply_cp_cascade_prefix(app.cascade.prefix) {
        widgets::popup_raise(ui.ctx(), "Critical path (dependency graph)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(index: usize, act: u32) -> sim::CascadeStep {
        sim::CascadeStep {
            index,
            node: act,
            act,
            duration_us: 10,
            cp_before_us: 20,
            cp_after_us: 10,
            marginal_saved_us: 10,
            cumulative_saved_us: 10,
            total_saved_us: 10,
            path_changed: true,
            entered_actions: 1,
            exited_actions: 1,
        }
    }

    #[test]
    fn headers_toggle_sort_without_changing_default_experiment_order() {
        let mut sort = CascadeSort::default();
        assert_eq!(sort.column, CascadeColumn::Step);
        assert!(!sort.descending);
        sort.select(CascadeColumn::Step);
        assert!(sort.descending);
        sort.select(CascadeColumn::Step);
        assert!(!sort.descending);
        sort.select(CascadeColumn::Marginal);
        assert!(sort.descending);
        sort.select(CascadeColumn::Marginal);
        assert!(!sort.descending);
        sort.select(CascadeColumn::Action);
        assert!(!sort.descending);
        sort.select(CascadeColumn::Step);
        assert!(!sort.descending);
    }

    #[test]
    fn numeric_sort_uses_exact_keys_and_step_order_for_ties() {
        let steps: Vec<_> = [
            10_000_000_000_000_001,
            10_000_000_000_000_000,
            10_000_000_000_000_001,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, value)| sim::CascadeStep {
            entered_actions: if index == 1 { 2 } else { 11 },
            exited_actions: if index == 1 { 2 } else { 11 },
            duration_us: value,
            cp_after_us: value,
            marginal_saved_us: value,
            cumulative_saved_us: value,
            total_saved_us: value,
            ..step(index + 1, index as u32)
        })
        .collect();
        for column in [
            CascadeColumn::Entered,
            CascadeColumn::Exited,
            CascadeColumn::Duration,
            CascadeColumn::CpAfter,
            CascadeColumn::Marginal,
            CascadeColumn::Cumulative,
            CascadeColumn::Total,
        ] {
            let mut sort = CascadeSort {
                column,
                descending: false,
            };
            assert_eq!(sort.row_order(&steps, |_| ""), [1, 0, 2], "{column:?}");
            sort.descending = true;
            assert_eq!(sort.row_order(&steps, |_| ""), [0, 2, 1], "{column:?}");
        }
        let mut negative = steps;
        negative[0].total_saved_us = -10;
        negative[1].total_saved_us = -2;
        negative[2].total_saved_us = 0;
        assert_eq!(
            CascadeSort {
                column: CascadeColumn::Total,
                descending: true
            }
            .row_order(&negative, |_| ""),
            [2, 1, 0],
        );
    }

    #[test]
    fn action_sort_uses_full_names_not_the_short_display_label() {
        let prefix = "x".repeat(42);
        let names = [
            format!("{prefix}z"),
            format!("{prefix}a"),
            format!("{prefix}a"),
        ];
        let steps = [step(1, 0), step(2, 1), step(3, 2)];
        let mut sort = CascadeSort {
            column: CascadeColumn::Action,
            descending: false,
        };
        assert_eq!(
            sort.row_order(&steps, |act| &names[act as usize]),
            [1, 2, 0]
        );
        sort.descending = true;
        assert_eq!(
            sort.row_order(&steps, |act| &names[act as usize]),
            [0, 1, 2]
        );
    }

    #[test]
    fn display_sort_does_not_reorder_report_or_applied_prefix() {
        let steps = vec![
            sim::CascadeStep {
                marginal_saved_us: 1,
                ..step(1, 5)
            },
            sim::CascadeStep {
                marginal_saved_us: 20,
                ..step(2, 9)
            },
            sim::CascadeStep {
                marginal_saved_us: 300,
                ..step(3, 2)
            },
        ];
        let original = steps.clone();
        let sort = CascadeSort {
            column: CascadeColumn::Marginal,
            descending: true,
        };
        let order = sort.row_order(&steps, |_| "");
        assert_eq!(order, [2, 1, 0]);
        let selected_step = steps[order[1]].index;
        let overrides = prefix_overrides(&[], &steps, selected_step);
        let acts: Vec<_> = overrides
            .iter()
            .filter_map(|o| match o {
                CpOverride::Node { act, .. } => Some(*act),
                CpOverride::FreeAdmission => None,
            })
            .collect();
        assert_eq!(
            acts,
            [5, 9],
            "first two removals, not first two displayed rows"
        );
        assert_eq!(steps, original, "algorithm/report order stays intact");
        assert_eq!(CascadeSort::default().row_order(&steps, |_| ""), [0, 1, 2]);
    }

    #[test]
    fn narrow_table_scrolls_instead_of_wrapping_or_expanding_its_viewport() {
        let path = crate::testdata("smoke-trace.json");
        let tr = crate::parse::parse(&path).unwrap();
        let ctx = egui::Context::default();
        let steps = vec![step(1, 0)];
        let mut sort = CascadeSort::default();
        let mut prefix = 1;
        let mut navigate = None;
        for _ in 0..2 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(360.0, 300.0),
                    )),
                    ..Default::default()
                },
                |root_ui| {
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        let width = ui.available_width();
                        let output =
                            step_table(ui, &tr, &steps, &mut sort, &mut prefix, &mut navigate);
                        assert!(output.content_size.x > output.inner_rect.width());
                        assert!(output.inner_rect.width() <= width);
                    });
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(prefix, 1);
        assert_eq!(navigate, None);
    }

    fn state_with_worker() -> (CascadeState, mpsc::Sender<Message>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let state = CascadeState {
            status: Status::Running,
            worker: Some(Worker {
                rx,
                cancel: cancel.clone(),
            }),
            ..CascadeState::default()
        };
        (state, tx, cancel)
    }

    #[test]
    fn streamed_steps_follow_the_tail_without_overriding_manual_selection() {
        let (mut state, tx, _) = state_with_worker();
        for index in 1..=2 {
            tx.send(Message::Step(step(index, 0)))
                .expect("should deliver a step");
        }
        state.poll();
        assert_eq!(state.prefix, 2);
        state.prefix = 1;
        tx.send(Message::Step(step(3, 0)))
            .expect("should deliver a step");
        state.poll();
        assert_eq!(state.prefix, 1);
        state.prefix = 3;
        tx.send(Message::Step(step(4, 0)))
            .expect("should deliver a step");
        state.poll();
        assert_eq!(state.prefix, 4);
    }

    #[test]
    fn invalidation_cancels_and_rejects_queued_old_results() {
        let (mut state, tx, cancel) = state_with_worker();
        tx.send(Message::Baseline {
            measured: 20,
            start: 20,
        })
        .unwrap();
        tx.send(Message::Step(step(1, 0))).unwrap();
        state.invalidate();
        assert!(cancel.load(Ordering::Relaxed));
        assert!(
            state.is_running(),
            "cannot overlap a canceled worker's scratch"
        );
        tx.send(Message::Finished(Ok(Some(
            "reached the requested removal limit",
        ))))
        .unwrap();
        state.poll();
        assert!(!state.is_running());
        assert_eq!(state.status, Status::Invalidated);
        assert_eq!(state.start_cp_us, None);
        assert!(state.steps.is_empty());
    }

    #[test]
    fn cancellation_retains_completed_rows_and_drop_signals_worker() {
        let (mut state, tx, cancel) = state_with_worker();
        tx.send(Message::Step(step(1, 0))).unwrap();
        state.poll();
        state.cancel();
        assert_eq!(state.status, Status::Canceling);
        assert!(cancel.load(Ordering::Relaxed));
        tx.send(Message::Finished(Ok(None))).unwrap();
        state.poll();
        assert_eq!(state.status, Status::Canceled);
        assert_eq!(state.steps.len(), 1);
        let (state, _tx, cancel) = state_with_worker();
        drop(state);
        assert!(cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn disconnected_worker_is_an_error_not_a_permanent_spinner() {
        let (mut state, tx, _) = state_with_worker();
        drop(tx);
        state.poll();
        assert!(matches!(state.status, Status::Failed(_)));
        assert!(!state.is_running());
    }

    #[test]
    fn prefix_preserves_manual_rows_and_replaces_existing_action_once() {
        let source = vec![
            CpOverride::FreeAdmission,
            CpOverride::Node {
                act: 5,
                w_us: 50,
                label: "at 50%".to_owned(),
            },
            CpOverride::Node {
                act: 7,
                w_us: 4,
                label: "lookup only".to_owned(),
            },
        ];
        let steps = vec![step(1, 5), step(2, 9)];
        let result = prefix_overrides(&source, &steps, 1);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], CpOverride::FreeAdmission);
        assert_eq!(result[2], source[2]);
        assert!(matches!(
            result[1],
            CpOverride::Node {
                act: 5,
                w_us: 0,
                ..
            }
        ));
        assert_eq!(prefix_overrides(&source, &steps, 2).len(), 4);
        assert_eq!(prefix_overrides(&source, &steps, 0), source);
    }

    #[test]
    fn streaming_retains_only_twenty_compact_rows() {
        let (mut state, tx, _) = state_with_worker();
        for index in 1..=40 {
            tx.send(Message::Step(step(index, index as u32))).unwrap();
        }
        tx.send(Message::Finished(Ok(Some(
            "reached the requested removal limit",
        ))))
        .unwrap();
        state.poll();
        assert_eq!(state.steps.len(), sim::MAX_CASCADE_STEPS);
        assert_eq!(state.prefix, sim::MAX_CASCADE_STEPS);
        state.limit = 1;
        assert_eq!(
            state.stop_reason,
            Some("reached the requested removal limit")
        );
    }

    #[test]
    fn applying_prefix_requires_same_scenario_and_invalidates_its_source() {
        let path = crate::testdata("smoke-trace.json");
        let mut app = Ready::new(crate::parse::parse(&path).unwrap(), None);
        app.cascade.steps = vec![step(1, 0)];
        app.cascade.status = Status::Complete;
        app.cp_overrides.push(CpOverride::FreeAdmission);
        assert!(!app.apply_cp_cascade_prefix(1), "changed launch scenario");
        app.cp_overrides.clear();
        assert!(!app.apply_cp_cascade_prefix(0));
        assert!(!app.apply_cp_cascade_prefix(2));
        assert!(app.apply_cp_cascade_prefix(1));
        assert!(matches!(
            app.cp_overrides.as_slice(),
            [CpOverride::Node {
                act: 0,
                w_us: 0,
                ..
            }]
        ));
        assert!(!app.cascade.valid);
        assert!(app.cascade.steps.is_empty());
        assert!(app.panels.cp);
        assert!(!app.apply_cp_cascade_prefix(1), "cannot reapply stale rows");
    }
}
