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
use buck2_event_observer::fmt_duration::fmt_duration_precise;
use serde::Serialize;
use tokio_stream::StreamExt;

use crate::LogCommandOutputFormat;
use crate::LogCommandOutputFormatOptions;
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
/// `<start> <total> <off_path> <user> <potential> <kind>[(reused)] <name> <category> <identifier> <execution_kind>`
///
/// Columns:
/// - start = offset from the start of the build
/// - total = wall-clock time for this node
/// - off_path = time that overlapped other work (did not extend the build)
/// - user = the portion of the runtime you can actually optimize
/// - potential = build time you'd save if this node were instant
///
/// Consecutive identical (or 'waiting') entries are collapsed into a single `... (×N)` row.
///
/// The `tabulated` format produces tab-delimited output:
/// `<kind>[(reused)]\t<name>\t<category>\t<identifier>\t<execution_kind>\t<total_duration>\t<user_duration>\t<potential_improvement_duration>\t<non_critical_path_time>\t<start_offset>`
///
/// In the `tabulated`, `json`, and `csv` formats all durations are in microseconds, including the
/// start offset (measured from the beginning of the build).
#[derive(Debug, clap::Parser)]
pub struct CriticalPathCommand {
    #[clap(flatten)]
    common: CommonPathArgs,
}

impl BuckSubcommand for CriticalPathCommand {
    const COMMAND_NAME: &'static str = "log-critical-path";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        log_critical_path_command_exec(ctx, self.common, PathKind::Critical).await
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
/// `<start> <total> <off_path> <user> <potential> <kind>[(reused)] <name> <category> <identifier> <execution_kind>`
///
/// Columns:
/// - start = offset from the start of the build
/// - total = wall-clock time for this node
/// - off_path = time that overlapped other work (did not extend the build)
/// - user = the portion of the runtime you can actually optimize
/// - potential = build time you'd save if this node were instant
///
/// Consecutive identical (or 'waiting') entries are collapsed into a single `... (×N)` row.
///
/// The `tabulated` format produces tab-delimited output:
/// `<kind>[(reused)]\t<name>\t<category>\t<identifier>\t<execution_kind>\t<total_duration>\t<user_duration>\t<potential_improvement_duration>\t<non_critical_path_time>\t<start_offset>`
///
/// In the `tabulated`, `json`, and `csv` formats all durations are in microseconds, including the
/// start offset (measured from the beginning of the build).
///
/// Note that this prints a "potential improvement" just like `log critical-path`, but for slowest paths it's not computed.
#[derive(Debug, clap::Parser)]
pub struct SlowestPathCommand {
    #[clap(flatten)]
    common: CommonPathArgs,
}

impl BuckSubcommand for SlowestPathCommand {
    const COMMAND_NAME: &'static str = "log-slowest-path";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        log_critical_path_command_exec(ctx, self.common, PathKind::Slowest).await
    }
}

/// Options shared by the `critical-path` and `slowest-path` subcommands.
#[derive(Debug, clap::Parser)]
struct CommonPathArgs {
    #[clap(flatten)]
    event_log: EventLogOptions,
    #[clap(flatten)]
    format: LogCommandOutputFormat,
    /// In the `readable` format (the default), hide entries whose impact -- the largest of their
    /// total, off-path, user, and potential-improvement durations -- is below this. Repetitive and
    /// structural nodes tend to be near-zero, so a small value like `1ms` focuses the output on
    /// entries that matter. Accepts units (e.g. `500us`, `1ms`, `1s`). By default nothing is
    /// hidden.
    #[clap(long)]
    threshold: Option<humantime::Duration>,
}

