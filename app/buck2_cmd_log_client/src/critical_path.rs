/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::io::Write;
use std::time::Duration;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ClientIoError;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_error::conversion::from_any_with_tag;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_observer::display::CriticalPathEntryDisplay;
use buck2_event_observer::display::TargetDisplayOptions;
use serde::Serialize;
use tokio_stream::StreamExt;

use crate::LogCommandOutputFormat;
use crate::LogCommandOutputFormatWithWriter;
use crate::transform_format;

/// Show the critical path for a selected build.
///
/// This produces output listing every node on the critical path.
///
/// It includes the kind of node, its name, category and identifier, as well as total duration
/// (runtime of this node), user duration (duration the user can improve), potential improvement
/// before this node stops being on the critical path, non-critical path time, and start offset.
///
/// Entries whose value was reused from a previous command rather than computed have `(reused)`
/// appended to their kind in the `readable` and `tabulated` formats. The `json` and `csv` formats
/// leave the kind untouched and carry reuse as a boolean `reused` field, emitted as the last
/// `csv` column.
///
/// The `readable` format produces space-aligned columnar output with a header:
/// `<start_offset> <total> <waiting> <user> <potential> <kind>[(reused)] <name> <category> <identifier> <execution_kind>`
///
/// The `tabulated` format produces tab-delimited output:
/// `<kind>[(reused)]\t<name>\t<category>\t<identifier>\t<execution_kind>\t<total_duration>\t<user_duration>\t<potential_improvement_duration>\t<non_critical_path_time>\t<start_offset>`
///
/// All durations are in microseconds. Start offset is in microseconds from the beginning of the build.
#[derive(Debug, clap::Parser)]
pub struct CriticalPathCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,
    #[clap(flatten)]
    format: LogCommandOutputFormat,
}

impl BuckSubcommand for CriticalPathCommand {
    const COMMAND_NAME: &'static str = "log-critical-path";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self { event_log, format } = self;
        log_critical_path_command_exec(ctx, event_log, format, PathKind::Critical).await
    }
}

/// Show the slowest path for a selected build.
///
/// While the critical path represents something closer to the "ideal" build time, the slowest path gives a
/// better idea of what the build actually spends its time on. This can better highlight build overhead.
///
/// This produces output listing every node on the slowest path.
///
/// It includes the kind of node, its name, category and identifier, as well as total duration
/// (runtime of this node), user duration (duration the user can improve), non-critical time, and start offset.
///
/// Entries whose value was reused from a previous command rather than computed have `(reused)`
/// appended to their kind in the `readable` and `tabulated` formats. The `json` and `csv` formats
/// leave the kind untouched and carry reuse as a boolean `reused` field, emitted as the last
/// `csv` column.
///
/// The `readable` format produces space-aligned columnar output with a header:
/// `<start_offset> <total> <waiting> <user> <potential> <kind>[(reused)] <name> <category> <identifier> <execution_kind>`
///
/// The `tabulated` format produces tab-delimited output:
/// `<kind>[(reused)]\t<name>\t<category>\t<identifier>\t<execution_kind>\t<total_duration>\t<user_duration>\t<potential_improvement_duration>\t<non_critical_path_time>\t<start_offset>`
///
/// All durations are in microseconds. Start offset is in microseconds from the beginning of the build.
///
/// Note that this prints a "potential improvement" just like `log critical-path`, but for slowest paths it's not computed.
#[derive(Debug, clap::Parser)]
pub struct SlowestPathCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,
    #[clap(flatten)]
    format: LogCommandOutputFormat,
}

impl BuckSubcommand for SlowestPathCommand {
    const COMMAND_NAME: &'static str = "log-slowest-path";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let Self { event_log, format } = self;
        log_critical_path_command_exec(ctx, event_log, format, PathKind::Slowest).await
    }
}

/// Distinguishes between critical path and slowest path computation.
enum PathKind {
    /// Critical path: the longest dependency chain determining minimum build time.
    Critical,
    /// Slowest path: the actual longest path taken, including build overhead.
    Slowest,
}

async fn log_critical_path_command_exec(
    ctx: ClientCommandContext<'_>,
    event_log: EventLogOptions,
    format: LogCommandOutputFormat,
    path_kind: PathKind,
) -> ExitResult {
    let log_path = event_log.get(&ctx).await?;

    let (invocation, mut events) = log_path.unpack_stream().await?;
    let path_name = match path_kind {
        PathKind::Critical => "critical path",
        PathKind::Slowest => "slowest path",
    };
    buck2_client_ctx::eprintln!(
        "Showing {} from: {}",
        path_name,
        invocation.display_command_line()
    )?;

    while let Some(event) = events.try_next().await? {
        if let StreamValue::Event(event) = event
            && let Some(buck2_data::buck_event::Data::Instant(instant)) = event.data
            && let Some(buck2_data::instant_event::Data::BuildGraphInfo(build_graph)) = instant.data
        {
            match path_kind {
                PathKind::Critical => {
                    log_critical_path(&build_graph.critical_path2, format.clone()).await?;
                }
                PathKind::Slowest => {
                    log_critical_path(&build_graph.slowest_path, format.clone()).await?;
                }
            }
        }
    }

    ExitResult::success()
}

