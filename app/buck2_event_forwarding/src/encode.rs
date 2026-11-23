/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The messages a helper reads, encoded and compressed as in a `.pb.zst` event log.

use async_compression::Level;
use async_compression::tokio::write::ZstdEncoder;
use buck2_cli_proto::CommandResult;
use buck2_event_log::utils::Invocation;
use buck2_event_log::write::StreamValueForWrite;
use buck2_server::command_observer::StartedCommand;
use dupe::Dupe;
use tokio::io::AsyncWrite;

/// Compresses what is written to `input` into one zstd frame.
pub(crate) fn compressed<W: AsyncWrite>(input: W) -> ZstdEncoder<W> {
    ZstdEncoder::with_quality(input, Level::Default)
}

/// The log's header.
pub(crate) fn header(command: &StartedCommand<'_>) -> Vec<u8> {
    let invocation = Invocation {
        command_line_args: command.sanitized_argv.to_vec(),
        expanded_command_line_args: Vec::new(),
        working_dir: command.working_dir.to_owned(),
        trace_id: command.trace_id.dupe(),
        start_time: Some(command.start_time),
    };
    let mut buf = Vec::new();
    invocation
        .encode_for_protobuf_log(&mut buf)
        .expect("encoding into a `Vec` should not fail");
    buf
}

pub(crate) fn result(result: &CommandResult) -> Vec<u8> {
    encode(StreamValueForWrite::Result(result))
}

fn encode(value: StreamValueForWrite<'_>) -> Vec<u8> {
    let mut buf = Vec::new();
    value
        .encode_for_protobuf_log(&mut buf)
        .expect("encoding into a `Vec` should not fail");
    buf
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::SystemTime;

    use buck2_cli_proto::command_progress::Progress;
    use buck2_cli_proto::command_result;
    use buck2_core::fs::project::ProjectRoot;
    use buck2_event_log::read::EventLogPathBuf;
    use buck2_event_log::stream_value::StreamValue;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::abs_path::AbsPathBuf;
    use buck2_settings::EventForwarding;
    use buck2_wrapper_common::invocation_id::TraceId;
    use futures::TryStreamExt;
    use tokio::io::AsyncWriteExt;

    use super::*;

    const ARGV: [&str; 3] = ["buck2", "build", "//foo:bar"];
    const WORKING_DIR: &str = "/repo/foo";

    fn start_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(1792164531123)
    }

    /// Encodes the header of a command with `trace_id`.
    fn header_of(trace_id: &TraceId) -> Vec<u8> {
        let argv = ARGV.map(str::to_owned);
        let project_root = ProjectRoot::new_unchecked(AbsNormPathBuf::unchecked_new(
            PathBuf::from(if cfg!(windows) { "C:\\repo" } else { "/repo" }),
        ));
        header(&StartedCommand {
            trace_id,
            sanitized_argv: &argv,
            working_dir: WORKING_DIR,
            project_root: &project_root,
            event_forwarding_keys: &["key".to_owned()],
            event_forwarding_settings: &EventForwarding::default(),
            start_time: start_time(),
        })
    }

    fn build_result() -> CommandResult {
        CommandResult {
            result: Some(command_result::Result::BuildResponse(
                buck2_cli_proto::BuildResponse {
                    project_root: "/repo".to_owned(),
                    ..Default::default()
                },
            )),
        }
    }

    #[tokio::test]
    async fn the_messages_make_an_event_log_that_buck2_reads() {
        let trace_id = TraceId::new();
        let mut log = compressed(Vec::new());
        log.write_all(&header_of(&trace_id)).await.unwrap();
        log.write_all(&result(&build_result())).await.unwrap();
        log.shutdown().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{trace_id}.pb.zst"));
        std::fs::write(&path, log.into_inner()).unwrap();

        let (invocation, values) = EventLogPathBuf::infer(AbsPathBuf::new(path).unwrap())
            .unwrap()
            .unpack_stream()
            .await
            .unwrap();
        let values: Vec<_> = values
            .map_ok(|value| match value {
                StreamValue::Event(event) => Progress::Event(event),
                StreamValue::Result(result) => Progress::Result(result),
                StreamValue::PartialResult(_) => panic!("no partial results were written"),
            })
            .try_collect()
            .await
            .unwrap();
        assert_eq!(
            (
                invocation.command_line_args,
                invocation.working_dir,
                invocation.trace_id,
                invocation.start_time,
            ),
            (
                ARGV.map(str::to_owned).to_vec(),
                WORKING_DIR.to_owned(),
                trace_id,
                Some(start_time()),
            )
        );
        assert_eq!(values, vec![Progress::Result(Box::new(build_result())),]);
    }
}