/// Resolves `--threshold` against `--format`, rejecting a threshold for the
/// machine-readable formats, which never filter.
fn parse_threshold(
    format: &LogCommandOutputFormat,
    threshold: Option<humantime::Duration>,
) -> buck2_error::Result<Duration> {
    match (&format.format, threshold) {
        (_, None) => Ok(Duration::ZERO),
        (LogCommandOutputFormatOptions::Readable, Some(threshold)) => Ok(threshold.into()),
        (
            LogCommandOutputFormatOptions::Tabulated
            | LogCommandOutputFormatOptions::Json
            | LogCommandOutputFormatOptions::Csv,
            Some(_),
        ) => Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "`--threshold` is only supported with `--format readable`; the machine-readable formats always include every entry"
        )),
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
    common: CommonPathArgs,
    path_kind: PathKind,
) -> ExitResult {
    let CommonPathArgs {
        event_log,
        format,
        threshold,
    } = common;
    let threshold = parse_threshold(&format, threshold)?;
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
                    log_critical_path(&build_graph.critical_path2, format.clone(), threshold)
                        .await?;
                }
                PathKind::Slowest => {
                    log_critical_path(&build_graph.slowest_path, format.clone(), threshold).await?;
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

impl CriticalPathEntry<'_> {
    /// The largest duration signal this entry carries: how much time it either cost the
    /// build (`total`), spent off the path (`non_critical`), had that the user
    /// could optimize (`user`), or could save (`potential`). Used to decide
    /// whether the entry is worth showing.
    fn impact(&self) -> Duration {
        [
            self.total_duration.inner,
            self.non_critical_path_time.inner,
            self.user_duration.inner,
            self.potential_improvement_duration.inner,
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(Duration::ZERO)
    }

    /// Whether two entries are identical apart from their timing, and so can be
    /// collapsed into a single `readable` row.
    fn same_identity(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.name == other.name
            && self.category == other.category
            && self.identifier == other.identifier
            && self.execution_kind == other.execution_kind
            && self.reused == other.reused
    }
}

/// The `kind` shared by all waiting entries (see `CriticalPathEntryDisplay`).
const WAITING_KIND: &str = "waiting";

/// Pick the color for a row based on how much time it could save.
fn row_style(potential: Duration) -> console::Style {
    let style = console::Style::new();
    if potential < Duration::from_millis(100) {
        style.black().bright() // gray
    } else if potential < Duration::from_secs(1) {
        style // normal
    } else if potential < Duration::from_secs(5) {
        style.yellow()
    } else if potential < Duration::from_secs(10) {
        style.red()
    } else {
        style.red().bright().bold()
    }
}

/// Collapses consecutive equivalent entries in the `readable` output into a
/// single `... (×N)` row with summed durations, so repetitive bursts don't drown
/// out the entries that matter.
///
/// Two kinds of run are collapsed:
///
/// - Identical entries (same kind/name/category/...), e.g. a burst of the same
///   action; distinct named nodes stay separate so you can see *which* is slow.
///
/// - Any consecutive `waiting` entries, regardless of category: these are all
///   unattributed gaps, so `waiting unknown` interleaved with `waiting for_deps`
///   collapses to a single `waiting unknown / for_deps (×N)` row.
#[derive(Default)]
struct Run<'a> {
    current: Option<RunState<'a>>,
    /// Hidden entries seen since the last visible one.
    pending_hidden: u64,
}

struct RunState<'a> {
    /// First entry of the run, used for the columns (category, identifier, ...)
    /// that are shared across the whole run.
    first: CriticalPathEntry<'a>,
    /// Distinct names seen in the run, in first-seen order. Usually one entry;
    /// for a collapsed waiting run it holds each category (e.g. `unknown`).
    names: Vec<String>,
    count: u64,
    /// Hidden entries that sat between members of this run.
    hidden: u64,
    /// Summed durations. Each stays `None` until a present value is added, so an
    /// always-absent column (e.g. `potential` on the slowest path) still renders
    /// blank rather than a misleading `0`.
    total: Option<Duration>,
    off_path: Option<Duration>,
    user: Option<Duration>,
    potential: Option<Duration>,
}

/// Add an optional duration into a running total, leaving it `None` until the
/// first present value.
fn add_opt(acc: &mut Option<Duration>, v: Option<Duration>) {
    if let Some(v) = v {
        *acc = Some(acc.unwrap_or_default() + v);
    }
}

/// Render a summed column: blank when absent, human-readable otherwise.
fn fmt_opt(d: Option<Duration>) -> String {
    match d {
        Some(d) => fmt_duration_precise(d),
        None => String::new(),
    }
}

/// Whether `entry` can extend a run whose first entry is `first`.
fn can_extend(first: &CriticalPathEntry, entry: &CriticalPathEntry) -> bool {
    if first.kind == WAITING_KIND && entry.kind == WAITING_KIND && first.reused == entry.reused {
        true
    } else {
        first.same_identity(entry)
    }
}

