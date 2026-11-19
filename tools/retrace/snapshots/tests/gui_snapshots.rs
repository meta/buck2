/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Headless GUI snapshots. Baselines live in tests/snapshots/;
//! validate through the portable Buck target:
//!   buck2 test fbcode//buck2/tools/retrace:gui_snapshots
//! For deliberate baseline updates, use the recipe in docs/validation.md.
//! The target supplies the selected platform's graphics libraries, fixture path
//! and GL backend. Do not copy an LD_LIBRARY_PATH from a different platform.
//! Cargo runs every semantic/interaction test on every OS. Linux/Mesa pixel
//! comparisons are enabled by Buck, or explicitly with RETRACE_PIXEL_SNAPSHOTS=1.
//! Pixel comparison is not portable to macOS/Windows renderer baselines.
//! After an approved update, delete the `.old.png` files kittest leaves beside the rewritten
//! baselines.
//!
//! These render through wgpu, not a CPU rasterizer. On a devserver wgpu
//! lands on Mesa's software GL (`libEGL_mesa.so.0` and `swrast_dri.so`),
//! there being no Vulkan loader installed; wgpu ranks Vulkan above GL, so a
//! host that has both would pick a different backend and shift pixels. The
//! buck target pins `WGPU_BACKEND=gl` and runs local-only for that reason.
//!
//! Analysis renders from synchronously computed state. Execution previews
//! settle their bounded local reader before a snapshot; no test fetches RE.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use egui_kittest::kittest::NodeT as _;
use egui_kittest::kittest::Queryable as _;
use retrace::analyses::fmt_when_utc;
use retrace::execution;
use retrace::gui;
use retrace::parse;
use retrace::testdata;

/// kittest creates its output directory and clears stale `.new.png` and
/// `.diff.png` files in it on every run, pass or fail, so that directory has
/// to be writable. Under buck the baselines arrive as a read-only resource
/// named by `RETRACE_SNAPSHOTS`, so they are copied once into a scratch
/// directory and the comparison runs against the copy; a failure leaves its
/// `.diff.png` beside them and names the path.
///
/// Without that variable the checked-in `snapshots/tests/snapshots` is used directly,
/// which is what lets `UPDATE_SNAPSHOTS` rewrite the baselines in place.
struct SnapshotScratch {
    path: PathBuf,
    retain: std::cell::Cell<bool>,
}

thread_local! {
    static SCRATCH: std::cell::OnceCell<SnapshotScratch> = const { std::cell::OnceCell::new() };
}

impl Drop for SnapshotScratch {
    fn drop(&mut self) {
        // Only this test thread's unique scratch, never the source baselines.
        if !self.retain.get() && !std::thread::panicking() {
            std::fs::remove_dir_all(&self.path).ok();
        }
    }
}

fn snapshot_options() -> egui_kittest::SnapshotOptions {
    let opts = egui_kittest::SnapshotOptions::default();
    let Ok(baselines) = std::env::var("RETRACE_SNAPSHOTS") else {
        return opts.output_path(
            PathBuf::from(option_env!("CARGO_MANIFEST_DIR").unwrap_or("."))
                .join("snapshots/tests/snapshots"),
        );
    };
    let dir = SCRATCH.with(|scratch| {
        scratch
            .get_or_init(|| {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let dir = std::env::temp_dir().join(format!(
                    "retrace-snapshots-{}-{epoch}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir(&dir).expect("create the unique scratch snapshot directory");
                for entry in
                    std::fs::read_dir(&baselines).expect("read the baseline resource directory")
                {
                    let src = entry.expect("baseline directory entry").path();
                    let name = src.file_name().expect("baseline file name");
                    std::fs::copy(&src, dir.join(name))
                        .expect("copy a baseline into the scratch directory");
                }
                SnapshotScratch {
                    path: dir,
                    retain: std::cell::Cell::new(false),
                }
            })
            .path
            .clone()
    });
    opts.output_path(dir)
}

fn snapshot<State>(h: &mut egui_kittest::Harness<'_, State>, name: &str) {
    let opted_in = match std::env::var("RETRACE_PIXEL_SNAPSHOTS").ok().as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        Some(_) => panic!("RETRACE_PIXEL_SNAPSHOTS must be 0 or 1"),
    };
    if !(cfg!(fbcode_build) || opted_in) {
        static NOTICE: std::sync::Once = std::sync::Once::new();
        NOTICE.call_once(|| eprintln!("GUI semantic assertions are enabled; Linux/Mesa pixel comparisons require RETRACE_PIXEL_SNAPSHOTS=1 outside Buck."));
        return;
    }
    match std::env::consts::OS {
        "linux" => {
            if let Err(failure) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.snapshot(name)))
            {
                SCRATCH.with(|scratch| {
                    if let Some(scratch) = scratch.get() {
                        scratch.retain.set(true);
                    }
                });
                std::panic::resume_unwind(failure);
            }
        }
        _ => panic!(
            "Pixel baselines require Linux/Mesa; run the portable semantic tests without RETRACE_PIXEL_SNAPSHOTS."
        ),
    }
}

/// A deterministic Ready over the smoke fixture: graph + DAG loaded
/// synchronously, analysis computed inline, embedded popups.
fn ready() -> gui::Ready {
    ready_over("smoke")
}

fn ready_over(stem: &str) -> gui::Ready {
    let mut tr = parse::parse(&testdata(&format!("{stem}-trace.json"))).unwrap();
    // the health panel prints these, and on a fixture this small they are
    // whatever the machine did in the sub-millisecond range
    tr.parse_ms = 7;
    tr.index_ms = 3;
    let g = retrace::graph::load_graph(&testdata(&format!("{stem}-graph.jsonl"))).unwrap();
    let run_id = Some("aaaa-bbbb".to_owned());
    let mut quality = retrace::quality::Quality::assess(&tr, run_id.as_deref(), Some(&g));
    let dag = retrace::dag::build_full(&tr, &g, run_id.as_deref()).map(|(b, st)| {
        quality = quality.stitched(&st);
        Arc::new(b)
    });
    let mut app = gui::Ready::new(tr, run_id.clone());
    if let Some(b) = &dag {
        let d = &b.dag;
        app.cp_list = d
            .cp_path
            .iter()
            .map(|&n| d.act_of_node[n as usize] as usize)
            .collect();
        app.cp_set = Some(app.cp_list.iter().copied().collect());
    }
    app.an = Some(Box::new(
        gui::compute_analysis(&app.tr, Some(&g), dag.as_deref(), run_id.as_deref())
            .expect("analysis"),
    ));
    app.gstate = gui::GState::Ready {
        g: Arc::new(gui::GraphSummary::new(
            &g,
            dag.as_ref()
                .map(|b| retrace::dag::chain_from_bundle(b, &Default::default())),
        )),
        dag,
        quality: Box::new(quality),
    };
    app.native_popups = false;
    app
}

fn harness(app: gui::Ready) -> egui_kittest::Harness<'static, gui::Ready> {
    harness_sized(app, egui::vec2(1450.0, 900.0))
}

fn harness_sized(app: gui::Ready, size: egui::Vec2) -> egui_kittest::Harness<'static, gui::Ready> {
    let mut h = egui_kittest::Harness::builder()
        .with_size(size)
        .with_options(snapshot_options())
        .build_ui_state(|ui, app| gui::update_ready(app, ui), app);
    // kittest builds its own context, so it does not pass through the
    // window's setup and would render arrows as the replacement square
    gui::install_fonts(&h.ctx);
    // a few frames: windows lay out on the second pass
    h.run();
    h
}

fn select_fixture_action(stem: &str, name: &str) -> gui::Ready {
    let mut app = ready_over(stem);
    let idx = app
        .tr
        .acts
        .iter()
        .position(|a| app.tr.names[a.name as usize] == name)
        .unwrap_or_else(|| panic!("{stem} fixture has no action named {name}"));
    app.select(idx);
    app
}

fn click_disclosure(h: &mut egui_kittest::Harness<'_, gui::Ready>, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label)
        .click();
    h.run();
    // A hover tooltip must not become part of a layout snapshot.
    h.event(egui::Event::PointerGone);
    h.run();
}

const STAGE_BODY_EXPLANATION: &str =
    "Stage totals are not event order. Counters can overlap; the remainder is not a measured wait.";

