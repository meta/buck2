/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Bounded graph parsing. The existing row scanner runs against
//! batch-local interners; global names, rows, header decisions and malformed
//! line runs are merged strictly in input order. There is one outstanding
//! batch per worker, no all-file second interner, and no post-decode semaphore.
//!
//! The raw byte/line limits are flush targets, not RSS limits: the last line
//! may be oversized. While parsing, raw bytes, local names and lookup maps,
//! rows and scratch coexist; arbitrary skipped JSON values keep the existing
//! scanner's temporary allocation behavior. Completed batches drop the local
//! lookup map before handoff. Vector/hash-table capacity, allocator overhead,
//! thread stacks, the final graph/CSR and concurrent trace parsing are extra.
//! An ordered merge holds one local-to-global ID vector. Cancellation is
//! cooperative between lines/batches; a single large scan is not preemptible.

use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::sync_channel;
use std::thread::ScopedJoinHandle;
use std::time::Duration;

use super::*;

const BATCH_BYTES: usize = 4 << 20;
const BATCH_LINES: usize = 4096;

#[derive(Clone, Copy)]
pub(super) struct Config {
    workers: usize,
    bytes: usize,
    lines: usize,
}

impl Config {
    pub(super) fn new(workers: usize) -> Self {
        Self {
            workers,
            bytes: BATCH_BYTES,
            lines: BATCH_LINES,
        }
    }
}

struct RawBatch {
    bytes: Vec<u8>,
    ends: Vec<usize>,
    // A read error is ordered AFTER the complete preceding lines, rather
    // than escaping from the speculative reader ahead of a schema refusal.
    read_error: Option<std::io::Error>,
}

enum Line {
    Dropped,
    Bad,
    Unknown,
    Header(Vec<u8>),
    Row {
        target: Option<u32>,
        join: bool,
        begin: usize,
        end: usize,
    },
}

struct ParsedBatch {
    names: Vec<Arc<str>>,
    lines: Vec<Line>,
    deps: Vec<u32>,
    read_error: Option<std::io::Error>,
}

fn read_batch<R: BufRead, F: Fn() -> anyhow::Result<()>>(
    reader: &mut R,
    config: Config,
    check: &F,
) -> anyhow::Result<Option<RawBatch>> {
    let mut batch = RawBatch {
        bytes: Vec::with_capacity(config.bytes),
        ends: Vec::with_capacity(config.lines),
        read_error: None,
    };
    loop {
        check()?;
        let start = batch.bytes.len();
        match reader.read_until(b'\n', &mut batch.bytes) {
            Ok(0) => return Ok((!batch.ends.is_empty()).then_some(batch)),
            Ok(_) => batch.ends.push(batch.bytes.len()),
            Err(error) => {
                // read_until may append a partial line before returning an
                // error. The ordinary loader never scans that partial line.
                batch.bytes.truncate(start);
                batch.read_error = Some(error);
                return Ok(Some(batch));
            }
        }
        if batch.bytes.len() >= config.bytes || batch.ends.len() >= config.lines {
            return Ok(Some(batch));
        }
    }
}