impl<'a> Run<'a> {
    /// Record an entry hidden by `--threshold`.
    fn hide(&mut self) {
        self.pending_hidden += 1;
    }

    /// Add an entry, either extending the current run or flushing it and starting
    /// a new one.
    fn push(
        &mut self,
        writer: &mut dyn std::io::Write,
        entry: CriticalPathEntry<'a>,
    ) -> std::io::Result<()> {
        let pending_hidden = std::mem::take(&mut self.pending_hidden);
        match &mut self.current {
            Some(state) if can_extend(&state.first, &entry) => {
                state.count += 1;
                state.hidden += pending_hidden;
                add_opt(&mut state.total, entry.total_duration.inner);
                add_opt(&mut state.off_path, entry.non_critical_path_time.inner);
                add_opt(&mut state.user, entry.user_duration.inner);
                add_opt(
                    &mut state.potential,
                    entry.potential_improvement_duration.inner,
                );
                if !state.names.iter().any(|n| n == &entry.name) {
                    state.names.push(entry.name);
                }
            }
            _ => {
                self.flush(writer)?;
                self.current = Some(RunState {
                    names: vec![entry.name.clone()],
                    total: entry.total_duration.inner,
                    off_path: entry.non_critical_path_time.inner,
                    user: entry.user_duration.inner,
                    potential: entry.potential_improvement_duration.inner,
                    count: 1,
                    hidden: 0,
                    first: entry,
                });
            }
        }
        Ok(())
    }

    /// Write out the pending run (if any) and clear it.
    fn flush(&mut self, writer: &mut dyn std::io::Write) -> std::io::Result<()> {
        let Some(state) = self.current.take() else {
            return Ok(());
        };
        let e = &state.first;
        let count = match (state.count, state.hidden) {
            (1, _) => String::new(),
            (n, 0) => format!(" (×{n})"),
            (n, hidden) => format!(" (×{n}, {hidden} hidden)"),
        };
        let line = format!(
            "{:>10} {:>10} {:>10} {:>10} {:>10} {}{} {} {} {} {}{}",
            fmt_duration_precise(Duration::from_micros(e.start_offset)),
            fmt_opt(state.total),
            fmt_opt(state.off_path),
            fmt_opt(state.user),
            fmt_opt(state.potential),
            e.kind,
            if e.reused { "(reused)" } else { "" },
            state.names.join(" / "),
            e.category.unwrap_or_default(),
            e.identifier.unwrap_or_default(),
            e.execution_kind.unwrap_or_default(),
            count,
        );

        // Color by potential, falling back to total where potential wasn't
        // computed (the slowest path), so those rows still convey magnitude.
        let color_key = state.potential.or(state.total).unwrap_or_default();
        writeln!(writer, "{}", row_style(color_key).apply_to(line))
    }
}

