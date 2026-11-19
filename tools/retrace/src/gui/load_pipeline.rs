/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Hardware-budgeted startup overlap.
//! Graph bytes may be read before the trace is parsed, but graph admission,
//! selected-run validation and DAG construction stay with Ready::poll_graph.
//! No graph, trace, or result is cached globally. The one global atomic only
//! limits speculative worker scratch to one request; a busy slot falls back
//! to the existing serial startup. The normal graph worker lifecycle after
//! adoption is unchanged.

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use super::Worker;
use crate::graph;
use crate::parse;
use crate::types::Trace;

static SPECULATIVE_GRAPH_BUSY: AtomicBool = AtomicBool::new(false);

struct Permit;

impl Permit {
    fn take() -> Option<Self> {
        SPECULATIVE_GRAPH_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        SPECULATIVE_GRAPH_BUSY.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    unix: (u64, u64, i64, i64),
}

impl FileIdentity {
    #[cfg(unix)]
    fn read(path: &str) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self {
            len: m.len(),
            modified: m.modified().ok()?,
            unix: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
        })
    }

    #[cfg(not(unix))]
    fn read(_path: &str) -> Option<Self> {
        // Do not guess a file-generation identity on unsupported platforms.
        // They retain serial startup even when overlap was requested.
        None
    }
}

struct PreloadedGraph {
    adopt: Option<mpsc::Sender<()>>,
    rx: Option<mpsc::Receiver<anyhow::Result<graph::Graph>>>,
    cancel: Arc<AtomicBool>,
    adopted: bool,
}

impl PreloadedGraph {
    fn start(path: String, workers: usize, max_input_bytes: u64) -> Option<Self> {
        let before = FileIdentity::read(&path)?;
        let permit = Permit::take()?;
        Self::start_owned(
            path,
            before,
            Some(permit),
            workers,
            max_input_bytes,
            move |path, cancel| {
                graph::load_graph_cancellable_with_workers_and_limit(
                    path,
                    cancel,
                    workers,
                    max_input_bytes,
                )
            },
        )
    }

    fn start_owned<F>(
        path: String,
        before: FileIdentity,
        permit: Option<Permit>,
        workers: usize,
        max_input_bytes: u64,
        load: F,
    ) -> Option<Self>
    where
        F: FnOnce(&str, &AtomicBool) -> anyhow::Result<graph::Graph> + Send + 'static,
    {
        let (adopt, adopted) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("retrace-graph-preload".to_owned())
            .spawn(move || {
                let _permit = permit;
                let result = load(&path, &worker_cancel);
                let after = FileIdentity::read(&path);
                // A parse/run refusal drops its private admission sender.
                // Cancellation is cooperative during loading, not an instant
                // interruption of an oversized row or a blocking file read.
                if worker_cancel.load(Ordering::Relaxed) || adopted.recv().is_err() {
                    return;
                }
                let unchanged = after.as_ref() == Some(&before)
                    && after.is_some()
                    && FileIdentity::read(&path) == after;
                let result = if unchanged {
                    result
                } else {
                    // Only a changed generation needs to be read again.
                    // Release the speculative result BEFORE loading again.
                    drop(result);
                    graph::load_graph_cancellable_with_workers_and_limit(
                        &path,
                        &worker_cancel,
                        workers,
                        max_input_bytes,
                    )
                };
                if let Ok(g) = &result {
                    graph::dropped_line_notice(&path, g.dropped_lines, g.dropped_interior);
                }
                drop(tx.send(result));
            })
            .ok()?;
        Some(Self {
            adopt: Some(adopt),
            rx: Some(rx),
            cancel,
            adopted: false,
        })
    }

    fn adopt(mut self) -> Worker<anyhow::Result<graph::Graph>> {
        self.adopted = true;
        if let Some(tx) = self.adopt.take() {
            // An unwinding panic uses Ready's existing disconnected-receiver
            // error; abort-profile panics still terminate the process. Neither
            // is converted into an absent graph.
            tx.send(()).ok();
        }
        Worker {
            rx: self.rx.take().expect("preload adopted once"),
            cancel: self.cancel.clone(),
        }
    }
}