fn parse_batch(raw: RawBatch, need_deps: bool, stop: &AtomicBool) -> anyhow::Result<ParsedBatch> {
    let mut it = Interner {
        targets: Vec::new(),
        index: FxHashMap::default(),
    };
    let mut lines = Vec::with_capacity(raw.ends.len());
    let mut deps = Vec::new();
    let mut dbuf = Vec::new();
    let mut obuf = Vec::new();
    let mut start = 0;
    for end in raw.ends {
        anyhow::ensure!(!stop.load(Ordering::Relaxed), "graph parser cancelled");
        let line = &raw.bytes[start..end];
        start = end;
        let mut a = 0;
        let mut z = line.len();
        while a < z && matches!(line[a], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            a += 1;
        }
        while z > a && matches!(line[z - 1], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            z -= 1;
        }
        if a == z {
            continue;
        }
        let b = &line[a..z];
        dbuf.clear();
        obuf.clear();
        let mut target = None;
        let mut join = false;
        let mark = it.mark();
        let kind = match scan_object(
            b,
            &mut it,
            &mut dbuf,
            &mut obuf,
            &mut target,
            &mut join,
            need_deps,
        ) {
            Ok(kind) => kind,
            Err(error) => {
                it.rollback(mark);
                lines.push(
                    if error == RowErr::Shape
                        && serde_json::from_slice::<serde_json::Value>(b).is_ok()
                    {
                        Line::Bad
                    } else {
                        Line::Dropped
                    },
                );
                continue;
            }
        };
        lines.push(match kind {
            Rec::Header => Line::Header(b.to_vec()),
            Rec::Unknown => Line::Unknown,
            Rec::Row => {
                let begin = deps.len();
                deps.extend_from_slice(&dbuf);
                deps.extend_from_slice(&obuf);
                Line::Row {
                    target,
                    join,
                    begin,
                    end: deps.len(),
                }
            }
        });
    }
    // The map shares names with the vector. Its lookup storage is scratch
    // and must not travel with completed queued batches.
    drop(it.index);
    Ok(ParsedBatch {
        names: it.targets,
        lines,
        deps,
        read_error: raw.read_error,
    })
}

fn merge_batch<F: Fn() -> anyhow::Result<()>>(
    batch: ParsedBatch,
    out: &mut GraphRows,
    path: &str,
    need_deps: bool,
    check: &F,
) -> anyhow::Result<()> {
    let mut remap = Vec::with_capacity(batch.names.len());
    for (i, name) in batch.names.into_iter().enumerate() {
        if i & 4095 == 0 {
            check()?;
        }
        // Replay local *insertion* order, including names from overwritten
        // keys and prefix/header-like rows, not merely final retained deps.
        // Moving the shared local name avoids another string allocation.
        let id = if let Some(&id) = out.it.index.get(name.as_ref()) {
            id
        } else {
            check_target_count(path, out.it.targets.len().saturating_add(1))?;
            let id = out.it.targets.len() as u32;
            out.it.targets.push(name.clone());
            out.it.index.insert(name, id);
            id
        };
        remap.push(id);
    }
    for line in batch.lines {
        check()?;
        if matches!(line, Line::Dropped) {
            out.pending_drops += 1;
            continue;
        }
        out.dropped_interior += out.pending_drops;
        out.pending_drops = 0;
        match line {
            Line::Dropped => unreachable!(),
            Line::Bad => out.bad_rows += 1,
            Line::Unknown => {}
            Line::Header(bytes) => {
                if !parse_graph_header(path, &bytes, &mut out.meta)? {
                    out.pending_drops += 1;
                }
            }
            Line::Row {
                target,
                join,
                begin,
                end,
            } => {
                if out.meta.is_none() {
                    out.pending_drops += 1;
                    continue;
                }
                if !need_deps {
                    continue;
                }
                let Some(target) = target else {
                    out.bad_rows += 1;
                    continue;
                };
                out.rows.t.push(remap[target as usize]);
                out.rows.j.push(join);
                out.rows
                    .deps
                    .extend(batch.deps[begin..end].iter().map(|&id| remap[id as usize]));
                let Ok(off) = u32::try_from(out.rows.deps.len()) else {
                    anyhow::bail!(
                        "{path}: more than {} dependency edges — the CSR uses u32 offsets; \
                         this is far past any real build and almost certainly a corrupt file",
                        u32::MAX
                    );
                };
                out.rows.off.push(off);
            }
        }
    }
    if let Some(error) = batch.read_error {
        return Err(error).with_context(|| path.to_owned());
    }
    Ok(())
}

struct Worker<'scope> {
    input: Option<SyncSender<RawBatch>>,
    output: Option<Receiver<anyhow::Result<ParsedBatch>>>,
    join: Option<ScopedJoinHandle<'scope, ()>>,
}

fn receive<F: Fn() -> anyhow::Result<()>>(
    worker: &Worker<'_>,
    check: &F,
) -> anyhow::Result<ParsedBatch> {
    loop {
        check()?;
        match worker
            .output
            .as_ref()
            .expect("worker output open")
            .recv_timeout(Duration::from_millis(50))
        {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("graph parser worker stopped before its ordered result")
            }
        }
    }
}