fn assert_compact_detail_order(h: &egui_kittest::Harness<'_, gui::Ready>) {
    let mut rows = [
        "Target",
        "Category",
        "Started",
        "Wall time",
        "Chronological activity",
        "Stage totals",
    ]
    .map(|label| (label, h.get_by_label(label).rect().center().y))
    .to_vec();
    rows.push((
        "stage totals bar",
        h.get_by_label("Stage totals — show stages")
            .rect()
            .center()
            .y,
    ));
    rows.push((
        "Stages",
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Stages")
            .rect()
            .center()
            .y,
    ));
    for label in ["Execution", "Graph & context"] {
        rows.push((label, h.get_by_label(label).rect().center().y));
    }
    for pair in rows.windows(2) {
        assert!(
            pair[0].1 < pair[1].1,
            "{} must precede {}: {rows:?}",
            pair[0].0,
            pair[1].0,
        );
    }
    assert!(h.query_by_label("Recorded client-clock activity").is_none());
    assert!(h.query_by_label("Observation").is_none());
    assert!(h.query_by_label("Copy RE diagnostics").is_none());
    assert!(h.query_by_label(STAGE_BODY_EXPLANATION).is_none());
}

/// The bare shell: menu bar, class legend, health chips.
#[test]
fn shell_smoke() {
    let app = ready();
    assert_eq!(app.n_events, 3);
    assert_eq!(app.tr.n_hits, 1);
    assert_eq!(app.an.as_ref().unwrap().stall.with_stages, 2);
    let mut h = harness(app);
    h.run();
    snapshot(&mut h, "shell_smoke");
}

#[test]
fn command_timeline_without_profile_does_not_claim_whole_command_duration() {
    let mut app = ready();
    assert_eq!(
        app.command_view.actions.map(|(start, end)| end - start),
        Some(900)
    );
    app.panels.command = true;
    let mut h = harness(app);
    h.run();
    h.get_by_label("Whole-command evidence is not available in this trace");
    h.get_by_label(&format!(
        "Recorded action window: {}. This is not the whole command.",
        retrace::humanize::dur(900.0)
    ));
}

#[test]
fn command_timeline_without_positions_has_no_fabricated_axis() {
    use retrace::command_profile::BuckCriticalPath;
    use retrace::command_profile::CommandProfile;
    use retrace::command_profile::CriticalPathEntry;
    let profile = CommandProfile {
        critical_path: Some(BuckCriticalPath {
            entries: vec![CriticalPathEntry {
                duration_us: Some(100),
                total_us: Some(100),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 900.0))
        .build_ui_state(
            |ui, (profile, state)| {
                gui::command_ui::contents(ui, profile, state, None);
            },
            (profile, gui::command_ui::State::default()),
        );
    gui::install_fonts(&h.ctx);
    h.run();
    h.get_by_label("Recorded window unavailable");
    h.get_by_label(
        "No positioned path entries were recorded; browse the path table for model weights.",
    );
    assert!(
        h.query_by_label(&format!("0 → {}", retrace::humanize::dur(1.0)))
            .is_none()
    );
}

#[test]
fn command_timeline_discloses_cropped_observations_and_unpositioned_phases() {
    use retrace::command_profile::CommandProfile;
    use retrace::command_profile::ObservedInterval;
    use retrace::command_profile::PhaseSummary;
    let profile = CommandProfile {
        lifecycle: vec![ObservedInterval {
            kind: "FileWatcher".into(),
            start_offset_us: -10,
            duration_us: 20,
            ..Default::default()
        }],
        phases: vec![PhaseSummary {
            kind: "Analysis".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 900.0))
        .build_ui_state(
            |ui, (profile, state)| {
                gui::command_ui::contents(ui, profile, state, None);
            },
            (profile, gui::command_ui::State::default()),
        );
    gui::install_fonts(&h.ctx);
    h.run();
    h.get_by_label("Some observations predate the recorded origin. They are cropped at zero in this chart; their original offsets remain in the evidence tables.");
    assert!(h.state().0.phases[0].union_intervals.is_empty());
}

fn command_only_ready(failed: bool) -> gui::Ready {
    let mut tr = parse::parse(&testdata("command-only-trace.json")).unwrap();
    assert!(tr.acts.is_empty());
    let run = &mut tr.runs[0];
    if failed {
        run.outcome = retrace::types::Outcome::Failed;
        run.argv = Some("buck2 uquery fbcode//demo:target unused-argument".into());
        let p = run.command_profile.as_mut().unwrap();
        p.command_kind = Some("uquery".into());
        p.result_outcome = Some("failed".into());
        p.result_error =
            Some("Query args supplied without any `%s` placeholder in the query".into());
        p.phases.clear();
        p.snapshots.clear();
        p.lifecycle.clear();
        p.command_start_offset_us = Some(0);
        p.command_end_offset_us = Some(4189);
    }
    let id = run.run.clone();
    let mut app = gui::Ready::new(tr, Some(id));
    app.native_popups = false;
    app
}

#[test]
fn command_only_profile_is_main_content_without_action_controls() {
    let mut h = command_only_harness(false);
    h.get_by_label("Command profile");
    if cfg!(fbcode_build) {
        h.get_by_label("Open build page");
    } else {
        assert!(
            h.query_by_label("Open build page").is_none(),
            "the open-source build has no built-in build page"
        );
    }
    assert!(!h.state().panels.command);
    assert!(h.query_by_label("Build timeline").is_none());
    assert!(h.query_by_label("Action window").is_none());
    snapshot(&mut h, "command_only_profile");
    h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Analysis")
        .into_iter()
        .max_by(|a, b| a.rect().center().y.total_cmp(&b.rect().center().y))
        .unwrap()
        .click();
    h.run();
    assert!(h.query_by_label("Longest recorded examples").is_some());
    h.get_by_label("Copy command ID").click();
    h.run();
    h.get_by_label("View").click();
    h.run();
    h.query_by_label("Parts of the window, and keys  ?")
        .or_else(|| h.query_by_label("Keyboard shortcuts"))
        .unwrap()
        .click();
    h.run();
    assert!(h.state().panels.help);
}

#[test]
fn command_only_failure_shows_recorded_diagnostic() {
    let mut h = command_only_harness(true);
    h.get_by_label("Recorded command diagnostic");
    h.get_by_label("Query args supplied without any `%s` placeholder in the query");
    h.get_by_label("Copy diagnostic").click();
    h.run();
    h.event(egui::Event::PointerGone);
    h.run();
    snapshot(&mut h, "command_only_failure");
}

fn command_only_harness(failed: bool) -> egui_kittest::Harness<'static, gui::Ready> {
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1450.0, 900.0))
        .with_options(snapshot_options())
        .build_ui_state(
            |ui, app| gui::update_ready(app, ui),
            command_only_ready(failed),
        );
    gui::install_fonts(&h.ctx);
    h.run();
    h
}

fn command_profile_example() -> retrace::command_profile::CommandProfile {
    use retrace::command_profile::*;
    let span = |kind: &str, start, dur, name: &str| ObservedInterval {
        kind: kind.into(),
        label: Some(name.into()),
        start_offset_us: start,
        duration_us: dur,
        ..Default::default()
    };
    let mut profile = CommandProfile {
        origin_us: 1_000_000,
        command_start_offset_us: Some(3_000_000),
        command_end_offset_us: Some(181_500_000),
        command_kind: Some("install".into()),
        result_outcome: Some("failed".into()),
        ..Default::default()
    };
    for (kind, a, b, count) in [
        ("LoadPackage", 4_000_000, 75_000_000, 9200),
        ("Analysis", 5_000_000, 172_000_000, 329738),
        ("Materialization", 72_000_000, 176_000_000, 12000),
    ] {
        profile.phases.push(PhaseSummary {
            kind: kind.into(),
            count,
            completed: count,
            total_duration_us: (b - a) * 3,
            union_intervals: vec![[a, b]],
            union_complete: true,
            top: vec![span(kind, a, 2_400_000, "fb4a · sample recorded target")],
            ..Default::default()
        });
    }
    profile.lifecycle = vec![
        span(
            "FileWatcher",
            3_000_000,
            1_000_000,
            "synchronize source changes",
        ),
        span(
            "InstallEventInfo",
            160_000_000,
            21_000_000,
            "fb4a · file-ready RPC",
        ),
    ];
    profile.critical_path = Some(BuckCriticalPath {
        entries: vec![
            CriticalPathEntry {
                kind: "GenericEntry".into(),
                label: "buckd_command_init".into(),
                start_offset_us: Some(0),
                duration_us: Some(1_000_000),
                total_us: Some(1_000_000),
                ..Default::default()
            },
            CriticalPathEntry {
                kind: "Analysis".into(),
                label: "fb4a analysis".into(),
                start_offset_us: Some(5_000_000),
                duration_us: Some(35_000_000),
                total_us: Some(40_000_000),
                ..Default::default()
            },
            CriticalPathEntry {
                kind: "ActionExecution".into(),
                label: "fb4a cached artifacts".into(),
                start_offset_us: Some(80_000_000),
                duration_us: Some(50_500_000),
                total_us: Some(50_500_000),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    profile.snapshots = (0..=18)
        .map(|i| SnapshotSample {
            offset_us: i * 10_000_000,
            cpu_us: Some(i as u64 * 240_000_000),
            rss_bytes: Some((4 + i as u64 * 3) << 30),
            download_bytes: Some((i as u64) << 29),
            ..Default::default()
        })
        .collect();
    profile
}

#[test]
fn command_timeline_and_phase_selection() {
    use retrace::gui::command_ui;
    let state = (
        command_profile_example(),
        command_ui::State {
            actions: Some((69_400_000, 176_200_000)),
            ..Default::default()
        },
    );
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 900.0))
        .with_options(snapshot_options())
        .build_ui_state(
            |ui, (profile, state)| {
                command_ui::contents(ui, profile, state, Some(50_492_169));
            },
            state,
        );
    gui::install_fonts(&h.ctx);
    h.run();
    snapshot(&mut h, "command_timeline");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Analysis")
        .click();
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Phase(1));
    assert!(h.query_by_label("Longest recorded examples").is_some());
    h.event(egui::Event::PointerGone);
    h.run();
    snapshot(&mut h, "command_analysis_phase");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "File watcher")
        .click();
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Lifecycle(0));
    h.get_by_label("synchronize source changes");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Installer file-ready RPCs")
        .click();
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Lifecycle(1));
    h.get_by_label("This is an installer file-ready RPC, not total device installation time. Artifact production and installer activity may overlap.");
}

