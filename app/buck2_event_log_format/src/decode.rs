/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::CommandProgress;
use buck2_cli_proto::command_progress;
use buck2_error::BuckErrorContext;
use bytes::BytesMut;
use prost::Message;

use crate::stream_value::StreamValue;

/// Decodes one line of a JSON event log.
pub fn json_line(line: &str) -> serde_json::Result<StreamValue> {
    serde_json::from_str(line)
}

/// Decodes one length-delimited frame of a protobuf event log.
pub fn protobuf_frame(frame: BytesMut) -> buck2_error::Result<StreamValue> {
    let progress = CommandProgress::decode_length_delimited(frame)
        .buck_error_context("Invalid CommandProgress")?;
    match progress.progress {
        Some(command_progress::Progress::Event(event)) => Ok(StreamValue::Event(event)),
        Some(command_progress::Progress::Result(result)) => Ok(StreamValue::Result(result)),
        Some(command_progress::Progress::PartialResult(result)) => {
            Ok(StreamValue::PartialResult(result))
        }
        None => Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::InvalidEvent,
            "Event type not recognized"
        )),
    }
}
