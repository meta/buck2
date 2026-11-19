/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Lazy, generation-bound command and output evidence. Large log strings are
//! written once during ordered ingestion, never retained in the action graph.
//! References in the trace address a sibling file, not paths supplied by logs.

use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::io::Read as _;
use std::io::Seek as _;
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::FileExt as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(windows)]
use std::os::windows::fs::FileExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use sha1::Digest as _;

use crate::types::Trace;

const HEADER_BYTES: u64 = 512;
pub const PREVIEW_BYTES: usize = 64 * 1024;
const METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 32_768;

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
pub struct DetailRef {
    pub offset: u64,
    pub len: u64,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
pub struct Command {
    pub argv: Vec<String>,
    /// Explicit logged values, not a claim to include inherited environment.
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub fallback_exe: Vec<String>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
#[serde(rename_all = "snake_case")]
pub enum OutputState {
    Recorded,
    /// Only for an explicitly measured empty stream, not a default proto string.
    Empty,
    EmptyOrUnavailable,
    #[default]
    NotRecorded,
    ProducerNotice,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
pub struct Output {
    pub state: OutputState,
    pub content: Option<DetailRef>,
    pub note: Option<String>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
pub struct Attempt {
    pub index: u32,
    pub status: String,
    pub executor: String,
    pub action_digest: Option<String>,
    pub use_case: Option<String>,
    pub exit_code: Option<i32>,
    pub start_us: Option<i64>,
    pub wall_time_us: Option<i64>,
    pub command: Option<Command>,
    pub stdout: Output,
    pub stderr: Output,
    pub error: Option<String>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
pub struct CommandObservation {
    pub span_id: u64,
    pub start_us: i64,
    pub executor: String,
    pub action_digest: Option<String>,
    pub command: Command,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
pub struct ActionManifest {
    pub span_id: u64,
    pub reports: Vec<DetailRef>,
    pub observations: Vec<DetailRef>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    allocative::Allocative
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "records are decoded and written one at a time: boxing the frequent `Attempt` \
              variant would add a heap allocation per action to save transient stack space"
)]
pub enum Record {
    Attempt(Attempt),
    Command(CommandObservation),
    Action(ActionManifest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreInfo {
    pub sha1: String,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Header {
    schema: u32,
    run: String,
    capture: String,
    store: StoreInfo,
}

pub fn store_path(directory: &Path, run: &str) -> anyhow::Result<PathBuf> {
    let valid = run.len() == 36
        && run.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    anyhow::ensure!(valid, "invalid execution-details build UUID");
    Ok(directory.join(format!("{run}-execution.bin")))
}

/// Created beside its publication destination by ingest::Staged. Dropping
/// that staging guard, rather than this writer, owns unsuccessful cleanup.
pub struct DetailWriter {
    writer: BufWriter<File>,
    hash: sha1::Sha1,
    bytes: u64,
    #[cfg(test)]
    fail_after: Option<u64>,
}

impl DetailWriter {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut writer = BufWriter::with_capacity(1024 * 1024, options.open(path)?);
        writer.write_all(&[b' '; HEADER_BYTES as usize])?;
        Ok(Self {
            writer,
            hash: sha1::Sha1::new(),
            bytes: HEADER_BYTES,
            #[cfg(test)]
            fail_after: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn fail_writes_after(&mut self, additional_bytes: u64) {
        self.fail_after = Some(self.bytes.saturating_add(additional_bytes));
    }

    pub fn append_text(&mut self, text: &str) -> anyhow::Result<DetailRef> {
        let offset = self.bytes;
        self.write_all(text.as_bytes())?;
        Ok(DetailRef {
            offset,
            len: self.bytes - offset,
        })
    }

    pub fn append(&mut self, record: &Record) -> anyhow::Result<DetailRef> {
        let offset = self.bytes;
        serde_json::to_writer(&mut *self, record)?;
        Ok(DetailRef {
            offset,
            len: self.bytes - offset,
        })
    }

    pub fn finish(mut self) -> anyhow::Result<StoreInfo> {
        self.writer.flush()?;
        Ok(StoreInfo {
            sha1: self
                .hash
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            bytes: self.bytes,
        })
    }
}

impl std::io::Write for DetailWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        if self
            .fail_after
            .is_some_and(|limit| self.bytes.saturating_add(buf.len() as u64) > limit)
        {
            return Err(std::io::Error::other(
                "injected execution store write failure",
            ));
        }
        let n = self.writer.write(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes = self
            .bytes
            .checked_add(n as u64)
            .ok_or_else(|| std::io::Error::other("execution store size overflow"))?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

/// The body hash participates in the trace's capture fingerprint; only this
/// fixed header is patched afterwards, avoiding a circular content hash.
pub fn stamp(path: &Path, run: &str, capture: &str, info: &StoreInfo) -> anyhow::Result<()> {
    let header = Header {
        schema: 1,
        run: run.to_owned(),
        capture: capture.to_owned(),
        store: info.clone(),
    };
    let mut raw = serde_json::to_vec(&header)?;
    anyhow::ensure!(
        raw.len() < HEADER_BYTES as usize,
        "execution store header too large"
    );
    raw.resize(HEADER_BYTES as usize, b' ');
    raw[HEADER_BYTES as usize - 1] = b'\n';
    let mut file = OpenOptions::new().write(true).open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() == info.bytes,
        "execution store length changed"
    );
    file.seek(std::io::SeekFrom::Start(0))?;
    file.write_all(&raw)?;
    file.flush()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl Stamp {
    fn read(file: &File) -> anyhow::Result<Self> {
        let m = file.metadata()?;
        anyhow::ensure!(m.is_file(), "execution details is not a regular file");
        Ok(Self {
            bytes: m.len(),
            modified: m.modified()?,
            #[cfg(unix)]
            identity: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
        })
    }
}

#[derive(Clone, Debug)]
pub struct DetailReader {
    file: Arc<File>,
    stamp: Stamp,
}

#[derive(Debug)]
pub struct TextPreview {
    pub text: String,
    pub total_bytes: u64,
    pub truncated: bool,
}

impl DetailReader {
    pub fn open(path: &Path, run: &str, capture: &str, info: &StoreInfo) -> anyhow::Result<Self> {
        let mut file = File::open(path)?;
        let stamp = Stamp::read(&file)?;
        anyhow::ensure!(
            stamp.bytes == info.bytes && stamp.bytes >= HEADER_BYTES,
            "execution-details length mismatch; regenerate the conversion"
        );
        let mut bytes = [0; HEADER_BYTES as usize];
        file.read_exact(&mut bytes)?;
        let header: Header = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            header.schema == 1
                && header.run == run
                && header.capture == capture
                && header.store == *info,
            "execution details belong to a different capture; reopen or regenerate the conversion"
        );
        let out = Self {
            file: Arc::new(file),
            stamp,
        };
        out.validate()?;
        Ok(out)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            Stamp::read(&self.file)? == self.stamp,
            "execution details changed while open; reopen the trace"
        );
        Ok(())
    }

    fn check(&self, reference: DetailRef) -> anyhow::Result<()> {
        self.validate()?;
        anyhow::ensure!(
            reference.offset >= HEADER_BYTES
                && reference
                    .offset
                    .checked_add(reference.len)
                    .is_some_and(|end| end <= self.stamp.bytes),
            "execution-details reference is out of bounds"
        );
        Ok(())
    }

    fn read_at(&self, mut offset: u64, mut bytes: &mut [u8]) -> anyhow::Result<()> {
        while !bytes.is_empty() {
            #[cfg(unix)]
            let n = self.file.read_at(bytes, offset)?;
            #[cfg(windows)]
            let n = self.file.seek_read(bytes, offset)?;
            anyhow::ensure!(n != 0, "execution details ended during a read");
            offset += n as u64;
            bytes = &mut bytes[n..];
        }
        Ok(())
    }

    pub fn preview(&self, reference: DetailRef, limit: usize) -> anyhow::Result<TextPreview> {
        self.check(reference)?;
        let n = reference.len.min(limit.min(PREVIEW_BYTES) as u64) as usize;
        let mut bytes = vec![0; n];
        self.read_at(reference.offset, &mut bytes)?;
        // An incomplete UTF-8 codepoint at the preview boundary isn't corrupt
        // stored text; leave it for a larger preview instead of adding U+FFFD.
        if let Err(e) = std::str::from_utf8(&bytes) {
            if (n as u64) < reference.len && e.error_len().is_none() {
                bytes.truncate(e.valid_up_to());
            } else {
                anyhow::bail!("execution details contain invalid UTF-8; regenerate the conversion");
            }
        }
        self.validate()?;
        let truncated = (bytes.len() as u64) < reference.len;
        Ok(TextPreview {
            // The writer stores strings. Reject corruption instead of lossy
            // expansion, which could exceed the bounded preview budget.
            text: String::from_utf8(bytes)?,
            total_bytes: reference.len,
            truncated,
        })
    }

    pub fn copy_to(&self, reference: DetailRef, path: &Path) -> anyhow::Result<()> {
        self.check(reference)?;
        write_new_file(path, |dest| {
            let mut left = reference.len;
            let mut offset = reference.offset;
            let mut buf = vec![0; 64 * 1024];
            while left > 0 {
                let n = left.min(buf.len() as u64) as usize;
                self.read_at(offset, &mut buf[..n])?;
                dest.write_all(&buf[..n])?;
                offset += n as u64;
                left -= n as u64;
            }
            self.validate()
        })
    }

    fn record(&self, reference: DetailRef, budget: &mut u64) -> anyhow::Result<Record> {
        self.check(reference)?;
        anyhow::ensure!(
            reference.len <= *budget,
            "execution metadata exceeds the bounded detail-view budget"
        );
        *budget -= reference.len;
        let mut raw = vec![0; reference.len as usize];
        self.read_at(reference.offset, &mut raw)?;
        let record = serde_json::from_slice(&raw)?;
        self.validate()?;
        Ok(record)
    }

    fn validate_output(&self, output: &Output) -> anyhow::Result<()> {
        if let Some(reference) = output.content {
            self.check(reference)?;
        }
        let consistent = match output.state {
            OutputState::Recorded | OutputState::ProducerNotice => {
                output.content.is_some_and(|r| r.len > 0)
            }
            OutputState::Empty => output.content.is_none_or(|r| r.len == 0),
            OutputState::EmptyOrUnavailable | OutputState::NotRecorded => output.content.is_none(),
        };
        anyhow::ensure!(
            consistent,
            "execution output state contradicts its stored content"
        );
        Ok(())
    }

    pub fn action(&self, reference: DetailRef, span: u64) -> anyhow::Result<LoadedAction> {
        let mut budget = METADATA_BYTES;
        let Record::Action(manifest) = self.record(reference, &mut budget)? else {
            anyhow::bail!("execution reference is not an action manifest");
        };
        anyhow::ensure!(
            manifest.span_id == span,
            "execution manifest names another action"
        );
        anyhow::ensure!(
            manifest
                .reports
                .len()
                .saturating_add(manifest.observations.len())
                <= MAX_RECORDS,
            "too many execution records for one detail view"
        );
        let mut attempts = Vec::with_capacity(manifest.reports.len());
        for (i, r) in manifest.reports.into_iter().enumerate() {
            let Record::Attempt(attempt) = self.record(r, &mut budget)? else {
                anyhow::bail!("command report reference has wrong record kind");
            };
            anyhow::ensure!(
                attempt.index as usize == i,
                "command report indices are not in recorded order"
            );
            self.validate_output(&attempt.stdout)?;
            self.validate_output(&attempt.stderr)?;
            attempts.push(attempt);
        }
        let mut observations = Vec::with_capacity(manifest.observations.len());
        for r in manifest.observations {
            let Record::Command(observation) = self.record(r, &mut budget)? else {
                anyhow::bail!("command observation reference has wrong record kind");
            };
            observations.push(observation);
        }
        Ok(LoadedAction {
            reader: self.clone(),
            attempts,
            observations,
        })
    }
}

#[derive(Debug)]
pub struct LoadedAction {
    pub reader: DetailReader,
    pub attempts: Vec<Attempt>,
    pub observations: Vec<CommandObservation>,
}

impl LoadedAction {
    /// Never zip observations to reports: several resource-control restarts
    /// can belong to one report and hybrid local/remote work can overlap.
    pub fn command_for(&self, index: usize) -> Option<&Command> {
        let attempt = self.attempts.get(index)?;
        if let Some(command) = &attempt.command {
            return Some(command);
        }
        let digest = attempt.action_digest.as_deref()?;
        let compatible = |o: &&CommandObservation| {
            o.executor == attempt.executor && o.action_digest.as_deref() == Some(digest)
        };
        let mut candidates = self.observations.iter().filter(compatible);
        let first = candidates.next()?;
        // Clock samples framing the local stage/report differ slightly. The
        // same command in every compatible observation is unambiguous without
        // claiming a particular restart was this command report.
        // Do not use report timestamps to disambiguate differing restarts:
        // the real start may slightly precede the report's clock sample.
        candidates
            .all(|o| o.command == first.command)
            .then_some(&first.command)
    }
}

/// Creates `path`, which must not already exist, readable only by the user, and fills it with
/// `write`. If writing fails the file is removed, so a failed save never leaves a partial copy.
pub fn write_new_file(
    path: &Path,
    write: impl FnOnce(&mut File) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    let result = write(&mut file).and_then(|()| Ok(file.flush()?));
    drop(file);
    if result.is_err() {
        std::fs::remove_file(path).ok();
    }
    result
}

pub fn load(trace: &Trace, record: &serde_json::Value) -> anyhow::Result<Option<LoadedAction>> {
    let args = &record["args"];
    let Some(execution) = args.get("execution") else {
        return Ok(None);
    };
    anyhow::ensure!(
        execution["schema"] == 1,
        "unsupported execution-details schema"
    );
    let reference = serde_json::from_value(execution["reference"].clone())?;
    let run = args["run"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("execution details missing build identity"))?;
    let span = args["span_id"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("execution details missing action identity"))?;
    let info = trace
        .runs
        .iter()
        .find(|r| r.run == run)
        .ok_or_else(|| anyhow::anyhow!("execution details missing run header"))?;
    let store = info
        .execution_store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("execution store descriptor missing"))?;
    let capture = info
        .capture
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("execution capture stamp missing"))?;
    trace.validate_source()?;
    let path = store_path(
        Path::new(&trace.path).parent().unwrap_or(Path::new(".")),
        run,
    )?;
    let loaded = DetailReader::open(&path, run, capture, store)?.action(reference, span)?;
    trace.validate_source()?;
    Ok(Some(loaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "06331c48-75f3-446b-8874-dc0769ab836b";
    const CAPTURE: &str = "retrace-v1-1111111111111111111111111111111111111111";

    fn fixture() -> (PathBuf, StoreInfo, DetailRef, DetailRef) {
        let path = crate::temp_path("execution.bin");
        let mut writer = DetailWriter::create(&path).unwrap();
        let text = writer
            .append_text(&format!("{}é", "x".repeat(PREVIEW_BYTES - 1)))
            .unwrap();
        let attempt = writer
            .append(&Record::Attempt(Attempt {
                index: 0,
                status: "Success".into(),
                executor: "local".into(),
                stdout: Output {
                    state: OutputState::NotRecorded,
                    ..Default::default()
                },
                stderr: Output {
                    state: OutputState::Recorded,
                    content: Some(text),
                    note: None,
                },
                ..Default::default()
            }))
            .unwrap();
        let reference = writer
            .append(&Record::Action(ActionManifest {
                span_id: 42,
                reports: vec![attempt],
                observations: vec![],
            }))
            .unwrap();
        let info = writer.finish().unwrap();
        stamp(&path, RUN, CAPTURE, &info).unwrap();
        (path, info, reference, text)
    }

    #[test]
    fn production_load_requires_matching_run_capture_span_and_source_generation() {
        struct Scratch {
            dir: PathBuf,
            trace: PathBuf,
            store: PathBuf,
        }
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_file(&self.trace).ok();
                std::fs::remove_file(&self.store).ok();
                std::fs::remove_dir(&self.dir).ok();
            }
        }
        let dir = crate::temp_path("execution-load");
        std::fs::create_dir(&dir).unwrap();
        let scratch = Scratch {
            trace: dir.join("trace.json"),
            store: store_path(&dir, RUN).unwrap(),
            dir,
        };
        let (path, info, reference, _) = fixture();
        std::fs::rename(path, &scratch.store).unwrap();
        std::fs::write(
            &scratch.trace,
            "[\n{\"ph\":\"X\",\"name\":\"action\",\"ts\":0,\"dur\":1}\n]\n",
        )
        .unwrap();
        let mut trace = crate::parse::parse(scratch.trace.to_str().unwrap()).unwrap();
        trace.runs.push(crate::types::RunInfo {
            run: RUN.into(),
            capture: Some(CAPTURE.into()),
            execution_store: Some(info),
            ..Default::default()
        });
        let record = serde_json::json!({"args": {
            "run": RUN, "span_id": 42,
            "execution": {"schema": 1, "reference": reference}
        }});
        let loaded = load(&trace, &record).unwrap().unwrap();
        assert_eq!(loaded.attempts.len(), 1);
        assert_eq!(loaded.attempts[0].stdout.state, OutputState::NotRecorded);
        assert_eq!(loaded.attempts[0].stderr.state, OutputState::Recorded);
        drop(loaded);
        assert!(
            load(&trace, &serde_json::json!({"args": {}}))
                .unwrap()
                .is_none()
        );
        let mut wrong = record.clone();
        wrong["args"]["span_id"] = serde_json::json!(43);
        assert!(load(&trace, &wrong).is_err());
        wrong = record.clone();
        wrong["args"]["execution"]["schema"] = serde_json::json!(2);
        assert!(load(&trace, &wrong).is_err());
        wrong = record.clone();
        wrong["args"]["run"] = serde_json::json!("00000000-0000-0000-0000-000000000000");
        assert!(load(&trace, &wrong).is_err());
        trace.runs[0].capture = Some("wrong-generation".into());
        assert!(load(&trace, &record).is_err());
        trace.runs[0].capture = Some(CAPTURE.into());
        std::fs::write(&scratch.trace, "[]\n").unwrap();
        assert!(load(&trace, &record).is_err());
    }

    #[test]
    fn lazy_execution_preview_is_bounded_utf8_safe_and_preserves_missingness() {
        let (path, info, reference, text) = fixture();
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        let action = reader.action(reference, 42).unwrap();
        assert_eq!(action.attempts[0].stdout.state, OutputState::NotRecorded);
        assert_eq!(action.attempts[0].stderr.state, OutputState::Recorded);
        let preview = reader.preview(text, usize::MAX).unwrap();
        assert_eq!(preview.text.len(), PREVIEW_BYTES - 1);
        assert_eq!(preview.total_bytes, PREVIEW_BYTES as u64 + 1);
        assert!(preview.truncated);
        assert!(!preview.text.contains(char::REPLACEMENT_CHARACTER));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn execution_preview_refuses_corrupt_utf8_instead_of_expanding_it() {
        let (path, info, _, text) = fixture();
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(std::io::SeekFrom::Start(text.offset)).unwrap();
        file.write_all(&[0xff]).unwrap();
        drop(file);
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        assert!(reader.preview(text, PREVIEW_BYTES).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn execution_output_state_and_reference_must_agree() {
        let (path, info, _, text) = fixture();
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        for (state, content, valid) in [
            (OutputState::Recorded, Some(text), true),
            (OutputState::ProducerNotice, Some(text), true),
            (OutputState::Recorded, None, false),
            (OutputState::Empty, None, true),
            (OutputState::Empty, Some(text), false),
            (OutputState::EmptyOrUnavailable, None, true),
            (OutputState::EmptyOrUnavailable, Some(text), false),
            (OutputState::NotRecorded, None, true),
            (OutputState::NotRecorded, Some(text), false),
            (
                OutputState::Recorded,
                Some(DetailRef { offset: 0, len: 1 }),
                false,
            ),
        ] {
            let output = Output {
                state,
                content,
                note: None,
            };
            assert_eq!(reader.validate_output(&output).is_ok(), valid, "{output:?}");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn execution_copy_is_exact_and_never_overwrites() {
        let (path, info, _, text) = fixture();
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        let destination = crate::temp_path("stderr.txt");
        reader.copy_to(text, &destination).unwrap();
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            format!("{}é", "x".repeat(PREVIEW_BYTES - 1))
        );
        assert!(reader.copy_to(text, &destination).is_err());
        assert_eq!(std::fs::metadata(&destination).unwrap().len(), text.len);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(destination).unwrap();
    }

    #[test]
    fn failed_write_leaves_no_file() {
        let destination = crate::temp_path("failed-save.txt");
        let result = write_new_file(&destination, |file| {
            file.write_all(b"partial")?;
            anyhow::bail!("disk full")
        });
        assert!(result.is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn execution_store_rejects_wrong_capture_span_and_bounds() {
        let (path, info, reference, _) = fixture();
        assert!(DetailReader::open(&path, RUN, "different", &info).is_err());
        assert!(DetailReader::open(&path, "another", CAPTURE, &info).is_err());
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        assert!(reader.action(reference, 43).is_err());
        for bad in [
            DetailRef { offset: 0, len: 1 },
            DetailRef {
                offset: u64::MAX,
                len: 2,
            },
            DetailRef {
                offset: HEADER_BYTES,
                len: u64::MAX,
            },
        ] {
            assert!(reader.preview(bad, 10).is_err());
        }
        assert!(store_path(Path::new("/tmp"), "../../escape").is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn execution_store_refuses_in_place_replacement() {
        let (path, info, reference, _) = fixture();
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert!(reader.action(reference, 42).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn execution_store_hash_covers_output_even_if_its_length_is_unchanged() {
        let first = crate::temp_path("first-execution.bin");
        let second = crate::temp_path("second-execution.bin");
        let mut a = DetailWriter::create(&first).unwrap();
        let mut b = DetailWriter::create(&second).unwrap();
        a.append_text("aaa").unwrap();
        b.append_text("bbb").unwrap();
        let a = a.finish().unwrap();
        let b = b.finish().unwrap();
        assert_eq!(a.bytes, b.bytes);
        assert_ne!(a.sha1, b.sha1);
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }

    #[test]
    fn command_matching_never_zips_hybrid_or_restart_observations() {
        let (path, info, reference, _) = fixture();
        let reader = DetailReader::open(&path, RUN, CAPTURE, &info).unwrap();
        let mut action = reader.action(reference, 42).unwrap();
        action.attempts[0].action_digest = Some("digest".into());
        action.attempts[0].start_us = Some(100);
        action.attempts[0].wall_time_us = Some(10);
        let command = Command {
            argv: vec!["rustc".into()],
            ..Default::default()
        };
        action.observations.push(CommandObservation {
            span_id: 1,
            start_us: 95,
            executor: "local".into(),
            action_digest: Some("digest".into()),
            command: command.clone(),
        });
        assert_eq!(
            action.command_for(0),
            Some(&command),
            "separate clock samples do not erase a unique recorded command"
        );
        let mut other = action.observations[0].clone();
        other.span_id = 2;
        other.start_us = 105;
        other.command.env.push(("RESTART".into(), "1".into()));
        action.observations.push(other);
        assert!(
            action.command_for(0).is_none(),
            "clock skew must not choose the later restart's environment"
        );
        action.observations.pop();
        action.attempts[0].executor = "remote".into();
        assert!(
            action.command_for(0).is_none(),
            "remote report cannot steal overlapping local argv"
        );
        std::fs::remove_file(path).unwrap();
    }
}
