/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Bounded command-level observations, independent of the action DAG.
//!
//! Span unions describe observed coverage, never CPU, exclusive work or causal
//! dependencies. A bounded union that discards a disjoint interval is explicitly
//! a lower bound. The optional unabridged `--phases` view remains separate.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use buck2_event_log::stream_value::StreamValue;

use super::compact::InstantKind;
use super::compact::instant_kind;
use crate::command_profile::BuckCriticalPath;
use crate::command_profile::CommandProfile;
use crate::command_profile::CriticalPathEntry;
use crate::command_profile::ObservedInterval;
use crate::command_profile::PhaseSummary;
use crate::command_profile::SnapshotSample;

const MAX_OPEN_PHASES: usize = 131_072;
const MAX_OPEN_LIFECYCLE: usize = 4_096;
const MAX_LIFECYCLE: usize = 4_096;
const MAX_UNION: usize = 4_096;
const MAX_TOP: usize = 128;
const MAX_CP: usize = 4_096;
const MAX_CP_SPANS: usize = 64;
const MAX_LABEL: usize = 512;
const MAX_SNAPSHOTS: usize = 16_384;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Kind {
    name: &'static str,
    phase: bool,
}

struct Open {
    kind: Kind,
    start: i64,
    label: Option<String>,
}

struct Phase {
    summary: PhaseSummary,
    union: BTreeMap<i64, i64>,
    /// Same segments indexed by length, not a second set of observations.
    /// At capacity, prefer greater observed coverage over early tiny fragments.
    union_by_length: BTreeSet<(i64, i64)>,
    top: BTreeMap<(i64, u64), ObservedInterval>,
    intervals: u64,
    total_overflowed: bool,
}

impl Phase {
    fn new(kind: &str) -> Self {
        Self {
            summary: PhaseSummary {
                kind: kind.to_owned(),
                union_complete: true,
                ..Default::default()
            },
            union: BTreeMap::new(),
            union_by_length: BTreeSet::new(),
            top: BTreeMap::new(),
            intervals: 0,
            total_overflowed: false,
        }
    }

    fn interval(&mut self, interval: ObservedInterval) {
        self.intervals = self.intervals.saturating_add(1);
        match self
            .summary
            .total_duration_us
            .checked_add(interval.duration_us)
        {
            Some(total) => self.summary.total_duration_us = total,
            None => {
                self.total_overflowed = true;
                self.summary.total_duration_us = i64::MAX;
                // The individual interval is valid; only the sum overflowed.
            }
        }
        let start = interval.start_offset_us;
        let end = start + interval.duration_us; // checked before constructing it
        if interval.duration_us != 0 {
            let mut lo = start;
            let mut hi = end;
            if let Some((&s, &e)) = self.union.range(..=start).next_back() {
                if e >= start {
                    lo = s;
                    hi = hi.max(e);
                    self.remove_union(s);
                }
            }
            while let Some((&s, &e)) = self.union.range(lo..).next() {
                if s > hi {
                    break;
                }
                hi = hi.max(e);
                self.remove_union(s);
            }
            if self.union.len() == MAX_UNION {
                self.summary.union_complete = false;
                let &(shortest, first) = self.union_by_length.first().unwrap();
                if hi - lo > shortest {
                    self.remove_union(first);
                }
            }
            if self.union.len() < MAX_UNION {
                self.union.insert(lo, hi);
                self.union_by_length.insert((hi - lo, lo));
            }
        }
        let key = (interval.duration_us, self.intervals);
        if self.top.len() < MAX_TOP {
            self.top.insert(key, interval);
        } else if self
            .top
            .first_key_value()
            .is_some_and(|(min, _)| key.0 > min.0)
        {
            self.top.pop_first();
            self.top.insert(key, interval);
        }
    }

    fn remove_union(&mut self, start: i64) {
        let end = self.union.remove(&start).unwrap();
        let removed = self.union_by_length.remove(&(end - start, start));
        debug_assert!(removed);
    }

    fn finish(mut self) -> PhaseSummary {
        self.summary.union_intervals = self.union.into_iter().map(Into::into).collect();
        self.summary.dropped_examples = self.intervals.saturating_sub(self.top.len() as u64);
        self.summary.top = self.top.into_iter().rev().map(|(_, v)| v).collect();
        self.summary
    }
}

pub(super) struct Collector {
    profile: CommandProfile,
    origin: Option<i64>,
    open: HashMap<u64, Open>,
    /// Independent reservations: bulk phases cannot starve command lifecycle.
    /// Index0=lifecycle, index1=phase; duplicate replacement releases its slot.
    open_counts: [usize; 2],
    phases: BTreeMap<&'static str, Phase>,
}

impl Collector {
    pub(super) fn new(origin: Option<i64>, run: String) -> Self {
        Self {
            profile: CommandProfile {
                schema_version: 1,
                origin_us: origin.unwrap_or(0),
                run: Some(run),
                ..Default::default()
            },
            origin,
            open: HashMap::new(),
            open_counts: [0; 2],
            phases: BTreeMap::new(),
        }
    }

    fn warn(&mut self, message: &str) {
        if self.profile.warnings.len() < 32 && !self.profile.warnings.iter().any(|s| s == message) {
            self.profile.warnings.push(message.to_owned());
        }
    }

    fn note(&mut self, message: &str) {
        if self.profile.notes.len() < 32 && !self.profile.notes.iter().any(|s| s == message) {
            self.profile.notes.push(message.to_owned());
        }
    }

    fn origin(&mut self, timestamp: i64) -> i64 {
        if self.origin.is_none() {
            self.origin = Some(timestamp);
            self.profile.origin_us = timestamp;
            self.note("Invocation start is absent; profile offsets use the first observed non-action event, not client startup.");
        }
        self.origin.unwrap()
    }

    pub(super) fn action_start(&mut self, sid: u64) {
        self.replace_start(sid);
    }

    fn reject(&mut self) {
        self.profile.rejected_records = self.profile.rejected_records.saturating_add(1);
    }

    fn phase(&mut self, kind: Kind) -> &mut Phase {
        self.phases
            .entry(kind.name)
            .or_insert_with(|| Phase::new(kind.name))
    }

    fn replace_start(&mut self, sid: u64) {
        if let Some(previous) = self.remove_open(sid) {
            self.reject();
            if previous.kind.phase {
                let phase = self.phase(previous.kind);
                phase.summary.rejected_intervals =
                    phase.summary.rejected_intervals.saturating_add(1);
                phase.summary.union_complete = false;
            }
        }
    }

    fn remove_open(&mut self, sid: u64) -> Option<Open> {
        let open = self.open.remove(&sid)?;
        self.open_counts[usize::from(open.kind.phase)] -= 1;
        Some(open)
    }

    fn refuse_timestamp(&mut self, sid: u64) {
        self.reject();
        if let Some(open) = self.remove_open(sid) {
            if open.kind.phase {
                let phase = self.phase(open.kind);
                phase.summary.rejected_intervals =
                    phase.summary.rejected_intervals.saturating_add(1);
                phase.summary.union_complete = false;
            }
        }
        self.warn("A missing or invalid event timestamp was rejected; any open profile interval with that span ID was invalidated, not closed by a later end.");
    }

