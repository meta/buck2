/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use axum::body::Bytes;
use buck2_cli_proto::CommandProgress;
use buck2_cli_proto::CommandProgressForWrite;
use buck2_cli_proto::command_progress;
use buck2_cli_proto::command_progress_for_write;
use buck2_data::buck_event::Data as EventData;
use buck2_error::BuckErrorContext;
use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_log::utils::Encoding;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_hash::IntentionallyStdHashMap;
use futures::StreamExt;
use prost::Message;
use serde::Serialize;

/// What the viewer gets told about the build up front. Mirrors
/// `InvocationInfo` in `nest/apps/trailcam/core/src/invocation.ts`; field
/// names and nullability must match it.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InvocationInfo {
    pub uuid: String,
    pub command: Option<String>,
    pub cli_args: Vec<String>,
    pub command_outcome: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub client: Option<String>,
    pub duration_ms: Option<f64>,
    pub command_duration_ms: Option<f64>,
    /// Unix seconds.
    pub start_time: Option<f64>,
    pub creation_time: Option<f64>,
    pub local_actions_count: Option<u64>,
    pub remote_actions_count: Option<u64>,
    pub skipped_actions_count: Option<u64>,
    pub cache_hit_count: Option<u64>,
    pub cache_hit_rate: Option<f64>,
    pub first_build_since_rebase: Option<bool>,
    pub error_messages: Vec<String>,
    pub buck2_revision: Option<String>,
    pub remote_execution_id: Option<String>,
    pub remote_execution: Option<RemoteExecutionStats>,
    pub event_log_ref: Option<String>,
    pub has_event_log: Option<bool>,
    pub re_log_ref: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteExecutionStats {
    pub upload_speed_max: Option<String>,
    pub upload_speed_avg: Option<String>,
    pub download_speed_max: Option<String>,
    pub download_speed_avg: Option<String>,
    pub bytes_uploaded: Option<String>,
    pub bytes_downloaded: Option<String>,
}

/// The ref `/api/invocation` hands out for the one log this server has; the
/// bundle's local backend ignores it and always asks for `/api/event-log`.
pub(crate) const EVENT_LOG_REF: &str = "event-log";

/// The log in the one encoding the viewer decodes: zstd-compressed,
/// length-delimited `CommandProgress`.
pub(crate) enum LogBody {
    /// The file on disk already has that encoding; stream it as is.
    File { path: AbsPathBuf, len: u64 },
    /// Re-encoded from another on-disk encoding.
    Bytes(Bytes),
}

pub(crate) struct LoadedLog {
    pub(crate) info: InvocationInfo,
    pub(crate) body: LogBody,
}

/// Reads the whole log once: to summarize it for `/api/invocation`, and to
/// re-encode it when it is not stored as protobuf+zstd.
pub(crate) async fn load(log: &EventLogPathBuf) -> buck2_error::Result<LoadedLog> {
    let needs_reencode = log.extension() != Encoding::PROTO_ZSTD.extensions[0];
    let mut encoder = if needs_reencode {
        Some(
            zstd::stream::write::Encoder::new(Vec::new(), 3)
                .buck_error_context("Creating zstd encoder")?,
        )
    } else {
        None
    };

    let (invocation, mut stream) = log
        .unpack_stream()
        .await
        .buck_error_context("Opening event log")?;
    let mut summary = Summary::default();
    while let Some(value) = stream.next().await {
        let value = value.buck_error_context("Reading event log")?;
        summary.observe(&value);
        if let Some(encoder) = &mut encoder {
            write_value(encoder, &value)?;
        }
    }

    let body = match encoder {
        Some(encoder) => LogBody::Bytes(Bytes::from(
            encoder
                .finish()
                .buck_error_context("Finishing zstd stream")?,
        )),
        None => {
            let path = log.path().to_owned();
            let len = tokio::fs::metadata(&path)
                .await
                .with_buck_error_context(|| format!("Reading size of {}", path.display()))?
                .len();
            LogBody::File { path, len }
        }
    };

    let span_ms = summary.span_ms();
    let mut info = summary.info;
    info.uuid = invocation.trace_id.to_string();
    if info.cli_args.is_empty() {
        info.cli_args = invocation.command_line_args.clone();
    }
    if info.start_time.is_none() {
        info.start_time = invocation
            .start_time
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
    }
    if info.duration_ms.is_none() {
        info.duration_ms = span_ms;
    }
    if info.command_outcome.is_none() {
        info.command_outcome = Some("RUNNING".to_owned());
    }
    info.event_log_ref = Some(EVENT_LOG_REF.to_owned());
    info.has_event_log = Some(true);
    Ok(LoadedLog { info, body })
}