#[derive(Default)]
struct OptionalDuration {
    inner: Option<Duration>,
}

impl OptionalDuration {
    fn new<T, E>(d: Option<T>) -> Result<Self, E>
    where
        T: TryInto<Duration, Error = E>,
    {
        Ok(Self {
            inner: d.map(|d| d.try_into()).transpose()?,
        })
    }
}

impl fmt::Display for OptionalDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(inner) = self.inner {
            inner.as_micros().fmt(f)?;
        } else {
            "".fmt(f)?;
        }
        Ok(())
    }
}

impl Serialize for OptionalDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let Some(micros) = self.inner.map(|d| d.as_micros()) {
            serializer.serialize_some(&micros)
        } else {
            serializer.serialize_none()
        }
    }
}

/// Represents a single entry on the critical path.
///
/// Contains information about the node type, timing information, and metadata.
#[derive(Default, Serialize)]
struct CriticalPathEntry<'a> {
    /// The kind of critical path entry (e.g., "action", "analysis", "materialization").
    kind: &'a str,
    /// The name/label of the entry (e.g., target label, package name).
    /// Empty string for entries without names.
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    /// Optional category (e.g., for actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    /// Optional identifier (e.g., action identifier, file path for materializations).
    #[serde(skip_serializing_if = "Option::is_none")]
    identifier: Option<&'a str>,
    /// Optional execution kind for actions (e.g., "local", "remote").
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_kind: Option<&'a str>,
    /// Total wall-clock duration of this entry.
    total_duration: OptionalDuration,
    /// User-improvable duration (portion the user can optimize).
    user_duration: OptionalDuration,
    /// Potential improvement duration before this node drops off the critical path.
    potential_improvement_duration: OptionalDuration,
    /// Time spent off the critical path (non-critical path duration).
    non_critical_path_time: OptionalDuration,
    /// Start offset in microseconds from the beginning of the build.
    start_offset: u64,
    /// Whether this entry's DICE value was reused from a previous command rather than computed.
    /// Kept as the last field so the new `csv` column appends to the pre-existing columns.
    reused: bool,
}

/// A csv row for a critical path entry. The csv writer takes the header from the first row and
/// rejects rows of another width, so unlike the JSON output every column is always present.
#[derive(Serialize)]
struct CriticalPathCsvRow<'a> {
    kind: &'a str,
    name: &'a str,
    category: Option<&'a str>,
    identifier: Option<&'a str>,
    execution_kind: Option<&'a str>,
    total_duration: &'a OptionalDuration,
    user_duration: &'a OptionalDuration,
    potential_improvement_duration: &'a OptionalDuration,
    non_critical_path_time: &'a OptionalDuration,
    start_offset: u64,
    reused: bool,
}

impl CriticalPathEntry<'_> {
    fn csv_row(&self) -> CriticalPathCsvRow<'_> {
        CriticalPathCsvRow {
            kind: self.kind,
            name: &self.name,
            category: self.category,
            identifier: self.identifier,
            execution_kind: self.execution_kind,
            total_duration: &self.total_duration,
            user_duration: &self.user_duration,
            potential_improvement_duration: &self.potential_improvement_duration,
            non_critical_path_time: &self.non_critical_path_time,
            start_offset: self.start_offset,
            reused: self.reused,
        }
    }
}