    pub(super) fn result(&mut self, value: &StreamValue) {
        use buck2_cli_proto::command_result::Result;
        let StreamValue::Result(result) = value else {
            return;
        };
        if let Some(Result::TargetsResponse(targets)) = &result.result
            && targets.error_count != 0
        {
            self.warn(&format!("Targets completed with {} package error(s); --keep-going permits a successful command response with partial results.", targets.error_count));
        }
        if let Some(Result::TestResponse(test)) = &result.result
            && test.executor_exit_code == 0
            && test.build_errors.is_empty()
            && !test.test_errors.is_empty()
        {
            self.warn("Test-run diagnostics were recorded alongside a successful executor exit; they do not establish command failure.");
        }
        let tests_failed = matches!(&result.result, Some(Result::TestResponse(test))
            if test.executor_exit_code != 0 && test.build_errors.is_empty());
        let (outcome, error) = match &result.result {
            Some(Result::Error(error)) => (Some(false), Some(error)),
            Some(Result::BuildResponse(build)) => {
                (Some(build.errors.is_empty()), build.errors.first())
            }
            Some(Result::BxlResponse(bxl)) => (Some(bxl.errors.is_empty()), bxl.errors.first()),
            Some(Result::TestResponse(test)) => (
                Some(test.executor_exit_code == 0 && test.build_errors.is_empty()),
                test.build_errors.first().or_else(|| {
                    (test.executor_exit_code != 0)
                        .then(|| test.test_errors.first())
                        .flatten()
                }),
            ),
            Some(
                Result::InstallResponse(_)
                | Result::AqueryResponse(_)
                | Result::CqueryResponse(_)
                | Result::UqueryResponse(_)
                | Result::TargetsResponse(_)
                | Result::ConfiguredTargetsResponse(_)
                | Result::TargetsShowOutputsResponse(_)
                | Result::GenericResponse(_),
            ) => (Some(true), None),
            _ => (None, None),
        };
        if let Some(outcome) = outcome {
            self.profile.result_outcome = Some(
                if tests_failed {
                    "tests-failed"
                } else if outcome {
                    "success"
                } else {
                    "failed"
                }
                .to_owned(),
            );
        }
        if let Some(error) = error {
            self.profile.result_error = Some(bounded_to(&error.message, 16_384));
        }
    }

    pub(super) fn event(&mut self, event: &buck2_data::BuckEvent) {
        use buck2_data::buck_event::Data;
        // Action records are normalized before this observer in production.
        // Keep the raw test oracle's clock-origin behavior identical.
        if matches!(&event.data,
            Some(Data::SpanStart(s)) if matches!(s.data, Some(buck2_data::span_start_event::Data::ActionExecution(_))))
        {
            self.replace_start(event.span_id);
            return;
        }
        if matches!(&event.data,
            Some(Data::SpanEnd(e)) if matches!(e.data, Some(buck2_data::span_end_event::Data::ActionExecution(_))))
        {
            return;
        }
        let consumed = self.open.contains_key(&event.span_id)
            || match &event.data {
                Some(Data::SpanStart(s)) => s.data.as_ref().is_some_and(start_kind_kept),
                Some(Data::SpanEnd(e)) => e.data.as_ref().is_some_and(end_kind_kept),
                Some(Data::Instant(i)) => match instant_kind(i.data.as_ref()) {
                    InstantKind::BuildGraphInfo
                    | InstantKind::InstallFinished
                    | InstantKind::Snapshot => true,
                    InstantKind::ReSession
                    | InstantKind::SystemInfo
                    | InstantKind::TransitiveSetProjectionDef
                    | InstantKind::ActionGraphStructureDef
                    | InstantKind::ActionGraphAlias
                    | InstantKind::ActionGraphStart
                    | InstantKind::ActionGraphFooter
                    | InstantKind::ConcurrentCommands
                    | InstantKind::Ignored => false,
                },
                _ => false,
            };
        if !consumed {
            return;
        }
        let Some(ts) = event.timestamp.as_ref().and_then(timestamp_us) else {
            self.refuse_timestamp(event.span_id);
            return;
        };
        let origin = self.origin(ts);
        let Some(offset) = ts.checked_sub(origin) else {
            self.refuse_timestamp(event.span_id);
            return;
        };
        if offset < 0 {
            self.warn("Some daemon observations precede the invocation timestamp; clock alignment is uncertain and negative offsets are retained.");
        }
        match &event.data {
            Some(Data::SpanStart(start)) => {
                self.replace_start(event.span_id);
                let Some(kind) = start.data.as_ref().and_then(start_kind) else {
                    return;
                };
                if kind.name == "Command" {
                    if self.profile.command_start_offset_us.is_none() {
                        self.profile.command_start_offset_us = Some(offset);
                        if let Some(buck2_data::span_start_event::Data::Command(command)) =
                            &start.data
                        {
                            self.profile.command_kind = command_kind(command).map(str::to_owned);
                        }
                    } else {
                        self.warn("Multiple command starts were observed; only the first command-start marker is retained.");
                    }
                }
                if kind.phase {
                    let phase = self.phase(kind);
                    phase.summary.count = phase.summary.count.saturating_add(1);
                }
                let group = usize::from(kind.phase);
                let limit = if kind.phase {
                    MAX_OPEN_PHASES
                } else {
                    MAX_OPEN_LIFECYCLE
                };
                if self.open_counts[group] >= limit {
                    self.reject();
                    if kind.phase {
                        let phase = self.phase(kind);
                        phase.summary.rejected_intervals =
                            phase.summary.rejected_intervals.saturating_add(1);
                        phase.summary.union_complete = false;
                    }
                    self.warn(if kind.phase {
                        "The bounded phase open-span limit was reached; affected phase intervals are unavailable. Lifecycle capacity is reserved separately."
                    } else {
                        "The bounded lifecycle open-span limit was reached; affected lifecycle intervals are unavailable. Phase capacity is reserved separately."
                    });
                    return;
                }
                let label = start
                    .data
                    .as_ref()
                    .and_then(start_label)
                    .map(|s| bounded(&s));
                self.open.insert(
                    event.span_id,
                    Open {
                        kind,
                        start: offset,
                        label,
                    },
                );
                self.open_counts[group] += 1;
            }
            Some(Data::SpanEnd(end)) => {
                let cancelled = matches!(
                    &end.data,
                    Some(buck2_data::span_end_event::Data::SpanCancelled(_))
                );
                let kind = end.data.as_ref().and_then(end_kind);
                let closes_command = kind.is_some_and(|k| k.name == "Command")
                    || (cancelled
                        && self
                            .open
                            .get(&event.span_id)
                            .is_some_and(|open| open.kind.name == "Command"));
                if closes_command {
                    if self
                        .profile
                        .command_start_offset_us
                        .is_none_or(|start| offset >= start)
                    {
                        self.profile.command_end_offset_us = Some(offset);
                        self.profile.command_cancelled = cancelled;
                    } else {
                        self.reject();
                        self.warn("A command-end timestamp precedes its start; that invalid endpoint was omitted.");
                    }
                }
                let Some(open) = self.remove_open(event.span_id) else {
                    if let Some(kind) = kind.filter(|k| k.phase) {
                        // End records without retained starts include starts
                        // rejected at the bounded open-span cap. These are
                        // separate record diagnostics, not additive counts of
                        // independently lost intervals.
                        let phase = self.phase(kind);
                        phase.summary.unmatched_ends =
                            phase.summary.unmatched_ends.saturating_add(1);
                        phase.summary.union_complete = false;
                    }
                    return;
                };
                if !cancelled && kind != Some(open.kind) {
                    self.reject();
                    if open.kind.phase {
                        let phase = self.phase(open.kind);
                        phase.summary.rejected_intervals =
                            phase.summary.rejected_intervals.saturating_add(1);
                        phase.summary.union_complete = false;
                    }
                    return;
                }
                if open.kind.phase {
                    let phase = self.phase(open.kind);
                    if cancelled {
                        phase.summary.cancelled = phase.summary.cancelled.saturating_add(1);
                    } else {
                        phase.summary.completed = phase.summary.completed.saturating_add(1);
                    }
                }
                let duration = end.duration.as_ref().and_then(duration_us);
                let Some(duration) = duration.filter(|d| {
                    open.start
                        .checked_add(*d)
                        .and_then(|end| origin.checked_add(end))
                        .is_some()
                }) else {
                    self.reject();
                    if open.kind.phase {
                        let phase = self.phase(open.kind);
                        phase.summary.rejected_intervals =
                            phase.summary.rejected_intervals.saturating_add(1);
                        phase.summary.union_complete = false;
                    }
                    return;
                };
                let interval = ObservedInterval {
                    kind: open.kind.name.to_owned(),
                    label: open.label,
                    start_offset_us: open.start,
                    duration_us: duration,
                    cancelled,
                };
                if open.kind.phase {
                    self.phase(open.kind).interval(interval);
                } else if self.profile.lifecycle.len() < MAX_LIFECYCLE {
                    self.profile.lifecycle.push(interval);
                } else {
                    self.profile.dropped_lifecycle =
                        self.profile.dropped_lifecycle.saturating_add(1);
                }
            }
            Some(Data::Instant(instant)) => match instant_kind(instant.data.as_ref()) {
                InstantKind::BuildGraphInfo => {
                    let Some(buck2_data::instant_event::Data::BuildGraphInfo(info)) = &instant.data
                    else {
                        unreachable!("instant classifier preserves the payload kind");
                    };
                    self.critical_path(info)
                }
                InstantKind::InstallFinished => {
                    let Some(buck2_data::instant_event::Data::InstallFinished(install)) =
                        &instant.data
                    else {
                        unreachable!("instant classifier preserves the payload kind");
                    };
                    self.profile.install_duration_us = self.duration(install.duration.as_ref());
                    self.note("Install duration is the producer-reported tail after installer/artifact readiness, not total installation work; its timestamp is not an interval endpoint.");
                }
                InstantKind::Snapshot => {
                    let Some(buck2_data::instant_event::Data::Snapshot(snapshot)) = &instant.data
                    else {
                        unreachable!("instant classifier preserves the payload kind");
                    };
                    let sample = SnapshotSample {
                        offset_us: offset,
                        cpu_us: snapshot
                            .buck2_user_cpu_us
                            .checked_add(snapshot.buck2_system_cpu_us),
                        download_bytes: Some(snapshot.re_download_bytes),
                        upload_bytes: Some(snapshot.re_upload_bytes),
                        rss_bytes: snapshot.buck2_rss,
                        heap_bytes: snapshot.malloc_bytes_active,
                        // Non-optional producer scalar: zero may also be an
                        // older producer's default, not a verified empty queue.
                        deferred_materializer_queue_size: Some(
                            snapshot.deferred_materializer_queue_size,
                        ),
                    };
                    if self.profile.snapshots.len() < MAX_SNAPSHOTS {
                        self.profile.snapshots.push(sample);
                    } else {
                        self.profile.snapshots[MAX_SNAPSHOTS - 1] = sample;
                        self.profile.dropped_snapshots =
                            self.profile.dropped_snapshots.saturating_add(1);
                    }
                }
                InstantKind::ReSession
                | InstantKind::SystemInfo
                | InstantKind::TransitiveSetProjectionDef
                | InstantKind::ActionGraphStructureDef
                | InstantKind::ActionGraphAlias
                | InstantKind::ActionGraphStart
                | InstantKind::ActionGraphFooter
                | InstantKind::ConcurrentCommands
                | InstantKind::Ignored => {}
            },
            _ => {}
        }
    }

