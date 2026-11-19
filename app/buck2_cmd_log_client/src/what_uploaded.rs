/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::fmt::Formatter;
use std::io::Write;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ClientIoError;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_data::ReUploadMetrics;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_observer::display;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_hash::BuckMutMap;
use tokio_stream::StreamExt;

use crate::LogCommandOutputFormat;
use crate::LogCommandOutputFormatWithWriter;
use crate::transform_format;

/// Outputs stats about uploads to RE from the selected invocation.
#[derive(Debug, clap::Parser)]
pub struct WhatUploadedCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,
    #[clap(flatten)]
    output: LogCommandOutputFormat,
    #[clap(
        long = "aggregate-by-ext",
        help = "Aggregates the output by file extension"
    )]
    aggregate_by_extension: bool,
}

#[derive(serde::Serialize)]
struct ActionRecord {
    action: String,
    digests_uploaded: u64,
    bytes_uploaded: u64,
}

impl Display for ActionRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\t{}\t{}",
            self.action, self.digests_uploaded, self.bytes_uploaded
        )
    }
}

#[derive(serde::Serialize)]
struct ExtensionRecord {
    extension: String,
    digests_uploaded: u64,
    bytes_uploaded: u64,
}

impl Display for ExtensionRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\t{}\t{}",
            self.extension, self.digests_uploaded, self.bytes_uploaded
        )
    }
}

fn get_action_record(
    state: &BuckMutMap<u64, buck2_data::ActionExecutionStart>,
    upload: &ReUploadEvent,
) -> ActionRecord {
    let digests_uploaded = upload.inner.digests_uploaded.unwrap_or_default();
    let bytes_uploaded = upload.inner.bytes_uploaded.unwrap_or_default();
    let unknown = "unknown action".to_owned();
    let action_str = if let Some(action) = state.get(&upload.parent_span_id) {
        display::display_action_identity(
            action.key.as_ref(),
            action.name.as_ref(),
            TargetDisplayOptions::for_log(),
        )
        .unwrap_or(unknown)
    } else {
        unknown
    };
    ActionRecord {
        action: action_str,
        digests_uploaded,
        bytes_uploaded,
    }
}

fn print_uploads(
    output: &mut LogCommandOutputFormatWithWriter,
    record: &ActionRecord,
) -> Result<(), ClientIoError> {
    match output {
        LogCommandOutputFormatWithWriter::Readable(w)
        | LogCommandOutputFormatWithWriter::Tabulated(w) => Ok(writeln!(w, "{record}")?),
        LogCommandOutputFormatWithWriter::Csv(writer) => Ok(writer.serialize(record)?),
        LogCommandOutputFormatWithWriter::Json(w) => {
            serde_json::to_writer(w.by_ref(), &record)?;
            w.write_all("\n".as_bytes())?;
            Ok(())
        }
    }
}

fn print_extension_stats(
    output: &mut LogCommandOutputFormatWithWriter,
    stats_by_extension: &BuckMutMap<String, ReUploadMetrics>,
) -> Result<(), ClientIoError> {
    let mut records: Vec<ExtensionRecord> = stats_by_extension
        .iter()
        .map(|(ext, m)| ExtensionRecord {
            extension: ext.to_owned(),
            bytes_uploaded: m.bytes_uploaded,
            digests_uploaded: m.digests_uploaded,
        })
        .collect();
    records.sort_by_key(|a| a.bytes_uploaded);
    for record in records {
        match output {
            LogCommandOutputFormatWithWriter::Readable(w)
            | LogCommandOutputFormatWithWriter::Tabulated(w) => {
                writeln!(w, "{record}")?;
            }
            LogCommandOutputFormatWithWriter::Csv(writer) => writer.serialize(record)?,
            LogCommandOutputFormatWithWriter::Json(w) => {
                serde_json::to_writer(w.by_ref(), &record)?;
                w.write_all("\n".as_bytes())?;
            }
        }
    }
    Ok(())
}

struct ReUploadEvent<'a> {
    pub parent_span_id: u64,
    pub inner: &'a buck2_data::ReUploadEnd,
}

impl BuckSubcommand for WhatUploadedCommand {
    const COMMAND_NAME: &'static str = "log-what-uploaded";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self {
            event_log,
            output,
            aggregate_by_extension,
        } = self;

        buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
            let mut output = transform_format(output, w);
            let log_path = event_log.get(&ctx).await?;

            let (invocation, mut events) = log_path.unpack_stream().await?;
            buck2_client_ctx::eprintln!(
                "Showing uploads from: {}",
                invocation.display_command_line()
            )?;

            let mut total_digests_uploaded = 0;
            let mut total_bytes_uploaded = 0;
            let mut state = BuckMutMap::default();
            let mut stats_by_extension: BuckMutMap<String, ReUploadMetrics> = BuckMutMap::default();
            while let Some(event) = events.try_next().await? {
                match event {
                    // Insert parent span information so we can refer back to it later.
                    StreamValue::Event(event) => {
                        if let Some(buck2_data::buck_event::Data::SpanStart(start)) = &event.data
                            && let Some(buck2_data::span_start_event::Data::ActionExecution(
                                action,
                            )) = &start.data
                        {
                            state.insert(event.span_id, action.clone());
                        }

                        if let Some(buck2_data::buck_event::Data::SpanEnd(end)) = &event.data
                            && let Some(buck2_data::span_end_event::Data::ReUpload(u)) =
                                end.data.as_ref()
                        {
                            let upload = ReUploadEvent {
                                parent_span_id: event.parent_id,
                                inner: u,
                            };
                            if aggregate_by_extension {
                                for (extension, metrics) in &upload.inner.stats_by_extension {
                                    let entry = stats_by_extension
                                        .entry(extension.to_owned())
                                        .or_default();
                                    entry.bytes_uploaded += metrics.bytes_uploaded;
                                    entry.digests_uploaded += metrics.digests_uploaded;
                                }
                            } else {
                                let record = get_action_record(&state, &upload);
                                total_digests_uploaded += record.digests_uploaded;
                                total_bytes_uploaded += record.bytes_uploaded;
                                print_uploads(&mut output, &record)?;
                            }
                        }
                    }
                    StreamValue::Result(..) | StreamValue::PartialResult(..) => {}
                }
            }
            if aggregate_by_extension {
                print_extension_stats(&mut output, &stats_by_extension)?;
            } else {
                buck2_client_ctx::eprintln!(
                    "total: digests: {}, bytes: {}",
                    total_digests_uploaded,
                    total_bytes_uploaded
                )?;
            }

            Ok(())
        })
        .await?;
        ExitResult::success()
    }
}
