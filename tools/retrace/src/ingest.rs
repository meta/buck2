/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Build-log ingestion: a buck2 event log (or just its trace id) in,
//! `-trace.json`, `-graph.json`, and `-execution.bin` artifacts on disk out.
//!
//! This replaces the external converter step (`buck2 log show |
//! trace-buck.py`): the log is decoded natively via `buck2_event_log`
//! and the writer/reader pair now lives in one diff stack, so an
//! encoding change is a co-edit instead of a cross-repo skew (the
//! failure mode that motivated this module).  Writing durable artifacts
//! to disk is deliberate — buck2's own logs expire
//! within days (locally only the newest few are kept; the uploaded
//! copy has a TTL), while the emitted artifacts are the long-term record a
//! later analysis or batch tool reads. Explicit ingestion always exports;
//! viewer auto-open can reuse a completed conversion after validating its
//! invocation header, source identity, options, and all three artifacts.
//!
//! Publication: every output is written to a uniquely named staging
//! file beside its final name. After all three are complete, publish
//! execution evidence first, then graph, then trace. A pre-publication
//! refusal removes its staged files. Publication is not one atomic step:
//! an I/O failure or concurrent reader can observe a partial generation.
//! The graph/trace capture stamp (`graph::check_pair`) and execution-store
//! identity checks refuse mismatched artifacts. Writers of the same command
//! hold one cross-process lock through receipt publication.

#[cfg(test)]
mod action_projection;
mod cache;
mod compact;
mod decode_pool;
pub mod emit;
mod heap_size;
mod normalized;
mod profile;
pub mod scan;
pub mod testlog;

use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use buck2_event_log::read::EventLogPathBuf;

pub(super) fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn create_output_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// The file remains in place: unlinking a lock file would let later writers
/// acquire another inode while an earlier process still owns the first lock.
struct WriterLock(std::fs::File);

impl WriterLock {
    fn acquire(
        out_dir: &Path,
        id: &str,
        purpose: &str,
        cancel: Option<&AtomicBool>,
    ) -> anyhow::Result<Self> {
        let file = Self::open(out_dir, id, purpose)?;
        let mut told = false;
        loop {
            check_cancel(cancel)?;
            match file.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    // The window shows its own progress; the CLI would otherwise look hung.
                    if cancel.is_none() && !told {
                        eprintln!(
                            "retrace: waiting for another retrace to finish with {id} ({purpose})"
                        );
                        told = true;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        Ok(Self(file))
    }

    /// Like [`Self::acquire`], but `None` at once if another process holds the lock.
    fn try_acquire(out_dir: &Path, id: &str, purpose: &str) -> Option<Self> {
        let file = Self::open(out_dir, id, purpose).ok()?;
        file.try_lock().ok()?;
        Some(Self(file))
    }

    fn open(out_dir: &Path, id: &str, purpose: &str) -> anyhow::Result<std::fs::File> {
        anyhow::ensure!(is_uuid(id), "invalid command UUID");
        create_output_dir(out_dir).map_err(|e| anyhow::anyhow!("{}: {e}", out_dir.display()))?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        Ok(options.open(out_dir.join(format!("{}-{purpose}.lock", id.to_ascii_lowercase())))?)
    }
}

pub(super) fn check_cancel(cancel: Option<&AtomicBool>) -> anyhow::Result<()> {
    anyhow::ensure!(
        !cancel.is_some_and(|c| c.load(Ordering::Relaxed)),
        "ingestion cancelled"
    );
    Ok(())
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        drop(self.0.unlock());
    }
}

/// Removes the staging files of builds no running retrace holds, once per process. Otherwise the
/// half-written download or conversion of a build stays until that build is opened again, and a
/// half-downloaded log can be gigabytes.
fn sweep_abandoned_staging(out_dir: &Path) {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| sweep_abandoned_staging_now(out_dir));
}

fn sweep_abandoned_staging_now(out_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(out_dir) else {
        return;
    };
    let ids: std::collections::BTreeSet<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".partial").and(name.get(..36))?;
            is_uuid(id).then(|| id.to_ascii_lowercase())
        })
        .collect();
    for id in ids {
        let (Some(_ingest), Some(_download)) = (
            WriterLock::try_acquire(out_dir, &id, "ingest"),
            WriterLock::try_acquire(out_dir, &id, "download"),
        ) else {
            continue;
        };
        remove_orphaned_staging(out_dir, &id);
        sweep_staging(out_dir, &download_staging_prefixes(&id));
    }
}

fn download_staging_prefixes(id: &str) -> [String; 2] {
    [format!("{id}.pb.zst."), format!("{id}-download.stderr.")]
}

fn remove_orphaned_staging(out_dir: &Path, id: &str) {
    let prefixes = [
        "trace.json",
        "graph.json",
        "execution.bin",
        "ingest-cache.json",
    ]
    .map(|name| format!("{id}-{name}."));
    sweep_staging(out_dir, &prefixes);
}

fn sweep_staging(out_dir: &Path, prefixes: &[String]) {
    let Ok(entries) = std::fs::read_dir(out_dir) else {
        return;
    };
    let mut removed = 0u64;
    let mut bytes = 0u64;
    // The per-key writer lock excludes live conversions for these exact
    // artifact names. Never traverse subdirectories or remove other files.
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(".partial")
            && prefixes.iter().any(|prefix| name.starts_with(prefix))
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            let size = entry.metadata().map_or(0, |m| m.len());
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
                bytes = bytes.saturating_add(size);
            }
        }
    }
    if removed != 0 {
        eprintln!(
            "retrace: removed {removed} abandoned staging file(s), {bytes} bytes; generated partial artifacts are not recoverable"
        );
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum HeapMode {
    #[cfg(test)]
    Reference,
    Fast,
    #[cfg(test)]
    Verify,
}

impl HeapMode {
    #[cfg(test)]
    fn bytes(
        self,
        item: &buck2_error::Result<buck2_event_log::stream_value::StreamValue>,
    ) -> usize {
        let reference = || match item {
            Ok(value) => allocative::size_of_unique_allocated_data(value),
            Err(error) => allocative::size_of_reachable(error)
                .saturating_sub(std::mem::size_of::<buck2_error::Error>()),
        };
        match self {
            Self::Reference => reference(),
            Self::Fast => heap_size::item_heap_bytes(item),
            Self::Verify => {
                let fast = heap_size::item_heap_bytes(item);
                assert_eq!(fast, reference(), "fast decoded heap accounting differs");
                fast
            }
        }
    }
}

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);
static PROCESS_START_NANOS: OnceLock<u128> = OnceLock::new();

/// An output written beside where it will be published.  Dropped
/// unpublished (any early return, a refusal included), it removes what
/// was written.  The staging name carries the process id, the process
/// start time and a per-process sequence number, so two invocations in
/// one process, two processes, and a reused pid against a leftover
/// file never share a name.
pub(crate) struct Staged {
    pub(crate) write_to: PathBuf,
    pub(crate) publish_at: PathBuf,
    published: bool,
}

impl Staged {
    pub(crate) fn beside(publish_at: PathBuf) -> Staged {
        let start = PROCESS_START_NANOS.get_or_init(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        });
        let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = publish_at
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let write_to = publish_at.with_file_name(format!(
            "{name}.{}-{start}-{seq}.partial",
            std::process::id()
        ));
        Staged {
            write_to,
            publish_at,
            published: false,
        }
    }

    /// Moves the finished file to its final name (one rename).
    pub(crate) fn publish(mut self) -> anyhow::Result<PathBuf> {
        std::fs::rename(&self.write_to, &self.publish_at)
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.publish_at.display()))?;
        self.published = true;
        Ok(self.publish_at.clone())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.published {
            std::fs::remove_file(&self.write_to).ok();
        }
    }
}

/// Where the log comes from.
pub enum Source {
    /// An event-log file (`.pb.zst` and friends, per buck2's own
    /// extension inference).
    Path(PathBuf),
    /// A build's trace id (the UUID buck2 prints); resolved against
    /// the local log dir first, then downloaded.
    TraceId(String),
    /// The newest log in the local log dir.
    Last,
}