    fn critical_path(&mut self, info: &buck2_data::BuildGraphExecutionInfo) {
        if self.profile.critical_path.is_some() {
            self.warn("Multiple BuildGraphInfo records were observed; the last producer-reported path is retained.");
        }
        let mut path = BuckCriticalPath {
            // Buck's origin is a server-context Instant, created before its
            // Command span. The wire does not give that Instant's wall time.
            command_start_offset_us: self.profile.command_start_offset_us,
            approximate_alignment: self.profile.command_start_offset_us.is_some(),
            backend: info.backend_name.as_deref().map(bounded),
            dropped_entries: info.critical_path2.len().saturating_sub(MAX_CP) as u64,
            ..Default::default()
        };
        self.note("Producer critical-path offsets use Buck's own command clock. Alignment to the recorded daemon Command start is approximate; no error bound is recorded.");
        for entry in info.critical_path2.iter().take(MAX_CP) {
            let (kind, label) = cp_label(entry);
            if entry.span_ids.len() > MAX_CP_SPANS {
                self.warn("Some producer critical-path entries have more than 64 span IDs; additional IDs were omitted.");
                self.reject();
            }
            let start_offset_us = entry
                .start_offset_ns
                .and_then(|n| i64::try_from(n / 1_000).ok());
            let mut total_us = self.duration(entry.total_duration.as_ref());
            if entry.was_reused != Some(true)
                && start_offset_us
                    .zip(total_us)
                    .is_some_and(|(start, total)| start.checked_add(total).is_none())
            {
                total_us = None;
                self.reject();
                self.warn("A producer critical-path total duration overflowed its positioned end; that total was omitted while other entry evidence was retained.");
            }
            path.entries.push(CriticalPathEntry {
                kind: kind.to_owned(),
                label: bounded(&label),
                span_ids: entry.span_ids.iter().copied().take(MAX_CP_SPANS).collect(),
                acts: Vec::new(),
                start_offset_us,
                duration_us: self.duration(entry.duration.as_ref()),
                total_us,
                user_us: self.duration(entry.user_duration.as_ref()),
                queue_us: self.duration(entry.queue_duration.as_ref()),
                noncritical_us: self.duration(entry.non_critical_path_duration.as_ref()),
                potential_improvement_us: self
                    .duration(entry.potential_improvement_duration.as_ref()),
                reused: entry.was_reused,
            });
        }
        self.profile.critical_path = Some(path);
    }

    fn duration(&mut self, value: Option<&prost_types::Duration>) -> Option<i64> {
        let value = value?;
        let result = duration_us(value);
        if result.is_none() {
            self.reject();
            self.warn(
                "Invalid or overflowing producer durations were omitted, not converted to zero.",
            );
        }
        result
    }

