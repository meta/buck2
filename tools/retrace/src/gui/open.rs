/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The Open menu: getting a different build into the window.
//!
//! A build arrives named three different ways — a trace file someone
//! handed you, a build id out of a task or a paste, or one of the builds
//! this checkout just ran — so there are three ways in. All three end at
//! the same [`Request`], which the app turns into a fresh load.
//!
//! Nothing here loads anything itself. The menu only records what was
//! asked for; replacing the trace is the app's job, because the app is
//! what owns the `Ready` being replaced. That split is what frees the
//! previous build: assigning a new state drops the old `Ready` whole, and
//! with it the trace, the graph, the minimap texture and every cache
//! derived from them. No field has to be remembered and reset.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use super::Ready;
use crate::analyses;
use crate::ingest;

/// How many local logs the Recent builds submenu offers. buck2 keeps
/// fewer than this in most checkouts; the cap is so a log directory
/// nobody has cleaned cannot produce a menu taller than the screen.
const RECENT_LIMIT: usize = 20;

/// What the menu asked for.
pub enum Request {
    /// A trace JSON already on disk. Its graph sidecar is discovered
    /// beside it, the same way a command-line path is.
    Trace(PathBuf),
    /// A log, a build id, or the newest local log. Ingest resolves it
    /// and writes the trace/graph pair before the parse.
    ///
    /// The second field is the checkout to search for local logs. The
    /// menu resolves it and hands it over, because a window's working
    /// directory is not the checkout it is showing — see [`local_root`].
    Ingest(ingest::Source, PathBuf),
}

impl Request {
    /// What to call this load while it is running.
    pub fn label(&self) -> String {
        match self {
            Request::Trace(p) => p.display().to_string(),
            Request::Ingest(ingest::Source::Path(p), _) => p.display().to_string(),
            Request::Ingest(ingest::Source::TraceId(id), _) => id.clone(),
            Request::Ingest(ingest::Source::Last, _) => "the newest local command".to_owned(),
        }
    }
}

/// The build-id / path entry.
#[derive(Default)]
pub struct Prompt {
    pub text: String,
    /// Set for the one frame after opening, to put the caret in the box.
    pub focus: bool,
}

/// Everything the Open menu needs to remember between frames.
#[derive(Default)]
pub struct OpenState {
    /// A native file dialog running off the UI thread. The window keeps
    /// painting the current build while it is up, so cancelling costs
    /// nothing.
    pub picking: Option<mpsc::Receiver<Option<PathBuf>>>,
    pub prompt: Option<Prompt>,
    /// The local log listing, read once per menu open. The error is kept
    /// rather than dropped: "not inside a buck project" is the answer to
    /// why the list is empty, and it belongs where the list would be.
    recent: Option<Result<Vec<ingest::RecentLog>, String>>,
    /// Handed up to the app, which is the only thing that can act on it.
    pub req: Option<Request>,
    /// Why the last Open did not happen. Set by the app when a load
    /// fails, cleared when the window is dismissed.
    pub err: Option<String>,
}

/// The checkout whose logs the local-log menus offer, or `None` if there
/// is not one to offer.
///
/// The build on screen first, then the process's working directory. A
/// window is as likely to have been opened from a desktop icon as from a
/// shell inside a checkout, so the working directory is a last resort
/// rather than the answer: it is neither chosen by nor visible to the
/// person clicking, and reaching for it silently is what turns "Newest
/// local build" into a button that can only fail.
pub fn local_root(app: &Ready) -> Option<PathBuf> {
    std::path::Path::new(&app.tr.path)
        .parent()
        .and_then(ingest::project_root)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .as_deref()
                .and_then(ingest::project_root)
        })
}