fn write_value(out: &mut impl Write, value: &StreamValue) -> buck2_error::Result<()> {
    let mut buf = Vec::new();
    match value {
        // Same shape as `CommandProgress` on the wire, without cloning the event.
        StreamValue::Event(e) => CommandProgressForWrite {
            progress: Some(command_progress_for_write::Progress::Event(
                e.encode_to_vec(),
            )),
        }
        .encode_length_delimited(&mut buf)?,
        StreamValue::Result(r) => CommandProgress {
            progress: Some(command_progress::Progress::Result(r.clone())),
        }
        .encode_length_delimited(&mut buf)?,
        StreamValue::PartialResult(p) => CommandProgress {
            progress: Some(command_progress::Progress::PartialResult(p.clone())),
        }
        .encode_length_delimited(&mut buf)?,
    }
    out.write_all(&buf)
        .buck_error_context("Writing re-encoded event log")?;
    Ok(())
}

fn timestamp_secs(t: &prost_types::Timestamp) -> f64 {
    t.seconds as f64 + f64::from(t.nanos) / 1e9
}

fn command_name(start: &buck2_data::CommandStart) -> Option<&'static str> {
    use buck2_data::command_start::Data;
    Some(match start.data.as_ref()? {
        Data::Build(_) => "build",
        Data::Targets(_) => "targets",
        Data::Query(_) => "query",
        Data::Cquery(_) => "cquery",
        Data::Test(_) => "test",
        Data::Audit(_) => "audit",
        Data::Docs(_) => "docs",
        Data::Clean(_) => "clean",
        Data::Aquery(_) => "aquery",
        Data::Install(_) => "install",
        Data::Materialize(_) => "materialize",
        Data::Profile(_) => "profile",
        Data::Bxl(_) => "bxl",
        Data::Lsp(_) => "lsp",
        Data::FileStatus(_) => "file-status",
        Data::Starlark(_) => "starlark",
        Data::Subscribe(_) => "subscribe",
        Data::Trace(_) => "trace",
        Data::Ctargets(_) => "ctargets",
        Data::StarlarkDebugAttach(_) => "starlark-debug-attach",
        Data::Explain(_) => "explain",
        Data::ExpandExternalCell(_) => "expand-external-cell",
        Data::Complete(_) => "complete",
        Data::Hydration(_) => "hydration",
    })
}

/// Facts gathered from the event stream: the command start and end spans,
/// the command result, and the event timestamps. A log written by the buck2
/// client has no invocation record (that goes to telemetry only), so action
/// counts and cache statistics are left for the viewer to derive from the
/// events themselves.
#[derive(Default)]
struct Summary {
    info: InvocationInfo,
    first_event_secs: Option<f64>,
    last_event_secs: Option<f64>,
}

impl Summary {
    fn span_ms(&self) -> Option<f64> {
        Some((self.last_event_secs? - self.first_event_secs?) * 1e3)
    }

    fn observe(&mut self, value: &StreamValue) {
        match value {
            StreamValue::Event(event) => self.observe_event(event),
            StreamValue::Result(result) => self.observe_result(result),
            StreamValue::PartialResult(_) => {}
        }
    }

    fn observe_event(&mut self, event: &buck2_data::BuckEvent) {
        if let Some(t) = &event.timestamp {
            let secs = timestamp_secs(t);
            self.first_event_secs.get_or_insert(secs);
            self.last_event_secs = Some(secs);
        }
        match &event.data {
            Some(EventData::SpanStart(start)) => {
                if let Some(buck2_data::span_start_event::Data::Command(start)) = &start.data {
                    if let Some(name) = command_name(start) {
                        self.info.command = Some(name.to_owned());
                    }
                    if !start.cli_args.is_empty() {
                        self.info.cli_args = start.cli_args.clone();
                    }
                    self.observe_metadata(&start.metadata);
                }
            }
            _ => {}
        }
    }