#[test]
fn command_path_inspection_keeps_clock_scope() {
    use retrace::gui::command_ui;
    let state = (
        command_profile_example(),
        command_ui::State {
            path_table: true,
            ..Default::default()
        },
    );
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 1100.0))
        .with_options(snapshot_options())
        .build_ui_state(
            |ui, (profile, state)| {
                command_ui::contents(ui, profile, state, None);
            },
            state,
        );
    gui::install_fonts(&h.ctx);
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Command);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Next step")
        .click();
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Entry(0));
    h.get_by_label("buckd_command_init");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Next step")
        .click();
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Entry(1));
    assert!(
        h.query_by_label("Reported critical path · Buck command clock")
            .is_some()
    );
    assert!(h.query_by_label("Path cost").is_some());
    h.event(egui::Event::PointerGone);
    h.run();
    snapshot(&mut h, "command_reported_path");
}

#[test]
fn incomplete_command_keeps_cancellation_partial_coverage_and_missing_end() {
    use retrace::command_profile::*;
    use retrace::gui::command_ui;
    let profile = CommandProfile {
        origin_us: 1_000_000,
        command_start_offset_us: Some(2_000_000),
        lifecycle: vec![ObservedInterval {
            kind: "FileWatcher".into(),
            label: Some("watcher canceled while synchronizing changes".into()),
            start_offset_us: 3_000_000,
            duration_us: 2_000_000,
            cancelled: true,
        }],
        phases: vec![PhaseSummary {
            kind: "Analysis".into(),
            count: 3,
            completed: 1,
            open_at_eof: 2,
            total_duration_us: 1_000_000,
            union_intervals: vec![[6_000_000, 7_000_000]],
            union_complete: false,
            top: vec![ObservedInterval {
                kind: "Analysis".into(),
                label: Some("one completed analysis; two starts remain open".into()),
                start_offset_us: 6_000_000,
                duration_us: 1_000_000,
                ..Default::default()
            }],
            ..Default::default()
        }],
        snapshots: vec![
            SnapshotSample {
                offset_us: 0,
                cpu_us: Some(0),
                ..Default::default()
            },
            SnapshotSample {
                offset_us: 10_000_000,
                cpu_us: Some(10_000_000),
                ..Default::default()
            },
        ],
        warnings: vec![
            "Only a readable prefix was retained; open spans have no invented end.".into(),
        ],
        ..Default::default()
    };
    profile.validate().unwrap();
    assert_eq!(
        command_ui::extent(&profile, None),
        10_000_000,
        "the latest snapshot is later than any completed interval"
    );
    let marker_only = CommandProfile {
        origin_us: 1_000_000,
        command_start_offset_us: Some(2_000_000),
        ..Default::default()
    };
    assert_eq!(command_ui::extent(&marker_only, None), 2_000_000);
    assert_eq!(profile.phases[0].active_us(), None);
    assert_eq!(profile.phases[0].retained_active_us(), Some(1_000_000));
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 950.0))
        .with_options(snapshot_options())
        .build_ui_state(
            |ui, (profile, state)| {
                command_ui::contents(ui, profile, state, None);
            },
            (profile, command_ui::State::default()),
        );
    gui::install_fonts(&h.ctx);
    h.run();
    h.get_by_label("Command end was not recorded. The chart ends at the latest retained observation, not a claimed command completion.");
    assert!(h.query_by_label("Command end marker").is_none());
    assert!(
        h.query_by_label("Reported critical path · Buck command clock")
            .is_none()
    );
    assert!(h.query_by_label("Buck reported path").is_none());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "File watcher")
        .click();
    h.run();
    assert_eq!(h.state().1.selection, command_ui::Selection::Lifecycle(0));
    h.get_by_label("Canceled");
    h.get_by_label("watcher canceled while synchronizing changes");
    h.event(egui::Event::PointerGone);
    h.run();
    snapshot(&mut h, "command_incomplete_lifecycle");

    // The label carries a partial-coverage indicator; select this model row
    // directly so the assertion is about evidence, not indicator wording.
    h.state_mut().1.selection = command_ui::Selection::Phase(0);
    h.run();
    h.get_by_label(&format!(
        "at least {} (partial interval coverage)",
        retrace::humanize::dur(1_000_000.0)
    ));
    snapshot(&mut h, "command_incomplete_phase");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Browse reported path")
        .click();
    h.run();
    assert!(
        !h.state().1.path_table,
        "missing path control must remain disabled"
    );
    assert!(h.query_by_label("Path cost").is_none());
}

