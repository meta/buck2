/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Reuse completed conversions without decoding the event stream again.
//!
//! A build UUID is only a lookup key: growing/torn copies of one build can
//! carry different captures. The receipt binds the source file identity,
//! conversion options/version and all published artifacts. Fast validation
//! uses stat identity plus bounded generation-header reads on Unix; platforms
//! without change-time identity conservatively hash the source and artifacts.
//! Normal trace/graph readers
//! still validate the contents and their pairing after a hit.
//!
//! Writers hold a per-key cross-process lock through receipt publication.
//! The receipt is published last; a crash before that is a miss.

use std::fs::File;
use std::io::BufRead as _;
use std::io::Read;
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use buck2_event_log::read::EventLogPathBuf;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use super::IngestOutput;
use super::Staged;
use crate::types::CaptureState;
use crate::types::Outcome;

/// Bump when scan/emission semantics change, even if the wire format does
/// not. This is deliberately independent of GUI-only binary rebuilds.
/// Version 14 records the bytes the RE worker moved for each action.
const CONVERSION_VERSION: u32 = 14;
const MAX_METADATA_BYTES: u64 = 4 * 1024 * 1024;

fn content_digest(path: &Path, cancel: Option<&AtomicBool>) -> anyhow::Result<[u8; 20]> {
    super::check_cancel(cancel)?;
    let before = FileIdentity::read(path)?;
    let mut file = File::open(path)?;
    let digest = digest_reader(&mut file, cancel)?;
    anyhow::ensure!(
        FileIdentity::read(path)? == before,
        "file changed during cache validation"
    );
    Ok(digest)
}

fn digest_reader(reader: &mut impl Read, cancel: Option<&AtomicBool>) -> anyhow::Result<[u8; 20]> {
    use sha1::Digest as _;

    let mut hash = sha1::Sha1::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        super::check_cancel(cancel)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    super::check_cancel(cancel)?;
    Ok(hash.finalize().into())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct FileIdentity {
    bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanos: i64,
}

impl FileIdentity {
    fn read(path: &Path) -> anyhow::Result<Self> {
        let m = std::fs::metadata(path)?;
        anyhow::ensure!(m.is_file(), "{} is not a regular file", path.display());
        Ok(Self {
            bytes: m.len(),
            modified: m.modified()?,
            #[cfg(unix)]
            device: m.dev(),
            #[cfg(unix)]
            inode: m.ino(),
            #[cfg(unix)]
            changed_seconds: m.ctime(),
            #[cfg(unix)]
            changed_nanos: m.ctime_nsec(),
        })
    }
}

/// On platforms without change-time/inode metadata, length and mtime do not
/// detect a same-sized replacement copied with preserved timestamps. Hash all
/// generated files there, not just the source event log. Unix keeps the cheap
/// metadata path. This private receipt shape can safely invalidate old receipts.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ArtifactIdentity {
    metadata: FileIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_digest: Option<[u8; 20]>,
}

impl ArtifactIdentity {
    fn read(path: &Path, cancel: Option<&AtomicBool>) -> anyhow::Result<Self> {
        Self::read_with_policy(path, cancel, !cfg!(unix))
    }