/// Recognize an argument that names a log rather than a trace file:
/// a UUID, or an existing file with a known event-log extension.
pub fn classify_source(arg: &str) -> Option<Source> {
    if is_uuid(arg) {
        return Some(Source::TraceId(arg.to_owned()));
    }
    let p = Path::new(arg);
    let name = p.file_name()?.to_str()?;
    const LOG_EXTENSIONS: [&str; 6] = [
        ".pb.zst",
        ".pb.gz",
        ".pb",
        ".proto",
        ".json-lines.zst",
        ".json-lines.gz",
    ];
    if LOG_EXTENSIONS.iter().any(|e| name.ends_with(e)) || name.ends_with(".json-lines") {
        return Some(Source::Path(p.to_owned()));
    }
    None
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The emitted pair plus what to tell the user.
#[derive(Debug)]
pub struct IngestOutput {
    pub trace_path: PathBuf,
    /// where the graph sidecar is published; the file exists only when
    /// this ingest produced a graph
    pub graph_path: PathBuf,
    pub status_lines: Vec<String>,
    pub warnings: Vec<String>,
    pub outcome: crate::types::Outcome,
    pub dep_capture: crate::types::CaptureState,
}

/// Resolve the source, decode, and write `<trace_id>-trace.json` +
/// `<trace_id>-graph.json` under `out_dir`.
///
/// `from` is where the search for this checkout's logs starts, and it is
/// an argument rather than the process's working directory because the
/// two are only the same thing for a command line. See [`project_root`].
pub fn run(
    source: &Source,
    out_dir: &Path,
    from: &Path,
    phases: bool,
) -> anyhow::Result<IngestOutput> {
    run_with_cache(source, out_dir, from, phases, false, None)
}

/// Viewer auto-open: inspect only the invocation header before looking for
/// an unchanged, previously completed conversion. Explicit `run` remains a
/// force-export, and also records a receipt for a subsequent viewer open.
pub fn run_cached(
    source: &Source,
    out_dir: &Path,
    from: &Path,
    phases: bool,
) -> anyhow::Result<IngestOutput> {
    run_with_cache(source, out_dir, from, phases, true, None)
}

pub fn run_cached_cancellable(
    source: &Source,
    out_dir: &Path,
    from: &Path,
    phases: bool,
    cancel: &AtomicBool,
) -> anyhow::Result<IngestOutput> {
    run_with_cache(source, out_dir, from, phases, true, Some(cancel))
}

fn run_with_cache(
    source: &Source,
    out_dir: &Path,
    from: &Path,
    phases: bool,
    reuse: bool,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<IngestOutput> {
    check_cancel(cancel)?;
    sweep_abandoned_staging(out_dir);
    let mut writer_lock = match source {
        Source::TraceId(id) => Some(WriterLock::acquire(out_dir, id, "ingest", cancel)?),
        _ => None,
    };
    if let Source::TraceId(id) = source {
        remove_orphaned_staging(out_dir, &id.to_ascii_lowercase());
    }
    if reuse {
        if let Source::TraceId(id) = source {
            let cached = cache::lookup_id(id, out_dir, phases, cancel);
            check_cancel(cancel)?;
            if let Some(out) = cached.ok().flatten() {
                return Ok(out);
            }
        }
    }
    let log = resolve(source, out_dir, from, cancel)?;
    // A failed cache probe must not prevent normal ingestion. In particular,
    // a missing receipt from an older version is not proof of an equivalent
    // capture merely because the output names contain the same build UUID.
    let id = trace_id(&log).map_err(|e| {
        anyhow::anyhow!(
            "{}: cannot read the invocation header; the log may be truncated before its \
         first complete record or use an unsupported encoding: {e:#}",
            log.path().display()
        )
    })?;
    if let Source::TraceId(requested) = source {
        anyhow::ensure!(
            id.eq_ignore_ascii_case(requested),
            "requested build {requested}, but the event log contains build {id}"
        );
    }
    if writer_lock.is_none() {
        writer_lock = Some(WriterLock::acquire(out_dir, &id, "ingest", cancel)?);
    }
    let _writer_lock = writer_lock;
    // Re-read identities and receipts after acquiring the cross-process lock.
    let request = cache::Request::read_for_locked_run(&log, &id, cancel)?;
    remove_orphaned_staging(out_dir, &id);
    let cache_miss = if reuse && !cache::has_receipt(out_dir, &id) {
        eprintln!(
            "retrace: converting {} (first time this log has been opened)",
            log.path().display(),
        );
        None
    } else if reuse {
        let cached = request
            .as_ref()
            .map(|r| cache::lookup(r, out_dir, phases, cancel));
        check_cancel(cancel)?;
        let reason = match cached {
            Some(Ok(Some(out))) => return Ok(out),
            Some(Err(error)) => format!("{error:#}"),
            _ => "source identity unavailable or no reusable receipt".to_owned(),
        };
        eprintln!(
            "retrace: converting {} (cache miss: {reason})",
            log.path().display(),
        );
        Some(reason)
    } else {
        None
    };
    // The lock, staging names, scan identity and publication must refer to
    // the same command even when the optional cache probe fails.
    let source_run = id;
    let execution = Staged::beside(crate::execution::store_path(out_dir, &source_run)?);
    let mut execution_writer = crate::execution::DetailWriter::create(&execution.write_to)?;
    let scanned = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(scan::scan_with_details_cancellable(
            &log,
            phases,
            &mut execution_writer,
            cancel,
        ))?;
    anyhow::ensure!(
        scanned.trace_id == source_run,
        "event log was replaced during conversion"
    );
    let execution_info = execution_writer.finish()?;
    // The scan reopens the path after the lightweight header probe. A
    // replacement in between must not return another build to a UUID caller.
    if let Source::TraceId(id) = source {
        anyhow::ensure!(
            scanned.trace_id.eq_ignore_ascii_case(id),
            "requested build {id}, but the event log contains build {}",
            scanned.trace_id
        );
    }

    // A paused writer must not turn a partial read into a permanent cache
    // hit. A failed build with a complete Result is still cacheable.
    let cacheable = scanned.incomplete.is_none() && scanned.command_result_seen;
    let trace_id = scanned.trace_id.clone();

    let trace = Staged::beside(out_dir.join(format!("{}-trace.json", scanned.trace_id)));
    let graph = Staged::beside(out_dir.join(format!("{}-graph.json", scanned.trace_id)));
    let graph_path = graph.publish_at.clone();
    let incomplete = scanned.incomplete.clone();
    check_cancel(cancel)?;
    let summary =
        emit::emit_with_execution(scanned, &trace, Some(&graph), phases, Some(&execution_info))?;
    let emitted_capture = summary.capture;
    check_cancel(cancel)?;
    let mut status_lines = summary.status_lines;
    let mut warnings = summary.warnings;
    crate::execution::stamp(
        &execution.write_to,
        &trace_id,
        &emitted_capture,
        &execution_info,
    )?;
    let execution_path = execution.publish()?;
    status_lines.push(format!(
        "{}: recorded command/output evidence ({} bytes; read on demand)",
        execution_path.display(),
        execution_info.bytes
    ));
    // The graph side settles first so that, once the trace is visible,
    // the sidecar beside it is this ingest's.  A reader that opened the
    // previous trace before this point can still meet the new sidecar;
    // the capture stamp in both headers is what refuses that pair.
    let remove_graph = matches!(summary.graph, emit::GraphOutcome::NotWritten);
    match summary.graph {
        emit::GraphOutcome::Written => {
            graph.publish()?;
        }
        emit::GraphOutcome::NotWritten => {
            drop(graph);
        }
    }
    let trace_path = trace.publish()?;
    if remove_graph {
        match std::fs::remove_file(&graph_path) {
            Ok(()) => status_lines.push(format!(
                "{}: removed; it belonged to an earlier ingest of this build and this \
                     one produced no graph",
                graph_path.display()
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::anyhow!("{}: {e}", graph_path.display())),
        }
    }
    match incomplete {
        None => {}
        Some(scan::Incomplete::NoResult) => status_lines.push(
            "note: the log ends early (cancelled or still-running build); \
             counts cover what was recorded"
                .to_owned(),
        ),
        Some(scan::Incomplete::Undecodable { records, error }) => {
            warnings.push(format!(
                "the log stopped decoding {records} records in: {error}"
            ));
            warnings.push(
                "Only the readable prefix is shown. A writer interrupted within a record, \
                 damaged bytes, or an unsupported encoding can produce this error; the \
                 log alone does not establish which occurred."
                    .to_owned(),
            );
        }
    }
    // the console says what the artifacts record
    for p in summary.dep_capture.problems() {
        status_lines.push(format!("capture: {p}"));
    }
    if summary.outcome == crate::types::Outcome::Unknown {
        status_lines.push(
            "command outcome: unknown (the retained completion evidence does not establish \
             success or failure)"
                .to_owned(),
        );
    }
    let mut out = IngestOutput {
        trace_path,
        graph_path,
        status_lines,
        warnings,
        outcome: summary.outcome,
        dep_capture: summary.dep_capture,
    };
    if cacheable {
        if let Some(request) = request.filter(|r| r.trace_id == trace_id) {
            if let Err(e) = cache::record(
                &request,
                out_dir,
                phases,
                &out,
                &emitted_capture,
                &execution_info,
                cancel,
            ) {
                check_cancel(cancel)?;
                // The durable artifacts succeeded. Cache bookkeeping is an
                // optimization and cannot turn that success into a refusal.
                out.warnings.push(format!(
                    "could not record reusable conversion; next open may reconvert: {e:#}"
                ));
            }
        }
    }
    // A cache miss describes this open, not the capture. Keep it visible to
    // callers (including the GUI), but out of the reusable receipt so a later
    // cache hit cannot replay an obsolete conversion reason.
    if let Some(reason) = cache_miss {
        out.status_lines.insert(
            0,
            format!("retrace: generated fresh artifacts (cache miss: {reason})"),
        );
    }
    Ok(out)
}

/// Read only the Invocation frame, before the event stream is decoded. Both
/// execution-store naming and the later cache fast path use this identity.
fn trace_id(log: &EventLogPathBuf) -> anyhow::Result<String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let (invocation, _events) = log
                .unpack_stream()
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))?;
            Ok(invocation.trace_id.to_string())
        })
}