#[test]
fn command_timeline_is_visible_inside_the_actual_shell() {
    use retrace::command_profile::*;
    let mut app = ready();
    let profile = CommandProfile {
        origin_us: 1_000_000,
        run: app.run_id.clone(),
        command_start_offset_us: Some(0),
        command_end_offset_us: Some(1500),
        command_kind: Some("build".into()),
        result_outcome: Some("failed".into()),
        lifecycle: vec![ObservedInterval {
            kind: "FileWatcher".into(),
            start_offset_us: 0,
            duration_us: 100,
            label: Some("synchronize fixture inputs".into()),
            ..Default::default()
        }],
        phases: vec![PhaseSummary {
            kind: "Analysis".into(),
            count: 1,
            completed: 1,
            total_duration_us: 200,
            union_intervals: vec![[50, 250]],
            union_complete: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.validate().unwrap();
    Arc::get_mut(&mut app.tr)
        .expect("fixture trace is uniquely owned")
        .runs[0]
        .command_profile = Some(profile);
    app.command_view = gui::command_ui::State::new(&app.tr, app.run_id.as_deref());
    app.panels.command = true;
    let mut h = harness(app);
    h.get_by_label("Build timeline — whole command");
    h.get_by_label("Where the command spent time");
    assert!(
        h.query_by_label("Whole-command evidence is not available in this trace")
            .is_none()
    );
    assert!(
        h.query_by_label("Reported critical path · Buck command clock")
            .is_none()
    );
    snapshot(&mut h, "command_timeline_shell");
}

#[test]
fn analysis_menu_order_and_command_timeline_is_opt_in() {
    let mut app = ready();
    Arc::get_mut(&mut app.tr).unwrap().runs[0].command_profile = Some(command_profile_example());
    let mut loaded = gui::Ready::new(Arc::try_unwrap(app.tr).ok().unwrap(), app.run_id);
    loaded.gstate = app.gstate;
    loaded.cp_list = app.cp_list;
    loaded.an = app.an;
    loaded.native_popups = false;
    assert!(!loaded.panels.command);
    let mut h = harness(loaded);
    assert!(h.query_by_label("Where the command spent time").is_none());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Analysis")
        .click();
    h.run();
    let y = ["Build Overview", "Critical Path", "Build Timeline"]
        .map(|label| h.get_by_label(label).rect().center().y);
    assert!(y[0] < y[1] && y[1] < y[2]);
    assert!(h.query_by_label("Greedy CP cascade").is_none());
    h.get_by_label("Build Timeline").click();
    h.run();
    assert!(h.state().panels.command);
    h.get_by_label("Where the command spent time");
}

#[test]
fn timeline_canvas() {
    // no windows: the ruler, concurrency strip, gutter, bars with cache
    // outlines, minimap, scrollbars
    let mut h = harness(ready());
    h.run();
    assert!(h.state().painted > 0);
    assert!(h.state().minimap.is_some());
    assert!(!h.state().conc.is_empty());
    assert!(h.state().canvas_w > 0.0 && h.state().canvas_h > 0.0);
    snapshot(&mut h, "timeline_canvas");
}

#[test]
fn ranked_findings_disclose_that_scenario_overrides_are_not_applied() {
    let mut app = ready();
    app.panels.findings = true;
    app.cp_overrides.push(gui::CpOverride::Node {
        act: 0,
        w_us: 0,
        label: "removed".to_owned(),
    });
    let mut h = harness(app);
    h.run();
    h.get_by_label("Findings use recorded durations; what-if overrides are not applied.");
    h.get_by_label("▲ 1 what-if override(s) are active; these findings ignore them and may list actions your scenario already removed.");
}

#[test]
fn escape_closes_the_timeline_context_menu() {
    let mut h = harness(ready());
    let pos = egui::pos2(100.0, 150.0);
    let menu = gui::widgets::MenuAt::open(&h.ctx, pos);
    h.state_mut().ctx_menu = Some((0, menu));
    h.run();
    assert!(h.state().ctx_menu.is_some());
    h.event(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    });
    h.run();
    assert!(h.state().ctx_menu.is_none());
}

fn right_click_at(h: &mut egui_kittest::Harness<'_, gui::Ready>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
    }
    // Each queued event receives a frame. Do not settle animations while the
    // button is held: the default 250ms steps can exceed egui's click timeout.
    h.run();
}

#[test]
fn overview_context_menu_resets_view_without_clearing_filter_or_selection() {
    let mut app = ready();
    app.upp /= 4.0;
    app.row_h = gui::canvas::ROW_H_MAX;
    app.filter = "rustc".into();
    app.apply_filter();
    app.selected = Some(0);
    let mut h = harness(app);
    let before = (h.state().t_left, h.state().upp, h.state().row_h);
    let overview = h.get_by_label("Timeline overview").rect();
    right_click_at(&mut h, overview.center());
    assert_eq!(
        (h.state().t_left, h.state().upp, h.state().row_h),
        before,
        "opening the menu must not recenter or zoom the view",
    );
    for label in [
        "Time zoom in",
        "Time zoom out",
        "Lane zoom in",
        "Lane zoom out",
    ] {
        h.get_by_label(label);
    }
    h.event(egui::Event::PointerGone);
    h.run();
    snapshot(&mut h, "overview_zoom_menu");
    h.get_by_label("Reset view").click();
    h.run();
    let app = h.state();
    assert_eq!(app.t_left, 0.0);
    assert!((app.upp * f64::from(app.canvas_w) - app.tr.span as f64).abs() < 0.01);
    assert_eq!(app.scroll, 0.0);
    assert_eq!(
        app.row_h,
        retrace::config::get().appearance.row_height.unwrap_or(11.0)
    );
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.filter, "rustc");
}

#[test]
fn shell_with_overview() {
    let mut app = ready();
    assert_eq!(
        app.an
            .as_ref()
            .unwrap()
            .breakdown
            .iter()
            .map(|r| r.count)
            .sum::<i64>(),
        3
    );
    assert_eq!(
        app.an
            .as_ref()
            .unwrap()
            .breakdown
            .iter()
            .map(|r| r.total_us)
            .sum::<i64>(),
        1600
    );
    // the overview must not depend on the machine: not on its zone
    // (Date), and not on where the fixtures happen to live (Trace file)
    if let Some(an) = &mut app.an {
        for r in &mut an.ov_rows {
            match r.key {
                "Date" => r.value = fmt_when_utc(1_000_000),
                "Trace file" => r.value = "<fixtures>/smoke-trace.json".to_owned(),
                _ => {}
            }
        }
    }
    app.panels.overview = true;
    let action_text = app
        .an
        .as_ref()
        .unwrap()
        .ov_rows
        .iter()
        .find(|r| r.key == "Actions run")
        .unwrap()
        .value
        .clone();
    let mut h = harness(app);
    h.run();
    h.get_by_label(&action_text);
    let window = h
        .ctx
        .memory(|memory| memory.area_rect(egui::Id::new(("popup", "Build overview"))))
        .expect("overview window");
    assert!(
        window.contains_rect(h.get_by_label("worst wait ▸").rect()),
        "queue navigation must be visible on first open"
    );
    snapshot(&mut h, "shell_overview");
}

#[test]
fn tables_breakdown_peers_stall() {
    let mut app = ready();
    let an = app.an.as_ref().unwrap();
    assert_eq!(an.stall.nacts, 3);
    assert_eq!(an.stall.with_stages, 2);
    assert_eq!(an.stall.total_dur, 1600);
    assert_eq!(
        an.breakdown
            .iter()
            .find(|r| r.ty == "binary")
            .unwrap()
            .total_us,
        800
    );
    app.panels.breakdown = true;
    app.panels.peers = true;
    app.panels.stall = true;
    let mut h = harness(app);
    h.run();
    snapshot(&mut h, "tables_breakdown_peers_stall");
}

#[test]
fn health_window() {
    let mut app = ready();
    assert_eq!(app.n_events, 3);
    assert_eq!(app.tr.n_failed, 1);
    app.panels.health = true;
    let mut h = harness(app);
    h.run();
    snapshot(&mut h, "health");
}

#[test]
fn detail_popup_and_cp_walker() {
    // the cache-hit link action's detail popup (stable fixture index:
    // find by name) plus the CP walker
    let mut app = ready();
    let idx = (0..app.tr.acts.len())
        .find(|&i| app.tr.names[app.tr.acts[i].name as usize] == "z (cxx_link)")
        .unwrap();
    app.select(idx);
    app.panels.cp = true;
    let mut h = harness(app);
    h.run();
    snapshot(&mut h, "detail_and_walker");
}

/// Schematic client-state fixtures, not additional production measurements.
/// Expand the optional chronological view to show cancellation overlapping
/// local work; the default summary keeps the totals bar immediately available.
#[test]
fn detail_observed_remote_local_overlap() {
    let app = select_fixture_action("activity", "fallback (rustc)");
    let d = app.detail.as_ref().unwrap();
    assert_eq!(d.activity.uncovered_us, Some(1_000));
    assert!(d.activity.spans.iter().any(|s| s.cancelled));
    assert!(d.activity.spans.iter().any(|remote| {
        remote.cancelled
            && d.activity.spans.iter().any(|local| {
                local.lane == retrace::activity::Lane::Local
                    && local.start_us < remote.end_us
                    && remote.start_us < local.end_us
            })
    }));
    assert!(d.diagnostics.contains("reSessionID-fixture"));
    let mut h = harness(app);
    assert_compact_detail_order(&h);
    click_disclosure(&mut h, "Chronological activity");
    h.get_by_label("Recorded client-clock activity");
    h.get_by_label("Observation");
    assert!(h.query_all_by_label("Canceled").next().is_some());
    h.get_by_label("Stage totals — show stages");
    snapshot(&mut h, "detail_observed_overlap");
}