    pub(super) fn finish(mut self, incomplete: bool) -> anyhow::Result<CommandProfile> {
        for (_, open) in std::mem::take(&mut self.open) {
            if open.kind.phase {
                let phase = self.phase(open.kind);
                phase.summary.open_at_eof = phase.summary.open_at_eof.saturating_add(1);
                phase.summary.union_complete = false;
            } else {
                self.profile.lifecycle_open_at_eof =
                    self.profile.lifecycle_open_at_eof.saturating_add(1);
            }
        }
        if incomplete {
            self.warn(
                "The command log is incomplete; observations describe only its readable prefix.",
            );
            for phase in self.phases.values_mut() {
                phase.summary.union_complete = false;
            }
        }
        if !self.profile.snapshots.is_empty() {
            self.note("Snapshot CPU, transfer counters and the materializer queue are daemon-wide context, not per-phase or per-action attribution. Scalar zero may mean zero or unavailable in the producer.");
        }
        if self.profile.dropped_snapshots != 0 {
            self.warn("Snapshot limit reached: the first 16,383 samples and latest sample are retained; omitted samples cannot establish monotonicity or continuous window coverage.");
        }
        if self.profile.dropped_lifecycle != 0 {
            self.warn("Lifecycle observations exceeded the bounded limit; the retained intervals are a prefix, not full command coverage.");
        }
        if self
            .profile
            .critical_path
            .as_ref()
            .is_some_and(|p| p.dropped_entries != 0)
        {
            self.warn("The producer critical path exceeded the bounded entry limit; only a prefix is retained.");
        }
        if self.phases.values().any(|p| !p.summary.union_complete) {
            self.warn("One or more phase unions are incomplete; their retained covered time is only a lower bound.");
        }
        if self.phases.values().any(|p| p.total_overflowed) {
            self.warn("A phase duration sum overflowed: its total_duration_us is saturated at i64::MAX, a diagnostic sentinel rather than an exact summed duration.");
        }
        self.profile
            .lifecycle
            .sort_by_key(|i| (i.start_offset_us, i.duration_us));
        self.profile.phases = self.phases.into_values().map(Phase::finish).collect();
        enforce_size_limit(&mut self.profile, crate::command_profile::MAX_PROFILE_BYTES)?;
        self.profile.validate()?;
        Ok(self.profile)
    }
}

// Per-kind cardinality caps bound live memory. JSON escaping can expand a
// bounded label by 6x, so independently enforce the reader's serialized budget
// without allocating a second serialized copy. Only exceptional profiles need
// more than one counting pass; every discarded observation is explicit.
fn enforce_size_limit(profile: &mut CommandProfile, limit: usize) -> anyhow::Result<()> {
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let Some(remaining) = self.0.checked_sub(bytes.len()) else {
                return Err(std::io::Error::other("profile byte budget exceeded"));
            };
            self.0 = remaining;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut noted = false;
    while serde_json::to_writer(Budget(limit), &*profile).is_err() {
        if !noted {
            profile.warnings.push("Command-profile byte or aggregate-interval limit reached; retained paths, intervals, examples and samples were reduced. Omitted evidence cannot establish complete coverage.".to_owned());
            noted = true;
        }
        let mut dropped = 0;
        let removed = halve(&mut profile.lifecycle);
        profile.dropped_lifecycle = profile.dropped_lifecycle.saturating_add(removed);
        dropped += removed;
        if let Some(path) = &mut profile.critical_path {
            let removed = halve(&mut path.entries);
            path.dropped_entries = path.dropped_entries.saturating_add(removed);
            dropped += removed;
        }
        for phase in &mut profile.phases {
            let removed = halve(&mut phase.union_intervals);
            phase.union_complete &= removed == 0;
            dropped += removed;
            let removed = halve(&mut phase.top);
            phase.dropped_examples = phase.dropped_examples.saturating_add(removed);
            dropped += removed;
        }
        // Retain the last endpoint as well as a prefix when possible. This is
        // still incomplete sampling, never evidence of intervening monotonicity.
        let before = profile.snapshots.len();
        if before > 1 {
            let last = profile.snapshots.pop().unwrap();
            profile.snapshots.truncate((before / 2).saturating_sub(1));
            profile.snapshots.push(last);
        } else {
            profile.snapshots.clear();
        }
        let removed = (before - profile.snapshots.len()) as u64;
        profile.dropped_snapshots = profile.dropped_snapshots.saturating_add(removed);
        dropped += removed;
        anyhow::ensure!(
            dropped != 0,
            "command-profile scalar metadata exceeds its byte budget"
        );
    }
    Ok(())
}

fn halve<T>(items: &mut Vec<T>) -> u64 {
    let before = items.len();
    items.truncate(before / 2);
    (before - items.len()) as u64
}

fn bounded(value: &str) -> String {
    bounded_to(value, MAX_LABEL)
}