    fn read_with_policy(
        path: &Path,
        cancel: Option<&AtomicBool>,
        hash_contents: bool,
    ) -> anyhow::Result<Self> {
        super::check_cancel(cancel)?;
        let metadata = FileIdentity::read(path)?;
        let content_digest = hash_contents
            .then(|| content_digest(path, cancel))
            .transpose()?;
        anyhow::ensure!(
            FileIdentity::read(path)? == metadata,
            "artifact changed during cache validation"
        );
        Ok(Self {
            metadata,
            content_digest,
        })
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Request {
    pub(super) trace_id: String,
    source: PathBuf,
    identity: FileIdentity,
    #[cfg(not(unix))]
    source_digest: [u8; 20],
}

impl Request {
    /// Called after the header's per-command lock is held. A source may have
    /// been replaced while waiting for that lock; never use another command's
    /// receipt or publish its artifacts under the first command's lock.
    pub(super) fn read_for_locked_run(
        log: &EventLogPathBuf,
        locked_run: &str,
        cancel: Option<&AtomicBool>,
    ) -> anyhow::Result<Option<Self>> {
        // Cache bookkeeping is optional. If identity probing fails, native
        // scanning still validates against the original locked command ID.
        let request = Self::read_cancellable(log, cancel).ok();
        super::check_cancel(cancel)?;
        if let Some(request) = &request {
            anyhow::ensure!(
                request.trace_id == locked_run,
                "event log was replaced while acquiring its writer lock: locked command \
                 {locked_run}, but its current header names {}",
                request.trace_id
            );
        }
        Ok(request)
    }

    #[cfg(test)]
    fn read(log: &EventLogPathBuf) -> anyhow::Result<Self> {
        Self::read_cancellable(log, None)
    }

    fn read_cancellable(
        log: &EventLogPathBuf,
        cancel: Option<&AtomicBool>,
    ) -> anyhow::Result<Self> {
        super::check_cancel(cancel)?;
        let source = std::fs::canonicalize(log.path().as_path())?;
        let identity = FileIdentity::read(&source)?;
        #[cfg(not(unix))]
        let source_digest = content_digest(&source, cancel)?;
        let trace_id = super::trace_id(log)?;
        super::check_cancel(cancel)?;
        anyhow::ensure!(super::is_uuid(&trace_id), "invalid build UUID");
        let request = Self {
            trace_id,
            source,
            identity,
            #[cfg(not(unix))]
            source_digest,
        };
        anyhow::ensure!(
            FileIdentity::read(&request.source)? == request.identity,
            "event log changed while reading its header"
        );
        Ok(request)
    }

    #[cfg(test)]
    fn unchanged(&self) -> bool {
        self.unchanged_cancellable(None).unwrap_or(false)
    }

    fn unchanged_cancellable(&self, cancel: Option<&AtomicBool>) -> anyhow::Result<bool> {
        super::check_cancel(cancel)?;
        if FileIdentity::read(&self.source).ok().as_ref() != Some(&self.identity) {
            return Ok(false);
        }
        #[cfg(not(unix))]
        {
            // Windows has no stable std API for change-time. Validate bytes
            // conservatively instead of trusting only size/mtime. This avoids
            // decompression and conversion, but does perform a sequential read.
            if content_digest(&self.source, cancel)? != self.source_digest {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Artifact {
    identity: ArtifactIdentity,
    capture: String,
}

impl Artifact {
    fn read(
        path: &Path,
        trace_id: &str,
        is_trace: bool,
        cancel: Option<&AtomicBool>,
    ) -> anyhow::Result<Self> {
        let identity = ArtifactIdentity::read(path, cancel)?;
        let mut reader = std::io::BufReader::new(File::open(path)?.take(MAX_METADATA_BYTES + 1));
        let mut line = String::new();
        if is_trace {
            reader.read_line(&mut line)?;
            anyhow::ensure!(line.trim() == "[", "trace array header missing");
            line.clear();
        }
        reader.read_line(&mut line)?;
        anyhow::ensure!(
            line.len() as u64 <= MAX_METADATA_BYTES,
            "artifact header too large"
        );
        let v: Value = serde_json::from_str(line.trim().trim_end_matches(','))?;
        let header = if is_trace {
            anyhow::ensure!(
                v["name"] == "run_start" && v["cat"] == "run",
                "not a run header"
            );
            &v["args"]
        } else {
            &v["graph"]
        };
        anyhow::ensure!(
            header["run"].as_str() == Some(trace_id),
            "build UUID mismatch"
        );
        let capture = header["capture"]
            .as_str()
            .filter(|c| c.starts_with("retrace-v1-") && c.len() == 51)
            .ok_or_else(|| anyhow::anyhow!("generation stamp missing"))?
            .to_owned();
        anyhow::ensure!(
            FileIdentity::read(path)? == identity.metadata,
            "artifact changed while reading its header"
        );
        Ok(Self { identity, capture })
    }
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    conversion_version: u32,
    request: Request,
    phases: bool,
    trace: Artifact,
    graph: Option<Artifact>,
    execution: ExecutionArtifact,
    status_lines: Vec<String>,
    warnings: Vec<String>,
    outcome: String,
    dep_capture: Value,
}

#[derive(Deserialize, Serialize)]
struct ExecutionArtifact {
    identity: ArtifactIdentity,
    store: crate::execution::StoreInfo,
}

fn receipt_path(out_dir: &Path, trace_id: &str) -> PathBuf {
    out_dir.join(format!("{trace_id}-ingest-cache.json"))
}

/// Whether this build was converted into `out_dir` before (a receipt exists, valid or not).
pub(super) fn has_receipt(out_dir: &Path, trace_id: &str) -> bool {
    receipt_path(out_dir, trace_id).exists()
}

fn read_receipt(out_dir: &Path, trace_id: &str) -> anyhow::Result<Receipt> {
    let mut raw = Vec::new();
    File::open(receipt_path(out_dir, trace_id))?
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut raw)?;
    anyhow::ensure!(
        raw.len() as u64 <= MAX_METADATA_BYTES,
        "cache receipt too large"
    );
    Ok(serde_json::from_slice(&raw)?)
}

/// An explicit build ID may reuse a previously converted arbitrary local
/// source without downloading another copy. Still require that exact source
/// to exist unchanged: a UUID alone cannot establish capture identity.
pub(super) fn lookup_id(
    id: &str,
    out_dir: &Path,
    phases: bool,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<Option<IngestOutput>> {
    super::check_cancel(cancel)?;
    let id = id.to_ascii_lowercase();
    anyhow::ensure!(super::is_uuid(&id), "invalid build UUID");
    let receipt = read_receipt(out_dir, &id)?;
    if receipt.conversion_version != CONVERSION_VERSION
        || receipt.phases != phases
        || receipt.request.trace_id != id
        || FileIdentity::read(&receipt.request.source).ok().as_ref()
            != Some(&receipt.request.identity)
    {
        return Ok(None);
    }
    // read_cancellable validates source contents on non-Unix platforms. Do
    // not perform the same full hash once here and again while reading it.
    let request = Request::read_cancellable(&super::infer(&receipt.request.source)?, cancel)?;
    anyhow::ensure!(request.trace_id == id, "cached source build UUID mismatch");
    lookup(&request, out_dir, phases, cancel)
}

pub(super) fn lookup(
    request: &Request,
    out_dir: &Path,
    phases: bool,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<Option<IngestOutput>> {
    super::check_cancel(cancel)?;
    let receipt = read_receipt(out_dir, &request.trace_id)?;
    anyhow::ensure!(
        receipt.conversion_version == CONVERSION_VERSION,
        "conversion version changed (cached {}, current {CONVERSION_VERSION})",
        receipt.conversion_version
    );
    anyhow::ensure!(receipt.request == *request, "source file identity changed");
    anyhow::ensure!(receipt.phases == phases, "phase-export option changed");
    let trace_path = out_dir.join(format!("{}-trace.json", request.trace_id));
    let graph_path = out_dir.join(format!("{}-graph.json", request.trace_id));
    if Artifact::read(&trace_path, &request.trace_id, true, cancel)? != receipt.trace {
        anyhow::bail!("trace artifact changed");
    }
    match &receipt.graph {
        Some(expected) => {
            let graph = Artifact::read(&graph_path, &request.trace_id, false, cancel)?;
            if &graph != expected || graph.capture != receipt.trace.capture {
                anyhow::bail!("graph artifact changed or belongs to another generation");
            }
        }
        None if graph_path.exists() => anyhow::bail!("unexpected graph sidecar"),
        None => {}
    }
    let execution_path = crate::execution::store_path(out_dir, &request.trace_id)?;
    if ArtifactIdentity::read(&execution_path, cancel)? != receipt.execution.identity {
        anyhow::bail!("execution artifact changed");
    }
    crate::execution::DetailReader::open(
        &execution_path,
        &request.trace_id,
        &receipt.trace.capture,
        &receipt.execution.store,
    )?;
    if !request.unchanged_cancellable(cancel)? {
        anyhow::bail!("source changed during cache validation");
    }
    let dep_capture = CaptureState::from_json(&receipt.dep_capture)
        .ok_or_else(|| anyhow::anyhow!("invalid cached capture state"))?;
    let outcome = match receipt.outcome.as_str() {
        "success" => Outcome::Success,
        "failed" => Outcome::Failed,
        "unknown" => Outcome::Unknown,
        _ => return Ok(None),
    };
    let mut status_lines = vec![format!(
        "retrace: reusing cached conversion for {} (event stream not decoded)",
        request.trace_id
    )];
    status_lines.extend(receipt.status_lines);
    Ok(Some(IngestOutput {
        trace_path,
        graph_path,
        status_lines,
        warnings: receipt.warnings,
        outcome,
        dep_capture,
    }))
}

pub(super) fn record(
    request: &Request,
    out_dir: &Path,
    phases: bool,
    out: &IngestOutput,
    expected_capture: &str,
    execution_info: &crate::execution::StoreInfo,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    let trace = Artifact::read(&out.trace_path, &request.trace_id, true, cancel)?;
    anyhow::ensure!(
        trace.capture == expected_capture,
        "published artifacts are not this conversion's generation"
    );
    let graph = if out.graph_path.exists() {
        let g = Artifact::read(&out.graph_path, &request.trace_id, false, cancel)?;
        anyhow::ensure!(g.capture == trace.capture, "artifact generation mismatch");
        Some(g)
    } else {
        None
    };
    let execution_path = crate::execution::store_path(out_dir, &request.trace_id)?;
    let execution = ExecutionArtifact {
        identity: ArtifactIdentity::read(&execution_path, cancel)?,
        store: execution_info.clone(),
    };
    crate::execution::DetailReader::open(
        &execution_path,
        &request.trace_id,
        expected_capture,
        execution_info,
    )?;
    anyhow::ensure!(
        request.unchanged_cancellable(cancel)?,
        "source changed before cache publication"
    );
    // Serialize by reference so bookkeeping never clones an action graph.
    let receipt = serde_json::json!({
        "conversion_version": CONVERSION_VERSION,
        "request": request,
        "phases": phases,
        "trace": trace,
        "graph": graph,
        "execution": execution,
        "status_lines": out.status_lines,
        "warnings": out.warnings,
        "outcome": out.outcome.as_str(),
        "dep_capture": out.dep_capture.to_json(),
    });
    let staged = Staged::beside(receipt_path(out_dir, &request.trace_id));
    let mut file = super::private_file(&staged.write_to)?;
    serde_json::to_writer_pretty(&mut file, &receipt)?;
    file.write_all(b"\n")?;
    file.flush()?;
    drop(file);
    staged.publish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::Source;
    use crate::ingest::run;
    use crate::ingest::run_cached;
    use crate::ingest::testlog;

    fn dir(stem: &str) -> PathBuf {
        let d = crate::temp_path(stem);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn reused(out: &IngestOutput) -> bool {
        out.status_lines
            .iter()
            .any(|s| s.contains("reusing cached conversion"))
    }

    #[test]
    fn content_hash_checks_cancellation_between_bounded_reads() {
        use std::sync::atomic::Ordering;

        struct CancelAfterRead<'a> {
            cancel: &'a AtomicBool,
            reads: usize,
        }

        impl Read for CancelAfterRead<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(buffer.len() <= 1024 * 1024);
                self.reads += 1;
                buffer.fill(7);
                self.cancel.store(true, Ordering::Relaxed);
                Ok(buffer.len())
            }
        }

        let cancel = AtomicBool::new(false);
        let mut reader = CancelAfterRead {
            cancel: &cancel,
            reads: 0,
        };
        let error = digest_reader(&mut reader, Some(&cancel)).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert_eq!(
            reader.reads, 1,
            "do not continue reading after cancellation"
        );
        assert!(digest_reader(&mut reader, Some(&cancel)).is_err());
        assert_eq!(
            reader.reads, 1,
            "an already cancelled request reads nothing"
        );

        let expected = [
            0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
            0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
        ];
        assert_eq!(digest_reader(&mut &b"abc"[..], None).unwrap(), expected);
    }

    #[test]
    fn conservative_artifact_identity_detects_preserved_metadata_edits() {
        let d = dir("cache-artifact-content-identity");
        let path = d.join("artifact.json");
        let original = b"{\"run\":\"same\",\"capture\":\"same\"}\n{\"dur\":1}\n";
        let changed = b"{\"run\":\"same\",\"capture\":\"same\"}\n{\"dur\":2}\n";
        assert_eq!(original.len(), changed.len());
        std::fs::write(&path, original).unwrap();
        // Exercise the conservative policy on every test platform, not only
        // where metadata lacks Unix change-time/inode identity.
        let mut before = ArtifactIdentity::read_with_policy(&path, None, true).unwrap();
        std::fs::write(&path, changed).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(before.metadata.modified))
            .unwrap();
        let after = ArtifactIdentity::read_with_policy(&path, None, true).unwrap();
        assert_eq!(before.metadata.bytes, after.metadata.bytes);
        assert_eq!(before.metadata.modified, after.metadata.modified);
        assert_ne!(before.content_digest, after.content_digest);
        // Ignore richer Unix metadata for this assertion: the digest alone
        // must reject the otherwise matching length/mtime/header replacement.
        before.metadata = after.metadata.clone();
        assert_ne!(before, after);
        let cheap = ArtifactIdentity::read_with_policy(&path, None, false).unwrap();
        assert!(cheap.content_digest.is_none());
    }

    fn complete_no_graph_log(dir: &Path) -> PathBuf {
        use prost::Message as _;
        let path = testlog::cache_hit_log(dir);
        // The shared cache-hit fixture intentionally has only CommandEnd;
        // automatic reuse requires a decoded client Result as well.
        let result = buck2_cli_proto::CommandProgress {
            progress: Some(buck2_cli_proto::command_progress::Progress::Result(
                Box::new(buck2_cli_proto::CommandResult {
                    result: Some(buck2_cli_proto::command_result::Result::BuildResponse(
                        buck2_cli_proto::BuildResponse::default(),
                    )),
                }),
            )),
        }
        .encode_length_delimited_to_vec();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&result)
            .unwrap();
        path
    }

    #[test]
    fn first_open_is_not_reported_as_a_cache_miss() {
        let d = dir("first-open");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        let first = run_cached(&source, &d, &d, false).unwrap();
        assert!(!reused(&first));
        assert!(
            !first
                .status_lines
                .iter()
                .any(|line| line.contains("cache miss") || line.contains("os error")),
            "a log converted for the first time is not a cache miss: {:?}",
            first.status_lines
        );
    }

    #[test]
    fn cache_miss_reason_is_returned_once_not_replayed_from_the_receipt() {
        let d = dir("cache-miss-status");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        let forced = run(&source, &d, &d, false).unwrap();
        assert!(
            !forced
                .status_lines
                .iter()
                .any(|line| line.contains("cache miss:"))
        );

        let changed = run_cached(&source, &d, &d, true).unwrap();
        assert!(!reused(&changed));
        assert_eq!(
            changed
                .status_lines
                .iter()
                .filter(|line| line.contains("cache miss: phase-export option changed"))
                .count(),
            1,
            "GUI callers need the same specific cache-miss reason as the console"
        );
        let cached = run_cached(&source, &d, &d, true).unwrap();
        assert!(reused(&cached));
        assert!(
            !cached
                .status_lines
                .iter()
                .any(|line| line.contains("cache miss:"))
        );
    }

    #[test]
    fn completed_unknown_outcome_reuses_without_claiming_success() {
        use buck2_cli_proto::command_progress::Progress;
        use buck2_data::buck_event::Data;
        let d = dir("cache-unknown-result");
        let path = testlog::demo_log(&d, Some(1), true, true);
        testlog::rewrite(&path, |records| {
            records.retain(|record| {
                !matches!(&record.progress, Some(Progress::Event(event))
                if matches!(&event.data, Some(Data::SpanEnd(end))
                if matches!(&end.data, Some(buck2_data::span_end_event::Data::Command(_)))))
            });
            for record in records {
                if let Some(Progress::Result(result)) = &mut record.progress {
                    result.result = Some(buck2_cli_proto::command_result::Result::PingResponse(
                        Default::default(),
                    ));
                }
            }
        });
        let source = Source::Path(path);
        let first = run_cached(&source, &d, &d, false).unwrap();
        assert_eq!(first.outcome, Outcome::Unknown);
        let second = run_cached(&source, &d, &d, false).unwrap();
        assert_eq!(second.outcome, Outcome::Unknown);
        assert!(reused(&second));
    }

    #[test]
    fn waiting_for_another_writer_can_be_cancelled_without_touching_its_files() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering;
        use std::time::Duration;
        let d = dir("cache-cancel-lock");
        let held = super::super::WriterLock::acquire(&d, testlog::RUN_ID, "ingest", None).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let directory = d.clone();
        let worker = std::thread::spawn(move || {
            super::super::WriterLock::acquire(&directory, testlog::RUN_ID, "ingest", Some(&signal))
                .err()
                .unwrap()
                .to_string()
        });
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().contains("cancelled"));
        drop(held);
        assert!(super::super::WriterLock::acquire(&d, testlog::RUN_ID, "ingest", None).is_ok());
    }

    #[test]
    fn lock_child_probe() {
        let Some(path) = std::env::var_os("RETRACE_TEST_LOCK_PATH") else {
            return;
        };
        let file = File::options().read(true).write(true).open(path).unwrap();
        assert!(matches!(
            file.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
    }

    #[test]
    fn writer_lock_excludes_another_process() {
        let d = dir("cache-process-lock");
        let _held = super::super::WriterLock::acquire(&d, testlog::RUN_ID, "ingest", None).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ingest::cache::tests::lock_child_probe",
                "--nocapture",
            ])
            .env(
                "RETRACE_TEST_LOCK_PATH",
                d.join(format!("{}-ingest.lock", testlog::RUN_ID)),
            )
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "the child must actually run the lock probe: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[test]
    fn concurrent_cached_conversions_recheck_after_acquiring_the_lock() {
        let d = dir("cache-concurrent-convert");
        let log = testlog::demo_log(&d, Some(1), true, true);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let d = d.clone();
                let log = log.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    run_cached(&Source::Path(log), &d, &d, false).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| reused(r)).count(), 1);
        assert_eq!(results[0].dep_capture, results[1].dep_capture);
    }