#[test]
fn detail_input_preparation_is_not_network_speed() {
    let app = select_fixture_action("activity", "input_setup (cxx_compile)");
    let d = app.detail.as_ref().unwrap();
    assert_eq!(
        d.activity
            .spans
            .iter()
            .filter(|s| s.name == "Re.WorkerDownload")
            .count(),
        1
    );
    let mut h = harness(app);
    assert_compact_detail_order(&h);
    snapshot(&mut h, "detail_input_preparation");
}

/// Activating the totals bar opens the numeric detail; it is not a second
/// toggle, and a retained selection must not force a closed disclosure open.
#[test]
fn detail_stage_totals_click_and_disclosure_toggle() {
    let mut h = harness(select_fixture_action(
        "activity",
        "input_setup (cxx_compile)",
    ));
    assert_compact_detail_order(&h);
    for _ in 0..2 {
        h.get_by_label("Stage totals — show stages").click();
        h.run();
        h.event(egui::Event::PointerGone);
        h.run();
        h.get_by_label(STAGE_BODY_EXPLANATION);
        assert!(h.query_by_label("Recorded client-clock activity").is_none());
    }
    // The input-preparation segment dominates this schematic action. A click
    // near the bar center should visibly highlight that selected stage row.
    snapshot(&mut h, "detail_stage_totals_expanded");
    click_disclosure(&mut h, "Stages");
    h.run();
    assert!(h.query_by_label(STAGE_BODY_EXPLANATION).is_none());
    h.get_by_label("Stage totals — show stages");
    click_disclosure(&mut h, "Stages");
    h.get_by_label(STAGE_BODY_EXPLANATION);
    click_disclosure(&mut h, "Chronological activity");
    h.get_by_label("Recorded client-clock activity");
    // Opening another action starts with the compact view, independently of
    // the previous action's expanded sections and selected stage.
    let next = {
        let app = h.state();
        app.tr
            .acts
            .iter()
            .position(|a| app.tr.names[a.name as usize] == "fallback (rustc)")
            .unwrap()
    };
    h.state_mut().select(next);
    h.run();
    assert_compact_detail_order(&h);
}

#[test]
fn detail_legacy_timing_unavailable() {
    let app = select_fixture_action("smoke", "y (rustc emit)");
    let d = app.detail.as_ref().unwrap();
    assert_eq!(d.activity.evidence, retrace::activity::Evidence::Missing);
    assert!(d.activity_rows.is_empty());
    assert!(d.activity.spans.is_empty());
    assert!(d.activity.gaps.is_empty());
    assert_eq!(d.activity.covered_us, None);
    assert_eq!(d.activity.uncovered_us, None);
    let mut h = harness(app);
    assert_compact_detail_order(&h);
    click_disclosure(&mut h, "Chronological activity");
    h.get_by_label("Copy RE diagnostics");
    h.get_by_label("Stage totals — show stages");
    assert!(h.query_by_label("Observation").is_none());
    assert!(
        h.query_by_label_contains("Time since this action started")
            .is_none(),
        "duration-only totals must not fabricate a chronological axis",
    );
    snapshot(&mut h, "detail_legacy_timing_unavailable");
}

const EXECUTION_RUN: &str = "06331c48-75f3-446b-8874-dc0769ab836b";
const EXECUTION_CAPTURE: &str = "retrace-v1-1111111111111111111111111111111111111111";
const EXECUTION_COMMAND: &str =
    "fixture-cc --input 'path with spaces.cc' '--define=quoted'\"'\"'value'";
const EXECUTION_STDOUT: &str = "captured stdout\nfixture stdout only\n";
const EXECUTION_STDERR_HEAD: &str = "captured stderr (ANSI stays text): \u{1b}[31merror\u{1b}[0m\n";

/// A tiny real store/trace pair, built through the public writer rather than
/// injecting private GUI state. Only these two owned files are cleaned up.
struct ExecutionFixture {
    directory: PathBuf,
    trace: PathBuf,
    store: PathBuf,
    stderr_bytes: u64,
}

impl ExecutionFixture {
    fn create() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "retrace-execution-snapshot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        #[cfg_attr(
            not(unix),
            expect(unused_mut, reason = "only Unix sets the directory mode")
        )]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(&directory).unwrap();
        let mut fixture = Self {
            trace: directory.join("execution-trace.json"),
            store: execution::store_path(&directory, EXECUTION_RUN).unwrap(),
            directory,
            stderr_bytes: 0,
        };
        let mut writer = execution::DetailWriter::create(&fixture.store).unwrap();
        let stdout = writer.append_text(EXECUTION_STDOUT).unwrap();
        let mut stderr = EXECUTION_STDERR_HEAD.to_owned();
        while stderr.len() <= execution::PREVIEW_BYTES {
            stderr.push_str("stderr detail line\n");
        }
        stderr.push_str("TAIL_AFTER_PREVIEW_MUST_NOT_BE_DISPLAYED\n");
        let stderr = writer.append_text(&stderr).unwrap();
        fixture.stderr_bytes = stderr.len;
        let failed = writer
            .append(&execution::Record::Attempt(execution::Attempt {
                index: 0,
                status: "Failure".into(),
                executor: "local".into(),
                command: Some(execution::Command {
                    argv: vec!["failed-fixture-cc".into()],
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .unwrap();
        let success = writer
            .append(&execution::Record::Attempt(execution::Attempt {
                index: 1,
                status: "Success".into(),
                executor: "local".into(),
                // The button is present, but even an accidental auto-fetch
                // must be refused locally instead of reaching a real service.
                action_digest: Some("not-a-real-cas-digest".into()),
                use_case: Some("fixture-only".into()),
                command: Some(execution::Command {
                    argv: vec![
                        "fixture-cc".into(),
                        "--input".into(),
                        "path with spaces.cc".into(),
                        "--define=quoted'value".into(),
                    ],
                    env: vec![("FIXTURE_MODE".into(), "recorded-only".into())],
                    ..Default::default()
                }),
                stdout: execution::Output {
                    state: execution::OutputState::Recorded,
                    content: Some(stdout),
                    note: None,
                },
                stderr: execution::Output {
                    state: execution::OutputState::Recorded,
                    content: Some(stderr),
                    note: None,
                },
                ..Default::default()
            }))
            .unwrap();
        let first = writer
            .append(&execution::Record::Action(execution::ActionManifest {
                span_id: 42,
                reports: vec![failed, success],
                observations: vec![],
            }))
            .unwrap();
        let other = writer
            .append(&execution::Record::Attempt(execution::Attempt {
                index: 0,
                status: "Success".into(),
                executor: "local".into(),
                command: Some(execution::Command {
                    argv: vec!["fixture-gen".into(), "--second-action".into()],
                    ..Default::default()
                }),
                stdout: execution::Output {
                    state: execution::OutputState::Empty,
                    ..Default::default()
                },
                ..Default::default()
            }))
            .unwrap();
        let second = writer
            .append(&execution::Record::Action(execution::ActionManifest {
                span_id: 43,
                reports: vec![other],
                observations: vec![],
            }))
            .unwrap();
        let info = writer.finish().unwrap();
        execution::stamp(&fixture.store, EXECUTION_RUN, EXECUTION_CAPTURE, &info).unwrap();
        let trace = format!(
            r#"[
{{"ph":"M","cat":"run","name":"run_start","ts":1000000,"pid":1,"args":{{"v":1,"run":"{EXECUTION_RUN}","capture":"{EXECUTION_CAPTURE}","execution_store":{{"sha1":"{sha1}","bytes":{bytes}}},"tool":"buck2","cwd":"/fixture","argv":["buck2","build","fbcode//fixture:one"]}}}},
{{"ph":"X","cat":"recipe","name":"command-one (genrule)","ts":1100000,"dur":1000000,"pid":1,"tid":0,"args":{{"v":1,"run":"{EXECUTION_RUN}","span_id":42,"target":"fbcode//fixture:one","src":"genrule","kind":"local","exit":0,"sig":0,"dir":"/fixture","stages":{{"execute_us":900000}},"execution":{{"schema":1,"reference":{{"offset":{first_offset},"len":{first_len}}}}}}}}},
{{"ph":"X","cat":"recipe","name":"command-two (genrule)","ts":2200000,"dur":500000,"pid":1,"tid":0,"args":{{"v":1,"run":"{EXECUTION_RUN}","span_id":43,"target":"fbcode//fixture:two","src":"genrule","kind":"local","exit":0,"sig":0,"dir":"/fixture","stages":{{"execute_us":450000}},"execution":{{"schema":1,"reference":{{"offset":{second_offset},"len":{second_len}}}}}}}}},
{{"ph":"M","cat":"run","name":"run_end","ts":3000000,"pid":1,"args":{{"v":1,"run":"{EXECUTION_RUN}","exit":0}}}}
]
"#,
            sha1 = info.sha1,
            bytes = info.bytes,
            first_offset = first.offset,
            first_len = first.len,
            second_offset = second.offset,
            second_len = second.len,
        );
        std::fs::write(&fixture.trace, trace).unwrap();
        fixture
    }

    fn ready(&self) -> gui::Ready {
        let mut tr = parse::parse(self.trace.to_str().unwrap()).unwrap();
        tr.parse_ms = 7;
        tr.index_ms = 3;
        let run = Some(EXECUTION_RUN.to_owned());
        let mut app = gui::Ready::new(tr, run.clone());
        app.an = Some(Box::new(
            gui::compute_analysis(&app.tr, None, None, run.as_deref()).unwrap(),
        ));
        app.native_popups = false;
        app.select(0);
        app
    }
}

impl Drop for ExecutionFixture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.trace).ok();
        std::fs::remove_file(&self.store).ok();
        std::fs::remove_dir(&self.directory).ok();
    }
}