pub(super) fn scan<R: BufRead, F: Fn() -> anyhow::Result<()>>(
    reader: &mut R,
    path: &str,
    need_deps: bool,
    config: Config,
    check: &F,
) -> anyhow::Result<GraphRows> {
    scan_with_parser(reader, path, need_deps, config, check, parse_batch)
}

fn scan_with_parser<R: BufRead, F: Fn() -> anyhow::Result<()>, P>(
    reader: &mut R,
    path: &str,
    need_deps: bool,
    config: Config,
    check: &F,
    parse: P,
) -> anyhow::Result<GraphRows>
where
    P: Fn(RawBatch, bool, &AtomicBool) -> anyhow::Result<ParsedBatch> + Sync,
{
    anyhow::ensure!(
        config.workers > 0 && config.workers <= 4 && config.bytes > 0 && config.lines > 0,
        "invalid graph parser batch configuration"
    );
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(config.workers);
        for index in 0..config.workers {
            // Rendezvous input plus a single output slot. The coordinator
            // consumes the prior result before assigning this worker again.
            let (input, jobs) = sync_channel::<RawBatch>(0);
            let (results, output) = sync_channel(1);
            let stop = &stop;
            let parse = &parse;
            let join = std::thread::Builder::new()
                .name(format!("retrace-graph-{index}"))
                .spawn_scoped(scope, move || {
                    while let Ok(raw) = jobs.recv() {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        if results.send(parse(raw, need_deps, stop)).is_err() {
                            break;
                        }
                    }
                })
                .context("starting graph parser worker")?;
            workers.push(Worker {
                input: Some(input),
                output: Some(output),
                join: Some(join),
            });
        }
        let result = (|| {
            let mut out = GraphRows::new();
            let mut sent = 0usize;
            let mut consumed = 0usize;
            loop {
                // Never retain more than config.workers unmerged jobs. Read
                // the next raw batch only after making a free slot.
                if sent - consumed == config.workers {
                    let batch = receive(&workers[consumed % config.workers], check)?;
                    merge_batch(batch, &mut out, path, need_deps, check)?;
                    consumed += 1;
                }
                let Some(raw) = read_batch(reader, config, check)? else {
                    break;
                };
                let last = raw.read_error.is_some();
                workers[sent % config.workers]
                    .input
                    .as_ref()
                    .expect("worker input open")
                    .send(raw)
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "graph parser worker stopped before receiving its ordered input"
                        )
                    })?;
                sent += 1;
                if last {
                    break;
                }
            }
            while consumed < sent {
                let batch = receive(&workers[consumed % config.workers], check)?;
                merge_batch(batch, &mut out, path, need_deps, check)?;
                consumed += 1;
            }
            Ok(out)
        })();
        stop.store(true, Ordering::Relaxed);
        // Close EVERY endpoint before joining: early schema/read/cancel
        // errors must not leave another worker blocked in result.send().
        for worker in &mut workers {
            worker.input.take();
            worker.output.take();
        }
        let mut worker_panicked = false;
        for worker in &mut workers {
            worker_panicked |= worker
                .join
                .take()
                .expect("worker joined once")
                .join()
                .is_err();
        }
        // Preserve an earlier ordered refusal. An unwinding worker panic
        // can never turn otherwise successful input into clean EOF. As with
        // the rest of this binary, panic=abort terminates the process.
        if result.is_ok() && worker_panicked {
            anyhow::bail!("graph parser worker panicked");
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::Cursor;
    use std::io::Read;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn shown(rows: &GraphRows) -> String {
        let mut index: Vec<_> = rows.it.index.iter().collect();
        index.sort_unstable();
        format!(
            "{:?}|{index:?}|{:?}|{:?}|{}|{}|{}",
            rows.it.targets,
            rows.rows,
            rows.meta,
            rows.dropped_interior,
            rows.pending_drops,
            rows.bad_rows
        )
    }

    fn compare(body: &[u8], need_deps: bool) {
        let ordinary =
            scan_rows_serial(&mut Cursor::new(body), "test-graph", need_deps, &|| Ok(()));
        for workers in [1, 2, 3, 4] {
            for (bytes, lines) in [(1, 1), (53, 3), (4096, 4096)] {
                let actual = scan(
                    &mut Cursor::new(body),
                    "test-graph",
                    need_deps,
                    Config {
                        workers,
                        bytes,
                        lines,
                    },
                    &|| Ok(()),
                );
                match (&ordinary, actual) {
                    (Ok(a), Ok(b)) => {
                        assert_eq!(shown(a), shown(&b), "workers={workers} bytes={bytes}")
                    }
                    (Err(a), Err(b)) => assert_eq!(format!("{a:#}"), format!("{b:#}")),
                    pair => panic!(
                        "parallel graph scanner changed admission: {} vs {}",
                        pair.0.is_ok(),
                        pair.1.is_ok()
                    ),
                }
            }
        }
    }

    #[test]
    fn local_interners_preserve_prefix_duplicates_rollback_and_drop_runs() {
        let cases: &[&[u8]] = &[
            b"",
            b" \t\n\x0b\x0c\r\n{broken}\n",
            br#"{"t":"before-header","d":["before-dep"]}
{"t":"header-prefix","d":["header-dep"],"graph":{"enc":2,"run":"r"}}
{"t":"discarded-target","t":"actual","d":["discarded-dep"],"d":["shared"],"o":["old-o"],"o":["shared","new-o"],"j":1,"j":false}
{"d":["no-target"]}
{"t":"footer-prefix","graph_end":{},"d":["ignored-after-footer"]}
{"t":"rolled-back","d":["bad-shape",3]}
{"t":"malformed","d":["bad-json"]} x
{}
{"t":"shared","d":[]}
{"t":"unicode\u00e9","d":["quote\"","slash\\","\ud83d\ude00"]}
{"t":"torn"
"#,
            b"{bad}\n{\"graph\":{\"enc\":2}}\n{bad}\n{\"t\":3}\n{bad}\n\n",
            b"{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[]}\n{\"graph\":{\"enc\":2}}\n",
            b"{\"graph\":{\"enc\":1}}\n{bad}\n",
            b"{\"graph\":{\"enc\":2,\"kind\":\"cp_chain\"}}\n",
            b"{\"graph\":{\"enc\":2}}\n{\"t\":\"\xff\"}\n{\"t\":\"a\"}\n",
        ];
        for case in cases {
            compare(case, true);
            compare(case, false);
        }
        let rows = scan(
            &mut Cursor::new(cases[2]),
            "test-graph",
            true,
            Config {
                workers: 2,
                bytes: 53,
                lines: 3,
            },
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(
            rows.it.targets[..4]
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<&str>>(),
            ["before-header", "before-dep", "header-prefix", "header-dep"]
        );
        for retained in [
            "discarded-target",
            "discarded-dep",
            "old-o",
            "no-target",
            "footer-prefix",
        ] {
            assert!(
                rows.it.index.contains_key(retained),
                "lost retained prefix {retained}"
            );
        }
        for rejected in [
            "rolled-back",
            "bad-shape",
            "malformed",
            "bad-json",
            "ignored-after-footer",
        ] {
            assert!(
                !rows.it.index.contains_key(rejected),
                "leaked rejected name {rejected}"
            );
        }
    }

    #[test]
    fn parallel_graph_preserves_full_indexes_cycles_and_refusals() {
        let path = crate::temp_path("parallel-graph.jsonl");
        let p = path.to_str().unwrap();
        let cases = [
            "{\"t\":\"a\",\"d\":[\"x\"],\"graph\":{\"enc\":2}}\n{\"t\":\"b\"}\n{\"t\":\"c\",\"graph_end\":1}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"b\",\"b\"]}\n{\"t\":\"a\",\"o\":[\"tset:x\"]}\n{\"t\":\"b\"}\n{\"t\":\"tset:x\",\"d\":[\"b\"],\"j\":1}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"a\"]}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"ghost\"]}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"a\",\"d\":[\"ghost\"]}\n{\"t\":",
            "{\"t\":\"head-lost\"}\n",
        ];
        for body in cases {
            std::fs::write(&path, body).unwrap();
            for need_deps in [false, true] {
                for repair_cycles in [false, true] {
                    let opts = GraphLoadOpts {
                        need_deps,
                        quiet: true,
                        repair_cycles,
                        max_input_bytes: crate::parse::DEFAULT_MAX_INPUT_BYTES,
                    };
                    let expected = load_graph_with_workers(p, opts, || Ok(()), 0);
                    for workers in [2, 3, 4] {
                        let actual = load_graph_with_workers(p, opts, || Ok(()), workers);
                        match (&expected, actual) {
                            (Ok(a), Ok(mut b)) => {
                                b.load_ms = a.load_ms;
                                assert_eq!(a, &b);
                            }
                            (Err(a), Err(b)) => assert_eq!(format!("{a:#}"), format!("{b:#}")),
                            pair => panic!(
                                "full graph changed admission: {} vs {}",
                                pair.0.is_ok(),
                                pair.1.is_ok()
                            ),
                        }
                    }
                }
            }
        }
        std::fs::write(&path, cases[0]).expect("should write header/footer classification fixture");
        let graph = load_graph_with_workers(p, GraphLoadOpts::default(), || Ok(()), 0)
            .expect("should load the sole action row");
        assert_eq!(graph.targets.len(), 1);
        assert_eq!(&*graph.targets[0], "b");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn raw_batches_keep_oversized_and_unterminated_lines_intact() {
        let config = Config {
            workers: 2,
            bytes: 8,
            lines: 2,
        };
        let mut reader = Cursor::new(b"12345678901234567890\nx\ny\nunterminated");
        let first = read_batch(&mut reader, config, &|| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(first.bytes, b"12345678901234567890\n");
        assert_eq!(first.ends.len(), 1);
        let second = read_batch(&mut reader, config, &|| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(second.bytes, b"x\ny\n");
        assert_eq!(second.ends.len(), 2);
        let third = read_batch(&mut reader, config, &|| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(third.bytes, b"unterminated");
        assert!(
            read_batch(&mut reader, config, &|| Ok(()))
                .unwrap()
                .is_none()
        );
    }

    struct ErrorAtEnd(Cursor<Vec<u8>>);
    impl Read for ErrorAtEnd {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let buffer = self.fill_buf()?;
            let n = out.len().min(buffer.len());
            out[..n].copy_from_slice(&buffer[..n]);
            self.consume(n);
            Ok(n)
        }
    }
    impl BufRead for ErrorAtEnd {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            if self.0.position() as usize == self.0.get_ref().len() {
                Err(std::io::Error::other("injected graph read failure"))
            } else {
                self.0.fill_buf()
            }
        }
        fn consume(&mut self, n: usize) {
            self.0.consume(n);
        }
    }

    #[test]
    fn ordered_schema_failure_precedes_later_read_failure_and_partial_line_is_ignored() {
        for body in [
            "{\"graph\":{\"enc\":1}}\n{\"t\":\"later\"}\n",
            "{\"graph\":{\"enc\":2}}\n{\"t\":\"ok\"}\n",
            "{\"graph\":{\"enc\":2}}\n{\"graph\":{\"enc\":1}}",
        ] {
            let reader = || ErrorAtEnd(Cursor::new(body.as_bytes().to_vec()));
            let expected = scan_rows_serial(&mut reader(), "read-failure", true, &|| Ok(()))
                .err()
                .unwrap();
            for lines in [1, 16] {
                let actual = scan(
                    &mut reader(),
                    "read-failure",
                    true,
                    Config {
                        workers: 4,
                        bytes: 4096,
                        lines,
                    },
                    &|| Ok(()),
                )
                .err()
                .unwrap();
                assert_eq!(format!("{expected:#}"), format!("{actual:#}"));
            }
        }
    }

    #[test]
    fn out_of_order_workers_are_merged_in_input_order() {
        let second_finished = AtomicBool::new(false);
        let body = b"{\"graph\":{\"enc\":2}}\n{\"t\":\"second\"}\n{\"t\":\"third\"}\n";
        let got = scan_with_parser(
            &mut Cursor::new(body),
            "ordered",
            true,
            Config {
                workers: 2,
                bytes: 4096,
                lines: 1,
            },
            &|| Ok(()),
            |raw, need_deps, stop| {
                if raw.bytes.starts_with(b"{\"graph\"") {
                    let limit = Instant::now() + Duration::from_secs(5);
                    while !second_finished.load(Ordering::Acquire) {
                        anyhow::ensure!(Instant::now() < limit, "second worker never ran");
                        std::thread::yield_now();
                    }
                    parse_batch(raw, need_deps, stop)
                } else {
                    let result = parse_batch(raw, need_deps, stop);
                    second_finished.store(true, Ordering::Release);
                    result
                }
            },
        )
        .unwrap();
        let expected =
            scan_rows_serial(&mut Cursor::new(body), "ordered", true, &|| Ok(())).unwrap();
        assert_eq!(shown(&expected), shown(&got));
    }

    #[test]
    fn early_failure_cancellation_and_worker_error_close_all_endpoints() {
        let body = format!(
            "{{\"graph\":{{\"enc\":2}}}}\n{}",
            "{\"t\":\"a\"}\n".repeat(64)
        );
        let checks = Cell::new(0);
        let error = scan(
            &mut Cursor::new(body.as_bytes()),
            "cancel",
            true,
            Config {
                workers: 4,
                bytes: 32,
                lines: 1,
            },
            &|| {
                checks.set(checks.get() + 1);
                anyhow::ensure!(checks.get() < 12, "injected cancellation");
                Ok(())
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "injected cancellation");
        let parsed = AtomicUsize::new(0);
        let failed = scan_with_parser(
            &mut Cursor::new(body.as_bytes()),
            "worker",
            true,
            Config {
                workers: 4,
                bytes: 32,
                lines: 1,
            },
            &|| Ok(()),
            |_, _, _| {
                parsed.fetch_add(1, Ordering::Relaxed);
                anyhow::bail!("injected parser failure")
            },
        )
        .err()
        .unwrap();
        assert_eq!(failed.to_string(), "injected parser failure");
        assert!(parsed.load(Ordering::Relaxed) > 0);
        let body = b"{\"graph\":{\"enc\":1}}\n{\"t\":\"later\"}\n";
        let failed = scan_with_parser(
            &mut Cursor::new(body),
            "earlier",
            true,
            Config {
                workers: 2,
                bytes: 4096,
                lines: 1,
            },
            &|| Ok(()),
            |raw, need_deps, stop| {
                if raw.bytes.starts_with(b"{\"graph\"") {
                    parse_batch(raw, need_deps, stop)
                } else {
                    anyhow::bail!("later parser failure")
                }
            },
        )
        .err()
        .unwrap();
        assert!(failed.to_string().contains("header carries enc 1"));
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn unwinding_worker_panic_is_not_eof_or_a_replacement_for_an_earlier_refusal() {
        for enc in [1, 2] {
            let body = format!("{{\"graph\":{{\"enc\":{enc}}}}}\n{{\"t\":\"later\"}}\n");
            let error = scan_with_parser(
                &mut Cursor::new(body.as_bytes()),
                "worker-panic",
                true,
                Config {
                    workers: 2,
                    bytes: 4096,
                    lines: 1,
                },
                &|| Ok(()),
                |raw, need_deps, stop| {
                    if raw.bytes.starts_with(b"{\"graph\"") {
                        parse_batch(raw, need_deps, stop)
                    } else {
                        panic!("injected graph parser panic");
                    }
                },
            )
            .err()
            .unwrap();
            if enc == 1 {
                assert!(error.to_string().contains("header carries enc 1"));
            } else {
                assert!(error.to_string().contains("graph parser worker stopped"));
            }
        }
    }
}
