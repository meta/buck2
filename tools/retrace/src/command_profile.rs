/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Whole-command evidence, independent of the action DAG and analysis pass.
//!
//! Recorded client-clock intervals, Buck's critical-path model, and daemon-wide
//! cumulative counters are different measurements. This module does not turn
//! durations into timestamps, sum overlapping phases into command wall time,
//! or attribute daemon counters to individual actions.

use anyhow::Context as _;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;
/// Limit each profile before deserializing its owned strings and arrays.
pub const MAX_PROFILE_BYTES: usize = 8 << 20;
pub const MAX_PROFILE_RECORDS: usize = 64;
/// Across lifecycle, phase unions and example intervals, not per phase.
pub const MAX_PROFILE_INTERVALS: usize = 65_536;
const MAX_ITEMS: usize = 16_384;
const MAX_TEXT_BYTES: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandProfile {
    pub schema_version: u32,
    /// Absolute microseconds used only as the origin of recorded offsets.
    /// This need not be the client command's start; the producer explains a
    /// fallback origin in `warnings`.
    pub origin_us: i64,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub command_kind: Option<String>,
    #[serde(default)]
    pub result_outcome: Option<String>,
    /// Bounded diagnostic from the terminal response, not command stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_error: Option<String>,
    #[serde(default)]
    pub command_start_offset_us: Option<i64>,
    #[serde(default)]
    pub command_end_offset_us: Option<i64>,
    #[serde(default)]
    pub command_cancelled: bool,
    /// A duration only. Never derive a start/end from this field.
    #[serde(default)]
    pub client_wall_us: Option<i64>,
    /// Installer-reported duration only, not a positioned lifecycle interval.
    #[serde(default)]
    pub install_duration_us: Option<i64>,
    #[serde(default)]
    pub lifecycle: Vec<ObservedInterval>,
    #[serde(default)]
    pub dropped_lifecycle: u64,
    #[serde(default)]
    pub lifecycle_open_at_eof: u64,
    #[serde(default)]
    pub rejected_records: u64,
    #[serde(default)]
    pub critical_path: Option<BuckCriticalPath>,
    #[serde(default)]
    pub phases: Vec<PhaseSummary>,
    #[serde(default)]
    pub snapshots: Vec<SnapshotSample>,
    #[serde(default)]
    pub dropped_snapshots: u64,
    #[serde(default)]
    pub snapshot_windows: Vec<SnapshotWindow>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Scope and interpretation notes do not imply missing evidence.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Default for CommandProfile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            origin_us: 0,
            run: None,
            command_kind: None,
            result_outcome: None,
            result_error: None,
            command_start_offset_us: None,
            command_end_offset_us: None,
            command_cancelled: false,
            client_wall_us: None,
            install_duration_us: None,
            lifecycle: Vec::new(),
            dropped_lifecycle: 0,
            lifecycle_open_at_eof: 0,
            rejected_records: 0,
            critical_path: None,
            phases: Vec::new(),
            snapshots: Vec::new(),
            dropped_snapshots: 0,
            snapshot_windows: Vec::new(),
            warnings: Vec::new(),
            notes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedInterval {
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    pub start_offset_us: i64,
    pub duration_us: i64,
    #[serde(default)]
    pub cancelled: bool,
}

impl ObservedInterval {
    pub fn end_offset_us(&self) -> Option<i64> {
        (self.duration_us >= 0)
            .then(|| self.start_offset_us.checked_add(self.duration_us))
            .flatten()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuckCriticalPath {
    #[serde(default)]
    pub backend: Option<String>,
    /// Offset, relative to `CommandProfile.origin_us`, of the daemon command
    /// start used by Buck's `start_offset_ns`. Absent means entry offsets
    /// cannot be placed on the profile's clock.
    #[serde(default)]
    pub command_start_offset_us: Option<i64>,
    /// Alignment to the recorded daemon Command start, not an exact clock join.
    #[serde(default)]
    pub approximate_alignment: bool,
    #[serde(default)]
    pub entries: Vec<CriticalPathEntry>,
    #[serde(default)]
    pub dropped_entries: u64,
}

impl BuckCriticalPath {
    /// Recorded start position only; a model duration is not an observed end.
    /// On reused entries this is validation completion, not original execution.
    pub fn positioned_start_us(&self, entry: &CriticalPathEntry) -> Option<i64> {
        self.command_start_offset_us?
            .checked_add(entry.start_offset_us?)
    }

    /// Complete sum of the reported model weights, not observed command wall.
    pub fn model_duration_us(&self) -> Option<i64> {
        if self.dropped_entries != 0 {
            return None;
        }
        self.entries.iter().try_fold(0i64, |sum, e| {
            let d = e.duration_us?;
            (d >= 0).then(|| sum.checked_add(d)).flatten()
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriticalPathEntry {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub span_ids: Vec<u64>,
    /// File-order positions, among the trace's action records, of the actions in `span_ids`
    /// that the trace holds, so a viewer can open them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acts: Vec<u32>,
    /// Relative to Buck's daemon-command origin, NOT `origin_us` directly.
    #[serde(default)]
    pub start_offset_us: Option<i64>,
    /// Buck's model weight. Missing, measured zero, and reused are distinct.
    #[serde(default)]
    pub duration_us: Option<i64>,
    #[serde(default)]
    pub total_us: Option<i64>,
    #[serde(default)]
    pub user_us: Option<i64>,
    #[serde(default)]
    pub queue_us: Option<i64>,
    #[serde(default)]
    pub noncritical_us: Option<i64>,
    #[serde(default)]
    pub potential_improvement_us: Option<i64>,
    #[serde(default)]
    pub reused: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseSummary {
    pub kind: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub completed: u64,
    #[serde(default)]
    pub cancelled: u64,
    #[serde(default)]
    pub unmatched_ends: u64,
    #[serde(default)]
    pub open_at_eof: u64,
    /// Sum of observed spans, including overlap; NOT phase/command wall time.
    #[serde(default)]
    pub total_duration_us: i64,
    /// Sorted disjoint half-open intervals on the profile's clock.
    #[serde(default)]
    pub union_intervals: Vec<[i64; 2]>,
    /// False means the retained union is only a lower bound on observations.
    #[serde(default)]
    pub union_complete: bool,
    #[serde(default)]
    pub top: Vec<ObservedInterval>,
    #[serde(default)]
    pub dropped_examples: u64,
    #[serde(default)]
    pub rejected_intervals: u64,
    #[serde(default)]
    pub clipped_intervals: u64,
}

impl PhaseSummary {
    pub fn has_incomplete_evidence(&self) -> bool {
        !self.union_complete
            || self.open_at_eof != 0
            || self.unmatched_ends != 0
            || self.rejected_intervals != 0
            || self.clipped_intervals != 0
    }

    /// Exact union of retained observations only when none were lost/clipped.
    /// Open spans still have no invented end and contribute no interval.
    pub fn active_us(&self) -> Option<i64> {
        if !self.union_complete || self.rejected_intervals != 0 || self.clipped_intervals != 0 {
            return None;
        }
        self.retained_active_us()
    }

    /// Lower-bound coverage if `active_us` is unavailable. Never add this to
    /// another phase's coverage: their unions can overlap.
    pub fn retained_active_us(&self) -> Option<i64> {
        let mut last = None;
        self.union_intervals.iter().try_fold(0i64, |sum, &[a, b]| {
            if b < a || last.is_some_and(|end| a < end) {
                return None;
            }
            last = Some(b);
            sum.checked_add(b.checked_sub(a)?)
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSample {
    pub offset_us: i64,
    #[serde(default)]
    pub cpu_us: Option<u64>,
    #[serde(default)]
    pub download_bytes: Option<u64>,
    #[serde(default)]
    pub upload_bytes: Option<u64>,
    #[serde(default)]
    pub rss_bytes: Option<u64>,
    #[serde(default)]
    pub heap_bytes: Option<u64>,
    /// Pending daemon materializer commands across both priority queues, not
    /// actions or requests ahead of a selected action. This is a gauge.
    /// None means the converted trace omitted the field. Native protobuf uses
    /// a scalar: Some(0) may mean empty or an older producer's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_materializer_queue_size: Option<u64>,
}

/// Range of retained, reported gauge values, not a continuous-time maximum.
/// Zero remains the producer's scalar value; it does not establish presence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterializerQueueRange {
    pub minimum: u64,
    pub maximum: u64,
    pub sample_count: u64,
}

/// No interpolation, extrapolation, rates, or action attribution. Missing
/// samples are omitted, not treated as zero; decreasing values are valid.
pub fn materializer_queue_range<'a>(
    samples: impl IntoIterator<Item = &'a SnapshotSample>,
) -> Option<MaterializerQueueRange> {
    let mut values = samples
        .into_iter()
        .filter_map(|sample| sample.deferred_materializer_queue_size);
    let first = values.next()?;
    let mut range = MaterializerQueueRange {
        minimum: first,
        maximum: first,
        sample_count: 1,
    };
    for value in values {
        range.minimum = range.minimum.min(value);
        range.maximum = range.maximum.max(value);
        range.sample_count += 1;
    }
    Some(range)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterWindow {
    pub first: Option<u64>,
    pub last: Option<u64>,
    /// Available only after checking every retained sample for reset/missing
    /// values and confirming that no source samples were dropped.
    pub delta: Option<u64>,
    pub reset_observed: bool,
    pub missing_samples: u64,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotWindow {
    pub start_offset_us: i64,
    pub end_offset_us: i64,
    pub sample_count: u64,
    pub first_offset_us: Option<i64>,
    pub last_offset_us: Option<i64>,
    /// First-to-last sampled span, never the full requested-window duration.
    pub covered_us: Option<i64>,
    pub max_gap_us: Option<i64>,
    pub dropped_samples: u64,
    pub download: CounterWindow,
    pub upload: CounterWindow,
    pub cpu: CounterWindow,
    pub warnings: Vec<String>,
}

impl SnapshotWindow {
    /// Endpoint coverage, not proof of continuous observation between samples.
    pub fn coverage_fraction(&self) -> Option<f64> {
        let full = self.end_offset_us.checked_sub(self.start_offset_us)?;
        let covered = self.covered_us?;
        (full > 0 && covered >= 0 && covered <= full).then_some(covered as f64 / full as f64)
    }

    /// Counter units per second over sampled endpoints only. For `cpu`, the
    /// units are CPU microseconds/second; use `cpu_cores` for average cores.
    pub fn rate_per_second(&self, counter: &CounterWindow) -> Option<f64> {
        let covered = self.covered_us?;
        (covered > 0).then(|| counter.delta.map(|d| d as f64 * 1e6 / covered as f64))?
    }

    /// Average daemon CPU cores, not CPU attributed to a phase or action.
    pub fn cpu_cores(&self) -> Option<f64> {
        self.rate_per_second(&self.cpu).map(|rate| rate / 1e6)
    }
}

fn text_bound(s: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        s.len() <= MAX_TEXT_BYTES,
        "profile text exceeds {MAX_TEXT_BYTES} bytes"
    );
    Ok(())
}

fn optional_duration(value: Option<i64>) -> anyhow::Result<()> {
    anyhow::ensure!(value.is_none_or(|n| n >= 0), "negative profile duration");
    Ok(())
}

impl CommandProfile {
    /// Evidence loss is numeric, not dependent on producer warning prose.
    /// Approximate alignment, reuse and bounded representative examples are
    /// interpretation limits, not missing observations by themselves.
    pub fn has_incomplete_evidence(&self) -> bool {
        !self.warnings.is_empty()
            || self.rejected_records != 0
            || self.dropped_lifecycle != 0
            || self.lifecycle_open_at_eof != 0
            || self.dropped_snapshots != 0
            || self
                .critical_path
                .as_ref()
                .is_some_and(|path| path.dropped_entries != 0)
            || self
                .phases
                .iter()
                .any(PhaseSummary::has_incomplete_evidence)
    }

    pub fn has_evidence(&self) -> bool {
        self.command_kind.is_some()
            || self.result_outcome.is_some()
            || self.result_error.is_some()
            || self.command_start_offset_us.is_some()
            || self.command_end_offset_us.is_some()
            || !self.lifecycle.is_empty()
            || !self.phases.is_empty()
            || !self.snapshots.is_empty()
            || self.critical_path.is_some()
    }

    pub fn absolute_us(&self, offset: i64) -> Option<i64> {
        self.origin_us.checked_add(offset).filter(|&t| t >= 0)
    }

    fn offset(&self, offset: i64) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.absolute_us(offset).is_some(),
            "profile timestamp is unrepresentable"
        );
        Ok(())
    }

    fn interval(&self, interval: &ObservedInterval) -> anyhow::Result<()> {
        text_bound(&interval.kind)?;
        if let Some(label) = &interval.label {
            text_bound(label)?;
        }
        self.offset(interval.start_offset_us)?;
        self.offset(
            interval
                .end_offset_us()
                .context("invalid observed profile interval")?,
        )
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema_version == SCHEMA_VERSION,
            "unsupported command-profile schema {}",
            self.schema_version
        );
        anyhow::ensure!(self.origin_us >= 0, "negative profile origin");
        for text in [
            &self.run,
            &self.command_kind,
            &self.result_outcome,
            &self.result_error,
        ]
        .into_iter()
        .flatten()
        {
            text_bound(text)?;
        }
        for offset in [self.command_start_offset_us, self.command_end_offset_us]
            .into_iter()
            .flatten()
        {
            self.offset(offset)?;
        }
        if let (Some(a), Some(b)) = (self.command_start_offset_us, self.command_end_offset_us) {
            anyhow::ensure!(b >= a, "command end precedes command start");
        }
        optional_duration(self.client_wall_us)?;
        optional_duration(self.install_duration_us)?;
        anyhow::ensure!(
            self.lifecycle.len() <= MAX_ITEMS
                && self.phases.len() <= 256
                && self.snapshots.len() <= MAX_ITEMS
                && self.snapshot_windows.len() <= 256
                && self.warnings.len() <= 1024
                && self.notes.len() <= 1024,
            "command-profile collection exceeds reader limit"
        );
        let intervals = self
            .phases
            .iter()
            .try_fold(self.lifecycle.len(), |n, phase| {
                n.checked_add(phase.union_intervals.len())?
                    .checked_add(phase.top.len())
            });
        anyhow::ensure!(
            intervals.is_some_and(|n| n <= MAX_PROFILE_INTERVALS),
            "command-profile aggregate interval limit exceeded"
        );
        for interval in &self.lifecycle {
            self.interval(interval)?;
        }
        if let Some(cp) = &self.critical_path {
            if let Some(backend) = &cp.backend {
                text_bound(backend)?;
            }
            if let Some(start) = cp.command_start_offset_us {
                self.offset(start)?;
            }
            anyhow::ensure!(
                cp.entries.len() <= MAX_ITEMS,
                "too many Buck critical-path entries"
            );
            for entry in &cp.entries {
                text_bound(&entry.kind)?;
                text_bound(&entry.label)?;
                anyhow::ensure!(
                    entry.span_ids.len() <= 256,
                    "too many span IDs in a Buck critical-path entry"
                );
                optional_duration(entry.start_offset_us)?;
                for duration in [
                    entry.duration_us,
                    entry.total_us,
                    entry.user_us,
                    entry.queue_us,
                    entry.noncritical_us,
                    entry.potential_improvement_us,
                ] {
                    optional_duration(duration)?;
                }
                if entry.reused != Some(true)
                    && let (Some(start), Some(total)) = (entry.start_offset_us, entry.total_us)
                {
                    let end = start
                        .checked_add(total)
                        .context("Buck critical-path end overflow")?;
                    if let Some(base) = cp.command_start_offset_us {
                        self.offset(
                            base.checked_add(end)
                                .context("Buck critical-path anchored end overflow")?,
                        )?;
                    }
                }
                if cp.command_start_offset_us.is_some() && entry.start_offset_us.is_some() {
                    self.offset(
                        cp.positioned_start_us(entry)
                            .context("Buck critical-path start overflow")?,
                    )?;
                }
            }
        }
        for phase in &self.phases {
            text_bound(&phase.kind)?;
            optional_duration(Some(phase.total_duration_us))?;
            anyhow::ensure!(
                phase.union_intervals.len() <= MAX_ITEMS && phase.top.len() <= 1024,
                "phase collection exceeds reader limit"
            );
            anyhow::ensure!(
                phase.retained_active_us().is_some(),
                "invalid phase union intervals"
            );
            for &[a, b] in &phase.union_intervals {
                self.offset(a)?;
                self.offset(b)?;
            }
            for interval in &phase.top {
                self.interval(interval)?;
            }
        }
        for sample in &self.snapshots {
            self.offset(sample.offset_us)?;
        }
        for window in &self.snapshot_windows {
            self.offset(window.start_offset_us)?;
            self.offset(window.end_offset_us)?;
            anyhow::ensure!(
                window.end_offset_us >= window.start_offset_us,
                "snapshot window end precedes start"
            );
            for text in &window.warnings {
                text_bound(text)?;
            }
            // Derived summaries must agree with source samples. This prevents
            // stale or hand-edited delta fields from bypassing reset checks.
            let mut expected = summarize_snapshots(
                &self.snapshots,
                window.start_offset_us,
                window.end_offset_us,
                self.dropped_snapshots,
            )?;
            // Wording is not evidence. Validate numeric and missingness fields,
            // including each counter's reset/coverage result, independent of prose.
            expected.warnings.clone_from(&window.warnings);
            expected
                .download
                .unavailable_reason
                .clone_from(&window.download.unavailable_reason);
            expected
                .upload
                .unavailable_reason
                .clone_from(&window.upload.unavailable_reason);
            expected
                .cpu
                .unavailable_reason
                .clone_from(&window.cpu.unavailable_reason);
            anyhow::ensure!(
                window == &expected,
                "snapshot summary disagrees with retained samples"
            );
        }
        for warning in self.warnings.iter().chain(&self.notes) {
            text_bound(warning)?;
        }
        Ok(())
    }

    pub fn from_value(value: &Value) -> anyhow::Result<Self> {
        // Count the serialized size without allocating a second JSON string.
        // serde_json's parsed Value has already passed its recursion limit.
        struct Budget(usize);
        impl std::io::Write for Budget {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > self.0 {
                    return Err(std::io::Error::other("command profile exceeds size limit"));
                }
                self.0 -= bytes.len();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(Budget(MAX_PROFILE_BYTES), value).context("command-profile size")?;
        // Check collection lengths before deserializing typed elements: a
        // small JSON array of empty objects must not expand into an unbounded
        // allocation of the substantially larger Rust entry types.
        fn list<'a>(value: &'a Value, key: &str, max: usize) -> anyhow::Result<&'a [Value]> {
            let values = value
                .get(key)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            anyhow::ensure!(
                values.len() <= max,
                "command-profile {key} exceeds reader limit"
            );
            Ok(values)
        }
        let mut intervals = list(value, "lifecycle", MAX_ITEMS)?.len();
        list(value, "snapshots", MAX_ITEMS)?;
        list(value, "warnings", 1024)?;
        list(value, "notes", 1024)?;
        for window in list(value, "snapshot_windows", 256)? {
            list(window, "warnings", 1024)?;
        }
        for phase in list(value, "phases", 256)? {
            let union = list(phase, "union_intervals", MAX_ITEMS)?.len();
            let top = list(phase, "top", 1024)?.len();
            intervals = intervals
                .checked_add(union)
                .and_then(|n| n.checked_add(top))
                .context("command-profile aggregate interval limit exceeded")?;
        }
        anyhow::ensure!(
            intervals <= MAX_PROFILE_INTERVALS,
            "command-profile aggregate interval limit exceeded"
        );
        if let Some(cp) = value.get("critical_path") {
            for entry in list(cp, "entries", MAX_ITEMS)? {
                list(entry, "span_ids", 256)?;
                list(entry, "acts", 256)?;
            }
        }
        let profile = Self::deserialize(value).context("decode command profile")?;
        profile.validate()?;
        Ok(profile)
    }
}

/// Decode one metadata `args` object, binding the profile to its recorded run.
/// Ordinary old metadata without `profile` remains a backward-compatible None.
pub fn decode_args(args: &Value) -> anyhow::Result<Option<CommandProfile>> {
    let Some(value) = args.get("profile") else {
        return Ok(None);
    };
    let run = args
        .get("run")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("command profile has no recorded run ID")?;
    text_bound(run)?;
    let mut profile = CommandProfile::from_value(value)?;
    anyhow::ensure!(
        profile.run.as_deref().is_none_or(|r| r == run),
        "command-profile run disagrees with metadata run"
    );
    profile.run = Some(run.to_owned());
    Ok(Some(profile))
}

/// Read profile metadata already selected by the caller's metadata category.
/// No implicit first/latest selection across duplicate or appended captures.
pub fn read_args<'a>(
    records: impl IntoIterator<Item = &'a Value>,
    run: Option<&str>,
) -> anyhow::Result<Option<CommandProfile>> {
    let mut found = None;
    for args in records {
        if args.get("profile").is_none() {
            continue;
        }
        if let Some(want) = run {
            if args
                .get("run")
                .and_then(Value::as_str)
                .is_some_and(|r| r != want)
            {
                continue;
            }
        }
        let profile = decode_args(args)?;
        anyhow::ensure!(
            found.is_none(),
            "multiple command profiles require an unambiguous capture/run"
        );
        found = profile;
    }
    Ok(found)
}

/// Daemon-wide endpoint deltas within a requested offset window. No endpoint
/// interpolation/extrapolation and no attribution to the phase/action naming
/// that window. Dropped samples invalidate monotonicity certification even if
/// the retained endpoints happen to increase.
pub fn summarize_snapshots(
    samples: &[SnapshotSample],
    start: i64,
    end: i64,
    dropped_samples: u64,
) -> anyhow::Result<SnapshotWindow> {
    anyhow::ensure!(
        end >= start && end.checked_sub(start).is_some(),
        "invalid snapshot window"
    );
    let selected: Vec<&SnapshotSample> = samples
        .iter()
        .filter(|s| s.offset_us >= start && s.offset_us <= end)
        .collect();
    let first = selected.first().map(|s| s.offset_us);
    let last = selected.last().map(|s| s.offset_us);
    let ordered = selected.windows(2).all(|w| w[1].offset_us > w[0].offset_us);
    let covered = first
        .zip(last)
        .and_then(|(a, b)| b.checked_sub(a))
        .filter(|&n| n >= 0 && ordered);
    let max_gap = if ordered {
        selected
            .windows(2)
            .filter_map(|w| w[1].offset_us.checked_sub(w[0].offset_us))
            .max()
    } else {
        None
    };
    let counter = |get: fn(&SnapshotSample) -> Option<u64>| {
        let mut prior = None;
        let mut resets = false;
        let mut missing = 0;
        for sample in &selected {
            match get(sample) {
                None => missing += 1,
                Some(value) => {
                    resets |= prior.is_some_and(|p| value < p);
                    prior = Some(value);
                }
            }
        }
        let first = selected.first().and_then(|s| get(s));
        let last = selected.last().and_then(|s| get(s));
        let reason = if selected.len() < 2 || covered.is_none_or(|n| n <= 0) {
            Some("need two strictly ordered samples spanning positive time")
        } else if dropped_samples != 0 {
            Some("source samples were dropped; resets across omitted samples are unverified")
        } else if missing != 0 {
            Some("counter is absent from one or more samples")
        } else if resets {
            Some("counter decreased inside the sampled window")
        } else {
            None
        };
        CounterWindow {
            first,
            last,
            delta: reason
                .is_none()
                .then(|| last?.checked_sub(first?))
                .flatten(),
            reset_observed: resets,
            missing_samples: missing,
            unavailable_reason: reason.map(str::to_owned),
        }
    };
    let mut warnings =
        vec!["Daemon-wide counters; not attributed to this phase or action.".to_owned()];
    if first != Some(start) || last != Some(end) {
        warnings.push(
            "Counter coverage is only the sampled endpoints; no extrapolation to window edges."
                .to_owned(),
        );
    }
    if !ordered {
        warnings.push(
            "Snapshot timestamps are not strictly increasing; no certified delta or rate."
                .to_owned(),
        );
    }
    if dropped_samples != 0 {
        warnings.push(
            "Dropped snapshots leave counter resets and intermediate coverage unverified."
                .to_owned(),
        );
    }
    Ok(SnapshotWindow {
        start_offset_us: start,
        end_offset_us: end,
        sample_count: selected.len() as u64,
        first_offset_us: first,
        last_offset_us: last,
        covered_us: covered,
        max_gap_us: max_gap,
        dropped_samples,
        download: counter(|s| s.download_bytes),
        upload: counter(|s| s.upload_bytes),
        cpu: counter(|s| s.cpu_us),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn numeric_evidence_loss_does_not_depend_on_warning_prose() {
        let complete = CommandProfile {
            notes: vec!["approximate daemon alignment; error bound unavailable".into()],
            critical_path: Some(BuckCriticalPath {
                approximate_alignment: true,
                entries: vec![CriticalPathEntry {
                    reused: Some(true),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            phases: vec![PhaseSummary {
                union_complete: true,
                dropped_examples: 10,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!complete.has_incomplete_evidence());
        let mut open = complete.clone();
        open.lifecycle_open_at_eof = 1;
        assert!(open.has_incomplete_evidence());
        let mut dropped = complete.clone();
        dropped.critical_path.as_mut().unwrap().dropped_entries = 1;
        assert!(dropped.has_incomplete_evidence());
        for phase in [
            PhaseSummary {
                union_complete: false,
                ..Default::default()
            },
            PhaseSummary {
                union_complete: true,
                open_at_eof: 1,
                ..Default::default()
            },
            PhaseSummary {
                union_complete: true,
                unmatched_ends: 1,
                ..Default::default()
            },
            PhaseSummary {
                union_complete: true,
                rejected_intervals: 1,
                ..Default::default()
            },
            PhaseSummary {
                union_complete: true,
                clipped_intervals: 1,
                ..Default::default()
            },
        ] {
            let mut profile = complete.clone();
            profile.phases = vec![phase];
            assert!(profile.has_incomplete_evidence());
            assert!(profile.warnings.is_empty());
        }
    }

    fn record(run: &str, profile: CommandProfile) -> Value {
        json!({"run":run,"profile":profile})
    }

    #[test]
    fn old_metadata_and_missing_optional_evidence_are_not_zero_measurements() {
        assert_eq!(read_args([&json!({"run":"r"})], Some("r")).unwrap(), None);
        let p = CommandProfile::from_value(&json!({"schema_version":1,"origin_us":100})).unwrap();
        assert_eq!(p.client_wall_us, None);
        assert_eq!(p.install_duration_us, None);
        assert_eq!(p.command_start_offset_us, None);
        assert_eq!(p.command_end_offset_us, None);
        assert_eq!(p.critical_path, None);
        assert!(p.lifecycle.is_empty());
    }

    #[test]
    fn materializer_queue_absence_and_scalar_zero_remain_distinct() {
        let absent: SnapshotSample = serde_json::from_value(json!({"offset_us": 0})).unwrap();
        assert_eq!(absent.deferred_materializer_queue_size, None);
        assert_eq!(materializer_queue_range([&absent]), None);
        assert!(
            serde_json::to_value(&absent)
                .unwrap()
                .get("deferred_materializer_queue_size")
                .is_none()
        );

        // Keep the reported zero, without claiming that an unversioned
        // producer distinguished an empty queue from its protobuf default.
        let zero: SnapshotSample = serde_json::from_value(json!({
            "offset_us": 1, "deferred_materializer_queue_size": 0
        }))
        .unwrap();
        assert_eq!(zero.deferred_materializer_queue_size, Some(0));
        assert_eq!(
            materializer_queue_range([&absent, &zero]),
            Some(MaterializerQueueRange {
                minimum: 0,
                maximum: 0,
                sample_count: 1,
            })
        );
        assert_eq!(
            serde_json::from_value::<SnapshotSample>(serde_json::to_value(&zero).unwrap()).unwrap(),
            zero
        );
    }

    #[test]
    fn materializer_queue_range_uses_only_retained_window_samples() {
        let profile = CommandProfile {
            dropped_snapshots: 10,
            snapshots: [(0, Some(900)), (10, Some(40)), (20, None), (30, Some(12))]
                .into_iter()
                .map(|(offset_us, value)| SnapshotSample {
                    offset_us,
                    deferred_materializer_queue_size: value,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        // Missing/dropped values do not become zero. The observed range is
        // still reportable, but cannot establish the continuous-time peak.
        assert_eq!(
            materializer_queue_range(
                profile
                    .snapshots
                    .iter()
                    .filter(|s| (10..=30).contains(&s.offset_us))
            ),
            Some(MaterializerQueueRange {
                minimum: 12,
                maximum: 40,
                sample_count: 2,
            })
        );
        assert_eq!(
            materializer_queue_range(&profile.snapshots)
                .unwrap()
                .maximum,
            900
        );
        assert_eq!(
            materializer_queue_range(profile.snapshots.iter().filter(|s| s.offset_us == 20)),
            None
        );
        // A single reported sample is valid gauge evidence, unlike a rate.
        assert_eq!(
            materializer_queue_range(profile.snapshots.iter().filter(|s| s.offset_us == 10))
                .unwrap()
                .sample_count,
            1
        );
    }

    #[test]
    fn profile_reader_binds_exact_run_and_refuses_duplicates_and_mismatch() {
        let a = record("a", CommandProfile::default());
        let b = record("b", CommandProfile::default());
        assert_eq!(
            read_args([&a, &b], Some("b"))
                .unwrap()
                .unwrap()
                .run
                .as_deref(),
            Some("b")
        );
        assert!(read_args([&a, &b], None).is_err());
        assert!(read_args([&a, &a], Some("a")).is_err());
        assert!(read_args([&a], Some("missing")).unwrap().is_none());
        let bad = record(
            "a",
            CommandProfile {
                run: Some("b".into()),
                ..Default::default()
            },
        );
        assert!(decode_args(&bad).is_err());
        assert!(decode_args(&json!({"profile":CommandProfile::default()})).is_err());
    }

    #[test]
    fn recorded_offsets_allow_overlap_and_zero_without_fabricating_client_bounds() {
        let p = CommandProfile {
            origin_us: 100,
            client_wall_us: Some(500),
            install_duration_us: Some(200),
            lifecycle: vec![
                ObservedInterval {
                    kind: "remote".into(),
                    start_offset_us: -10,
                    duration_us: 20,
                    cancelled: true,
                    ..Default::default()
                },
                ObservedInterval {
                    kind: "local".into(),
                    start_offset_us: 0,
                    duration_us: 20,
                    ..Default::default()
                },
                ObservedInterval {
                    kind: "instant".into(),
                    start_offset_us: 5,
                    duration_us: 0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        p.validate().unwrap();
        let again = CommandProfile::from_value(&serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(again, p);
        assert_eq!(again.command_end_offset_us, None);
        assert_eq!(again.lifecycle[2].end_offset_us(), Some(5));
    }

    #[test]
    fn invalid_profile_times_schema_and_oversize_text_are_refused() {
        for value in [
            json!({"schema_version":2,"origin_us":0}),
            json!({"schema_version":1,"origin_us":-1}),
            json!({"schema_version":1,"origin_us":i64::MAX,"command_end_offset_us":1}),
            json!({"schema_version":1,"origin_us":0,"command_start_offset_us":20,"command_end_offset_us":10}),
            json!({"schema_version":1,"origin_us":0,"client_wall_us":-1}),
            json!({"schema_version":1,"origin_us":0,"install_duration_us":-1}),
            json!({"schema_version":1,"origin_us":0,"lifecycle":[{"kind":"x","start_offset_us":0,"duration_us":-1}]}),
            json!({"schema_version":1,"origin_us":0,"warnings":["x".repeat(MAX_TEXT_BYTES+1)]}),
            json!({"schema_version":1,"origin_us":0,"unknown":"x".repeat(MAX_PROFILE_BYTES)}),
            json!({"schema_version":1,"origin_us":0,"lifecycle":vec![json!({});MAX_ITEMS+1]}),
            json!({"schema_version":1,"origin_us":0,"critical_path":{"entries":[{"kind":"x","label":"x","span_ids":vec![0;257]}]}}),
        ] {
            assert!(CommandProfile::from_value(&value).is_err());
        }
    }

    #[test]
    fn cp_model_offsets_require_their_own_command_origin() {
        let entry = CriticalPathEntry {
            kind: "analysis".into(),
            label: "a".into(),
            start_offset_us: Some(30),
            duration_us: Some(0),
            reused: Some(true),
            ..Default::default()
        };
        let mut cp = BuckCriticalPath {
            entries: vec![entry.clone()],
            ..Default::default()
        };
        assert_eq!(cp.positioned_start_us(&entry), None);
        assert_eq!(cp.model_duration_us(), Some(0));
        cp.command_start_offset_us = Some(20);
        assert_eq!(cp.positioned_start_us(&entry), Some(50));
        cp.entries[0].duration_us = None;
        assert_eq!(cp.model_duration_us(), None);
        cp.entries[0].duration_us = Some(10);
        cp.dropped_entries = 1;
        assert_eq!(cp.model_duration_us(), None);
    }

    #[test]
    fn cp_end_overflow_is_invalid_but_reused_total_is_not_a_positioned_span() {
        let mut profile = CommandProfile {
            critical_path: Some(BuckCriticalPath {
                entries: vec![CriticalPathEntry {
                    kind: "analysis".into(),
                    label: "a".into(),
                    start_offset_us: Some(i64::MAX),
                    total_us: Some(1),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(profile.validate().is_err());
        profile.critical_path.as_mut().unwrap().entries[0].reused = Some(true);
        profile.validate().unwrap();
        let path = profile.critical_path.as_mut().unwrap();
        path.entries[0].reused = Some(false);
        path.entries[0].start_offset_us = Some(1);
        path.command_start_offset_us = Some(i64::MAX - 1);
        assert!(
            profile.validate().is_err(),
            "anchored end must also fit the profile clock"
        );
    }

    #[test]
    fn aggregate_interval_budget_is_checked_before_typed_allocation() {
        let phase = serde_json::json!({"kind":"x","union_intervals":vec![[0,0];MAX_ITEMS]});
        let value = serde_json::json!({"schema_version":1,"origin_us":0,"phases":vec![phase;5]});
        let error = CommandProfile::from_value(&value).unwrap_err();
        assert!(error.to_string().contains("aggregate interval limit"));
    }

    #[test]
    fn phase_union_is_not_span_sum_and_partial_union_is_only_a_lower_bound() {
        let mut phase = PhaseSummary {
            kind: "load".into(),
            count: 2,
            completed: 2,
            total_duration_us: 30,
            union_intervals: vec![[0, 20]],
            union_complete: true,
            ..Default::default()
        };
        assert_eq!(phase.active_us(), Some(20));
        assert_eq!(phase.total_duration_us, 30);
        phase.union_complete = false;
        assert_eq!(phase.active_us(), None);
        assert_eq!(phase.retained_active_us(), Some(20));
        phase.union_intervals = vec![[0, 20], [10, 30]];
        assert_eq!(phase.retained_active_us(), None);
        let p = CommandProfile {
            phases: vec![phase],
            ..Default::default()
        };
        assert!(p.validate().is_err());
    }

    fn samples() -> Vec<SnapshotSample> {
        vec![
            SnapshotSample {
                offset_us: 10,
                download_bytes: Some(100),
                upload_bytes: Some(0),
                cpu_us: Some(0),
                ..Default::default()
            },
            SnapshotSample {
                offset_us: 20,
                download_bytes: Some(200),
                upload_bytes: Some(0),
                cpu_us: Some(20),
                ..Default::default()
            },
            SnapshotSample {
                offset_us: 30,
                download_bytes: Some(300),
                upload_bytes: Some(0),
                cpu_us: Some(40),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn snapshot_deltas_use_covered_endpoints_not_full_window_or_counter_totals() {
        let w = summarize_snapshots(&samples(), 0, 40, 0).unwrap();
        assert_eq!(w.covered_us, Some(20));
        assert_eq!(w.coverage_fraction(), Some(0.5));
        assert_eq!(w.download.delta, Some(200));
        assert_eq!(w.upload.delta, Some(0));
        assert_eq!(w.rate_per_second(&w.download), Some(10_000_000.0));
        assert_eq!(w.rate_per_second(&w.cpu), Some(2_000_000.0));
        assert_eq!(w.cpu_cores(), Some(2.0));
        assert_eq!(w.max_gap_us, Some(10));
    }

    #[test]
    fn resets_missing_samples_and_dropped_middle_refuse_certified_deltas() {
        let mut s = samples();
        s[1].download_bytes = Some(0); // endpoint still greater than first
        let w = summarize_snapshots(&s, 0, 40, 0).unwrap();
        assert_eq!(w.download.delta, None);
        assert!(w.download.reset_observed);
        assert_eq!(w.upload.delta, Some(0));
        s[1].download_bytes = None;
        let w = summarize_snapshots(&s, 0, 40, 0).unwrap();
        assert_eq!(w.download.delta, None);
        assert_eq!(w.download.missing_samples, 1);
        let w = summarize_snapshots(&samples(), 0, 40, 1).unwrap();
        assert_eq!(w.download.delta, None);
        assert_eq!(w.upload.delta, None);
        assert_eq!(w.rate_per_second(&w.cpu), None);
    }

    #[test]
    fn unordered_duplicate_or_insufficient_snapshots_never_produce_a_rate() {
        let mut s = samples();
        s.swap(0, 1);
        assert_eq!(
            summarize_snapshots(&s, 0, 40, 0).unwrap().download.delta,
            None
        );
        s[0].offset_us = s[1].offset_us;
        assert_eq!(
            summarize_snapshots(&s, 0, 40, 0).unwrap().download.delta,
            None
        );
        for slice in [&s[..0], &s[..1]] {
            let w = summarize_snapshots(slice, 0, 40, 0).unwrap();
            assert_eq!(w.rate_per_second(&w.download), None);
        }
        assert!(summarize_snapshots(&s, 40, 0, 0).is_err());
    }

    #[test]
    fn serialized_snapshot_summaries_cannot_bypass_reset_validation() {
        let snapshots = samples();
        let window = summarize_snapshots(&snapshots, 0, 40, 0).unwrap();
        let mut profile = CommandProfile {
            snapshots,
            snapshot_windows: vec![window],
            ..Default::default()
        };
        let value = serde_json::to_value(&profile).unwrap();
        assert_eq!(CommandProfile::from_value(&value).unwrap(), profile);
        profile.snapshot_windows[0]
            .warnings
            .push("Older producer wording.".into());
        profile.snapshot_windows[0].download.unavailable_reason =
            Some("Different explanation".into());
        profile.validate().unwrap();
        profile.snapshot_windows[0].download.delta = Some(999);
        assert!(CommandProfile::from_value(&serde_json::to_value(profile).unwrap()).is_err());
    }
}