/// The menu itself. Drawn inside `Open` in the menu bar.
pub fn menu(app: &mut Ready, ui: &mut egui::Ui) {
    let root = local_root(app);
    // the two local-log items cannot work without a checkout, so they say
    // so instead of offering a click whose only outcome is an error
    let no_root = "no buck project here \u{2014} this window was not opened from a checkout, and \
                   the build on screen is not inside one";
    if ui
        .button("Trace file\u{2026}")
        .on_hover_text("a retrace `-trace.json`; the `-graph.json` beside it is picked up too")
        .clicked()
    {
        start_pick(app);
        ui.close();
    }

    if ui
        .button("Command ID\u{2026}")
        .on_hover_text(
            "the uuid buck2 prints. Looked for in this checkout's logs \
             first, then downloaded",
        )
        .clicked()
    {
        app.open.prompt = Some(Prompt {
            focus: true,
            ..Default::default()
        });
        ui.close();
    }

    ui.separator();

    // built on the frame the submenu first opens: a directory listing
    // plus a stat per log is too much to repeat every frame, and too
    // little to be worth caching past the menu closing
    let sub = match &root {
        Some(root) => {
            let sub = ui.menu_button("Recent commands", |ui| {
                if app.open.recent.is_none() {
                    app.open.recent =
                        Some(ingest::recent_logs(RECENT_LIMIT, root).map_err(|e| format!("{e:#}")));
                }
                match &app.open.recent {
                    Some(Ok(logs)) if logs.is_empty() => {
                        ui.weak("no local logs \u{2014} run a Buck command first");
                    }
                    Some(Ok(logs)) => {
                        let rows: Vec<(String, PathBuf)> = logs
                            .iter()
                            .map(|l| (recent_label(l), l.path.clone()))
                            .collect();
                        for (label, path) in rows {
                            if ui.button(label).clicked() {
                                app.open.req =
                                    Some(Request::Ingest(ingest::Source::Path(path), root.clone()));
                                ui.close();
                            }
                        }
                    }
                    Some(Err(e)) => {
                        ui.weak(e.as_str());
                    }
                    None => {}
                }
            });
            Some(sub)
        }
        None => {
            ui.add_enabled(false, egui::Button::new("Recent commands"))
                .on_disabled_hover_text(no_root);
            None
        }
    };
    if sub.is_some_and(|s| s.response.clicked_elsewhere()) {
        app.open.recent = None;
    }

    match &root {
        Some(root) => {
            if ui
                .button("Newest local command")
                .on_hover_text("the log buck2 wrote last, whatever it was")
                .clicked()
            {
                app.open.req = Some(Request::Ingest(ingest::Source::Last, root.clone()));
                ui.close();
            }
        }
        None => {
            ui.add_enabled(false, egui::Button::new("Newest local command"))
                .on_disabled_hover_text(no_root);
        }
    }

    ui.separator();

    if ui
        .button("Reload this command")
        .on_hover_text("re-read the trace from disk")
        .clicked()
    {
        app.open.req = Some(Request::Trace(PathBuf::from(&app.tr.path)));
        ui.close();
    }
}

/// One line in the Recent builds submenu. The command and the time are
/// what tell two builds apart; the size is what warns that this one will
/// take a while.
fn recent_label(l: &ingest::RecentLog) -> String {
    let when = l
        .modified
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| jiff::Timestamp::from_second(d.as_secs() as i64).ok())
        .and_then(|ts| {
            jiff::fmt::strtime::format("%Y-%m-%d %H:%M", &ts.to_zoned(jiff::tz::TimeZone::system()))
                .ok()
        })
        .unwrap_or_else(|| "?".to_owned());
    let what = if l.command.is_empty() {
        l.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        l.command.clone()
    };
    format!("{when}  {what}  ({})", analyses::fmt_bytes(l.bytes as i64))
}

/// Open the OS file dialog on a worker thread.
///
/// Off the UI thread on purpose: the dialog is modal to whatever thread
/// runs it, and the window has a build on screen that should keep
/// repainting — including while someone browses for two minutes and then
/// cancels.
fn start_pick(app: &mut Ready) {
    let (tx, rx) = mpsc::channel();
    let start = std::path::Path::new(&app.tr.path)
        .parent()
        .map(|p| p.to_owned());
    std::thread::spawn(move || {
        let mut d = rfd::FileDialog::new()
            .set_title("Open a retrace trace")
            .add_filter("retrace trace", &["json"])
            .add_filter("any file", &["*"]);
        if let Some(p) = start {
            d = d.set_directory(p);
        }
        drop(tx.send(d.pick_file()));
    });
    app.open.picking = Some(rx);
}

/// Collect a finished file dialog and a submitted prompt. Called once
/// per frame, before the menu bar.
pub fn poll(app: &mut Ready) {
    let Some(rx) = &app.open.picking else {
        return;
    };
    match rx.try_recv() {
        Err(mpsc::TryRecvError::Empty) => {}
        // a picker that died leaves no path and no explanation worth
        // showing; the menu is still there to try again
        Err(mpsc::TryRecvError::Disconnected) => app.open.picking = None,
        Ok(picked) => {
            app.open.picking = None;
            match picked {
                Some(p) => app.open.req = Some(Request::Trace(p)),
                None => app.toast = Some(("open cancelled".to_owned(), Instant::now())),
            }
        }
    }
}

