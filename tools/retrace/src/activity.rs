/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Positioned client-clock observations, not a partition of worker service.
//! Construct once when an action opens. Duration-only counters never create
//! intervals; overlapping observations contribute only their union to coverage.

use serde_json::Value;

/// Bound work and retained labels when opening an untrusted trace record.
pub const MAX_ACTIVITY_SPANS: usize = 16_384;
const MAX_NAME_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    Current,
    LegacyLimited,
    Missing,
    Unsupported,
    Invalid,
}

impl Evidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "recorded client activity",
            Self::LegacyLimited => "limited legacy observations",
            Self::Missing => "activity intervals not recorded",
            Self::Unsupported => "unsupported activity version",
            Self::Invalid => "incomplete or invalid activity evidence",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    Client,
    Remote,
    Local,
}

impl Lane {
    pub fn label(self) -> &'static str {
        match self {
            Self::Client => "Client operations",
            Self::Remote => "Remote states observed by client",
            Self::Local => "Local operations",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub start_us: i64,
    pub end_us: i64,
}

impl Interval {
    pub fn duration_us(self) -> i64 {
        self.end_us - self.start_us
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivitySpan {
    pub name: String,
    pub label: String,
    pub lane: Lane,
    pub start_us: i64,
    pub end_us: i64,
    pub cancelled: bool,
}

impl ActivitySpan {
    pub fn duration_us(&self) -> i64 {
        self.end_us - self.start_us
    }

    /// An observed end is not evidence that a command succeeded.
    pub fn status_label(&self) -> &'static str {
        if self.cancelled { "canceled" } else { "ended" }
    }
}

#[derive(Clone, Debug)]
pub struct Activity {
    pub evidence: Evidence,
    pub duration_us: Option<i64>,
    pub spans: Vec<ActivitySpan>,
    pub covered: Vec<Interval>,
    /// Complement of the validated v1 observations, never inferred for old,
    /// unsupported, or rejected input. These are not necessarily idle periods.
    pub gaps: Vec<Interval>,
    /// For limited evidence this is only the union of the usable observations.
    pub covered_us: Option<i64>,
    pub uncovered_us: Option<i64>,
    pub rejected_spans: usize,
    pub clipped_spans: usize,
    pub explanations: Vec<String>,
    pub diagnosis: String,
    /// Current-record summary facts, not additional interval coverage.
    pub highlights: Vec<String>,
}

impl Activity {
    pub fn from_record(rec: &Value) -> Self {
        let duration_us = rec.get("dur").and_then(Value::as_i64).filter(|d| *d >= 0);
        let mut out = Self {
            evidence: Evidence::Missing,
            duration_us,
            spans: Vec::new(),
            covered: Vec::new(),
            gaps: Vec::new(),
            covered_us: None,
            uncovered_us: None,
            rejected_spans: 0,
            clipped_spans: 0,
            explanations: Vec::new(),
            diagnosis: String::new(),
            highlights: Vec::new(),
        };
        let args = &rec["args"];
        if let Some(activity) = args.get("activity") {
            match activity.get("version").and_then(Value::as_u64) {
                Some(1) => {
                    out.evidence = Evidence::Current;
                    if let Some(rejected) = activity.get("rejected") {
                        match rejected.as_u64().and_then(|n| usize::try_from(n).ok()) {
                            Some(n) => out.rejected_spans = n,
                            None => out.reject(),
                        }
                    }
                    if let Some(spans) = activity.get("spans").and_then(Value::as_array) {
                        out.read_spans(spans, None);
                    } else {
                        out.reject();
                    }
                    if out.rejected_spans > 0 {
                        out.evidence = Evidence::Invalid;
                    }
                }
                Some(_) => out.evidence = Evidence::Unsupported,
                None => out.evidence = Evidence::Invalid,
            }
        } else if let Some(spans) = args.get("re").and_then(|re| re.get("cancelled_stages")) {
            out.evidence = Evidence::LegacyLimited;
            match (spans.as_array(), rec.get("ts").and_then(Value::as_i64)) {
                (Some(spans), Some(ts)) => out.read_spans(spans, Some(ts)),
                _ => out.reject(),
            }
            if out.rejected_spans > 0 {
                out.evidence = Evidence::Invalid;
            }
        }
        if duration_us.is_none() {
            out.evidence = Evidence::Invalid;
            out.explanations.push(
                "Action duration is missing or invalid; no time positions can be established."
                    .to_owned(),
            );
        }
        out.spans.sort_by(|a, b| {
            (a.start_us, a.end_us, a.lane, &a.name, a.cancelled).cmp(&(
                b.start_us,
                b.end_us,
                b.lane,
                &b.name,
                b.cancelled,
            ))
        });
        out.covered = union(out.spans.iter().map(|s| Interval {
            start_us: s.start_us,
            end_us: s.end_us,
        }));
        if out.has_complete_evidence() || !out.spans.is_empty() {
            out.covered_us = Some(total(&out.covered));
        }
        if out.has_complete_evidence() {
            let duration = out.duration_us.unwrap();
            let mut at = 0;
            for range in &out.covered {
                if at < range.start_us {
                    out.gaps.push(Interval {
                        start_us: at,
                        end_us: range.start_us,
                    });
                }
                at = range.end_us;
            }
            if at < duration {
                out.gaps.push(Interval {
                    start_us: at,
                    end_us: duration,
                });
            }
            out.uncovered_us = Some(total(&out.gaps));
        }
        out.explain(rec);
        out
    }

    /// Complete parsing of the recorded interval list, NOT a claim that the
    /// instrumentation observed all work or that the build capture is complete.
    pub fn has_complete_evidence(&self) -> bool {
        self.evidence == Evidence::Current && self.duration_us.is_some() && self.rejected_spans == 0
    }