    #[test]
    fn a_replaced_local_header_cannot_change_the_locked_publication_command() {
        use prost::Message as _;

        let d = dir("cache-replaced-header-under-lock");
        let path = testlog::demo_log(&d, Some(1), true, true);
        let log = super::super::infer(&path).unwrap();
        let locked_run = super::super::trace_id(&log).unwrap();
        let _writer = super::super::WriterLock::acquire(&d, &locked_run, "ingest", None).unwrap();
        let replacement_run = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let _other_writer =
            super::super::WriterLock::acquire(&d, replacement_run, "ingest", None).unwrap();
        let live_staging = d.join(format!("{replacement_run}-trace.json.live.partial"));
        std::fs::write(&live_staging, b"another active generation").unwrap();

        // Deterministically model a source replacement between the initial
        // header read and the post-lock request probe, without a timing race.
        let replacement = buck2_data::Invocation {
            trace_id: Some(replacement_run.to_owned()),
            ..Default::default()
        }
        .encode_length_delimited_to_vec();
        std::fs::write(&path, replacement).unwrap();
        let error = Request::read_for_locked_run(&log, &locked_run, None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("replaced while acquiring"), "{message}");
        assert!(message.contains(&locked_run) && message.contains(replacement_run));
        assert_eq!(
            std::fs::read(&live_staging).unwrap(),
            b"another active generation"
        );
        assert!(!d.join(format!("{locked_run}-trace.json")).exists());
        assert!(!d.join(format!("{replacement_run}-trace.json")).exists());
    }