async fn log_critical_path(
    path: &Vec<buck2_data::CriticalPathEntry2>,
    format: LogCommandOutputFormat,
) -> buck2_error::Result<()> {
    let target_display_options = TargetDisplayOptions::for_log();

    buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
        let mut log_writer = transform_format(format, w);
        if let LogCommandOutputFormatWithWriter::Readable(writer) = &mut log_writer {
            #[allow(clippy::write_literal)] // easier to match the format below
            writeln!(
                writer,
                "{:>13} {:>10} {:>10} {:>10} {:>10} {} {} {} {} {}",
                "start_offset",
                "total",
                "waiting",
                "user",
                "potential",
                "kind",
                "name",
                "category",
                "identifier",
                "execution_kind",
            )?;
        }

        for entry in path {
            let entry_display =
                match CriticalPathEntryDisplay::from_entry(entry, target_display_options)? {
                    Some(display) => display,
                    None => continue,
                };

            let reused_suffix = entry_display.reused_suffix();

            let critical_path = CriticalPathEntry {
                kind: entry_display.kind,
                name: entry_display.name,
                category: entry_display.category,
                identifier: entry_display.identifier,
                execution_kind: entry_display.execution_kind,
                total_duration: OptionalDuration::new(entry.total_duration)?,
                user_duration: OptionalDuration::new(entry.user_duration)?,
                potential_improvement_duration: OptionalDuration::new(
                    entry.potential_improvement_duration,
                )?,
                non_critical_path_time: OptionalDuration::new(entry.non_critical_path_duration)?,
                start_offset: entry.start_offset_ns.map_or(0, |v| v / 1000),
                reused: entry_display.reused,
            };

            let res: Result<(), ClientIoError> = {
                match &mut log_writer {
                    LogCommandOutputFormatWithWriter::Readable(writer) => {
                        writeln!(
                            writer,
                            "{:>13} {:>10} {:>10} {:>10} {:>10} {}{} {} {} {} {}",
                            critical_path.start_offset,
                            critical_path.total_duration,
                            critical_path.non_critical_path_time,
                            critical_path.user_duration,
                            critical_path.potential_improvement_duration,
                            critical_path.kind,
                            reused_suffix,
                            critical_path.name,
                            critical_path.category.unwrap_or_default(),
                            critical_path.identifier.unwrap_or_default(),
                            critical_path.execution_kind.unwrap_or_default(),
                        )?;
                    }
                    LogCommandOutputFormatWithWriter::Tabulated(writer) => {
                        // This should match the format specified in the docstrings on CriticalPathCommand and SlowestPathCommand
                        writeln!(
                            writer,
                            "{}{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                            critical_path.kind,
                            reused_suffix,
                            critical_path.name,
                            critical_path.category.unwrap_or_default(),
                            critical_path.identifier.unwrap_or_default(),
                            critical_path.execution_kind.unwrap_or_default(),
                            critical_path.total_duration,
                            critical_path.user_duration,
                            critical_path.potential_improvement_duration,
                            critical_path.non_critical_path_time,
                            critical_path.start_offset
                        )?;
                    }
                    LogCommandOutputFormatWithWriter::Json(writer) => {
                        serde_json::to_writer(writer.by_ref(), &critical_path)?;
                        writer.write_all("\n".as_bytes())?;
                    }
                    LogCommandOutputFormatWithWriter::Csv(writer) => {
                        writer
                            .serialize(critical_path.csv_row())
                            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::LogCmd))?;
                    }
                }
                Ok(())
            };
            res?
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::CriticalPathEntry;
    use super::OptionalDuration;

    fn d(us: u64) -> OptionalDuration {
        OptionalDuration {
            inner: Some(Duration::from_micros(us)),
        }
    }

    /// The `Csv` arm of `log_critical_path`, with the writer from `transform_format`.
    fn to_csv(entries: Vec<CriticalPathEntry<'_>>) -> (String, Result<(), csv::Error>) {
        let mut out = Vec::new();
        let mut res = Ok(());
        {
            let mut w = csv::WriterBuilder::new().from_writer(&mut out);
            for e in entries {
                if let Err(e) = w.serialize(e.csv_row()) {
                    res = Err(e);
                    break;
                }
            }
        }
        (String::from_utf8(out).unwrap(), res)
    }

    /// Every row has every column, whatever optional fields are present.
    #[test]
    fn test_csv_rows_have_every_column() {
        let (out, res) = to_csv(vec![
            CriticalPathEntry {
                kind: "buckd_command_init",
                total_duration: d(10),
                ..Default::default()
            },
            CriticalPathEntry {
                kind: "listing",
                name: "root//".to_owned(),
                total_duration: d(20),
                ..Default::default()
            },
            CriticalPathEntry {
                kind: "action",
                name: "root//:a (<unspecified>)".to_owned(),
                execution_kind: Some("LOCAL"),
                ..Default::default()
            },
            CriticalPathEntry {
                kind: "materialization",
                name: "root//:a (<unspecified>)".to_owned(),
                identifier: Some("buck-out/v2/gen/x"),
                ..Default::default()
            },
        ]);
        res.unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            "kind,name,category,identifier,execution_kind,total_duration,user_duration,potential_improvement_duration,non_critical_path_time,start_offset,reused"
        );
        assert_eq!(lines.len(), 5);
        for line in &lines[1..] {
            assert_eq!(line.split(',').count(), 11, "{line}");
        }
        assert!(
            lines[1].starts_with("buckd_command_init,,,,,10,"),
            "{}",
            lines[1]
        );
        assert!(
            lines[3].starts_with("action,root//:a (<unspecified>),,,LOCAL,"),
            "{}",
            lines[3]
        );
        assert!(
            lines[4].starts_with("materialization,root//:a (<unspecified>),,buck-out/v2/gen/x,,"),
            "{}",
            lines[4]
        );
    }
}