    fn reject(&mut self) {
        self.rejected_spans = self.rejected_spans.saturating_add(1);
    }

    fn read_spans(&mut self, spans: &[Value], legacy_start: Option<i64>) {
        self.rejected_spans = self
            .rejected_spans
            .saturating_add(spans.len().saturating_sub(MAX_ACTIVITY_SPANS));
        for row in spans.iter().take(MAX_ACTIVITY_SPANS) {
            let parsed = if let Some(action_start) = legacy_start {
                (|| {
                    Some((
                        row.get("name")?.as_str()?,
                        row.get("ts")?.as_i64()?.checked_sub(action_start)?,
                        row.get("dur")?.as_i64()?,
                        true,
                    ))
                })()
            } else {
                (|| {
                    let fields = row.as_array()?;
                    if fields.len() != 4 {
                        return None;
                    }
                    Some((
                        fields[0].as_str()?,
                        fields[1].as_i64()?,
                        fields[2].as_i64()?,
                        fields[3].as_bool()?,
                    ))
                })()
            };
            let Some((name, start, duration, cancelled)) = parsed else {
                self.reject();
                continue;
            };
            if name.is_empty()
                || name.len() > MAX_NAME_BYTES
                || name.chars().any(char::is_control)
                || duration < 0
            {
                self.reject();
                continue;
            }
            let (Some(end), Some(action_duration)) =
                (start.checked_add(duration), self.duration_us)
            else {
                self.reject();
                continue;
            };
            let start_us = start.clamp(0, action_duration);
            let end_us = end.clamp(0, action_duration);
            // Preserve observed zero-duration events inside the action, but do
            // not turn a wholly outside interval into a fake boundary marker.
            if end < 0 || start > action_duration || (duration > 0 && start_us == end_us) {
                self.reject();
                continue;
            }
            if start_us != start || end_us != end {
                self.clipped_spans += 1;
            }
            // Re.* also contains client scaffolding and output materialization.
            // Only known remote states belong on the remote-observation lane;
            // unknown names retain their literal identity on the client lane.
            let lane = if matches!(
                name,
                "Re.Queue" | "Re.Execute" | "Re.WorkerDownload" | "Re.WorkerUpload"
            ) {
                Lane::Remote
            } else if name.starts_with("Local.") {
                Lane::Local
            } else {
                Lane::Client
            };
            let label = match name {
                "Re.WorkerDownload" => "remote input preparation state",
                "Re.Queue" => "remote queue state",
                "Re.Execute" => "remote execution state",
                "Re.WorkerUpload" => "remote output preparation state",
                "Re.Download" => "client output materialization",
                "Local.MaterializeInputs" => "local input preparation",
                "Local.PrepareOutputs" | "Local.PrepareOutputDirs" => {
                    "local output-directory preparation"
                }
                "Local.FinalizeOutputs" => "local output finalization",
                "CasArtifact.CheckExpiration" => "CAS artifact expiry check",
                "CasArtifact.ExtendExpiration" => "CAS artifact expiry extension",
                "CasArtifact.RecheckExpiration" => "CAS artifact expiry recheck",
                "CasArtifact.TreeMetadata" => "CAS artifact tree metadata",
                "CasArtifact.DeclareOutputs" => "CAS artifact output declaration",
                "Local.Execute" | "Local.Execute.command" => "local execution",
                "Local.Queued" => "local queue",
                "CacheQuery" => "cache lookup",
                "LocalCache.CheckIdenticalAction" => "local identical-action cache check",
                "LocalCache.CheckFilteredInputs" => "local filtered-input cache check",
                "RemoteCache.CheckIdenticalAction" => "remote cached-action validation",
                "RemoteCache.CheckFilteredInputs" => "remote cached filtered-input validation",
                "CacheHit" => "cache-hit result handling",
                "Prepare" => "action request preparation",
                "ReUpload" => "client input upload",
                "ReSubmit" => "client remote submission",
                other => other,
            };
            self.spans.push(ActivitySpan {
                name: name.to_owned(),
                label: label.to_owned(),
                lane,
                start_us,
                end_us,
                cancelled,
            });
        }
    }