impl Drop for PreloadedGraph {
    fn drop(&mut self) {
        if !self.adopted {
            self.cancel.store(true, Ordering::Relaxed);
        }
        // No UI-thread join: the worker checks cancellation while reading
        // and in large index/cycle sweeps, then releases the scratch permit.
        // Once adopted it has exactly the existing graph-worker lifecycle.
    }
}

pub struct LoadedTrace {
    pub tr: Trace,
    /// Conversion warnings retained for a GUI launched without a terminal.
    pub notices: Vec<String>,
    pub status_lines: Vec<String>,
    /// Also applies when a busy or unsupported preload falls back to Ready's
    /// serial graph worker; zero retains the caller's uncapped choice.
    max_input_bytes: u64,
    preload: Option<PreloadedGraph>,
    #[cfg(test)]
    pub overlap_requested: bool,
    /// Diagnostic only; a busy/unsupported preload keeps serial startup.
    #[cfg(test)]
    pub overlap_started: bool,
}

impl LoadedTrace {
    pub(super) fn into_parts(self) -> (Trace, Option<Worker<anyhow::Result<graph::Graph>>>, u64) {
        (
            self.tr,
            self.preload.map(PreloadedGraph::adopt),
            self.max_input_bytes,
        )
    }
}

#[cfg(test)]
pub(super) fn load(
    path: &str,
    opts: &parse::LoadOpts,
    explicit_graph: Option<&str>,
    overlap_possible: bool,
) -> anyhow::Result<LoadedTrace> {
    load_cancellable(
        path,
        opts,
        explicit_graph,
        overlap_possible,
        &AtomicBool::new(false),
    )
}