/// The build-id entry window.
pub fn prompt_window(app: &mut Ready, ctx: &egui::Context) {
    let Some(prompt) = &mut app.open.prompt else {
        return;
    };
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    egui::Window::new("Open command ID")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Command ID, event log path, or trace JSON path:");
            let te = ui.add(
                egui::TextEdit::singleline(&mut prompt.text)
                    .desired_width(460.0)
                    .hint_text("e.g. 8f1c0e2a-4b77-4a2c-9e31-2d5f0a6c1b83"),
            );
            if std::mem::take(&mut prompt.focus) {
                te.request_focus();
            }
            submit = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                submit |= ui.button("Open").clicked();
                cancel |= ui.button("Cancel").clicked();
            });
            ui.label(
                egui::RichText::new(
                    "A Command ID is looked for in this checkout's logs first, then downloaded. \
                     Completed cached conversions are reused. Commands without actions open a command profile.",
                )
                .size(10.5)
                .weak(),
            );
        });

    let text = app.open.prompt.as_ref().map_or("", |p| p.text.trim());
    if submit && !text.is_empty() {
        app.open.req = Some(classify(text, typed_root(app)));
        app.open.prompt = None;
    } else if cancel || !open {
        app.open.prompt = None;
    }
}

/// The window a failed Open puts its error in.
///
/// A failed load leaves the build that was on screen where it was, so
/// the error has nowhere else to go — and it is the kind of message (a
/// path, a reason, a suggestion) that has to stay on screen long enough
/// to be read and acted on.
pub fn error_window(app: &mut Ready, ctx: &egui::Context) {
    let Some(err) = app.open.err.clone() else {
        return;
    };
    let mut open = true;
    let mut dismiss = false;
    egui::Window::new("Open failed")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(egui::RichText::new(err).size(11.5));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("The command already open is untouched.")
                    .size(10.5)
                    .weak(),
            );
            dismiss |= ui.button("Dismiss").clicked();
        });
    if dismiss || !open {
        app.open.err = None;
    }
}

/// Where a typed build id looks for a local log before downloading it.
///
/// Unlike the local-log menus this has an answer even with no checkout
/// to search — the id is fetched from the log store instead — so it
/// takes the trace's own directory as a last resort rather than
/// disabling itself.
fn typed_root(app: &Ready) -> PathBuf {
    local_root(app).unwrap_or_else(|| {
        std::path::Path::new(&app.tr.path)
            .parent()
            .map_or_else(PathBuf::new, std::path::Path::to_owned)
    })
}

/// What the user typed, resolved to a way of loading it. A build id or a
/// log extension goes through ingest; anything else is taken as a trace
/// JSON already on disk, which is what the error message will say if it
/// is not one.
pub(super) fn classify(text: &str, root: PathBuf) -> Request {
    match ingest::classify_source(text) {
        Some(src) => Request::Ingest(src, root),
        None => Request::Trace(PathBuf::from(text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/tmp/checkout")
    }

    #[test]
    fn a_build_id_is_ingested_and_a_trace_path_is_not() {
        assert!(matches!(
            classify("8f1c0e2a-4b77-4a2c-9e31-2d5f0a6c1b83", root()),
            Request::Ingest(ingest::Source::TraceId(_), _)
        ));
        assert!(matches!(
            classify("/tmp/whatever-trace.json", root()),
            Request::Trace(_)
        ));
    }

    #[test]
    fn every_request_can_name_itself_while_it_loads() {
        // the spinner shows this string, so an empty one is a blank
        // screen with no clue what is happening
        for r in [
            Request::Trace(PathBuf::from("/tmp/x-trace.json")),
            Request::Ingest(ingest::Source::Last, root()),
            Request::Ingest(ingest::Source::TraceId("abc".to_owned()), root()),
            Request::Ingest(ingest::Source::Path(PathBuf::from("/tmp/l.pb.zst")), root()),
        ] {
            assert!(!r.label().is_empty());
        }
    }

    #[test]
    fn a_recent_log_labels_itself_even_with_nothing_to_go_on() {
        let bare = ingest::RecentLog {
            path: PathBuf::from("/tmp/odd-name.pb.zst"),
            trace_id: String::new(),
            command: String::new(),
            modified: None,
            bytes: 0,
        };
        let l = recent_label(&bare);
        assert!(l.contains("odd-name.pb.zst"), "{l}");

        let full = ingest::RecentLog {
            command: "build".to_owned(),
            trace_id: "8f1c0e2a-4b77-4a2c-9e31-2d5f0a6c1b83".to_owned(),
            modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_757_000_000)),
            bytes: 5 << 20,
            ..bare
        };
        let l = recent_label(&full);
        assert!(l.contains("build"), "{l}");
        assert!(l.starts_with("2025-"), "{l}");
    }
}