    fn explain(&mut self, rec: &Value) {
        self.explanations.push("Intervals are observed on the client clock. Remote labels describe client-observed states, not physical worker service; lanes and intervals may overlap. Coverage uses their union, never their sum.".to_owned());
        self.explanations.push("An ended state does not establish command success. A canceled state does not identify its cause. State intervals do not identify worker attempts; historical worker timings are not positioned here.".to_owned());
        if [("CacheHit", "cache_hit_us"), ("Prepare", "prepare_us")]
            .iter()
            .any(|(name, key)| {
                self.spans.iter().any(|s| s.name == *name)
                    && rec["args"]["stages"].get(*key).is_none()
            })
        {
            self.explanations.push("This older conversion retains cached-result or request-preparation intervals but omits their named counters. Reopen the original event log to classify them in Stage totals; the existing counters are unchanged.".to_owned());
        }
        self.explanations.push(match self.evidence {
            Evidence::Current if self.spans.is_empty() => "This version records no activity spans for the action. The uncovered action window is absence of observations, not evidence that nothing happened.",
            Evidence::Current => "Uncovered gaps are the complement of the recorded client intervals, not measured idleness. The stage-counter residual is a separate arithmetic quantity and need not equal these gaps.",
            Evidence::LegacyLimited => "Only explicitly positioned canceled intervals survive in this older record. They are limited evidence; a complete coverage percentage or uncovered timeline cannot be reconstructed.",
            Evidence::Missing => "This older record has no positioned activity evidence. Stage durations and worker metadata cannot reconstruct a client timeline; re-convert the original event log when available.",
            Evidence::Unsupported => "The activity schema version is unsupported. No intervals or uncovered gaps were inferred; use a compatible Retrace version.",
            Evidence::Invalid => "Some activity evidence is missing, invalid, or rejected. Usable intervals may be shown, but full coverage and uncovered gaps cannot be established.",
        }.to_owned());
        if self.rejected_spans > 0 {
            self.explanations.push(format!(
                "{} interval(s) were rejected by the producer or reader.",
                self.rejected_spans
            ));
        }
        if self.clipped_spans > 0 {
            self.explanations.push(format!(
                "{} interval(s) were clipped to the action boundaries.",
                self.clipped_spans
            ));
        }
        let downloads: Vec<_> = self
            .spans
            .iter()
            .filter(|s| s.name == "Re.WorkerDownload")
            .collect();
        let download_ranges = union(downloads.iter().map(|s| Interval {
            start_us: s.start_us,
            end_us: s.end_us,
        }));
        let download_us = total(&download_ranges);
        let canceled = downloads.iter().any(|s| s.cancelled);
        let fetch = current_input_preparation_us(rec);
        let upload_admission = self.upload_admission(rec);
        let dominates = |us: i64| us > 0 && self.duration_us.is_some_and(|d| us >= d / 2);
        let wall = self
            .duration_us
            .map(seconds)
            .unwrap_or_else(|| "unknown duration".to_owned());
        let local_ranges = union(
            self.spans
                .iter()
                .filter(|s| s.lane == Lane::Local)
                .map(|s| Interval {
                    start_us: s.start_us,
                    end_us: s.end_us,
                }),
        );
        let canceled_ranges = union(downloads.iter().filter(|s| s.cancelled).map(|s| Interval {
            start_us: s.start_us,
            end_us: s.end_us,
        }));
        let local_overlap = intersection_us(&canceled_ranges, &local_ranges);
        self.diagnosis = if canceled {
            format!(
                "A remote input preparation state was canceled: {} observed across the {wall} action. {} Cancellation cause and the build-delay effect are not established by this record.",
                seconds(total(&canceled_ranges)),
                if local_overlap > 0 {
                    format!(
                        "Its recorded overlap with local activity is {}.",
                        seconds(local_overlap)
                    )
                } else {
                    "No overlap with local activity is recorded in the available intervals."
                        .to_owned()
                }
            )
        } else if dominates(download_us) {
            format!(
                "Most of the action window is covered by the remote input preparation state: {} of {wall}. This locates the client-observed wait; its underlying cause is not recorded here.",
                seconds(download_us)
            )
        } else if fetch.is_some_and(dominates) {
            format!(
                "The reported input preparation duration dominates this action: {} of {wall}. Logical input bytes divided by this time do not measure network bandwidth.",
                seconds(fetch.unwrap())
            )
        } else if let Some((wait, upload)) = upload_admission.filter(|(wait, _)| dominates(*wait)) {
            format!(
                "Recorded client upload admission dominates this action: {} waiting for Buck's upload permit, within the {} input-upload stage and {wall} action. The remaining {} of that stage includes checks, preparation and any storage calls; it is not pure transfer time.",
                seconds(wait),
                seconds(upload),
                seconds(upload - wait)
            )
        } else if self.uncovered_us.is_some_and(dominates) {
            format!(
                "{} of the {wall} action is outside the recorded activity intervals. These gaps are missing observations, not measured idleness or daemon CPU work.",
                seconds(self.uncovered_us.unwrap())
            )
        } else if self.has_complete_evidence() {
            match self.spans.iter().max_by_key(|s| s.duration_us()) {
                Some(span) => format!(
                    "Longest observed interval: {} for {} within the {wall} action. Its duration alone does not establish the underlying cause.",
                    span.label,
                    seconds(span.duration_us())
                ),
                None => format!(
                    "No activity intervals were recorded for the {wall} action; this is not evidence that nothing happened."
                ),
            }
        } else {
            "Positioned evidence is insufficient to reconstruct the whole action. The stage-counter residual must not be interpreted as an idle period or daemon CPU work.".to_owned()
        };
        if let Some(fetch) = fetch {
            self.highlights.push(format!(
                "Reported current input preparation: {} (counter, not a positioned interval)",
                seconds(fetch)
            ));
            if download_us > 0 && fetch != download_us {
                self.highlights.push(format!("Observed remote input-state union: {}; the reported input preparation counter is separate, not additive", seconds(download_us)));
            }
        }
        if let Some(bytes) = rec["args"]
            .get("bytes_in")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n >= 0.0)
        {
            self.highlights.push(format!(
                "Logical input size: {} (may be cache-resident)",
                crate::analyses::fmt_bytes_f(bytes)
            ));
            if let Some(fetch) = fetch.filter(|us| *us >= 1000) {
                let rate = bytes / (fetch as f64 / 1e6);
                if rate.is_finite() {
                    self.highlights.push(format!(
                        "Effective input preparation rate: {}/s (not network bandwidth)",
                        crate::analyses::fmt_bytes_f(rate)
                    ));
                }
            }
        }
        let args = &rec["args"];
        if let Some(us) = crate::analyses::current_execution_us(
            args["stages"].get("execute_us").and_then(Value::as_i64),
            args.get("cache").is_some_and(Value::is_object),
            args.get("execution_metadata").and_then(Value::as_str),
            args["stages"].get("execute_source").and_then(Value::as_str),
        )
        .filter(|us| *us >= 0)
        {
            self.highlights.push(format!(
                "Recorded current execution: {} (counter)",
                seconds(us)
            ));
        }
        if downloads.len() > 1 {
            self.explanations.push(format!("{} remote input preparation intervals were observed. Multiple intervals do not by themselves identify distinct worker attempts or a retry cause.", downloads.len()));
        }
        if dominates(download_us) || fetch.is_some_and(dominates) || canceled {
            self.explanations.push(CAS_FOLLOW_UP.to_owned());
        }
    }