fn execution_preview(h: &egui_kittest::Harness<'_, gui::Ready>) -> String {
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .value()
        .expect("the execution preview exposes its displayed text")
}

fn assert_no_execution_fetch(h: &egui_kittest::Harness<'_, gui::Ready>) {
    for label in [
        "Cancel fetch",
        "Copy metadata",
        "Copy manual fetch command",
        "CAS action definition (not execution output)",
        "current Action Cache — historical match unverified",
    ] {
        assert!(
            h.query_by_label(label).is_none(),
            "selecting an action/tab must not fetch remote evidence: {label}",
        );
    }
}

fn click_execution_tab(h: &mut egui_kittest::Harness<'_, gui::Ready>, label: &str) {
    h.get_by_label(label).click();
    // Pump the actual local reader, rather than snapshotting a scheduling
    // accident or reaching into private ExecutionUi state. The deadline is a
    // failure bound, not a fixed delay added to every test.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        assert_no_execution_fetch(h);
        if h.query_by_label("Reading recorded output…").is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "local execution preview did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.event(egui::Event::PointerGone);
    h.run();
}

#[test]
fn detail_execution_command_defaults_to_final_report() {
    let fixture = ExecutionFixture::create();
    let mut h = harness(fixture.ready());
    h.get_by_label("Source: command report in event log");
    h.get_by_label("Environment (1 recorded entries)");
    h.get_by_label("Copy command");
    // Report 1 has only `failed-fixture-cc`; this exact command therefore
    // proves final-report selection without relying on ComboBox labels.
    assert_eq!(execution_preview(&h), EXECUTION_COMMAND);
    assert!(h.query_by_label("Recorded stdout").is_none());
    assert!(h.query_by_label("Recorded stderr").is_none());
    assert_eq!(
        h.get_by_label("Fetch command metadata (no inputs)")
            .accesskit_node()
            .is_disabled(),
        !cfg!(fbcode_build),
        "remote metadata lookup requires the Meta integration",
    );
    assert_no_execution_fetch(&h);
    snapshot(&mut h, "detail_execution_command");
}

#[test]
fn detail_execution_output_tabs_are_bounded_and_reset_for_another_action() {
    let fixture = ExecutionFixture::create();
    let mut h = harness(fixture.ready());
    click_execution_tab(&mut h, "stdout");
    h.get_by_label("Recorded stdout");
    h.get_by_label("Copy text");
    assert_eq!(execution_preview(&h), EXECUTION_STDOUT);
    assert!(h.query_by_label_contains("Preview:").is_none());
    snapshot(&mut h, "detail_execution_stdout");

    click_execution_tab(&mut h, "stderr");
    h.get_by_label("Recorded stderr");
    h.get_by_label("Copy preview");
    h.get_by_label("Save preserves the entire recorded value.");
    let preview = execution_preview(&h);
    assert_eq!(preview.len(), execution::PREVIEW_BYTES);
    assert!(preview.starts_with(EXECUTION_STDERR_HEAD));
    assert!(!preview.contains("TAIL_AFTER_PREVIEW_MUST_NOT_BE_DISPLAYED"));
    h.get_by_label(&format!(
        "Preview: {} of {} available bytes. Search covers only this prefix.",
        execution::PREVIEW_BYTES,
        fixture.stderr_bytes,
    ));
    snapshot(&mut h, "detail_execution_stderr_truncated");

    // Filter the old stderr, then switch actions. A private new detail must
    // reset both the active tab and its search, not show the old stream.
    let tabs_y = h.get_by_label("Command").rect().bottom();
    let search = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|node| node.rect().top() > tabs_y)
        .expect("execution preview search field");
    search.focus();
    search.type_text("stderr detail");
    h.run();
    assert!(!execution_preview(&h).contains("ANSI stays text"));
    h.get_by_label_contains("matching line(s) in this preview");
    h.state_mut().select(1);
    h.run();
    h.get_by_label("Copy command");
    assert_eq!(execution_preview(&h), "fixture-gen --second-action");
    assert!(h.query_by_label_contains("matching line(s)").is_none());
    assert!(h.query_by_label("Copy preview").is_none());
    assert_no_execution_fetch(&h);
    click_execution_tab(&mut h, "stdout");
    h.get_by_label("Recorded stdout is empty.");
    assert_eq!(execution_preview(&h), "");
    h.state_mut().select(0);
    h.run();
    h.get_by_label("Copy command");
    assert_eq!(execution_preview(&h), EXECUTION_COMMAND);
    click_execution_tab(&mut h, "stderr");
    assert!(execution_preview(&h).starts_with(EXECUTION_STDERR_HEAD));
    assert!(h.query_by_label_contains("matching line(s)").is_none());
}

#[test]
fn detail_execution_legacy_output_is_unknown_not_empty() {
    let mut h = harness(select_fixture_action("smoke", "y (rustc emit)"));
    h.get_by_label("Source: legacy trace record");
    assert_eq!(execution_preview(&h), "rustc lib.rs");
    for tab in ["stdout", "stderr"] {
        click_execution_tab(&mut h, tab);
        h.get_by_label("No output is attributed to this selection. Select a recorded command report when available.");
        assert!(
            h.query_by_role(egui::accesskit::Role::MultilineTextInput)
                .is_none()
        );
        assert!(
            h.query_by_label(&format!("Recorded {tab} is empty."))
                .is_none()
        );
        assert!(h.get_by_label("Copy text").accesskit_node().is_disabled());
        assert!(
            h.get_by_label("Inspect current cached result")
                .accesskit_node()
                .is_disabled(),
        );
    }
    snapshot(&mut h, "detail_execution_legacy_output");
}

/// Run the scenario worker to completion on this thread. It streams the
/// replay and then one marginal per override, so a snapshot taken while
/// it is in flight would pin whichever rows happened to have landed.
fn settle_cp_sim(app: &mut gui::Ready) {
    let rx = app.cp_sim_rx.take().expect("a scenario worker is running");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "scenario worker timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("scenario worker disconnected before completion")
            }
            Ok(gui::CpSimMsg::Complete) => break,
            Ok(gui::CpSimMsg::View(v)) => app.cp_sim = Some(v),
            Ok(gui::CpSimMsg::Margin(k, m)) => app.cp_margins[k] = Some(m),
        }
    }
}