    #[test]
    fn completed_pair_reopens_without_replacing_either_artifact() {
        let d = dir("cache-hit");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        let first = run_cached(&source, &d, &d, false).unwrap();
        assert!(!reused(&first));
        let t = FileIdentity::read(&first.trace_path).unwrap();
        let g = FileIdentity::read(&first.graph_path).unwrap();
        let epath = crate::execution::store_path(&d, testlog::RUN_ID).unwrap();
        let execution = FileIdentity::read(&epath).unwrap();
        let second = run_cached(&source, &d, &d, false).unwrap();
        assert!(reused(&second), "{:?}", second.warnings);
        assert_eq!(FileIdentity::read(&second.trace_path).unwrap(), t);
        assert_eq!(FileIdentity::read(&second.graph_path).unwrap(), g);
        assert_eq!(FileIdentity::read(&epath).unwrap(), execution);
        assert_eq!(first.dep_capture, second.dep_capture);
        assert_eq!(first.outcome, second.outcome);
        // Explicit export is also the refresh operation.
        assert!(!reused(&run(&source, &d, &d, false).unwrap()));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn missing_or_changed_execution_details_reconvert_instead_of_hiding_evidence() {
        let d = dir("cache-execution");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        run_cached(&source, &d, &d, false).unwrap();
        let path = crate::execution::store_path(&d, testlog::RUN_ID).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        assert!(reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        assert!(reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn options_source_changes_and_converter_versions_invalidate() {
        let d = dir("cache-invalidate");
        let log = testlog::demo_log(&d, Some(1), true, true);
        let source = Source::Path(log.clone());
        run_cached(&source, &d, &d, false).unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, true).unwrap()));
        assert!(reused(&run_cached(&source, &d, &d, true).unwrap()));
        let request = Request::read(&super::super::infer(&log).unwrap()).unwrap();
        let path = receipt_path(&d, testlog::RUN_ID);
        for version in [CONVERSION_VERSION - 1, CONVERSION_VERSION + 1] {
            let mut receipt: Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            receipt["conversion_version"] = serde_json::json!(version);
            std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            let refreshed = run_cached(&source, &d, &d, true).unwrap();
            assert!(!reused(&refreshed));
            let trace: Value =
                serde_json::from_slice(&std::fs::read(&refreshed.trace_path).unwrap()).unwrap();
            let action = trace
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["cat"] == "recipe")
                .unwrap();
            assert_eq!(action["args"]["activity"]["version"], 1);
            assert!(action["args"]["activity"]["spans"].is_array());
            assert!(action["args"]["span_id"].as_u64().is_some());
        }
        // Unix replacement changes inode identity even with preserved mtime.
        // Elsewhere byte-identical replacement may be reused, but changed
        // bytes must invalidate independently of their size and timestamps.
        let replacement = d.join("replacement.pb");
        std::fs::copy(&log, &replacement).unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(request.identity.modified))
            .unwrap();
        std::fs::rename(replacement, &log).unwrap();
        #[cfg(unix)]
        {
            assert!(!request.unchanged());
            assert!(!reused(&run_cached(&source, &d, &d, true).unwrap()));
        }
        #[cfg(not(unix))]
        {
            assert!(request.unchanged());
            assert!(reused(&run_cached(&source, &d, &d, true).unwrap()));
            let mut bytes = std::fs::read(&log).unwrap();
            *bytes.last_mut().unwrap() ^= 1;
            std::fs::write(&log, bytes).unwrap();
            File::options()
                .write(true)
                .open(&log)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(request.identity.modified))
                .unwrap();
            assert!(!request.unchanged());
        }
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn missing_changed_or_unreceipted_artifacts_reconvert() {
        let d = dir("cache-artifacts");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        let first = run_cached(&source, &d, &d, false).unwrap();
        std::fs::remove_file(&first.graph_path).unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&first.trace_path)
            .unwrap()
            .write_all(b" \n")
            .unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::remove_file(receipt_path(&d, testlog::RUN_ID)).unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        assert!(reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn incomplete_capture_is_not_cached_and_a_completed_replacement_is() {
        let d = dir("cache-incomplete");
        let log = testlog::demo_log(&d, Some(1), true, false);
        let source = Source::Path(log);
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        assert!(!receipt_path(&d, testlog::RUN_ID).exists());
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        let complete = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        assert!(!reused(&run_cached(&complete, &d, &d, false).unwrap()));
        assert!(reused(&run_cached(&complete, &d, &d, false).unwrap()));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn no_graph_capture_reuses_without_accepting_an_unrelated_sidecar() {
        let d = dir("cache-no-graph");
        let source = Source::Path(complete_no_graph_log(&d));
        let first = run_cached(&source, &d, &d, false).unwrap();
        assert!(!first.graph_path.exists());
        assert!(reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::write(&first.graph_path, b"unrelated graph").unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        assert!(!first.graph_path.exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn build_id_probe_does_not_decode_a_broken_event_body() {
        use prost::Message as _;
        let d = dir("cache-header-only");
        let log = d.join("header-only.pb");
        let mut bytes = buck2_data::Invocation {
            trace_id: Some(testlog::RUN_ID.to_owned()),
            ..Default::default()
        }
        .encode_length_delimited_to_vec();
        bytes.extend_from_slice(&[255, 255, 255]);
        std::fs::write(&log, bytes).unwrap();
        let request = Request::read(&super::super::infer(&log).unwrap()).unwrap();
        assert_eq!(request.trace_id, testlog::RUN_ID);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn build_id_reuses_a_valid_local_source_before_downloading() {
        let d = dir("cache-build-id");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        run_cached(&source, &d, &d, false).unwrap();
        // d is not a Buck checkout and has no UUID.pb.zst download. The
        // receipt names the arbitrary local source from the first open.
        let out = run_cached(&Source::TraceId(testlog::RUN_ID.to_owned()), &d, &d, false).unwrap();
        assert!(reused(&out));
        assert!(!d.join(format!("{}.pb.zst", testlog::RUN_ID)).exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn explicitly_requested_build_id_must_match_the_header() {
        let d = dir("cache-wrong-build-id");
        std::fs::write(d.join(".buckconfig"), b"").unwrap();
        let logs = d.join("buck-out/v2/log");
        std::fs::create_dir_all(&logs).unwrap();
        let log = testlog::demo_log(&d, Some(1), true, true);
        let wrong = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        std::fs::copy(log, logs.join(format!("stamp_build_{wrong}_events.pb"))).unwrap();
        let error = run_cached(&Source::TraceId(wrong.to_owned()), &d, &d, false).unwrap_err();
        assert!(error.to_string().contains("requested build"), "{error:#}");
        assert!(!d.join(format!("{wrong}-trace.json")).exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn corrupt_receipt_and_wrong_emitted_generation_cannot_become_hits() {
        let d = dir("cache-generation");
        let log = testlog::demo_log(&d, Some(1), true, true);
        let source = Source::Path(log.clone());
        let out = run_cached(&source, &d, &d, false).unwrap();
        let request = Request::read(&super::super::infer(&log).unwrap()).unwrap();
        let stored = crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        let execution_info = stored.runs[0].execution_store.as_ref().unwrap();
        assert!(
            record(
                &request,
                &d,
                false,
                &out,
                "not-the-emitted-generation",
                execution_info,
                None,
            )
            .is_err()
        );
        let receipt = receipt_path(&d, testlog::RUN_ID);
        std::fs::write(&receipt, b"{torn receipt").unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        assert!(reused(&run_cached(&source, &d, &d, false).unwrap()));
        // Receipt publication failure does not discard usable output.
        std::fs::remove_file(&receipt).unwrap();
        std::fs::create_dir(&receipt).unwrap();
        let out = run_cached(&source, &d, &d, false).unwrap();
        assert!(out.trace_path.is_file() && out.graph_path.is_file());
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("could not record reusable conversion"))
        );
        crate::parse::parse(out.trace_path.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn same_uuid_different_capture_and_mixed_pair_cannot_reuse() {
        let d = dir("cache-capture-a");
        let other = dir("cache-capture-b");
        let source = Source::Path(testlog::demo_log(&d, Some(1), true, true));
        let first = run_cached(&source, &d, &d, false).unwrap();
        let different = Source::Path(complete_no_graph_log(&other));
        let second = run_cached(&different, &d, &other, false).unwrap();
        assert!(!reused(&second));
        assert!(!second.graph_path.exists());
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        // Pair a completed trace with the graph from an incomplete capture.
        let torn = run(
            &Source::Path(testlog::demo_log(&other, Some(1), true, false)),
            &other,
            &other,
            false,
        )
        .unwrap();
        std::fs::copy(torn.graph_path, &first.graph_path).unwrap();
        assert!(!reused(&run_cached(&source, &d, &d, false).unwrap()));
        std::fs::remove_dir_all(d).unwrap();
        std::fs::remove_dir_all(other).unwrap();
    }
}