    /// An aggregate counter is safely attributable to this parent only for one
    /// complete, unclipped upload interval. Do not position the counter or infer
    /// the same split for overlapping attempts, canceled work, or old captures.
    fn upload_admission(&self, rec: &Value) -> Option<(i64, i64)> {
        if !self.has_complete_evidence() || self.clipped_spans != 0 {
            return None;
        }
        let mut uploads = self.spans.iter().filter(|s| s.name == "ReUpload");
        let upload = uploads.next()?;
        if upload.cancelled || uploads.next().is_some() {
            return None;
        }
        let args = &rec["args"];
        if args["re"].get("invalid_counter_fields").is_some_and(|v| {
            v.as_array()
                .is_none_or(|fields| fields.iter().any(|f| !f.is_string()))
        }) {
            return None;
        }
        let parent = args["stages"]["input_upload_us"].as_i64()?;
        let wait = crate::detail::re_i64(args, "iu_sem_wait_us")?;
        (parent == upload.duration_us() && wait <= parent).then_some((wait, parent))
    }

    /// Copyable evidence, not executable shell/SQL. JSON-quoted identifiers
    /// preserve line boundaries even when the trace contains hostile strings.
    pub fn diagnostics_text(&self, rec: &Value, re_session: Option<&str>) -> String {
        let args = &rec["args"];
        let mut out = format!(
            "Retrace action activity diagnostics\nEvidence: {}\n",
            self.evidence.label()
        );
        for (label, value) in [
            ("Build/run ID", &args["run"]),
            ("Action digest", &args["digest"]),
            ("Configuration", &args["cfg"]),
            ("Action span ID", &args["span_id"]),
            ("Target", &args["target"]),
            ("Executor", &args["kind"]),
            ("Execution metadata provenance", &args["execution_metadata"]),
        ] {
            out.push_str(&format!("{label}: {}\n", quoted(value)));
        }
        let session = re_session
            .or_else(|| args.get("re_session").and_then(Value::as_str))
            .or_else(|| args.get("re_session_id").and_then(Value::as_str));
        out.push_str(&format!(
            "RE session ID: {}\n",
            session.map_or_else(
                || "not recorded".to_owned(),
                |s| quoted(&Value::String(s.to_owned()))
            )
        ));
        out.push_str(&format!(
            "Cache-result metadata present: {}\n",
            args.get("cache").is_some_and(Value::is_object)
        ));
        match (rec.get("ts").and_then(Value::as_i64), self.duration_us) {
            (Some(start), Some(duration)) => {
                if let Some(end) = start.checked_add(duration) {
                    out.push_str(&format!("Client-clock epoch window (us): [{start}, {end}]\nAction duration (us): {duration}\n"));
                } else {
                    out.push_str("Client-clock epoch window: invalid (overflow)\n");
                }
            }
            _ => out.push_str("Client-clock epoch window: not established\n"),
        }
        if let Some(us) = self.covered_us {
            out.push_str(&format!(
                "Observed interval union (us): {us}{}\n",
                if self.has_complete_evidence() {
                    ""
                } else {
                    " (limited evidence)"
                }
            ));
        }
        if let Some(us) = self.uncovered_us {
            out.push_str(&format!("Uncovered by recorded intervals (us): {us}\n"));
        }
        if let (Some(duration), Some(stages)) = (self.duration_us, crate::detail::stages_of(args)) {
            let counters = stages.iter().fold(0i64, |sum, &v| sum.saturating_add(v));
            out.push_str(&format!(
                "Separate positive stage-counter residual (us): {} (not a positioned interval)\n",
                duration.saturating_sub(counters).max(0)
            ));
        }
        out.push_str(&format!("\nDiagnosis: {}\n", self.diagnosis));
        for highlight in &self.highlights {
            out.push_str(&format!("{highlight}\n"));
        }
        for explanation in &self.explanations {
            out.push_str(&format!("{explanation}\n"));
        }
        out.push_str("\nRecorded activity: offsets from action start; all durations in us.\n");
        for span in &self.spans {
            out.push_str(&format!(
                "{} | {} | {} | start={} end={} duration={} | {}\n",
                span.lane.label(),
                quoted(&Value::String(span.name.clone())),
                span.label,
                span.start_us,
                span.end_us,
                span.duration_us(),
                span.status_label()
            ));
        }
        if self.spans.is_empty() {
            out.push_str("No usable positioned intervals.\n");
        }
        for gap in &self.gaps {
            out.push_str(&format!(
                "Uncovered gap | start={} end={} duration={}\n",
                gap.start_us,
                gap.end_us,
                gap.duration_us()
            ));
        }
        out.push_str("\nBounded server follow-up (instructions only; no query has been run):\nUse the exact action digest and RE session above in remote_execution_action, restricted to the recorded epoch window with a small clock/recording margin and a row limit. Verify configuration, execution start, cache provenance and worker execution_id; do not equate a historical cache result with this window. Join only those execution_id values to remote_execution_cas_daemon (event download_directory), checking session and host. Compare input_fetch_ms with duration, materialize_duration_ms, blob_download_duration_ms, dedup_wait_duration_ms and deferred_symlinks_duration_ms. Retain nulls, deployed worker/CAS versions and failed records. Logical input/cache-miss bytes are not network-wire bytes; phase times do not by themselves prove queueing. If dedup handling dominates, request the matching owner-session/digest and retry evidence; these are not established by the elapsed counter. Do not broaden to unrelated sessions.\n");
        out
    }
}

const CAS_FOLLOW_UP: &str = "For remote input preparation, inspect exact worker-execution CAS diagnostics for mixed directory/cache setup, batch-download, and shared-download completion/retry phases. Tree retrieval and filesystem work can overlap inside these phases; they are not isolated network, queue, or disk durations. These phase counters are not available in this client timeline; the cause remains unverified.";

fn current_input_preparation_us(rec: &Value) -> Option<i64> {
    let args = &rec["args"];
    if args.get("cache").is_some_and(Value::is_object)
        || args
            .get("execution_metadata")
            .and_then(Value::as_str)
            .is_some_and(|s| s == "reused" || s.starts_with("historical"))
    {
        return None;
    }
    args.get("stages")?
        .get("materialize_us")?
        .as_i64()
        .filter(|us| *us >= 0)
}

fn union(ranges: impl IntoIterator<Item = Interval>) -> Vec<Interval> {
    let mut ranges: Vec<_> = ranges
        .into_iter()
        .filter(|r| r.end_us > r.start_us)
        .collect();
    ranges.sort_by_key(|r| (r.start_us, r.end_us));
    let mut merged: Vec<Interval> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start_us <= last.end_us {
                last.end_us = last.end_us.max(range.end_us);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

fn total(ranges: &[Interval]) -> i64 {
    ranges
        .iter()
        .fold(0i64, |sum, r| sum.saturating_add(r.duration_us()))
}

fn quoted(value: &Value) -> String {
    match value {
        Value::String(s) if s.len() > 4096 => format!(
            "{} (truncated)",
            serde_json::to_string(&s.chars().take(4096).collect::<String>()).unwrap()
        ),
        Value::Null | Value::Array(_) | Value::Object(_) => "not recorded".to_owned(),
        _ => value.to_string(),
    }
}

fn seconds(us: i64) -> String {
    format!("{:.2}s", us as f64 / 1e6)
}

fn intersection_us(a: &[Interval], b: &[Interval]) -> i64 {
    let (mut i, mut j, mut sum) = (0, 0, 0i64);
    while i < a.len() && j < b.len() {
        let start = a[i].start_us.max(b[j].start_us);
        let end = a[i].end_us.min(b[j].end_us);
        if end > start {
            sum = sum.saturating_add(end - start);
        }
        if a[i].end_us <= b[j].end_us {
            i += 1;
        } else {
            j += 1;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record(duration: i64, spans: Value) -> Value {
        json!({"ts": 1_000_000, "dur": duration, "args": {"activity": {"version": 1, "spans": spans}}})
    }

    #[test]
    fn local_and_remote_cache_checks_remain_named_client_observations() {
        let rec = record(
            100,
            json!([
                ["LocalCache.CheckIdenticalAction", 0, 70, false],
                ["LocalCache.CheckFilteredInputs", 10, 10, true],
                ["RemoteCache.CheckFilteredInputs", 70, 30, false],
            ]),
        );
        let activity = Activity::from_record(&rec);
        assert_eq!(activity.covered_us, Some(100));
        assert_eq!(activity.uncovered_us, Some(0));
        assert!(activity.spans.iter().all(|s| s.lane == Lane::Client));
        assert!(
            activity
                .spans
                .iter()
                .any(|s| s.label == "local identical-action cache check")
        );
        assert!(
            activity
                .spans
                .iter()
                .any(|s| s.label == "remote cached filtered-input validation")
        );
        assert!(activity.spans.iter().any(|s| s.cancelled));
    }

    #[test]
    fn upload_admission_diagnosis_uses_measured_wait_without_changing_coverage() {
        // A measured action from a large real capture. Only 3,815 bytes were
        // uploaded; the long parent is mostly permit waiting.
        let mut rec = record(88_098_471, json!([["ReUpload", 100, 87_291_307, false]]));
        rec["args"]["stages"] = json!({"input_upload_us": 87_291_307});
        let without_counter = Activity::from_record(&rec);
        rec["args"]["re"] = json!({"iu_sem_wait_us": 86_433_080});
        let measured = Activity::from_record(&rec);
        assert!(
            measured
                .diagnosis
                .contains("86.43s waiting for Buck's upload permit")
        );
        assert!(measured.diagnosis.contains("87.29s input-upload stage"));
        assert!(measured.diagnosis.contains("remaining 0.86s"));
        assert!(measured.diagnosis.contains("not pure transfer time"));
        assert_eq!(measured.covered, without_counter.covered);
        assert_eq!(measured.gaps, without_counter.gaps);
        assert_eq!(measured.spans.len(), 1, "counter creates no interval");
    }

    #[test]
    fn upload_admission_diagnosis_refuses_ambiguous_or_invalid_counters() {
        let mut base = record(100, json!([["ReUpload", 0, 90, false]]));
        base["args"]["stages"] = json!({"input_upload_us": 90});
        base["args"]["re"] = json!({"iu_sem_wait_us": 80});
        assert_eq!(
            Activity::from_record(&base).upload_admission(&base),
            Some((80, 90))
        );
        let mut cases = Vec::new();
        for invalid in [Value::Null, json!(-1), json!(91), json!("80")] {
            let mut rec = base.clone();
            rec["args"]["re"]["iu_sem_wait_us"] = invalid;
            cases.push(rec);
        }
        for markers in [json!(["iu_sem_wait_us"]), json!(false), json!([7])] {
            let mut rec = base.clone();
            rec["args"]["re"]["invalid_counter_fields"] = markers;
            cases.push(rec);
        }
        for spans in [
            json!([["ReUpload", 0, 90, true]]),
            json!([["ReUpload", 0, 90, false], ["ReUpload", 0, 0, false]]),
            json!([["ReUpload", -1, 91, false]]),
            json!([["ReUpload", 0, 89, false]]),
        ] {
            let mut rec = base.clone();
            rec["args"]["activity"]["spans"] = spans;
            cases.push(rec);
        }
        let mut missing = base.clone();
        missing["args"].as_object_mut().unwrap().remove("activity");
        cases.push(missing);
        for rec in cases {
            let activity = Activity::from_record(&rec);
            assert_eq!(activity.upload_admission(&rec), None, "{rec}");
            assert!(
                !activity.diagnosis.contains("upload admission dominates"),
                "{rec}"
            );
        }
    }

    #[test]
    fn output_finalization_and_cas_metadata_have_specific_non_download_labels() {
        let rec = record(
            100,
            json!([
                ["Local.PrepareOutputDirs", 0, 5, false],
                ["CasArtifact.TreeMetadata", 5, 15, false],
                ["CasArtifact.DeclareOutputs", 20, 5, false],
                ["Local.FinalizeOutputs", 25, 75, true],
            ]),
        );
        let a = Activity::from_record(&rec);
        assert_eq!(a.covered_us, Some(100));
        assert_eq!(a.uncovered_us, Some(0));
        assert_eq!(a.spans[0].label, "local output-directory preparation");
        assert_eq!(a.spans[1].label, "CAS artifact tree metadata");
        assert_eq!(a.spans[1].lane, Lane::Client);
        assert_eq!(a.spans[3].label, "local output finalization");
        assert_eq!(a.spans[3].lane, Lane::Local);
        assert!(a.spans[3].cancelled);
    }

    #[test]
    fn older_cache_stage_projection_gets_a_hint_not_invented_counters() {
        let mut rec = record(
            100,
            json!([["Prepare", 0, 5, false], ["CacheHit", 20, 60, false]]),
        );
        rec["args"]["stages"] = json!({"cache_query_us": 15});
        let old = Activity::from_record(&rec);
        assert_eq!(old.spans[0].label, "action request preparation");
        assert_eq!(old.spans[1].label, "cache-hit result handling");
        assert!(
            old.explanations
                .iter()
                .any(|line| line.contains("omits their named counters"))
        );
        assert_eq!(
            crate::detail::stages_of(&rec["args"])
                .unwrap()
                .iter()
                .sum::<i64>(),
            15
        );
        rec["args"]["stages"]["prepare_us"] = 5.into();
        rec["args"]["stages"]["cache_hit_us"] = 60.into();
        let current = Activity::from_record(&rec);
        assert!(
            !current
                .explanations
                .iter()
                .any(|line| line.contains("omits their named counters"))
        );
        assert_eq!(old.covered, current.covered);
    }

    #[test]
    fn rust_fallback_keeps_canceled_remote_state_overlapping_local_work() {
        // Exact observation tuples from the Rust fixture projected from the
        // 2026-09-10 isolated build, action span 18898334. This selected list
        // intentionally omits minor spans; do not assert full-capture coverage.
        let rec = record(
            582_589_770,
            json!([
                ["Prepare", 1109, 15, false],
                ["CacheQuery", 1131, 96313, false],
                ["ReUpload", 97473, 518509, false],
                ["ReSubmit", 616017, 2414923, false],
                ["Re.Queue", 3030953, 118342, false],
                ["Re.WorkerDownload", 3149358, 579439598, true],
                ["Local.MaterializeInputs", 551891486, 11200744, false],
                ["Local.PrepareOutputs", 563092354, 9495270, false],
                ["Local.Execute", 572587641, 2809091, false]
            ]),
        );
        let a = Activity::from_record(&rec);
        let remote = a
            .spans
            .iter()
            .find(|s| s.name == "Re.WorkerDownload")
            .unwrap();
        let local = a.spans.iter().find(|s| s.name == "Local.Execute").unwrap();
        assert_eq!(remote.duration_us(), 579_439_598);
        assert_eq!(remote.status_label(), "canceled");
        assert_eq!(local.status_label(), "ended");
        assert_eq!(local.lane, Lane::Local);
        assert!(remote.start_us < local.start_us && remote.end_us > local.end_us);
        assert!(a.covered_us.unwrap() < a.spans.iter().map(ActivitySpan::duration_us).sum::<i64>());
        assert_eq!(a.covered_us.unwrap() + a.uncovered_us.unwrap(), 582_589_770);
        assert!(a.diagnosis.contains("canceled") && a.diagnosis.contains("not established"));
        for value in ["579.44s", "582.59s", "23.51s"] {
            assert!(a.diagnosis.contains(value), "{}", a.diagnosis);
        }
        assert!(!a.diagnosis.contains("memory pressure"));
    }

    #[test]
    fn cpp_repeated_download_intervals_are_not_labeled_worker_attempts() {
        // Exact intervals from action span 14788821. Only the state evidence
        // selected by the ingestion regression fixture is supplied here.
        let mut rec = record(
            1_622_235_649,
            json!([
                ["Re.Queue", 36140238, 94947, false],
                ["Re.WorkerDownload", 36236107, 1577896801, false],
                ["Re.Queue", 1614132913, 11788, false],
                ["Re.WorkerDownload", 1614144765, 850932, false],
                ["Re.WorkerUpload", 1621027080, 1208061, false]
            ]),
        );
        rec["args"]["stages"] = json!({"materialize_us": 850809});
        let a = Activity::from_record(&rec);
        let downloads: Vec<_> = a
            .spans
            .iter()
            .filter(|s| s.name == "Re.WorkerDownload")
            .collect();
        assert_eq!(downloads.len(), 2);
        assert_eq!(
            downloads.iter().map(|s| s.duration_us()).sum::<i64>(),
            1_578_747_733
        );
        assert!(downloads.iter().all(|s| s.status_label() == "ended"));
        assert!(a.diagnosis.contains("remote input preparation state"));
        assert!(a.diagnosis.contains("1578.75s of 1622.24s"));
        assert!(a.highlights.iter().any(|s| s.contains("0.85s")));
        assert!(
            a.explanations
                .iter()
                .any(|s| s.contains("Multiple intervals do not"))
        );
        assert!(a.spans.iter().all(|s| !s.label.contains("attempt")));
    }

    #[test]
    fn patch_op_420_second_preparation_counter_does_not_invent_a_timeline() {
        // Exact saved selected row for patch_op_data.cpp (non-PIC), digest
        // 5bbb99825c682890654433184e92aa42974e25a674c06d6a547a2370f2729635:148.
        // The CAS follow-up is separate evidence, not baked into the diagnosis.
        let rec = json!({"ts": 1789078425312266i64, "dur": 433429207,
            "args": {"kind": "remote", "cfg": "cfg:opt-linux-x86_64-fbcode-platform010-clang21-no-san-opt-by-default#04f4b80167fa12a7",
            "digest": "5bbb99825c682890654433184e92aa42974e25a674c06d6a547a2370f2729635:148",
            "bytes_in": 155665732, "stages": {"materialize_us": 420386166},
            "re": {"input_fetch_us": 420386166, "worker_download_wait_us": 420386086}
        }});
        let a = Activity::from_record(&rec);
        assert_eq!(a.evidence, Evidence::Missing);
        assert!(a.spans.is_empty() && a.covered.is_empty() && a.gaps.is_empty());
        assert_eq!(a.covered_us, None);
        assert!(a.diagnosis.contains("input preparation duration dominates"));
        assert!(a.diagnosis.contains("do not measure network bandwidth"));
        assert!(a.diagnosis.contains("420.39s of 433.43s"));
        assert!(
            a.highlights
                .iter()
                .any(|s| s.contains("not network bandwidth"))
        );
        assert!(
            a.explanations
                .iter()
                .any(|s| s.contains("exact worker-execution CAS diagnostics"))
        );
        assert!(
            !a.diagnosis.contains("296"),
            "external dedup evidence is not a client observation"
        );
    }

    #[test]
    fn client_scaffolding_is_not_labeled_as_physical_remote_work() {
        let a = Activity::from_record(&record(
            100,
            json!([
                ["Re.Download", 0, 5, false],
                ["Re.MaterializeFailedInputs", 5, 5, false],
                ["Re.BeforeActionExecution.action_digest", 10, 5, false],
                ["Re.AfterActionExecution.action_digest", 15, 5, false],
                ["Re.Unknown.future_state", 20, 5, false],
                ["Re.Queue", 25, 5, false]
            ]),
        ));
        assert!(a.spans[..5].iter().all(|s| s.lane == Lane::Client));
        assert_eq!(a.spans[5].lane, Lane::Remote);
        assert_eq!(a.spans[4].label, "Re.Unknown.future_state");
    }

    #[test]
    fn reused_worker_counters_are_not_promoted_but_local_observations_survive() {
        let mut rec = record(100, json!([["Local.MaterializeInputs", 10, 80, false]]));
        rec["args"]["execution_metadata"] = json!("reused");
        rec["args"]["stages"] = json!({"materialize_us": 10000, "execute_us": 9999});
        let a = Activity::from_record(&rec);
        assert_eq!(a.spans[0].duration_us(), 80);
        assert!(a.diagnosis.contains("local input preparation"));
        assert!(
            !a.highlights
                .iter()
                .any(|s| s.contains("Reported current input preparation")
                    || s.contains("Recorded current execution"))
        );
    }

    #[test]
    fn union_and_gaps_are_not_stage_counter_arithmetic() {
        let mut rec = record(
            100,
            json!([
                ["Re.WorkerDownload", 10, 70, false],
                ["Local.Execute", 30, 60, false],
                ["Prepare", 10, 70, false]
            ]),
        );
        rec["args"]["stages"] = json!({"materialize_us": 70, "execute_us": 60, "daemon_us": 2});
        let a = Activity::from_record(&rec);
        assert_eq!(
            a.covered,
            vec![Interval {
                start_us: 10,
                end_us: 90
            }]
        );
        assert_eq!(a.covered_us, Some(80));
        assert_eq!(a.uncovered_us, Some(20));
        assert_eq!(
            a.gaps,
            vec![
                Interval {
                    start_us: 0,
                    end_us: 10
                },
                Interval {
                    start_us: 90,
                    end_us: 100
                }
            ]
        );
        assert!(
            a.diagnostics_text(&rec, None)
                .contains("stage-counter residual (us): 0")
        );
    }

    #[test]
    fn old_duration_only_capture_does_not_invent_intervals() {
        let rec = json!({"ts": 0, "dur": 100, "args": {
            "re": {"worker_download_wait_us": 99, "worker_download_cancelled_wait_us": 90},
            "stages": {"materialize_us": 99, "execute_us": 1}
        }});
        let a = Activity::from_record(&rec);
        assert_eq!(a.evidence, Evidence::Missing);
        assert!(a.spans.is_empty() && a.gaps.is_empty());
        assert_eq!((a.covered_us, a.uncovered_us), (None, None));
        assert!(!a.has_complete_evidence());
    }

    #[test]
    fn legacy_explicit_cancellation_is_only_limited_positioned_evidence() {
        let rec = json!({"ts": 1000, "dur": 100, "args": {"re": {"cancelled_stages": [
            {"name": "Re.WorkerDownload", "ts": 1020, "dur": 60}
        ]}}});
        let a = Activity::from_record(&rec);
        assert_eq!(a.evidence, Evidence::LegacyLimited);
        assert_eq!(a.spans[0].start_us, 20);
        assert_eq!(a.covered_us, Some(60));
        assert_eq!(a.uncovered_us, None);
        assert!(a.gaps.is_empty());
    }

    #[test]
    fn measured_zero_and_empty_v1_are_distinct_from_missing() {
        let a = Activity::from_record(&record(100, json!([])));
        assert_eq!(a.evidence, Evidence::Current);
        assert_eq!((a.covered_us, a.uncovered_us), (Some(0), Some(100)));
        assert!(
            a.explanations
                .iter()
                .any(|s| s.contains("not evidence that nothing happened"))
        );
        let a = Activity::from_record(&record(0, json!([["Re.Queue", 0, 0, false]])));
        assert_eq!(a.spans.len(), 1);
        assert_eq!(a.spans[0].duration_us(), 0);
        assert_eq!((a.covered_us, a.uncovered_us), (Some(0), Some(0)));
        assert!(a.gaps.is_empty());
    }

    #[test]
    fn clipping_keeps_real_overlap_not_fake_outside_markers() {
        let rec = record(
            100,
            json!([
                ["Prepare", -20, 40, false],
                ["Local.Execute", 70, 60, false],
                ["Re.Queue", 150, 10, false],
                ["CacheQuery", -10, 0, false]
            ]),
        );
        let a = Activity::from_record(&rec);
        assert_eq!(a.clipped_spans, 2);
        assert_eq!(a.rejected_spans, 2);
        assert_eq!(a.spans.len(), 2);
        assert_eq!(a.covered_us, Some(50));
        assert!(!a.has_complete_evidence());
        assert_eq!(a.uncovered_us, None);
        assert!(a.gaps.is_empty());
    }

    #[test]
    fn invalid_fields_and_overflow_never_establish_full_coverage() {
        let rec = record(
            100,
            json!([
                ["Local.Execute", 10, 20, false],
                ["Re.Queue", 0, -1, false],
                ["Re.Queue", i64::MAX, 1, false],
                ["Re.Queue", 1.5, 10, false],
                ["Re.Queue", 0, 10, "false"],
                ["Re.Queue\nignore diagnostics", 0, 10, false],
                ["Re.Queue", 0, 10, false, "extra"],
                ["Re.Queue", u64::MAX, 0, false]
            ]),
        );
        let a = Activity::from_record(&rec);
        assert_eq!(a.evidence, Evidence::Invalid);
        assert_eq!(a.rejected_spans, 7);
        assert_eq!(a.covered_us, Some(20));
        assert_eq!(a.uncovered_us, None);
        assert!(a.gaps.is_empty());
        let bad_legacy = json!({"ts": i64::MIN, "dur": 100, "args": {"re": {"cancelled_stages": [{"name": "Re.Queue", "ts": i64::MAX, "dur": 1}]}}});
        assert_eq!(
            Activity::from_record(&bad_legacy).evidence,
            Evidence::Invalid
        );
        let no_duration = json!({"args": {"activity": {"version": 1, "spans": []}}});
        assert_eq!(
            Activity::from_record(&no_duration).evidence,
            Evidence::Invalid
        );
    }

    #[test]
    fn schema_errors_and_rejections_do_not_fall_back_to_legacy() {
        for payload in [
            json!(null),
            json!({}),
            json!({"version": 1}),
            json!({"version": 1, "spans": [], "rejected": -1}),
        ] {
            let rec = json!({"ts": 0, "dur": 100, "args": {"activity": payload}});
            let a = Activity::from_record(&rec);
            assert_eq!(a.evidence, Evidence::Invalid);
            assert_eq!(a.uncovered_us, None);
        }
        let rec = json!({"ts": 0, "dur": 100, "args": {"activity": {"version": 2, "spans": []}, "re": {"cancelled_stages": [{"name": "Re.Queue", "ts": 0, "dur": 100}]}}});
        let a = Activity::from_record(&rec);
        assert_eq!(a.evidence, Evidence::Unsupported);
        assert!(a.spans.is_empty());
        let mut rec = record(100, json!([["Re.Queue", 0, 100, false]]));
        rec["args"]["activity"]["rejected"] = json!(1);
        let a = Activity::from_record(&rec);
        assert_eq!(a.evidence, Evidence::Invalid);
        assert_eq!(a.covered_us, Some(100));
        assert_eq!(a.uncovered_us, None);
    }

    #[test]
    fn retention_is_bounded_and_invalidates_gap_claims() {
        let spans = vec![json!(["Re.Queue", 0, 1, false]); MAX_ACTIVITY_SPANS + 2];
        let a = Activity::from_record(&record(100, json!(spans)));
        assert_eq!(a.spans.len(), MAX_ACTIVITY_SPANS);
        assert_eq!(a.rejected_spans, 2);
        assert_eq!(a.evidence, Evidence::Invalid);
        assert_eq!(a.uncovered_us, None);
        assert_eq!(a.covered_us, Some(1));
    }

    #[test]
    fn diagnostic_copy_keeps_exact_spans_identity_and_unknown_causes() {
        let mut rec = record(
            100,
            json!([
                ["Re.WorkerDownload", 10, 70, false],
                ["Local.Execute", 50, 20, true]
            ]),
        );
        rec["args"]["run"] = json!("run\nnot instructions");
        rec["args"]["cfg"] = json!("cfg-ARM");
        rec["args"]["digest"] = json!("abc:148");
        rec["args"]["span_id"] = json!(u64::MAX);
        let a = Activity::from_record(&rec);
        let text = a.diagnostics_text(&rec, Some("session-123"));
        for s in [
            "cfg-ARM",
            "abc:148",
            "18446744073709551615",
            "session-123",
            "run\\nnot instructions",
            "start=10 end=80 duration=70 | ended",
            "start=50 end=70 duration=20 | canceled",
            "[1000000, 1000100]",
            "Uncovered gap | start=80 end=100 duration=20",
            "no query has been run",
            "not physical worker service",
        ] {
            assert!(text.contains(s), "missing {s}: {text}");
        }
        assert!(!text.contains("run\nnot instructions"));
        assert!(text.contains("not establish command success"));
    }
}