fn assert_cp_legend_is_visible(h: &egui_kittest::Harness<'_, gui::Ready>) {
    let window = h
        .ctx
        .memory(|memory| {
            memory.area_rect(egui::Id::new(("popup", "Critical path (dependency graph)")))
        })
        .expect("CP window");
    let visible = window.intersect(h.ctx.content_rect());
    let mut labels = retrace::detail::STAGE_CHIP
        .iter()
        .map(|chip| format!("\u{25a0} {chip}"))
        .collect::<Vec<_>>();
    labels.extend([
        "\u{25a0} unaccounted".to_owned(),
        "\u{25a7} not measured".to_owned(),
    ]);
    if h.state().cp_sim.is_some() {
        labels.push("\u{25a0} simulated".to_owned());
    }
    let chip_height = h.get_by_label("\u{25a0} cache lookup").rect().height();
    for label in labels {
        let rect = h.get_by_label(&label).rect();
        assert!(
            visible.contains_rect(rect),
            "CP legend chip must fit inside the visible popup: {label}",
        );
        assert!(
            rect.height() <= chip_height + 1.0,
            "CP legend color and name must stay on one row: {label}",
        );
    }
}

#[test]
fn cp_walker_scenario_list_scrolls() {
    // chain fixture: core(100) -> 7 chain steps(1500..200) -> tree(50),
    // with query_impl(900) and events(200) hanging off core in parallel.
    // CP = 100 + 5700 + 50 = 5850.
    let mut app = ready_over("chain");
    let act = |app: &gui::Ready, needle: &str| -> usize {
        (0..app.tr.acts.len())
            .find(|&i| app.tr.names[app.tr.acts[i].name as usize].contains(needle))
            .unwrap_or_else(|| panic!("fixture has no action matching {needle}"))
    };
    // eight overrides: more than the box shows at once, so the snapshot
    // pins the scrollbar as well as the rows
    for needle in [
        "buck2_build_api",
        "buck2_server_commands",
        "buck2_execute_impl",
        "buck2_action_impl",
        "buck2_client",
        "buck2_daemon",
        "rustc link bin/buck2",
        "buck2_events",
    ] {
        app.set_cp_override(act(&app, needle), 0, "removed".to_owned());
    }
    settle_cp_sim(&mut app);

    let sim = app.cp_sim.as_ref().expect("replay");
    assert_eq!(sim.base_cp_us, 5850);
    // every chain step gone, so the 900us query_impl branch gates
    assert_eq!(sim.cp_us, 1050);
    // Marginals, in row order. The first three still poke above the
    // 900us shadow branch; everything below it is worth exactly nothing
    // once the rest of the scenario is applied — including `buck2_events`,
    // which was never on the critical path to begin with.
    assert_eq!(
        app.cp_margins,
        [600, 300, 100, 0, 0, 0, 0, 0].map(Some).to_vec()
    );
    // 1000 != 4800: the reason the box carries a footer
    let summed: i64 = app.cp_margins.iter().flatten().sum();
    assert_eq!(summed, 1000);
    assert_eq!(sim.base_cp_us - sim.cp_us, 4800);

    app.panels.cp = true;
    let mut h = harness(app);
    h.run();
    assert_cp_legend_is_visible(&h);
    let window = h
        .ctx
        .memory(|memory| {
            memory.area_rect(egui::Id::new(("popup", "Critical path (dependency graph)")))
        })
        .expect("CP window");
    assert!(
        window.contains_rect(h.get_by_label("Open detail ▸").rect()),
        "scenario rows must not hide the step controls"
    );
    snapshot(&mut h, "cp_scenario_list");
}

#[test]
fn cp_step_navigation_follows_sorted_rows_and_opens_cascade_without_editing() {
    let mut app = ready_over("chain");
    app.panels.cp = true;
    app.sorts.insert("cp", (2, true));
    let last_step = app.cp_list.len() - 1;
    let mut h = harness_sized(app, egui::vec2(1400.0, 600.0));
    assert_cp_legend_is_visible(&h);
    assert!(
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Previous step")
            .accesskit_node()
            .is_disabled()
    );
    for step in 1..=last_step {
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Next step")
            .click();
        h.run();
        assert_eq!(h.state().cp_step, step);
    }
    assert!(
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Next step")
            .accesskit_node()
            .is_disabled()
    );
    let selected = h.state().cp_list[last_step];
    let name = h.state().tr.names[h.state().tr.acts[selected].name as usize].clone();
    let row = h.get_by_role_and_label(egui::accesskit::Role::Button, &name);
    assert_eq!(
        row.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    assert!(row.rect().top() > h.get_by_label("Open detail ▸").rect().bottom());
    assert!(row.rect().bottom() < 600.0);
    let window = h
        .ctx
        .memory(|memory| {
            memory.area_rect(egui::Id::new(("popup", "Critical path (dependency graph)")))
        })
        .expect("CP window");
    let footer = h
        .query_by_label_contains("Bar shares of")
        .expect("counter composition explanation")
        .rect();
    assert!(
        footer.bottom() + 30.0 <= window.bottom(),
        "counter composition and its bar must fit below the step table"
    );
    assert!(
        h.query_by_label_contains("gap after previous step:")
            .is_none()
    );
    h.event(egui::Event::PointerGone);
    h.run();
    snapshot(&mut h, "cp_selected_step");
    // Sorting relocates the selected action but must not follow it. The last
    // visible row remains at the same position after reversing the order.
    let old_bottom_row_y = h
        .get_by_role_and_label(egui::accesskit::Role::Button, &name)
        .rect()
        .center()
        .y;
    let longest = *h
        .state()
        .cp_list
        .iter()
        .max_by_key(|&&act| h.state().tr.acts[act].dur)
        .expect("nonempty critical path");
    let longest_name = h.state().tr.names[h.state().tr.acts[longest].name as usize].clone();
    h.get_by_label_contains("Duration").click();
    h.run();
    assert_eq!(h.state().cp_step, last_step);
    assert_eq!(h.state().cp_selected, Some(selected));
    let new_bottom_row_y = h
        .get_by_role_and_label(egui::accesskit::Role::Button, &longest_name)
        .rect()
        .center()
        .y;
    assert!(
        (new_bottom_row_y - old_bottom_row_y).abs() < 1.0,
        "sorting must retain the body scroll offset instead of following the selected action"
    );
    // Explicit navigation, unlike sorting, must reveal the selected row.
    for label in ["Previous step", "Next step"] {
        h.get_by_role_and_label(egui::accesskit::Role::Button, label)
            .click();
        h.run();
    }
    assert_eq!(h.state().cp_step, last_step);
    let row = h.get_by_role_and_label(egui::accesskit::Role::Button, &name);
    assert_eq!(
        row.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    let pos = row.rect().center();
    right_click_at(&mut h, pos);
    assert!(
        h.get_by_label("Cascade…").rect().top()
            > h.get_by_label("Simulate as cache hit").rect().bottom()
    );
    let before = h.state().cp_overrides.clone();
    h.get_by_label("Cascade…").click();
    h.run();
    assert!(h.state().panels.cp && h.state().panels.cascade);
    assert_eq!(h.state().cp_overrides, before);
    assert!(!h.state().cascade.is_running());
    assert!(h.state().cascade.steps.is_empty());
}

/// Exercise the action table without the timeline or graph workers. Its rows
/// have unique names and durations so reversing the sort moves the selection
/// well outside the viewport in both the ordinary and large-table cases.
struct ActionTableHarness {
    tr: retrace::types::Trace,
    items: Vec<usize>,
    sort: (u8, bool),
    selected: usize,
    reveal: Option<usize>,
}

fn action_table_harness(rows: usize) -> egui_kittest::Harness<'static, ActionTableHarness> {
    let mut tr = parse::parse(&testdata("chain-trace.json")).unwrap();
    let prototype = tr.acts[0];
    tr.names = (0..rows).map(|i| format!("table action {i:04}")).collect();
    tr.acts = (0..rows)
        .map(|i| retrace::types::Act {
            name: i as u32,
            ts: i as i64,
            dur: i as i64 + 1,
            ..prototype
        })
        .collect();
    let app = ActionTableHarness {
        tr,
        items: (0..rows).collect(),
        sort: (2, false),
        selected: rows / 4,
        reveal: None,
    };
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 400.0))
        .build_ui_state(
            |ui, app| {
                const TABLE_ID: &str = "scroll-regression";
                let mut nav = None;
                if let Some(sort) = gui::widgets::act_table(
                    ui,
                    &app.tr,
                    &app.items,
                    TABLE_ID,
                    app.sort,
                    &mut nav,
                    gui::widgets::ActTableOpts {
                        selected: Some(app.selected),
                        reveal: app.reveal.take(),
                        max_height: Some(240.0),
                        ..Default::default()
                    },
                ) {
                    app.sort = sort;
                }
                ui.label("After action table");
            },
            app,
        );
    gui::install_fonts(&h.ctx);
    h.run();
    h.state_mut().reveal = Some(h.state().selected);
    h.run();
    h
}