fn bounded_to(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit - 3;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

fn timestamp_us(value: &prost_types::Timestamp) -> Option<i64> {
    if !(0..1_000_000_000).contains(&value.nanos) {
        return None;
    }
    value
        .seconds
        .checked_mul(1_000_000)?
        .checked_add(i64::from(value.nanos) / 1_000)
        .filter(|v| *v >= 0)
}

fn duration_us(value: &prost_types::Duration) -> Option<i64> {
    if value.seconds < 0 || !(0..1_000_000_000).contains(&value.nanos) {
        return None;
    }
    value
        .seconds
        .checked_mul(1_000_000)?
        .checked_add(i64::from(value.nanos) / 1_000)
}

macro_rules! kinds {
    ($data:expr, $enum:path) => {{
        use $enum as D;
        let (name, phase) = match $data {
            D::Command(_) => ("Command", false),
            D::CommandCritical(_) => ("CommandCritical", false),
            D::FileWatcher(_) => ("FileWatcher", false),
            D::DiceStateUpdate(_) => ("DiceStateUpdate", false),
            D::DiceCriticalSection(_) => ("DiceCriticalSection", false),
            D::DiceBlockConcurrentCommand(_) => ("DiceBlockConcurrentCommand", false),
            D::DiceSynchronizeSection(_) => ("DiceSynchronizeSection", false),
            D::DiceCleanup(_) => ("DiceCleanup", false),
            D::ExclusiveCommandWait(_) => ("ExclusiveCommandWait", false),
            D::ConnectToInstaller(_) => ("ConnectToInstaller", false),
            D::InstallEventInfo(_) => ("InstallEventInfo", false),
            D::CreateOutputSymlinks(_) => ("CreateOutputSymlinks", false),
            D::Load(_) => ("Load", true),
            D::LoadPackage(_) => ("LoadPackage", true),
            D::Analysis(_) => ("Analysis", true),
            D::AnalysisStage(_) => ("AnalysisStage", true),
            D::AnalysisResolveQueries(_) => ("AnalysisResolveQueries", true),
            D::Materialization(_) => ("Materialization", true),
            D::FinalMaterialization(_) => ("FinalMaterialization", true),
            _ => return None,
        };
        Some(Kind { name, phase })
    }};
}

pub(super) fn start_kind_kept(data: &buck2_data::span_start_event::Data) -> bool {
    start_kind(data).is_some()
}
pub(super) fn end_kind_kept(data: &buck2_data::span_end_event::Data) -> bool {
    end_kind(data).is_some()
}
fn start_kind(data: &buck2_data::span_start_event::Data) -> Option<Kind> {
    if matches!(data, buck2_data::span_start_event::Data::DynamicLambda(_)) {
        return Some(Kind {
            name: "DynamicLambda",
            phase: true,
        });
    }
    kinds!(data, buck2_data::span_start_event::Data)
}
fn end_kind(data: &buck2_data::span_end_event::Data) -> Option<Kind> {
    if matches!(
        data,
        buck2_data::span_end_event::Data::DeferredEvaluation(_)
    ) {
        return Some(Kind {
            name: "DynamicLambda",
            phase: true,
        });
    }
    kinds!(data, buck2_data::span_end_event::Data)
}

fn command_kind(command: &buck2_data::CommandStart) -> Option<&'static str> {
    use buck2_data::command_start::Data;
    Some(match command.data.as_ref()? {
        Data::Build(_) => "build",
        Data::Install(_) => "install",
        Data::Test(_) => "test",
        Data::Bxl(_) => "bxl",
        Data::Targets(_) => "targets",
        Data::Query(_) => "uquery",
        Data::Cquery(_) => "cquery",
        Data::Aquery(_) => "aquery",
        Data::Audit(_) => "audit",
        Data::Docs(_) => "docs",
        Data::Clean(_) => "clean",
        Data::Materialize(_) => "materialize",
        Data::Profile(_) => "profile",
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

fn configured(label: &buck2_data::ConfiguredTargetLabel) -> String {
    label
        .label
        .as_ref()
        .map(|l| format!("{}:{}", l.package, l.name))
        .unwrap_or_default()
}

fn start_label(data: &buck2_data::span_start_event::Data) -> Option<String> {
    use buck2_data::span_start_event::Data;
    match data {
        Data::Load(p) => Some(p.module_id.clone()),
        Data::LoadPackage(p) => Some(p.path.clone()),
        Data::Analysis(p) => Some(format!(
            "{} {}",
            p.rule,
            super::scan::analysis_detail(p.target.clone())
        )),
        Data::AnalysisResolveQueries(p) => p.standard_target.as_ref().map(configured),
        Data::Materialization(p) => p.action_digest.clone(),
        Data::InstallEventInfo(p) => Some(format!("{} {}", p.artifact_name, p.file_path)),
        Data::ConnectToInstaller(p) => Some(format!("localhost:{}", p.tcp_port)),
        Data::ExclusiveCommandWait(p) => p.command_name.clone(),
        Data::DiceBlockConcurrentCommand(p) => Some(p.current_active_trace_id.clone()),
        _ => None,
    }
}

fn cp_label(entry: &buck2_data::CriticalPathEntry2) -> (&'static str, String) {
    use buck2_data::critical_path_entry2::Entry;
    match &entry.entry {
        Some(Entry::Analysis(p)) => (
            "Analysis",
            match &p.target {
                Some(buck2_data::critical_path_entry2::analysis::Target::StandardTarget(t)) => {
                    configured(t)
                }
                None => String::new(),
            },
        ),
        Some(Entry::ActionExecution(p)) => (
            "ActionExecution",
            p.name
                .as_ref()
                .map(|n| format!("{} {}", n.category, n.identifier))
                .unwrap_or_default(),
        ),
        Some(Entry::FinalMaterialization(p)) => ("FinalMaterialization", p.path.clone()),
        Some(Entry::ComputeCriticalPath(_)) => ("ComputeCriticalPath", String::new()),
        Some(Entry::Load(p)) => ("Load", p.package.clone()),
        Some(Entry::Listing(p)) => ("Listing", p.package.clone()),
        Some(Entry::GenericEntry(p)) => ("GenericEntry", p.kind.clone()),
        Some(Entry::TestExecution(p)) => ("TestExecution", p.suite.clone()),
        Some(Entry::TestListing(p)) => ("TestListing", p.suite.clone()),
        Some(Entry::Waiting(p)) => ("Waiting", p.category.clone().unwrap_or_default()),
        Some(Entry::DynamicAnalysis(p)) => {
            ("DynamicAnalysis", format!("dynamic analysis {}", p.index))
        }
        Some(Entry::EnsureTransitiveSetProjection(_)) => {
            ("EnsureTransitiveSetProjection", String::new())
        }
        Some(Entry::AnonAnalysis(p)) => (
            "AnonAnalysis",
            p.anon_target
                .as_ref()
                .and_then(|t| t.name.as_ref())
                .map(|l| format!("{}:{}", l.package, l.name))
                .unwrap_or_default(),
        ),
        Some(Entry::PageIn(p)) => ("PageIn", p.key_type.clone()),
        Some(Entry::Install(p)) => (
            "Install",
            p.target_labels.first().map(configured).unwrap_or_default(),
        ),
        None => ("Unknown", String::new()),
    }
}

#[cfg(test)]
mod tests {
    use buck2_data::buck_event::Data;
    use buck2_data::span_end_event::Data as End;
    use buck2_data::span_start_event::Data as Start;

    use super::*;

    #[test]
    fn command_results_preserve_failure_and_bound_diagnostics() {
        use buck2_cli_proto::command_result::Result;
        let mut collector = Collector::new(Some(0), "run".into());
        collector.result(&StreamValue::Result(Box::new(
            buck2_cli_proto::CommandResult {
                result: Some(Result::Error(buck2_data::ErrorReport {
                    message: "é".repeat(20_000),
                    ..Default::default()
                })),
            },
        )));
        assert_eq!(collector.profile.result_outcome.as_deref(), Some("failed"));
        let error = collector.profile.result_error.as_ref().unwrap();
        assert!(error.len() <= 16_384);
        assert!(error.ends_with("..."));
        collector.profile.validate().unwrap();
    }

    #[test]
    fn cancelled_command_and_open_lifecycle_are_observations_not_rejections() {
        let mut c = Collector::new(Some(0), "run".to_owned());
        c.event(&start(1, 10, Start::Command(Default::default())));
        c.event(&start(2, 11, Start::FileWatcher(Default::default())));
        c.event(&end(
            1,
            30,
            Some(20),
            End::SpanCancelled(Default::default()),
        ));
        // An unrelated timestamp-less RE record is not profile evidence.
        let mut irrelevant = start(99, 0, Start::ReUpload(Default::default()));
        irrelevant.timestamp = None;
        c.event(&irrelevant);
        let p = c.finish(true).unwrap();
        assert!(p.command_cancelled);
        assert_eq!(p.command_end_offset_us, Some(30));
        assert_eq!(p.lifecycle_open_at_eof, 1);
        assert_eq!(p.rejected_records, 0);
        assert_eq!(p.lifecycle.len(), 1);
        assert!(p.lifecycle[0].cancelled);
    }

    #[test]
    fn test_failure_and_partial_targets_keep_their_distinct_result_semantics() {
        use buck2_cli_proto::command_result::Result;
        let mut c = Collector::new(Some(0), "run".to_owned());
        c.result(&StreamValue::Result(Box::new(
            buck2_cli_proto::CommandResult {
                result: Some(Result::TestResponse(buck2_cli_proto::TestResponse {
                    executor_exit_code: 1,
                    ..Default::default()
                })),
            },
        )));
        assert_eq!(c.profile.result_outcome.as_deref(), Some("tests-failed"));
        let mut c = Collector::new(Some(0), "run".to_owned());
        c.result(&StreamValue::Result(Box::new(
            buck2_cli_proto::CommandResult {
                result: Some(Result::TargetsResponse(buck2_cli_proto::TargetsResponse {
                    error_count: 2,
                    ..Default::default()
                })),
            },
        )));
        assert_eq!(c.profile.result_outcome.as_deref(), Some("success"));
        assert!(
            c.profile
                .warnings
                .iter()
                .any(|w| w.contains("2 package error"))
        );
    }

    #[test]
    fn query_names_are_not_collapsed_to_other() {
        use buck2_data::command_start::Data;
        for (data, expected) in [
            (Data::Query(Default::default()), "uquery"),
            (Data::Cquery(Default::default()), "cquery"),
            (Data::Aquery(Default::default()), "aquery"),
            (Data::Audit(Default::default()), "audit"),
            (Data::Targets(Default::default()), "targets"),
            (Data::Test(Default::default()), "test"),
        ] {
            assert_eq!(
                command_kind(&buck2_data::CommandStart {
                    data: Some(data),
                    ..Default::default()
                }),
                Some(expected)
            );
        }
    }

    fn duration(us: i64) -> prost_types::Duration {
        prost_types::Duration {
            seconds: us / 1_000_000,
            nanos: (us % 1_000_000) as i32 * 1_000,
        }
    }

    fn event(sid: u64, us: i64, data: Data) -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            span_id: sid,
            timestamp: Some(prost_types::Timestamp {
                seconds: us / 1_000_000,
                nanos: (us % 1_000_000) as i32 * 1_000,
            }),
            data: Some(data),
            ..Default::default()
        }
    }

    fn start(sid: u64, us: i64, data: Start) -> buck2_data::BuckEvent {
        event(
            sid,
            us,
            Data::SpanStart(buck2_data::SpanStartEvent { data: Some(data) }),
        )
    }

    fn end(sid: u64, us: i64, dur: Option<i64>, data: End) -> buck2_data::BuckEvent {
        event(
            sid,
            us,
            Data::SpanEnd(buck2_data::SpanEndEvent {
                data: Some(data),
                duration: dur.map(duration),
                ..Default::default()
            }),
        )
    }

    fn interval(start: i64, duration: i64) -> ObservedInterval {
        ObservedInterval {
            kind: "Analysis".to_owned(),
            start_offset_us: start,
            duration_us: duration,
            ..Default::default()
        }
    }

    #[test]
    fn overlapping_phases_keep_union_separate_from_sum_and_lifecycle() {
        let mut c = Collector::new(Some(100), "run".to_owned());
        for e in [
            start(1, 110, Start::Command(Default::default())),
            start(2, 120, Start::Analysis(Default::default())),
            start(3, 130, Start::Analysis(Default::default())),
            end(2, 160, Some(40), End::Analysis(Default::default())),
            end(3, 180, Some(50), End::Analysis(Default::default())),
            start(4, 180, Start::Analysis(Default::default())),
            end(4, 180, Some(0), End::Analysis(Default::default())),
            end(1, 200, Some(90), End::Command(Default::default())),
        ] {
            c.event(&e);
        }
        let p = c.finish(false).unwrap();
        let phase = &p.phases[0];
        assert_eq!((phase.count, phase.completed, phase.cancelled), (3, 3, 0));
        assert_eq!(phase.total_duration_us, 90);
        assert_eq!(phase.union_intervals, [[20, 80]]);
        assert_eq!(phase.active_us(), Some(60));
        assert_eq!(
            phase.top.iter().map(|i| i.duration_us).collect::<Vec<_>>(),
            [50, 40, 0]
        );
        assert_eq!(p.lifecycle.len(), 1);
        assert_eq!(p.lifecycle[0].duration_us, 90);
        assert_eq!(
            (p.command_start_offset_us, p.command_end_offset_us),
            (Some(10), Some(100))
        );
        assert!(p.client_wall_us.is_none());
    }

    #[test]
    fn cancelled_missing_invalid_duplicate_and_open_spans_do_not_invent_coverage() {
        let mut c = Collector::new(Some(0), "run".to_owned());
        for e in [
            start(1, 10, Start::Analysis(Default::default())),
            end(1, 30, Some(20), End::SpanCancelled(Default::default())),
            start(2, 40, Start::Analysis(Default::default())),
            end(2, 50, None, End::Analysis(Default::default())),
            start(3, 60, Start::Analysis(Default::default())),
            end(3, 70, Some(-1), End::Analysis(Default::default())),
            start(4, 80, Start::Analysis(Default::default())),
            start(4, 81, Start::Fake(Default::default())),
            end(4, 90, Some(10), End::Analysis(Default::default())),
            start(5, 100, Start::Analysis(Default::default())),
            end(5, 110, Some(10), End::Load(Default::default())),
            start(6, 120, Start::Analysis(Default::default())),
        ] {
            c.event(&e);
        }
        let p = c.finish(true).unwrap();
        let a = &p.phases[0];
        assert_eq!(
            (
                a.count,
                a.completed,
                a.cancelled,
                a.unmatched_ends,
                a.open_at_eof
            ),
            (6, 2, 1, 1, 1)
        );
        assert_eq!(a.union_intervals, [[10, 30]]);
        assert_eq!(a.total_duration_us, 20);
        assert!(a.top[0].cancelled);
        assert!(!a.union_complete && a.active_us().is_none());
        assert_eq!(a.rejected_intervals, 4);
        assert_eq!(p.rejected_records, 4);
        assert!(p.warnings.iter().any(|w| w.contains("readable prefix")));
        assert!(
            duration_us(&prost_types::Duration {
                seconds: i64::MAX,
                nanos: 0
            })
            .is_none()
        );
    }

    #[test]
    fn producer_path_preserves_kinds_missingness_and_its_separate_clock() {
        use buck2_data::critical_path_entry2::Entry;
        let kinds = [
            Entry::Analysis(Default::default()),
            Entry::AnonAnalysis(Default::default()),
            Entry::DynamicAnalysis(Default::default()),
            Entry::ActionExecution(Default::default()),
            Entry::FinalMaterialization(Default::default()),
            Entry::ComputeCriticalPath(Default::default()),
            Entry::Load(Default::default()),
            Entry::Listing(Default::default()),
            Entry::GenericEntry(Default::default()),
            Entry::TestExecution(Default::default()),
            Entry::TestListing(Default::default()),
            Entry::Waiting(Default::default()),
            Entry::EnsureTransitiveSetProjection(Default::default()),
            Entry::PageIn(Default::default()),
        ];
        let mut c = Collector::new(Some(1_000), "run".to_owned());
        c.event(&start(1, 1_100, Start::Command(Default::default())));
        c.critical_path(&buck2_data::BuildGraphExecutionInfo {
            backend_name: Some("longest-path-graph".to_owned()),
            critical_path2: kinds
                .into_iter()
                .enumerate()
                .map(|(i, entry)| buck2_data::CriticalPathEntry2 {
                    entry: Some(entry),
                    span_ids: vec![i as u64],
                    start_offset_ns: Some(123_456),
                    duration: Some(duration(0)),
                    total_duration: Some(duration(7)),
                    user_duration: Some(duration(2)),
                    queue_duration: Some(duration(0)),
                    potential_improvement_duration: Some(duration(-1)),
                    was_reused: Some(true),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        let p = c.finish(false).unwrap();
        let cp = p.critical_path.unwrap();
        assert_eq!(cp.backend.as_deref(), Some("longest-path-graph"));
        assert_eq!(cp.command_start_offset_us, Some(100));
        assert!(cp.approximate_alignment);
        assert_eq!(cp.entries.len(), 14);
        assert_eq!(cp.entries[0].kind, "Analysis");
        assert_eq!(cp.entries[13].kind, "PageIn");
        for e in &cp.entries {
            assert_eq!(
                (
                    e.start_offset_us,
                    e.duration_us,
                    e.total_us,
                    e.user_us,
                    e.queue_us
                ),
                (Some(123), Some(0), Some(7), Some(2), Some(0))
            );
            assert!(e.noncritical_us.is_none() && e.potential_improvement_us.is_none());
            assert_eq!(e.reused, Some(true));
            // Reused entries have a validation-completion position, not an
            // interval for their original execution.
            assert_eq!(cp.positioned_start_us(e), Some(223));
        }
        assert_eq!(p.rejected_records, 14);
        assert_eq!(p.lifecycle_open_at_eof, 1);
    }

    #[test]
    fn install_response_and_raw_snapshot_fields_have_explicit_scope() {
        let mut c = Collector::new(None, "run".to_owned());
        c.event(&start(
            1,
            100,
            Start::Command(buck2_data::CommandStart {
                data: Some(buck2_data::command_start::Data::Install(Default::default())),
                ..Default::default()
            }),
        ));
        c.event(&event(
            0,
            120,
            Data::Instant(buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::Snapshot(Box::new(
                    buck2_data::Snapshot {
                        buck2_rss: Some(7),
                        buck2_max_rss: 999,
                        malloc_bytes_active: None,
                        buck2_user_cpu_us: 5,
                        buck2_system_cpu_us: 8,
                        re_download_bytes: 123,
                        re_upload_bytes: 0,
                        deferred_materializer_queue_size: 663_727,
                        ..Default::default()
                    },
                ))),
            }),
        ));
        c.event(&event(
            0,
            170,
            Data::Instant(buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::InstallFinished(
                    buck2_data::InstallFinished {
                        duration: Some(duration(3)),
                        ..Default::default()
                    },
                )),
            }),
        ));
        c.event(&end(1, 200, Some(100), End::Command(Default::default())));
        c.result(&StreamValue::Result(Box::new(
            buck2_cli_proto::CommandResult {
                result: Some(buck2_cli_proto::command_result::Result::InstallResponse(
                    Default::default(),
                )),
            },
        )));
        assert_eq!(c.profile.result_outcome.as_deref(), Some("success"));
        c.result(&StreamValue::Result(Box::new(
            buck2_cli_proto::CommandResult {
                result: Some(buck2_cli_proto::command_result::Result::Error(
                    Default::default(),
                )),
            },
        )));
        let p = c.finish(false).unwrap();
        assert_eq!(p.result_outcome.as_deref(), Some("failed"));
        assert_eq!(p.command_kind.as_deref(), Some("install"));
        assert_eq!(
            (
                p.origin_us,
                p.command_start_offset_us,
                p.command_end_offset_us
            ),
            (100, Some(0), Some(100))
        );
        assert_eq!(p.install_duration_us, Some(3));
        assert_eq!(
            p.snapshots,
            [SnapshotSample {
                offset_us: 20,
                cpu_us: Some(13),
                download_bytes: Some(123),
                upload_bytes: Some(0),
                rss_bytes: Some(7),
                heap_bytes: None,
                deferred_materializer_queue_size: Some(663_727),
            }]
        );
        assert!(p.client_wall_us.is_none());
        assert!(p.notes.iter().any(|w| w.contains("daemon-wide")));
    }

    #[test]
    fn native_materializer_zero_preserves_the_unversioned_scalar() {
        let mut c = Collector::new(None, "run".to_owned());
        c.event(&event(
            0,
            120,
            Data::Instant(buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::Snapshot(Box::default())),
            }),
        ));
        let p = c.finish(false).unwrap();
        assert_eq!(p.snapshots[0].deferred_materializer_queue_size, Some(0));
        // The profile notes already warn that producer scalar zero does not
        // prove presence; the timeline's gauge help repeats that limitation.
        assert!(p.notes.iter().any(|note| note.contains("Scalar zero")));
    }

    #[test]
    fn cardinality_caps_and_escaped_json_budget_make_loss_explicit() {
        let mut phase = Phase::new("Analysis");
        for i in 0..MAX_UNION + 1 {
            phase.interval(interval(i as i64 * 2, 1));
        }
        let p = phase.finish();
        assert_eq!(p.union_intervals.len(), MAX_UNION);
        assert!(!p.union_complete);
        assert_eq!(p.top.len(), MAX_TOP);
        assert_eq!(p.dropped_examples, (MAX_UNION + 1 - MAX_TOP) as u64);

        let mut c = Collector::new(Some(0), "run".to_owned());
        for i in 0..MAX_OPEN_PHASES + 1 {
            c.event(&start(i as u64, 0, Start::Analysis(Default::default())));
        }
        let p = c.finish(false).unwrap();
        assert_eq!(p.phases[0].count, (MAX_OPEN_PHASES + 1) as u64);
        assert_eq!(p.phases[0].open_at_eof, MAX_OPEN_PHASES as u64);
        assert_eq!(p.rejected_records, 1);

        let mut profile = CommandProfile {
            run: Some("run".to_owned()),
            lifecycle: (0..100)
                .map(|i| ObservedInterval {
                    label: Some("\u{1}".repeat(MAX_LABEL)),
                    ..interval(i, 1)
                })
                .collect(),
            critical_path: Some(BuckCriticalPath {
                entries: (0..100)
                    .map(|_| CriticalPathEntry {
                        label: "\u{1}".repeat(MAX_LABEL),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }),
            phases: vec![p.phases[0].clone()],
            snapshots: (0..100)
                .map(|i| SnapshotSample {
                    offset_us: i,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        enforce_size_limit(&mut profile, 16_384).unwrap();
        assert!(serde_json::to_vec(&profile).unwrap().len() <= 16_384);
        assert!(profile.dropped_lifecycle > 0 && profile.dropped_snapshots > 0);
        assert!(profile.critical_path.as_ref().unwrap().dropped_entries > 0);
        assert_eq!(profile.snapshots.last().unwrap().offset_us, 99);
        let text = "🦀".repeat(MAX_LABEL);
        assert!(bounded(&text).len() <= MAX_LABEL);
    }

    #[test]
    fn invalid_absolute_ranges_are_rejected_without_losing_valid_observations() {
        let mut c = Collector::new(Some(1), "run".to_owned());
        c.event(&start(1, 1, Start::Analysis(Default::default())));
        c.event(&end(
            1,
            2,
            Some(i64::MAX),
            End::Analysis(Default::default()),
        ));
        c.event(&start(2, 3, Start::Analysis(Default::default())));
        c.event(&end(2, 4, Some(1), End::Analysis(Default::default())));
        let mut invalid = start(3, 1, Start::Analysis(Default::default()));
        invalid.timestamp.as_mut().unwrap().seconds = -1;
        c.event(&invalid);
        invalid.timestamp = None;
        c.event(&invalid);
        c.critical_path(&buck2_data::BuildGraphExecutionInfo {
            critical_path2: [false, true]
                .into_iter()
                .map(|reused| buck2_data::CriticalPathEntry2 {
                    start_offset_ns: Some(1_000),
                    total_duration: Some(duration(i64::MAX)),
                    duration: Some(duration(42)),
                    was_reused: Some(reused),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        let p = c.finish(false).unwrap();
        assert_eq!(p.rejected_records, 4);
        assert_eq!(p.phases[0].union_intervals, [[2, 3]]);
        assert!(!p.phases[0].union_complete);
        let entries = &p.critical_path.as_ref().unwrap().entries;
        assert_eq!(entries[0].total_us, None);
        assert_eq!(entries[0].duration_us, Some(42));
        assert_eq!(
            entries[1].total_us,
            Some(i64::MAX),
            "reused totals are not positioned spans"
        );
        let args = serde_json::json!({"run":"run", "profile":p});
        assert!(crate::command_profile::decode_args(&args).is_ok());
    }

    #[test]
    fn bounded_union_keeps_late_long_coverage_and_both_indexes_coherent() {
        fn indexes_match(p: &Phase) {
            assert_eq!(p.union.len(), p.union_by_length.len());
            assert_eq!(
                p.union_by_length,
                p.union.iter().map(|(&a, &b)| (b - a, a)).collect()
            );
            assert!(p.union.len() <= MAX_UNION);
        }
        let mut p = Phase::new("Analysis");
        for i in 0..MAX_UNION {
            p.interval(interval(i as i64 * 2, 1));
        }
        indexes_match(&p);
        let late = 100_000;
        p.interval(interval(late, 1_765_055));
        assert_eq!(p.union.get(&late), Some(&(late + 1_765_055)));
        assert!(!p.union.contains_key(&0));
        assert!(!p.summary.union_complete);
        indexes_match(&p);
        // Equal small segments do not churn the deterministic retained set.
        let before = p.union.clone();
        p.interval(interval(late + 2_000_000, 1));
        assert_eq!(p.union, before);
        // A new span may bridge retained segments; the gap is covered by that
        // span, not filled merely to reduce cardinality.
        p.interval(interval(0, 10));
        assert_eq!(p.union.get(&0), Some(&11));
        indexes_match(&p);
        p.interval(interval(0, late + 1_765_055));
        assert_eq!(p.union.len(), 1);
        indexes_match(&p);
        assert!(
            !p.summary.union_complete,
            "previous discarded evidence remains explicit"
        );
        assert_eq!(p.finish().retained_active_us(), Some(late + 1_765_055));
    }

    #[test]
    fn phase_open_limit_reserves_lifecycle_and_releases_replaced_cancelled_slots() {
        let mut c = Collector::new(Some(0), "run".to_owned());
        for i in 0..MAX_OPEN_PHASES {
            c.event(&start(i as u64, 0, Start::Analysis(Default::default())));
        }
        let cmd = MAX_OPEN_PHASES as u64 + 1;
        c.event(&start(cmd, 1, Start::Command(Default::default())));
        assert_eq!(c.open_counts, [1, MAX_OPEN_PHASES]);
        c.event(&start(0, 2, Start::FileWatcher(Default::default())));
        assert_eq!(c.open_counts, [2, MAX_OPEN_PHASES - 1]);
        c.event(&start(cmd + 1, 3, Start::Analysis(Default::default())));
        c.event(&end(1, 4, Some(4), End::SpanCancelled(Default::default())));
        assert_eq!(c.open_counts, [2, MAX_OPEN_PHASES - 1]);
        c.event(&start(cmd + 2, 5, Start::Analysis(Default::default())));
        assert_eq!(c.open_counts, [2, MAX_OPEN_PHASES]);
        c.event(&end(0, 6, Some(4), End::FileWatcher(Default::default())));
        c.event(&end(cmd, 7, Some(6), End::Command(Default::default())));
        assert_eq!(c.open_counts, [0, MAX_OPEN_PHASES]);
        let p = c.finish(false).unwrap();
        assert_eq!(p.lifecycle.len(), 2);
        assert_eq!(
            p.rejected_records, 1,
            "only duplicate replacement rejected an interval"
        );
        assert_eq!(p.phases[0].cancelled, 1);
        assert_eq!(p.phases[0].open_at_eof, MAX_OPEN_PHASES as u64);
        assert_eq!(p.phases[0].count, MAX_OPEN_PHASES as u64 + 2);
        assert!(!p.phases[0].union_complete);
    }

    #[test]
    fn timestamp_refusal_invalidates_only_matching_profile_sid_in_raw_and_compacted_inputs() {
        let mut missing = end(1, 20, Some(10), End::Analysis(Default::default()));
        missing.timestamp = None;
        let mut invalid = end(2, 40, Some(10), End::Analysis(Default::default()));
        invalid.timestamp.as_mut().unwrap().nanos = -1;
        let mut command_missing = end(3, 60, Some(10), End::Command(Default::default()));
        command_missing.timestamp = None;
        let events = [
            start(1, 10, Start::Analysis(Default::default())),
            missing,
            end(1, 21, Some(11), End::Analysis(Default::default())),
            start(2, 30, Start::Analysis(Default::default())),
            invalid,
            end(2, 41, Some(11), End::Analysis(Default::default())),
            start(3, 50, Start::Command(Default::default())),
            command_missing,
            end(3, 61, Some(11), End::Command(Default::default())),
            start(4, 1000, Start::Analysis(Default::default())),
            end(4, 1001, Some(1), End::Analysis(Default::default())),
        ];
        let mut expected = None;
        for compact in [false, true] {
            for phases in [false, true] {
                let mut c = Collector::new(Some(0), "run".to_owned());
                for event in &events {
                    let value = StreamValue::Event(Box::new(event.clone()));
                    let value = if compact {
                        super::super::compact::clean(value, phases)
                    } else {
                        value
                    };
                    let StreamValue::Event(event) = value else {
                        unreachable!()
                    };
                    c.event(&event);
                }
                assert_eq!(c.open_counts, [0, 0]);
                let p = c.finish(false).unwrap();
                assert_eq!(p.rejected_records, 3);
                assert!(p.lifecycle.is_empty());
                assert_eq!(p.phases[0].union_intervals, [[1000, 1001]]);
                assert_eq!(p.phases[0].rejected_intervals, 2);
                assert_eq!(p.phases[0].unmatched_ends, 2);
                assert!(!p.phases[0].union_complete);
                if let Some(expected) = &expected {
                    assert_eq!(&p, expected);
                } else {
                    expected = Some(p);
                }
            }
        }
    }

    #[test]
    fn rejected_start_and_unmatched_end_are_separate_bounded_record_diagnostics() {
        let mut collector = Collector::new(Some(0), "run".to_owned());
        for sid in 1..=MAX_OPEN_PHASES as u64 {
            collector.event(&start(sid, 1, Start::Analysis(Default::default())));
        }
        let sid = MAX_OPEN_PHASES as u64 + 1;
        collector.event(&start(sid, 2, Start::Analysis(Default::default())));
        collector.event(&end(sid, 3, Some(1), End::Analysis(Default::default())));
        assert_eq!(collector.open.len(), MAX_OPEN_PHASES);
        let profile = collector.finish(false).unwrap();
        assert_eq!(profile.rejected_records, 1, "one start record was rejected");
        let phase = &profile.phases[0];
        assert_eq!(phase.rejected_intervals, 1);
        assert_eq!(
            phase.unmatched_ends, 1,
            "the matching end had no retained start"
        );
        assert_eq!(phase.open_at_eof, MAX_OPEN_PHASES as u64);
    }
}