pub(super) fn load_cancellable(
    path: &str,
    opts: &parse::LoadOpts,
    explicit_graph: Option<&str>,
    overlap_possible: bool,
    cancel: &AtomicBool,
) -> anyhow::Result<LoadedTrace> {
    // This entry is called only AFTER native ingestion/cache validation
    // has returned the published artifact pair, never just from its UUID.
    let graph_path = explicit_graph
        .map(str::to_owned)
        .or_else(|| graph::sibling_graph_path(path));
    let plan =
        crate::parallelism::viewer_plan(opts.threads, overlap_possible && graph_path.is_some());
    let preload = if plan.overlap {
        graph_path
            .and_then(|path| PreloadedGraph::start(path, plan.graph_workers, opts.max_input_bytes))
    } else {
        None
    };
    let overlap_started = preload.is_some();
    // A missing/unsupported/busy preload leaves no graph pool in this load's
    // budget, so the trace may use the standalone allocation. Cancellation of
    // an older request remains cooperative, not a process-wide scheduler.
    let threads = if overlap_started {
        plan.trace_threads
    } else {
        crate::parallelism::trace_threads(opts.threads)
    };
    let tr = parse::parse_opts_with_threads_cancellable(path, opts, threads, cancel)?;
    Ok(LoadedTrace {
        tr,
        notices: Vec::new(),
        status_lines: Vec::new(),
        max_input_bytes: opts.max_input_bytes,
        preload,
        #[cfg(test)]
        overlap_requested: plan.overlap,
        #[cfg(test)]
        overlap_started,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use super::super::GState;
    use super::super::Ready;
    use super::*;

    fn body(name: &str) -> String {
        format!("{{\"graph\":{{\"enc\":2}}}}\n{{\"t\":\"{name}\",\"d\":[]}}\n")
    }

    fn settle(app: &mut Ready) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            app.poll_graph();
            if !matches!(app.gstate, GState::Loading(_) | GState::Building(_)) {
                return;
            }
            assert!(Instant::now() < deadline, "graph did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Bypass only the process-wide scratch permit so parallel unit tests
    /// exercise actual adoption deterministically instead of a busy fallback.
    fn with_preload(mut loaded: LoadedTrace, graph_path: &str) -> LoadedTrace {
        let max_input_bytes = loaded.max_input_bytes;
        loaded.preload = PreloadedGraph::start_owned(
            graph_path.to_owned(),
            FileIdentity::read(graph_path).unwrap(),
            None,
            0,
            max_input_bytes,
            move |path, cancel| {
                graph::load_graph_cancellable_with_workers_and_limit(
                    path,
                    cancel,
                    0,
                    max_input_bytes,
                )
            },
        );
        assert!(loaded.preload.is_some());
        loaded.overlap_requested = true;
        loaded.overlap_started = true;
        loaded
    }

    #[test]
    fn changed_graph_is_reloaded_at_adoption_and_drop_cancels() {
        let path = crate::temp_path("preload-graph.json");
        let p = path.to_str().unwrap().to_owned();
        std::fs::write(&path, body("old")).unwrap();
        let before = FileIdentity::read(&p).unwrap();
        let (loaded, ready) = mpsc::channel();
        let pending = PreloadedGraph::start_owned(
            p.clone(),
            before,
            None,
            0,
            parse::DEFAULT_MAX_INPUT_BYTES,
            move |p, cancel| {
                let g = graph::load_graph_cancellable(p, cancel);
                loaded.send(()).unwrap();
                g
            },
        )
        .unwrap();
        ready.recv_timeout(Duration::from_secs(60)).unwrap();
        std::fs::write(&path, body("new")).unwrap();
        let g = pending
            .adopt()
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .unwrap();
        assert_eq!(
            g.targets
                .iter()
                .map(|name| name.as_ref())
                .collect::<Vec<_>>(),
            ["new"]
        );

        let (seen, wait) = mpsc::channel();
        let pending = PreloadedGraph::start_owned(
            p.clone(),
            FileIdentity::read(&p).unwrap(),
            None,
            0,
            parse::DEFAULT_MAX_INPUT_BYTES,
            move |_, cancel| {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                seen.send(()).unwrap();
                anyhow::bail!("cancelled")
            },
        )
        .unwrap();
        drop(pending);
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn absent_preload_keeps_late_sibling_discovery_and_parse_error() {
        let path = crate::temp_path("preload-trace.json");
        std::fs::write(
            &path,
            "[\n{\"ph\":\"X\",\"name\":\"a\",\"ts\":0,\"dur\":1}\n]\n",
        )
        .unwrap();
        let loaded = load(
            path.to_str().unwrap(),
            &parse::LoadOpts::default(),
            None,
            true,
        )
        .unwrap();
        assert!(!loaded.overlap_started);
        let graph_path = path.with_file_name(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .replace("trace", "graph"),
        );
        std::fs::write(&graph_path, body("a")).unwrap();
        let mut app = Ready::from_loaded(loaded, None, None).unwrap();
        assert!(matches!(app.gstate, GState::Loading(_)));
        settle(&mut app);
        assert!(matches!(app.gstate, GState::Ready { .. }));
        drop(app);
        std::fs::write(&path, "{\"not-a-trace\":true}").unwrap();
        let result = load(
            path.to_str().unwrap(),
            &parse::LoadOpts::default(),
            None,
            true,
        );
        assert!(result.is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(graph_path).unwrap();
    }

    #[test]
    fn unchanged_speculative_error_is_not_read_again_on_adoption() {
        let path = crate::temp_path("preload-retry-graph.json");
        let p = path.to_str().unwrap();
        std::fs::write(&path, body("current")).unwrap();
        let pending = PreloadedGraph::start_owned(
            p.to_owned(),
            FileIdentity::read(p).unwrap(),
            None,
            0,
            parse::DEFAULT_MAX_INPUT_BYTES,
            |_, _| anyhow::bail!("speculative failure"),
        )
        .unwrap();
        let result = pending
            .adopt()
            .recv_timeout(Duration::from_secs(60))
            .unwrap();
        let Err(error) = result else {
            panic!("unchanged speculative refusal was discarded")
        };
        assert!(error.to_string().contains("speculative failure"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn preloaded_graph_matches_serial_viewer_state() {
        fn shown(app: &Ready) -> serde_json::Value {
            let GState::Ready {
                g,
                dag: Some(bundle),
                quality,
            } = &app.gstate
            else {
                panic!("expected ready graph and DAG");
            };
            // Ignore clocks, map iteration and file paths. The graph/core
            // suites separately compare every scanner/DAG field; this test
            // exercises GUI adoption, quality and view state without a
            // dependency on the optional measurement driver.
            serde_json::json!({
                "summary": app.summary,
                "run": app.run_id,
                "cp_list": app.cp_list,
                "concurrency": app.conc,
                "concurrency_max": app.conc_max,
                "graph_rows": g.n_rows,
                "graph_edges": g.n_edges,
                "graph_joins": g.n_joins,
                "graph_chain": format!("{:?}", g.chain),
                "dag": format!("{:?}", bundle.dag),
                "node_events": bundle.node_ev,
                "events": format!("{:?}", bundle.events),
                "quality": format!("{quality:?}"),
            })
        }
        let (tp, gp) = (
            crate::testdata("smoke-trace.json"),
            crate::testdata("smoke-graph.jsonl"),
        );
        let mut fingerprints = Vec::new();
        for overlap in [false, true] {
            let mut loaded = load(&tp, &parse::LoadOpts::default(), Some(&gp), false).unwrap();
            assert!(!loaded.overlap_requested && !loaded.overlap_started);
            if overlap {
                loaded = with_preload(loaded, &gp);
            }
            let mut app = Ready::from_loaded(loaded, None, Some(gp.clone())).unwrap();
            settle(&mut app);
            assert!(matches!(app.gstate, GState::Ready { dag: Some(_), .. }));
            (app.conc, app.conc_max) = super::super::canvas::build_concurrency(&app.tr);
            fingerprints.push(shown(&app));
        }
        assert_eq!(fingerprints[0], fingerprints[1]);
    }

    #[test]
    fn preloaded_wrong_pair_keeps_explicit_and_discovered_policies() {
        let (tp, gp) = (
            crate::testdata("review-trace.json"),
            crate::testdata("review-mismatched-run-graph.jsonl"),
        );
        let path = crate::temp_path("preload-pair-trace.json");
        let graph_path = path.with_file_name(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .replace("trace", "graph"),
        );
        std::fs::copy(tp, &path).unwrap();
        std::fs::copy(gp, &graph_path).unwrap();
        let p = path.to_str().unwrap();
        let gp = graph_path.to_str().unwrap();
        for explicit in [false, true] {
            let mut outcomes = Vec::new();
            for overlap in [false, true] {
                let graph_arg = explicit.then_some(gp);
                let mut loaded = load(p, &parse::LoadOpts::default(), graph_arg, false).unwrap();
                if overlap {
                    loaded = with_preload(loaded, gp);
                }
                let mut app =
                    Ready::from_loaded(loaded, None, graph_arg.map(str::to_owned)).unwrap();
                settle(&mut app);
                assert!(app.cp_set.is_none());
                let outcome = match &app.gstate {
                    GState::Failed(e) if explicit => e.clone(),
                    GState::Absent if !explicit => app.graph_warning.clone().unwrap(),
                    _ => panic!("incorrect mismatched-pair admission"),
                };
                outcomes.push(outcome);
            }
            assert_eq!(outcomes[0], outcomes[1]);
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(graph_path).unwrap();
    }

    #[test]
    fn serial_and_preloaded_graphs_honor_the_requested_input_cap() {
        let trace_path = crate::temp_path("graph-cap-trace.json");
        let graph_path = crate::temp_path("graph-cap-graph.jsonl");
        let trace = "[\n{\"ph\":\"X\",\"cat\":\"recipe\",\"name\":\"a\",\"ts\":0,\"dur\":1}\n]\n";
        let graph = format!("{}{}\n", body("a"), " ".repeat(512));
        const CAP: u64 = 256;
        assert!((trace.len() as u64) < CAP && (graph.len() as u64) > CAP);
        std::fs::write(&trace_path, trace).expect("should write capped-load trace");
        std::fs::write(&graph_path, graph).expect("should write capped-load graph");
        let tp = trace_path.to_str().expect("should have a UTF-8 test path");
        let gp = graph_path.to_str().expect("should have a UTF-8 test path");
        for max_input_bytes in [CAP, 0] {
            for overlap in [false, true] {
                let opts = parse::LoadOpts {
                    max_input_bytes,
                    ..Default::default()
                };
                let mut loaded =
                    load(tp, &opts, Some(gp), false).expect("trace fits the requested cap");
                assert_eq!(loaded.max_input_bytes, max_input_bytes);
                if overlap {
                    loaded = with_preload(loaded, gp);
                }
                let mut app = Ready::from_loaded(loaded, None, Some(gp.to_owned()))
                    .expect("should adopt parsed trace");
                settle(&mut app);
                if max_input_bytes == 0 {
                    assert!(matches!(app.gstate, GState::Ready { .. }));
                } else {
                    let GState::Failed(error) = &app.gstate else {
                        panic!("oversized graph must fail with and without preload");
                    };
                    assert!(error.contains("256 byte input cap"), "{error}");
                }
            }
        }
        std::fs::remove_file(trace_path).expect("should remove test trace");
        std::fs::remove_file(graph_path).expect("should remove test graph");
    }

    #[test]
    fn graph_generation_reload_keeps_the_requested_input_cap() {
        let path = crate::temp_path("generation-graph-cap.jsonl");
        let p = path
            .to_str()
            .expect("should have a UTF-8 test path")
            .to_owned();
        std::fs::write(&path, body("a")).expect("should write initial graph");
        const CAP: u64 = 256;
        let (read, ready) = mpsc::channel();
        let pending = PreloadedGraph::start_owned(
            p.clone(),
            FileIdentity::read(&p).expect("should read graph identity"),
            None,
            0,
            CAP,
            move |path, cancel| {
                let result =
                    graph::load_graph_cancellable_with_workers_and_limit(path, cancel, 0, CAP);
                assert!(result.is_ok(), "initial generation fits");
                read.send(()).expect("should signal initial read");
                result
            },
        )
        .expect("should start graph preload");
        ready
            .recv_timeout(Duration::from_secs(60))
            .expect("should finish initial read");
        std::fs::write(&path, format!("{}{}\n", body("a"), " ".repeat(512)))
            .expect("should write oversized replacement graph");
        let result = pending
            .adopt()
            .recv_timeout(Duration::from_secs(60))
            .expect("should finish replacement read");
        let Err(error) = result else {
            panic!("replacement graph must retain the caller's cap");
        };
        assert!(error.to_string().contains("256 byte input cap"), "{error}");
        std::fs::remove_file(path).expect("should remove test graph");
    }

    #[test]
    fn invalid_run_selection_cancels_its_unused_preload() {
        let (tp, gp) = (
            crate::testdata("smoke-trace.json"),
            crate::testdata("smoke-graph.jsonl"),
        );
        let mut loaded = load(&tp, &parse::LoadOpts::default(), Some(&gp), false).unwrap();
        let (seen, wait) = mpsc::channel();
        loaded.preload = PreloadedGraph::start_owned(
            gp.clone(),
            FileIdentity::read(&gp).unwrap(),
            None,
            0,
            parse::DEFAULT_MAX_INPUT_BYTES,
            move |_, cancel| {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                seen.send(()).unwrap();
                anyhow::bail!("unused preload cancelled")
            },
        );
        assert!(loaded.preload.is_some());
        assert!(Ready::from_loaded(loaded, Some("not-a-run"), Some(gp)).is_err());
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
    }
}