fn table_action_y(h: &egui_kittest::Harness<'_, ActionTableHarness>, action: usize) -> f32 {
    let name = format!("table action {action:04}");
    h.get_by_role_and_label(egui::accesskit::Role::Button, &name)
        .rect()
        .center()
        .y
}

fn assert_selected_table_row_visible(h: &egui_kittest::Harness<'_, ActionTableHarness>) {
    let name = format!("table action {:04}", h.state().selected);
    let row = h.get_by_role_and_label(egui::accesskit::Role::Button, &name);
    assert_eq!(
        row.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True),
        "revealing a row must preserve its selected styling"
    );
    assert!(row.rect().top() >= h.get_by_label_contains("Duration").rect().bottom());
    assert!(row.rect().bottom() <= h.get_by_label("After action table").rect().top());
}

fn check_action_table_scroll_and_sort(rows: usize) {
    let mut h = action_table_harness(rows);
    assert_selected_table_row_visible(&h);
    let selected = h.state().selected;
    let headers = ["#", "Start", "Duration", "Action"];
    let header_positions = headers.map(|label| h.get_by_label_contains(label).rect().min);
    let before_sort = table_action_y(&h, selected);
    h.get_by_label_contains("Duration").click();
    h.run();
    assert_eq!(h.state().sort, (2, true));
    assert_eq!(h.state().selected, selected);
    // Ascending row k becomes action (rows - 1 - k) when reversed. Its
    // visible position must remain unchanged, without depending on the
    // table implementation's private scroll-state IDs.
    let same_position_action = rows - 1 - selected;
    assert!(
        (table_action_y(&h, same_position_action) - before_sort).abs() < 1.0,
        "sorting must not reveal the selected action or reset body scrolling"
    );

    let point = egui::pos2(
        h.get_by_label("Action").rect().center().x,
        h.get_by_label("Action").rect().bottom() + 40.0,
    );
    h.event(egui::Event::PointerMoved(point));
    h.run();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    // Scrolling changes the hovered row, so allow delayed tooltips to repaint.
    h.run_steps(10);
    // Virtualized row positions are rounded to the pixel grid on each frame.
    assert!(
        (before_sort - table_action_y(&h, same_position_action) - 100.0).abs() < 3.0,
        "mouse wheel must move the row body by the requested distance: before={before_sort}, after={}",
        table_action_y(&h, same_position_action)
    );
    for (label, expected) in headers.into_iter().zip(header_positions) {
        let actual = h.get_by_label_contains(label).rect().min;
        assert!(
            actual.distance(expected) < 1.0,
            "column header {label} must stay fixed when sorting or scrolling rows"
        );
    }

    // Follow requests must still work across a large distance in either
    // direction, even when the selected row was not rendered in this frame.
    h.event(egui::Event::PointerGone);
    h.run();
    for selected in [rows - 1, 0] {
        h.state_mut().selected = selected;
        h.state_mut().reveal = Some(selected);
        h.run();
        assert_selected_table_row_visible(&h);
    }
}

#[test]
fn action_table_sort_preserves_scroll_and_headers_stay_fixed() {
    check_action_table_scroll_and_sort(80);
}

#[test]
fn virtualized_action_table_sort_preserves_scroll_and_headers_stay_fixed() {
    check_action_table_scroll_and_sort(1200);
}

#[test]
fn cp_greedy_cascade() {
    // Compute the tiny fixture synchronously: the screenshot must not depend
    // on how many streamed rows the background worker delivered this frame.
    let mut app = ready_over("chain");
    let gui::GState::Ready { dag: Some(b), .. } = &app.gstate else {
        panic!("chain fixture must have a DAG");
    };
    let cascade = retrace::sim::GreedyCascade::new(
        &b.dag,
        b.dag.weight_us.clone(),
        retrace::sim::MAX_CASCADE_STEPS,
    );
    app.cascade.measured_cp_us = Some(cascade.base_cp_us());
    app.cascade.start_cp_us = Some(cascade.start_cp_us());
    app.cascade.steps = cascade.collect();
    app.cascade.prefix = 3;
    app.cascade.status = gui::cascade::Status::Complete;
    assert!(!app.cascade.steps.is_empty());
    assert_eq!(
        app.cascade
            .steps
            .iter()
            .map(|s| s.marginal_saved_us)
            .sum::<i64>(),
        app.cascade.steps.last().unwrap().cumulative_saved_us,
    );
    app.panels.cascade = true;
    let mut h = harness(app);
    h.run();
    snapshot(&mut h, "cp_greedy_cascade");
    let original = h.state().cascade.steps.clone();
    let mut by_saving = original.clone();
    by_saving.sort_by(|a, b| {
        b.marginal_saved_us
            .cmp(&a.marginal_saved_us)
            .then(a.index.cmp(&b.index))
    });
    h.get_by_label("Marginal saving").click();
    h.run();
    assert_eq!(h.state().cascade.steps, original);
    assert_eq!(h.state().cascade.prefix, 3);
    let y: Vec<_> = by_saving
        .iter()
        .map(|step| h.get_by_label(&step.index.to_string()).rect().center().y)
        .collect();
    assert!(y.windows(2).all(|pair| pair[0] < pair[1]));
    h.get_by_label("2").click();
    h.run();
    assert_eq!(h.state().cascade.prefix, 2);
    h.get_by_label("Apply first 2 removal(s) in experiment order");
}

#[test]
fn stage_glossary() {
    let mut app = ready();
    app.panels.glossary = true;
    let mut h = harness(app);
    h.run();
    h.get_by_label(&format!("■ {}", retrace::detail::STAGE_LABEL[0]));
    h.get_by_label(&format!(
        "■ {}",
        retrace::detail::STAGE_LABEL[retrace::detail::STAGE_N - 1]
    ));
    assert!(h.state().panels.glossary);
    snapshot(&mut h, "stage_glossary");
}

/// The head — time ruler and in-flight strip — is fixed, and the plot below
/// it scrolls under it.
///
/// Pinned because the two used to be painted into the same unclipped
/// painter. A row is only flush with the top of the plot when the scroll
/// offset is a whole number of rows; the rest of the time the topmost row
/// starts above it, and its bar painted straight over the strip. At the tall
/// row heights this scene uses, that hid the strip completely.
#[test]
fn scrolled_lanes_stay_under_the_head() {
    let mut app = ready();
    // A short window and the tallest rows the app allows, so the fixture's
    // five lanes overflow the plot with room to scroll. At 1.7 rows down,
    // the topmost row starts 33.6 points above the plot -- more than the
    // 30-point strip is tall, so an unclipped bar covers it outright.
    app.row_h = gui::canvas::ROW_H_MAX;
    app.scroll = app.row_h * 1.7;
    let mut h = harness_sized(app, egui::vec2(1000.0, 230.0));
    h.run();
    let app = h.state();
    let intended_scroll = app.row_h * 1.7;
    assert!(app.tr.lanes.len() as f32 * app.row_h - app.canvas_h >= intended_scroll);
    assert!((app.scroll - intended_scroll).abs() < 0.01);
    // The pixel baseline verifies that the overflowing bars are clipped.
    snapshot(&mut h, "scrolled_head");
}

#[test]
fn longest_actions_show_the_data_each_action_moved() {
    let mut h = harness(ready());
    h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Analysis")
        .into_iter()
        .min_by(|a, b| a.rect().center().y.total_cmp(&b.rect().center().y))
        .expect("the Analysis menu")
        .click();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Longest actions")
        .click();
    h.run();
    h.get_by_label_contains("Inputs");
    h.get_by_label_contains("Outputs");
    // `y (rustc emit)` recorded 5,000,000 bytes of inputs and ran locally, so it shows its
    // logical input size and no RE transfer.
    h.get_by_label("4.8 MiB");
}