    /// The command result closes a complete log and is the one place that
    /// says how the command ended: as an error, or as a response whose error
    /// list says whether the build or test run as a whole failed. These are
    /// the top-level errors buck2 printed after the build, as opposed to the
    /// per-action failures the viewer reads from the events.
    fn observe_result(&mut self, result: &buck2_cli_proto::CommandResult) {
        use buck2_cli_proto::command_result::Result;
        let (errors, failed): (Vec<&buck2_data::ErrorReport>, bool) = match &result.result {
            Some(Result::Error(e)) => (vec![e], true),
            Some(Result::BuildResponse(r)) => (r.errors.iter().collect(), !r.errors.is_empty()),
            Some(Result::TestResponse(r)) => {
                let errors: Vec<_> = r.build_errors.iter().chain(r.test_errors.iter()).collect();
                let failed = !errors.is_empty() || r.executor_exit_code != 0;
                (errors, failed)
            }
            Some(_) => (Vec::new(), false),
            None => return,
        };
        self.info.command_outcome = Some(if failed { "FAILURE" } else { "SUCCESS" }.to_owned());
        self.info.error_messages = errors.into_iter().map(|e| e.message.clone()).collect();
    }

    fn observe_metadata(&mut self, metadata: &IntentionallyStdHashMap<String, String>) {
        let get = |key: &str| metadata.get(key).filter(|v| !v.is_empty()).cloned();
        self.info.username = get("username").or(self.info.username.take());
        self.info.hostname = get("hostname").or(self.info.hostname.take());
        self.info.buck2_revision = get("buck2_revision").or(self.info.buck2_revision.take());
    }
}

#[cfg(test)]
mod tests {
    use buck2_cli_proto::command_result;

    use super::*;

    fn result(r: command_result::Result) -> StreamValue {
        StreamValue::Result(Box::new(buck2_cli_proto::CommandResult { result: Some(r) }))
    }

    fn error(message: &str) -> buck2_data::ErrorReport {
        buck2_data::ErrorReport {
            message: message.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn outcome_from_build_response() {
        let mut summary = Summary::default();
        summary.observe(&result(command_result::Result::BuildResponse(
            buck2_cli_proto::BuildResponse::default(),
        )));
        assert_eq!(summary.info.command_outcome.as_deref(), Some("SUCCESS"));
        assert!(summary.info.error_messages.is_empty());

        let mut summary = Summary::default();
        summary.observe(&result(command_result::Result::BuildResponse(
            buck2_cli_proto::BuildResponse {
                errors: vec![error("boom")],
                ..Default::default()
            },
        )));
        assert_eq!(summary.info.command_outcome.as_deref(), Some("FAILURE"));
        assert_eq!(summary.info.error_messages, vec!["boom".to_owned()]);
    }

    #[test]
    fn outcome_from_error_result() {
        let mut summary = Summary::default();
        summary.observe(&result(command_result::Result::Error(error("no daemon"))));
        assert_eq!(summary.info.command_outcome.as_deref(), Some("FAILURE"));
        assert_eq!(summary.info.error_messages, vec!["no daemon".to_owned()]);
    }

    #[test]
    fn command_start_fills_identity_and_timing() {
        let start = buck2_data::CommandStart {
            metadata: IntentionallyStdHashMap::from([
                ("username".to_owned(), "jd".to_owned()),
                ("hostname".to_owned(), "devvm".to_owned()),
            ]),
            cli_args: vec!["buck2".to_owned(), "build".to_owned()],
            data: Some(buck2_data::command_start::Data::Build(
                buck2_data::BuildCommandStart::default(),
            )),
            ..Default::default()
        };
        let event = buck2_data::BuckEvent {
            timestamp: Some(prost_types::Timestamp {
                seconds: 10,
                nanos: 500_000_000,
            }),
            data: Some(EventData::SpanStart(buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Command(start)),
            })),
            ..Default::default()
        };
        let mut summary = Summary::default();
        summary.observe(&StreamValue::Event(Box::new(event)));
        assert_eq!(summary.info.command.as_deref(), Some("build"));
        assert_eq!(summary.info.username.as_deref(), Some("jd"));
        assert_eq!(summary.info.hostname.as_deref(), Some("devvm"));
        assert_eq!(summary.info.cli_args, vec!["buck2", "build"]);
        assert_eq!(summary.first_event_secs, Some(10.5));
        // No result yet: the log is incomplete.
        assert_eq!(summary.info.command_outcome, None);
    }
}
