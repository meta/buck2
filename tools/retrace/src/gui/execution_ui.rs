/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Selected-action execution evidence. Only local metadata is loaded at open;
//! stream bodies are bounded, lazy reads. Copy, Save and RE fetch are separate
//! user actions, never side effects of selecting a tab.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use crate::execution;
use crate::execution::Command;
use crate::execution::DetailReader;
use crate::execution::DetailRef;
use crate::execution::LoadedAction;
use crate::execution::Output;
use crate::execution::OutputState;
use crate::execution::TextPreview;
use crate::execution_fetch;
use crate::execution_fetch::FetchKind;
use crate::execution_fetch::FetchRequest;
use crate::execution_fetch::FetchResult;
use crate::types::Trace;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Command,
    Stdout,
    Stderr,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Command => "Command",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Command => 0,
            Self::Stdout => 1,
            Self::Stderr => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selection {
    Attempt(usize),
    Observation(usize),
    Legacy,
    NoReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewKey {
    selection: Selection,
    tab: Tab,
}

struct PendingPreview {
    key: PreviewKey,
    rx: mpsc::Receiver<Result<TextPreview, String>>,
}

struct PendingFetch {
    key: PreviewKey,
    request: FetchRequest,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<FetchResult>,
}

struct Fetched {
    key: PreviewKey,
    result: FetchResult,
    preview: TextPreview,
}

// Closing a popup requests cancellation, but does not immediately stop a
// subprocess. Keep the next popup from starting an overlapping remote job.
static FETCH_BUSY: AtomicBool = AtomicBool::new(false);

struct FetchPermit;

impl Drop for FetchPermit {
    fn drop(&mut self) {
        FETCH_BUSY.store(false, Ordering::Release);
    }
}

/// State belongs to one open detail, not to the action graph or all actions.
/// Switching actions drops pending result receivers; no result can be adopted
/// by a different run/span. Each background read is capped at 64 KiB; a read
/// whose receiver was dropped may briefly overlap the next selection's read.
pub struct ExecutionUi {
    loaded: Option<LoadedAction>,
    load_error: Option<String>,
    legacy: Option<Command>,
    legacy_digest: Option<String>,
    legacy_use_case: Option<String>,
    selection: Selection,
    tab: Tab,
    search: [String; 3],
    preview: Option<(PreviewKey, Result<TextPreview, String>)>,
    pending: Option<PendingPreview>,
    saving: Option<mpsc::Receiver<Result<Option<PathBuf>, String>>>,
    notice: Option<String>,
    fetching: Option<PendingFetch>,
    fetched: Option<Fetched>,
    fetch_search: String,
}

impl ExecutionUi {
    pub fn new(tr: &Trace, rec: &serde_json::Value) -> Self {
        let (loaded, load_error) = match execution::load(tr, rec) {
            Ok(value) => (value, None),
            Err(error) => (
                None,
                Some(format!("Execution details unavailable: {error:#}")),
            ),
        };
        let selection = initial_selection(loaded.as_ref());
        Self {
            loaded,
            load_error,
            legacy: crate::detail::command_argv(rec).map(|argv| Command {
                argv,
                ..Command::default()
            }),
            legacy_digest: rec
                .pointer("/args/digest")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            legacy_use_case: rec
                .pointer("/args/re/use_case")
                .or_else(|| rec.pointer("/args/use_case"))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            selection,
            tab: Tab::Command,
            search: Default::default(),
            preview: None,
            pending: None,
            saving: None,
            notice: None,
            fetching: None,
            fetched: None,
            fetch_search: String::new(),
        }
    }

    fn key(&self) -> PreviewKey {
        PreviewKey {
            selection: self.selection,
            tab: self.tab,
        }
    }

    fn command(&self) -> Option<&Command> {
        match self.selection {
            Selection::Attempt(index) => self.loaded.as_ref()?.command_for(index),
            Selection::Observation(index) => {
                Some(&self.loaded.as_ref()?.observations.get(index)?.command)
            }
            Selection::Legacy => self.legacy.as_ref(),
            Selection::NoReport => None,
        }
    }

    fn output(&self) -> Option<&Output> {
        let Selection::Attempt(index) = self.selection else {
            return None;
        };
        let attempt = self.loaded.as_ref()?.attempts.get(index)?;
        match self.tab {
            Tab::Command => None,
            Tab::Stdout => Some(&attempt.stdout),
            Tab::Stderr => Some(&attempt.stderr),
        }
    }

    fn source(&self) -> &'static str {
        match self.selection {
            Selection::Attempt(index) => {
                if self.tab != Tab::Command
                    || self.loaded.as_ref().is_some_and(|loaded| {
                        loaded
                            .attempts
                            .get(index)
                            .is_some_and(|a| a.command.is_some())
                    })
                {
                    "Source: command report in event log"
                } else {
                    "Source: local execution-start record in event log"
                }
            }
            Selection::Observation(_) => {
                "Source: recorded execution-start span; not assigned to a command report"
            }
            Selection::Legacy => "Source: legacy trace record",
            Selection::NoReport => {
                "No command report or command-start record was captured for this action"
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.pending {
            match pending.rx.try_recv() {
                Ok(result) => {
                    if pending.key == self.key() {
                        self.preview = Some((pending.key, result));
                    }
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.preview = Some((pending.key, Err("Output preview worker stopped".into())));
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
            }
        }
        if let Some(rx) = &self.saving {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("Save worker stopped".into())),
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(120));
                    None
                }
            };
            if let Some(result) = result {
                self.notice = Some(match result {
                    Ok(Some(path)) => format!("Saved {}", path.display()),
                    Ok(None) => "Save canceled".to_owned(),
                    Err(error) => format!("Save failed: {error}"),
                });
                self.saving = None;
            }
        }
        if let Some(pending) = &self.fetching {
            let result = match pending.rx.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("Metadata fetch worker stopped")),
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(120));
                    None
                }
            };
            if let Some(result) = result {
                match result {
                    Ok(result)
                        if accepts_fetch(
                            pending.key,
                            pending.key,
                            &pending.request,
                            &result.request,
                        ) =>
                    {
                        let mut preview = CappedText::default();
                        preview.push(&result.text);
                        let mut preview = preview.finish();
                        preview.truncated |= result.text_truncated;
                        self.fetched = Some(Fetched {
                            key: pending.key,
                            result,
                            preview,
                        });
                    }
                    Ok(_) => {} // mismatched echoed request; never adopt foreign evidence
                    Err(message) => {
                        self.notice = Some(message.to_owned());
                    }
                }
                self.fetching = None;
            }
        }
    }

    fn ensure_preview(&mut self) {
        let key = self.key();
        if self.preview.as_ref().is_some_and(|(k, _)| *k == key)
            || self.pending.as_ref().is_some_and(|p| p.key == key)
        {
            return;
        }
        self.preview = None;
        // A selection/tab change abandons only the result, not a shared file
        // handle or another detail's request. Each positional read is bounded.
        self.pending = None;
        if self.tab == Tab::Command {
            if let Some(command) = self.command() {
                self.preview = Some((key, Ok(command_preview(&command.argv))));
            }
            return;
        }
        let Some(output) = self.output() else {
            return;
        };
        if output.state == OutputState::Empty {
            self.preview = Some((
                key,
                Ok(TextPreview {
                    text: String::new(),
                    total_bytes: 0,
                    truncated: false,
                }),
            ));
        } else if let Some(reference) = output.content {
            let reader = self
                .loaded
                .as_ref()
                .expect("output requires loaded action")
                .reader
                .clone();
            let (tx, rx) = mpsc::channel();
            let started = std::thread::Builder::new()
                .name("retrace-output-preview".into())
                .spawn(move || {
                    let result = reader
                        .preview(reference, execution::PREVIEW_BYTES)
                        .map_err(|error| format!("{error:#}"));
                    drop(tx.send(result));
                });
            match started {
                Ok(_) => self.pending = Some(PendingPreview { key, rx }),
                Err(e) => {
                    self.preview = Some((key, Err(format!("cannot start preview worker: {e}"))))
                }
            }
        }
    }

    fn selectors(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        if loaded.attempts.len() > 1 {
            egui::ComboBox::from_id_salt("execution-attempt")
                .selected_text(match self.selection {
                    Selection::Attempt(i) => attempt_label(&loaded.attempts[i]),
                    _ => "Choose a recorded command report".to_owned(),
                })
                .show_ui(ui, |ui| {
                    for (i, attempt) in loaded.attempts.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.selection,
                            Selection::Attempt(i),
                            attempt_label(attempt),
                        );
                    }
                });
        } else if let Some(attempt) = loaded.attempts.first() {
            ui.horizontal_wrapped(|ui| {
                ui.weak(format!("{} · {}", attempt.executor, attempt.status));
                if self.selection != Selection::Attempt(0)
                    && ui.small_button("Command report").clicked()
                {
                    self.selection = Selection::Attempt(0);
                }
            });
        }
        if !loaded.observations.is_empty() {
            // Observations are separately selectable; no positional guess
            // turns a resource-control restart into a report/attempt.
            egui::ComboBox::from_id_salt("execution-observation")
                .selected_text(match self.selection {
                    Selection::Observation(i) => observation_label(&loaded.observations[i]),
                    _ => format!("Observed commands ({})", loaded.observations.len()),
                })
                .show_ui(ui, |ui| {
                    for (i, observation) in loaded.observations.iter().enumerate() {
                        ui.selectable_value(&mut self.selection, Selection::Observation(i), observation_label(observation));
                    }
                })
                .response
                .on_hover_text("Recorded start spans, not additional command reports. Selecting one does not attribute stdout/stderr or a final status to it.");
        }
    }

    /// Returns a clipboard toast for the parent. All other status remains next
    /// to its explicit action, including save failures and missing evidence.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<&'static str> {
        self.poll(ui.ctx());
        let mut copied = None;
        self.selectors(ui);
        ui.horizontal(|ui| {
            for tab in [Tab::Command, Tab::Stdout, Tab::Stderr] {
                ui.selectable_value(&mut self.tab, tab, tab.label());
            }
        });
        // Explicit fetches outlive tab changes. Retain their keyed result so
        // returning to the requested tab does not repeat network work.
        if let Some(error) = &self.load_error {
            ui.label(error);
        }
        self.ensure_preview();
        let has_command = self.command().is_some();
        if self.tab != Tab::Command || has_command {
            ui.weak(self.source());
        }
        if self.tab == Tab::Command {
            if !has_command {
                ui.label("Command arguments were not recorded, or cannot be associated unambiguously with this report.");
            } else if self
                .command()
                .is_some_and(|command| command.argv.is_empty())
            {
                ui.label("Recorded command argument list is empty.");
            }
            ui.weak(
                "Shell-quoted arguments only; environment and working directory are not applied.",
            );
        } else {
            let message = match self.output() {
                Some(output) => output_message(self.tab, output.state),
                None => "No output is attributed to this selection. Select a recorded command report when available.".to_owned(),
            };
            ui.label(message);
            if let Some(note) = self.output().and_then(|output| output.note.as_deref()) {
                ui.weak(note);
            }
        }
        if let Some(pending) = &self.pending {
            if pending.key == self.key() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading recorded output…");
                });
            }
        }
        if let Some((key, result)) = &self.preview {
            if *key == self.key() {
                match result {
                    Err(error) => {
                        ui.label(format!("Preview unavailable: {error}"));
                    }
                    Ok(preview) => {
                        let search = &mut self.search[self.tab.index()];
                        preview_ui(ui, preview, search, self.tab, true);
                    }
                }
            }
        }
        let can_copy = self.tab == Tab::Command && has_command
            || self.preview.as_ref().is_some_and(|(_, p)| p.is_ok());
        let truncated = self
            .preview
            .as_ref()
            .and_then(|(_, p)| p.as_ref().ok())
            .is_some_and(|preview| preview.truncated);
        let copy_label = if self.tab == Tab::Command {
            "Copy command"
        } else if self
            .output()
            .is_some_and(|output| output.state == OutputState::ProducerNotice)
        {
            "Copy notice"
        } else if truncated {
            "Copy preview"
        } else {
            "Copy text"
        };
        let can_save = if self.tab == Tab::Command {
            has_command
        } else {
            self.output().is_some_and(|output| {
                output.content.is_some() || output.state == OutputState::Empty
            })
        };
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(can_copy, egui::Button::new(copy_label)).clicked() {
                let text = if self.tab == Tab::Command {
                    self.command().map(|command| crate::analyses::shlex_join(&command.argv))
                } else {
                    self.preview.as_ref().and_then(|(_, p)| p.as_ref().ok()).map(|p| p.text.clone())
                };
                if let Some(text) = text {
                    ui.ctx().copy_text(text);
                    copied = Some(if self.tab == Tab::Command { "recorded command copied" }
                        else if self.output().is_some_and(|output| output.state == OutputState::ProducerNotice) { "producer notice copied (not complete output)" }
                        else if truncated { "output preview copied (not the entire stream)" }
                        else { "recorded output text copied" });
                }
            }
            if ui.add_enabled(can_save && self.saving.is_none(), egui::Button::new("Save…"))
                .on_hover_text("Choose a new file. Recorded output saves the complete stored bytes, not only this preview. Existing files are never overwritten.")
                .clicked()
            {
                if let Some(payload) = self.save_payload() {
                    self.start_save(payload);
                }
            }
            if self.saving.is_some() { ui.spinner(); ui.weak("Save in progress…"); }
        });
        if self.tab == Tab::Command {
            if let Some(command) = self.command() {
                command_context(ui, command);
            }
        }
        if let Some(message) = self.fetch_ui(ui) {
            copied = Some(message);
        }
        if let Selection::Attempt(index) = self.selection {
            if let Some(attempt) = self.loaded.as_ref().and_then(|d| d.attempts.get(index)) {
                if let Some(error) = &attempt.error {
                    ui.strong("Recorded command error");
                    let preview = text_preview(error);
                    egui::ScrollArea::both()
                        .id_salt("execution-command-error")
                        .max_height(100.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&preview.text).monospace())
                                    .wrap(),
                            );
                        });
                    if preview.truncated {
                        ui.weak(format!(
                            "Error preview truncated: {} of {} recorded bytes.",
                            preview.text.len(),
                            preview.total_bytes
                        ));
                    }
                }
            }
        }
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        copied
    }

    fn save_payload(&self) -> Option<SavePayload> {
        if self.tab == Tab::Command {
            self.command()
                .map(|command| SavePayload::Text(crate::analyses::shlex_join(&command.argv)))
        } else {
            let output = self.output()?;
            if let Some(reference) = output.content {
                Some(SavePayload::Stored(
                    self.loaded.as_ref()?.reader.clone(),
                    reference,
                ))
            } else if output.state == OutputState::Empty {
                Some(SavePayload::Text(String::new()))
            } else {
                None
            }
        }
    }

    fn fetch_request(&self) -> Option<FetchRequest> {
        let (digest, use_case) = match self.selection {
            Selection::Attempt(index) => {
                let attempt = self.loaded.as_ref()?.attempts.get(index)?;
                (attempt.action_digest.as_ref()?, attempt.use_case.clone())
            }
            Selection::Observation(index) => {
                let observation = self.loaded.as_ref()?.observations.get(index)?;
                (observation.action_digest.as_ref()?, None)
            }
            Selection::Legacy | Selection::NoReport => {
                (self.legacy_digest.as_ref()?, self.legacy_use_case.clone())
            }
        };
        Some(FetchRequest {
            kind: if self.tab == Tab::Command {
                FetchKind::ActionMetadata {
                    action_digest: digest.clone(),
                }
            } else {
                FetchKind::CurrentCacheMetadata {
                    action_digest: digest.clone(),
                }
            },
            use_case,
        })
    }

    fn start_fetch(&mut self, request: FetchRequest) {
        if self.fetching.is_some() {
            return;
        }
        if FETCH_BUSY
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.notice = Some(
                "Another metadata fetch is still running or canceling; try again when it finishes."
                    .to_owned(),
            );
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_request = request.clone();
        let (tx, rx) = mpsc::channel();
        let permit = FetchPermit;
        let worker = std::thread::Builder::new()
            .name("retrace-execution-fetch".into())
            .spawn(move || {
                let _permit = permit;
                drop(tx.send(execution_fetch::run(&worker_request, &worker_cancel)));
            });
        if let Err(error) = worker {
            self.notice = Some(format!("Could not start metadata fetch: {error}"));
            return;
        }
        self.fetching = Some(PendingFetch {
            key: self.key(),
            request,
            cancel,
            rx,
        });
        self.fetched = None;
        self.fetch_search.clear();
        self.notice = None;
    }

    fn fetch_ui(&mut self, ui: &mut egui::Ui) -> Option<&'static str> {
        let mut copied = None;
        let unavailable = self.output().is_none_or(|output| {
            matches!(
                output.state,
                OutputState::NotRecorded
                    | OutputState::EmptyOrUnavailable
                    | OutputState::ProducerNotice
            ) || output.state == OutputState::Recorded && output.content.is_none()
        });
        if self.tab == Tab::Command || unavailable {
            let request = self.fetch_request();
            let label = if self.tab == Tab::Command {
                "Fetch command metadata (no inputs)"
            } else {
                "Inspect current cached result"
            };
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(cfg!(all(fbcode_build, unix)) && request.is_some() && self.fetching.is_none(), egui::Button::new(label))
                    .on_hover_text("Explicit RE request using this selection's recorded action digest. Does not download the action's input tree.")
                    .on_disabled_hover_text(if cfg!(all(fbcode_build, unix)) {
                        "Requires a recorded action digest and no fetch currently running."
                    } else {
                        "Remote retrieval requires Meta's internal frecli on Linux or macOS. Recorded commands and output remain available."
                    })
                    .clicked()
                {
                    if let Some(request) = request { self.start_fetch(request); }
                }
                if let Some(pending) = &self.fetching {
                    ui.spinner();
                    if ui.small_button("Cancel fetch").clicked() { pending.cancel.store(true, Ordering::Relaxed); }
                }
            });
            if self.tab != Tab::Command {
                ui.weak("The current cached result may not describe this recorded attempt; it is not recorded stdout/stderr.");
            }
        }
        let mut save = None;
        if let Some(fetched) = &self.fetched {
            if fetched.key == self.key() {
                ui.push_id("execution-fetched-metadata", |ui| {
                    ui.separator();
                    ui.strong(fetched.result.provenance.label());
                    ui.label(format!(
                        "{}: {}",
                        fetched.result.outcome.label(),
                        fetched.result.message
                    ));
                    if fetched.result.request.use_case.is_none() {
                        ui.weak("No RE use case was recorded; this request uses frecli's default.");
                    }
                    if fetched.result.lossy_utf8 {
                        ui.weak("Non-UTF-8 bytes were replaced in this metadata text.");
                    }
                    if !fetched.result.text.is_empty() {
                        preview_ui(
                            ui,
                            &fetched.preview,
                            &mut self.fetch_search,
                            self.tab,
                            false,
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Copy metadata").clicked() {
                                ui.ctx().copy_text(fetched.result.text.clone());
                                copied =
                                    Some("fetched metadata copied (not recorded command/output)");
                            }
                            if ui
                                .add_enabled(
                                    self.saving.is_none(),
                                    egui::Button::new("Save metadata…"),
                                )
                                .clicked()
                            {
                                save = Some(SavePayload::Metadata(fetched.result.text.clone()));
                            }
                        });
                    }
                    if !fetched.result.tool_stderr.is_empty() {
                        egui::CollapsingHeader::new("frecli diagnostics (not action stderr)")
                            .id_salt("execution-fetch-diagnostics")
                            .show(ui, |ui| {
                                egui::ScrollArea::both().max_height(120.0).show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&fetched.result.tool_stderr)
                                                .monospace(),
                                        )
                                        .wrap(),
                                    );
                                });
                                if fetched.result.tool_stderr_truncated {
                                    ui.weak("Tool diagnostics truncated.");
                                }
                            });
                    }
                    if !fetched.result.manual_argv.is_empty()
                        && ui.button("Copy manual fetch command").clicked()
                    {
                        ui.ctx()
                            .copy_text(crate::analyses::shlex_join(&fetched.result.manual_argv));
                        copied = Some("manual metadata-fetch command copied; nothing executed");
                    }
                });
            }
        }
        if let Some(payload) = save {
            self.start_save(payload);
        }
        copied
    }

    fn start_save(&mut self, payload: SavePayload) {
        let file_name = if matches!(&payload, SavePayload::Metadata(_)) {
            "retrace-fetched-metadata.txt".to_owned()
        } else {
            format!("retrace-{}.txt", self.tab.label().to_ascii_lowercase())
        };
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("retrace-save-evidence".into())
            .spawn(move || {
                let result = (|| {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("Save execution evidence")
                        .set_file_name(file_name)
                        .save_file()
                    else {
                        return Ok(None);
                    };
                    match payload {
                        SavePayload::Stored(reader, reference) => reader.copy_to(reference, &path),
                        SavePayload::Text(text) | SavePayload::Metadata(text) => {
                            execution::write_new_file(&path, |file| {
                                Ok(file.write_all(text.as_bytes())?)
                            })
                        }
                    }
                    .map_err(|error| format!("{}: {error:#}", path.display()))?;
                    Ok(Some(path))
                })();
                drop(tx.send(result));
            });
        match started {
            Ok(_) => {
                self.saving = Some(rx);
                self.notice = None;
            }
            Err(e) => self.notice = Some(format!("cannot start save worker: {e}")),
        }
    }
}