async fn log_critical_path(
    path: &Vec<buck2_data::CriticalPathEntry2>,
    format: LogCommandOutputFormat,
    threshold: Duration,
) -> buck2_error::Result<()> {
    let target_display_options = TargetDisplayOptions::for_log();

    // Aggregate stats for the `readable` footer, so hiding entries stays honest.
    // `hidden_total` and `hidden_user` are the hidden entries' summed `total`
    // (wall-clock) and `user` (optimizable) times.
    let mut hidden_count: u64 = 0;
    let mut hidden_total = Duration::ZERO;
    let mut hidden_user = Duration::ZERO;
    // Collapses consecutive equivalent entries in the `readable` output.
    let mut run = Run::default();

    buck2_client_ctx::stdio::print_with_writer::<buck2_error::Error, _>(async move |w| {
        let mut log_writer = transform_format(format, w);
        if let LogCommandOutputFormatWithWriter::Readable(writer) = &mut log_writer {
            writeln!(
                writer,
                "  start     = offset from the start of the build\n  \
                 total     = wall-clock time for this node\n  \
                 off_path  = time that overlapped other work (did not extend the build)\n  \
                 user      = the portion of the runtime you can actually optimize\n  \
                 potential = build time you'd save if this node were instant\n"
            )?;
            #[allow(clippy::write_literal)] // easier to match the format below
            writeln!(
                writer,
                "{:>10} {:>10} {:>10} {:>10} {:>10} {} {} {} {} {}",
                "start",
                "total",
                "off_path",
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
                        // Hide entries below the threshold -- typically `waiting` nodes, which are
                        // generally just noise. We track these entries so we can tell the user how
                        // many we dropped.
                        if critical_path.impact() < threshold {
                            hidden_count += 1;
                            hidden_total += critical_path.total_duration.inner.unwrap_or_default();
                            hidden_user += critical_path.user_duration.inner.unwrap_or_default();
                            run.hide();
                            continue;
                        }

                        run.push(writer, critical_path)?;
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

        if let LogCommandOutputFormatWithWriter::Readable(writer) = &mut log_writer {
            run.flush(writer)?;
            if hidden_count > 0 {
                writeln!(
                    writer,
                    "\n(hid {} low-impact entries under {}, totaling {} wall-clock, {} user)",
                    hidden_count,
                    fmt_duration_precise(threshold),
                    fmt_duration_precise(hidden_total),
                    fmt_duration_precise(hidden_user),
                )?;
            }
        }

        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

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

    fn threshold_for(args: &[&str]) -> buck2_error::Result<Duration> {
        let args = CommonPathArgs::try_parse_from(
            std::iter::once("critical-path").chain(args.iter().copied()),
        )
        .expect("arguments should parse");
        parse_threshold(&args.format, args.threshold)
    }

    fn entry<'a>(kind: &'a str, name: &str, total: Duration) -> CriticalPathEntry<'a> {
        CriticalPathEntry {
            kind,
            name: name.to_owned(),
            total_duration: OptionalDuration { inner: Some(total) },
            ..Default::default()
        }
    }

    /// Feed `Run` a sequence of entries (`None` = hidden by `--threshold`) and
    /// return each row it prints, minus the `start` column. The other duration
    /// columns are blank here, so only `total` survives whitespace-splitting.
    fn collapse(entries: Vec<Option<CriticalPathEntry<'_>>>) -> Vec<String> {
        let mut out = Vec::new();
        let mut run = Run::default();
        for entry in entries {
            match entry {
                Some(entry) => run.push(&mut out, entry).unwrap(),
                None => run.hide(),
            }
        }
        run.flush(&mut out).unwrap();
        console::strip_ansi_codes(std::str::from_utf8(&out).unwrap())
            .lines()
            .map(|line| {
                line.split_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn test_collapsed_rows_report_hidden_entries_between_them() {
        let wait = || Some(entry(WAITING_KIND, "for_deps", Duration::from_secs(3)));
        let action = |name| Some(entry("action", name, Duration::from_secs(2)));

        assert_eq!(
            collapse(vec![
                // A wait, an action too small to show, and another wait: one
                // row, but it must admit something else ran in between.
                wait(),
                None,
                wait(),
                // Hidden entries *between* two different rows belong to
                // neither of them.
                None,
                action("//foo:bar"),
                action("//foo:bar"),
                None,
                None,
                action("//foo:baz"),
            ]),
            vec![
                "6.00s waiting for_deps (×2, 1 hidden)",
                "4.00s action //foo:bar (×2)",
                "2.00s action //foo:baz",
            ],
        );
    }

    #[test]
    fn test_threshold_filters_readable_output() {
        assert_eq!(Duration::ZERO, threshold_for(&[]).unwrap());
        assert_eq!(
            Duration::from_millis(1),
            threshold_for(&["--threshold", "1ms"]).unwrap()
        );
    }

    #[test]
    fn test_machine_readable_formats_reject_threshold() {
        for format in ["tabulated", "json", "csv"] {
            // Without `--threshold`, every format is fine...
            assert_eq!(
                Duration::ZERO,
                threshold_for(&["--format", format]).unwrap()
            );
            // ...but asking a machine-readable format to filter is an error, even
            // for `0s`, rather than a silently ignored flag.
            for threshold in ["0s", "1ms"] {
                assert!(
                    threshold_for(&["--format", format, "--threshold", threshold]).is_err(),
                    "`--format {format} --threshold {threshold}` should be rejected",
                );
            }
        }
    }
}