fn resolve(
    source: &Source,
    out_dir: &Path,
    from: &Path,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<EventLogPathBuf> {
    match source {
        Source::Path(p) => infer(p),
        Source::Last => {
            let dir = local_log_dir(from)?;
            let logs = buck2_event_log::file_names::get_local_logs(&dir)
                .map_err(|e| anyhow::anyhow!("{e:#}"))?;
            logs.into_iter().next_back().ok_or_else(|| {
                anyhow::anyhow!("no event logs under {dir}; run a build first, or pass a path")
            })
        }
        Source::TraceId(id) => {
            let trace_id = buck2_wrapper_common::invocation_id::TraceId::from_str(id)
                .map_err(|e| anyhow::anyhow!("{id}: not a trace id ({e:#})"))?;
            if let Ok(dir) = local_log_dir(from) {
                if let Ok(Some(log)) =
                    buck2_event_log::file_names::find_log_by_trace_id(&dir, &trace_id)
                {
                    return Ok(log);
                }
            }
            download(&trace_id.to_string(), out_dir, cancel)
        }
    }
}

fn infer(p: &Path) -> anyhow::Result<EventLogPathBuf> {
    let abs = std::fs::canonicalize(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
    let abs =
        buck2_fs::paths::abs_path::AbsPathBuf::new(abs).map_err(|e| anyhow::anyhow!("{e:#}"))?;
    EventLogPathBuf::infer(abs).map_err(|e| anyhow::anyhow!("{e:#}"))
}

/// The buck project at or above `from` — the directory holding
/// `.buckconfig` — or `None` if `from` is not inside one.
///
/// The starting directory is always the caller's to name. A command line
/// is run inside the checkout it is asking about, so the process's
/// working directory is the right answer there; a window is as likely to
/// have been opened from a desktop icon, where the working directory is
/// neither chosen by nor visible to the person clicking.
pub fn project_root(from: &Path) -> Option<PathBuf> {
    let mut dir = from.to_owned();
    loop {
        if dir.join(".buckconfig").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// `buck-out/v2/log` under the project enclosing `from`.
fn local_log_dir(from: &Path) -> anyhow::Result<buck2_fs::paths::abs_norm_path::AbsNormPathBuf> {
    let root = project_root(from).ok_or_else(|| {
        anyhow::anyhow!(
            "not inside a buck project (no .buckconfig above {}); pass a log path instead",
            from.display()
        )
    })?;
    buck2_fs::paths::abs_norm_path::AbsNormPathBuf::new(root.join("buck-out/v2/log"))
        .map_err(|e| anyhow::anyhow!("{e:#}"))
}

/// Where an ingest puts artifacts when nobody named a directory: a
/// per-user cache directory, never the working directory.
///
/// Both callers that have no `--out=` to honour resolve a log inside a
/// buck project, so their working directory is a repo, and a gigabyte of
/// trace JSON must not land there as a side effect of *viewing* a build.
/// `retrace ingest` is the exception and passes `.` explicitly: an
/// export asked for by name exports where it was invoked.
pub fn default_out_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if Path::new(&x).is_absolute() {
            return Path::new(&x).join("retrace");
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if Path::new(&h).is_absolute() {
            return Path::new(&h).join(".cache").join("retrace");
        }
    }
    // no XDG_CACHE_HOME and no HOME: a temp dir still honors "never the
    // working directory"
    dirs::cache_dir()
        .filter(|p| p.is_absolute())
        .map(|p| p.join("retrace"))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("retrace-{}", std::process::id())))
}

/// One local event log, as [`recent_logs`] lists them.
#[derive(Clone, Debug)]
pub struct RecentLog {
    pub path: PathBuf,
    /// The build's trace id, taken from the file name; empty when the
    /// name does not follow buck2's convention.
    pub trace_id: String,
    /// The buck2 subcommand the log was recorded for (`build`, `test`,
    /// ...); empty alongside an empty `trace_id`.
    pub command: String,
    /// Last write time and size on disk. Both absent if the log was
    /// rotated away between the listing and the stat.
    pub modified: Option<std::time::SystemTime>,
    pub bytes: u64,
}

/// The local event logs, newest first — the same set `buck2 log path
/// --all` reports.
///
/// Read straight off the log directory rather than asked of the daemon:
/// buck2 keeps only a handful of local logs, so the listing is short,
/// and a viewer must not have to start a build daemon to offer a menu.
///
/// `from` is where the search for the checkout starts; see
/// [`project_root`] for why that is the caller's to say.
pub fn recent_logs(limit: usize, from: &Path) -> anyhow::Result<Vec<RecentLog>> {
    let dir = local_log_dir(from)?;
    let logs =
        buck2_event_log::file_names::get_local_logs(&dir).map_err(|e| anyhow::anyhow!("{e:#}"))?;
    Ok(logs
        .iter()
        .rev()
        .take(limit)
        .map(|log| {
            let path = log.path().as_path().to_owned();
            // `<stamp>_<command>_<uuid>_events.<ext>`; anything else
            // still lists, just without the two parsed fields
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let mut parts = name.split('_');
            let (command, trace_id) = match (parts.next(), parts.next(), parts.next()) {
                (Some(_), Some(cmd), Some(id)) if is_uuid(id) => (cmd.to_owned(), id.to_owned()),
                _ => (String::new(), String::new()),
            };
            let meta = std::fs::metadata(&path).ok();
            RecentLog {
                trace_id,
                command,
                modified: meta.as_ref().and_then(|m| m.modified().ok()),
                bytes: meta.as_ref().map_or(0, |m| m.len()),
                path,
            }
        })
        .collect())
}

/// Fetch an uploaded log by trace id (the same store `buck2 log show
/// --trace-id` uses).  Kept as a subprocess on purpose: the manifold
/// CLI owns auth, and linking buck2's client context for twenty lines
/// is not worth the dependency.
#[cfg(not(fbcode_build))]
fn download(
    id: &str,
    _out_dir: &Path,
    _cancel: Option<&AtomicBool>,
) -> anyhow::Result<EventLogPathBuf> {
    anyhow::bail!(
        "Command ID {id} has no local event log. Remote log retrieval is available only in Meta builds; pass an event-log path instead."
    )
}

#[cfg(any(fbcode_build, all(test, unix)))]
struct DownloadLimits {
    duration: std::time::Duration,
    bytes: u64,
    diagnostics: u64,
}

/// Own and reap the entire child lifetime. Bounds are checked before and after
/// reaping, so a child that exits between polls cannot bypass its output limit.
#[cfg(any(fbcode_build, all(test, unix)))]
fn wait_download(
    mut command: std::process::Command,
    destination: &Path,
    diagnostic: &Path,
    cancel: Option<&AtomicBool>,
    limits: DownloadLimits,
) -> anyhow::Result<std::process::ExitStatus> {
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use anyhow::Context as _;

    check_cancel(cancel)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    struct DownloadChild {
        child: std::process::Child,
        running: bool,
    }
    impl Drop for DownloadChild {
        fn drop(&mut self) {
            if !self.running {
                return;
            }
            #[cfg(unix)]
            drop(
                Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", self.child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status(),
            );
            #[cfg(windows)]
            drop(
                Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &self.child.id().to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status(),
            );
            drop(self.child.kill());
            drop(self.child.wait());
        }
    }
    let mut child = DownloadChild {
        child: command.spawn().context("starting event-log downloader")?,
        running: true,
    };
    // Declared after `child`, so it is dropped first: the slot is freed before the child is reaped.
    let _tracked = crate::interrupt::TrackedChild::track(child.child.id());
    let started = Instant::now();
    let check_bounds = || -> anyhow::Result<()> {
        check_cancel(cancel)?;
        anyhow::ensure!(
            started.elapsed() < limits.duration,
            "log download exceeded its time limit ({:?})",
            limits.duration
        );
        anyhow::ensure!(
            std::fs::metadata(destination)?.len() <= limits.bytes,
            "log download exceeded its {} byte limit; download it manually and pass its path",
            limits.bytes
        );
        anyhow::ensure!(
            std::fs::metadata(diagnostic)?.len() <= limits.diagnostics,
            "log downloader exceeded its diagnostic-output limit"
        );
        Ok(())
    };
    loop {
        check_bounds()?;
        if let Some(status) = child.child.try_wait()? {
            child.running = false;
            check_bounds()?;
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(fbcode_build)]
fn download(
    id: &str,
    out_dir: &Path,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<EventLogPathBuf> {
    use std::io::Read as _;
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;

    const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024 * 1024;
    const MAX_DIAGNOSTIC_BYTES: u64 = 256 * 1024;
    let _download_lock = WriterLock::acquire(out_dir, id, "download", cancel)?;
    sweep_staging(out_dir, &download_staging_prefixes(id));
    let dst = out_dir.join(format!("{id}.pb.zst"));
    if !dst.exists() {
        // Download to a staging name and rename only on success: an
        // interrupted fetch (Ctrl-C, kill) must not leave a torn file
        // at the final path, where every later `retrace <id>` would
        // silently ingest it as a "truncated" log forever.  The manifold
        // CLI opens the destination itself. A separate per-command lock
        // prevents another downloader from publishing concurrently.
        let tmp = Staged::beside(dst.clone());
        let diagnostic = Staged::beside(out_dir.join(format!("{id}-download.stderr")));
        let stderr = private_file(&diagnostic.write_to)?;
        let destination = std::fs::canonicalize(out_dir)?.join(tmp.write_to.file_name().unwrap());
        // Preserve private permissions even for an explicitly named shared
        // output directory. Manifold truncates this existing destination.
        drop(private_file(&destination)?);
        let mut command = Command::new("manifold");
        command
            .args([
                "get",
                "--overwrite_local_path",
                "--",
                &format!("buck2_logs/flat/{id}.pb.zst"),
            ])
            .arg(&destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr);
        let status = wait_download(
            command,
            &destination,
            &diagnostic.write_to,
            cancel,
            DownloadLimits {
                duration: Duration::from_secs(20 * 60),
                bytes: MAX_LOG_BYTES,
                diagnostics: MAX_DIAGNOSTIC_BYTES,
            },
        )?;
        let mut error = String::new();
        std::fs::File::open(&diagnostic.write_to)?
            .take(MAX_DIAGNOSTIC_BYTES)
            .read_to_string(&mut error)?;
        if !status.success() {
            anyhow::bail!(
                "no local log for {id} and the download failed (uploaded logs expire after a \
                 retention window): {}",
                error.trim()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))?;
        }
        tmp.publish()?;
    }
    infer(&dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::testlog::DemoOpts;
    use crate::ingest::testlog::FooterSpec;
    use crate::ingest::testlog::RUN_ID;
    use crate::ingest::testlog::cache_hit_log;
    use crate::ingest::testlog::demo_log;
    use crate::ingest::testlog::demo_log_with;
    use crate::ingest::testlog::many_tsets_log;
    use crate::ingest::testlog::reorder_log;
    use crate::ingest::testlog::torn_log_with_start_stamp;
    use crate::types::CaptureState;
    use crate::types::Finality;
    use crate::types::Outcome;

    /// The walk starts at the directory it is handed, never at the
    /// process's. A window's working directory is not the checkout it is
    /// showing, so a search that used one for the other is right only by
    /// luck.
    #[test]
    fn the_project_root_walk_starts_at_the_directory_it_is_given() {
        let base = std::env::temp_dir().join(format!("retrace-root-{}", std::process::id()));
        let repo = base.join("repo");
        let deep = repo.join("a").join("b");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(repo.join(".buckconfig"), "").unwrap();

        assert_eq!(project_root(&deep).as_deref(), Some(repo.as_path()));
        assert_eq!(project_root(&repo).as_deref(), Some(repo.as_path()));
        assert_ne!(project_root(&outside).as_deref(), Some(repo.as_path()));

        std::fs::remove_dir_all(&base).ok();
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("retrace-ingest-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn startup_sweep_removes_only_partials_no_retrace_holds() {
        let dir = tmp("startup-sweep");
        let held = "11111111-1111-4111-8111-111111111111";
        let free = "22222222-2222-4222-8222-222222222222";
        let partials = |id: &str| {
            [
                dir.join(format!("{id}-trace.json.1-2-3.partial")),
                dir.join(format!("{id}.pb.zst.1-2-3.partial")),
            ]
        };
        for path in partials(held).iter().chain(&partials(free)) {
            std::fs::write(path, b"x").unwrap();
        }
        let published = dir.join(format!("{free}-trace.json"));
        std::fs::write(&published, b"x").unwrap();
        let _held = WriterLock::acquire(&dir, held, "ingest", None).unwrap();
        sweep_abandoned_staging_now(&dir);
        assert!(
            partials(held).iter().all(|p| p.exists()),
            "a live writer's partials stay"
        );
        assert!(
            partials(free).iter().all(|p| !p.exists()),
            "abandoned partials go"
        );
        assert!(published.exists(), "published files are never swept");
    }

    #[test]
    fn output_directory_errors_name_the_path() {
        let dir = tmp("output-directory-error");
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"existing file").expect("should create a blocking file");
        let output = blocked.join("output");
        let error = WriterLock::acquire(&output, RUN_ID, "ingest", None)
            .err()
            .expect("a file cannot be the output directory's parent");
        assert!(error.to_string().contains(&output.display().to_string()));
    }

    #[cfg(not(fbcode_build))]
    #[test]
    fn oss_remote_lookup_is_explicitly_unavailable_without_running_a_child() {
        let dir = tmp("oss-remote-unavailable");
        let error = download(RUN_ID, &dir, None)
            .err()
            .expect("OSS remote download must be unavailable")
            .to_string();
        assert!(error.contains("available only in Meta builds"), "{error}");
        assert!(error.contains("event-log path"), "{error}");
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn downloader_harness_bounds_completion_output_timeout_and_cancellation() {
        use std::process::Command;
        use std::process::Stdio;
        use std::time::Duration;
        use std::time::Instant;

        for (name, script, expected) in [
            ("success", "printf ok", None),
            ("bytes", "printf 123456789", Some("byte limit")),
            (
                "diagnostics",
                "printf 123456789 >&2",
                Some("diagnostic-output limit"),
            ),
            ("timeout", "sleep 30", Some("time limit")),
        ] {
            let dir = tmp(&format!("download-{name}"));
            let destination = dir.join("log");
            let diagnostic = dir.join("stderr");
            let mut command = Command::new("sh");
            command
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(private_file(&destination).unwrap())
                .stderr(private_file(&diagnostic).unwrap());
            let started = Instant::now();
            let result = wait_download(
                command,
                &destination,
                &diagnostic,
                None,
                DownloadLimits {
                    duration: if name == "timeout" {
                        Duration::from_millis(20)
                    } else {
                        Duration::from_secs(10)
                    },
                    bytes: 8,
                    diagnostics: 8,
                },
            );
            match expected {
                Some(message) => assert!(result.unwrap_err().to_string().contains(message)),
                None => assert!(result.unwrap().success()),
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the limits stopped the 30 s child early"
            );
        }
        let dir = tmp("download-cancelled");
        let cancel = AtomicBool::new(true);
        let result = wait_download(
            Command::new("retrace-nonexistent-test-child"),
            &dir.join("absent"),
            &dir.join("absent-stderr"),
            Some(&cancel),
            DownloadLimits {
                duration: Duration::from_secs(1),
                bytes: 1,
                diagnostics: 1,
            },
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn orphan_sweep_removes_only_the_locked_keys_regular_staging_files() {
        let dir = tmp("orphan-sweep");
        let _lock = WriterLock::acquire(&dir, RUN_ID, "ingest", None).unwrap();
        let stale = dir.join(format!("{RUN_ID}-trace.json.dead.partial"));
        let other = dir.join("00000000-0000-0000-0000-000000000000-trace.json.dead.partial");
        let published = dir.join(format!("{RUN_ID}-trace.json"));
        let nested = dir.join(format!("{RUN_ID}-graph.json.directory.partial"));
        for path in [&stale, &other, &published] {
            std::fs::write(path, b"test").unwrap();
        }
        std::fs::create_dir(&nested).unwrap();
        remove_orphaned_staging(&dir, RUN_ID);
        assert!(!stale.exists());
        assert!(other.exists() && published.exists() && nested.is_dir());
    }

    #[test]
    fn a_mismatched_command_id_does_not_sweep_another_writers_staging() {
        let d = tmp("wrong-id-preserves-staging");
        std::fs::write(d.join(".buckconfig"), b"").unwrap();
        let logs = d.join("buck-out/v2/log");
        std::fs::create_dir_all(&logs).unwrap();
        let log = demo_log(&d, Some(1), true, true);
        let requested = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        std::fs::copy(log, logs.join(format!("stamp_build_{requested}_events.pb"))).unwrap();
        let _other_writer = WriterLock::acquire(&d, RUN_ID, "ingest", None).unwrap();
        let live_staging = d.join(format!("{RUN_ID}-trace.json.live.partial"));
        std::fs::write(&live_staging, b"another writer's in-progress trace").unwrap();

        let error = run(&Source::TraceId(requested.to_owned()), &d, &d, false).unwrap_err();
        assert!(error.to_string().contains("requested build"), "{error:#}");
        assert_eq!(
            std::fs::read(&live_staging).unwrap(),
            b"another writer's in-progress trace"
        );
        assert!(!d.join(format!("{requested}-trace.json")).exists());
        assert!(!d.join(format!("{RUN_ID}-trace.json")).exists());
    }

    /// Put the demo's stages below all four display-only phase kinds. The
    /// cancelled child closes normally as evidence; the open child must not
    /// acquire an invented end when its phase ancestors or action finish.
    fn phase_ancestor_log(d: &Path) -> PathBuf {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::span_end_event::Data as End;
        use buck2_data::span_start_event::Data as Start;
        use prost::Message as _;

        fn event(
            us: i32,
            sid: u64,
            parent_id: u64,
            data: Data,
        ) -> buck2_cli_proto::CommandProgress {
            buck2_cli_proto::CommandProgress {
                progress: Some(Progress::Event(Box::new(buck2_data::BuckEvent {
                    timestamp: Some(prost_types::Timestamp {
                        seconds: 1_700_000_000,
                        nanos: us * 1_000,
                    }),
                    trace_id: RUN_ID.to_owned(),
                    span_id: sid,
                    parent_id,
                    data: Some(data),
                }))),
            }
        }
        fn start(data: Start) -> Data {
            Data::SpanStart(buck2_data::SpanStartEvent { data: Some(data) })
        }
        fn end(us: i32, data: End) -> Data {
            Data::SpanEnd(buck2_data::SpanEndEvent {
                duration: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: us * 1_000,
                }),
                data: Some(data),
                ..Default::default()
            })
        }

        let log = demo_log(d, Some(1), true, true);
        let input = std::fs::read(&log).unwrap();
        let mut input = input.as_slice();
        let mut output = buck2_data::Invocation::decode_length_delimited(&mut input)
            .unwrap()
            .encode_length_delimited_to_vec();
        while !input.is_empty() {
            let mut frame =
                buck2_cli_proto::CommandProgress::decode_length_delimited(&mut input).unwrap();
            let mut action_start = false;
            let mut action_end = false;
            if let Some(Progress::Event(e)) = &mut frame.progress {
                if e.span_id == 20 {
                    action_start = matches!(&e.data, Some(Data::SpanStart(_)));
                    action_end = matches!(&e.data, Some(Data::SpanEnd(_)));
                } else if e.parent_id == 20 {
                    e.parent_id = 33;
                }
            }
            if action_end {
                for e in [
                    event(
                        140_000,
                        34,
                        33,
                        start(Start::ExecutorStage(buck2_data::ExecutorStageStart {
                            stage: Some(buck2_data::executor_stage_start::Stage::Re(
                                buck2_data::ReStage {
                                    stage: Some(buck2_data::re_stage::Stage::WorkerDownload(
                                        Default::default(),
                                    )),
                                },
                            )),
                        })),
                    ),
                    event(
                        150_000,
                        34,
                        33,
                        end(10_000, End::SpanCancelled(Default::default())),
                    ),
                    event(151_000, 35, 33, start(Start::ReSubmit(Default::default()))),
                    event(
                        200_000,
                        33,
                        32,
                        end(94_996, End::SpanCancelled(Default::default())),
                    ),
                    event(
                        201_000,
                        32,
                        31,
                        end(95_997, End::Analysis(Default::default())),
                    ),
                    event(
                        202_000,
                        31,
                        30,
                        end(96_998, End::LoadPackage(Default::default())),
                    ),
                    event(203_000, 30, 20, end(97_999, End::Load(Default::default()))),
                ] {
                    e.encode_length_delimited(&mut output).unwrap();
                }
            }
            frame.encode_length_delimited(&mut output).unwrap();
            if action_start {
                for e in [
                    event(
                        105_001,
                        30,
                        20,
                        start(Start::Load(buck2_data::LoadBuildFileStart {
                            module_id: "fbcode//demo/BUCK".to_owned(),
                            ..Default::default()
                        })),
                    ),
                    event(
                        105_002,
                        31,
                        30,
                        start(Start::LoadPackage(buck2_data::LoadPackageStart {
                            path: "fbcode//demo".to_owned(),
                        })),
                    ),
                    event(
                        105_003,
                        32,
                        31,
                        start(Start::Analysis(buck2_data::AnalysisStart {
                            rule: "demo_rule".to_owned(),
                            target: Some(buck2_data::analysis_start::Target::StandardTarget(
                                buck2_data::ConfiguredTargetLabel {
                                    label: Some(buck2_data::TargetLabel {
                                        package: "fbcode//demo".to_owned(),
                                        name: "t2".to_owned(),
                                    }),
                                    ..Default::default()
                                },
                            )),
                        })),
                    ),
                    event(
                        105_004,
                        33,
                        32,
                        start(Start::Materialization(buck2_data::MaterializationStart {
                            action_digest: Some("abc:123".to_owned()),
                        })),
                    ),
                ] {
                    e.encode_length_delimited(&mut output).unwrap();
                }
            }
        }
        std::fs::write(&log, output).unwrap();
        log
    }

    #[test]
    fn optional_phase_projection_preserves_ancestry_and_exact_emission() {
        let d = tmp("optional-phases");
        let log = phase_ancestor_log(&d);
        let path = infer(&log).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let full = runtime.block_on(scan::scan(&path)).unwrap();
        let lean = runtime
            .block_on(scan::scan_with_phases(&path, false))
            .unwrap();
        assert_eq!(full.phases.len(), 4, "public scan keeps all phase kinds");
        assert!(lean.phases.is_empty());
        assert_eq!(full.capture_state(), lean.capture_state());
        assert_eq!(full.actions.len(), lean.actions.len());
        assert_eq!(full.snapshots.len(), lean.snapshots.len());
        assert_eq!(full.tset_def_events_seen, lean.tset_def_events_seen);
        for scanned in [&full, &lean] {
            let stages = &scanned.stages[&20];
            assert_eq!(stages.len(), 3, "the open ReSubmit has no invented end");
            assert_eq!(stages[0].name, "Re.Queue");
            assert_eq!(stages[1].name, "ReUpload");
            assert!(matches!(
                stages[1].details,
                Some(scan::StageDetails::Upload(_))
            ));
            assert_eq!(stages[2].name, "Re.WorkerDownload");
            assert_eq!(stages[2].dur, 10_000);
            assert!(stages[2].cancelled);
            assert!(stages[2].details.is_none());
        }
        let emit_pair = |scanned, phases, tag: &str| {
            let trace = Staged::beside(d.join(format!("{tag}-trace.json")));
            let graph = Staged::beside(d.join(format!("{tag}-graph.json")));
            emit::emit(scanned, &trace, Some(&graph), phases).unwrap();
            (
                std::fs::read(&trace.write_to).unwrap(),
                std::fs::read(&graph.write_to).unwrap(),
            )
        };
        let reference = emit_pair(full, false, "full-no-phases");
        assert_eq!(reference, emit_pair(lean, false, "lean-no-phases"));
        // Public scan/emit intentionally support callers without an execution
        // store. Keep their exact parity oracle above, then compare production
        // against the full three-artifact protocol rather than stripping new
        // references, fingerprints, or graph content from either output.
        let emit_with_details = |scan_phases, emit_phases, tag: &str| {
            let trace = Staged::beside(d.join(format!("{tag}-trace.json")));
            let graph = Staged::beside(d.join(format!("{tag}-graph.json")));
            let execution = Staged::beside(d.join(format!("{tag}-execution.bin")));
            let mut writer = crate::execution::DetailWriter::create(&execution.write_to).unwrap();
            let scanned = runtime
                .block_on(scan::scan_with_details(&path, scan_phases, &mut writer))
                .unwrap();
            let run = scanned.trace_id.clone();
            let info = writer.finish().unwrap();
            let summary =
                emit::emit_with_execution(scanned, &trace, Some(&graph), emit_phases, Some(&info))
                    .unwrap();
            crate::execution::stamp(&execution.write_to, &run, &summary.capture, &info).unwrap();
            (
                std::fs::read(&trace.write_to).unwrap(),
                std::fs::read(&graph.write_to).unwrap(),
                std::fs::read(&execution.write_to).unwrap(),
            )
        };
        let production_reference = emit_with_details(true, false, "full-details-no-phases");
        assert_eq!(
            production_reference,
            emit_with_details(false, false, "lean-details-no-phases")
        );
        let out = run(&Source::Path(log.clone()), &d, &d, false).unwrap();
        assert_eq!(
            production_reference,
            (
                std::fs::read(out.trace_path).unwrap(),
                std::fs::read(out.graph_path).unwrap(),
                std::fs::read(crate::execution::store_path(&d, RUN_ID).unwrap()).unwrap(),
            )
        );

        let with_phases = emit_pair(
            runtime.block_on(scan::scan(&path)).unwrap(),
            true,
            "full-phases",
        );
        assert_eq!(
            with_phases,
            emit_pair(
                runtime
                    .block_on(scan::scan_with_phases(&path, true))
                    .unwrap(),
                true,
                "explicit-phases"
            )
        );
        assert_ne!(reference.0, with_phases.0);
        let production_with_phases = emit_with_details(true, true, "full-details-phases");
        let out = run(&Source::Path(log), &d, &d, true).unwrap();
        assert_eq!(
            production_with_phases,
            (
                std::fs::read(out.trace_path).unwrap(),
                std::fs::read(out.graph_path).unwrap(),
                std::fs::read(crate::execution::store_path(&d, RUN_ID).unwrap()).unwrap(),
            )
        );
    }

    #[test]
    fn optional_phase_projection_preserves_incomplete_and_schema_evidence() {
        let d = tmp("optional-phases-evidence");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for with_result in [false, true] {
            let log = demo_log(&d, Some(1), true, with_result);
            if with_result {
                let mut bytes = std::fs::read(&log).unwrap();
                bytes.extend_from_slice(&[0x01, 0x0e]);
                std::fs::write(&log, bytes).unwrap();
            }
            let path = infer(&log).unwrap();
            let full = runtime.block_on(scan::scan(&path)).unwrap();
            let lean = runtime
                .block_on(scan::scan_with_phases(&path, false))
                .unwrap();
            assert!(full.incomplete.is_some());
            assert_eq!(full.capture_state(), lean.capture_state());
            assert_eq!(
                format!("{:?}", full.incomplete),
                format!("{:?}", lean.incomplete)
            );
            assert_eq!(full.command_result_seen, lean.command_result_seen);
        }
        let path = infer(&demo_log(&d, Some(9), true, true)).unwrap();
        let full = runtime.block_on(scan::scan(&path)).err().unwrap();
        let lean = runtime
            .block_on(scan::scan_with_phases(&path, false))
            .err()
            .unwrap();
        assert_eq!(full.to_string(), lean.to_string());
        assert!(lean.to_string().contains("schema 9"));
    }

    #[test]
    fn round_trips_through_the_readers() {
        let d = tmp("happy");
        let log = demo_log(&d, Some(1), true, true);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();

        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        assert!(tr.health.warnings.is_empty(), "{:?}", tr.health.warnings);
        assert_eq!(tr.acts.len(), 2);
        assert_eq!(tr.n_hits, 0);
        assert_eq!(tr.runs.len(), 1);
        assert_eq!(tr.runs[0].run, RUN_ID);
        assert_eq!(tr.runs[0].result, Some(0));
        assert_eq!(tr.runs[0].outcome, Outcome::Success);
        assert_eq!(
            tr.runs[0]
                .dep_capture
                .as_ref()
                .and_then(|c| c.is_complete()),
            Some(true),
            "{:?}",
            tr.runs[0].dep_capture
        );
        assert_eq!(out.outcome, Outcome::Success);
        // the authoritative naming path end to end: CommandEnd's
        // unresolved patterns -> emitted `targets` -> the parsed run
        // (the fixture's argv is bare `buck2 build`, so recovery
        // cannot be what supplied this)
        assert_eq!(tr.run_targets(None), ["fbcode//demo/..."]);

        let g = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
        assert_eq!(g.meta.enc, Some(2));
        assert_eq!(g.undefined_joins, 0);
        assert_eq!(g.undefined_deps, 0);
        // nothing staged is left behind, and the pair carries one stamp
        assert!(std::fs::read_dir(&d).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".partial")
        }));
        assert!(tr.runs[0].capture.is_some());
        assert_eq!(tr.runs[0].capture, g.meta.capture);
        crate::graph::check_pair(&tr.runs[0], &g.meta).unwrap();
        let cov = g.meta.coverage.as_ref().unwrap();
        assert_eq!(cov.nodes, Some(3)); // 2 actions + 1 join row
        assert_eq!(cov.edges, Some(3)); // t2->t1, t2->tset, tset->t1
        assert_eq!(cov.dangling_deps, Some(0));
        assert_eq!((cov.action_rows, cov.join_rows), (Some(2), Some(1)));
        // both halves carry the same capture state
        assert_eq!(cov.dep_capture, tr.runs[0].dep_capture);
        if let Some(CaptureState::Started(c)) = &tr.runs[0].dep_capture {
            assert_eq!(c.schema, Some(1));
            assert_eq!(c.declared.tset_def_events, Some(1));
            assert_eq!(c.observed.tset_def_events, Some(1));
            assert!(c.is_complete(), "{:?}", c.problems());
        } else {
            panic!("fixture enables capture");
        }

        // The race loser is recorded as an attempt, not a separate row,
        // and both it and the winner carry their client-clock start.
        let raw = std::fs::read_to_string(&out.trace_path).unwrap();
        assert!(raw.contains("\"attempt_status\":[\"Cancelled\"]"));
        assert!(raw.contains("\"tries\":2"));
        assert!(raw.contains("\"elastic\":true"));
        assert!(raw.contains("\"worker_bytes_in\":4096"), "{raw}");
        assert!(raw.contains("\"worker_bytes_out\":512"), "{raw}");
        assert!(raw.contains("\"client_start\":"), "{raw}");
        assert!(raw.contains("\"winner\":{\"client_start\":"), "{raw}");
        {
            let x = crate::analyses::load_extras(&tr);
            let retried = x.args.iter().find(|a| a.tries == Some(2)).unwrap();
            assert_eq!(retried.wall_time_us, Some(200_000));
            assert_eq!(
                (retried.worker_bytes_in, retried.worker_bytes_out),
                (Some(4096), Some(512))
            );
            assert_eq!(retried.winner_dur_us, Some(100_000));
            let f = crate::analyses::Facts::for_run(&tr, &x, None).unwrap();
            let c = crate::analyses::retry_census(&f, 10).expect("one retried action");
            // the loser's 30 ms and the winner's 100 ms fit inside the
            // 200 ms action together, so only the exact rule can place them
            assert_eq!((c.concurrent, c.rerun, c.unknown), (1, 0, 0));
            assert_eq!(c.exact_fates, 1);
        }
        assert!(raw.contains("\"iu_digests_queried\":0"));
        // a measured zero batch size is kept: suppressed, it would fall
        // back to the declared-size proxy and price a transfer that
        // never happened
        assert!(raw.contains("\"iu_bytes_uploaded\":0"));
        // zero timings are suppressed on the wire...
        assert!(!raw.contains("\"iu_sem_wait_us\""));
        // ...and the reader still recognises the row as instrumented,
        // with measured zeros rather than an absent breakdown
        let x = crate::analyses::load_extras(&tr);
        let uploads: Vec<_> = x.args.iter().filter_map(|a| a.iu.as_ref()).collect();
        assert_eq!(uploads.len(), 1, "one instrumented upload in the fixture");
        assert_eq!(uploads[0].sem_wait_us, 0);
        assert_eq!(uploads[0].cas_call_us, 0);
        let with_bytes: Vec<_> = x
            .args
            .iter()
            .filter(|a| a.iu.is_some())
            .map(|a| a.iu_bytes_uploaded)
            .collect();
        assert_eq!(
            with_bytes,
            [Some(0)],
            "the recorded zero batch size survives"
        );
    }

    #[test]
    fn repeated_dependencies_keep_identical_graph_bytes() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::instant_event::Data as Instant;
        use buck2_data::span_end_event::Data as End;

        let dir = tmp("resolved-dependency-dedup");
        let log = demo_log(&dir, Some(1), true, true);
        let original = run(&Source::Path(log.clone()), &dir, &dir, false)
            .expect("the ordinary log should ingest");
        let expected = std::fs::read(&original.graph_path).expect("should read ordinary graph");
        testlog::rewrite(&log, |records| {
            for record in records {
                let Some(Progress::Event(event)) = &mut record.progress else {
                    continue;
                };
                match &mut event.data {
                    Some(Data::SpanEnd(end)) if event.span_id == 20 => {
                        if let Some(End::ActionExecution(action)) = &mut end.data {
                            let producers = action.graph_producers.get_or_insert_default();
                            producers.span_ids.extend([10, 10]);
                            producers.tset_ids.extend([0, 0]);
                        }
                    }
                    Some(Data::Instant(instant)) => {
                        if let Some(Instant::TransitiveSetProjectionDef(def)) = &mut instant.data {
                            def.producers
                                .get_or_insert_default()
                                .span_ids
                                .extend([10, 10]);
                        }
                    }
                    _ => {}
                }
            }
        });
        let out = run(&Source::Path(log), &dir, &dir, false)
            .expect("duplicate dependencies should ingest");
        let actual = std::fs::read(&out.graph_path).expect("should read deduplicated graph");
        assert_eq!(
            actual, expected,
            "stable dedup must preserve the entire ordinary graph"
        );
        let header: serde_json::Value = serde_json::from_slice(
            actual
                .split(|b| *b == b'\n')
                .next()
                .expect("graph has a header"),
        )
        .expect("graph header should be JSON");
        assert_eq!(header["graph"]["coverage"]["edges"], 3);
    }

    #[test]
    fn a_projection_row_is_named_after_its_wire_id() {
        // The id is the identity `dep_tset_ids` and `child_ids` join
        // on; the display key is a capped rendering that distinct
        // projections can share and is never used for anything.
        let d = tmp("tsetname");
        let log = demo_log(&d, Some(1), true, true);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let g = std::fs::read_to_string(&out.graph_path).unwrap();
        assert!(g.contains("\"tset:0\""), "{g}");
    }

    #[test]
    fn equal_display_keys_do_not_merge_distinct_projections() {
        // Two projections with byte-identical display keys but
        // different ids own different producers.  Joining them by
        // label would give `D` a false edge to `A` and a 30s critical
        // path; the recorded one is `B -> D` = 21s.
        let d = tmp("equal-label");
        let log = crate::ingest::testlog::equal_label_log(&d);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let g = std::fs::read_to_string(&out.graph_path).unwrap();
        assert!(g.contains("\"tset:0\"") && g.contains("\"tset:1\""), "{g}");
        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        let graph = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
        assert_eq!(graph.undefined_joins, 0);
        let (b, st) = crate::dag::build_full(&tr, &graph, None).unwrap();
        assert_eq!(st.n_matched, st.n_events, "every action has a graph node");
        assert_eq!(
            b.dag.cp, 21_000_000,
            "B (1s) -> D (20s); no invented A -> D edge"
        );
    }

    #[test]
    fn graph_bytes_are_identical_across_repeated_ingests() {
        // The graph artifact must hash identically run to run: a
        // byte-stable file is how a rewrite of the emitter is proven
        // to have moved nothing.  The fixture's diamond of eight defs
        // drives the join-row worklist — the one place map iteration
        // order could reach the file — and its four same-key defs pin
        // that a label never merges rows.
        let d = tmp("det");
        let log = many_tsets_log(&d);
        let mut graphs: Vec<Vec<u8>> = Vec::new();
        for i in 0..3 {
            let o = tmp(&format!("det-out-{i}"));
            let out = run(&Source::Path(log.clone()), &o, &o, false).unwrap();
            if graphs.is_empty() {
                // pin the shape the fixture promises: t1 + the eight
                // defs, each its own row; t1->7, 7->{8,11,13,14},
                // 8->9, 11->12, 14->15
                let g = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
                let cov = g.meta.coverage.as_ref().unwrap();
                assert_eq!(cov.nodes, Some(9));
                assert_eq!(cov.edges, Some(8));
                assert_eq!(cov.dangling_tsets, Some(0));
            }
            graphs.push(std::fs::read(&out.graph_path).unwrap());
        }
        assert_eq!(graphs[0], graphs[1]);
        assert_eq!(graphs[1], graphs[2]);
        // the stamp is part of those bytes, so it is a function of the
        // capture and never a per-run nonce
        let header = String::from_utf8_lossy(&graphs[0]);
        assert!(header.lines().next().unwrap().contains("\"capture\":"));
    }

    #[test]
    fn capture_fingerprint_covers_edges_metadata_and_footer_not_just_totals() {
        let d = tmp("content-stamp");
        let log = demo_log(&d, Some(1), true, true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut captures = Vec::new();
        for variant in 0..4 {
            let mut scanned = runtime.block_on(scan::scan(&infer(&log).unwrap())).unwrap();
            // Each case has the same trace id, action/stage/definition
            // counts, and start/end timestamps as the original.
            match variant {
                1 => {
                    // Change the recorded edge set without inventing a
                    // cycle. The tset still supplies the same dependency,
                    // but the duplicate direct edge no longer exists.
                    assert!(!scanned.actions[1].dep_span_ids.is_empty());
                    scanned.actions[1].dep_span_ids.clear();
                }
                2 => scanned.actions[1].last.cpu_us = Some(123_456),
                3 => scanned.footer.as_mut().unwrap().aliases += 1,
                _ => {}
            }
            let trace = Staged::beside(d.join(format!("v{variant}-trace.json")));
            let graph = Staged::beside(d.join(format!("v{variant}-graph.json")));
            emit::emit(scanned, &trace, Some(&graph), false).unwrap();
            let tr = crate::parse::parse(trace.write_to.to_str().unwrap()).unwrap();
            let g = crate::graph::load_graph(graph.write_to.to_str().unwrap()).unwrap();
            let capture = tr.runs[0].capture.clone().unwrap();
            assert!(capture.starts_with("retrace-v1-"));
            assert_eq!(g.meta.capture.as_ref(), Some(&capture));
            assert!(
                !captures.contains(&capture),
                "variant {variant} must have its own generation"
            );
            captures.push(capture);
            if variant == 3 {
                assert_eq!(
                    tr.runs[0].dep_capture.as_ref().unwrap().is_complete(),
                    Some(false)
                );
            }
        }
    }

    #[test]
    fn native_submit_boundary_marker_and_cache_resource_absence_survive() {
        let d = tmp("timing-contracts");
        let log = demo_log(&d, Some(1), true, true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut scanned = runtime.block_on(scan::scan(&infer(&log).unwrap())).unwrap();
        let a = &mut scanned.actions[1];
        a.execution_kind = 3; // legacy cache result can carry old execution resources
        let sid = a.sid;
        let ts = a.ts;
        scanned.stages.entry(sid).or_default().push(scan::StageRec {
            name: "ReSubmit",
            ts,
            dur: 1000,
            cancelled: false,
            details: Some(scan::StageDetails::Submit(Box::new(
                buck2_data::ReSubmitEnd {
                    slot_wait_us: 100,
                    request_prep_us: 200,
                    rpc_open_us: 700,
                    ..Default::default()
                },
            ))),
            cache_timings_invalid: false,
        });
        let trace = Staged::beside(d.join("timings-trace.json"));
        emit::emit(scanned, &trace, None, false).unwrap();
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&trace.write_to).unwrap()).unwrap();
        let args = rows
            .as_array()
            .unwrap()
            .iter()
            .find_map(|r| {
                let a = r.get("args")?;
                (a.get("kind")?.as_str()? == "action_cache").then_some(a)
            })
            .unwrap();
        assert!(args.get("cpu_us").is_none() && args.get("elastic").is_none());
        assert_eq!(args["winner"]["dur_us"], 100_000);
    }

    #[test]
    fn cached_execution_metadata_is_historical_but_prior_local_spans_survive() {
        // Every cache execution kind, plus affirmative command-level
        // reuse even when the outer action kind still says Remote.
        for (kind, remote_cache_hit) in [
            (3, false),
            (7, false),
            (9, false),
            (10, false),
            (12, false),
            (2, true),
        ] {
            for with_local_attempt in [false, true] {
                let d = tmp(&format!("cached-execution-{kind}-{with_local_attempt}"));
                let log =
                    testlog::cached_execution_log(&d, kind, remote_cache_hit, with_local_attempt);
                let out = run(&Source::Path(log), &d, &d, false).unwrap();
                let rows: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&out.trace_path).unwrap()).unwrap();
                let args = rows
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["cat"] == "recipe")
                    .unwrap()
                    .get("args")
                    .unwrap();
                assert_eq!(args["execution_metadata"], "reused");
                assert_eq!(args["historical_execution"]["exec_us"], 900_000);
                assert!(args.get("exec_us").is_none());
                assert!(args["re"].get("exec_us").is_none());
                if with_local_attempt {
                    assert_eq!(args["stages"]["execute_us"], 30_000);
                    assert_eq!(args["stages"]["execute_source"], "client_span");
                    assert_eq!(args["attempts"][0]["dur_us"], 30_000);
                    assert_eq!(args["winner"]["dur_us"], 5_000);
                } else {
                    assert!(args["stages"].get("execute_us").is_none());
                    assert!(args["stages"].get("execute_source").is_none());
                }
                let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
                let extras = crate::analyses::load_extras(&tr);
                assert_eq!(extras.args[0].exec_us, None);
                assert_eq!(
                    extras.args[0].stages.as_ref().unwrap().execute_us,
                    if with_local_attempt { 30_000 } else { 0 },
                    "the reader preserves current spans without promoting historical metadata"
                );
            }
        }
    }

    #[test]
    fn remote_execution_missing_new_timing_fields_does_not_prove_reuse() {
        // Older/default RemoteCommand payloads have cache_hit=false and
        // cache_hit_type=0 (ActionCache), despite not identifying a hit.
        // Missing worker/CPU fields cannot resolve that ambiguity either.
        let d = tmp("remote-execution-provenance-unknown");
        let log = testlog::cached_execution_log(&d, 2, false, false);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out.trace_path).unwrap()).unwrap();
        let args = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["cat"] == "recipe")
            .unwrap()
            .get("args")
            .unwrap();
        assert!(args.get("execution_metadata").is_none());
        assert!(args.get("historical_execution").is_none());
        assert_eq!(args["exec_us"], 900_000);
        assert_eq!(args["stages"]["execute_us"], 900_000);
        assert!(args["stages"].get("execute_source").is_none());
    }

    #[test]
    fn staging_names_never_collide_within_a_process() {
        let d = tmp("staging");
        let final_path = d.join("x-trace.json");
        let a = Staged::beside(final_path.clone());
        let b = Staged::beside(final_path.clone());
        assert_ne!(a.write_to, b.write_to);
        for s in [&a, &b] {
            assert!(s.write_to.to_string_lossy().ends_with(".partial"));
            assert_eq!(s.write_to.parent(), final_path.parent());
            assert_ne!(s.write_to, final_path);
        }
        std::fs::write(&a.write_to, b"a").unwrap();
        std::fs::write(&b.write_to, b"b").unwrap();
        let a_path = a.write_to.clone();
        drop(a);
        assert!(!a_path.exists(), "an unpublished staging file is removed");
        let published = b.publish().unwrap();
        assert_eq!(published, final_path);
        assert_eq!(std::fs::read(&final_path).unwrap(), b"b");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    }

    #[test]
    fn re_ingest_with_no_usable_graph_removes_the_earlier_sidecar() {
        let d = tmp("reingest");
        let first = demo_log(&d, Some(1), true, true);
        let out = run(&Source::Path(first), &d, &d, false).unwrap();
        assert!(out.graph_path.exists());
        // the same build id, ingested from a log with no dependency
        // channels and no BuildGraphInfo: no graph can be written
        let second = cache_hit_log(&d);
        let out = run(&Source::Path(second), &d, &d, false).unwrap();
        assert!(!out.graph_path.exists(), "the stale sidecar is gone");
        assert!(crate::graph::sibling_graph_path(out.trace_path.to_str().unwrap()).is_none());
        assert!(
            out.status_lines
                .iter()
                .any(|s| s.contains("removed; it belonged to an earlier ingest")),
            "{:?}",
            out.status_lines
        );
        crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
    }

    /// The mixed-generation case two renames alone cannot prevent: a
    /// graph written from a torn copy of a build beside the trace of the
    /// whole log.  The stamps differ, so the pair is refused.
    #[test]
    fn a_torn_and_a_whole_copy_of_one_build_do_not_pair() {
        let d1 = tmp("pair-whole");
        let whole = run(
            &Source::Path(demo_log(&d1, Some(1), true, true)),
            &d1,
            &d1,
            false,
        )
        .unwrap();
        let d2 = tmp("pair-torn");
        let torn = run(
            &Source::Path(demo_log(&d2, Some(1), true, false)),
            &d2,
            &d2,
            false,
        )
        .unwrap();
        let tr = crate::parse::parse(whole.trace_path.to_str().unwrap()).unwrap();
        let own = crate::graph::load_graph(whole.graph_path.to_str().unwrap()).unwrap();
        crate::graph::check_pair(&tr.runs[0], &own.meta).unwrap();
        let other = crate::graph::load_graph(torn.graph_path.to_str().unwrap()).unwrap();
        assert_ne!(own.meta.capture, other.meta.capture);
        let err = crate::graph::check_pair(&tr.runs[0], &other.meta).unwrap_err();
        assert!(format!("{err:#}").contains("different capture"), "{err:#}");
    }

    #[test]
    fn graph_rows_keep_their_own_names_when_scan_order_differs_from_time_order() {
        // The consumer (`t2`) closes its span BEFORE the producer
        // (`t1`) even though it started later, so scan order and
        // timestamp order disagree and the lane sort's permutation of
        // `evs` is not the identity.  Every other synthetic log here
        // has the two orders equal, which is exactly why the row-name
        // desynchronisation this pins down was invisible to them.
        let d = tmp("reorder");
        let log = reorder_log(&d);

        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let raw = std::fs::read_to_string(&out.graph_path).unwrap();
        let mut dep_rows = 0;
        for line in raw.lines().skip(1) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let Some(t) = v.get("t").and_then(|x| x.as_str()) else {
                continue;
            };
            let deps = v.get("d").and_then(|x| x.as_array());
            if let Some(deps) = deps.filter(|d| !d.is_empty()) {
                dep_rows += 1;
                assert!(
                    t.starts_with("t2 "),
                    "the dep-carrying row must be the consumer, got `{t}`"
                );
                assert!(
                    deps[0].as_str().unwrap_or_default().starts_with("t1 "),
                    "the consumer's dep must be the producer, got {deps:?}"
                );
            }
        }
        assert_eq!(dep_rows, 1);
    }

    #[test]
    fn a_foreign_footer_is_refused_even_without_a_start_stamp() {
        // pre-stamp recorders wrote only the footer; the end-of-scan
        // check must refuse on its own, not just via the start stamp
        let d = tmp("footer-only");
        let log = testlog::demo_log_opts(&d, Some(9), true, true, false);
        let err = run(&Source::Path(log), &d, &d, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema 9"), "{err}");
        assert!(err.contains("reads schema 1"), "{err}");
    }

    #[test]
    fn unknown_action_exit_remains_null_without_changing_failure_counts() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::span_end_event::Data as End;

        for failed in [false, true] {
            let dir = tmp(&format!("unknown-action-exit-{failed}"));
            let log = demo_log(&dir, Some(1), true, true);
            testlog::rewrite(&log, |records| {
                for record in records {
                    if let Some(Progress::Event(event)) = &mut record.progress
                        && event.span_id == 10
                        && let Some(Data::SpanEnd(end)) = &mut event.data
                        && let Some(End::ActionExecution(action)) = &mut end.data
                    {
                        action.failed = failed;
                        for command in &mut action.commands {
                            if let Some(details) = &mut command.details {
                                details.signed_exit_code = None;
                            }
                        }
                    }
                }
            });
            let out = run(&Source::Path(log), &dir, &dir, false)
                .expect("unknown exit should not prevent ingestion");
            let rows: serde_json::Value =
                serde_json::from_slice(&std::fs::read(out.trace_path).expect("should read trace"))
                    .expect("trace should be JSON");
            let rows = rows.as_array().expect("trace should be an array");
            let action = rows
                .iter()
                .find(|r| r["cat"] == "recipe" && r["args"]["span_id"] == 10)
                .expect("action should remain visible");
            assert_eq!(action["args"]["exit"], serde_json::Value::Null);
            assert_eq!(action["args"]["sig"], serde_json::Value::Null);
            let end = rows
                .iter()
                .find(|r| r["name"] == "run_end")
                .expect("run end should exist");
            assert_eq!(end["args"]["failed"], u64::from(failed));
        }
    }

    #[test]
    fn failed_action_keeps_the_measured_successful_command_and_explicit_failure() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::span_end_event::Data as End;
        let d = tmp("failed-action-successful-command");
        let log = demo_log(&d, Some(1), true, true);
        testlog::rewrite(&log, |records| {
            for record in records {
                if let Some(Progress::Event(event)) = &mut record.progress
                    && event.span_id == 10
                    && let Some(Data::SpanEnd(end)) = &mut event.data
                    && let Some(End::ActionExecution(action)) = &mut end.data
                {
                    action.failed = true;
                    action.error = Some(buck2_data::action_execution_end::Error::MissingOutputs(
                        Default::default(),
                    ));
                }
            }
        });
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.trace_path).unwrap()).unwrap();
        let action = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["cat"] == "recipe" && r["args"]["span_id"] == 10)
            .unwrap();
        assert_eq!(action["args"]["exit"], 0);
        assert_eq!(action["args"]["failed"], true);
        assert_eq!(action["args"]["error_kind"], "missing_outputs");
        assert_eq!(action["args"]["winner"]["status"], "Success");
        let end = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "run_end")
            .unwrap();
        assert_eq!(end["args"]["failed"], 1);
        assert!(end["args"].get("remade").is_none() && end["args"].get("uptodate").is_none());
    }

    #[test]
    fn unfinished_walk_keeps_trace_and_counts_only_dangling_tsets() {
        let d = tmp("unfinished-walk-tset-hole");
        let mut footer = FooterSpec::exact(1, false);
        footer.unfinished_walk = true;
        let log = demo_log_with(
            &d,
            DemoOpts {
                footer: Some(footer),
                define_tset: false,
                with_result: true,
                with_start_stamp: true,
                with_command_end: true,
            },
        );
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let graph = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
        let coverage = graph.meta.coverage.unwrap();
        assert_eq!(coverage.dangling_tsets, Some(1));
        assert_eq!(coverage.dangling_deps, Some(0));
        assert_eq!(out.dep_capture.is_complete(), Some(false));
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("unfinished definition walk")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn recording_presence_not_edge_count_controls_graph_publication() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::span_end_event::Data as End;
        for dangling in [false, true] {
            let d = tmp(if dangling {
                "all-dangling-graph"
            } else {
                "edgeless-graph"
            });
            let log = demo_log(&d, Some(1), false, true);
            testlog::rewrite(&log, |records| {
                for record in records {
                    if let Some(Progress::Event(event)) = &mut record.progress
                        && let Some(Data::SpanEnd(end)) = &mut event.data
                        && let Some(End::ActionExecution(action)) = &mut end.data
                    {
                        action.graph_producers = Some(buck2_data::ActionGraphProducers {
                            reused_ids: if dangling { vec![0] } else { vec![] },
                            ..Default::default()
                        });
                    }
                }
            });
            let out = run(&Source::Path(log), &d, &d, false).unwrap();
            let graph = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
            let coverage = graph.meta.coverage.unwrap();
            assert_eq!(coverage.action_rows, Some(2));
            assert_eq!(coverage.edges, Some(0));
            assert_eq!(coverage.dangling_deps, Some(if dangling { 2 } else { 0 }));
            assert!(
                !std::fs::read_to_string(out.graph_path)
                    .unwrap()
                    .contains("cp_chain")
            );
        }
    }

    #[test]
    fn conflicting_capture_records_stay_visible_and_never_replace_the_first_footer() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        use buck2_data::instant_event::Data as Instant;
        for conflict in [false, true] {
            let d = tmp(if conflict {
                "conflicting-footers"
            } else {
                "identical-footers"
            });
            let log = demo_log(&d, Some(1), true, true);
            testlog::rewrite(&log, |records| {
                let mut duplicate = records.iter().find(|record| matches!(&record.progress,
                    Some(Progress::Event(event)) if matches!(&event.data,
                    Some(Data::Instant(instant)) if matches!(&instant.data, Some(Instant::ActionGraphFooter(_)))))).unwrap().clone();
                if let Some(Progress::Event(event)) = &mut duplicate.progress
                    && let Some(Data::Instant(instant)) = &mut event.data
                    && let Some(Instant::ActionGraphFooter(footer)) = &mut instant.data
                {
                    event.timestamp = None;
                    if conflict {
                        footer.action_ends = 99;
                    }
                }
                records.push(duplicate);
            });
            let out = run(&Source::Path(log), &d, &d, false).unwrap();
            let CaptureState::Started(state) = out.dep_capture else {
                panic!("capture missing");
            };
            assert_eq!(state.declared.actions_recorded, Some(2));
            assert_eq!(state.integrity.conflicting_footers, u64::from(conflict));
            assert_eq!(state.is_complete(), !conflict);
        }
    }

    #[test]
    fn duplicate_completed_producer_ids_cannot_rebind_dependencies_or_stages() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        let dir = tmp("duplicate-producer");
        let log = demo_log(&dir, Some(1), true, true);
        testlog::rewrite(&log, |records| {
            let mut start = records.iter().find(|r| matches!(&r.progress,
                Some(Progress::Event(e)) if matches!(&e.data, Some(Data::SpanStart(s))
                    if matches!(s.data, Some(buck2_data::span_start_event::Data::ActionExecution(_)))))).unwrap().clone();
            let Some(Progress::Event(start_event)) = &mut start.progress else {
                unreachable!()
            };
            let sid = start_event.span_id;
            let Some(Data::SpanStart(s)) = &mut start_event.data else {
                unreachable!()
            };
            let Some(buck2_data::span_start_event::Data::ActionExecution(action)) = &mut s.data
            else {
                unreachable!()
            };
            let key = action.key.as_mut().unwrap();
            key.key.push_str("-conflicting-producer");
            let new_key = key.clone();
            let mut end = records.iter().find(|r| matches!(&r.progress,
                Some(Progress::Event(e)) if e.span_id == sid && matches!(&e.data, Some(Data::SpanEnd(s))
                    if matches!(s.data, Some(buck2_data::span_end_event::Data::ActionExecution(_)))))).unwrap().clone();
            let Some(Progress::Event(end_event)) = &mut end.progress else {
                unreachable!()
            };
            let Some(Data::SpanEnd(e)) = &mut end_event.data else {
                unreachable!()
            };
            let Some(buck2_data::span_end_event::Data::ActionExecution(action)) = &mut e.data
            else {
                unreachable!()
            };
            action.key = Some(new_key);
            records.extend([start, end]);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let scan = runtime.block_on(scan::scan(&infer(&log).unwrap())).unwrap();
        assert_eq!(scan.integrity.ambiguous_producer_spans, 1);
        assert_eq!(scan.ambiguous_spans.len(), 1);
        for sid in &scan.ambiguous_spans {
            assert!(!scan.stages.contains_key(sid));
        }
        let out = run(&Source::Path(log), &dir, &dir, false).unwrap();
        let CaptureState::Started(capture) = out.dep_capture else {
            unreachable!()
        };
        assert!(!capture.is_complete());
        assert_eq!(capture.integrity.ambiguous_producer_spans, 1);
    }

    #[test]
    fn file_watcher_metadata_distinguishes_producer_counts_from_retained_paths() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        let dir = tmp("watcher-metadata");
        let log = testlog::command_profile_log(&dir);
        testlog::rewrite(&log, |records| {
            for record in records {
                if let Some(Progress::Event(event)) = &mut record.progress
                    && let Some(Data::SpanEnd(end)) = &mut event.data
                    && let Some(buck2_data::span_end_event::Data::FileWatcher(watcher)) =
                        &mut end.data
                {
                    watcher.stats = Some(buck2_data::FileWatcherStats {
                        fresh_instance: true,
                        events_total: 40_000,
                        events_processed: 25_000,
                        incomplete_events_reason: Some(
                            "producer path list is incomplete".to_owned(),
                        ),
                        events: (0..10_005)
                            .map(|n| buck2_data::FileWatcherEvent {
                                path: format!("path-{}", n % 3),
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    });
                }
            }
        });
        let out = run(&Source::Path(log), &dir, &dir, false).unwrap();
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.trace_path).unwrap()).unwrap();
        let header = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "run_start")
            .unwrap();
        assert_eq!(header["args"]["file_changes_total"], 3);
        assert_eq!(header["args"]["file_changes"].as_array().unwrap().len(), 3);
        let watcher = &header["args"]["file_watcher"];
        assert_eq!(watcher["events_processed"], 25_000);
        assert_eq!(watcher["events_total"], 40_000);
        assert_eq!(watcher["fresh_instance"], true);
        assert_eq!(watcher["paths_truncated"], true);
        assert_eq!(
            watcher["incomplete_events_reasons"],
            serde_json::json!(["producer path list is incomplete"])
        );
    }

    #[test]
    fn missing_machine_memory_is_not_emitted_as_zero() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        let dir = tmp("missing-memory");
        let log = demo_log(&dir, Some(1), true, true);
        testlog::rewrite(&log, |records| {
            for record in records {
                if let Some(Progress::Event(event)) = &mut record.progress
                    && let Some(Data::Instant(instant)) = &mut event.data
                    && let Some(buck2_data::instant_event::Data::SystemInfo(info)) =
                        &mut instant.data
                {
                    info.system_total_memory_bytes = None;
                }
            }
        });
        let out = run(&Source::Path(log), &dir, &dir, false).unwrap();
        let bytes = std::fs::read_to_string(out.trace_path).unwrap();
        let rows: serde_json::Value = serde_json::from_str(&bytes).unwrap();
        let header = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "run_start")
            .unwrap();
        assert!(header["args"].get("mem").is_none());
    }

    #[test]
    fn concatenated_invocation_does_not_silently_start_another_command() {
        use prost::Message as _;
        let dir = tmp("duplicate-invocation");
        let log = demo_log(&dir, Some(1), true, true);
        let mut bytes = std::fs::read(&log).unwrap();
        bytes.extend(
            buck2_data::Invocation {
                command_line_args: vec!["buck2".to_owned(), "query".to_owned()],
                trace_id: Some("00000000-0000-0000-0000-000000000000".to_owned()),
                ..Default::default()
            }
            .encode_length_delimited_to_vec(),
        );
        std::fs::write(&log, bytes).unwrap();
        let out = run(&Source::Path(log), &dir, &dir, false).unwrap();
        let CaptureState::Started(capture) = out.dep_capture else {
            unreachable!()
        };
        assert!(capture.decode_error.is_some());
        assert!(!capture.is_complete());
        assert_eq!(
            out.outcome,
            Outcome::Success,
            "the first command Result was retained"
        );
    }

    #[test]
    fn foreign_capture_schema_is_refused_by_name() {
        let d = tmp("schema");
        let log = demo_log(&d, Some(9), true, true);
        let err = run(&Source::Path(log), &d, &d, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema 9"), "{err}");
        assert!(err.contains("reads schema 1"), "{err}");
    }

    #[test]
    fn a_torn_foreign_log_is_refused_by_its_start_stamp() {
        let d = tmp("stamp9");
        let log = torn_log_with_start_stamp(&d, 9);
        let err = run(&Source::Path(log), &d, &d, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema 9"), "{err}");
        assert!(err.contains("reads schema 1"), "{err}");
    }

    #[test]
    fn a_torn_log_with_a_matching_stamp_degrades_instead_of_refusing() {
        let d = tmp("stamp1");
        let log = torn_log_with_start_stamp(&d, 1);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        assert!(
            out.status_lines.iter().any(|s| s.contains("ends early")),
            "{:?}",
            out.status_lines
        );
    }

    /// A build killed before its result: the run_end says so instead
    /// of inventing exit 0, and every reader of the outcome agrees.
    #[test]
    fn no_result_and_no_command_end_leave_the_outcome_unknown() {
        let d = tmp("unknown-outcome");
        let log = torn_log_with_start_stamp(&d, 1);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        assert_eq!(out.outcome, Outcome::Unknown);
        assert!(
            out.status_lines
                .iter()
                .any(|s| s.starts_with("command outcome: unknown")),
            "{:?}",
            out.status_lines
        );
        assert!(
            out.status_lines
                .iter()
                .any(|s| s.starts_with("capture: no dependency-capture footer")),
            "{:?}",
            out.status_lines
        );
        let raw = std::fs::read_to_string(&out.trace_path).unwrap();
        assert!(raw.contains("\"exit\":null"), "{raw}");
        assert!(raw.contains("\"outcome\":\"unknown\""), "{raw}");

        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        let r = &tr.runs[0];
        assert!(r.ended);
        assert_eq!(r.result, None);
        assert_eq!(r.outcome, Outcome::Unknown);
        let Some(CaptureState::Started(c)) = &r.dep_capture else {
            panic!("{:?}", r.dep_capture);
        };
        assert!(!c.footer_present && !c.command_result_seen && !c.command_end_seen);
        assert!(!c.is_complete());
        assert!(
            tr.health
                .warnings
                .iter()
                .any(|w| w.contains("outcome is unknown")),
            "{:?}",
            tr.health.warnings
        );
        assert!(
            tr.health
                .warnings
                .iter()
                .any(|w| w.contains("no dependency-capture footer")),
            "{:?}",
            tr.health.warnings
        );

        // the overview row never calls it a success
        let x = crate::analyses::load_extras(&tr);
        let f = crate::analyses::Facts::for_run(&tr, &x, None).unwrap();
        let o = crate::analyses::overview_facts(&f);
        let rows = crate::analyses::overview_rows(&f, &o, None, &crate::analyses::fmt_when_utc);
        let result = rows
            .iter()
            .find(|r| r.key == "Result")
            .expect("a Result row");
        assert!(result.value.starts_with("unknown"), "{}", result.value);
        assert!(!result.value.contains("success"), "{}", result.value);
    }

    #[test]
    fn footer_counts_that_disagree_mark_the_capture_incomplete() {
        let d = tmp("count-mismatch");
        let log = demo_log_with(
            &d,
            DemoOpts {
                footer: Some(FooterSpec {
                    action_ends: 999,
                    ..FooterSpec::exact(1, true)
                }),
                define_tset: true,
                with_result: true,
                with_start_stamp: true,
                with_command_end: true,
            },
        );
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        assert_eq!(out.outcome, Outcome::Success);
        assert!(
            out.status_lines
                .iter()
                .any(|s| s.contains("declares 999 action(s), 2 were recorded")),
            "{:?}",
            out.status_lines
        );
        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        let r = &tr.runs[0];
        assert_eq!(
            r.outcome,
            Outcome::Success,
            "outcome and completeness are independent"
        );
        assert_eq!(
            r.dep_capture.as_ref().and_then(|c| c.is_complete()),
            Some(false)
        );
        assert!(
            tr.health
                .warnings
                .iter()
                .any(|w| w.contains("999") && w.contains("2 were recorded")),
            "{:?}",
            tr.health.warnings
        );
        let g = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
        let Some(CaptureState::Started(c)) = &g.meta.coverage.as_ref().unwrap().dep_capture else {
            panic!("{:?}", g.meta.coverage);
        };
        assert_eq!(c.declared.actions_recorded, Some(999));
        assert_eq!(c.observed.actions_recorded, Some(2));
        assert_eq!(c.finality, Finality::Final);
    }

    /// A set referenced but not defined, on a capture the recorder vouches for, is corruption.
    #[test]
    fn undefined_set_on_a_complete_capture_is_refused() {
        let d = tmp("promise-corrupt");
        let log = demo_log_with(
            &d,
            DemoOpts {
                footer: Some(FooterSpec {
                    ..FooterSpec::exact(1, false)
                }),
                define_tset: false,
                with_result: true,
                with_start_stamp: true,
                with_command_end: true,
            },
        );
        let err = run(&Source::Path(log), &d, &d, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refusing to emit a silently-holed graph"),
            "{err}"
        );
    }

    #[test]
    fn a_concurrent_command_leaves_a_complete_capture_partial() {
        use buck2_cli_proto::command_progress::Progress;

        let d = tmp("concurrent-command");
        let log = demo_log(&d, Some(1), true, true);
        let alone = run(&Source::Path(log.clone()), &d, &d, false).unwrap();
        assert_eq!(alone.dep_capture.is_complete(), Some(true));
        testlog::rewrite(&log, |records| {
            records.insert(
                1,
                buck2_cli_proto::CommandProgress {
                    progress: Some(Progress::Event(Box::new(buck2_data::BuckEvent {
                        trace_id: testlog::RUN_ID.to_owned(),
                        data: Some(buck2_data::buck_event::Data::Instant(
                            buck2_data::InstantEvent {
                                data: Some(
                                    buck2_data::ConcurrentCommands {
                                        trace_ids: vec![
                                            "other-trace".to_owned(),
                                            testlog::RUN_ID.to_owned(),
                                        ],
                                    }
                                    .into(),
                                ),
                            },
                        )),
                        ..Default::default()
                    }))),
                },
            );
        });
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let CaptureState::Started(c) = &out.dep_capture else {
            panic!("started");
        };
        assert_eq!(
            c.concurrent_commands,
            ["other-trace"],
            "its own trace id is not another command"
        );
        assert!(!c.is_complete() && c.omission_only());
        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        assert!(
            tr.health
                .warnings
                .iter()
                .any(|w| w.contains("ran alongside 1 other command")),
            "{:?}",
            tr.health.warnings
        );
    }

    #[test]
    fn a_nonfinal_footer_is_incomplete_even_when_every_count_agrees() {
        let nonfinal = FooterSpec {
            nonfinal: true,
            ..FooterSpec::exact(1, true)
        };
        let d = tmp("nonfinal");
        let log = demo_log_with(
            &d,
            DemoOpts {
                footer: Some(nonfinal),
                define_tset: true,
                with_result: true,
                with_start_stamp: true,
                with_command_end: true,
            },
        );
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let CaptureState::Started(c) = &out.dep_capture else {
            panic!("started");
        };
        assert_eq!(c.finality, Finality::Nonfinal);
        assert!(!c.is_complete());
        assert_eq!(c.declared.actions_recorded, c.observed.actions_recorded);
        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        assert!(
            tr.health.warnings.iter().any(|w| w.contains("nonfinal")),
            "{:?}",
            tr.health.warnings
        );

        // an undefined definition under a nonfinal footer is an expected
        // gap, counted as dangling rather than refused
        let d = tmp("nonfinal-dangling");
        let log = demo_log_with(
            &d,
            DemoOpts {
                footer: Some(FooterSpec {
                    nonfinal: true,
                    ..FooterSpec::exact(1, false)
                }),
                define_tset: false,
                with_result: true,
                with_start_stamp: true,
                with_command_end: true,
            },
        );
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("nonfinal capture footer")),
            "{:?}",
            out.warnings
        );
        let g = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
        assert_eq!(g.meta.coverage.as_ref().unwrap().dangling_tsets, Some(1));
    }

    /// A build run without the recorder has no capture; that is not a
    /// gap in one and must not warn.
    #[test]
    fn a_log_without_dependency_capture_is_not_a_gap() {
        let d = tmp("no-capture");
        let log = cache_hit_log(&d);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        assert_eq!(out.dep_capture, CaptureState::NotEnabled);
        assert!(
            out.status_lines.iter().all(|s| !s.starts_with("capture:")),
            "{:?}",
            out.status_lines
        );
        let raw = std::fs::read_to_string(&out.trace_path).unwrap();
        assert!(
            raw.contains("\"dep_capture\":{\"state\":\"not-enabled\"}"),
            "{raw}"
        );
        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        assert_eq!(tr.runs[0].dep_capture, Some(CaptureState::NotEnabled));
        assert!(
            tr.health.warnings.iter().all(|w| !w.contains("capture")),
            "{:?}",
            tr.health.warnings
        );
    }

    #[test]
    fn undefined_tset_on_a_complete_log_refuses_the_graph() {
        let d = tmp("corrupt");
        let log = demo_log(&d, Some(1), false, true);
        let err = run(&Source::Path(log), &d, &d, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refusing to emit a silently-holed graph"),
            "{err}"
        );
        // a refusal publishes nothing and stages nothing behind
        let left: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            left.iter().all(|n| !n.ends_with("-trace.json")
                && !n.ends_with("-graph.json")
                && !n.ends_with(".partial")),
            "{left:?}"
        );
    }

    #[test]
    fn undefined_tset_on_a_truncated_log_degrades_to_dangling() {
        let d = tmp("torn");
        let log = demo_log(&d, Some(1), false, false);
        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        assert!(
            out.warnings.iter().any(|w| w.contains("truncated")),
            "{:?}",
            out.warnings
        );
        let g = crate::graph::load_graph(out.graph_path.to_str().unwrap()).unwrap();
        assert_eq!(g.meta.coverage.as_ref().unwrap().dangling_tsets, Some(1));
    }

    #[test]
    fn a_decode_failure_is_not_reported_as_a_cancelled_build() {
        // A finished build with one unreadable frame appended: wire type
        // 6 exists in no protobuf encoding, so the stream stops on an
        // error instead of at a clean end.  Both endings shorten the
        // scan by the same visible amount, and naming this one a
        // cancelled build sends the reader hunting for a build that
        // never went wrong.
        let d = tmp("undecodable");
        let log = demo_log(&d, Some(1), true, true);
        let mut bytes = std::fs::read(&log).unwrap();
        bytes.extend_from_slice(&[0x01, 0x0e]);
        std::fs::write(&log, &bytes).unwrap();

        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let reported = out
            .warnings
            .iter()
            .find(|s| s.contains("stopped decoding"))
            .unwrap_or_else(|| panic!("no decode failure reported: {:?}", out.warnings));
        assert!(
            reported.contains("invalid wire type value: 6"),
            "{reported}"
        );
        assert!(
            out.warnings
                .iter()
                .any(|s| s.contains("Only the readable prefix is shown"))
        );
        assert!(out.status_lines.iter().all(|s| !s.starts_with("error:")));
        assert!(
            out.status_lines.iter().all(|s| !s.contains("cancelled or")),
            "{:?}",
            out.status_lines
        );
        // the Result frame precedes the garbage: the outcome is known
        // while the capture is not complete
        assert_eq!(out.outcome, Outcome::Success);
        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        let Some(CaptureState::Started(c)) = &tr.runs[0].dep_capture else {
            panic!("{:?}", tr.runs[0].dep_capture);
        };
        let error = c
            .decode_error
            .as_deref()
            .expect("decode error survives in capture metadata");
        assert!(
            reported.contains(error),
            "warning lost the recorded decoder error: {reported}"
        );
        assert!(c.records_read.is_some_and(|records| records > 0));
        assert!(!c.is_complete());
        assert_eq!(tr.runs[0].outcome, Outcome::Success);
    }

    #[test]
    fn classify_source_recognizes_ids_and_log_files() {
        assert!(matches!(classify_source(RUN_ID), Some(Source::TraceId(..))));
        assert!(matches!(
            classify_source("x/y/foo.pb.zst"),
            Some(Source::Path(..))
        ));
        assert!(classify_source("build-trace.json").is_none());
        assert!(classify_source("11111111-2222-3333-4444-55555555555z").is_none());
    }

    /// The reader takes the presence of `args.cache` as the hit signal and
    /// never looks at the `hit` field inside it, so attaching the object to
    /// an action the cache did not serve would inflate every hit rate the
    /// tool reports. Pin both directions of that contract.
    #[test]
    fn only_a_cache_served_action_carries_a_cache_object() {
        let d = tmp("cachehit");
        let log = cache_hit_log(&d);

        let out = run(&Source::Path(log), &d, &d, false).unwrap();
        let raw = std::fs::read_to_string(&out.trace_path).unwrap();
        assert_eq!(raw.matches("\"hit\":true").count(), 1, "{raw}");
        assert!(raw.contains("\"hits\":1"), "{raw}");
        assert!(raw.contains("\"restored_bytes\":4096"), "{raw}");

        let tr = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        assert_eq!(tr.acts.len(), 2);
        assert_eq!(tr.n_hits, 1);
        assert_eq!(
            tr.acts
                .iter()
                .filter(|a| a.flags & crate::types::F_CACHE_HIT != 0)
                .count(),
            1
        );
    }
}