impl Drop for ExecutionUi {
    fn drop(&mut self) {
        if let Some(pending) = &self.fetching {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn accepts_fetch(
    current: PreviewKey,
    requested: PreviewKey,
    request: &FetchRequest,
    echoed: &FetchRequest,
) -> bool {
    current == requested && request == echoed
}

enum SavePayload {
    Text(String),
    Metadata(String),
    Stored(DetailReader, DetailRef),
}

fn initial_selection(loaded: Option<&LoadedAction>) -> Selection {
    match loaded {
        Some(loaded) if !loaded.attempts.is_empty() => {
            Selection::Attempt(loaded.attempts.len() - 1)
        }
        Some(loaded) if !loaded.observations.is_empty() => Selection::Observation(0),
        Some(_) => Selection::NoReport,
        _ => Selection::Legacy,
    }
}

fn attempt_label(attempt: &execution::Attempt) -> String {
    format!(
        "Attempt {} · {} · {}",
        u64::from(attempt.index) + 1,
        attempt.executor,
        attempt.status
    )
}

fn observation_label(observation: &execution::CommandObservation) -> String {
    format!(
        "Observed {} command · span {}",
        observation.executor, observation.span_id
    )
}

fn output_message(tab: Tab, state: OutputState) -> String {
    let name = tab.label();
    match state {
        OutputState::Recorded => format!("Recorded {name}"),
        OutputState::Empty => format!("Recorded {name} is empty."),
        OutputState::EmptyOrUnavailable => {
            format!("{name} is empty or unavailable; the event log does not distinguish these.")
        }
        OutputState::NotRecorded => format!("{name} was not recorded."),
        OutputState::ProducerNotice => {
            format!("Producer notice about {name}; not the complete stream.")
        }
    }
}

/// Exact shell-quoted prefix with bounded storage, without first allocating the
/// potentially large complete command. Copy formats the full argv on demand.
fn command_preview(argv: &[String]) -> TextPreview {
    let mut out = CappedText::default();
    for (i, word) in argv.iter().enumerate() {
        if i != 0 {
            out.push(" ");
        }
        let safe = !word.is_empty()
            && word
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
        if safe {
            out.push(word);
        } else {
            out.push("'");
            for (i, part) in word.split('\'').enumerate() {
                if i != 0 {
                    out.push("'\"'\"'");
                }
                out.push(part);
            }
            out.push("'");
        }
    }
    out.finish()
}

#[derive(Default)]
struct CappedText {
    text: String,
    total: u64,
    truncated: bool,
}

fn text_preview(text: &str) -> TextPreview {
    let mut preview = CappedText::default();
    preview.push(text);
    preview.finish()
}

impl CappedText {
    fn push(&mut self, text: &str) {
        self.total = self.total.saturating_add(text.len() as u64);
        if self.truncated {
            return;
        }
        let available = execution::PREVIEW_BYTES - self.text.len();
        let mut take = text.len().min(available);
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.text.push_str(&text[..take]);
        self.truncated = take < text.len();
    }

    fn finish(self) -> TextPreview {
        TextPreview {
            text: self.text,
            total_bytes: self.total,
            truncated: self.truncated,
        }
    }
}

fn preview_ui(
    ui: &mut egui::Ui,
    preview: &TextPreview,
    search: &mut String,
    tab: Tab,
    full_save: bool,
) {
    ui.horizontal(|ui| {
        ui.label("Find:");
        ui.add(
            egui::TextEdit::singleline(search)
                .hint_text("Filter preview lines (case-sensitive)")
                .char_limit(1024)
                .desired_width(ui.available_width()),
        );
    });
    let filtered;
    let text = if search.is_empty() {
        preview.text.as_str()
    } else {
        let lines: Vec<_> = preview
            .text
            .lines()
            .filter(|line| line.contains(search.as_str()))
            .collect();
        ui.weak(format!("{} matching line(s) in this preview", lines.len()));
        filtered = lines.join("\n");
        filtered.as_str()
    };
    if preview.truncated {
        ui.weak(format!(
            "Preview: {} of {} available bytes. Search covers only this prefix.",
            preview.text.len(),
            preview.total_bytes
        ));
        ui.weak(if full_save { "Save preserves the entire recorded value." }
            else { "Save retains only the metadata text retrieved for this request, which may itself be truncated." });
    }
    egui::ScrollArea::both()
        .id_salt(("execution-text", tab.index()))
        .max_height(180.0)
        .show(ui, |ui| {
            let mut text = text;
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6),
            );
        });
}

fn command_context(ui: &mut egui::Ui, command: &Command) {
    if !command.env.is_empty() {
        egui::CollapsingHeader::new(format!("Environment ({} recorded entries)", command.env.len()))
            .id_salt("execution-env").show(ui, |ui| {
                ui.weak("Explicit recorded values; inherited environment may be absent. Values can contain secrets.");
                let mut text = CappedText::default();
                for (name, value) in &command.env { text.push(name); text.push("="); text.push(value); text.push("\n"); }
                let text = text.finish();
                egui::ScrollArea::both().id_salt("execution-env-text").max_height(140.0).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&text.text).monospace()).wrap());
                });
                if text.truncated { ui.weak("Environment preview truncated."); }
            });
    }
    if !command.fallback_exe.is_empty() {
        egui::CollapsingHeader::new("Worker fallback executable")
            .id_salt("execution-fallback")
            .show(ui, |ui| {
                ui.weak("Recorded fallback configuration; not proof of which process executed.");
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(command_preview(&command.fallback_exe).text)
                            .monospace(),
                    )
                    .wrap(),
                );
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_preview_matches_copy_quoting_and_preserves_empty_arguments() {
        for words in [
            vec![],
            vec![""],
            vec!["cc", "a b", "a'b", "line\nbreak", "é", "-Dfoo=bar"],
            vec!["", ""],
        ] {
            let argv = words.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let preview = command_preview(&argv);
            assert_eq!(preview.text, crate::analyses::shlex_join(&argv));
            assert_eq!(preview.total_bytes, preview.text.len() as u64);
            assert!(!preview.truncated);
        }
    }

    #[test]
    fn preview_is_a_utf8_prefix_not_a_truncated_command_to_execute() {
        let argv = vec![
            "x".repeat(execution::PREVIEW_BYTES - 2),
            "é'tail".to_owned(),
            "last".to_owned(),
        ];
        let full = crate::analyses::shlex_join(&argv);
        let preview = command_preview(&argv);
        assert!(preview.truncated);
        assert!(preview.text.len() <= execution::PREVIEW_BYTES);
        assert!(full.starts_with(&preview.text));
        assert_eq!(preview.total_bytes, full.len() as u64);
    }

    #[test]
    fn empty_and_missing_output_have_distinct_user_messages() {
        let states = [
            OutputState::Recorded,
            OutputState::Empty,
            OutputState::EmptyOrUnavailable,
            OutputState::NotRecorded,
            OutputState::ProducerNotice,
        ];
        let messages = states
            .into_iter()
            .map(|state| output_message(Tab::Stdout, state))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(messages.len(), states.len());
        assert!(
            output_message(Tab::Stderr, OutputState::EmptyOrUnavailable)
                .contains("does not distinguish")
        );
        assert!(
            output_message(Tab::Stderr, OutputState::ProducerNotice)
                .contains("not the complete stream")
        );
    }

    #[test]
    fn large_error_preview_retains_a_bounded_prefix_and_exact_total() {
        let error = format!("important error: {}", "é".repeat(execution::PREVIEW_BYTES));
        let preview = text_preview(&error);
        assert!(preview.truncated);
        assert!(preview.text.len() <= execution::PREVIEW_BYTES);
        assert!(error.starts_with(&preview.text));
        assert!(preview.text.starts_with("important error: "));
        assert_eq!(preview.total_bytes, error.len() as u64);
    }

    #[test]
    fn fetch_results_require_exact_selection_tab_and_echoed_request() {
        let key = PreviewKey {
            selection: Selection::Attempt(1),
            tab: Tab::Command,
        };
        let request = FetchRequest {
            kind: FetchKind::ActionMetadata {
                action_digest: "abc:12".into(),
            },
            use_case: Some("test".into()),
        };
        assert!(accepts_fetch(key, key, &request, &request));
        assert!(!accepts_fetch(
            PreviewKey {
                selection: Selection::Attempt(0),
                ..key
            },
            key,
            &request,
            &request
        ));
        assert!(!accepts_fetch(
            PreviewKey {
                tab: Tab::Stdout,
                ..key
            },
            key,
            &request,
            &request
        ));
        let other = FetchRequest {
            use_case: None,
            ..request.clone()
        };
        assert!(!accepts_fetch(key, key, &request, &other));
        let other = FetchRequest {
            kind: FetchKind::CurrentCacheMetadata {
                action_digest: "abc:12".into(),
            },
            ..request.clone()
        };
        assert!(!accepts_fetch(key, key, &request, &other));
    }
}
