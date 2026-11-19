/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::Display;
use std::fmt::Formatter;
use std::io::Write;
use std::path::Path;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ClientIoError;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_event_log::stream_value::StreamValue;
use serde::Serialize;
use tokio_stream::StreamExt;

use crate::LogCommandOutputFormat;
use crate::LogCommandOutputFormatWithWriter;
use crate::transform_format;

/// Outputs materializations from selected invocation.
///
/// The output is a tab-separated list containing the path,
/// the materialization method, the file count, and the total size (after decompression).
#[derive(Debug, clap::Parser)]
pub struct WhatMaterializedCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,

    #[clap(
        long = "sort-by-size",
        short = 's',
        help = "Sort the output by total bytes in ascending order",
        conflicts_with = "aggregate_by_ext"
    )]
    sort_by_total_bytes: bool,

    /// Aggregates the output by file extension
    #[clap(long, conflicts_with = "sort_by_total_bytes")]
    aggregate_by_ext: bool,

    #[clap(flatten)]
    output: LogCommandOutputFormat,
}

#[derive(serde::Serialize)]
struct Record {
    path: String,
    method: &'static str,
    file_count: u64,
    total_bytes: u64,
    action_digest: Option<String>,
}

impl Display for Record {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\t{}\t{}\t{}\t{}",
            self.path,
            self.method,
            self.action_digest.as_deref().unwrap_or("none"),
            self.file_count,
            self.total_bytes
        )
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct AggregationKey<'a> {
    extension: &'a str,
    method: &'static str,
}

#[derive(serde::Serialize)]
struct AggregatedRecord<'a> {
    extension: &'a str,
    method: &'static str,
    file_count: u64,
    total_bytes: u64,
}

impl<'a> AggregatedRecord<'a> {
    fn update(&mut self, value: &Record) {
        self.file_count += value.file_count;
        self.total_bytes += value.total_bytes;
    }

    fn get_key(&self) -> AggregationKey<'a> {
        AggregationKey {
            extension: self.extension,
            method: self.method,
        }
    }
}

impl Display for AggregatedRecord<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\t{}\t{}\t{}",
            self.extension, self.method, self.file_count, self.total_bytes
        )
    }
}

impl<'a> From<&'a Record> for AggregatedRecord<'a> {
    fn from(value: &'a Record) -> Self {
        Self {
            extension: Path::new(&value.path)
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or("<empty>"),
            method: value.method,
            file_count: value.file_count,
            total_bytes: value.total_bytes,
        }
    }
}

fn write_output<T: Display + Serialize>(
    output: &mut LogCommandOutputFormatWithWriter,
    record: &T,
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

fn get_record(materialization: &buck2_data::MaterializationEnd) -> Record {
    let method = match materialization
        .method
        .and_then(|v| buck2_data::MaterializationMethod::try_from(v).ok())
    {
        Some(buck2_data::MaterializationMethod::CasDownload) => "cas",
        Some(buck2_data::MaterializationMethod::LocalCopy) => "copy",
        Some(buck2_data::MaterializationMethod::HttpDownload) => "http",
        Some(buck2_data::MaterializationMethod::Write) => "write",
        _ => "<unknown>",
    };
    Record {
        path: materialization.path.clone(),
        method,
        file_count: materialization.file_count,
        total_bytes: materialization.total_bytes,
        action_digest: materialization.action_digest.clone(),
    }
}

impl BuckSubcommand for WhatMaterializedCommand {
    const COMMAND_NAME: &'static str = "log-what-materialized";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self {
            event_log,
            output,
            sort_by_total_bytes,
            aggregate_by_ext,
        } = self;
        buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
            let mut output = transform_format(output, w);
            let log_path = event_log.get(&ctx).await?;

            let (invocation, mut events) = log_path.unpack_stream().await?;

            buck2_client_ctx::eprintln!(
                "Showing materializations from: {}",
                invocation.display_command_line()
            )?;

            let mut records: Vec<Record> = Vec::new();
            while let Some(event) = events.try_next().await? {
                match event {
                    StreamValue::Event(event) => match &event.data {
                        Some(buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                            data: Some(buck2_data::span_end_event::Data::Materialization(m)),
                            ..
                        })) if m.success =>
                        // Only log what has been materialized.
                        {
                            let record = get_record(m);
                            if sort_by_total_bytes || aggregate_by_ext {
                                records.push(record);
                            } else {
                                write_output(&mut output, &record)?;
                            }
                        }
                        _ => {}
                    },
                    StreamValue::Result(..) | StreamValue::PartialResult(..) => {}
                };
            }

            if aggregate_by_ext {
                let mut kv: BTreeMap<AggregationKey, AggregatedRecord> = BTreeMap::new();
                for r in records.iter() {
                    let v: AggregatedRecord = r.into();
                    let k = v.get_key();
                    kv.entry(k).and_modify(|e| e.update(r)).or_insert(v);
                }
                kv.iter()
                    .try_for_each(|(_, v)| write_output(&mut output, v))?;
            } else if sort_by_total_bytes {
                records.sort_by_key(|a| a.total_bytes);
                records
                    .iter()
                    .try_for_each(|r| write_output(&mut output, r))?;
            }

            buck2_error::Ok(())
        })
        .await?;
        ExitResult::success()
    }
}
